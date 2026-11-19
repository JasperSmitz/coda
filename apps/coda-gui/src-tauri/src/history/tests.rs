//! Play history: recording rules, bounds, persistence (atomic, FIFO-safe,
//! corrupt files set aside), local keys and their resolution, stops, the
//! moods' cache and picks.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use coda_core::library::m3u::PlaylistFile;
use coda_core::model::{Track, TrackMetadata};

use super::*;
use crate::browse::BrowseHandle;

fn server() -> String {
    server_id_of("https://music.example/", "listener")
}

fn nd() -> Scope {
    Scope::Navidrome(server())
}

fn remote(kind: ContextKind, id: &str) -> Context {
    Context {
        source: SourceKind::Navidrome,
        server: Some(server()),
        kind,
        id: id.into(),
        name: format!("Name {id}"),
        subtitle: Some("Band".into()),
        cover_art: Some(format!("cover-{id}")),
    }
}

fn stop(
    kind: ContextKind,
    id: &str,
    track: &str,
    index: usize,
    total: usize,
    position_ms: u64,
    at: u64,
) -> Stop {
    Stop {
        source: SourceKind::Navidrome,
        server: Some(server()),
        kind,
        id: id.into(),
        track: track.into(),
        index,
        total,
        position_ms,
        at,
    }
}

fn mood(genre: &str, albums: &[&str]) -> Mood {
    Mood {
        genre: genre.into(),
        album_count: None,
        albums: albums
            .iter()
            .map(|id| MoodAlbum {
                id: (*id).into(),
                name: format!("Album {id}"),
                artist: Some("Band".into()),
                artist_id: None,
                cover_art: None,
                duration_ms: None,
            })
            .collect(),
    }
}

fn local_track(rel: &str, artist: &str, album: &str) -> Track {
    Track::local(
        PathBuf::from(format!("/m/{rel}")),
        TrackMetadata {
            title: Some(rel.into()),
            artist: Some(artist.into()),
            album: Some(album.into()),
            duration: Some(Duration::from_secs(60)),
            ..TrackMetadata::default()
        },
    )
}

fn library(albums: usize) -> Library {
    let mut library = Library::new("/m".into());
    library.extend((0..albums).map(|i| {
        local_track(
            &format!("Band {}/Album {i}/1.flac", i % 3),
            &format!("Band {}", i % 3),
            &format!("Album {i}"),
        )
    }));
    library.add_playlist(PlaylistFile {
        path: "/m/mix.m3u".into(),
        name: "mix".into(),
        entries: vec!["/m/Band 0/Album 0/1.flac".into()],
        unresolved: 0,
        cut: false,
    });
    library
}

fn handle(library: Library) -> BrowseHandle {
    let browse = BrowseHandle::new(Arc::new(CoverService::new(None)));
    browse.replace_library(Some(library));
    browse
}

#[test]
fn plays_are_kept_newest_last_and_bounded() {
    let mut history = History::default();
    for i in 0..(MAX_PLAYS + 10) {
        history.record_play(remote(ContextKind::Album, &format!("a{i}")), i as u64);
    }
    assert_eq!(history.plays().count(), MAX_PLAYS);
    assert_eq!(history.plays().next().unwrap().context.id, "a10");
    assert!(history.revision() > 0);
}

#[test]
fn recent_is_distinct_newest_first_and_per_source() {
    let mut history = History::default();
    history.record_play(remote(ContextKind::Album, "a"), 1);
    history.record_play(remote(ContextKind::Playlist, "p"), 2);
    history.record_play(local_folder_context(&["Jazz".into()]), 3);
    history.record_play(remote(ContextKind::Album, "a"), 4);
    // The same id as another kind is another context.
    history.record_play(remote(ContextKind::Artist, "a"), 5);
    let recent: Vec<(ContextKind, String)> = history
        .recent(&nd(), 10)
        .into_iter()
        .map(|p| (p.context.kind, p.context.id))
        .collect();
    assert_eq!(
        recent,
        vec![
            (ContextKind::Artist, "a".into()),
            (ContextKind::Album, "a".into()),
            (ContextKind::Playlist, "p".into()),
        ]
    );
    assert_eq!(history.recent(&nd(), 1).len(), 1);
    let local = history.recent(&Scope::Local, 10);
    assert_eq!(local.len(), 1);
    assert_eq!(local[0].context.id, "Jazz");
}

#[test]
fn listens_count_once_per_context() {
    let mut history = History::default();
    let album = remote(ContextKind::Album, "al");
    let playlist = remote(ContextKind::Playlist, "pl");
    history.record_listen(vec![album.clone(), playlist.clone(), album.clone()], 10);
    history.record_listen(vec![album.clone()], 20);
    let key = album.key();
    assert_eq!(history.count(&key).unwrap().listens, 2);
    assert_eq!(history.count(&key).unwrap().last, 20);
    assert_eq!(history.count(&playlist.key()).unwrap().listens, 1);
    // The moods' album listens: albums only, by id.
    let albums = history.album_listens(&nd());
    assert_eq!(albums.len(), 1);
    assert_eq!(albums["al"].listens, 2);
    // A renamed playlist shows its latest name.
    let renamed = Context {
        name: "New name".into(),
        ..playlist.clone()
    };
    history.record_listen(vec![renamed], 30);
    assert_eq!(
        history.count(&playlist.key()).unwrap().context.name,
        "New name"
    );
    // Nothing usable: no change.
    let before = history.revision();
    history.record_listen(Vec::new(), 40);
    assert_eq!(history.revision(), before);
}

