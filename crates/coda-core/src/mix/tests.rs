use super::*;
use crate::library::Library;
use crate::model::{Track, TrackMetadata, TrackSource};
use crate::playback::{EndReason, PlaybackEvent};
use crate::player::{PROVEN_PLAYBACK, Player, PlayerCommand, RepeatMode};
use crate::queue::{MAX_QUEUE_LEN, Placement, Rng};

const SERVER: &str = "00c36aa1074c465c";

fn genre(name: &str) -> Ingredient {
    Ingredient::Genre { name: name.into() }
}

fn artist(name: &str) -> Ingredient {
    Ingredient::Artist {
        name: name.into(),
        id: None,
    }
}

fn album(artist: &str, name: &str) -> Ingredient {
    Ingredient::Album {
        artist: artist.into(),
        name: name.into(),
        id: None,
    }
}

fn song(artist: &str, album: &str, n: u32, genres: &[&str]) -> Track {
    Track::local(
        format!("/m/{artist}/{album}/{n:02}.flac").into(),
        TrackMetadata {
            title: Some(format!("{artist} {album} {n}")),
            artist: Some(artist.into()),
            album: Some(album.into()),
            track_no: Some(n),
            genres: genres.iter().map(|g| (*g).to_owned()).collect(),
            ..TrackMetadata::default()
        },
    )
}

fn path(track: &Track) -> String {
    match &track.source {
        TrackSource::Local { path } => path.to_string_lossy().into_owned(),
        TrackSource::Navidrome { song_id } => song_id.clone(),
    }
}

// ----- model -------------------------------------------------------------

#[test]
fn a_mix_is_cleaned_and_checked() {
    let mix = Mix::new(
        "  Prog day ",
        MixSource::Local,
        vec![
            genre(" Prog Rock "),
            Ingredient::Artist {
                name: "Yes".into(),
                id: Some("ar-1".into()),
            },
            genre("Prog Rock"),
            album("Genesis", "Foxtrot"),
        ],
    )
    .unwrap();
    assert_eq!(mix.name, "Prog day");
    assert_eq!(
        mix.ingredients,
        [
            genre("Prog Rock"),
            artist("Yes"),
            album("Genesis", "Foxtrot")
        ],
        "trimmed, duplicates dropped, no ids for a local mix"
    );
    assert_eq!(mix.id.as_str().len(), 16);
    assert_ne!(mix.id, MixId::random());

    let err = |name: &str, source: MixSource, ingredients: Vec<Ingredient>| {
        Mix::new(name, source, ingredients).unwrap_err()
    };
    assert_eq!(
        err(" ", MixSource::Local, vec![genre("Rock")]),
        MixError::NoName
    );
    assert_eq!(
        err("a\u{7}b", MixSource::Local, vec![genre("Rock")]),
        MixError::NoName
    );
    assert_eq!(
        err("Mix", MixSource::Local, vec![]),
        MixError::NoIngredients
    );
    assert_eq!(
        err("Mix", MixSource::Local, vec![genre(" ")]),
        MixError::BadIngredient
    );
    assert_eq!(
        err("Mix", MixSource::Local, vec![album("", "X")]),
        MixError::BadIngredient
    );
    let many: Vec<Ingredient> = (0..=MAX_INGREDIENTS)
        .map(|i| genre(&format!("g{i}")))
        .collect();
    assert_eq!(
        err("Mix", MixSource::Local, many.clone()),
        MixError::TooManyIngredients
    );
    assert!(Mix::new("Mix", MixSource::Local, many[..MAX_INGREDIENTS].to_vec()).is_ok());
    let server = |s: &str| MixSource::Navidrome { server: s.into() };
    assert_eq!(
        err("Mix", server("https://x"), vec![genre("Rock")]),
        MixError::BadServer
    );
    // Text is capped like tags.
    let long = "x".repeat(MAX_METADATA_CHARS + 10);
    let mix = Mix::new(long.clone(), MixSource::Local, vec![genre(&long)]).unwrap();
    assert_eq!(mix.name.chars().count(), MAX_METADATA_CHARS);
}

