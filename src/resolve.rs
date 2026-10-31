//! Load resolution: turns the player's source-based commands into backend
//! commands right before they are sent. Local tracks become file targets;
//! Navidrome tracks become freshly signed stream URLs. The player never
//! sees credentials and the backend never sees track sources.
//!
//! No terminal and no I/O (signing only draws a random salt), so it is
//! shared by the app and by tests that drive a real backend.

use std::collections::VecDeque;

use crate::model::TrackSource;
use crate::navidrome;
use crate::playback::{EndReason, LoadId, PlaybackCommand, PlaybackEvent, PlaybackTarget};
use crate::player::{Player, PlayerCommand};

/// Error reason when a remote track is loaded without a Navidrome client.
pub const NAVIDROME_NOT_CONFIGURED: &str = "Navidrome is not configured";

/// Start of the status when remote tracks are skipped without a client.
pub const REMOTE_SKIPPED: &str = "Navidrome is not configured; skipped";

/// Resolves a track source into a backend target: a local path as is, or
/// a freshly signed stream URL for a Navidrome song. The error is a short
/// reason without URLs or credentials.
pub fn resolve_source(
    source: &TrackSource,
    navidrome: Option<&navidrome::Client>,
) -> Result<PlaybackTarget, String> {
    match source {
        TrackSource::Local { path } => Ok(PlaybackTarget::File(path.clone())),
        TrackSource::Navidrome { song_id } => {
            let client = navidrome.ok_or_else(|| NAVIDROME_NOT_CONFIGURED.to_owned())?;
            client
                .stream_url(song_id)
                .map(PlaybackTarget::Stream)
                .map_err(|e| e.to_string())
        }
    }
}

/// Converts one player command into a backend command. A load whose
/// source cannot be resolved is returned as `Err((load, reason))`.
pub fn resolve_command(
    command: PlayerCommand,
    navidrome: Option<&navidrome::Client>,
) -> Result<PlaybackCommand, (LoadId, String)> {
    match command.into_playback() {
        Ok(command) => Ok(command),
        Err((load, source)) => match resolve_source(&source, navidrome) {
            Ok(target) => Ok(PlaybackCommand::Load { load, target }),
            Err(reason) => {
                tracing::info!(%load, %reason, "could not resolve track source");
                Err((load, reason))
            }
        },
    }
}

