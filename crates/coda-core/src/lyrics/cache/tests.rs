use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::lyrics::Lyrics;

fn track(id: &str, album: &str, artist: &str, title: &str, n: u32) -> Track {
    let mut track = Track::navidrome(
        id.into(),
        TrackMetadata {
            title: Some(title.into()),
            artist: Some(artist.into()),
            album: Some(album.into()),
            album_artist: None,
            track_no: Some(n),
            disc_no: Some(1),
            duration: Some(Duration::from_secs(200)),
            cover_art: Some(format!("mf-{id}")),
        },
    );
    track.artist_id = Some(format!("ar-{artist}"));
    track
}

fn lyrics(text: &str) -> Option<IndexedLyrics> {
    IndexedLyrics::new(&Lyrics::untimed(text.lines()))
}

/// A cache with two albums: a1 (s1 with lyrics, s2 checked without) and
/// a2 (s3 unchecked).
fn sample() -> Cache {
    let mut cache = Cache {
        refreshed: 1_000,
        ..Cache::default()
    };
    cache.put_album(
        "a1",
        Some(2),
        vec![
            track("s1", "First", "Band", "One", 1),
            track("s2", "First", "Band", "Two", 2),
        ],
        500,
    );
    cache.put_album(
        "a2",
        Some(1),
        vec![track("s3", "Second", "Act", "Three", 1)],
        500,
    );
    cache.put_lyrics("s1", lyrics("Golden river\nRuns to the sea"), 600);
    cache.put_lyrics("s2", None, 600);
    cache
}

/// Every song with a slot is in the index there, and nothing else is.
fn assert_consistent(cache: &Cache) {
    let with_slot = cache.songs.values().filter(|s| s.slot.is_some()).count();
    assert_eq!(cache.index.len(), with_slot);
    for (id, song) in &cache.songs {
        if let Some(slot) = song.slot {
            let (key, _) = cache.index.iter().nth(slot as usize).unwrap();
            assert_eq!(key, id);
        }
    }
    let checked = cache.songs.values().filter(|s| s.checked.is_some()).count();
    assert_eq!(cache.checked, checked);
}

fn store(cache: Cache) -> Store {
    Store::with_cache(None, cache)
}

#[test]
fn the_file_round_trips() {
    let cache = sample();
    let bytes = cache.to_bytes().unwrap();
    let back = Cache::from_bytes(&bytes).unwrap();
    assert_eq!(back.refreshed, 1_000);
    assert_eq!(back.albums.len(), 2);
    assert_eq!(back.songs.len(), 3);
    assert_eq!(back.checked, 2);
    assert_eq!(back.index.len(), 1);
    let s1 = &back.songs["s1"];
    assert_eq!(s1.track("s1"), cache.songs["s1"].track("s1"));
    assert_eq!(s1.checked, Some(600));
    assert!(s1.slot.is_some());
    assert_eq!(back.songs["s2"].checked, Some(600));
    assert!(back.songs["s2"].slot.is_none());
    assert_eq!(back.songs["s3"].checked, None);
    assert_eq!(
        back.albums["a1"].songs,
        vec![Arc::from("s1"), Arc::from("s2")]
    );
    assert_eq!(back.albums["a1"].song_count, Some(2));
    // The same bytes again (the order is stable).
    assert_eq!(back.to_bytes().unwrap(), bytes);
}

#[test]
fn a_saved_store_opens_with_the_same_contents_and_private_modes() {
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().join("coda").join("lyrics");
    let store = Store::with_cache(Some(file_path(&cache_dir, "k")), sample());
    store.save_now().unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&cache_dir), 0o700);
    assert_eq!(mode(&file_path(&cache_dir, "k")), 0o600);
    let opened = Store::open(&cache_dir, "k");
    assert_eq!(opened.status().songs, 3);
    assert_eq!(opened.status().checked, 2);
    assert_eq!(opened.status().with_lyrics, 1);
    assert_eq!(opened.status().state, State::Idle);
    let found = opened.search("golden river", &HashSet::new(), 50);
    assert_eq!(found.total, 1);
    assert_eq!(found.matches[0].key.title, "One");
    // No temp file is left.
    let names: Vec<_> = fs::read_dir(&cache_dir).unwrap().collect();
    assert_eq!(names.len(), 1);
}

