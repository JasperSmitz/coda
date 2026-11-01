//! Library scan against generated directory trees.

mod common;

use std::collections::BTreeSet;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

use coda_player::library::m3u::PlaylistFile;
use coda_player::library::scan::{self, ScanEvent};
use coda_player::library::tree::LibraryTree;
use coda_player::model::{MAX_METADATA_CHARS, Track};
use lofty::config::WriteOptions;
use lofty::prelude::{Accessor, TagExt};
use lofty::tag::{ItemKey, Tag, TagType};
use tokio::sync::mpsc;

struct Outcome {
    batches: Vec<Vec<Track>>,
    playlists: Vec<PlaylistFile>,
    finished: Option<(usize, usize, usize)>,
}

impl Outcome {
    fn tracks(&self) -> Vec<&Track> {
        self.batches.iter().flatten().collect()
    }

    fn relative_paths(&self, root: &Path) -> BTreeSet<String> {
        self.tracks()
            .iter()
            .map(|t| {
                t.local_path()
                    .unwrap()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }
}

fn run_scan_with(root: &Path, batch_size: usize, interval: Duration) -> Outcome {
    let (tx, mut rx) = mpsc::channel(1024);
    scan::scan_with(root, &tx, batch_size, interval);
    drop(tx);
    let mut outcome = Outcome {
        batches: Vec::new(),
        playlists: Vec::new(),
        finished: None,
    };
    while let Some(event) = rx.blocking_recv() {
        match event {
            ScanEvent::Batch(batch) => {
                assert!(outcome.finished.is_none(), "batch after Finished");
                assert!(outcome.playlists.is_empty(), "batch after a playlist");
                outcome.batches.push(batch);
            }
            ScanEvent::Playlist(file) => {
                assert!(outcome.finished.is_none(), "playlist after Finished");
                outcome.playlists.push(file);
            }
            ScanEvent::Finished {
                tracks,
                skipped,
                errors,
            } => outcome.finished = Some((tracks, skipped, errors)),
        }
    }
    outcome
}

fn run_scan(root: &Path) -> Outcome {
    run_scan_with(root, scan::BATCH_SIZE, scan::BATCH_INTERVAL)
}

/// A canonical temp root, as the app would pass after config validation.
fn root() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(dir.path()).unwrap();
    (dir, path)
}

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"not really audio").unwrap();
}

#[test]
fn nesting_extension_filter_and_hidden_skip() {
    let (_guard, root) = root();
    touch(&root.join("a/b/deep.flac"));
    touch(&root.join("a/upper.MP3"));
    touch(&root.join("top.Opus"));
    for ext in [
        "ogg", "oga", "m4a", "aac", "wav", "aif", "aiff", "wv", "ape", "mpc",
    ] {
        touch(&root.join(format!("exts/x.{ext}")));
    }
    touch(&root.join("notes.txt"));
    touch(&root.join("a/cover.jpg"));
    touch(&root.join("noext"));
    touch(&root.join(".hidden/secret.mp3"));
    touch(&root.join("a/.dotfile.mp3"));
    std::fs::create_dir_all(root.join("emptydir")).unwrap();

    let outcome = run_scan(&root);
    let mut expected: BTreeSet<String> = ["a/b/deep.flac", "a/upper.MP3", "top.Opus"]
        .into_iter()
        .map(String::from)
        .collect();
    for ext in [
        "ogg", "oga", "m4a", "aac", "wav", "aif", "aiff", "wv", "ape", "mpc",
    ] {
        expected.insert(format!("exts/x.{ext}"));
    }
    assert_eq!(outcome.relative_paths(&root), expected);
    assert_eq!(outcome.finished, Some((expected.len(), 0, 0)));
    for track in outcome.tracks() {
        assert!(track.local_path().unwrap().is_absolute());
    }

    // Tree: directories without audio are omitted.
    let mut tree = LibraryTree::new(root.clone());
    tree.extend(outcome.tracks().into_iter().cloned());
    let dirs: Vec<&str> = tree.root().dirs().iter().map(|d| d.name()).collect();
    assert_eq!(dirs, ["a", "exts"]);
}

#[test]
fn untagged_and_unparseable_files_fall_back_to_file_name() {
    let (_guard, root) = root();
    touch(&root.join("Garbage Song.flac"));
    common::write_wav(&root.join("Plain Wav.wav"), 1000);
    let outcome = run_scan(&root);
    let mut titles: Vec<_> = outcome.tracks().iter().map(|t| t.title.clone()).collect();
    titles.sort();
    assert_eq!(titles, ["Garbage Song", "Plain Wav"]);
    let wav = outcome
        .tracks()
        .into_iter()
        .find(|t| t.title == "Plain Wav")
        .unwrap();
    assert_eq!(wav.artist, None);
    let duration = wav.duration.expect("wav duration");
    assert!(duration.abs_diff(Duration::from_secs(1)) < Duration::from_millis(50));
}