#[test]
fn a_navidrome_mix_keeps_exact_genres_and_usable_ids() {
    let mix = Mix::new(
        "Remote",
        MixSource::Navidrome {
            server: SERVER.into(),
        },
        vec![
            genre(" Rock "),
            Ingredient::Artist {
                name: "Yes".into(),
                id: Some("ar-1".into()),
            },
            Ingredient::Album {
                artist: "Yes".into(),
                name: "Fragile".into(),
                id: Some("bad\nid".into()),
            },
        ],
    )
    .unwrap();
    assert_eq!(
        mix.ingredients,
        [
            genre(" Rock "),
            Ingredient::Artist {
                name: "Yes".into(),
                id: Some("ar-1".into())
            },
            album("Yes", "Fragile"),
        ],
        "the server's exact genre value; a bad id falls back to the name"
    );
    // An empty id, or one over the id cap, is no id.
    let ids = |id: String| {
        Mix::new(
            "Remote",
            MixSource::Navidrome {
                server: SERVER.into(),
            },
            vec![Ingredient::Artist {
                name: "Yes".into(),
                id: Some(id),
            }],
        )
        .unwrap()
        .ingredients
    };
    assert_eq!(ids(String::new()), [artist("Yes")]);
    assert_eq!(
        ids("a".repeat(crate::navidrome::api::MAX_ID_CHARS + 1)),
        [artist("Yes")]
    );
    let longest = "a".repeat(crate::navidrome::api::MAX_ID_CHARS);
    assert_eq!(
        ids(longest.clone()),
        [Ingredient::Artist {
            name: "Yes".into(),
            id: Some(longest)
        }]
    );
}

#[test]
fn mix_ids_are_checked() {
    assert!(MixId::parse("0123abcdEF-_x").is_some());
    for bad in ["", "a b", "a/b", "é", &"a".repeat(65)] {
        assert_eq!(MixId::parse(bad), None, "{bad:?}");
    }
    let id = MixId::random();
    assert_eq!(MixId::parse(id.as_str()), Some(id));
}

#[test]
fn the_no_songs_message_names_the_ingredients() {
    assert_eq!(no_songs_message([]), None);
    let missing = [genre("Jazz"), artist("Yes"), album("Yes", "Fragile")];
    assert_eq!(
        no_songs_message(&missing).unwrap(),
        "No songs for genre Jazz, artist Yes, album Fragile by Yes"
    );
}

// ----- local resolution --------------------------------------------------

fn library(tracks: &[Track]) -> Library {
    let mut library = Library::new("/m".into());
    library.extend(tracks.iter().cloned());
    library
}

fn paths(tracks: &[Track]) -> Vec<String> {
    let mut paths: Vec<String> = tracks.iter().map(path).collect();
    paths.sort();
    paths
}

#[test]
fn local_ingredients_resolve_by_name_across_a_rescan() {
    let tracks = vec![
        song("Yes", "Fragile", 1, &["Prog Rock"]),
        song("Yes", "Fragile", 2, &["Prog Rock", "Rock"]),
        song("Yes", "Close to the Edge", 1, &["Prog Rock"]),
        song("Genesis", "Foxtrot", 1, &["prog rock"]),
        song("Abba", "Arrival", 1, &["Pop"]),
    ];
    let mix = Mix::new(
        "Prog",
        MixSource::Local,
        vec![
            genre("PROG ROCK"),
            artist("yes"),
            album("YES", "fragile"),
            genre("Jazz"),
            album("Abba", "Fragile"),
        ],
    )
    .unwrap();
    let first = library(&tracks);
    let mut reversed = tracks.clone();
    reversed.reverse();
    // A rescan finds the files in another order: every id changes.
    let second = library(&reversed);
    assert_ne!(
        first.tags().artist_by_name("Yes"),
        second.tags().artist_by_name("Yes")
    );
    for lib in [&first, &second] {
        let pools: Vec<Vec<Track>> = mix
            .ingredients
            .iter()
            .map(|i| local::pool(lib, i))
            .collect();
        assert_eq!(pools[0].len(), 4, "the genre, ignoring case");
        assert_eq!(
            paths(&pools[1]),
            [
                "/m/Yes/Close to the Edge/01.flac",
                "/m/Yes/Fragile/01.flac",
                "/m/Yes/Fragile/02.flac"
            ]
        );
        assert_eq!(
            paths(&pools[2]),
            ["/m/Yes/Fragile/01.flac", "/m/Yes/Fragile/02.flac"]
        );
        assert!(pools[3].is_empty(), "no such genre");
        assert!(pools[4].is_empty(), "no such album by that artist");
        let active = ActiveMix::new(mix.clone(), true, local::picker(lib, &mix), &Player::new());
        assert_eq!(
            active.empty_ingredients(),
            [&genre("Jazz"), &album("Abba", "Fragile")]
        );
    }
}