#[test]
fn unusable_files_count_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = file_path(dir.path(), "k");
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("corrupt", b"{\"v\":1,\"refreshed\":".to_vec()),
        ("not json", vec![0xff, 0x00, 0x13]),
        (
            "wrong version",
            br#"{"v":2,"refreshed":1,"albums":[],"songs":[]}"#.to_vec(),
        ),
        ("wrong shape", br#"{"v":1,"albums":7}"#.to_vec()),
    ];
    for (case, bytes) in cases {
        fs::write(&path, bytes).unwrap();
        let store = Store::open(dir.path(), "k");
        assert_eq!(store.status().songs, 0, "{case}");
    }
    // A folder where the file should be.
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert_eq!(Store::open(dir.path(), "k").status().songs, 0);
    // No file at all.
    assert_eq!(Store::open(dir.path(), "none").status().songs, 0);
}

#[test]
fn an_oversized_file_counts_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = file_path(dir.path(), "k");
    let file = File::create(&path).unwrap();
    // Sparse: no disk space used.
    file.set_len(MAX_FILE_BYTES + 1).unwrap();
    assert!(open_file(&path).is_err());
    assert_eq!(Store::open(dir.path(), "k").status().songs, 0);
}

#[test]
fn loading_cleans_and_caps_like_server_data() {
    let long = "x".repeat(crate::model::MAX_METADATA_CHARS + 50);
    let long_line = "y".repeat(crate::lyrics::MAX_LINE_CHARS + 50);
    let long_id = "i".repeat(MAX_ID_CHARS + 1);
    let json = serde_json::json!({
        "v": 1, "refreshed": 5,
        "albums": [{ "id": "a", "n": 2, "t": 1 }, { "id": "", "t": 1 }],
        "songs": [
            { "id": "s", "al": "a", "ti": format!("  {long} "), "ar": "   ", "t": 3,
              "ly": format!("{long_line}\n\n  la la  ") },
            { "id": long_id, "al": "a", "ti": "too long an id", "t": 3 },
            { "id": "orphan", "al": "missing", "ti": "no album", "t": 3 },
            { "id": "s", "al": "a", "ti": "duplicate", "t": 3 },
            { "id": "u", "al": "a", "ly": "unchecked lyrics are not kept" }
        ]
    });
    let cache = Cache::from_bytes(&serde_json::to_vec(&json).unwrap()).unwrap();
    assert_eq!(cache.albums.len(), 1);
    assert_eq!(cache.songs.len(), 2);
    let s = cache.songs["s"].track("s");
    assert_eq!(
        s.title.chars().count(),
        crate::model::MAX_METADATA_CHARS,
        "title capped"
    );
    assert_eq!(s.artist, None, "blank is absent");
    assert_eq!(
        cache.songs["u"].track("u").title,
        crate::model::UNKNOWN_TITLE
    );
    assert!(cache.songs["u"].slot.is_none());
    let (_, l) = cache.index.iter().next().unwrap();
    let lines: Vec<&str> = l.text().split('\n').collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].chars().count(), crate::lyrics::MAX_LINE_CHARS);
    assert_eq!(lines[1], "la la");
}

#[test]
fn the_file_holds_no_url_or_credentials() {
    // Only what the cache is given can end up in it; this checks the
    // writer adds nothing else (the key is a digest of the URL).
    let key = server_key("https://music.example.org/navidrome/", "listener");
    assert!(!key.contains("example"));
    assert_eq!(key.len(), 32);
    assert_ne!(
        key,
        server_key("https://music.example.org/navidrome/", "other")
    );
    let bytes = sample().to_bytes().unwrap();
    let text = String::from_utf8(bytes).unwrap();
    for needle in ["http", "example", "listener", "token", "salt"] {
        assert!(!text.contains(needle), "{needle}");
    }
}