#[test]
fn tags_written_with_lofty_are_read_back() {
    let (_guard, root) = root();
    let path = root.join("tagged.wav");
    common::write_wav(&path, 500);
    let mut tag = Tag::new(TagType::Id3v2);
    tag.set_title("The Title".into());
    tag.set_artist("The Artist".into());
    tag.set_album("The Album".into());
    tag.insert_text(ItemKey::AlbumArtist, "The Album Artist".into());
    tag.set_track(7);
    tag.set_disk(2);
    tag.save_to_path(&path, WriteOptions::default()).unwrap();

    let outcome = run_scan(&root);
    let tracks = outcome.tracks();
    assert_eq!(tracks.len(), 1);
    let t = tracks[0];
    assert_eq!(t.title, "The Title");
    assert_eq!(t.artist.as_deref(), Some("The Artist"));
    assert_eq!(t.album.as_deref(), Some("The Album"));
    assert_eq!(t.album_artist.as_deref(), Some("The Album Artist"));
    assert_eq!(t.track_no, Some(7));
    assert_eq!(t.disc_no, Some(2));
    assert_eq!(t.display_name(), "The Artist - The Title");
}

#[test]
fn oversized_tags_are_capped() {
    let (_guard, root) = root();
    let path = root.join("huge.wav");
    common::write_wav(&path, 100);
    let mut tag = Tag::new(TagType::Id3v2);
    tag.set_title(format!("a{}", "\u{301}".repeat(50_000)));
    tag.set_artist("雨".repeat(5_000));
    tag.save_to_path(&path, WriteOptions::default()).unwrap();

    let outcome = run_scan(&root);
    let t = outcome.tracks()[0];
    assert_eq!(t.title.chars().count(), MAX_METADATA_CHARS);
    assert!(t.title.starts_with('a'));
    assert_eq!(
        t.artist.as_deref(),
        Some("雨".repeat(MAX_METADATA_CHARS).as_str())
    );
}

#[test]
fn directory_symlinks_are_not_followed_but_file_symlinks_are() {
    let (_guard, root) = root();
    let (_outside_guard, outside) = self::root();
    touch(&outside.join("album/out.mp3"));
    common::write_wav(&outside.join("real.wav"), 100);
    touch(&root.join("in/own.mp3"));
    symlink(outside.join("album"), root.join("linked-dir")).unwrap();
    symlink(root.join("in"), root.join("loop")).unwrap();
    symlink(outside.join("real.wav"), root.join("in/link.wav")).unwrap();
    symlink(outside.join("missing.mp3"), root.join("in/dangling.mp3")).unwrap();
    symlink(outside.join("album"), root.join("dir-named.mp3")).unwrap();

    let outcome = run_scan(&root);
    let expected: BTreeSet<String> = ["in/link.wav", "in/own.mp3"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(outcome.relative_paths(&root), expected);
    // Dangling symlink and the directory symlink with an audio name.
    assert_eq!(outcome.finished, Some((2, 2, 0)));
}

#[test]
fn batches_are_incremental_by_size() {
    let (_guard, root) = root();
    for i in 0..450 {
        touch(&root.join(format!("d{}/t{i:03}.mp3", i % 3)));
    }
    let outcome = run_scan_with(&root, 200, Duration::from_secs(3600));
    let sizes: Vec<usize> = outcome.batches.iter().map(Vec::len).collect();
    assert_eq!(sizes, [200, 200, 50]);
    assert_eq!(outcome.finished, Some((450, 0, 0)));
}

#[test]
fn batches_are_incremental_by_time() {
    let (_guard, root) = root();
    for i in 0..5 {
        touch(&root.join(format!("t{i}.mp3")));
    }
    let outcome = run_scan_with(&root, 1000, Duration::ZERO);
    let sizes: Vec<usize> = outcome.batches.iter().map(Vec::len).collect();
    assert_eq!(sizes, [1, 1, 1, 1, 1]);
}

#[test]
fn empty_directory_finishes_with_no_tracks() {
    let (_guard, root) = root();
    let outcome = run_scan(&root);
    assert!(outcome.batches.is_empty());
    assert_eq!(outcome.finished, Some((0, 0, 0)));
}

#[test]
fn stops_early_when_receiver_is_dropped() {
    let (_guard, root) = root();
    for i in 0..500 {
        touch(&root.join(format!("t{i:03}.mp3")));
    }
    let (tx, mut rx) = mpsc::channel(1);
    let worker = std::thread::spawn(move || {
        scan::scan_with(&root, &tx, 1, Duration::from_secs(3600));
    });
    match rx.blocking_recv() {
        Some(ScanEvent::Batch(batch)) => assert_eq!(batch.len(), 1),
        other => panic!("expected a batch, got {other:?}"),
    }
    drop(rx);
    // The scan thread must notice and return instead of walking all files.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !worker.is_finished() {
        assert!(std::time::Instant::now() < deadline, "scan did not stop");
        std::thread::sleep(Duration::from_millis(5));
    }
    worker.join().unwrap();
}