// ----- the active mix ----------------------------------------------------

/// A mix of three artists with plenty of songs.
fn big_mix() -> (Mix, Picker) {
    let mix = Mix::new(
        "Big three",
        MixSource::Local,
        vec![artist("King Crimson"), artist("Yes"), artist("Genesis")],
    )
    .unwrap();
    let pools = ["King Crimson", "Yes", "Genesis"]
        .iter()
        .map(|a| {
            (0..60)
                .map(|n| song(a, &format!("Album {}", n / 10), n, &[]))
                .collect()
        })
        .collect();
    (mix, Picker::from_pools(pools))
}

fn titles(player: &Player) -> Vec<String> {
    player
        .queue()
        .entries()
        .iter()
        .map(|e| e.track.title.clone())
        .collect()
}

/// Plays on to the entry `n` after the current one (as skips do).
fn skip(player: &mut Player, n: usize) {
    for _ in 0..n {
        player.play_next();
    }
}

#[test]
fn starting_a_mix_queues_25_songs_and_plays_the_first() {
    let (mix, picker) = big_mix();
    let mut player = Player::new();
    let mut active = ActiveMix::new(mix, true, picker, &player);
    player.enqueue(song("Other", "Old", 1, &[]));
    let mut rng = Rng::from_seed(7);
    let commands = active.start(&mut player, &mut rng).unwrap();
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, PlayerCommand::Load { .. }))
    );
    assert_eq!(player.queue().len(), START_TRACKS, "the queue was replaced");
    assert_eq!(
        player.queue().position(player.queue().current().unwrap()),
        Some(0)
    );
    assert!(active.owns_queue(&player));
    assert_eq!(active.wants_top_up(&player), None, "24 remain");
}

#[test]
fn a_mix_tops_up_when_five_or_fewer_remain() {
    let (mix, picker) = big_mix();
    let mut player = Player::new();
    let mut active = ActiveMix::new(mix, true, picker, &player);
    let mut rng = Rng::from_seed(7);
    active.start(&mut player, &mut rng).unwrap();
    // 24 after the current one; 6 after skipping 18.
    skip(&mut player, 18);
    assert_eq!(player.queue().remaining_after_current(), 6);
    assert_eq!(active.wants_top_up(&player), None);
    assert_eq!(active.top_up(&mut player, &mut rng).added, 0);
    skip(&mut player, 1);
    assert_eq!(active.wants_top_up(&player), Some(TOP_UP_TRACKS));
    let before = titles(&player);
    let current = player.queue().current();
    assert_eq!(active.top_up(&mut player, &mut rng).added, TOP_UP_TRACKS);
    assert_eq!(player.queue().len(), START_TRACKS + TOP_UP_TRACKS);
    assert_eq!(titles(&player)[..START_TRACKS], before[..], "appended");
    assert_eq!(player.queue().current(), current, "playback untouched");
    assert_eq!(active.wants_top_up(&player), None);
}