#[test]
fn search_orders_by_artist_album_disc_track_and_skips_excluded() {
    let mut cache = Cache::default();
    cache.put_album(
        "b",
        Some(2),
        vec![
            track("b2", "Zed", "beta", "Second", 2),
            track("b1", "Zed", "Beta", "First", 1),
        ],
        1,
    );
    cache.put_album(
        "a",
        Some(1),
        vec![track("a1", "Any", "Alpha", "Only", 1)],
        1,
    );
    for id in ["b2", "b1", "a1"] {
        cache.put_lyrics(id, lyrics("we sing along tonight"), 2);
    }
    let store = store(cache);
    let found = store.search("Sing Along", &HashSet::new(), 50);
    let ids: Vec<_> = found
        .matches
        .iter()
        .map(|m| m.key.navidrome_id().unwrap().to_owned())
        .collect();
    assert_eq!(ids, ["a1", "b1", "b2"]);
    assert_eq!(found.total, 3);
    let m = &found.matches[0];
    assert_eq!(m.snippet.text, "we sing along tonight");
    assert_eq!(m.snippet.highlights, vec![3..13]);
    // The track plays like any Navidrome track.
    assert_eq!(m.key.album_id.as_deref(), Some("a"));
    assert_eq!(m.key.artist_id.as_deref(), Some("ar-Alpha"));
    assert_eq!(m.key.cover_art.as_deref(), Some("mf-a1"));

    let exclude: HashSet<String> = ["b1".to_owned()].into();
    let found = store.search("sing along", &exclude, 1);
    assert_eq!(found.total, 2);
    assert_eq!(found.matches.len(), 1);
    assert_eq!(found.matches[0].key.navidrome_id(), Some("a1"));
    // Too short: no work.
    assert_eq!(store.search("we", &HashSet::new(), 50).total, 0);
}

#[test]
fn an_album_refetch_drops_its_old_songs_and_keeps_known_lyrics() {
    let mut cache = sample();
    // s2 is gone, s4 is new, s1 keeps its lyrics and takes new metadata.
    cache.put_album(
        "a1",
        Some(2),
        vec![
            track("s1", "First (Remaster)", "Band", "One", 1),
            track("s4", "First (Remaster)", "Band", "Four", 2),
        ],
        700,
    );
    assert_consistent(&cache);
    assert!(!cache.songs.contains_key("s2"));
    assert_eq!(cache.songs["s4"].checked, None);
    assert!(cache.songs["s1"].slot.is_some());
    assert_eq!(cache.songs["s1"].album.as_deref(), Some("First (Remaster)"));
    assert_eq!(cache.checked, 1);
    assert_eq!(cache.albums["a1"].listed, 700);
    // Removing an album drops its songs and their lyrics.
    cache.remove_album("a1");
    assert_consistent(&cache);
    assert!(cache.index.is_empty());
    assert_eq!(cache.songs.len(), 1);
    assert_eq!(cache.checked, 0);
}

#[test]
fn rechecked_lyrics_replace_the_old_ones() {
    let mut cache = sample();
    cache.put_lyrics("s1", lyrics("Silver lake"), 900);
    let store = store(cache);
    assert_eq!(store.search("golden", &HashSet::new(), 50).total, 0);
    assert_eq!(store.search("silver lake", &HashSet::new(), 50).total, 1);
    let mut cache = store.write();
    cache.put_lyrics("s1", None, 950);
    assert!(cache.index.is_empty());
    assert_eq!(cache.counts(State::Idle).with_lyrics, 0);
}

#[test]
fn a_deleted_file_is_not_written_again() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::with_cache(Some(file_path(dir.path(), "k")), sample());
    store.save_now().unwrap();
    assert!(file_path(dir.path(), "k").exists());
    store.delete_file().unwrap();
    assert!(!file_path(dir.path(), "k").exists());
    store.save_now().unwrap();
    assert!(!file_path(dir.path(), "k").exists());
    // Deleting a missing file is fine.
    remove_file(dir.path(), "k").unwrap();
    store.delete_file().unwrap();
}