#[test]
fn permission_errors_are_counted_and_scan_continues() {
    let (_guard, root) = root();
    touch(&root.join("ok/a.mp3"));
    touch(&root.join("locked/b.mp3"));
    let locked = root.join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let outcome = run_scan(&root);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    if std::fs::read_dir("/root").is_ok() {
        return; // Running as root: permissions are not enforced.
    }
    assert_eq!(
        outcome
            .relative_paths(&root)
            .into_iter()
            .collect::<Vec<_>>(),
        ["ok/a.mp3"]
    );
    assert_eq!(outcome.finished, Some((1, 0, 1)));
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_paths_are_skipped() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let (_guard, root) = root();
    touch(&root.join("good.mp3"));
    touch(&root.join(OsStr::from_bytes(b"bad\xff.mp3")));
    let outcome = run_scan(&root);
    assert_eq!(outcome.tracks().len(), 1);
    assert_eq!(outcome.finished, Some((1, 1, 0)));
}

#[tokio::test]
async fn spawn_scan_streams_to_finished() {
    let (_guard, root) = root();
    touch(&root.join("x/a.mp3"));
    touch(&root.join("b.flac"));
    let mut rx = scan::spawn_scan(root.clone());
    let mut tracks = 0;
    loop {
        match rx.recv().await.expect("scan ends with Finished") {
            ScanEvent::Batch(batch) => tracks += batch.len(),
            ScanEvent::Playlist(_) => {}
            ScanEvent::Finished { tracks: total, .. } => {
                assert_eq!(total, tracks);
                break;
            }
        }
    }
    assert_eq!(tracks, 2);
}

#[test]
fn playlists_follow_the_audio_rules_and_come_after_every_track() {
    let (_guard, root) = root();
    let (_outside_guard, outside) = self::root();
    touch(&root.join("Artist/Album/01.mp3"));
    touch(&root.join("Artist/Album/02.mp3"));
    std::fs::create_dir_all(root.join("lists")).unwrap();
    std::fs::write(
        root.join("lists/Road Trip.m3u8"),
        "#EXTM3U\n#EXTINF:1,x\n../Artist/Album/02.mp3\n../Artist/Album/01.mp3\nhttp://x/y\n",
    )
    .unwrap();
    std::fs::write(root.join("top.M3U"), "Artist/Album/01.mp3\r\n").unwrap();
    std::fs::write(root.join(".hidden.m3u"), "x.mp3\n").unwrap();
    std::fs::write(outside.join("linked.m3u"), "x.mp3\n").unwrap();
    std::fs::create_dir_all(outside.join("dir")).unwrap();
    std::fs::write(outside.join("dir/in-linked-dir.m3u"), "x.mp3\n").unwrap();
    // A file symlink to a playlist is read; a symlinked folder is not walked.
    symlink(outside.join("linked.m3u"), root.join("lists/via-link.m3u")).unwrap();
    symlink(outside.join("dir"), root.join("linked-dir")).unwrap();
    symlink(outside.join("missing.m3u"), root.join("dangling.m3u")).unwrap();

    let outcome = run_scan(&root);
    assert_eq!(
        outcome.finished,
        Some((2, 0, 0)),
        "playlists are not counted"
    );
    let mut names: Vec<&str> = outcome.playlists.iter().map(|p| p.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["Road Trip", "top", "via-link"]);
    let road = outcome
        .playlists
        .iter()
        .find(|p| p.name == "Road Trip")
        .unwrap();
    assert_eq!(
        road.entries,
        [
            root.join("Artist/Album/02.mp3"),
            root.join("Artist/Album/01.mp3")
        ]
    );
    assert_eq!(road.unresolved, 1);
    // Entries of the linked playlist resolve against the link's folder.
    let linked = outcome
        .playlists
        .iter()
        .find(|p| p.name == "via-link")
        .unwrap();
    assert_eq!(linked.entries, [root.join("lists/x.mp3")]);
}

#[test]
fn a_playlist_entry_through_a_symlinked_folder_is_missing() {
    use coda_player::library::Library;
    let (_guard, root) = root();
    let (_outside_guard, outside) = self::root();
    touch(&root.join("real/x.mp3"));
    touch(&outside.join("x.mp3"));
    symlink(outside.clone(), root.join("linked-dir")).unwrap();
    // The folder is there on disk, and so is the file through it.
    assert!(root.join("linked-dir/x.mp3").is_file());
    std::fs::write(
        root.join("list.m3u"),
        "linked-dir/x.mp3\nlinked-dir/../real/x.mp3\nreal/x.mp3\n",
    )
    .unwrap();
    let outcome = run_scan(&root);
    let mut library = Library::new(root.clone());
    library.extend(outcome.tracks().into_iter().cloned());
    let file = outcome.playlists.into_iter().next().unwrap();
    let id = library.add_playlist(file).unwrap();
    let playlist = library.playlist(id);
    let paths: Vec<_> = playlist
        .tracks
        .iter()
        .map(|t| library.track(*t).local_path().unwrap().to_owned())
        .collect();
    assert_eq!(paths, [root.join("real/x.mp3")]);
    assert_eq!(playlist.missing, 2);
}
