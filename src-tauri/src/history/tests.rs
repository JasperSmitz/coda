//! Play history: recording rules, bounds, persistence (atomic, FIFO-safe,
//! corrupt files set aside), local keys and their resolution, Daily Picks.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use coda_core::library::m3u::PlaylistFile;
use coda_core::model::{Track, TrackMetadata};

use super::*;
use crate::browse::BrowseHandle;

fn remote(kind: ContextKind, id: &str) -> Context {
    Context {
        source: SourceKind::Navidrome,
        kind,
        id: id.into(),
        name: format!("Name {id}"),
        subtitle: Some("Band".into()),
        cover_art: Some(format!("cover-{id}")),
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
        .recent(SourceKind::Navidrome, 10)
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
    assert_eq!(history.recent(SourceKind::Navidrome, 1).len(), 1);
    let local = history.recent(SourceKind::Local, 10);
    assert_eq!(local.len(), 1);
    assert_eq!(local[0].context.id, "Jazz");
}

#[test]
fn listens_count_once_per_context_and_most_played_sorts() {
    let mut history = History::default();
    let album = remote(ContextKind::Album, "al");
    let playlist = remote(ContextKind::Playlist, "pl");
    history.record_listen(vec![album.clone(), playlist.clone(), album.clone()], 10);
    history.record_listen(vec![album.clone()], 20);
    let key = album.key();
    assert_eq!(history.count(&key).unwrap().listens, 2);
    assert_eq!(history.count(&key).unwrap().last, 20);
    assert_eq!(history.count(&playlist.key()).unwrap().listens, 1);
    let most = history.most_played(
        SourceKind::Navidrome,
        &[ContextKind::Album, ContextKind::Playlist],
        10,
    );
    assert_eq!(
        most.iter()
            .map(|c| c.context.id.as_str())
            .collect::<Vec<_>>(),
        ["al", "pl"]
    );
    // Kinds filter: Navidrome's Home asks for playlists only.
    let rows = history.home_rows(SourceKind::Navidrome);
    assert_eq!(rows.most_played.len(), 1);
    assert_eq!(rows.most_played[0].context.kind, ContextKind::Playlist);
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
    let played = history.recent(SourceKind::Navidrome, 1).remove(0);
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
        history.recent(SourceKind::Navidrome, 1)[0]
            .context
            .name
            .chars()
            .count(),
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
    assert_eq!(
        history.recent(SourceKind::Local, 1)[0].context.cover_art,
        None
    );
}

#[test]
fn the_file_round_trips_and_holds_no_urls() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("coda-app").join("history.json");
    let mut history = History::default();
    history.record_play(remote(ContextKind::Playlist, "p"), 7);
    history.record_listen(vec![remote(ContextKind::Album, "a")], 8);
    history.set_daily(
        "2026-11-06".into(),
        vec![DailyAlbum {
            id: "al-1".into(),
            name: "One".into(),
            artist: Some("Band".into()),
            artist_id: None,
            year: Some(1999),
            cover_art: Some("al-1".into()),
        }],
    );
    save(&path, &history.to_file()).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("http") && !text.contains("coda://"));
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o077, 0, "private: {mode:o}");
    let loaded = load(Some(path.clone()));
    assert_eq!(loaded.path, Some(path));
    assert_eq!(loaded.history.to_file(), history.to_file());
    assert_eq!(loaded.history.daily("2026-11-06").unwrap().len(), 1);
    assert!(loaded.history.daily("2026-11-07").is_none());
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

#[test]
fn a_corrupt_file_is_set_aside_and_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    for bad in ["not json", "{\"version\": 99}", "{\"plays\": 3}"] {
        std::fs::write(&path, bad).unwrap();
        let loaded = load(Some(path.clone()));
        assert_eq!(loaded.path.as_ref(), Some(&path), "{bad}");
        assert_eq!(loaded.history.plays().count(), 0);
        assert!(!path.exists(), "moved aside: {bad}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("history.json.bad")).unwrap(),
            bad
        );
    }
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
    assert!(dir.path().join("history.json.bad").exists());
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
    });
    file.daily = Some(Daily {
        date: "yesterday".into(),
        albums: Vec::new(),
    });
    let history = History::from_file(file);
    assert_eq!(history.plays().count(), MAX_PLAYS);
    assert_eq!(history.counts_len(), 0);
    assert!(history.to_file().daily.is_none());
}

#[test]
fn the_first_picks_of_a_day_stay_and_another_server_clears_them() {
    let album = |id: &str| DailyAlbum {
        id: id.into(),
        name: id.into(),
        artist: None,
        artist_id: None,
        year: None,
        cover_art: None,
    };
    let mut history = History::default();
    let first = history.set_daily("2026-11-06".into(), vec![album("a"), album("")]);
    assert_eq!(first.len(), 1, "bad ids dropped");
    let again = history.set_daily("2026-11-06".into(), vec![album("b")]);
    assert_eq!(again[0].id, "a");
    let next_day = history.set_daily("2026-11-07".into(), vec![album("c")]);
    assert_eq!(next_day[0].id, "c");
    assert!(history.set_daily("bad".into(), vec![album("d")]).is_empty());
    history.clear_daily();
    assert!(history.daily("2026-11-07").is_none());
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
    let rows = history.home_rows(SourceKind::Local);
    let home = browse.read_library(|view| home_dto(rows, view.as_ref(), browse.covers()));
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
    let home = home_dto(history.home_rows(SourceKind::Navidrome), None, &covers);
    assert_eq!(home.recent[0].name, "Name p");
    assert!(
        home.recent[0]
            .cover_url
            .as_deref()
            .unwrap()
            .starts_with("coda://")
    );
    assert_eq!(home.most_played[0].listens, Some(1));
    // Local rows without a library resolve to nothing.
    history.record_play(local_folder_context(&[]), 3);
    assert!(
        home_dto(history.home_rows(SourceKind::Local), None, &covers)
            .recent
            .is_empty()
    );
}

#[test]
fn local_daily_picks_are_stable_for_a_day_and_change_with_it() {
    let browse = handle(library(40));
    let picks = |date: &str| {
        browse.read_library(|view| {
            local_daily_picks(&view.unwrap(), date, browse.covers())
                .into_iter()
                .map(|a| a.name)
                .collect::<Vec<_>>()
        })
    };
    let today = picks("2026-11-06");
    assert_eq!(today.len(), DAILY_PICKS);
    let distinct: HashSet<_> = today.iter().collect();
    assert_eq!(distinct.len(), DAILY_PICKS);
    assert_eq!(picks("2026-11-06"), today);
    assert_ne!(picks("2026-11-07"), today);
    // The same albums in another scan order: the same picks.
    let mut reversed = Library::new("/m".into());
    reversed.extend((0..40).rev().map(|i| {
        local_track(
            &format!("Band {}/Album {i}/1.flac", i % 3),
            &format!("Band {}", i % 3),
            &format!("Album {i}"),
        )
    }));
    browse.replace_library(Some(reversed));
    assert_eq!(picks("2026-11-06"), today);
    // Fewer albums than picks: all of them.
    browse.replace_library(Some(library(3)));
    assert_eq!(picks("2026-11-06").len(), 3);
}
