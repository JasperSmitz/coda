use std::time::{Duration, Instant};

use super::*;
use crate::model::{Track, TrackMetadata};
use crate::playback::{LoadId, PlaybackEvent};
use crate::player::{PlayerCommand, RepeatMode};
use crate::queue::QueueEntryId;

fn track(name: &str, seconds: u64) -> Track {
    Track::local(
        format!("/m/{name}.flac").into(),
        TrackMetadata {
            title: Some(name.into()),
            duration: Some(Duration::from_secs(seconds)),
            ..TrackMetadata::default()
        },
    )
}

/// A player with `names` queued (100 s each), playing the first.
fn playing(names: &[&str]) -> (Player, Vec<QueueEntryId>, LoadId) {
    let mut player = Player::new();
    let ids: Vec<QueueEntryId> = names
        .iter()
        .map(|n| player.enqueue(track(n, 100)).unwrap())
        .collect();
    let load = start(&mut player, ids[0]);
    (player, ids, load)
}

fn start(player: &mut Player, id: QueueEntryId) -> LoadId {
    let load = match player.play_entry(id).as_slice() {
        [PlayerCommand::Load { load, .. }, ..] => *load,
        other => panic!("{other:?}"),
    };
    player.handle_event(PlaybackEvent::Started { load });
    player.handle_event(PlaybackEvent::Position(Duration::from_secs(2)));
    load
}

fn next_title(player: &Player) -> Option<String> {
    player.up_next().map(|e| e.track.title.clone())
}

fn shown(screen: &NowPlaying, player: &Player, now: Instant) -> Option<String> {
    screen.up_next(player, now).map(|e| e.track.title.clone())
}

#[test]
fn up_next_is_what_actually_plays_next_under_each_repeat_mode() {
    let (mut player, ids, _) = playing(&["A", "B", "C"]);
    assert_eq!(next_title(&player).as_deref(), Some("B"));
    start(&mut player, ids[2]);
    assert_eq!(next_title(&player), None, "the end of the queue");
    player.set_repeat(RepeatMode::All);
    assert_eq!(next_title(&player).as_deref(), Some("A"), "wraps round");
    player.set_repeat(RepeatMode::One);
    assert_eq!(next_title(&player), None, "the same track again");
    start(&mut player, ids[0]);
    assert_eq!(next_title(&player), None);
    player.set_repeat(RepeatMode::Off);
    assert_eq!(next_title(&player).as_deref(), Some("B"));
    // Nothing playing: nothing next.
    let mut idle = Player::new();
    idle.enqueue(track("A", 100));
    assert_eq!(next_title(&idle), None);
}

#[test]
fn up_next_follows_the_track_heard_during_a_gapless_change() {
    let (mut player, _, a) = playing(&["A", "B", "C"]);
    let Some(PlayerCommand::SetNext {
        next: Some((b, _)), ..
    }) = player.sync_next()
    else {
        panic!("B is announced");
    };
    player.handle_event(PlaybackEvent::Advanced { from: a, to: b });
    // A is still heard: B is what comes next.
    assert_eq!(player.playing_entry().unwrap().track.title, "A");
    assert_eq!(next_title(&player).as_deref(), Some("B"));
    player.handle_event(PlaybackEvent::Started { load: b });
    assert_eq!(next_title(&player).as_deref(), Some("C"));
}

#[test]
fn up_next_shows_in_the_last_twenty_seconds() {
    let (mut player, _, _) = playing(&["A", "B"]);
    let screen = NowPlaying::default();
    let now = Instant::now();
    player.handle_event(PlaybackEvent::Duration(Some(Duration::from_secs(100))));
    player.handle_event(PlaybackEvent::Position(Duration::from_secs(79)));
    assert_eq!(shown(&screen, &player, now), None);
    player.handle_event(PlaybackEvent::Position(Duration::from_secs(80)));
    assert_eq!(shown(&screen, &player, now).as_deref(), Some("B"));
    // The tags' duration counts until mpv reports one.
    player.handle_event(PlaybackEvent::Duration(None));
    assert_eq!(shown(&screen, &player, now).as_deref(), Some("B"));
}

#[test]
fn short_tracks_show_up_next_throughout_and_unknown_lengths_never() {
    let mut player = Player::new();
    let first = player.enqueue(track("Short", 15)).unwrap();
    player.enqueue(track("B", 100));
    start(&mut player, first);
    player.handle_event(PlaybackEvent::Position(Duration::ZERO));
    let screen = NowPlaying::default();
    let now = Instant::now();
    assert_eq!(shown(&screen, &player, now).as_deref(), Some("B"));

    let mut player = Player::new();
    let first = player
        .enqueue(Track::local("/m/x.flac".into(), TrackMetadata::default()))
        .unwrap();
    player.enqueue(track("B", 100));
    start(&mut player, first);
    assert_eq!(shown(&screen, &player, now), None);
}