#[test]
fn counts_drop_the_least_recently_heard_beyond_the_bound() {
    let mut history = History::default();
    for i in 0..MAX_COUNTS {
        history.record_listen(
            vec![remote(ContextKind::Album, &format!("a{i}"))],
            i as u64 + 100,
        );
    }
    // a0 is heard again, so a1 is the oldest now.
    history.record_listen(vec![remote(ContextKind::Album, "a0")], 1_000_000);
    history.record_listen(vec![remote(ContextKind::Album, "new")], 1_000_001);
    assert_eq!(history.counts_len(), MAX_COUNTS);
    assert!(
        history
            .count(&remote(ContextKind::Album, "a0").key())
            .is_some()
    );
    assert!(
        history
            .count(&remote(ContextKind::Album, "a1").key())
            .is_none()
    );
    assert!(
        history
            .count(&remote(ContextKind::Album, "new").key())
            .is_some()
    );
}

#[test]
fn untrusted_text_is_cleaned_and_bad_ids_are_dropped() {
    let mut history = History::default();
    history.record_play(
        Context {
            name: "  Bad\u{7}\nname  ".into(),
            subtitle: Some("   ".into()),
            ..remote(ContextKind::Album, "a")
        },
        1,
    );
    let played = history.recent(&nd(), 1).remove(0);
    assert_eq!(played.context.name, "Bad  name");
    assert_eq!(played.context.subtitle, None);
    let long = "x".repeat(300);
    history.record_play(
        Context {
            name: long.clone(),
            ..remote(ContextKind::Album, "b")
        },
        2,
    );
    assert_eq!(
        history.recent(&nd(), 1)[0].context.name.chars().count(),
        256
    );
    let before = history.revision();
    history.record_play(remote(ContextKind::Album, ""), 3);
    history.record_play(remote(ContextKind::Album, &"y".repeat(5000)), 3);
    assert_eq!(history.revision(), before, "unusable ids are not recorded");
    // A local context never keeps a cover id.
    history.record_play(
        Context {
            source: SourceKind::Local,
            ..remote(ContextKind::Album, "k")
        },
        4,
    );
    assert_eq!(history.recent(&Scope::Local, 1)[0].context.cover_art, None);
}

#[test]
fn the_file_round_trips_and_holds_no_urls() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("coda-app").join("history.json");
    let mut history = History::default();
    history.record_play(remote(ContextKind::Playlist, "p"), 7);
    history.record_listen(vec![remote(ContextKind::Album, "a")], 8);
    history.set_album_genres(&remote(ContextKind::Album, "a").key(), vec!["Jazz".into()]);
    history.record_stop(stop(ContextKind::Playlist, "p", "s1", 1, 3, 42_000, 9));
    history.set_moods(&nd(), "2026-11-06".into(), vec![mood("Jazz", &["al-1"])]);
    save(&path, &history.to_file()).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("http") && !text.contains("coda://"));
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o077, 0, "private: {mode:o}");
    let loaded = load(Some(path.clone()));
    assert_eq!(loaded.path, Some(path));
    assert_eq!(loaded.history.to_file(), history.to_file());
    assert_eq!(loaded.history.moods(&nd(), "2026-11-06").unwrap().len(), 1);
    assert!(loaded.history.moods(&nd(), "2026-11-07").is_none());
    let key = remote(ContextKind::Playlist, "p").key();
    assert_eq!(loaded.history.stop(&key).unwrap().position_ms, 42_000);
    assert_eq!(
        loaded.history.heard_album_genres(&nd())[0].0,
        vec!["Jazz".to_owned()]
    );
}

#[test]
fn a_missing_file_is_an_empty_history_that_saves() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    let loaded = load(Some(path.clone()));
    assert_eq!(loaded.path, Some(path));
    assert_eq!(loaded.history.plays().count(), 0);
    assert!(load(None).path.is_none());
}

/// The file `load` set aside (`history.json.bad.<time>`), taken away.
fn take_aside(dir: &Path) -> Option<String> {
    let entry = std::fs::read_dir(dir).unwrap().flatten().find(|e| {
        e.file_name()
            .to_string_lossy()
            .starts_with("history.json.bad.")
    })?;
    let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
    std::fs::remove_file(entry.path()).unwrap();
    Some(text)
}

#[test]
fn a_corrupt_file_is_set_aside_and_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    for bad in ["not json", "{\"version\": 0}", "{\"plays\": 3}"] {
        std::fs::write(&path, bad).unwrap();
        let loaded = load(Some(path.clone()));
        assert_eq!(loaded.path.as_ref(), Some(&path), "{bad}");
        assert_eq!(loaded.history.plays().count(), 0);
        assert!(!path.exists(), "moved aside: {bad}");
        assert_eq!(take_aside(dir.path()).as_deref(), Some(bad));
    }
}

#[test]
fn a_newer_or_too_large_file_is_kept_and_not_saved_over() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    let newer = "{\"version\": 99, \"plays\": \"something new\"}";
    std::fs::write(&path, newer).unwrap();
    let loaded = load(Some(path.clone()));
    assert!(loaded.path.is_none(), "not saved this run");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
    assert!(take_aside(dir.path()).is_none());

    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_FILE_BYTES + 1).unwrap();
    let loaded = load(Some(path.clone()));
    assert!(loaded.path.is_none());
    assert!(path.exists());
    assert!(take_aside(dir.path()).is_none());
}

