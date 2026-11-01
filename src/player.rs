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
//!
//! Repeat ([`RepeatMode`]) changes only where an advance goes: `One`
//! reloads the same entry with a fresh `LoadId` on EOF, `All` wraps from
//! the last entry to the first. Errors never repeat a track, and under
//! `All` a run of failures as long as the queue stops playback.
//!
//! A load that reaches EOF without ever reporting a position of at least
//! [`PROVEN_PLAYBACK`] is a *suspect* end (an almost empty file, or a
//! stream that ends at once). It counts as a failure and is never repeated
//! by `One`, so repeat cannot reload as fast as mpv opens files (or hammer
//! a server). Position, unlike a reported duration, proves that real time
//! passed, which bounds reloads to about one per second without a clock.

use std::time::Duration;

use crate::model::{Track, TrackSource};
use crate::playback::{EndReason, LoadId, PlaybackCommand, PlaybackEvent};
use crate::queue::{MAX_QUEUE_LEN, Placement, Queue, QueueEntryId, Rng};

/// `p` restarts the current track instead of going back when the position
/// is beyond this.
pub const RESTART_THRESHOLD: Duration = Duration::from_secs(3);
pub const VOLUME_STEP: u8 = 5;
pub const MAX_VOLUME: u8 = 100;
/// mpv's volume at startup (it runs with `--no-config`).
pub const DEFAULT_VOLUME: u8 = 100;

/// Status text when every entry of a repeating queue failed in a row.
pub const NO_PLAYABLE_TRACKS: &str = "No playable tracks in queue";

/// Status text when an add is refused because the queue is full.
pub const QUEUE_FULL: &str = "Queue is full (10,000 entries)";

/// A load whose reported position reaches this has really played; see the
/// module docs on suspect ends.
pub const PROVEN_PLAYBACK: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

