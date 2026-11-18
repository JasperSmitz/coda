use super::*;
use crate::model::TrackMetadata;
use std::collections::HashMap;

fn song(artist: &str, album: &str, n: usize) -> Track {
    Track::local(
        format!("/m/{artist}/{album}/{n:03}.flac").into(),
        TrackMetadata {
            title: Some(format!("{artist}|{album}|{n}")),
            artist: Some(artist.into()),
            album: Some(album.into()),
            ..TrackMetadata::default()
        },
    )
}

/// `songs` songs by `artist`, `per_album` to an album.
fn artist_songs(artist: &str, songs: usize, per_album: usize) -> Vec<Track> {
    (0..songs)
        .map(|n| song(artist, &format!("Album {}", n / per_album), n))
        .collect()
}

fn artist_of(track: &Track) -> &str {
    track.artist.as_deref().unwrap()
}

#[test]
fn ingredients_take_turns_whatever_their_size() {
    // Each artist owns more songs than its share of the window (200 / 3),
    // so none ever sits out.
    let mut picker = Picker::from_pools(vec![
        artist_songs("King Crimson", 600, 10),
        artist_songs("Yes", 250, 10),
        artist_songs("Genesis", 120, 8),
    ]);
    assert_eq!(picker.window(), MAX_RECENT_WINDOW);
    let picks = picker.next(300, &mut Rng::from_seed(1));
    assert_eq!(picks.len(), 300);
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for track in &picks {
        *counts.entry(artist_of(track)).or_default() += 1;
    }
    assert_eq!(counts["King Crimson"], 100);
    assert_eq!(counts["Yes"], 100);
    assert_eq!(counts["Genesis"], 100);
    // Every round of three visits each ingredient once.
    for round in picks.chunks(3) {
        let mut artists: Vec<&str> = round.iter().map(artist_of).collect();
        artists.sort_unstable();
        assert_eq!(artists, ["Genesis", "King Crimson", "Yes"]);
    }
    // In a new random order each round.
    let orders: std::collections::HashSet<Vec<&str>> = picks
        .chunks(3)
        .map(|r| r.iter().map(artist_of).collect())
        .collect();
    assert!(orders.len() > 1);
}

/// Asserts no song in `picks` repeats within `window` picks.
fn assert_no_repeat_within(picks: &[Track], window: usize) {
    let mut last: HashMap<&TrackSource, usize> = HashMap::new();
    for (i, track) in picks.iter().enumerate() {
        if let Some(prev) = last.insert(&track.source, i) {
            assert!(
                i - prev > window,
                "{} again after {}",
                track.title,
                i - prev
            );
        }
    }
}

fn count_of(picks: &[Track], track: &Track) -> usize {
    picks.iter().filter(|p| p.source == track.source).count()
}

#[test]
fn a_song_in_two_ingredients_is_not_favoured() {
    // An album partly inside its genre: the album keeps its songs, and the
    // shared songs are no likelier than its album's other songs.
    let album: Vec<Track> = (0..12).map(|n| song("Yes", "Fragile", n)).collect();
    let mut genre: Vec<Track> = (0..60)
        .map(|n| song(&format!("Band {n}"), "X", n))
        .collect();
    genre.push(album[0].clone());
    genre.push(album[1].clone());
    let mut picker = Picker::from_pools(vec![genre, album.clone()]);
    let picks = picker.next(4000, &mut Rng::from_seed(9));
    assert_no_repeat_within(&picks, picker.window());
    let album_counts: Vec<usize> = album.iter().map(|t| count_of(&picks, t)).collect();
    let mean = album_counts.iter().sum::<usize>() / album.len();
    for c in &album_counts {
        assert!(c.abs_diff(mean) * 10 <= mean, "{album_counts:?}");
    }
}

#[test]
fn an_album_inside_its_genre_keeps_its_songs() {
    // Every album song is also in the genre: the album owns them, and the
    // genre's turns play only its other songs.
    let album: Vec<Track> = (0..10).map(|n| song("Yes", "Fragile", n)).collect();
    let mut genre: Vec<Track> = (0..40)
        .map(|n| song(&format!("Band {n}"), "X", n))
        .collect();
    genre.extend(album.iter().cloned());
    let mut picker = Picker::from_pools(vec![genre, album.clone()]);
    assert_eq!(picker.window(), 25);
    let mut rng = Rng::from_seed(3);
    // The first round: one turn each, so one album song and one other.
    let first = picker.next(2, &mut rng);
    assert_eq!(first.iter().filter(|t| artist_of(t) == "Yes").count(), 1);
    let picks = picker.next(3000, &mut rng);
    assert_no_repeat_within(&picks, picker.window());
    for track in &album {
        assert!(count_of(&picks, track) > 0, "{}", track.title);
    }
}