#[test]
fn a_saved_history_never_exceeds_what_load_accepts() {
    let mut file = HistoryFile {
        version: 1,
        ..HistoryFile::default()
    };
    for i in 0..200u64 {
        file.counts.push(Count {
            context: remote(ContextKind::Album, &format!("c{i}")),
            listens: 1,
            last: 1000 - i,
            genres: Vec::new(),
        });
        file.plays.push(Played {
            context: remote(ContextKind::Album, &format!("p{i}")),
            at: i,
        });
    }
    let whole = bounded_bytes(&file, u64::MAX).unwrap();
    let max = whole.len() as u64 / 3;
    let bytes = bounded_bytes(&file, max).unwrap();
    assert!(bytes.len() as u64 <= max);
    let saved: HistoryFile = serde_json::from_slice(&bytes).unwrap();
    // The most recent counts and the newest plays stay.
    assert!(!saved.counts.is_empty() && !saved.plays.is_empty());
    assert_eq!(saved.counts[0].context.id, "c0");
    assert_eq!(saved.plays.last().unwrap().context.id, "p199");
}

#[test]
fn a_fifo_is_never_opened_for_reading() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    // Would block forever on a plain open (no writer).
    let loaded = load(Some(path.clone()));
    assert_eq!(loaded.history.plays().count(), 0);
    assert!(std::fs::read_dir(dir.path()).unwrap().flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .starts_with("history.json.bad.")
    }));
    assert!(!path.exists());
}

#[test]
fn invalid_entries_in_a_file_are_skipped_and_bounds_applied() {
    let mut file = HistoryFile {
        version: 1,
        ..HistoryFile::default()
    };
    for i in 0..(MAX_PLAYS + 5) {
        file.plays.push(Played {
            context: remote(ContextKind::Album, &format!("a{i}")),
            at: i as u64,
        });
    }
    file.plays.push(Played {
        context: remote(ContextKind::Album, ""),
        at: 1,
    });
    file.counts.push(Count {
        context: remote(ContextKind::Album, "zero"),
        listens: 0,
        last: 1,
        genres: Vec::new(),
    });
    file.moods.push(MoodsDay {
        date: "yesterday".into(),
        source: SourceKind::Navidrome,
        server: Some(server()),
        moods: vec![mood("Jazz", &["a"])],
    });
    // A stop of a context no longer among the plays, and a bad one.
    file.stops
        .push(stop(ContextKind::Album, "a0", "s", 0, 1, 1, 1));
    file.stops
        .push(stop(ContextKind::Album, "a9", "s", 5, 2, 1, 1));
    file.stops
        .push(stop(ContextKind::Album, "a10", "s", 1, 2, 1, 1));
    // From an older file (or no known server): dropped.
    file.plays.push(Played {
        context: Context {
            server: None,
            ..remote(ContextKind::Album, "old")
        },
        at: 1,
    });
    let history = History::from_file(file);
    assert_eq!(history.plays().count(), MAX_PLAYS);
    assert_eq!(history.counts_len(), 0);
    assert!(history.to_file().moods.is_empty());
    let stops = history.to_file().stops;
    assert_eq!(stops.len(), 1, "{stops:?}");
    assert_eq!(stops[0].id, "a10");
}

#[test]
fn the_first_moods_of_a_day_stay_per_scope() {
    let mut history = History::default();
    let first = history.set_moods(
        &nd(),
        "2026-11-06".into(),
        vec![
            mood("Jazz", &["a", ""]),
            mood("", &["x"]),
            mood("Rock", &[]),
        ],
    );
    assert_eq!(
        first.len(),
        1,
        "bad ids, blank genres and empty moods dropped"
    );
    assert_eq!(first[0].albums.len(), 1);
    let again = history.set_moods(&nd(), "2026-11-06".into(), vec![mood("Pop", &["b"])]);
    assert_eq!(again[0].genre, "Jazz");
    let next_day = history.set_moods(&nd(), "2026-11-07".into(), vec![mood("Pop", &["c"])]);
    assert_eq!(next_day[0].genre, "Pop");
    assert!(
        history.moods(&nd(), "2026-11-06").is_none(),
        "one day per scope"
    );
    assert!(
        history
            .set_moods(&nd(), "bad".into(), vec![mood("Pop", &["d"])])
            .is_empty()
    );
    // Local and every server keep their own, at most a few scopes.
    assert!(history.moods(&Scope::Local, "2026-11-07").is_none());
    history.set_moods(
        &Scope::Local,
        "2026-11-07".into(),
        vec![mood("Folk", &["e"])],
    );
    assert_eq!(
        history.moods(&Scope::Local, "2026-11-07").unwrap()[0].genre,
        "Folk"
    );
    assert_eq!(history.moods(&nd(), "2026-11-07").unwrap()[0].genre, "Pop");
    for i in 0..5 {
        history.set_moods(
            &Scope::Navidrome(server_id_of(&format!("https://s{i}/"), "u")),
            "2026-11-07".into(),
            vec![mood("Pop", &["f"])],
        );
    }
    assert_eq!(history.to_file().moods.len(), MAX_MOOD_SCOPES);
    assert!(
        history.moods(&nd(), "2026-11-07").is_none(),
        "oldest dropped"
    );
    // Never more than today's and the other moods, three albums each.
    let many: Vec<Mood> = (0..9)
        .map(|i| mood(&format!("G{i}"), &["1", "2", "3", "4"]))
        .collect();
    let kept = history.set_moods(&Scope::Local, "2026-11-08".into(), many);
    assert_eq!(kept.len(), moods::MOODS_KEPT);
    assert!(kept.iter().all(|m| m.albums.len() == moods::MOOD_ALBUMS));
}