impl RepeatMode {
    /// `R`: off → all → one → off.
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::All => "all",
            Self::One => "one",
        }
    }
}

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
    /// The current load reported a position of at least [`PROVEN_PLAYBACK`].
    proven: bool,
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
    /// value. A rejection of the pending request drops it, falling back to
    /// the last reported value. Known gap: a late echo (or a rejection) of
    /// an earlier request equal to the pending one clears it early; a
    /// following stale echo can then show briefly until mpv's final echo
    /// arrives.
    requested_volume: Option<u8>,
    /// The volume to save: the user's latest request (or the restored
    /// value), or a change mpv reported on its own. A rejected request
    /// changes what is shown, not this.
    preferred_volume: u8,
    repeat: RepeatMode,
    /// Loads that failed or ended suspiciously since the last proven
    /// playback or user request. Under `All` it stops an endless loop over
    /// failing entries.
    failures_in_a_row: usize,
    /// Bumped when the volume or repeat mode changes; see [`Player::revision`].
    settings_revision: u64,
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
            proven: false,
            paused: false,
            position: None,
            duration: None,
            volume: DEFAULT_VOLUME,
            requested_volume: None,
            preferred_volume: DEFAULT_VOLUME,
            repeat: RepeatMode::Off,
            failures_in_a_row: 0,
            settings_revision: 0,
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

    /// The volume worth saving for the next session. It equals
    /// [`Self::volume`] except after a rejected request, where the display
    /// falls back to mpv's value but the user's preference is kept.
    pub fn preferred_volume(&self) -> u8 {
        self.preferred_volume
    }

    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    /// Increases whenever something worth saving changes: the queue, its
    /// current entry, the volume or the repeat mode.
    pub fn revision(&self) -> u64 {
        self.queue.revision() + self.settings_revision
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
    /// Returns `None` (and adds nothing) when the queue holds
    /// [`MAX_QUEUE_LEN`] entries.
    pub fn enqueue(&mut self, track: Track) -> Option<QueueEntryId> {
        (!self.is_queue_full()).then(|| self.queue.append(track))
    }

    /// `A` ("play next"): inserts a track after the current entry, behind
    /// earlier "play next" tracks (see [`Queue::insert_next`]); appends
    /// without a current entry. Never starts playback. Returns `None` (and
    /// adds nothing) when the queue is full.
    pub fn enqueue_next(&mut self, track: Track) -> Option<QueueEntryId> {
        (!self.is_queue_full()).then(|| self.queue.insert_next(track))
    }

    /// Adds `tracks` in order at `placement` (at the end, or as one "play
    /// next" run), up to [`MAX_QUEUE_LEN`] entries. Never starts playback.
    /// Returns how many were added.
    pub fn enqueue_all(&mut self, tracks: Vec<Track>, placement: Placement) -> usize {
        let room = MAX_QUEUE_LEN.saturating_sub(self.queue.len());
        let tracks = tracks.into_iter().take(room);
        match placement {
            Placement::End => tracks.map(|t| self.queue.append(t)).count(),
            Placement::Next => self.queue.insert_all_next(tracks).len(),
        }
    }

    pub fn is_queue_full(&self) -> bool {
        self.queue.len() >= MAX_QUEUE_LEN
    }

    /// Enter on a library track: insert a new entry directly after the
    /// current one (or append when there is none) and play it now. The
    /// previously current entry stays in place.
    pub fn play_track_now(&mut self, track: Track) -> Vec<PlayerCommand> {
        if self.refuse_if_unavailable() {
            return Vec::new();
        }
        if self.is_queue_full() {
            self.messages.push(QUEUE_FULL.into());
            return Vec::new();
        }
        let id = self.queue.insert_after_current(track);
        self.user_load(id)
    }

    /// Enter on a queue entry: play it.
    pub fn play_entry(&mut self, id: QueueEntryId) -> Vec<PlayerCommand> {
        if self.refuse_if_unavailable() || self.queue.get(id).is_none() {
            return Vec::new();
        }
        self.user_load(id)
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
            Some(id) => self.user_load(id),
            None => {
                self.messages.push("Queue is empty".into());
                Vec::new()
            }
        }
    }

    /// `n`: play the next entry; at the end of the queue, wrap around
    /// under [`RepeatMode::All`], else do nothing.
    pub fn play_next(&mut self) -> Vec<PlayerCommand> {
        if self.refuse_if_unavailable() {
            return Vec::new();
        }
        if self.queue.current().is_none() {
            self.messages.push("Nothing is playing".into());
            return Vec::new();
        }
        let next = self.queue.move_to_next().or_else(|| self.wrap_target());
        match next {
            Some(id) => self.user_load(id),
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
            Some(id) => self.user_load(id),
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
        self.preferred_volume = volume;
        self.settings_revision += 1;
        vec![PlayerCommand::SetVolume(volume)]
    }

    /// `R`: cycles the repeat mode. Allowed while playback is unavailable.
    pub fn cycle_repeat(&mut self) {
        self.set_repeat(self.repeat.next());
        self.messages
            .push(format!("Repeat {}", self.repeat.label()));
    }

    pub fn set_repeat(&mut self, mode: RepeatMode) {
        if self.repeat != mode {
            self.repeat = mode;
            self.settings_revision += 1;
        }
    }

    /// Moves an entry one place up. Never changes `current` or playback.
    pub fn move_up(&mut self, id: QueueEntryId) -> bool {
        self.queue.move_up(id)
    }

    /// Moves an entry one place down. Never changes `current` or playback.
    pub fn move_down(&mut self, id: QueueEntryId) -> bool {
        self.queue.move_down(id)
    }

    /// Shuffles the entries after the current one (all entries without a
    /// current one). Never changes `current` or playback. Returns the
    /// number of entries shuffled.
    pub fn shuffle_upcoming(&mut self, rng: &mut Rng) -> usize {
        self.queue.shuffle_after_current(|n| rng.below(n))
    }

    /// `C`: empties the queue. Playback stops like a deliberate stop: the
    /// load is forgotten before `Stop`, so its end-file is stale.
    pub fn clear_queue(&mut self) -> Vec<PlayerCommand> {
        let commands = if self.is_loaded() {
            self.stop()
        } else {
            Vec::new()
        };
        self.queue.clear();
        self.failures_in_a_row = 0;
        commands
    }

    /// Restores a saved session: appends `tracks`, makes `current` (an
    /// index into them) the current entry, and sets the repeat mode.
    /// Nothing starts playing. A `volume` other than mpv's startup volume
    /// is requested from the backend.
    pub fn restore(
        &mut self,
        tracks: Vec<Track>,
        current: Option<usize>,
        repeat: RepeatMode,
        volume: u8,
    ) -> Vec<PlayerCommand> {
        let ids: Vec<QueueEntryId> = tracks.into_iter().map_while(|t| self.enqueue(t)).collect();
        if let Some(id) = current.and_then(|i| ids.get(i)) {
            self.queue.set_current(*id);
        }
        self.set_repeat(repeat);
        let volume = volume.min(MAX_VOLUME);
        self.preferred_volume = volume;
        if self.unavailable.is_some() || volume == self.volume() {
            return Vec::new();
        }
        self.requested_volume = Some(volume);
        self.settings_revision += 1;
        vec![PlayerCommand::SetVolume(volume)]
    }

    /// Removes a queue entry. Removing the current entry while loaded acts
    /// like `n` first (wrapping under [`RepeatMode::All`] to the first
    /// other entry); with no next entry, playback stops.
    pub fn remove(&mut self, id: QueueEntryId) -> Vec<PlayerCommand> {
        let mut commands = Vec::new();
        if self.queue.current() == Some(id) && self.is_loaded() {
            let next = self
                .queue
                .id_after(id)
                .or_else(|| self.wrap_target().filter(|first| *first != id));
            commands = match next {
                Some(next) => self.user_load(next),
                None => self.stop(),
            };
        }
        self.queue.remove(id);
        commands
    }

    /// Remote entries cannot play (no Navidrome client) and `load`, the
    /// current one, is the second such failure in a row: instead of trying
    /// the following remote entries one by one, jump to the next local
    /// entry in a single pass (wrapping under [`RepeatMode::All`], like
    /// `n`). The current entry and the remote entries passed over count as
    /// failures in a row. Stops when no local entry remains, or (under
    /// `All`) once every entry has failed in a row.
    pub fn skip_remote_entries(&mut self, load: LoadId) -> Vec<PlayerCommand> {
        if self.current_load != Some(load) {
            return Vec::new();
        }
        let Some(pos) = self.queue.current().and_then(|id| self.queue.position(id)) else {
            return self.stop();
        };
        let entries = self.queue.entries();
        let wrap = if self.repeat == RepeatMode::All {
            pos
        } else {
            0
        };
        let mut skipped = 1;
        let mut target = None;
        for entry in entries[pos + 1..].iter().chain(&entries[..wrap]) {
            if entry.track.is_remote() {
                skipped += 1;
            } else {
                target = Some(entry.id);
                break;
            }
        }
        tracing::info!(%load, skipped, "remote entries skipped: Navidrome is not configured");
        self.messages.push(format!(
            "Navidrome is not configured; skipped {skipped} remote track{}",
            if skipped == 1 { "" } else { "s" }
        ));
        self.failures_in_a_row += skipped;
        let Some(target) = target else {
            self.failures_in_a_row = 0;
            return self.stop();
        };
        if self.repeat == RepeatMode::All && self.failures_in_a_row >= self.queue.len() {
            tracing::info!(
                failures = self.failures_in_a_row,
                "every queue entry failed"
            );
            self.messages.push(NO_PLAYABLE_TRACKS.into());
            self.failures_in_a_row = 0;
            return self.stop();
        }
        self.load_entry(target)
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
                    if position >= PROVEN_PLAYBACK && !self.proven {
                        self.proven = true;
                        self.failures_in_a_row = 0;
                    }
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
                let before = self.volume();
                let confirmed_before = self.volume;
                self.volume = volume.min(MAX_VOLUME);
                if self.requested_volume == Some(self.volume) {
                    self.requested_volume = None;
                }
                // A change with nothing pending was not asked for (or is
                // the final word on a burst): it becomes the preference. A
                // repeated report of the same value is not a change, so it
                // never overrides a preference kept after a rejection.
                if self.requested_volume.is_none() && self.volume != confirmed_before {
                    self.preferred_volume = self.volume;
                }
                if self.volume() != before {
                    self.settings_revision += 1;
                }
                Vec::new()
            }
            PlaybackEvent::VolumeRejected { volume, reason } => {
                tracing::warn!(volume, %reason, "volume change rejected");
                self.messages
                    .push(format!("Could not set volume: {reason}"));
                // Only a rejection of the pending request drops it: a later
                // request of a burst is still on its way. mpv applies sets in
                // order and echoes only its final value, so with nothing
                // pending any later echo is adopted as usual. The display
                // falls back; the saved preference is kept (as on
                // `BackendExited`), so a rejected restore never saves 100.
                if self.requested_volume == Some(volume) {
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
                // Keep the effective volume so it is still the one saved.
                self.volume = self.volume();
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
            EndReason::Eof if !self.proven => {
                tracing::info!(%load, "track ended without playing; not repeated");
                if let Some(stop) = self.count_failure() {
                    return stop;
                }
                self.advance(false)
            }
            EndReason::Eof if self.repeat == RepeatMode::One => match self.queue.current() {
                Some(id) => self.load_entry(id),
                None => {
                    self.clear_load();
                    Vec::new()
                }
            },
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
                if let Some(stop) = self.count_failure() {
                    return stop;
                }
                self.advance(true)
            }
            EndReason::Stopped => {
                // Stopped externally: keep the current entry, do not advance.
                self.clear_load();
                Vec::new()
            }
        }
    }

    /// Counts a failed or suspect end. Under [`RepeatMode::All`], once every
    /// entry has failed in a row, stops and returns the commands.
    fn count_failure(&mut self) -> Option<Vec<PlayerCommand>> {
        self.failures_in_a_row += 1;
        if self.repeat != RepeatMode::All || self.failures_in_a_row < self.queue.len() {
            return None;
        }
        tracing::info!(
            failures = self.failures_in_a_row,
            "every queue entry failed"
        );
        self.messages.push(NO_PLAYABLE_TRACKS.into());
        self.failures_in_a_row = 0;
        Some(self.stop())
    }

    /// Plays the next entry (wrapping to the first under
    /// [`RepeatMode::All`]), or stops at the end of the queue with
    /// `current` left on the last entry so Space replays it.
    fn advance(&mut self, after_error: bool) -> Vec<PlayerCommand> {
        let next = self.queue.move_to_next().or_else(|| self.wrap_target());
        match next {
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

    /// The first entry when repeating the whole queue, for an advance past
    /// the end.
    fn wrap_target(&self) -> Option<QueueEntryId> {
        (self.repeat == RepeatMode::All)
            .then(|| self.queue.first())
            .flatten()
    }

    fn stop(&mut self) -> Vec<PlayerCommand> {
        // Cleared before `Stop` is emitted, so its end-file is stale.
        self.clear_load();
        vec![PlayerCommand::Stop]
    }

    fn clear_load(&mut self) {
        self.current_load = None;
        self.started = false;
        self.proven = false;
        self.position = None;
        self.duration = None;
    }

    /// A load the user asked for: a fresh start for failure counting.
    fn user_load(&mut self, id: QueueEntryId) -> Vec<PlayerCommand> {
        self.failures_in_a_row = 0;
        self.load_entry(id)
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
        self.proven = false;
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
        let ids = names.iter().map(|n| p.enqueue(t(n)).unwrap()).collect();
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

    /// Plays entry `id`, delivers `Started` and a position that proves
    /// real playback.
    fn play_through(p: &mut Player, id: QueueEntryId) -> LoadId {
        let load = start(p, id);
        p.handle_event(PlaybackEvent::Position(PROVEN_PLAYBACK));
        load
    }

    /// Delivers `Started` and a proving position for a load just emitted.
    fn proven(p: &mut Player, cmds: &[PlayerCommand]) -> LoadId {
        let (load, _) = load_of(cmds);
        p.handle_event(PlaybackEvent::Started { load });
        p.handle_event(PlaybackEvent::Position(Duration::from_secs(2)));
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

    fn rejected(volume: u8) -> PlaybackEvent {
        PlaybackEvent::VolumeRejected {
            volume,
            reason: "property unavailable".into(),
        }
    }

    #[test]
    fn rejected_volume_falls_back_to_the_last_reported_value() {
        let (mut p, ids) = player_with(&["A"]);
        start(&mut p, ids[0]);
        p.handle_event(PlaybackEvent::Volume(50));
        p.take_messages();
        assert_eq!(p.preferred_volume(), 50, "mpv's own change is adopted");
        assert_eq!(volume_presses(&mut p, "+"), [55]);
        assert_eq!(p.volume(), 55);
        let revision = p.revision();
        assert!(p.handle_event(rejected(55)).is_empty());
        assert_eq!(p.volume(), 50, "the requested value is no longer shown");
        assert_eq!(
            p.take_messages(),
            ["Could not set volume: property unavailable"]
        );
        // The user's preference is what gets saved, and it did not change.
        assert_eq!(p.preferred_volume(), 55);
        assert_eq!(p.revision(), revision);
        // A repeated report of mpv's value does not override it.
        p.handle_event(PlaybackEvent::Volume(50));
        assert_eq!(p.preferred_volume(), 55);
        // The next press steps from the shown value.
        assert_eq!(volume_presses(&mut p, "+"), [55]);
        p.handle_event(PlaybackEvent::Volume(55));
        assert_eq!(p.volume(), 55);
        // Nothing pending afterwards: changes are adopted.
        p.handle_event(PlaybackEvent::Volume(30));
        assert_eq!(p.volume(), 30);
        assert_eq!(p.preferred_volume(), 30);
    }

    #[test]
    fn rejecting_an_earlier_request_of_a_burst_keeps_the_pending_one() {
        let (mut p, ids) = player_with(&["A"]);
        start(&mut p, ids[0]);
        assert_eq!(volume_presses(&mut p, "---"), [95, 90, 85]);
        p.handle_event(rejected(90));
        assert_eq!(p.volume(), 85, "85 is still on its way");
        assert_eq!(p.take_messages().len(), 1);
        p.handle_event(PlaybackEvent::Volume(95));
        assert_eq!(p.volume(), 85, "stale echo does not show");
        p.handle_event(PlaybackEvent::Volume(85));
        assert_eq!(p.volume(), 85);

        // The last request of a burst rejected: mpv's echo of the earlier
        // successful request (it may arrive on either side of the
        // rejection) ends up shown.
        for echo_first in [false, true] {
            let (mut p, ids) = player_with(&["A"]);
            start(&mut p, ids[0]);
            assert_eq!(volume_presses(&mut p, "--"), [95, 90]);
            if echo_first {
                p.handle_event(PlaybackEvent::Volume(95));
                assert_eq!(p.volume(), 90);
            }
            p.handle_event(rejected(90));
            if !echo_first {
                assert_eq!(p.volume(), 100, "last reported value until the echo");
                p.handle_event(PlaybackEvent::Volume(95));
            }
            assert_eq!(p.volume(), 95, "echo_first={echo_first}");
        }
    }

    #[test]
    fn rejected_restored_volume_falls_back() {
        let mut p = Player::new();
        let commands = p.restore(Vec::new(), None, RepeatMode::Off, 40);
        assert_eq!(commands, [PlayerCommand::SetVolume(40)]);
        assert_eq!(p.volume(), 40);
        // mpv's startup report may come on either side of the rejection.
        p.handle_event(PlaybackEvent::Volume(DEFAULT_VOLUME));
        let revision = p.revision();
        p.handle_event(rejected(40));
        assert_eq!(p.volume(), DEFAULT_VOLUME, "shows what mpv really has");
        p.handle_event(PlaybackEvent::Volume(DEFAULT_VOLUME));
        assert_eq!(p.volume(), DEFAULT_VOLUME);
        // The saved preference stays 40, and no save is triggered.
        assert_eq!(p.preferred_volume(), 40);
        assert_eq!(p.revision(), revision);
        assert_eq!(crate::session::Session::from_player(&p).volume, 40);
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
    fn backend_exit_keeps_the_effective_volume() {
        let (mut p, ids) = player_with(&["A"]);
        start(&mut p, ids[0]);
        assert_eq!(volume_presses(&mut p, "-"), [95]);
        let r = p.revision();
        p.handle_event(PlaybackEvent::BackendExited("gone".into()));
        assert_eq!(p.volume(), 95, "the request is what a save records");
        assert_eq!(p.revision(), r);
        // Nothing is pending any more: no later echo is waited for.
        assert!(p.volume_down().is_empty(), "unavailable");
        assert_eq!(p.volume(), 95);
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
        let remote = p
            .enqueue(Track::navidrome("s1".into(), TrackMetadata::default()))
            .unwrap();
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

    // ----- repeat ----------------------------------------------------------

    fn missing_queue(n: usize, repeat: RepeatMode) -> (usize, Vec<PlayerCommand>, Player) {
        let names: Vec<String> = (0..n).map(|i| format!("T{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let (mut p, _) = player_with(&refs);
        p.set_repeat(repeat);
        let mut cmds = p.toggle_pause();
        let mut loads = 0;
        while let [PlayerCommand::Load { load, .. }] = cmds.as_slice() {
            loads += 1;
            assert!(loads <= 10 * n + 10, "did not terminate");
            cmds = p.handle_event(ended(*load, EndReason::Error("file not found".into())));
        }
        (loads, cmds, p)
    }

    #[test]
    fn repeat_cycles_off_all_one_with_messages() {
        let mut p = Player::new();
        assert_eq!(p.repeat(), RepeatMode::Off);
        let r0 = p.revision();
        p.cycle_repeat();
        assert_eq!(p.repeat(), RepeatMode::All);
        p.cycle_repeat();
        assert_eq!(p.repeat(), RepeatMode::One);
        p.cycle_repeat();
        assert_eq!(p.repeat(), RepeatMode::Off);
        assert_eq!(
            p.take_messages(),
            ["Repeat all", "Repeat one", "Repeat off"]
        );
        assert!(p.revision() > r0);
        // Allowed while playback is unavailable.
        p.handle_event(PlaybackEvent::BackendExited("gone".into()));
        p.cycle_repeat();
        assert_eq!(p.repeat(), RepeatMode::All);
    }

    #[test]
    fn repeat_one_reloads_the_same_entry_with_a_fresh_load_id() {
        let (mut p, ids) = player_with(&["A", "B"]);
        p.set_repeat(RepeatMode::One);
        let l1 = play_through(&mut p, ids[0]);
        let (l2, path) = load_of(&p.handle_event(ended(l1, EndReason::Eof)));
        assert_ne!(l1, l2);
        assert_eq!(path, PathBuf::from("/m/A.flac"));
        assert_eq!(p.queue().current(), Some(ids[0]));
        assert_eq!(p.current_load(), Some(l2));
        // The old load's late events are stale.
        assert!(p.handle_event(ended(l1, EndReason::Eof)).is_empty());
        assert!(p.handle_event(ended(l1, EndReason::Stopped)).is_empty());
        assert_eq!(p.current_load(), Some(l2));
        // `n` and `p` behave as usual.
        let (_, path) = load_of(&p.play_next());
        assert_eq!(path, PathBuf::from("/m/B.flac"));
        assert!(p.play_next().is_empty());
        assert_eq!(p.take_messages(), ["End of queue"]);
    }

    #[test]
    fn repeat_one_error_advances_instead_of_looping() {
        let (mut p, ids) = player_with(&["A", "B"]);
        p.set_repeat(RepeatMode::One);
        let l1 = start(&mut p, ids[0]);
        let (_, path) = load_of(&p.handle_event(ended(l1, EndReason::Error("x".into()))));
        assert_eq!(path, PathBuf::from("/m/B.flac"));
        assert_eq!(p.queue().current(), Some(ids[1]));
    }

    #[test]
    fn repeat_all_wraps_on_eof_error_and_next() {
        let (mut p, ids) = player_with(&["A", "B"]);
        p.set_repeat(RepeatMode::All);
        let l1 = start(&mut p, ids[1]);
        let (_, path) = load_of(&p.handle_event(ended(l1, EndReason::Eof)));
        assert_eq!(path, PathBuf::from("/m/A.flac"));
        assert_eq!(p.queue().current(), Some(ids[0]));

        let l2 = start(&mut p, ids[1]);
        let (_, path) = load_of(&p.handle_event(ended(l2, EndReason::Error("x".into()))));
        assert_eq!(path, PathBuf::from("/m/A.flac"));

        start(&mut p, ids[1]);
        let (_, path) = load_of(&p.play_next());
        assert_eq!(path, PathBuf::from("/m/A.flac"));
        // `p` at the first entry still restarts rather than wrapping.
        let load = p.current_load().unwrap();
        p.handle_event(PlaybackEvent::Started { load });
        assert_eq!(p.play_previous(), [PlayerCommand::SeekAbsolute(0.0)]);
    }

    #[test]
    fn single_entry_repeat_all_reloads_it() {
        let (mut p, ids) = player_with(&["A"]);
        p.set_repeat(RepeatMode::All);
        let l1 = play_through(&mut p, ids[0]);
        let (l2, _) = load_of(&p.handle_event(ended(l1, EndReason::Eof)));
        assert_ne!(l1, l2);
        assert_eq!(p.queue().current(), Some(ids[0]));
    }

    #[test]
    fn all_missing_queue_terminates_under_every_repeat_mode() {
        for (repeat, expected_loads) in [
            (RepeatMode::Off, 7),
            (RepeatMode::All, 7),
            (RepeatMode::One, 7),
        ] {
            let (loads, cmds, mut p) = missing_queue(7, repeat);
            assert_eq!(loads, expected_loads, "{repeat:?}");
            assert_eq!(cmds, [PlayerCommand::Stop], "{repeat:?}");
            assert_eq!(p.status(), PlaybackStatus::Stopped, "{repeat:?}");
            let messages = p.take_messages();
            assert_eq!(
                messages.last().map(String::as_str) == Some(NO_PLAYABLE_TRACKS),
                repeat == RepeatMode::All,
                "{repeat:?}: {messages:?}"
            );
        }
    }

    #[test]
    fn repeat_all_failure_count_starts_mid_queue_and_resets_on_proven_playback() {
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        p.set_repeat(RepeatMode::All);
        // Start at C: C, A, B fail, then playback stops.
        let (mut load, _) = load_of(&p.play_entry(ids[2]));
        let mut failed = 0;
        loop {
            let cmds = p.handle_event(ended(load, EndReason::Error("x".into())));
            failed += 1;
            match cmds.as_slice() {
                [PlayerCommand::Load { load: next, .. }] => load = *next,
                [PlayerCommand::Stop] => break,
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(failed, 3);
        assert_eq!(p.queue().current(), Some(ids[1]));

        // Two failures, a real playback, then two more: never three in a
        // row. `Started` alone does not count as playing.
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        p.set_repeat(RepeatMode::All);
        let (l, _) = load_of(&p.play_entry(ids[0]));
        let (l, _) = load_of(&p.handle_event(ended(l, EndReason::Error("x".into()))));
        let (l, _) = load_of(&p.handle_event(ended(l, EndReason::Error("x".into()))));
        p.handle_event(PlaybackEvent::Started { load: l });
        p.handle_event(PlaybackEvent::Position(PROVEN_PLAYBACK));
        let (l, _) = load_of(&p.handle_event(ended(l, EndReason::Error("x".into()))));
        let (l, _) = load_of(&p.handle_event(ended(l, EndReason::Error("x".into()))));
        assert_eq!(p.current_load(), Some(l));
        assert_eq!(p.queue().current(), Some(ids[1]));
    }

    #[test]
    fn user_load_resets_the_failure_count() {
        let (mut p, ids) = player_with(&["A", "B"]);
        p.set_repeat(RepeatMode::All);
        let (l, _) = load_of(&p.play_entry(ids[0]));
        let (_, _) = load_of(&p.handle_event(ended(l, EndReason::Error("x".into()))));
        // The user picks A again: two more failures are allowed.
        let (l, _) = load_of(&p.play_entry(ids[0]));
        let (l, _) = load_of(&p.handle_event(ended(l, EndReason::Error("x".into()))));
        assert_eq!(
            p.handle_event(ended(l, EndReason::Error("x".into()))),
            [PlayerCommand::Stop]
        );
    }

    // ----- play next (`A`) ---------------------------------------------

    #[test]
    fn play_next_keeps_press_order_and_never_starts_playback() {
        let (mut p, ids) = player_with(&["A", "B"]);
        let load = start(&mut p, ids[0]);
        p.enqueue_next(t("X"));
        p.enqueue_next(t("Y"));
        assert_eq!(titles(&p), ["A*", "X", "Y", "B"]);
        assert_eq!(p.current_load(), Some(load));
        // Without a current entry it appends.
        let (mut p, _) = player_with(&["A"]);
        p.enqueue_next(t("X"));
        p.enqueue_next(t("Y"));
        assert_eq!(titles(&p), ["A", "X", "Y"]);
    }

    #[test]
    fn playback_moving_on_starts_a_new_play_next_run() {
        let (mut p, ids) = player_with(&["A", "B"]);
        let load = play_through(&mut p, ids[0]);
        p.enqueue_next(t("X"));
        p.enqueue_next(t("Y"));
        // A ends: X plays; the next `A` goes right after X, before Y.
        load_of(&p.handle_event(ended(load, EndReason::Eof)));
        p.enqueue_next(t("Z"));
        assert_eq!(titles(&p), ["A", "X*", "Z", "Y", "B"]);
    }

    #[test]
    fn play_next_with_shuffle_and_repeat() {
        // Shuffling forgets the run: the next insert is next again.
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        start(&mut p, ids[0]);
        p.enqueue_next(t("X"));
        let mut rng = Rng::from_seed(5);
        p.shuffle_upcoming(&mut rng);
        p.enqueue_next(t("Y"));
        assert_eq!(titles(&p)[..2], ["A*", "Y"]);
        // Repeat one replays the same entry: the run continues.
        let (mut p, ids) = player_with(&["A", "B"]);
        p.set_repeat(RepeatMode::One);
        let load = play_through(&mut p, ids[0]);
        p.enqueue_next(t("X"));
        let reload = p.handle_event(ended(load, EndReason::Eof));
        let mut load = proven(&mut p, &reload);
        p.enqueue_next(t("Y"));
        assert_eq!(titles(&p), ["A*", "X", "Y", "B"]);
        // Repeat all wrapping around to A moves on: a new run.
        p.set_repeat(RepeatMode::All);
        for _ in 0..4 {
            let next = p.handle_event(ended(load, EndReason::Eof));
            load = proven(&mut p, &next);
        }
        assert_eq!(titles(&p), ["A*", "X", "Y", "B"]);
        p.enqueue_next(t("Z"));
        assert_eq!(titles(&p), ["A*", "Z", "X", "Y", "B"]);
    }

    #[test]
    fn play_next_respects_the_queue_cap() {
        let mut p = Player::new();
        for i in 0..MAX_QUEUE_LEN - 2 {
            p.enqueue(t(&i.to_string()));
        }
        let first = p.queue().first().unwrap();
        start(&mut p, first);
        let added = p.enqueue_all(vec![t("X"), t("Y"), t("Z")], Placement::Next);
        assert_eq!(added, 2);
        assert_eq!(titles(&p)[..3], ["0*", "X", "Y"]);
        assert_eq!(p.enqueue_next(t("W")), None);
        assert_eq!(p.enqueue_all(vec![t("W")], Placement::End), 0);
        assert_eq!(p.queue().len(), MAX_QUEUE_LEN);
    }

    // ----- queue controls ------------------------------------------------

    #[test]
    fn moving_and_shuffling_never_touch_current_or_playback() {
        let (mut p, ids) = player_with(&["A", "B", "C", "D"]);
        let l1 = start(&mut p, ids[1]);
        assert!(p.move_up(ids[1]));
        assert!(!p.move_down(ids[3]), "already last");
        assert_eq!(titles(&p), ["B*", "A", "C", "D"]);
        let mut rng = Rng::from_seed(3);
        assert_eq!(p.shuffle_upcoming(&mut rng), 3);
        assert_eq!(p.queue().current(), Some(ids[1]));
        assert_eq!(p.current_load(), Some(l1));
        assert_eq!(p.status(), PlaybackStatus::Playing);
        assert_eq!(p.queue().position(ids[1]), Some(0));
    }

    #[test]
    fn clear_while_playing_stops_and_its_end_file_is_stale() {
        let (mut p, ids) = player_with(&["A", "B"]);
        let l1 = start(&mut p, ids[0]);
        assert_eq!(p.clear_queue(), [PlayerCommand::Stop]);
        assert_eq!(p.current_load(), None);
        assert!(p.queue().is_empty());
        for reason in [
            EndReason::Stopped,
            EndReason::Eof,
            EndReason::Error("x".into()),
        ] {
            assert!(p.handle_event(ended(l1, reason)).is_empty());
        }
        assert_eq!(p.status(), PlaybackStatus::Stopped);
        // Stopped: nothing to send.
        let (mut p, _) = player_with(&["A"]);
        assert!(p.clear_queue().is_empty());
        assert!(p.queue().is_empty());
    }

    // ----- restore -------------------------------------------------------

    #[test]
    fn restore_sets_queue_current_repeat_and_volume_without_playing() {
        let mut p = Player::new();
        let cmds = p.restore(vec![t("A"), t("B")], Some(1), RepeatMode::One, 40);
        assert_eq!(cmds, [PlayerCommand::SetVolume(40)]);
        assert_eq!(titles(&p), ["A", "B*"]);
        assert_eq!(p.repeat(), RepeatMode::One);
        assert_eq!(p.volume(), 40);
        assert_eq!(p.status(), PlaybackStatus::Stopped);
        // mpv's initial report of its startup volume does not undo it.
        p.handle_event(PlaybackEvent::Volume(100));
        assert_eq!(p.volume(), 40);
        p.handle_event(PlaybackEvent::Volume(40));
        assert_eq!(p.volume(), 40);
        // Space plays the restored current entry.
        let (_, path) = load_of(&p.toggle_pause());
        assert_eq!(path, PathBuf::from("/m/B.flac"));

        // The default volume needs no command; a bad index means no current.
        let mut p = Player::new();
        assert!(
            p.restore(vec![t("A")], Some(5), RepeatMode::Off, 100)
                .is_empty()
        );
        assert_eq!(p.queue().current(), None);
        assert_eq!(p.queue().len(), 1);
    }

    #[test]
    fn revision_tracks_saved_state_only() {
        let (mut p, ids) = player_with(&["A", "B"]);
        let r = p.revision();
        let l1 = start(&mut p, ids[0]);
        let r_loaded = p.revision();
        assert!(r_loaded > r, "current changed");
        p.handle_event(PlaybackEvent::Position(Duration::from_secs(3)));
        p.handle_event(PlaybackEvent::Paused(true));
        p.handle_event(PlaybackEvent::Volume(100));
        assert_eq!(p.revision(), r_loaded, "progress, pause, same volume");
        p.volume_down();
        let r_vol = p.revision();
        assert!(r_vol > r_loaded);
        p.handle_event(PlaybackEvent::Volume(95));
        assert_eq!(p.revision(), r_vol, "echo of the request changes nothing");
        p.handle_event(PlaybackEvent::Volume(50));
        assert!(p.revision() > r_vol, "external change");
        let r = p.revision();
        // Replaying the current entry does not change `current`.
        p.play_entry(ids[0]);
        assert_eq!(p.revision(), r);
        let _ = l1;
    }

    // ----- suspect ends, `d` under repeat, abandoning, queue cap ------------

    /// Every load starts and immediately reaches EOF, reporting at most
    /// `position`. Returns the number of loads before playback ended.
    fn instant_eofs(n: usize, repeat: RepeatMode, position: Duration) -> (usize, Player) {
        let names: Vec<String> = (0..n).map(|i| format!("T{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let (mut p, _) = player_with(&refs);
        p.set_repeat(repeat);
        let mut cmds = p.toggle_pause();
        let mut loads = 0;
        while let [PlayerCommand::Load { load, .. }] = cmds.as_slice() {
            loads += 1;
            assert!(loads <= 10 * n + 10, "{repeat:?}: did not terminate");
            p.handle_event(PlaybackEvent::Started { load: *load });
            p.handle_event(PlaybackEvent::Position(position));
            p.handle_event(PlaybackEvent::Duration(Some(Duration::from_secs(300))));
            cmds = p.handle_event(ended(*load, EndReason::Eof));
        }
        (loads, p)
    }

    #[test]
    fn tracks_that_end_at_once_are_never_hot_reloaded() {
        let almost = PROVEN_PLAYBACK - Duration::from_millis(1);
        // One: each suspect end advances, so the queue plays through once.
        let (loads, p) = instant_eofs(3, RepeatMode::One, almost);
        assert_eq!(loads, 3);
        assert_eq!(p.status(), PlaybackStatus::Stopped);
        // All: one round, then "No playable tracks". A reported duration
        // does not count as playing.
        let (loads, mut p) = instant_eofs(3, RepeatMode::All, almost);
        assert_eq!(loads, 3);
        assert_eq!(p.status(), PlaybackStatus::Stopped);
        assert_eq!(
            p.take_messages().last().map(String::as_str),
            Some(NO_PLAYABLE_TRACKS)
        );
        // A single entry under One or All plays once.
        for repeat in [RepeatMode::One, RepeatMode::All] {
            let (loads, _) = instant_eofs(1, repeat, Duration::ZERO);
            assert_eq!(loads, 1, "{repeat:?}");
        }
    }

    #[test]
    fn proven_playback_repeats() {
        let (mut p, ids) = player_with(&["A"]);
        p.set_repeat(RepeatMode::One);
        let mut load = play_through(&mut p, ids[0]);
        for _ in 0..5 {
            let cmds = p.handle_event(ended(load, EndReason::Eof));
            load = proven(&mut p, &cmds);
        }
        assert_eq!(p.queue().current(), Some(ids[0]));
        // A position before `Started` (the previous track's) proves nothing.
        let cmds = p.handle_event(ended(load, EndReason::Eof));
        let (load, _) = load_of(&cmds);
        p.handle_event(PlaybackEvent::Position(Duration::from_secs(9)));
        p.handle_event(PlaybackEvent::Started { load });
        assert!(p.handle_event(ended(load, EndReason::Eof)).is_empty());
        assert_eq!(p.status(), PlaybackStatus::Stopped);
    }

    #[test]
    fn removing_the_last_current_entry_wraps_under_repeat_all() {
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        p.set_repeat(RepeatMode::All);
        start(&mut p, ids[2]);
        let (_, path) = load_of(&p.remove(ids[2]));
        assert_eq!(path, PathBuf::from("/m/A.flac"));
        assert_eq!(titles(&p), ["A*", "B"]);
        // Without repeat it stops, as before.
        let (mut p, ids) = player_with(&["A", "B"]);
        start(&mut p, ids[1]);
        assert_eq!(p.remove(ids[1]), [PlayerCommand::Stop]);
        // The only entry: nothing to wrap to.
        let (mut p, ids) = player_with(&["A"]);
        p.set_repeat(RepeatMode::All);
        start(&mut p, ids[0]);
        assert_eq!(p.remove(ids[0]), [PlayerCommand::Stop]);
        assert!(p.queue().is_empty());
    }

    fn remote(id: &str) -> Track {
        Track::navidrome(id.into(), TrackMetadata::default())
    }

    fn any_load(cmds: &[PlayerCommand]) -> LoadId {
        match cmds {
            [PlayerCommand::Load { load, .. }] => *load,
            other => panic!("expected a single Load, got {other:?}"),
        }
    }

    fn mixed(p: &mut Player, spec: &str) -> Vec<QueueEntryId> {
        // `L` local, `R` remote, in order.
        spec.chars()
            .enumerate()
            .map(|(i, c)| match c {
                'L' => p.enqueue(t(&format!("L{i}"))).unwrap(),
                _ => p.enqueue(remote(&format!("R{i}"))).unwrap(),
            })
            .collect()
    }

    #[test]
    fn skipping_remote_entries_jumps_to_the_next_local_one() {
        // Off: [L1, R2, R3, L4] with R3 the second failure: L4 plays.
        let mut p = Player::new();
        let ids = mixed(&mut p, "LRRL");
        p.queue.set_current(ids[2]);
        let load = any_load(&p.play_entry(ids[2]));
        assert!(p.skip_remote_entries(LoadId(999)).is_empty(), "stale");
        let (_, path) = load_of(&p.skip_remote_entries(load));
        assert_eq!(path, PathBuf::from("/m/L3.flac"));
        assert_eq!(p.queue().current(), Some(ids[3]));
        assert_eq!(
            p.take_messages(),
            ["Navidrome is not configured; skipped 1 remote track"]
        );

        // Off: no local entry after it: stop, current left in place.
        let mut p = Player::new();
        let ids = mixed(&mut p, "LRRR");
        let load = any_load(&p.play_entry(ids[1]));
        assert_eq!(p.skip_remote_entries(load), [PlayerCommand::Stop]);
        assert_eq!(p.current_load(), None);
        assert_eq!(p.queue().current(), Some(ids[1]));
        assert_eq!(
            p.take_messages(),
            ["Navidrome is not configured; skipped 3 remote tracks"]
        );
        assert!(p.handle_event(ended(load, EndReason::Stopped)).is_empty());

        // All: wraps to an earlier local entry.
        let mut p = Player::new();
        p.set_repeat(RepeatMode::All);
        let ids = mixed(&mut p, "RLRRR");
        let load = any_load(&p.play_entry(ids[3]));
        let (_, path) = load_of(&p.skip_remote_entries(load));
        assert_eq!(path, PathBuf::from("/m/L1.flac"));
        assert_eq!(p.queue().current(), Some(ids[1]));

        // All, every entry remote: stops.
        let mut p = Player::new();
        p.set_repeat(RepeatMode::All);
        let ids = mixed(&mut p, "RRR");
        let load = any_load(&p.play_entry(ids[1]));
        assert_eq!(p.skip_remote_entries(load), [PlayerCommand::Stop]);
        assert_eq!(p.status(), PlaybackStatus::Stopped);
    }

    #[test]
    fn skipped_remote_entries_count_toward_repeat_all_termination() {
        // [R, R, L] under All where the local file is missing too: the run
        // of failures stops after one round instead of cycling forever.
        let mut p = Player::new();
        p.set_repeat(RepeatMode::All);
        let ids = mixed(&mut p, "RRL");
        let load = any_load(&p.play_entry(ids[0]));
        // R0 fails (first), R1 is the second and is skipped to L2.
        let load = any_load(&p.handle_event(ended(load, EndReason::Error("nc".into()))));
        let load = any_load(&p.skip_remote_entries(load));
        assert_eq!(
            p.handle_event(ended(load, EndReason::Error("file not found".into()))),
            [PlayerCommand::Stop]
        );
        assert_eq!(
            p.take_messages().last().map(String::as_str),
            Some(NO_PLAYABLE_TRACKS)
        );
    }

    #[test]
    fn queue_is_capped() {
        let mut p = Player::new();
        let tracks: Vec<Track> = (0..MAX_QUEUE_LEN + 3).map(|i| t(&i.to_string())).collect();
        p.restore(tracks, Some(MAX_QUEUE_LEN + 1), RepeatMode::Off, 100);
        assert_eq!(p.queue().len(), MAX_QUEUE_LEN);
        assert_eq!(p.queue().current(), None);
        assert!(p.is_queue_full());
        assert_eq!(p.enqueue(t("X")), None);
        assert!(p.play_track_now(t("X")).is_empty());
        assert_eq!(p.take_messages(), [QUEUE_FULL]);
        assert_eq!(p.queue().len(), MAX_QUEUE_LEN);
    }
}