#[test]
fn up_next_never_shows_with_repeat_one_or_at_the_end() {
    let (mut player, ids, _) = playing(&["A", "B"]);
    let now = Instant::now();
    let screen = NowPlaying::default();
    player.handle_event(PlaybackEvent::Duration(Some(Duration::from_secs(100))));
    player.handle_event(PlaybackEvent::Position(Duration::from_secs(95)));
    player.set_repeat(RepeatMode::One);
    assert_eq!(shown(&screen, &player, now), None);
    player.set_repeat(RepeatMode::Off);
    start(&mut player, ids[1]);
    player.handle_event(PlaybackEvent::Position(Duration::from_secs(95)));
    assert_eq!(shown(&screen, &player, now), None, "nothing follows");
    player.set_repeat(RepeatMode::All);
    assert_eq!(shown(&screen, &player, now).as_deref(), Some("A"));
}

#[test]
fn a_queue_change_shows_up_next_for_a_few_seconds() {
    let (mut player, _, _) = playing(&["A", "B"]);
    let mut screen = NowPlaying::default();
    let now = Instant::now();
    // The first look is not a change.
    screen.follow(&player, now);
    assert_eq!(shown(&screen, &player, now), None);
    // Unchanged: still nothing.
    screen.follow(&player, now);
    assert_eq!(shown(&screen, &player, now), None);
    // Something is put next (no key needed: an add can land later).
    player.enqueue_next(track("New", 100));
    screen.follow(&player, now);
    assert_eq!(shown(&screen, &player, now).as_deref(), Some("New"));
    let later = now + UP_NEXT_FLASH - Duration::from_millis(1);
    assert_eq!(shown(&screen, &player, later).as_deref(), Some("New"));
    let over = now + UP_NEXT_FLASH;
    assert_eq!(shown(&screen, &player, over), None);
    assert!(screen.expire(over), "the tick redraws it away");
    assert!(!screen.expire(over));
    // An append after the next entry changes nothing that shows.
    player.enqueue(track("Last", 100));
    screen.follow(&player, over);
    assert_eq!(shown(&screen, &player, over), None);
}

#[test]
fn a_skip_shows_up_next_and_a_track_ending_does_not() {
    let (mut player, _, a) = playing(&["A", "B", "C", "D"]);
    let mut screen = NowPlaying::default();
    let now = Instant::now();
    screen.follow(&player, now);
    // The track ends by itself and B is heard: no flash.
    let Some(PlayerCommand::SetNext {
        next: Some((b, _)), ..
    }) = player.sync_next()
    else {
        panic!("B is announced");
    };
    player.handle_event(PlaybackEvent::Advanced { from: a, to: b });
    screen.follow(&player, now);
    player.handle_event(PlaybackEvent::Started { load: b });
    screen.follow(&player, now);
    assert_eq!(next_title(&player).as_deref(), Some("C"));
    assert_eq!(shown(&screen, &player, now), None);
    // A skip (`>`) does.
    player.play_next();
    screen.user_acted();
    screen.follow(&player, now);
    assert_eq!(shown(&screen, &player, now).as_deref(), Some("D"));
}

#[test]
fn a_key_that_changes_nothing_next_shows_nothing() {
    let (player, _, _) = playing(&["A", "B"]);
    let mut screen = NowPlaying::default();
    let now = Instant::now();
    screen.follow(&player, now);
    screen.user_acted();
    screen.follow(&player, now);
    assert_eq!(shown(&screen, &player, now), None);
}

#[test]
fn the_volume_shows_for_two_seconds() {
    let mut screen = NowPlaying::default();
    let now = Instant::now();
    assert!(!screen.volume_shown(now));
    screen.volume_changed(now);
    assert!(screen.volume_shown(now + VOLUME_SHOWN - Duration::from_millis(1)));
    assert!(!screen.volume_shown(now + VOLUME_SHOWN));
    // Pressed again: two seconds from then.
    screen.volume_changed(now + Duration::from_secs(1));
    assert!(screen.volume_shown(now + VOLUME_SHOWN));
    assert!(!screen.expire(now + VOLUME_SHOWN));
    assert!(screen.expire(now + Duration::from_secs(3)));
}
