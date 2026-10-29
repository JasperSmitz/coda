//! Pure player coordinator: owns the [`Queue`] and the last reported
//! playback state, and turns user intents and [`PlaybackEvent`]s into
//! [`PlayerCommand`]s. No I/O and no Tokio.
//!
//! A [`PlayerCommand::Load`] names the queue entry's [`TrackSource`]; the
//! application resolves it into a backend [`PlaybackCommand`] (a local file
//! or a signed stream URL), so the player never handles credentials.
//!
//! The central rule: the queue advances only on
//! `TrackEnded { reason: Eof | Error }` whose `LoadId` equals
//! `current_load`. Every other `TrackEnded` is stale and ignored.

use std::time::Duration;

use crate::model::{Track, TrackSource};
use crate::playback::{EndReason, LoadId, PlaybackCommand, PlaybackEvent};
use crate::queue::{Queue, QueueEntryId};

/// `p` restarts the current track instead of going back when the position
/// is beyond this.
pub const RESTART_THRESHOLD: Duration = Duration::from_secs(3);
pub const VOLUME_STEP: u8 = 5;
pub const MAX_VOLUME: u8 = 100;
const DEFAULT_VOLUME: u8 = 100;

/// What the player asks of the playback backend. Mirrors
/// [`PlaybackCommand`], except that `Load` carries the track's source
/// rather than a resolved target.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerCommand {
    Load { load: LoadId, source: TrackSource },
    SetPause(bool),
    Stop,
    SeekRelative(f64),
    SeekAbsolute(f64),
    SetVolume(u8),
    Shutdown,
}