#[test]
fn stops_are_kept_for_played_contexts_only_and_bounded_with_them() {
    let mut history = History::default();
    // Not played: nothing kept.
    history.record_stop(stop(ContextKind::Album, "a", "s1", 0, 3, 5, 1));
    assert!(
        history
            .stop(&remote(ContextKind::Album, "a").key())
            .is_none()
    );
    history.record_play(remote(ContextKind::Album, "a"), 1);
    history.record_stop(stop(ContextKind::Album, "a", "s2", 1, 3, 5_000, 2));
    let key = remote(ContextKind::Album, "a").key();
    assert_eq!(history.stop(&key).unwrap().track, "s2");
    // The same stop again: no change.
    let before = history.revision();
    history.record_stop(stop(ContextKind::Album, "a", "s2", 1, 3, 5_000, 2));
    assert_eq!(history.revision(), before);
    // A later one replaces it.
    history.record_stop(stop(ContextKind::Album, "a", "s3", 2, 3, 0, 3));
    assert_eq!(history.stop(&key).unwrap().index, 2);
    // Unusable: a place beyond the tracks, a mix, no track.
    history.record_stop(stop(ContextKind::Album, "a", "s3", 3, 3, 0, 4));
    history.record_stop(stop(ContextKind::Album, "a", "", 0, 3, 0, 4));
    assert_eq!(history.stop(&key).unwrap().at, 3);
    // An absurd position is capped at a day.
    history.record_stop(stop(ContextKind::Album, "a", "s3", 2, 3, u64::MAX, 4));
    assert_eq!(history.stop(&key).unwrap().position_ms, 24 * 60 * 60 * 1000);
    history.record_play(remote(ContextKind::Mix, "m"), 4);
    history.record_stop(stop(ContextKind::Mix, "m", "s", 0, 3, 0, 4));
    assert!(history.stop(&remote(ContextKind::Mix, "m").key()).is_none());
    // Played to the end: no stop.
    history.clear_stop(&key);
    assert!(history.stop(&key).is_none());
    history.record_stop(stop(ContextKind::Album, "a", "s1", 0, 3, 0, 5));
    // Gone from the plays: its stop goes too.
    for i in 0..MAX_PLAYS {
        history.record_play(remote(ContextKind::Album, &format!("x{i}")), 10 + i as u64);
    }
    assert!(history.stop(&key).is_none());
}

#[test]
fn dates_are_checked() {
    assert!(valid_date("2026-01-31"));
    for bad in ["", "2026-1-31", "2026/01/31", "20260131xx", "2026-01-3a"] {
        assert!(!valid_date(bad), "{bad}");
    }
}

#[test]
fn local_contexts_resolve_by_stable_keys_across_generations() {
    let browse = handle(library(4));
    let contexts = browse.read_library(|view| {
        let view = view.unwrap();
        let tags = view.library.tags();
        let album = tags.albums(AlbumOrder::Name)[0];
        let artist = tags.artists()[0];
        vec![
            local_album_context(tags, album),
            local_artist_context(tags, artist),
            local_playlist_context(view.library, view.library.playlists()[0]),
            local_folder_context(&["Band 0".into()]),
            local_folder_context(&[]),
        ]
    });
    assert_eq!(contexts[0].id, "band 0\u{1f}album 0");
    assert_eq!(contexts[0].subtitle.as_deref(), Some("Band 0"));
    assert_eq!(contexts[2].id, "mix");
    assert_eq!(contexts[4].id, "/");
    // A rescan: new generation, new ids, same keys.
    browse.replace_library(Some(library(4)));
    let mut history = History::default();
    for (i, context) in contexts.iter().enumerate() {
        history.record_play(context.clone(), i as u64);
    }
    // Gone from the library: skipped.
    history.record_play(local_folder_context(&["Nope".into()]), 99);
    let rows = history.home_rows(&Scope::Local);
    let home =
        browse.read_library(|view| home_dto(rows, view.as_ref(), &HashMap::new(), browse.covers()));
    let generation = browse.library_generation();
    let shown: Vec<(ContextKind, String)> =
        home.recent.iter().map(|i| (i.kind, i.id.clone())).collect();
    assert_eq!(shown.len(), 5, "{shown:?}");
    assert_eq!(shown[0], (ContextKind::Folder, "/".into()));
    assert_eq!(home.recent[0].path, Some(Vec::new()));
    assert_eq!(home.recent[0].name, "Music");
    assert_eq!(home.recent[1].path, Some(vec!["Band 0".to_owned()]));
    assert_eq!(shown[2].0, ContextKind::Playlist);
    assert_eq!(shown[4].0, ContextKind::Album);
    for item in &home.recent[2..] {
        assert!(item.id.starts_with(&format!("{generation}:")), "{item:?}");
        assert!(item.cover_url.is_some(), "{item:?}");
    }
    assert_eq!(home.recent[4].name, "Album 0");
    assert_eq!(home.recent[4].subtitle.as_deref(), Some("Band 0"));
    assert!(home.recent[4].artist_id.is_some());
}