#[test]
fn cache_dir_follows_xdg() {
    assert_eq!(
        cache_dir(Some("/xdg".into()), Some("/home/u".into())),
        Some(PathBuf::from("/xdg/coda/lyrics"))
    );
    assert_eq!(
        cache_dir(Some("relative".into()), Some("/home/u".into())),
        Some(PathBuf::from("/home/u/.cache/coda/lyrics"))
    );
    assert_eq!(cache_dir(None, None), None);
}

#[test]
fn slots_stay_consistent_through_removals_and_replacements() {
    let mut cache = Cache::default();
    let tracks: Vec<Track> = (0..20)
        .map(|n| track(&format!("s{n}"), "A", "Band", "Song", n))
        .collect();
    cache.put_album("a", Some(20), tracks, 1);
    for n in 0..20 {
        let text = format!("verse number {n}");
        cache.put_lyrics(&format!("s{n}"), lyrics(&text), 2);
    }
    assert_consistent(&cache);
    // Remove some from the middle and the end, replace and clear others.
    let keep: Vec<Track> = (0..20)
        .filter(|n| n % 3 != 0)
        .map(|n| track(&format!("s{n}"), "A", "Band", "Song", n))
        .collect();
    cache.put_album("a", Some(13), keep, 3);
    assert_consistent(&cache);
    cache.put_lyrics("s1", lyrics("brand new words"), 4);
    cache.put_lyrics("s2", None, 4);
    cache.put_lyrics("s19", None, 4);
    assert_consistent(&cache);
    let store = store(cache);
    // 10, 11, 13, 14, 16, 17 (s1 replaced, s19 cleared).
    assert_eq!(store.search("verse number 1", &HashSet::new(), 50).total, 6);
    assert_eq!(store.search("brand new", &HashSet::new(), 50).total, 1);
    assert_eq!(store.search("number 2", &HashSet::new(), 50).total, 0);
}

#[test]
fn a_song_that_moves_to_another_album_keeps_its_lyrics() {
    let mut cache = sample();
    // s1 now belongs to a2 (fetched first), then a1 is refetched without it.
    cache.put_album(
        "a2",
        Some(2),
        vec![
            track("s3", "Second", "Act", "Three", 1),
            track("s1", "Second", "Act", "One", 2),
        ],
        700,
    );
    assert_eq!(&*cache.songs["s1"].album_id, "a2");
    assert!(!cache.albums["a1"].songs.iter().any(|s| &**s == "s1"));
    assert!(cache.songs["s1"].slot.is_some());
    cache.put_album(
        "a1",
        Some(1),
        vec![track("s2", "First", "Band", "Two", 2)],
        700,
    );
    assert!(cache.songs.contains_key("s1"), "kept: it is a2's now");
    assert_consistent(&cache);
    // The other order: removed with a1 first, then added back with a2.
    let mut cache = sample();
    cache.put_album(
        "a1",
        Some(1),
        vec![track("s2", "First", "Band", "Two", 2)],
        700,
    );
    assert!(!cache.songs.contains_key("s1"));
    cache.put_album(
        "a2",
        Some(2),
        vec![
            track("s3", "Second", "Act", "Three", 1),
            track("s1", "Second", "Act", "One", 2),
        ],
        700,
    );
    // Back as a new song, checked again; no stale index entry.
    assert_eq!(cache.songs["s1"].checked, None);
    assert!(cache.songs["s1"].slot.is_none());
    assert_consistent(&cache);
    let store = store(cache);
    assert_eq!(store.search("golden river", &HashSet::new(), 50).total, 0);
}