#[test]
fn replacing_or_clearing_the_queue_ends_the_mix_and_edits_do_not() {
    let (mix, picker) = big_mix();
    let mut player = Player::new();
    let mut active = ActiveMix::new(mix, false, picker, &player);
    let mut rng = Rng::from_seed(3);
    active.start(&mut player, &mut rng).unwrap();
    // Adding, reordering, removing and skipping keep the mix.
    player.enqueue(song("Other", "X", 1, &[]));
    player.enqueue_all(vec![song("Other", "X", 2, &[])], Placement::Next);
    let ids: Vec<_> = player.queue().entries().iter().map(|e| e.id).collect();
    player.move_down(ids[3]);
    player.remove(ids[5]);
    player.shuffle_upcoming(&mut rng);
    skip(&mut player, 2);
    player.play_previous();
    assert!(active.owns_queue(&player));
    // Playing something else replaces the queue: the mix has ended.
    player.play_context(vec![song("Other", "Y", 1, &[])], 0);
    assert!(!active.owns_queue(&player));
    assert_eq!(
        active.wants_top_up(&player),
        None,
        "one entry, but not ours"
    );
    assert_eq!(active.top_up(&mut player, &mut rng).added, 0);
    assert_eq!(player.queue().len(), 1);
    // An ended mix does not start again.
    assert_eq!(active.start(&mut player, &mut rng), Err(StartError::Ended));
    assert_eq!(player.queue().len(), 1);
    // Clearing ends a mix too, also one started on the same player.
    let (mix, picker) = big_mix();
    let mut active = ActiveMix::new(mix, false, picker, &player);
    active.start(&mut player, &mut rng).unwrap();
    assert!(active.owns_queue(&player));
    player.clear_queue();
    assert!(!active.owns_queue(&player));
    assert_eq!(
        active
            .append(&mut player, vec![song("A", "B", 1, &[])])
            .added,
        0
    );
    assert!(player.queue().is_empty());
}

#[test]
fn a_queue_replaced_before_the_mix_starts_ends_it() {
    let (mix, picker) = big_mix();
    let mut player = Player::new();
    let mut active = ActiveMix::new(mix, true, picker, &player);
    let mut rng = Rng::from_seed(3);
    // Adding before it starts does not end it.
    player.enqueue(song("Other", "X", 1, &[]));
    assert!(!active.has_ended(&player));
    // Playing something else does.
    player.play_context(vec![song("Other", "Y", 1, &[])], 0);
    assert!(active.has_ended(&player));
    assert_eq!(active.start(&mut player, &mut rng), Err(StartError::Ended));
    assert_eq!(player.queue().len(), 1, "the user's queue stays");
}

#[test]
fn a_restored_mix_adopts_the_restored_queue() {
    let (mix, picker) = big_mix();
    let mut player = Player::new();
    let mut active = ActiveMix::new(mix.clone(), true, picker, &player);
    player.restore(
        (0..3).map(|n| song("Yes", "Fragile", n, &[])).collect(),
        Some(0),
        crate::player::RepeatMode::All,
        50,
    );
    assert!(!active.owns_queue(&player), "not started");
    active.adopt(&player);
    assert_eq!(active.wants_top_up(&player), Some(TOP_UP_TRACKS));
    assert_eq!(
        active.top_up(&mut player, &mut Rng::from_seed(1)),
        Appended {
            added: TOP_UP_TRACKS,
            commands: Vec::new()
        },
        "a restored queue does not start playing"
    );
    assert_eq!(
        player.repeat(),
        crate::player::RepeatMode::All,
        "left alone"
    );
    assert_eq!(active.session(), SessionMix::Saved(mix.id.clone()));
    let quick = ActiveMix::new(mix.clone(), false, Picker::new(3), &Player::new());
    assert_eq!(quick.session(), SessionMix::Quick(mix));
}

#[test]
fn near_the_cap_the_oldest_played_entries_make_room() {
    let (mix, picker) = big_mix();
    let mut player = Player::new();
    let mut active = ActiveMix::new(mix, true, picker, &player);
    let mut rng = Rng::from_seed(5);
    active.start(&mut player, &mut rng).unwrap();
    // Fill the queue to 3 short of the cap, then play on to its end.
    let filler: Vec<Track> = (0..MAX_QUEUE_LEN - START_TRACKS - 3)
        .map(|n| song("Filler", "F", n as u32, &[]))
        .collect();
    player.enqueue_all(filler, Placement::End);
    let last_but_two = player.queue().entries()[MAX_QUEUE_LEN - 6].id;
    player.play_entry(last_but_two);
    let first = player.queue().entries()[0].id;
    assert_eq!(player.queue().remaining_after_current(), 2);
    assert_eq!(active.top_up(&mut player, &mut rng).added, TOP_UP_TRACKS);
    assert_eq!(player.queue().len(), MAX_QUEUE_LEN);
    assert_eq!(player.queue().current(), Some(last_but_two));
    assert_eq!(player.queue().remaining_after_current(), 2 + TOP_UP_TRACKS);
    assert!(player.queue().get(first).is_none(), "the oldest went first");
    assert!(active.owns_queue(&player), "a trim is not a replace");
}