#[test]
fn navidrome_cards_use_the_stored_names_and_cover() {
    let covers = CoverService::new(None);
    let mut history = History::default();
    history.record_play(remote(ContextKind::Playlist, "p"), 1);
    history.record_listen(vec![remote(ContextKind::Playlist, "p")], 2);
    let home = home_dto(history.home_rows(&nd()), None, &HashMap::new(), &covers);
    assert_eq!(home.recent[0].name, "Name p");
    assert!(
        home.recent[0]
            .cover_url
            .as_deref()
            .unwrap()
            .starts_with("coda://")
    );
    assert_eq!(home.recent[0].played_at, 1);
    assert!(home.recent[0].stop.is_none());
    // Where it stopped: the song id and the place as queued; the latest
    // of the play and the stop is when it was played.
    history.record_stop(stop(ContextKind::Playlist, "p", "s2", 1, 4, 61_000, 9));
    let home = home_dto(history.home_rows(&nd()), None, &HashMap::new(), &covers);
    assert_eq!(home.recent[0].played_at, 9);
    assert_eq!(
        home.recent[0].stop,
        Some(StopDto {
            index: 1,
            total: 4,
            track_id: Some("s2".into()),
            position_ms: 61_000,
        })
    );
    // A mix only while it is saved, with its current name; artists
    // without a cover are left out.
    history.record_play(remote(ContextKind::Mix, "0123456789abcdef"), 10);
    history.record_play(
        Context {
            cover_art: None,
            ..remote(ContextKind::Artist, "ar")
        },
        11,
    );
    let shown = |mixes: &HashMap<String, Mix>| {
        home_dto(history.home_rows(&nd()), None, mixes, &covers)
            .recent
            .iter()
            .map(|i| (i.kind, i.name.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        shown(&HashMap::new()),
        [(ContextKind::Playlist, "Name p".into())]
    );
    let mix = Mix {
        id: coda_core::mix::MixId::parse("0123456789abcdef").unwrap(),
        name: "Evening".into(),
        source: coda_core::mix::MixSource::Navidrome { server: server() },
        ingredients: vec![Ingredient::Genre {
            name: "Jazz".into(),
        }],
    };
    let mixes = HashMap::from([("0123456789abcdef".to_owned(), mix)]);
    assert_eq!(
        shown(&mixes),
        [
            (ContextKind::Mix, "Evening".into()),
            (ContextKind::Playlist, "Name p".into())
        ]
    );
    // Local rows without a library resolve to nothing.
    history.record_play(local_folder_context(&[]), 3);
    assert!(
        home_dto(
            history.home_rows(&Scope::Local),
            None,
            &HashMap::new(),
            &covers
        )
        .recent
        .is_empty()
    );
}

#[test]
fn server_ids_are_stable_hashes_and_scope_the_history() {
    let id = server();
    assert_eq!(id, server_id_of("https://music.example/", "listener"));
    assert_eq!(id.len(), 16);
    assert!(!id.contains("music") && !id.contains("listener"));
    assert_ne!(id, server_id_of("https://music.example/", "other"));
    assert_ne!(id, server_id_of("https://music2.example/", "listener"));
    let mut history = History::default();
    history.record_play(remote(ContextKind::Album, "a"), 1);
    history.record_listen(vec![remote(ContextKind::Playlist, "p")], 2);
    let other = Scope::Navidrome(server_id_of("https://other.example/", "listener"));
    assert!(history.recent(&other, 10).is_empty());
    assert!(history.home_rows(&other).recent.is_empty());
    assert_eq!(history.recent(&nd(), 10).len(), 1);
    assert_eq!(history.home_rows(&nd()).recent.len(), 1);
    // The same album id on two servers is two contexts.
    history.record_listen(
        vec![Context {
            server: Some(server_id_of("https://other.example/", "listener")),
            ..remote(ContextKind::Playlist, "p")
        }],
        3,
    );
    assert_eq!(history.counts_len(), 2);
    // A Navidrome context without a server is never recorded.
    let before = history.revision();
    history.record_play(
        Context {
            server: None,
            ..remote(ContextKind::Album, "b")
        },
        4,
    );
    assert_eq!(history.revision(), before);
}

// ----- per-track listens (2.0 N5b) ------------------------------------------

fn heard_local(id: &str) -> HeardTrack {
    HeardTrack {
        source: SourceKind::Local,
        server: None,
        id: id.into(),
        song: None,
    }
}

fn heard_song(id: &str, artist_id: Option<&str>, artist: &str) -> HeardTrack {
    HeardTrack {
        source: SourceKind::Navidrome,
        server: Some(server()),
        id: id.into(),
        song: Some(SongInfo {
            title: format!("Song {id}"),
            artist: Some(artist.into()),
            album: Some("Album".into()),
            album_id: Some("al-1".into()),
            artist_id: artist_id.map(Into::into),
            cover_art: Some("al-1".into()),
            duration_ms: Some(200_000),
            ..SongInfo::default()
        }),
    }
}

#[test]
fn track_listens_count_and_drop_the_least_recently_heard_beyond_the_bound() {
    let mut history = History::default();
    history.record_track_listen(heard_local("k"), 5);
    history.record_track_listen(heard_local("k"), 9);
    history.record_track_listen(heard_song("s1", Some("ar-1"), "Band"), 7);
    let count = history.track_count(&heard_local("k").key()).unwrap();
    assert_eq!((count.listens, count.last), (2, 9));
    assert_eq!(history.tracks_len(), 2);
    // Local and Navidrome keys never mix.
    assert_eq!(history.local_track_listens().len(), 1);

    let mut history = History::default();
    for i in 0..(MAX_TRACK_COUNTS + 3) {
        history.record_track_listen(heard_local(&format!("t{i}")), 100 + i as u64);
    }
    assert_eq!(history.tracks_len(), MAX_TRACK_COUNTS);
    for gone in ["t0", "t1", "t2"] {
        assert!(
            history.track_count(&heard_local(gone).key()).is_none(),
            "{gone}"
        );
    }
    assert!(history.track_count(&heard_local("t3").key()).is_some());
    // A file over the bound is trimmed the same way when loaded.
    let mut file = history.to_file();
    file.tracks.push(TrackCount {
        track: heard_local("older"),
        listens: 50,
        last: 1,
    });
    let loaded = History::from_file(file);
    assert_eq!(loaded.tracks_len(), MAX_TRACK_COUNTS);
    assert!(loaded.track_count(&heard_local("older").key()).is_none());
}

#[test]
fn untrusted_track_entries_are_cleaned_or_dropped() {
    let mut history = History::default();
    history.record_track_listen(heard_local(""), 1);
    history.record_track_listen(heard_local("a\0b"), 1);
    history.record_track_listen(heard_local(&"x".repeat(MAX_ID + 1)), 1);
    // A Navidrome song needs a known server.
    history.record_track_listen(
        HeardTrack {
            server: None,
            ..heard_song("s1", None, "Band")
        },
        1,
    );
    history.record_track_listen(
        HeardTrack {
            server: Some("https://music.example/".into()),
            ..heard_song("s2", None, "Band")
        },
        1,
    );
    assert_eq!(history.tracks_len(), 0);
    // Text is cleaned and capped, odd ids are dropped; a local entry
    // keeps nothing but its key.
    let mut song = heard_song("s3", Some(""), "Ba\nnd");
    if let Some(info) = song.song.as_mut() {
        info.title = format!("  {}", "t".repeat(MAX_TEXT + 10));
        info.cover_art = Some("c\0".into());
    }
    history.record_track_listen(song.clone(), 1);
    history.record_track_listen(
        HeardTrack {
            server: Some(server()),
            song: song.song.clone(),
            ..heard_local("local-key")
        },
        1,
    );
    let kept = history.track_count(&song.key()).unwrap();
    let info = kept.track.song.as_ref().unwrap();
    assert_eq!(info.title.chars().count(), MAX_TEXT);
    assert_eq!(info.artist.as_deref(), Some("Ba nd"));
    assert_eq!(
        (info.artist_id.clone(), info.cover_art.clone()),
        (None, None)
    );
    let local = history
        .track_count(&heard_local("local-key").key())
        .unwrap();
    assert_eq!(
        (local.track.server.clone(), local.track.song.clone()),
        (None, None)
    );
}

#[test]
fn track_listens_round_trip_through_the_file_without_urls() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    let mut history = History::default();
    history.record_track_listen(heard_local("band\u{1f}record\u{1f}1\u{1f}2\u{1f}song"), 3);
    history.record_track_listen(heard_song("s1", Some("ar-1"), "Band"), 4);
    let file = history.to_file();
    assert_eq!(file.version, 3);
    save(&path, &file).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("http") && !text.contains("coda://") && !text.contains("/m/"));
    let loaded = load(Some(path));
    assert_eq!(loaded.history.to_file(), file);
    // Most recently heard first.
    assert_eq!(loaded.history.to_file().tracks[0].track.id, "s1");
}