#[test]
fn failures_are_counted_and_skip_after_three_syncs() {
    let mut cache = sample();
    assert!(!cache.fail_song("s3", 10));
    assert!(!cache.fail_song("s3", 11));
    assert_eq!(cache.songs["s3"].checked, None);
    // Persisted across a save and load.
    let mut cache = Cache::from_bytes(&cache.to_bytes().unwrap()).unwrap();
    assert_eq!(cache.songs["s3"].failures, 2);
    assert!(cache.fail_song("s3", 12));
    assert_eq!(cache.songs["s3"].checked, Some(12));
    assert_eq!(cache.songs["s3"].failures, 0);
    // A success resets the count.
    cache.fail_song("s1", 13);
    cache.put_lyrics("s1", lyrics("again"), 14);
    assert_eq!(cache.songs["s1"].failures, 0);
    // Albums, known or new.
    assert!(!cache.fail_album("new", Some(4), 20));
    let mut cache = Cache::from_bytes(&cache.to_bytes().unwrap()).unwrap();
    assert_eq!(cache.album_failures.get("new"), Some(&1));
    assert!(!cache.fail_album("new", Some(4), 21));
    assert!(cache.fail_album("new", Some(4), 22));
    assert_eq!(cache.albums["new"].song_count, Some(4));
    assert_eq!(cache.albums["new"].listed, 22);
    assert!(cache.album_failures.is_empty());
    assert_consistent(&cache);
}

#[test]
fn the_reader_streams_and_refuses_odd_files() {
    // Unknown keys are ignored; the version may come anywhere.
    let ok = br#"{"v":1,"refreshed":3,"extra":[1,{"x":2}],"albums":[{"id":"a","t":1}],"songs":[{"id":"s","al":"a","ti":"T","t":2,"ly":"la la"}]}"#;
    let cache = Cache::from_bytes(ok).unwrap();
    assert_eq!(cache.refreshed, 3);
    assert_eq!(cache.index.len(), 1);
    let late = br#"{"refreshed":3,"albums":[],"songs":[],"v":1}"#;
    assert!(Cache::from_bytes(late).is_ok());
    let wrong_late = br#"{"refreshed":3,"albums":[],"songs":[],"v":2}"#;
    assert!(Cache::from_bytes(wrong_late).is_err());
    // A song before its album is dropped.
    let early = br#"{"v":1,"refreshed":3,"songs":[{"id":"s","al":"a","t":2,"ly":"x y z"}],"albums":[{"id":"a","t":1}]}"#;
    let cache = Cache::from_bytes(early).unwrap();
    assert!(cache.songs.is_empty());
    let missing = br#"{"v":1,"albums":[],"songs":[]}"#;
    assert!(Cache::from_bytes(missing).is_err());
    let trailing = br#"{"v":1,"refreshed":1,"albums":[],"songs":[]} x"#;
    assert!(Cache::from_bytes(trailing).is_err());
}

#[test]
fn an_unchanged_store_is_not_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = file_path(dir.path(), "k");
    let store = Store::with_cache(Some(path.clone()), sample());
    assert!(store.is_dirty());
    store.save_now().unwrap();
    assert!(!store.is_dirty());
    fs::remove_file(&path).unwrap();
    store.save_now().unwrap();
    assert!(!path.exists(), "nothing changed: no write");
    let opened = Store::open(dir.path(), "none");
    assert!(!opened.is_dirty());
    store.update(|c| c.put_lyrics("s3", lyrics("new"), 5));
    store.save_now().unwrap();
    assert!(path.exists());
}

#[test]
fn stale_temp_files_are_removed_when_a_cache_opens() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join(".lyrics.abc.tmp");
    let fresh = dir.path().join(".lyrics.def.tmp");
    let other = dir.path().join("keep.tmp");
    for path in [&old, &fresh, &other] {
        fs::write(path, b"x").unwrap();
    }
    let long_ago = SystemTime::now() - STALE_TEMP - Duration::from_secs(60);
    for path in [&old, &other] {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
    }
    Store::open(dir.path(), "k");
    assert!(!old.exists());
    assert!(fresh.exists(), "a save may be writing it");
    assert!(other.exists());
}