impl PlayerCommand {
    /// The backend command for everything except `Load`, which needs
    /// resolving first and is returned unchanged as `Err`.
    pub fn into_playback(self) -> Result<PlaybackCommand, (LoadId, TrackSource)> {
        Ok(match self {
            Self::Load { load, source } => return Err((load, source)),
            Self::SetPause(paused) => PlaybackCommand::SetPause(paused),
            Self::Stop => PlaybackCommand::Stop,
            Self::SeekRelative(seconds) => PlaybackCommand::SeekRelative(seconds),
            Self::SeekAbsolute(seconds) => PlaybackCommand::SeekAbsolute(seconds),
            Self::SetVolume(volume) => PlaybackCommand::SetVolume(volume),
            Self::Shutdown => PlaybackCommand::Shutdown,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    /// Nothing loaded.
    Stopped,
    /// A load was sent and its `Started` has not arrived yet.
    Loading,
    Playing,
    Paused,
    /// The backend exited; playback commands are no-ops.
    Unavailable,
}

#[derive(Debug)]
pub struct Player {
    queue: Queue,
    last_load: u64,
    current_load: Option<LoadId>,
    started: bool,
    paused: bool,
    position: Option<Duration>,
    duration: Option<Duration>,
    /// Last volume reported by the backend.
    volume: u8,
    /// The last requested volume, until mpv echoes it. Steps start from it,
    /// so late echoes of earlier requests cannot make rapid presses step
    /// from a stale value. mpv coalesces property changes (a burst of sets
    /// yields one echo of the final value; a burst ending on the previously
    /// reported value yields none), so its last echo is always the true
    /// value. Known gap: a late echo of an earlier request equal to the
    /// pending one clears it early; a following stale echo can then show
    /// briefly until mpv's final echo arrives.
    requested_volume: Option<u8>,
    unavailable: Option<String>,
    messages: Vec<String>,
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    pub fn new() -> Self {
        Self {
            queue: Queue::new(),
            last_load: 0,
            current_load: None,
            started: false,
            paused: false,
            position: None,
            duration: None,
            volume: DEFAULT_VOLUME,
            requested_volume: None,
            unavailable: None,
            messages: Vec::new(),
        }
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    pub fn current_load(&self) -> Option<LoadId> {
        self.current_load
    }

    pub fn status(&self) -> PlaybackStatus {
        if self.unavailable.is_some() {
            PlaybackStatus::Unavailable
        } else if self.current_load.is_none() {
            PlaybackStatus::Stopped
        } else if !self.started {
            PlaybackStatus::Loading
        } else if self.paused {
            PlaybackStatus::Paused
        } else {
            PlaybackStatus::Playing
        }
    }

    pub fn position(&self) -> Option<Duration> {
        self.position
    }

    pub fn duration(&self) -> Option<Duration> {
        self.duration
    }

    /// The effective volume: the pending request if there is one, else the
    /// last reported value.
    pub fn volume(&self) -> u8 {
        self.requested_volume.unwrap_or(self.volume)
    }

    /// The persistent reason playback is unavailable, if it is.
    pub fn unavailable_reason(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }

    /// User-visible messages produced since the last call.
    pub fn take_messages(&mut self) -> Vec<String> {
        std::mem::take(&mut self.messages)
    }

    fn is_loaded(&self) -> bool {
        self.current_load.is_some()
    }

    /// Loaded and opened by the backend, so seeking is meaningful.
    fn is_started(&self) -> bool {
        self.is_loaded() && self.started
    }

    /// Returns `true` (after recording a message) if playback is disabled.
    fn refuse_if_unavailable(&mut self) -> bool {
        match &self.unavailable {
            Some(reason) => {
                self.messages
                    .push(format!("Playback unavailable: {reason}"));
                true
            }
            None => false,
        }
    }

    // ----- user intents -------------------------------------------------

    /// Appends a track to the end of the queue without playing it.
    pub fn enqueue(&mut self, track: Track) -> QueueEntryId {
        self.queue.append(track)
    }

    /// Enter on a library track: insert a new entry directly after the
    /// current one (or append when there is none) and play it now. The
    /// previously current entry stays in place.
    pub fn play_track_now(&mut self, track: Track) -> Vec<PlayerCommand> {
        if self.refuse_if_unavailable() {
            return Vec::new();
        }
        let id = self.queue.insert_after_current(track);
        self.load_entry(id)
    }

    /// Enter on a queue entry: play it.
    pub fn play_entry(&mut self, id: QueueEntryId) -> Vec<PlayerCommand> {
        if self.refuse_if_unavailable() || self.queue.get(id).is_none() {
            return Vec::new();
        }
        self.load_entry(id)
    }

    /// Space: toggle pause when loaded; otherwise play the current entry,
    /// or the first entry when there is no current one.
    pub fn toggle_pause(&mut self) -> Vec<PlayerCommand> {
        if self.refuse_if_unavailable() {
            return Vec::new();
        }
        if self.is_loaded() {
            self.paused = !self.paused;
            return vec![PlayerCommand::SetPause(self.paused)];
        }
        match self.queue.current().or_else(|| self.queue.first()) {
            Some(id) => self.load_entry(id),
            None => {
                self.messages.push("Queue is empty".into());
                Vec::new()
            }
        }
    }

    /// `n`: play the next entry; at the end of the queue, do nothing.
    pub fn play_next(&mut self) -> Vec<PlayerCommand> {
        if self.refuse_if_unavailable() {
            return Vec::new();
        }
        if self.queue.current().is_none() {
            self.messages.push("Nothing is playing".into());
            return Vec::new();
        }
        match self.queue.move_to_next() {
            Some(id) => self.load_entry(id),
            None => {
                self.messages.push("End of queue".into());
                Vec::new()
            }
        }
    }

    /// `p`: restart the track if past [`RESTART_THRESHOLD`], else play the
    /// previous entry; at the first entry, restart the track. Restarting
    /// needs a started track; while loading it is a no-op.
    pub fn play_previous(&mut self) -> Vec<PlayerCommand> {
        if self.refuse_if_unavailable() {
            return Vec::new();
        }
        let past_threshold = self.position.is_some_and(|p| p > RESTART_THRESHOLD);
        if self.is_started() && past_threshold {
            return vec![PlayerCommand::SeekAbsolute(0.0)];
        }
        match self.queue.move_to_previous() {
            Some(id) => self.load_entry(id),
            None if self.is_started() => vec![PlayerCommand::SeekAbsolute(0.0)],
            None => Vec::new(),
        }
    }

    /// Seeks need a started track (mpv rejects seeks while a file is still
    /// opening); otherwise this is a no-op.
    pub fn seek_relative(&mut self, seconds: f64) -> Vec<PlayerCommand> {
        if self.refuse_if_unavailable() || !self.is_started() {
            return Vec::new();
        }
        vec![PlayerCommand::SeekRelative(seconds)]
    }

    pub fn volume_up(&mut self) -> Vec<PlayerCommand> {
        self.set_volume(self.volume().saturating_add(VOLUME_STEP).min(MAX_VOLUME))
    }

    pub fn volume_down(&mut self) -> Vec<PlayerCommand> {
        self.set_volume(self.volume().saturating_sub(VOLUME_STEP))
    }

    /// Requests `volume` unless it equals the effective volume: mpv sends
    /// no property change for an unchanged value, so such a request would
    /// never be confirmed (e.g. `+` at 100).
    fn set_volume(&mut self, volume: u8) -> Vec<PlayerCommand> {
        if self.refuse_if_unavailable() || !self.is_loaded() || volume == self.volume() {
            return Vec::new();
        }
        // A request that mpv never echoes (a burst ending on the value it
        // last reported) stays pending; that is harmless because it equals
        // mpv's actual value, and the next press replaces it.
        self.requested_volume = Some(volume);
        vec![PlayerCommand::SetVolume(volume)]
    }

    /// Removes a queue entry. Removing the current entry while loaded acts
    /// like `n` first; with no next entry, playback stops.
    pub fn remove(&mut self, id: QueueEntryId) -> Vec<PlayerCommand> {
        let mut commands = Vec::new();
        if self.queue.current() == Some(id) && self.is_loaded() {
            commands = match self.queue.id_after(id) {
                Some(next) => self.load_entry(next),
                None => self.stop(),
            };
        }
        self.queue.remove(id);
        commands
    }

    /// Quit: forget the current load first so its end-file is stale, then
    /// shut the backend down.
    pub fn quit(&mut self) -> Vec<PlayerCommand> {
        self.clear_load();
        vec![PlayerCommand::Shutdown]
    }

    // ----- backend events -----------------------------------------------

    pub fn handle_event(&mut self, event: PlaybackEvent) -> Vec<PlayerCommand> {
        match event {
            PlaybackEvent::Started { load } => {
                if self.current_load == Some(load) {
                    self.started = true;
                } else {
                    tracing::debug!(%load, "stale Started ignored");
                }
                Vec::new()
            }
            PlaybackEvent::TrackEnded { load, reason } => self.on_track_ended(load, reason),
            PlaybackEvent::Position(position) => {
                // Until the current load has started, positions belong to
                // the previous track.
                if self.is_loaded() && self.started {
                    self.position = Some(position);
                }
                Vec::new()
            }
            PlaybackEvent::Duration(duration) => {
                // Accepted while loading too: mpv may report the new
                // track's duration before `Started` reaches the player.
                if self.is_loaded() {
                    self.duration = duration;
                }
                Vec::new()
            }
            PlaybackEvent::Paused(paused) => {
                self.paused = paused;
                Vec::new()
            }
            PlaybackEvent::Volume(volume) => {
                // Always recorded; only the echo of the pending request
                // clears it. With nothing pending, a change is adopted.
                self.volume = volume.min(MAX_VOLUME);
                if self.requested_volume == Some(self.volume) {
                    self.requested_volume = None;
                }
                Vec::new()
            }
            PlaybackEvent::Error(message) => {
                self.messages.push(message);
                Vec::new()
            }
            PlaybackEvent::BackendExited(reason) => {
                tracing::warn!(%reason, "playback backend exited");
                self.clear_load();
                self.requested_volume = None;
                self.messages.push(format!("Playback stopped: {reason}"));
                self.unavailable = Some(reason);
                Vec::new()
            }
        }
    }

    fn on_track_ended(&mut self, load: LoadId, reason: EndReason) -> Vec<PlayerCommand> {
        if self.current_load != Some(load) {
            tracing::debug!(%load, ?reason, "stale TrackEnded ignored");
            return Vec::new();
        }
        match reason {
            EndReason::Eof => self.advance(false),
            EndReason::Error(message) => {
                let name = self
                    .queue
                    .current_entry()
                    .map(|e| e.track.display_name())
                    .unwrap_or_else(|| "track".into());
                tracing::info!(%load, track = %name, error = %message, "track failed");
                self.messages
                    .push(format!("Could not play {name}: {message}"));
                self.advance(true)
            }
            EndReason::Stopped => {
                // Stopped externally: keep the current entry, do not advance.
                self.clear_load();
                Vec::new()
            }
        }
    }

    /// Plays the next entry, or stops at the end of the queue with
    /// `current` left on the last entry so Space replays it.
    fn advance(&mut self, after_error: bool) -> Vec<PlayerCommand> {
        match self.queue.move_to_next() {
            Some(id) => self.load_entry(id),
            // After an error the backend may still be playing an older
            // track (a load rejected before reaching it), so stop it.
            None if after_error => self.stop(),
            None => {
                self.clear_load();
                Vec::new()
            }
        }
    }

    fn stop(&mut self) -> Vec<PlayerCommand> {
        // Cleared before `Stop` is emitted, so its end-file is stale.
        self.clear_load();
        vec![PlayerCommand::Stop]
    }

    fn clear_load(&mut self) {
        self.current_load = None;
        self.started = false;
        self.position = None;
        self.duration = None;
    }

    fn load_entry(&mut self, id: QueueEntryId) -> Vec<PlayerCommand> {
        let Some(entry) = self.queue.get(id) else {
            return Vec::new();
        };
        let source = entry.track.source.clone();
        let name = entry.track.display_name();
        self.queue.set_current(id);
        self.last_load += 1;
        let load = LoadId(self.last_load);
        tracing::info!(%load, track = %name, "loading track");
        self.current_load = Some(load);
        self.started = false;
        self.paused = false;
        self.position = None;
        self.duration = None;
        vec![PlayerCommand::Load { load, source }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackMetadata;
    use std::path::PathBuf;

    fn t(name: &str) -> Track {
        Track::local(format!("/m/{name}.flac").into(), TrackMetadata::default())
    }

    fn player_with(names: &[&str]) -> (Player, Vec<QueueEntryId>) {
        let mut p = Player::new();
        let ids = names.iter().map(|n| p.enqueue(t(n))).collect();
        (p, ids)
    }

    fn load_of(cmds: &[PlayerCommand]) -> (LoadId, PathBuf) {
        match cmds {
            [
                PlayerCommand::Load {
                    load,
                    source: TrackSource::Local { path },
                },
            ] => (*load, path.clone()),
            other => panic!("expected a single Load, got {other:?}"),
        }
    }

    fn titles(p: &Player) -> Vec<String> {
        let q = p.queue();
        q.entries()
            .iter()
            .map(|e| {
                let mark = if q.current() == Some(e.id) { "*" } else { "" };
                format!("{}{mark}", e.track.title)
            })
            .collect()
    }

    /// Plays entry `id` and delivers `Started` for it.
    fn start(p: &mut Player, id: QueueEntryId) -> LoadId {
        let (load, _) = load_of(&p.play_entry(id));
        p.handle_event(PlaybackEvent::Started { load });
        load
    }

    fn ended(load: LoadId, reason: EndReason) -> PlaybackEvent {
        PlaybackEvent::TrackEnded { load, reason }
    }

    #[test]
    fn every_load_gets_a_fresh_load_id() {
        let (mut p, ids) = player_with(&["A", "B"]);
        let (l1, path) = load_of(&p.play_entry(ids[0]));
        assert_eq!(path, PathBuf::from("/m/A.flac"));
        let (l2, _) = load_of(&p.play_entry(ids[0]));
        let (l3, _) = load_of(&p.play_next());
        assert!(l1 < l2 && l2 < l3);
        assert_eq!(p.current_load(), Some(l3));
        assert_eq!(p.status(), PlaybackStatus::Loading);
    }

    #[test]
    fn eof_on_current_advances() {
        let (mut p, ids) = player_with(&["A", "B"]);
        let l1 = start(&mut p, ids[0]);
        assert_eq!(p.status(), PlaybackStatus::Playing);
        let (l2, path) = load_of(&p.handle_event(ended(l1, EndReason::Eof)));
        assert_ne!(l1, l2);
        assert_eq!(path, PathBuf::from("/m/B.flac"));
        assert_eq!(p.queue().current(), Some(ids[1]));
    }

    #[test]
    fn error_on_current_advances_with_message() {
        let (mut p, ids) = player_with(&["A", "B"]);
        let l1 = start(&mut p, ids[0]);
        load_of(&p.handle_event(ended(l1, EndReason::Error("loading failed".into()))));
        assert_eq!(p.queue().current(), Some(ids[1]));
        let messages = p.take_messages();
        assert_eq!(messages, ["Could not play A: loading failed"]);
    }

    #[test]
    fn stopped_on_current_does_not_advance() {
        let (mut p, ids) = player_with(&["A", "B"]);
        let l1 = start(&mut p, ids[0]);
        assert!(p.handle_event(ended(l1, EndReason::Stopped)).is_empty());
        assert_eq!(p.status(), PlaybackStatus::Stopped);
        assert_eq!(p.queue().current(), Some(ids[0]));
        // Space replays the same entry.
        load_of(&p.toggle_pause());
        assert_eq!(p.queue().current(), Some(ids[0]));
    }

    #[test]
    fn stale_track_ended_after_user_played_another_track_is_ignored() {
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        let l1 = start(&mut p, ids[0]);
        // The user picks C; then A's EOF (raced) and A's stop arrive.
        let (l3, _) = load_of(&p.play_entry(ids[2]));
        for reason in [
            EndReason::Eof,
            EndReason::Error("x".into()),
            EndReason::Stopped,
        ] {
            assert!(p.handle_event(ended(l1, reason)).is_empty());
        }
        assert_eq!(p.queue().current(), Some(ids[2]));
        assert_eq!(p.current_load(), Some(l3));
        assert!(p.take_messages().is_empty());
        // A stale Started is ignored too.
        p.handle_event(PlaybackEvent::Started { load: l1 });
        assert_eq!(p.status(), PlaybackStatus::Loading);
    }

    #[test]
    fn deliberate_stop_end_file_is_ignored() {
        let (mut p, ids) = player_with(&["A"]);
        let l1 = start(&mut p, ids[0]);
        // Removing the current entry at the end of the queue stops.
        assert_eq!(p.remove(ids[0]), [PlayerCommand::Stop]);
        assert_eq!(p.current_load(), None);
        assert!(p.handle_event(ended(l1, EndReason::Stopped)).is_empty());
        assert!(p.handle_event(ended(l1, EndReason::Eof)).is_empty());

        let (mut p, ids) = player_with(&["A", "B"]);
        let l1 = start(&mut p, ids[0]);
        assert_eq!(p.quit(), [PlayerCommand::Shutdown]);
        assert!(p.handle_event(ended(l1, EndReason::Eof)).is_empty());
        assert_eq!(p.queue().current(), Some(ids[0]));
    }

    #[test]
    fn end_of_queue_stops_and_keeps_last_entry_current() {
        let (mut p, ids) = player_with(&["A", "B"]);
        let l1 = start(&mut p, ids[1]);
        assert!(p.handle_event(ended(l1, EndReason::Eof)).is_empty());
        assert_eq!(p.status(), PlaybackStatus::Stopped);
        assert_eq!(p.queue().current(), Some(ids[1]));
        load_of(&p.toggle_pause());
        assert_eq!(p.queue().current(), Some(ids[1]));
    }

    #[test]
    fn next_at_end_is_a_no_op_with_message() {
        let (mut p, ids) = player_with(&["A"]);
        let l1 = start(&mut p, ids[0]);
        assert!(p.play_next().is_empty());
        assert_eq!(p.take_messages(), ["End of queue"]);
        assert_eq!(p.current_load(), Some(l1));
    }

    #[test]
    fn next_without_current_is_a_no_op() {
        let (mut p, _) = player_with(&["A"]);
        assert!(p.play_next().is_empty());
        assert_eq!(p.take_messages(), ["Nothing is playing"]);
    }

    #[test]
    fn previous_restart_threshold() {
        let (mut p, ids) = player_with(&["A", "B"]);
        start(&mut p, ids[1]);
        p.handle_event(PlaybackEvent::Position(Duration::from_millis(3500)));
        assert_eq!(p.play_previous(), [PlayerCommand::SeekAbsolute(0.0)]);
        assert_eq!(p.queue().current(), Some(ids[1]));

        p.handle_event(PlaybackEvent::Position(Duration::from_secs(3)));
        let (_, path) = load_of(&p.play_previous());
        assert_eq!(path, PathBuf::from("/m/A.flac"));
    }

    #[test]
    fn previous_at_first_entry_seeks_to_zero() {
        let (mut p, ids) = player_with(&["A", "B"]);
        start(&mut p, ids[0]);
        p.handle_event(PlaybackEvent::Position(Duration::from_secs(1)));
        assert_eq!(p.play_previous(), [PlayerCommand::SeekAbsolute(0.0)]);
        // Stopped at the first entry: nothing to seek.
        let (mut p, _) = player_with(&["A"]);
        assert!(p.play_previous().is_empty());
    }

    #[test]
    fn seeks_are_no_ops_until_started() {
        let (mut p, ids) = player_with(&["A", "B"]);
        let (load, _) = load_of(&p.play_entry(ids[0]));
        assert_eq!(p.status(), PlaybackStatus::Loading);
        assert!(p.seek_relative(5.0).is_empty());
        // `p` at the first entry while loading: nothing to restart.
        assert!(p.play_previous().is_empty());
        assert_eq!(p.queue().current(), Some(ids[0]));
        p.handle_event(PlaybackEvent::Started { load });
        assert_eq!(p.seek_relative(5.0), [PlayerCommand::SeekRelative(5.0)]);
        assert_eq!(p.play_previous(), [PlayerCommand::SeekAbsolute(0.0)]);
        // `p` while loading a later entry still goes back.
        load_of(&p.play_next());
        let (_, path) = load_of(&p.play_previous());
        assert_eq!(path, PathBuf::from("/m/A.flac"));
    }

    #[test]
    fn duration_is_tracked_only_while_loaded() {
        let (mut p, ids) = player_with(&["A"]);
        p.handle_event(PlaybackEvent::Duration(Some(Duration::from_secs(9))));
        assert_eq!(p.duration(), None, "ignored while stopped");
        let (load, _) = load_of(&p.play_entry(ids[0]));
        p.handle_event(PlaybackEvent::Duration(Some(Duration::from_secs(3))));
        assert_eq!(
            p.duration(),
            Some(Duration::from_secs(3)),
            "accepted while loading"
        );
        p.handle_event(PlaybackEvent::Started { load });
        p.handle_event(PlaybackEvent::TrackEnded {
            load,
            reason: EndReason::Eof,
        });
        assert_eq!(p.duration(), None, "reset when the load ends");
    }

    #[test]
    fn space_from_stopped_and_empty() {
        let mut p = Player::new();
        assert!(p.toggle_pause().is_empty());
        assert_eq!(p.take_messages(), ["Queue is empty"]);

        let (mut p, ids) = player_with(&["A", "B"]);
        // No current entry: plays the first.
        load_of(&p.toggle_pause());
        assert_eq!(p.queue().current(), Some(ids[0]));
        p.handle_event(PlaybackEvent::Started {
            load: p.current_load().unwrap(),
        });
        assert_eq!(p.toggle_pause(), [PlayerCommand::SetPause(true)]);
        assert_eq!(p.status(), PlaybackStatus::Paused);
        assert_eq!(p.toggle_pause(), [PlayerCommand::SetPause(false)]);
        assert_eq!(p.status(), PlaybackStatus::Playing);
    }

    #[test]
    fn load_resets_pause() {
        let (mut p, ids) = player_with(&["A", "B"]);
        start(&mut p, ids[0]);
        p.toggle_pause();
        p.handle_event(PlaybackEvent::Paused(true));
        let (load, _) = load_of(&p.play_next());
        p.handle_event(PlaybackEvent::Started { load });
        assert_eq!(p.status(), PlaybackStatus::Playing);
    }

    #[test]
    fn seek_and_volume_need_a_loaded_track() {
        let (mut p, ids) = player_with(&["A"]);
        assert!(p.seek_relative(5.0).is_empty());
        assert!(p.volume_down().is_empty());
        start(&mut p, ids[0]);
        assert_eq!(p.seek_relative(-5.0), [PlayerCommand::SeekRelative(-5.0)]);
        assert_eq!(p.volume_up(), Vec::<PlayerCommand>::new(), "already at 100");
        assert_eq!(p.volume_down(), [PlayerCommand::SetVolume(95)]);
        p.handle_event(PlaybackEvent::Volume(95));
        p.handle_event(PlaybackEvent::Volume(3));
        assert_eq!(p.volume_down(), [PlayerCommand::SetVolume(0)]);
        assert!(p.volume_down().is_empty());
        assert_eq!(p.volume_up(), [PlayerCommand::SetVolume(5)]);
    }

    fn volume_presses(p: &mut Player, keys: &str) -> Vec<u8> {
        keys.chars()
            .flat_map(|k| match k {
                '-' => p.volume_down(),
                _ => p.volume_up(),
            })
            .map(|c| match c {
                PlayerCommand::SetVolume(v) => v,
                other => panic!("unexpected {other:?}"),
            })
            .collect()
    }

    #[test]
    fn rapid_volume_presses_step_from_the_last_request() {
        // Echoes lagging one press behind (the smoke failure: the 95 echo
        // arrived after the third `-`).
        let (mut p, ids) = player_with(&["A"]);
        start(&mut p, ids[0]);
        let mut sent = volume_presses(&mut p, "--");
        p.handle_event(PlaybackEvent::Volume(95));
        sent.extend(volume_presses(&mut p, "-"));
        p.handle_event(PlaybackEvent::Volume(90));
        sent.extend(volume_presses(&mut p, "+"));
        p.handle_event(PlaybackEvent::Volume(85));
        sent.extend(volume_presses(&mut p, "="));
        assert_eq!(sent, [95, 90, 85, 90, 95]);
        assert_eq!(p.volume(), 95, "stale echoes do not show");
        p.handle_event(PlaybackEvent::Volume(90));
        assert_eq!(p.volume(), 95);
        p.handle_event(PlaybackEvent::Volume(95));
        assert_eq!(p.volume(), 95);
        // Confirmed: nothing pending, so a later change is adopted.
        p.handle_event(PlaybackEvent::Volume(42));
        assert_eq!(p.volume(), 42);
    }

    #[test]
    fn coalesced_volume_echoes() {
        // A burst before any echo: mpv reports only the final value.
        let (mut p, ids) = player_with(&["A"]);
        start(&mut p, ids[0]);
        assert_eq!(volume_presses(&mut p, "---+="), [95, 90, 85, 90, 95]);
        assert_eq!(p.volume(), 95);
        p.handle_event(PlaybackEvent::Volume(95));
        assert_eq!(p.volume(), 95);
        p.handle_event(PlaybackEvent::Volume(60));
        assert_eq!(p.volume(), 60, "confirmed, so later changes are adopted");

        // There and back: mpv sends no echo at all. The pending request
        // equals mpv's value, and further presses step from it.
        let (mut p, ids) = player_with(&["A"]);
        start(&mut p, ids[0]);
        p.handle_event(PlaybackEvent::Volume(50));
        assert_eq!(volume_presses(&mut p, "+-"), [55, 50]);
        assert_eq!(p.volume(), 50);
        assert_eq!(volume_presses(&mut p, "+"), [55]);
        p.handle_event(PlaybackEvent::Volume(55));
        assert_eq!(p.volume(), 55);

        // Individual echoes, all delayed: a stale echo may show briefly,
        // but mpv's last echo is its true value and wins.
        let (mut p, ids) = player_with(&["A"]);
        start(&mut p, ids[0]);
        let sent = volume_presses(&mut p, "---+=");
        for v in &sent {
            p.handle_event(PlaybackEvent::Volume(*v));
        }
        assert_eq!(p.volume(), 95);
    }

    #[test]
    fn external_volume_change_is_adopted_when_nothing_is_pending() {
        let (mut p, ids) = player_with(&["A"]);
        start(&mut p, ids[0]);
        p.handle_event(PlaybackEvent::Volume(42));
        assert_eq!(p.volume(), 42);
        assert_eq!(volume_presses(&mut p, "+"), [47]);
    }

    #[test]
    fn volume_clamps_without_redundant_commands() {
        let (mut p, ids) = player_with(&["A"]);
        start(&mut p, ids[0]);
        // At 100: `+` sends nothing (mpv would never echo it).
        assert!(volume_presses(&mut p, "++").is_empty());
        // Pending 5 -> 0, then `-` at the requested 0 sends nothing.
        p.handle_event(PlaybackEvent::Volume(5));
        assert_eq!(volume_presses(&mut p, "--"), [0]);
        assert_eq!(p.volume(), 0);
        p.handle_event(PlaybackEvent::Volume(0));
        assert!(volume_presses(&mut p, "-").is_empty());
        // Up to 100 and stop there, even before the echoes arrive.
        p.handle_event(PlaybackEvent::Volume(97));
        assert_eq!(volume_presses(&mut p, "+++"), [100]);
    }

    #[test]
    fn backend_exit_clears_a_pending_volume_request() {
        let (mut p, ids) = player_with(&["A"]);
        start(&mut p, ids[0]);
        assert_eq!(volume_presses(&mut p, "-"), [95]);
        p.handle_event(PlaybackEvent::BackendExited("gone".into()));
        assert_eq!(p.volume(), 100, "falls back to the last reported value");
    }

    #[test]
    fn remove_current_while_loaded_plays_next() {
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        let l1 = start(&mut p, ids[1]);
        let (_, path) = load_of(&p.remove(ids[1]));
        assert_eq!(path, PathBuf::from("/m/C.flac"));
        assert_eq!(titles(&p), ["A", "C*"]);
        assert!(p.handle_event(ended(l1, EndReason::Stopped)).is_empty());
    }

    #[test]
    fn remove_current_at_end_stops() {
        let (mut p, ids) = player_with(&["A", "B"]);
        start(&mut p, ids[1]);
        assert_eq!(p.remove(ids[1]), [PlayerCommand::Stop]);
        assert_eq!(p.status(), PlaybackStatus::Stopped);
        assert_eq!(titles(&p), ["A*"]);
    }

    #[test]
    fn remove_non_current_or_while_stopped_emits_nothing() {
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        start(&mut p, ids[1]);
        assert!(p.remove(ids[0]).is_empty());
        assert!(p.remove(ids[2]).is_empty());
        assert_eq!(titles(&p), ["B*"]);

        let (mut p, ids) = player_with(&["A", "B"]);
        p.queue.set_current(ids[0]);
        assert!(p.remove(ids[0]).is_empty());
        assert_eq!(titles(&p), ["B*"]);
    }

    #[test]
    fn remote_entries_load_by_source_and_share_the_queue() {
        let mut p = Player::new();
        let remote = p.enqueue(Track::navidrome("s1".into(), TrackMetadata::default()));
        p.enqueue(t("A"));
        let cmds = p.play_entry(remote);
        let [PlayerCommand::Load { load, source }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        assert_eq!(
            *source,
            TrackSource::Navidrome {
                song_id: "s1".into()
            }
        );
        let (_, path) = load_of(&p.handle_event(ended(*load, EndReason::Eof)));
        assert_eq!(path, PathBuf::from("/m/A.flac"));
    }

    #[test]
    fn non_load_commands_map_one_to_one() {
        let cases = [
            (
                PlayerCommand::SetPause(true),
                PlaybackCommand::SetPause(true),
            ),
            (PlayerCommand::Stop, PlaybackCommand::Stop),
            (
                PlayerCommand::SeekRelative(-5.0),
                PlaybackCommand::SeekRelative(-5.0),
            ),
            (
                PlayerCommand::SeekAbsolute(0.0),
                PlaybackCommand::SeekAbsolute(0.0),
            ),
            (PlayerCommand::SetVolume(40), PlaybackCommand::SetVolume(40)),
            (PlayerCommand::Shutdown, PlaybackCommand::Shutdown),
        ];
        for (player, playback) in cases {
            assert_eq!(player.into_playback(), Ok(playback));
        }
        let source = TrackSource::Local {
            path: "/m/x".into(),
        };
        let load = PlayerCommand::Load {
            load: LoadId(3),
            source: source.clone(),
        };
        assert_eq!(load.into_playback(), Err((LoadId(3), source)));
    }

    #[test]
    fn all_missing_queue_terminates() {
        let names: Vec<String> = (0..20).map(|i| format!("T{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let (mut p, _) = player_with(&refs);
        let mut cmds = p.toggle_pause();
        let mut loads = 0;
        let mut steps = 0;
        while let [PlayerCommand::Load { load, .. }] = cmds.as_slice() {
            loads += 1;
            steps += 1;
            assert!(steps <= 100, "did not terminate");
            cmds = p.handle_event(ended(*load, EndReason::Error("file not found".into())));
        }
        assert_eq!(loads, 20);
        assert_eq!(cmds, [PlayerCommand::Stop]);
        assert_eq!(p.status(), PlaybackStatus::Stopped);
        assert_eq!(p.take_messages().len(), 20);
    }

    #[test]
    fn positions_before_started_are_ignored() {
        let (mut p, ids) = player_with(&["A", "B"]);
        start(&mut p, ids[0]);
        p.handle_event(PlaybackEvent::Position(Duration::from_secs(42)));
        let (load, _) = load_of(&p.play_next());
        assert_eq!(p.position(), None);
        p.handle_event(PlaybackEvent::Position(Duration::from_secs(43)));
        assert_eq!(p.position(), None);
        p.handle_event(PlaybackEvent::Started { load });
        p.handle_event(PlaybackEvent::Position(Duration::from_secs(1)));
        assert_eq!(p.position(), Some(Duration::from_secs(1)));
    }

    #[test]
    fn backend_exited_disables_playback() {
        let (mut p, ids) = player_with(&["A", "B"]);
        let l1 = start(&mut p, ids[0]);
        assert!(
            p.handle_event(PlaybackEvent::BackendExited("mpv exited".into()))
                .is_empty()
        );
        assert_eq!(p.status(), PlaybackStatus::Unavailable);
        assert_eq!(p.unavailable_reason(), Some("mpv exited"));
        assert_eq!(p.current_load(), None);
        p.take_messages();
        // Late events for the old load change nothing; nothing restarts.
        assert!(p.handle_event(ended(l1, EndReason::Eof)).is_empty());
        assert!(p.toggle_pause().is_empty());
        assert!(p.play_next().is_empty());
        assert!(p.play_previous().is_empty());
        assert!(p.play_entry(ids[1]).is_empty());
        assert!(p.play_track_now(t("X")).is_empty());
        assert!(p.seek_relative(5.0).is_empty());
        assert!(p.volume_down().is_empty());
        let messages = p.take_messages();
        assert_eq!(messages.len(), 7);
        assert!(
            messages
                .iter()
                .all(|m| m == "Playback unavailable: mpv exited")
        );
        assert_eq!(p.queue().len(), 2, "Enter did not modify the queue");
        assert_eq!(p.status(), PlaybackStatus::Unavailable);
    }

    // ----- Enter semantics (brief 3.2) -----------------------------------

    #[test]
    fn enter_inserts_after_current_and_plays() {
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        start(&mut p, ids[1]);
        let (_, path) = load_of(&p.play_track_now(t("X")));
        assert_eq!(path, PathBuf::from("/m/X.flac"));
        assert_eq!(titles(&p), ["A", "B", "X*", "C"]);
        // `p` returns to B.
        let (_, path) = load_of(&p.play_previous());
        assert_eq!(path, PathBuf::from("/m/B.flac"));
    }

    #[test]
    fn enter_after_stopped_last_entry() {
        let (mut p, ids) = player_with(&["A", "B"]);
        let load = start(&mut p, ids[1]);
        p.handle_event(ended(load, EndReason::Eof));
        assert_eq!(p.status(), PlaybackStatus::Stopped);
        load_of(&p.play_track_now(t("X")));
        assert_eq!(titles(&p), ["A", "B", "X*"]);
    }

    #[test]
    fn enter_without_current_appends() {
        let (mut p, _) = player_with(&["A", "B"]);
        load_of(&p.play_track_now(t("X")));
        assert_eq!(titles(&p), ["A", "B", "X*"]);
    }

    #[test]
    fn enter_on_empty_queue() {
        let mut p = Player::new();
        load_of(&p.play_track_now(t("X")));
        assert_eq!(titles(&p), ["X*"]);
    }

    #[test]
    fn enter_on_track_already_queued_adds_distinct_entry() {
        let (mut p, ids) = player_with(&["A", "X", "C"]);
        start(&mut p, ids[0]);
        load_of(&p.play_track_now(t("X")));
        assert_eq!(titles(&p), ["A", "X*", "X", "C"]);
        let current = p.queue().current().unwrap();
        assert_ne!(current, ids[1]);
        assert!(p.queue().get(ids[1]).is_some());
        assert_eq!(p.queue().position(ids[1]), Some(2));
    }
}