/// Turns player commands into backend commands, resolving each load's
/// source. A load that cannot be resolved never reaches the backend: it is
/// reported to `player` as `TrackEnded { Error }`, and the player's
/// follow-up commands (usually the next load, or `Stop` at the end of the
/// queue) are processed in its place, before anything that followed it.
///
/// A missing Navidrome client fails every remote load the same way, so
/// only the first such failure advances the queue entry by entry; on a
/// second one in the same call the player jumps to the next local entry in
/// one pass ([`Player::skip_remote_entries`], status [`REMOTE_SKIPPED`]),
/// or stops if there is none. This bounds the work done for a queue of
/// thousands of remote entries.
pub fn dispatch(
    player: &mut Player,
    navidrome: Option<&navidrome::Client>,
    commands: Vec<PlayerCommand>,
) -> Vec<PlaybackCommand> {
    let mut work: VecDeque<PlayerCommand> = commands.into();
    let mut out = Vec::new();
    let mut remote_failed = false;
    while let Some(command) = work.pop_front() {
        match resolve_command(command, navidrome) {
            Ok(command) => out.push(command),
            Err((load, reason)) => {
                let not_configured = reason == NAVIDROME_NOT_CONFIGURED;
                let follow_up = if not_configured && remote_failed {
                    player.skip_remote_entries(load)
                } else {
                    remote_failed |= not_configured;
                    player.handle_event(PlaybackEvent::TrackEnded {
                        load,
                        reason: EndReason::Error(reason),
                    })
                };
                for command in follow_up.into_iter().rev() {
                    work.push_front(command);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Track, TrackMetadata};
    use crate::navidrome::client::parse_base_url;

    fn client() -> navidrome::Client {
        navidrome::Client::new(
            parse_base_url("https://h.example/nd").unwrap(),
            "u".into(),
            navidrome::Password::new("p".into()),
        )
        .unwrap()
    }

    #[test]
    fn local_sources_resolve_to_file_targets() {
        let source = TrackSource::Local {
            path: "/music/a.flac".into(),
        };
        assert_eq!(
            resolve_source(&source, None),
            Ok(PlaybackTarget::File("/music/a.flac".into()))
        );
        assert_eq!(
            resolve_source(&source, Some(&client())),
            Ok(PlaybackTarget::File("/music/a.flac".into()))
        );
    }

    #[test]
    fn remote_sources_need_a_client() {
        let source = TrackSource::Navidrome {
            song_id: "s".into(),
        };
        assert_eq!(
            resolve_source(&source, None),
            Err(NAVIDROME_NOT_CONFIGURED.to_owned())
        );
        assert!(matches!(
            resolve_source(&source, Some(&client())),
            Ok(PlaybackTarget::Stream(_))
        ));
    }

    #[test]
    fn dispatch_skips_unresolvable_loads_and_keeps_order() {
        let mut player = Player::new();
        let remote = player
            .enqueue(Track::navidrome("r".into(), TrackMetadata::default()))
            .unwrap();
        player.enqueue(Track::local("/m/b.flac".into(), TrackMetadata::default()));
        let mut commands = player.play_entry(remote);
        commands.push(PlayerCommand::SetVolume(50));
        let out = dispatch(&mut player, None, commands);
        let [
            PlaybackCommand::Load {
                target: PlaybackTarget::File(path),
                ..
            },
            PlaybackCommand::SetVolume(50),
        ] = out.as_slice()
        else {
            panic!("{out:?}");
        };
        assert_eq!(path.to_str(), Some("/m/b.flac"));
        assert_eq!(player.take_messages().len(), 1);
    }

    #[test]
    fn a_run_of_remote_tracks_without_a_client_stops_quickly() {
        use crate::player::RepeatMode;
        for repeat in [RepeatMode::Off, RepeatMode::All, RepeatMode::One] {
            let mut player = Player::new();
            player.set_repeat(repeat);
            for i in 0..10_000 {
                player.enqueue(Track::navidrome(format!("r{i}"), TrackMetadata::default()));
            }
            let started = std::time::Instant::now();
            let commands = player.toggle_pause();
            let out = dispatch(&mut player, None, commands);
            assert!(started.elapsed() < std::time::Duration::from_secs(1));
            assert_eq!(out, [PlaybackCommand::Stop], "{repeat:?}");
            let messages = player.take_messages();
            assert_eq!(messages.len(), 2, "{repeat:?}: {messages:?}");
            assert!(
                messages.last().unwrap().starts_with(REMOTE_SKIPPED),
                "{messages:?}"
            );
            assert_eq!(player.current_load(), None);
            assert_eq!(
                player.queue().position(player.queue().current().unwrap()),
                Some(1)
            );
        }
    }

    fn queue_of(player: &mut Player, spec: &str) -> Vec<crate::queue::QueueEntryId> {
        spec.chars()
            .enumerate()
            .map(|(i, c)| {
                let track = match c {
                    'L' => Track::local(format!("/m/L{i}.flac").into(), TrackMetadata::default()),
                    _ => Track::navidrome(format!("r{i}"), TrackMetadata::default()),
                };
                player.enqueue(track).unwrap()
            })
            .collect()
    }

    fn eof_of_current(player: &mut Player) -> Vec<PlaybackCommand> {
        let load = player.current_load().unwrap();
        player.handle_event(PlaybackEvent::Started { load });
        player.handle_event(PlaybackEvent::Position(std::time::Duration::from_secs(5)));
        let commands = player.handle_event(PlaybackEvent::TrackEnded {
            load,
            reason: EndReason::Eof,
        });
        dispatch(player, None, commands)
    }

    fn loaded_path(out: &[PlaybackCommand]) -> String {
        match out {
            [
                PlaybackCommand::Load {
                    target: PlaybackTarget::File(path),
                    ..
                },
            ] => path.to_string_lossy().into_owned(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn remote_runs_without_a_client_are_skipped_to_the_next_local_entry() {
        use crate::player::RepeatMode;
        // Off: [L1, R2, R3, L4] reaches L4 after L1.
        let mut player = Player::new();
        let ids = queue_of(&mut player, "LRRL");
        let commands = player.play_entry(ids[0]);
        dispatch(&mut player, None, commands);
        assert_eq!(loaded_path(&eof_of_current(&mut player)), "/m/L3.flac");
        assert_eq!(player.queue().current(), Some(ids[3]));

        // All: wraps to an earlier local entry.
        let mut player = Player::new();
        player.set_repeat(RepeatMode::All);
        let ids = queue_of(&mut player, "RLRRR");
        let commands = player.play_entry(ids[1]);
        dispatch(&mut player, None, commands);
        assert_eq!(loaded_path(&eof_of_current(&mut player)), "/m/L1.flac");
        let messages = player.take_messages();
        assert_eq!(
            messages.last().map(String::as_str),
            Some("Navidrome is not configured; skipped 3 remote tracks"),
            "{messages:?}"
        );

        // All, every entry remote: stops.
        let mut player = Player::new();
        player.set_repeat(RepeatMode::All);
        let ids = queue_of(&mut player, "RRRR");
        let commands = player.play_entry(ids[2]);
        assert_eq!(
            dispatch(&mut player, None, commands),
            [PlaybackCommand::Stop]
        );
        assert_eq!(player.current_load(), None);
    }
}