#[test]
fn equal_pools_that_overlap_give_shared_songs_to_the_first() {
    // Two equal pools sharing 10 songs: the first owns the shared ones, the
    // second plays only its own 10, and the turns stay half each.
    let shared: Vec<Track> = (0..10).map(|n| song("Shared", "S", n)).collect();
    let mut a: Vec<Track> = (0..10).map(|n| song("A", "A", n)).collect();
    let mut b: Vec<Track> = (0..10).map(|n| song("B", "B", n)).collect();
    a.extend(shared.iter().cloned());
    b.extend(shared.iter().cloned());
    let mut picker = Picker::from_pools(vec![a, b]);
    let picks = picker.next(1000, &mut Rng::from_seed(5));
    assert_no_repeat_within(&picks, picker.window());
    let from_b = picks.iter().filter(|t| artist_of(t) == "B").count();
    assert_eq!(from_b, 500, "the second pool's turns play only its own");
    for track in &shared {
        assert!(count_of(&picks, track) > 0, "{}", track.title);
    }
}

#[test]
fn identical_pools_act_as_one() {
    let pool = artist_songs("Yes", 30, 10);
    let mut picker = Picker::from_pools(vec![pool.clone(), pool.clone()]);
    assert!(picker.empty().is_empty(), "the second found songs");
    assert_eq!(picker.window(), 15);
    let picks = picker.next(300, &mut Rng::from_seed(8));
    assert_eq!(picks.len(), 300);
    assert_no_repeat_within(&picks, picker.window());
    assert_eq!(picker.played_through(), [0], "the first owns them all");
}

#[test]
fn no_song_repeats_within_the_window() {
    // 20 songs, each its own artist and album: a window of 10.
    let pool: Vec<Track> = (0..20).map(|n| song(&format!("A{n}"), "X", n)).collect();
    let mut picker = Picker::from_pools(vec![pool]);
    assert_eq!(picker.window(), 10);
    let picks = picker.next(400, &mut Rng::from_seed(4));
    let mut last: HashMap<&TrackSource, usize> = HashMap::new();
    for (i, track) in picks.iter().enumerate() {
        if let Some(prev) = last.insert(&track.source, i) {
            assert!(i - prev > 10, "{} again after {}", track.title, i - prev);
        }
    }
    // The window is capped.
    let big: Vec<Track> = (0..1000).map(|n| song("B", "Y", n)).collect();
    assert_eq!(Picker::from_pools(vec![big]).window(), MAX_RECENT_WINDOW);
}

#[test]
fn a_small_ingredient_sits_out_while_its_songs_are_recent() {
    // A 2,000-song genre and a single: the single plays only once it has
    // left the window, not every other song.
    let single = song("Single", "S", 0);
    let mut picker = Picker::from_pools(vec![artist_songs("Big", 2000, 10), vec![single.clone()]]);
    assert_eq!(picker.window(), MAX_RECENT_WINDOW);
    let picks = picker.next(2000, &mut Rng::from_seed(2));
    assert_eq!(picks.len(), 2000);
    assert_no_repeat_within(&picks, MAX_RECENT_WINDOW);
    let at: Vec<usize> = (0..picks.len())
        .filter(|&i| picks[i].source == single.source)
        .collect();
    // Once in the first round, then about every 201 picks.
    assert!(at[0] < 2, "{at:?}");
    assert!((9..=10).contains(&at.len()), "{at:?}");
    for pair in at.windows(2) {
        assert!((201..=203).contains(&(pair[1] - pair[0])), "{at:?}");
    }
}

#[test]
fn when_every_song_is_recent_the_one_heard_longest_ago_plays() {
    // Not reachable with the real window (at most half the songs); forced
    // here to check the safety net.
    let mut picker = Picker::from_pools(vec![
        vec![song("A", "A", 0), song("A", "A", 1)],
        vec![song("B", "B", 0), song("B", "B", 1)],
    ]);
    picker.window = 4;
    let picks = picker.next(40, &mut Rng::from_seed(2));
    assert_eq!(picks.len(), 40);
    // Each turn replays its ingredient's song picked longest ago: each
    // ingredient alternates its two songs.
    for artist in ["A", "B"] {
        let own: Vec<&Track> = picks.iter().filter(|t| artist_of(t) == artist).collect();
        assert_eq!(own.len(), 20);
        for pair in own.windows(2) {
            assert_ne!(pair[0].source, pair[1].source, "{artist}");
        }
    }
}