#[test]
fn a_mix_without_songs_or_playback_does_not_start() {
    let mix = Mix::new("Empty", MixSource::Local, vec![genre("Jazz")]).unwrap();
    let mut player = Player::new();
    let mut active = ActiveMix::new(mix, true, Picker::from_pools(vec![Vec::new()]), &player);
    player.enqueue(song("Other", "Old", 1, &[]));
    let mut rng = Rng::from_seed(1);
    assert_eq!(
        active.start(&mut player, &mut rng),
        Err(StartError::NoSongs)
    );
    assert_eq!(player.queue().len(), 1, "the queue is kept");
    assert!(!active.owns_queue(&player));

    let (mix, picker) = big_mix();
    let mut active = ActiveMix::new(mix, true, picker, &player);
    player.handle_event(PlaybackEvent::BackendExited("gone".into()));
    let before = active.clone();
    assert_eq!(
        active.start(&mut player, &mut rng),
        Err(StartError::Refused)
    );
    assert_eq!(player.queue().len(), 1, "the queue is kept");
    assert!(!active.owns_queue(&player));
    // The picker is as it was: a retry picks what the first try would have.
    let next = |a: &ActiveMix| -> Vec<String> {
        let mut picker = a.picker().clone();
        picker
            .next(START_TRACKS, &mut Rng::from_seed(4))
            .into_iter()
            .map(|t| t.title)
            .collect()
    };
    assert_eq!(next(&active), next(&before));
}

// ----- going on after the end of the queue ------------------------------

/// The current track starts, plays past the proof of playback and ends
/// with `reason`; returns the player's commands.
fn play_and_end(player: &mut Player, reason: EndReason) -> Vec<PlayerCommand> {
    let load = player.current_load().expect("loaded");
    player.handle_event(PlaybackEvent::Started { load });
    player.handle_event(PlaybackEvent::Position(PROVEN_PLAYBACK));
    player.handle_event(PlaybackEvent::TrackEnded { load, reason })
}

/// A started mix playing its last entry (the user skipped past the rest,
/// before any top-up), with `repeat`.
fn mix_at_its_last_entry(repeat: RepeatMode) -> (ActiveMix, Player, Rng) {
    let (mix, picker) = big_mix();
    let mut player = Player::new();
    let mut active = ActiveMix::new(mix, true, picker, &player);
    let mut rng = Rng::from_seed(12);
    active.start(&mut player, &mut rng).unwrap();
    player.set_repeat(repeat);
    skip(&mut player, START_TRACKS - 1);
    assert_eq!(player.queue().remaining_after_current(), 0);
    assert!(player.current_load().is_some());
    (active, player, rng)
}

fn loaded_entry(commands: &[PlayerCommand]) -> Option<&TrackSource> {
    match commands {
        [PlayerCommand::Load { source, .. }] => Some(source),
        _ => None,
    }
}

#[test]
fn a_top_up_after_the_end_of_the_queue_plays_on() {
    let (mut active, mut player, mut rng) = mix_at_its_last_entry(RepeatMode::Off);
    assert!(play_and_end(&mut player, EndReason::Eof).is_empty());
    assert!(player.stopped_at_end());
    assert_eq!(player.current_load(), None);
    let appended = active.top_up(&mut player, &mut rng);
    assert_eq!(appended.added, TOP_UP_TRACKS);
    let first = &player.queue().entries()[START_TRACKS];
    assert_eq!(loaded_entry(&appended.commands), Some(&first.track.source));
    assert_eq!(player.queue().current(), Some(first.id));
    assert!(!player.stopped_at_end());
    // A later top-up, while playing, does not interrupt.
    skip(&mut player, TOP_UP_TRACKS - 1);
    let appended = active.top_up(&mut player, &mut rng);
    assert_eq!(appended.added, TOP_UP_TRACKS);
    assert!(appended.commands.is_empty());
}