#[test]
fn version_1_and_2_files_are_migrated_keeping_their_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    let album = serde_json::to_value(remote(ContextKind::Album, "al-1")).unwrap();
    let mut play = album.clone();
    play["at"] = 5.into();
    let mut count = album;
    count["listens"] = 4.into();
    count["last"] = 6.into();
    let v1 = serde_json::json!({
        "version": 1,
        "plays": [play],
        "counts": [count],
        "dailies": [{ "date": "2026-11-06", "server": server(), "albums": [] }],
    });
    std::fs::write(&path, v1.to_string()).unwrap();
    let loaded = load(Some(path.clone()));
    assert_eq!(loaded.path.as_ref(), Some(&path), "saved over (migrated)");
    let history = loaded.history;
    assert_eq!(history.plays().count(), 1);
    assert_eq!(history.counts_len(), 1);
    assert_eq!(history.tracks_len(), 0);
    // Saved as version 3, the old data kept, the Daily Picks dropped.
    save(&path, &history.to_file()).unwrap();
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["version"], 3);
    assert_eq!(saved["counts"][0]["listens"], 4);
    assert!(saved.get("dailies").is_none());
    assert!(take_aside(dir.path()).is_none(), "not set aside");
    // Version 2: its track counts kept too.
    let mut v2 = v1.clone();
    v2["version"] = 2.into();
    v2["tracks"] = serde_json::json!([{ "source": "local", "id": "k", "listens": 3, "last": 7 }]);
    std::fs::write(&path, v2.to_string()).unwrap();
    let loaded = load(Some(path.clone()));
    assert_eq!(loaded.path.as_ref(), Some(&path), "saved over (migrated)");
    assert_eq!(loaded.history.plays().count(), 1);
    assert_eq!(loaded.history.tracks_len(), 1);
    assert!(take_aside(dir.path()).is_none(), "not set aside");
    // A version 4 file is a newer build's: kept, not saved over.
    let newer = "{\"version\": 4, \"tracks\": []}";
    std::fs::write(&path, newer).unwrap();
    assert!(load(Some(path.clone())).path.is_none());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
}