#[test]
fn a_pool_is_played_through_once_each_of_its_songs_was_picked() {
    // One 500-song sample (a Navidrome genre): the window is 200, yet the
    // pool is reported once every song has played, which takes about 500
    // picks: no song plays twice before then.
    let sample: Vec<Track> = (0..500)
        .map(|n| song(&format!("Band {}", n % 50), &format!("Album {}", n / 10), n))
        .collect();
    let mut picker = Picker::from_pools(vec![sample.clone()]);
    assert_eq!(picker.window(), MAX_RECENT_WINDOW);
    let mut rng = Rng::from_seed(11);
    let mut picks: Vec<Track> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    while picker.played_through().is_empty() {
        assert!(seen.len() < sample.len(), "every song played, not reported");
        assert!(picks.len() < 520, "not played through in {}", picks.len());
        // A batch that plays the pool through is still full.
        let batch = picker.next(20, &mut rng);
        assert_eq!(batch.len(), 20);
        seen.extend(batch.iter().map(|t| t.source.clone()));
        picks.extend(batch);
    }
    assert_eq!(seen.len(), sample.len());
    assert_eq!(picker.played_through(), [0]);
    let first_plays: std::collections::HashSet<_> =
        picks[..sample.len()].iter().map(|t| &t.source).collect();
    assert_eq!(first_plays.len(), sample.len(), "a song played twice");
    // It keeps picking until the resolver replaces it.
    let more = picker.next(20, &mut rng);
    assert_eq!(more.len(), 20);
    picks.extend(more);
    // A new sample that shares the last 100 songs played: not played
    // through, and the recent picks still wait their turn.
    let recent: Vec<Track> = picks[picks.len() - 100..].to_vec();
    let mut next_sample: Vec<Track> = (500..900)
        .map(|n| song(&format!("Band {}", n % 50), &format!("Album {}", n / 10), n))
        .collect();
    next_sample.extend(recent);
    picker.set_pool(0, next_sample);
    assert!(picker.played_through().is_empty());
    picks.extend(picker.next(300, &mut rng));
    assert_no_repeat_within(&picks, MAX_RECENT_WINDOW);
}

#[test]
fn the_next_song_avoids_the_previous_album_then_artist() {
    // One artist, three albums: never the same album twice in a row.
    let mut picker = Picker::from_pools(vec![artist_songs("Yes", 15, 5)]);
    let picks = picker.next(200, &mut Rng::from_seed(6));
    for pair in picks.windows(2) {
        assert_ne!(
            pair[0].album, pair[1].album,
            "{} then {}",
            pair[0].title, pair[1].title
        );
    }
    // A genre of two artists: never the same artist twice in a row.
    let mut genre = artist_songs("Yes", 20, 4);
    genre.extend(artist_songs("Genesis", 20, 4));
    let mut picker = Picker::from_pools(vec![genre]);
    let picks = picker.next(200, &mut Rng::from_seed(6));
    for pair in picks.windows(2) {
        assert_ne!(artist_of(&pair[0]), artist_of(&pair[1]));
    }
    // An album alone has no alternative: it still plays.
    let mut picker = Picker::from_pools(vec![artist_songs("Yes", 3, 3)]);
    assert_eq!(picker.next(9, &mut Rng::from_seed(1)).len(), 9);
}

#[test]
fn empty_and_pending_pools_are_skipped() {
    let mut picker = Picker::from_pools(vec![Vec::new(), artist_songs("Yes", 10, 5)]);
    assert_eq!(picker.empty(), [0]);
    let picks = picker.next(6, &mut Rng::from_seed(1));
    assert_eq!(picks.len(), 6);
    assert!(picks.iter().all(|t| artist_of(t) == "Yes"));

    let mut none = Picker::from_pools(vec![Vec::new(), Vec::new()]);
    assert!(none.next(5, &mut Rng::from_seed(1)).is_empty());
    assert!(
        Picker::from_pools(Vec::new())
            .next(5, &mut Rng::from_seed(1))
            .is_empty()
    );

    // Pools still to come (a Navidrome fetch): skipped until they arrive.
    let mut picker = Picker::new(2);
    assert_eq!(picker.pending(), [0, 1]);
    assert!(!picker.is_ready());
    assert!(picker.next(3, &mut Rng::from_seed(1)).is_empty());
    picker.set_pool(1, artist_songs("Yes", 10, 5));
    assert_eq!(picker.pending(), [0]);
    let picks = picker.next(4, &mut Rng::from_seed(1));
    assert!(picks.iter().all(|t| artist_of(t) == "Yes"));
    picker.set_pool(0, artist_songs("Genesis", 10, 5));
    picker.set_pool(7, Vec::new());
    assert!(picker.is_ready());
    let picks = picker.next(10, &mut Rng::from_seed(1));
    assert_eq!(
        picks.iter().filter(|t| artist_of(t) == "Genesis").count(),
        5
    );
}

#[test]
fn picks_are_deterministic_for_a_seed() {
    let pools = || {
        vec![
            artist_songs("King Crimson", 40, 8),
            artist_songs("Yes", 25, 5),
            artist_songs("Genesis", 30, 10),
        ]
    };
    let run = |seed| -> Vec<String> {
        let mut picker = Picker::from_pools(pools());
        let mut rng = Rng::from_seed(seed);
        let mut titles: Vec<String> = Vec::new();
        for n in [25, 20, 20] {
            titles.extend(picker.next(n, &mut rng).into_iter().map(|t| t.title));
        }
        titles
    };
    assert_eq!(run(42), run(42));
    assert_ne!(run(42), run(43));
}