#[test]
fn a_refill_arriving_after_the_end_plays_on() {
    // A Navidrome fetch returns after playback reached the end.
    let (mut active, mut player, _) = mix_at_its_last_entry(RepeatMode::Off);
    play_and_end(&mut player, EndReason::Eof);
    let refill: Vec<Track> = (0..3).map(|n| song("Late", "L", n, &[])).collect();
    let appended = active.append(&mut player, refill.clone());
    assert_eq!(appended.added, 3);
    assert_eq!(loaded_entry(&appended.commands), Some(&refill[0].source));
    // A refill arriving while the last entry still plays only appends.
    let (mut active, mut player, _) = mix_at_its_last_entry(RepeatMode::Off);
    let appended = active.append(&mut player, refill);
    assert_eq!(appended.added, 3);
    assert!(appended.commands.is_empty());
}

#[test]
fn a_refill_after_the_end_plays_what_the_user_queued_first() {
    // After the end, the user queues a song before the refill arrives:
    // the refill plays on with what `>` would play, in queue order.
    let refill = || -> Vec<Track> { (0..3).map(|n| song("Late", "L", n, &[])).collect() };
    let user = song("User", "U", 1, &[]);
    for placement in [Placement::End, Placement::Next] {
        let (mut active, mut player, _) = mix_at_its_last_entry(RepeatMode::Off);
        play_and_end(&mut player, EndReason::Eof);
        let ended = player.queue().current().unwrap();
        player.enqueue_all(vec![user.clone()], placement);
        assert!(player.stopped_at_end(), "{placement:?}");
        let appended = active.append(&mut player, refill());
        assert_eq!(appended.added, 3, "{placement:?}");
        assert_eq!(
            loaded_entry(&appended.commands),
            Some(&user.source),
            "{placement:?}"
        );
        // The refill follows the user's song.
        let queue = player.queue();
        let current = queue.current().unwrap();
        assert_eq!(queue.id_before(current), Some(ended), "{placement:?}");
        assert_eq!(queue.remaining_after_current(), 3, "{placement:?}");
    }
    // `a` then `A`: the play-next song comes first, then the `a` one.
    let (mut active, mut player, _) = mix_at_its_last_entry(RepeatMode::Off);
    play_and_end(&mut player, EndReason::Eof);
    let next = song("User", "U", 2, &[]);
    player.enqueue(user.clone());
    player.enqueue_next(next.clone());
    let appended = active.append(&mut player, refill());
    assert_eq!(loaded_entry(&appended.commands), Some(&next.source));
    assert_eq!(player.queue().remaining_after_current(), 4);
}

#[test]
fn a_refill_after_the_end_near_the_cap_plays_the_users_song() {
    // At the cap the refill first trims played entries; the ended entry
    // and the user's song after it stay, and the user's song plays.
    let (mix, picker) = big_mix();
    let mut player = Player::new();
    let mut active = ActiveMix::new(mix, true, picker, &player);
    let mut rng = Rng::from_seed(5);
    active.start(&mut player, &mut rng).unwrap();
    let filler: Vec<Track> = (0..MAX_QUEUE_LEN - START_TRACKS - 1)
        .map(|n| song("Filler", "F", n as u32, &[]))
        .collect();
    player.enqueue_all(filler, Placement::End);
    let last = player.queue().entries()[MAX_QUEUE_LEN - 2].id;
    player.play_entry(last);
    play_and_end(&mut player, EndReason::Eof);
    assert!(player.stopped_at_end());
    let user = song("User", "U", 1, &[]);
    player.enqueue(user.clone());
    assert_eq!(player.queue().len(), MAX_QUEUE_LEN);
    let first = player.queue().entries()[0].id;
    let refill: Vec<Track> = (0..3).map(|n| song("Late", "L", n, &[])).collect();
    let appended = active.append(&mut player, refill);
    assert_eq!(appended.added, 3);
    assert!(player.queue().get(first).is_none(), "trimmed");
    assert_eq!(player.queue().len(), MAX_QUEUE_LEN);
    assert_eq!(loaded_entry(&appended.commands), Some(&user.source));
    let current = player.queue().current().unwrap();
    assert_eq!(player.queue().id_before(current), Some(last));
    assert_eq!(player.queue().remaining_after_current(), 3);
}