#[test]
fn local_album_listens_sum_their_tracks_and_artists_get_their_top_songs() {
    let mut library = Library::new("/m".into());
    let song = |rel: &str, album: &str, no: u32| {
        Track::local(
            PathBuf::from(format!("/m/{rel}")),
            TrackMetadata {
                title: Some(format!("Song {rel}")),
                artist: Some("Band".into()),
                album: Some(album.into()),
                track_no: Some(no),
                ..TrackMetadata::default()
            },
        )
    };
    library.extend([
        song("a1", "A", 1),
        song("a2", "A", 2),
        song("b1", "B", 1),
        song("c1", "C", 1),
        local_track("other.flac", "Other", "Elsewhere"),
    ]);
    let tags = library.tags();
    let key_of = |rel: &str| {
        let id = library
            .track_id_by_path(Path::new(&format!("/m/{rel}")))
            .unwrap();
        let track = library.track(id);
        let album = tags.album_of(track).unwrap();
        local_track_key(&local_album_key(tags, album), track).unwrap()
    };
    assert_eq!(key_of("a2"), "band\u{1f}a\u{1f}\u{1f}2\u{1f}song a2");
    let mut history = History::default();
    let mut hear = |rel: &str, times: u64, at: u64| {
        for _ in 0..times {
            history.record_track_listen(heard_local(&key_of(rel)), at);
        }
    };
    hear("a1", 2, 10);
    hear("a2", 3, 11);
    hear("b1", 4, 12);
    hear("other.flac", 9, 13);
    // Heard as often as a1, but more recently.
    hear("c1", 2, 20);
    let listens = history.local_track_listens();
    let sums = local_album_listens(&library, &listens);
    let by_name = |name: &str| {
        tags.albums(AlbumOrder::Name)
            .iter()
            .find(|a| tags.album(**a).name() == name)
            .map(|a| sums[a.0 as usize])
            .unwrap()
    };
    assert_eq!((by_name("A"), by_name("B"), by_name("C")), (5, 4, 2));
    assert_eq!(by_name("Elsewhere"), 9);

    let band = tags.artist_by_name("Band").unwrap();
    let top = local_artist_top_tracks(&library, band, &listens, TOP_TRACKS);
    let titles: Vec<(String, u64)> = top
        .iter()
        .map(|(t, _, n)| (library.track(*t).title.clone(), *n))
        .collect();
    assert_eq!(
        titles,
        [
            ("Song b1".to_owned(), 4),
            ("Song a2".to_owned(), 3),
            ("Song c1".to_owned(), 2),
            ("Song a1".to_owned(), 2),
        ]
    );
    assert_eq!(
        local_artist_top_tracks(&library, band, &listens, 1).len(),
        1
    );
    // Never heard: none.
    let other = tags.artist_by_name("Other").unwrap();
    assert!(local_artist_top_tracks(&library, other, &HashMap::new(), TOP_TRACKS).is_empty());
}

#[test]
fn navidrome_top_songs_are_the_servers_by_artist_id_or_name() {
    let mut history = History::default();
    for (id, times) in [("s1", 1), ("s2", 3), ("s3", 2)] {
        for _ in 0..times {
            history.record_track_listen(heard_song(id, Some("ar-1"), "Band"), 10);
        }
    }
    // No artist id: matched by its album artist's name.
    let mut feat = heard_song("s4", None, "Band feat. Guest");
    if let Some(info) = feat.song.as_mut() {
        info.album_artist = Some("band".into());
    }
    history.record_track_listen(feat, 11);
    // Another artist of the same name (another id): not this one's.
    history.record_track_listen(heard_song("s6", Some("ar-7"), "Band"), 11);
    history.record_track_listen(heard_song("x", Some("ar-2"), "Others"), 12);
    // Another server's song of the same artist.
    history.record_track_listen(
        HeardTrack {
            server: Some(server_id_of("https://other.example/", "listener")),
            ..heard_song("s5", Some("ar-1"), "Band")
        },
        13,
    );
    let ids =
        |top: Vec<TrackCount>| -> Vec<String> { top.into_iter().map(|c| c.track.id).collect() };
    assert_eq!(
        ids(history.navidrome_top_tracks(&server(), "ar-1", "Band", TOP_TRACKS)),
        ["s2", "s3", "s4", "s1"]
    );
    assert_eq!(
        ids(history.navidrome_top_tracks(&server(), "ar-1", "", TOP_TRACKS)),
        ["s2", "s3", "s1"]
    );
    assert!(
        history
            .navidrome_top_tracks(&server(), "ar-3", "Nobody", TOP_TRACKS)
            .is_empty()
    );
}

#[test]
fn local_track_keys_need_an_album_and_tell_unnumbered_tracks_apart() {
    let tagged = |rel: &str, album: Option<&str>, no: Option<u32>| {
        Track::local(
            PathBuf::from(format!("/m/{rel}")),
            TrackMetadata {
                title: Some("Intro".into()),
                artist: Some("Band".into()),
                album: album.map(Into::into),
                track_no: no,
                ..TrackMetadata::default()
            },
        )
    };
    let key = "band\u{1f}record";
    // No album tag: not counted (all would share "Unknown album").
    assert_eq!(
        local_track_key(key, &tagged("a/Intro.flac", None, Some(1))),
        None
    );
    assert_eq!(
        local_track_key(key, &tagged("a/Intro.flac", Some("  "), None)),
        None
    );
    // Unnumbered: the file name (not the path) keeps two "Intro"s apart.
    let one =
        local_track_key(key, &tagged("Band/Record/Intro.flac", Some("Record"), None)).unwrap();
    let two = local_track_key(
        key,
        &tagged("Band/Record/Intro (2).flac", Some("Record"), None),
    )
    .unwrap();
    assert_ne!(one, two);
    assert!(one.ends_with("\u{1f}intro.flac"), "{one}");
    assert!(!one.contains('/'), "no path: {one}");
    // A moved file keeps its key.
    assert_eq!(
        one,
        local_track_key(key, &tagged("Elsewhere/Intro.flac", Some("Record"), None)).unwrap()
    );
    // Numbered: by number and title only.
    assert_eq!(
        local_track_key(key, &tagged("x/1.flac", Some("Record"), Some(1))).unwrap(),
        "band\u{1f}record\u{1f}\u{1f}1\u{1f}intro"
    );
}

// ----- moods (2.0 N6) -------------------------------------------------------