#[test]
fn repeat_all_wraps_and_a_top_up_does_not_interrupt() {
    let (mut active, mut player, _) = mix_at_its_last_entry(RepeatMode::All);
    let first = player.queue().entries()[0].id;
    assert!(loaded_entry(&play_and_end(&mut player, EndReason::Eof)).is_some());
    assert_eq!(player.queue().current(), Some(first), "wrapped");
    assert!(!player.stopped_at_end());
    let appended = active.append(&mut player, vec![song("Late", "L", 1, &[])]);
    assert_eq!(appended.added, 1);
    assert!(appended.commands.is_empty());
    assert_eq!(player.queue().current(), Some(first));
}

#[test]
fn only_reaching_the_end_on_its_own_resumes() {
    let refill = || vec![song("Late", "L", 1, &[])];
    let not_resumed = |active: &mut ActiveMix, player: &mut Player, case: &str| {
        assert!(!player.stopped_at_end(), "{case}");
        let appended = active.append(player, refill());
        assert_eq!(appended.added, 1, "{case}");
        assert!(appended.commands.is_empty(), "{case}");
    };
    // The last track fails.
    let (mut active, mut player, _) = mix_at_its_last_entry(RepeatMode::Off);
    play_and_end(&mut player, EndReason::Error("broken".into()));
    not_resumed(&mut active, &mut player, "error");
    // It ends without having played (a suspect end).
    let (mut active, mut player, _) = mix_at_its_last_entry(RepeatMode::Off);
    let load = player.current_load().unwrap();
    player.handle_event(PlaybackEvent::Started { load });
    player.handle_event(PlaybackEvent::TrackEnded {
        load,
        reason: EndReason::Eof,
    });
    not_resumed(&mut active, &mut player, "not played");
    // It is stopped.
    let (mut active, mut player, _) = mix_at_its_last_entry(RepeatMode::Off);
    play_and_end(&mut player, EndReason::Stopped);
    not_resumed(&mut active, &mut player, "stopped");
    // Paused on the last track.
    let (mut active, mut player, _) = mix_at_its_last_entry(RepeatMode::Off);
    player.toggle_pause();
    not_resumed(&mut active, &mut player, "paused");
    // At the end, then Space replays the last track.
    let (mut active, mut player, _) = mix_at_its_last_entry(RepeatMode::Off);
    play_and_end(&mut player, EndReason::Eof);
    player.toggle_pause();
    not_resumed(&mut active, &mut player, "space");
    // At the end, then the last entry is removed.
    let (mut active, mut player, _) = mix_at_its_last_entry(RepeatMode::Off);
    play_and_end(&mut player, EndReason::Eof);
    player.remove(player.queue().current().unwrap());
    not_resumed(&mut active, &mut player, "removed");
    // At the end, then a pause reported by the backend (mpv, media keys),
    // even with an unpause echo after it.
    let (mut active, mut player, _) = mix_at_its_last_entry(RepeatMode::Off);
    play_and_end(&mut player, EndReason::Eof);
    player.handle_event(PlaybackEvent::Paused(true));
    player.handle_event(PlaybackEvent::Paused(false));
    not_resumed(&mut active, &mut player, "backend pause");
    // At the end with the played entries trimmed, then `<` (a no-op at
    // the first entry).
    let (mut active, mut player, _) = mix_at_its_last_entry(RepeatMode::Off);
    play_and_end(&mut player, EndReason::Eof);
    player.remove_played(START_TRACKS - 1);
    assert!(player.stopped_at_end());
    assert!(player.play_previous().is_empty());
    not_resumed(&mut active, &mut player, "previous");
    // At the end, then `>`.
    let (mut active, mut player, _) = mix_at_its_last_entry(RepeatMode::Off);
    play_and_end(&mut player, EndReason::Eof);
    player.play_next();
    not_resumed(&mut active, &mut player, "next");
    // A restored queue on its last entry.
    let (mix, picker) = big_mix();
    let mut player = Player::new();
    let mut active = ActiveMix::new(mix, true, picker, &player);
    player.restore(
        (0..3).map(|n| song("Yes", "Fragile", n, &[])).collect(),
        Some(2),
        RepeatMode::Off,
        50,
    );
    active.adopt(&player);
    not_resumed(&mut active, &mut player, "restored");
}