fn genre_library() -> Library {
    let mut library = Library::new("/m".into());
    let mut tracks = Vec::new();
    for (genre, albums) in [("Jazz", 5), ("Rock", 3), ("Folk", 1)] {
        for i in 0..albums {
            tracks.push(Track::local(
                PathBuf::from(format!("/m/{genre}/{i}/1.flac")),
                TrackMetadata {
                    title: Some("Song".into()),
                    artist: Some(format!("{genre} Band")),
                    album: Some(format!("{genre} {i}")),
                    duration: Some(Duration::from_secs(600)),
                    genres: vec![genre.into()],
                    ..TrackMetadata::default()
                },
            ));
        }
    }
    library.extend(tracks);
    library
}

#[test]
fn local_moods_follow_the_listens_and_resolve_to_cards() {
    use super::commands::{local_mood_dtos, local_moods};
    use super::moods::Listens;
    let browse = handle(genre_library());
    let now = 1_000 * 86_400;
    let key = |genre: &str, i: usize| format!("{} band\u{1f}{} {i}", lower(genre), lower(genre));
    // Rock was heard lately (one album this week), Jazz never.
    let listens: HashMap<String, Listens> = HashMap::from([
        (
            key("Rock", 0),
            Listens {
                listens: 9,
                last: now - 86_400,
            },
        ),
        (
            key("Rock", 1),
            Listens {
                listens: 2,
                last: now - 30 * 86_400,
            },
        ),
    ]);
    let picks =
        |date: &str| browse.read_library(|view| local_moods(&view.unwrap(), &listens, now, date));
    let today = picks("2026-11-19");
    let genres: Vec<&str> = today.iter().map(|m| m.genre.as_str()).collect();
    // Rock ranks first, but only two of its albums may play (one was
    // heard this week): it fills in after Jazz; Folk has one album.
    assert_eq!(genres, ["Jazz", "Rock", "Folk"]);
    assert_eq!(today[0].albums.len(), 3);
    assert_eq!(today[1].albums.len(), 2);
    assert!(!today[1].albums.iter().any(|a| a.id == key("Rock", 0)));
    // Never heard first, then the longest unheard.
    assert_eq!(today[1].albums[1].id, key("Rock", 1));
    // The same day: the same picks; stored by stable keys with names.
    assert_eq!(picks("2026-11-19"), today);
    assert_eq!(today[0].albums[0].artist.as_deref(), Some("Jazz Band"));
    // As cards: library ids, covers, lengths, the last listen and the
    // quick mix ingredient; an album gone from the library is left out.
    browse.replace_library(Some(genre_library()));
    let cards = browse
        .read_library(|view| local_mood_dtos(&view.unwrap(), &today, &listens, browse.covers()));
    let generation = browse.library_generation();
    assert_eq!(cards.len(), 3);
    assert_eq!(cards[0].album_count, Some(5));
    let rock = &cards[1].albums[1];
    assert!(rock.id.starts_with(&format!("{generation}:")));
    assert_eq!(rock.duration_ms, Some(600_000));
    assert_eq!(rock.last_played, Some(now - 30 * 86_400));
    assert_eq!(
        rock.ingredient,
        crate::mixes::IngredientDto::Album {
            artist: "Rock Band".into(),
            name: "Rock 1".into(),
            id: None,
        }
    );
    let mut gone = today.clone();
    gone[2].albums[0].id = "nope".into();
    let cards = browse
        .read_library(|view| local_mood_dtos(&view.unwrap(), &gone, &listens, browse.covers()));
    assert_eq!(cards.len(), 2, "a mood with no album left is left out");
}

#[test]
fn a_local_stop_is_found_by_its_track_in_the_context_now() {
    let album_of = |order: &[u32]| {
        let mut library = Library::new("/m".into());
        library.extend(order.iter().map(|n| {
            Track::local(
                PathBuf::from(format!("/m/Band/Record/{n}.flac")),
                TrackMetadata {
                    title: Some(format!("Song {n}")),
                    artist: Some("Band".into()),
                    album: Some("Record".into()),
                    track_no: Some(*n),
                    ..TrackMetadata::default()
                },
            )
        }));
        library
    };
    let browse = handle(album_of(&[1, 2, 3]));
    let (context, track) = browse.read_library(|view| {
        let view = view.unwrap();
        let tags = view.library.tags();
        let album = tags.albums(AlbumOrder::Name)[0];
        let second = tags.album(album).tracks()[1];
        (
            local_album_context(tags, album),
            local_track_key(&local_album_key(tags, album), view.library.track(second)).unwrap(),
        )
    });
    let stop = Stop {
        source: SourceKind::Local,
        server: None,
        kind: ContextKind::Album,
        id: context.id.clone(),
        track,
        index: 1,
        total: 3,
        position_ms: 30_000,
        at: 2,
    };
    let mut history = History::default();
    history.record_play(context, 1);
    history.record_stop(stop);
    let resolve = |browse: &BrowseHandle| {
        let rows = history.home_rows(&Scope::Local);
        browse.read_library(|view| {
            home_dto(rows, view.as_ref(), &HashMap::new(), browse.covers()).recent[0]
                .stop
                .clone()
                .unwrap()
        })
    };
    // A rescan that added a track before it: found by its track, with a
    // library id of this generation.
    browse.replace_library(Some(album_of(&[0, 1, 2, 3])));
    let found = resolve(&browse);
    assert_eq!(
        (found.index, found.total, found.position_ms),
        (2, 4, 30_000)
    );
    let generation = browse.library_generation();
    assert!(
        found
            .track_id
            .unwrap()
            .starts_with(&format!("{generation}:"))
    );
    // Gone: the recorded place, from its start, without a track.
    browse.replace_library(Some(album_of(&[1])));
    let gone = resolve(&browse);
    assert_eq!(
        (gone.index, gone.total, gone.track_id, gone.position_ms),
        (0, 1, None, 0)
    );
}
