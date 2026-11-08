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
//!
//! Gapless playback (M8, ADR 0005): [`Player::sync_next`] announces the
//! entry that follows the current load ([`PlayerCommand::SetNext`], with
//! a fresh `LoadId`) whenever it changes: the queue's next entry, the first
//! under [`RepeatMode::All`], or the current entry again under
//! [`RepeatMode::One`] once the current load is proven (so an instant-EOF
//! file is never repeated). The backend may play it on by itself and say
//! so with [`PlaybackEvent::Advanced`]. An advance to the announced load
//! moves the queue on exactly like an EOF would (a suspect end counts as a
//! failure); an advance to anything else (the queue changed while mpv was
//! already starting the old next entry) is corrected by loading the right
//! entry with a `Load`, so a removed track never plays on. Until the new
//! load is heard (`Started`), the previous track's tail is still playing:
//! [`Player::status`], [`Player::position`], [`Player::playing_entry`]
//! and [`Player::audible_load`] describe what is heard, not the queue's
//! current entry.

use std::time::Duration;

use crate::model::{Track, TrackSource};
use crate::playback::{EndReason, LoadId, PlaybackCommand, PlaybackEvent};
use crate::queue::{MAX_QUEUE_LEN, Placement, Queue, QueueEntry, QueueEntryId, Rng};

/// `<` restarts the current track instead of going back when the position
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

/// The part of a context of `len` tracks that fits in the queue when it
/// is played from `start` ([`Player::play_context`]): everything when it
/// fits; otherwise [`MAX_QUEUE_LEN`] tracks holding `start`, dropping
/// tracks before it first and then from the end.
pub fn context_window(len: usize, start: usize) -> std::ops::Range<usize> {
    let excess = len.saturating_sub(MAX_QUEUE_LEN);
    let from = excess.min(start);
    from..len - (excess - from)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

impl RepeatMode {
    /// `r`: off → all → one → off.
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
    Load {
        load: LoadId,
        source: TrackSource,
    },
    /// The entry to play without a gap after `after` (`None`: nothing).
    SetNext {
        after: LoadId,
        next: Option<(LoadId, TrackSource)>,
    },
    SetPause(bool),
    Stop,
    SeekRelative(f64),
    SeekAbsolute(f64),
    SetVolume(u8),
}

impl PlayerCommand {
    /// The backend command for everything that names no track source.
    /// `Load` and a `SetNext` with a next entry need resolving first and
    /// are returned unchanged as `Err`.
    pub fn into_playback(self) -> Result<PlaybackCommand, Self> {
        Ok(match self {
            Self::Load { .. } | Self::SetNext { next: Some(_), .. } => return Err(self),
            Self::SetNext { after, next: None } => PlaybackCommand::SetNext { after, next: None },
            Self::SetPause(paused) => PlaybackCommand::SetPause(paused),
            Self::Stop => PlaybackCommand::Stop,
            Self::SeekRelative(seconds) => PlaybackCommand::SeekRelative(seconds),
            Self::SeekAbsolute(seconds) => PlaybackCommand::SeekAbsolute(seconds),
            Self::SetVolume(volume) => PlaybackCommand::SetVolume(volume),
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

/// The next entry announced to the backend after the load `after`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Announced {
    after: LoadId,
    entry: Option<QueueEntryId>,
    load: Option<LoadId>,
}

/// Earlier announcements of local entries after the current load that are
/// remembered (see [`Player::on_advanced`]).
const MAX_EARLIER_NEXT: usize = 8;

/// The track still heard after a gapless advance, until the next one is.
#[derive(Debug, Clone, Copy)]
struct Outgoing {
    load: LoadId,
    entry: Option<QueueEntryId>,
    position: Option<Duration>,
    duration: Option<Duration>,
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
    /// What [`Self::sync_next`] last announced.
    announced: Option<Announced>,
    /// Earlier announcements of local entries after the same load, newest
    /// last: an advance to one of them is the same file as the entry now
    /// announced, if that entry is the same.
    earlier_next: Vec<(LoadId, QueueEntryId)>,
    /// After a gapless advance, the previous track until the new one is
    /// heard.
    outgoing: Option<Outgoing>,
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
            announced: None,
            earlier_next: Vec::new(),
            outgoing: None,
        }
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    pub fn current_load(&self) -> Option<LoadId> {
        self.current_load
    }

    /// What is heard: after a gapless advance, the previous track still
    /// plays until the current load is heard.
    pub fn status(&self) -> PlaybackStatus {
        if self.unavailable.is_some() {
            PlaybackStatus::Unavailable
        } else if self.outgoing.is_some() {
            if self.paused {
                PlaybackStatus::Paused
            } else {
                PlaybackStatus::Playing
            }
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

    /// The position of the track heard (see [`Self::status`]).
    pub fn position(&self) -> Option<Duration> {
        self.outgoing.map_or(self.position, |o| o.position)
    }

    /// The duration of the track heard (see [`Self::status`]).
    pub fn duration(&self) -> Option<Duration> {
        self.outgoing.map_or(self.duration, |o| o.duration)
    }

    /// The load heard: the current load, or during a gapless change the
    /// previous one until the current one is heard. Visualizer readings
    /// are shown for it.
    pub fn audible_load(&self) -> Option<LoadId> {
        self.outgoing.map(|o| o.load).or(self.current_load)
    }

    /// The queue entry heard (see [`Self::audible_load`]); the queue's
    /// current entry unless a gapless change is under way.
    pub fn playing_entry(&self) -> Option<&QueueEntry> {
        self.outgoing
            .and_then(|o| self.queue.get(o.entry?))
            .or_else(|| self.queue.current_entry())
    }

    /// The entry that will play after the one heard
    /// ([`Self::playing_entry`]): the next in the queue, or with repeat all
    /// the first after the last. `None` with repeat one (the same track
    /// plays again), at the end of the queue, and when nothing plays.
    pub fn up_next(&self) -> Option<&QueueEntry> {
        let playing = self.playing_entry()?;
        let next = match self.repeat {
            RepeatMode::One => return None,
            RepeatMode::Off => self.queue.id_after(playing.id)?,
            RepeatMode::All => self
                .queue
                .id_after(playing.id)
                .or_else(|| self.queue.first())?,
        };
        self.queue.get(next)
    }

    /// If `Started { load }` would end a gapless change, the load heard
    /// until then.
    pub fn handover_from(&self, load: LoadId) -> Option<LoadId> {
        self.outgoing
            .filter(|_| self.current_load == Some(load))
            .map(|o| o.load)
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

    /// Enter (or `s`) in the library: replaces the queue with `tracks`
    /// and plays the one at `start` (clamped to the last). A context
    /// longer than [`MAX_QUEUE_LEN`] keeps the window
    /// [`context_window`] picks, which always holds the start track.
    /// Like any user load, the track gets a fresh `LoadId`, so events of
    /// the replaced load are stale; the repeat mode is kept. Does nothing
    /// for an empty context, or while playback is unavailable (the queue
    /// is then left as it was).
    pub fn play_context(&mut self, tracks: Vec<Track>, start: usize) -> Vec<PlayerCommand> {
        if self.refuse_if_unavailable() || tracks.is_empty() {
            return Vec::new();
        }
        let start = start.min(tracks.len() - 1);
        let window = context_window(tracks.len(), start);
        self.queue.clear();
        let ids: Vec<QueueEntryId> = tracks
            .into_iter()
            .skip(window.start)
            .take(window.len())
            .map(|track| self.queue.append(track))
            .collect();
        self.user_load(ids[start - window.start])
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

    /// `>`: play the next entry; at the end of the queue, wrap around
    /// under [`RepeatMode::All`], else do nothing.
    pub fn play_next(&mut self) -> Vec<PlayerCommand> {
        if self.refuse_if_unavailable() {
            return Vec::new();
        }
        // During a gapless change the track heard is the previous entry:
        // its next one is the queue's current entry, not heard yet. Load it
        // at once (cutting the previous track's tail).
        if self.outgoing.is_some()
            && let Some(current) = self.queue.current()
        {
            return self.user_load(current);
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

    /// `<`: restart the track if past [`RESTART_THRESHOLD`], else play the
    /// previous entry; at the first entry, restart the track. Restarting
    /// needs a started track; while loading it is a no-op.
    pub fn play_previous(&mut self) -> Vec<PlayerCommand> {
        if self.refuse_if_unavailable() {
            return Vec::new();
        }
        // During a gapless change the previous entry is still heard: `<`
        // restarts or leaves that one. A seek would go to the new track, so
        // a restart is a load.
        if let Some(outgoing) = self.outgoing
            && let Some(heard) = outgoing.entry.filter(|id| self.queue.get(*id).is_some())
        {
            let past_threshold = outgoing.position.is_some_and(|p| p > RESTART_THRESHOLD);
            let target = match self.queue.id_before(heard) {
                Some(previous) if !past_threshold => previous,
                _ => heard,
            };
            return self.user_load(target);
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

    /// `r`: cycles the repeat mode. Allowed while playback is unavailable.
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
    /// like `>` first (wrapping under [`RepeatMode::All`] to the first
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
    /// `>`). The current entry and the remote entries passed over count as
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
            "{} {skipped} remote track{}",
            crate::resolve::REMOTE_SKIPPED,
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

    // ----- gapless lookahead ------------------------------------------------

    /// The entry to announce after the current load, if any: see the
    /// module docs.
    fn next_target(&self) -> Option<QueueEntryId> {
        let current = self.queue.current()?;
        if self.repeat == RepeatMode::One && self.proven {
            return Some(current);
        }
        self.queue.id_after(current).or_else(|| self.wrap_target())
    }

    /// Announces the next entry to the backend if it changed since the last
    /// announcement (the queue, the repeat mode, the current load or its
    /// proof of playback changed). Call it after every change; the app
    /// does so after each batch of player commands. A fresh load needs no
    /// announcement of "nothing next": the backend holds none after it.
    pub fn sync_next(&mut self) -> Option<PlayerCommand> {
        let after = match self.current_load {
            Some(load) if self.unavailable.is_none() => load,
            _ => {
                self.announced = None;
                return None;
            }
        };
        let entry = self.next_target();
        let previous = self.announced.filter(|a| a.after == after);
        if previous.is_some_and(|a| a.entry == entry) {
            return None;
        }
        match previous {
            None => self.earlier_next.clear(),
            Some(previous) => {
                let local = previous
                    .entry
                    .and_then(|id| self.queue.get(id))
                    .is_some_and(|e| !e.track.is_remote());
                if let (Some(load), Some(id), true) = (previous.load, previous.entry, local) {
                    if self.earlier_next.len() >= MAX_EARLIER_NEXT {
                        self.earlier_next.remove(0);
                    }
                    self.earlier_next.push((load, id));
                }
            }
        }
        let next = entry.and_then(|id| {
            let source = self.queue.get(id)?.track.source.clone();
            self.last_load += 1;
            Some((LoadId(self.last_load), source))
        });
        self.announced = Some(Announced {
            after,
            entry,
            load: next.as_ref().map(|(load, _)| *load),
        });
        if next.is_none() && previous.is_none_or(|a| a.load.is_none()) {
            return None;
        }
        match &next {
            Some((load, _)) => tracing::debug!(%after, %load, "announcing the next track"),
            None => tracing::debug!(%after, "no next track any more"),
        }
        Some(PlayerCommand::SetNext { after, next })
    }

    /// The Navidrome client changed: stream URLs signed by the old one
    /// must not start playing. An announced remote next entry is announced
    /// again (or cleared, if it can no longer be resolved). During a
    /// gapless change to a remote entry, that entry was opened with the old
    /// client and is not heard yet: it is loaded again, like a stale
    /// advance. The track heard plays on.
    pub fn client_changed(&mut self) -> Vec<PlayerCommand> {
        let remote = self
            .announced
            .and_then(|a| self.queue.get(a.entry?))
            .is_some_and(|e| e.track.is_remote());
        if remote {
            self.announced = None;
        }
        self.earlier_next.clear();
        let unheard_remote = self.outgoing.is_some()
            && self
                .queue
                .current_entry()
                .is_some_and(|e| e.track.is_remote());
        match self.queue.current() {
            Some(current) if unheard_remote => {
                tracing::info!("next track opened with the previous client; loading it again");
                // A load always starts playing; a paused player stays paused.
                let paused = self.paused;
                let mut commands = self.load_entry(current);
                if paused {
                    self.paused = true;
                    commands.push(PlayerCommand::SetPause(true));
                }
                commands
            }
            _ => Vec::new(),
        }
    }

    /// The backend went on from `from` to `to` without a gap.
    fn on_advanced(&mut self, from: LoadId, to: LoadId) -> Vec<PlayerCommand> {
        if self.current_load != Some(from) {
            tracing::debug!(%from, %to, "stale Advanced ignored");
            return Vec::new();
        }
        // The announced load; or an earlier announcement of the same local
        // entry (the queue changed and changed back while mpv was already
        // starting it): the same file, so it plays on without a cut.
        let announced = self
            .announced
            .filter(|a| a.after == from)
            .and_then(|a| {
                let entry = a.entry?;
                let same = a.load == Some(to) || self.earlier_next.contains(&(to, entry));
                same.then_some(entry)
            })
            .filter(|id| self.queue.get(*id).is_some());
        let Some(entry) = announced else {
            // The queue changed while mpv was already starting the old next
            // entry: play what the queue says instead, as after an EOF.
            tracing::info!(%from, %to, "mpv went on to an entry that is no longer next; correcting");
            let mut commands = self.on_track_ended(from, EndReason::Eof);
            if !commands
                .iter()
                .any(|c| matches!(c, PlayerCommand::Load { .. } | PlayerCommand::Stop))
            {
                // Nothing to play: the stale entry must not play on.
                commands.push(PlayerCommand::Stop);
            }
            return commands;
        };
        if !self.proven {
            tracing::info!(%from, "track ended without playing; not repeated");
            if let Some(stop) = self.count_failure() {
                return stop;
            }
        }
        let outgoing = Outgoing {
            load: from,
            entry: self.queue.current(),
            position: self.position,
            duration: self.duration,
        };
        self.queue.set_current(entry);
        tracing::info!(load = %to, "playing the next track without a gap");
        self.current_load = Some(to);
        self.started = false;
        self.proven = false;
        self.position = None;
        self.duration = None;
        self.outgoing = Some(outgoing);
        Vec::new()
    }

    // ----- backend events -----------------------------------------------

    pub fn handle_event(&mut self, event: PlaybackEvent) -> Vec<PlayerCommand> {
        match event {
            PlaybackEvent::Started { load } => {
                if self.current_load == Some(load) {
                    self.started = true;
                    self.outgoing = None;
                } else {
                    tracing::debug!(%load, "stale Started ignored");
                }
                Vec::new()
            }
            PlaybackEvent::TrackEnded { load, reason } => self.on_track_ended(load, reason),
            PlaybackEvent::Advanced { from, to } => self.on_advanced(from, to),
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
            // Display only; the application keeps the visualizer state.
            PlaybackEvent::Visualizer { .. } | PlaybackEvent::VisualizerUnavailable => Vec::new(),
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
                // Never the track's name: titles and paths stay out of logs.
                tracing::info!(%load, error = %message, "track failed");
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
        self.outgoing = None;
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
        self.queue.set_current(id);
        self.last_load += 1;
        let load = LoadId(self.last_load);
        let remote = matches!(source, TrackSource::Navidrome { .. });
        tracing::info!(%load, remote, "loading track");
        self.current_load = Some(load);
        self.outgoing = None;
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
        // `<` at the first entry while loading: nothing to restart.
        assert!(p.play_previous().is_empty());
        assert_eq!(p.queue().current(), Some(ids[0]));
        p.handle_event(PlaybackEvent::Started { load });
        assert_eq!(p.seek_relative(5.0), [PlayerCommand::SeekRelative(5.0)]);
        assert_eq!(p.play_previous(), [PlayerCommand::SeekAbsolute(0.0)]);
        // `<` while loading a later entry still goes back.
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
        assert_eq!(load.clone().into_playback(), Err(load));
        // A next entry needs resolving too; "no next entry" does not.
        let next = PlayerCommand::SetNext {
            after: LoadId(3),
            next: Some((LoadId(4), source)),
        };
        assert_eq!(next.clone().into_playback(), Err(next));
        assert_eq!(
            PlayerCommand::SetNext {
                after: LoadId(3),
                next: None
            }
            .into_playback(),
            Ok(PlaybackCommand::SetNext {
                after: LoadId(3),
                next: None
            })
        );
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
        assert!(p.play_context(vec![t("X")], 0).is_empty());
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

    // ----- play in context (M7) -----------------------------------------

    fn tracks(names: &[&str]) -> Vec<Track> {
        names.iter().map(|n| t(n)).collect()
    }

    #[test]
    fn play_context_replaces_the_queue_and_plays_the_start_track() {
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        let old = start(&mut p, ids[1]);
        p.set_repeat(RepeatMode::One);
        let (load, path) = load_of(&p.play_context(tracks(&["X", "Y", "Z"]), 1));
        assert_eq!(path, PathBuf::from("/m/Y.flac"));
        assert!(load > old, "a fresh LoadId");
        assert_eq!(titles(&p), ["X", "Y*", "Z"]);
        assert_eq!(p.current_load(), Some(load));
        assert_eq!(p.status(), PlaybackStatus::Loading);
        assert_eq!(p.repeat(), RepeatMode::One, "repeat is kept");
        // The replaced load's end is stale.
        assert!(p.handle_event(ended(old, EndReason::Eof)).is_empty());
        assert_eq!(titles(&p), ["X", "Y*", "Z"]);
        // `<` walks back through the context.
        let (_, path) = load_of(&p.play_previous());
        assert_eq!(path, PathBuf::from("/m/X.flac"));
        // Old entry ids are gone.
        assert!(p.queue().get(ids[0]).is_none());
    }

    #[test]
    fn play_context_clamps_the_start_and_ignores_an_empty_context() {
        let (mut p, _) = player_with(&["A"]);
        let (_, path) = load_of(&p.play_context(tracks(&["X", "Y"]), 9));
        assert_eq!(path, PathBuf::from("/m/Y.flac"));
        assert!(p.play_context(Vec::new(), 0).is_empty());
        assert_eq!(titles(&p), ["X", "Y*"], "an empty context changes nothing");
    }

    #[test]
    fn play_context_resets_failures_in_a_row() {
        let (mut p, ids) = player_with(&["A", "B"]);
        p.set_repeat(RepeatMode::All);
        let load = start(&mut p, ids[0]);
        p.handle_event(ended(load, EndReason::Error("bad".into())));
        assert_eq!(p.failures_in_a_row, 1);
        load_of(&p.play_context(tracks(&["X"]), 0));
        assert_eq!(p.failures_in_a_row, 0);
    }

    #[test]
    fn context_window_keeps_the_start_and_drops_from_the_front_first() {
        let max = MAX_QUEUE_LEN;
        assert_eq!(context_window(3, 1), 0..3);
        assert_eq!(context_window(max, max - 1), 0..max);
        // Too long by 5: the five before the start go first.
        assert_eq!(context_window(max + 5, 100), 5..max + 5);
        // Fewer than that before the start: the rest come off the end.
        assert_eq!(context_window(max + 5, 2), 2..max + 2);
        assert_eq!(context_window(max + 5, 0), 0..max);
        assert_eq!(context_window(max + 5, max + 4), 5..max + 5);
        for (len, start) in [
            (max + 1, 0),
            (max * 2, max),
            (max * 3, 7),
            (max * 3, max * 3 - 1),
        ] {
            let window = context_window(len, start);
            assert_eq!(window.len(), max, "{len} {start}");
            assert!(window.contains(&start), "{len} {start}");
        }
    }

    #[test]
    fn play_context_over_the_cap_keeps_a_window_with_the_start() {
        let mut p = Player::new();
        let names: Vec<String> = (0..MAX_QUEUE_LEN + 3).map(|i| i.to_string()).collect();
        let context: Vec<Track> = names.iter().map(|n| t(n)).collect();
        let (_, path) = load_of(&p.play_context(context, 1));
        assert_eq!(path, PathBuf::from("/m/1.flac"));
        assert_eq!(p.queue().len(), MAX_QUEUE_LEN);
        let entries = p.queue().entries();
        assert_eq!(entries[0].track.title, "1");
        assert_eq!(
            entries[MAX_QUEUE_LEN - 1].track.title,
            MAX_QUEUE_LEN.to_string()
        );
        assert_eq!(p.queue().position(p.queue().current().unwrap()), Some(0));
    }

    #[test]
    fn play_context_on_a_full_queue_replaces_it() {
        let mut p = Player::new();
        let full: Vec<Track> = (0..MAX_QUEUE_LEN).map(|i| t(&i.to_string())).collect();
        p.restore(full, None, RepeatMode::Off, 100);
        assert!(p.is_queue_full());
        load_of(&p.play_context(tracks(&["X"]), 0));
        assert_eq!(titles(&p), ["X*"]);
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
        // `>` and `<` behave as usual.
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
        // `<` at the first entry still restarts rather than wrapping.
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

    // ----- suspect ends, `D` under repeat, abandoning, queue cap ------------

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
        assert_eq!(p.queue().len(), MAX_QUEUE_LEN);
    }

    // ----- gapless lookahead (M8) ----------------------------------------

    /// The `(after, next title)` of the next announcement, if any.
    fn announced(p: &mut Player) -> Option<(LoadId, Option<(LoadId, String)>)> {
        match p.sync_next()? {
            PlayerCommand::SetNext { after, next } => Some((
                after,
                next.map(|(load, source)| match source {
                    TrackSource::Local { path } => (
                        load,
                        path.file_stem().unwrap().to_string_lossy().into_owned(),
                    ),
                    TrackSource::Navidrome { song_id } => (load, song_id),
                }),
            )),
            other => panic!("{other:?}"),
        }
    }

    /// The title announced next, asserting it follows the current load.
    fn next_title(p: &mut Player) -> Option<Option<String>> {
        let (after, next) = announced(p)?;
        assert_eq!(Some(after), p.current_load());
        Some(next.map(|(_, title)| title))
    }

    fn some(title: &str) -> Option<Option<String>> {
        Some(Some(title.to_owned()))
    }

    #[test]
    fn the_next_entry_is_announced_once_and_again_when_it_changes() {
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        assert_eq!(announced(&mut p), None, "nothing is loaded");
        start(&mut p, ids[0]);
        assert_eq!(next_title(&mut p), some("B"));
        assert_eq!(announced(&mut p), None, "unchanged");
        // Moving B away makes C next.
        p.move_down(ids[1]);
        assert_eq!(next_title(&mut p), some("C"));
        // Play next puts X first.
        p.enqueue_next(t("X"));
        assert_eq!(next_title(&mut p), some("X"));
        // Removing it brings C back.
        let x = p.queue().id_after(ids[0]).unwrap();
        assert!(p.remove(x).is_empty());
        assert_eq!(next_title(&mut p), some("C"));
        // Nothing after the last entry: "no next" is announced once.
        p.remove(ids[2]);
        p.remove(ids[1]);
        assert_eq!(next_title(&mut p), Some(None));
        assert_eq!(announced(&mut p), None);
        // A new entry at the end is next again.
        p.enqueue(t("D"));
        assert_eq!(next_title(&mut p), some("D"));
        // Clearing the queue stops: nothing to announce.
        assert_eq!(p.clear_queue(), [PlayerCommand::Stop]);
        assert_eq!(announced(&mut p), None);
    }

    #[test]
    fn every_announcement_gets_a_fresh_load_id() {
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        let a = start(&mut p, ids[0]);
        let (_, b) = announced(&mut p).unwrap();
        p.move_down(ids[1]);
        let (_, c) = announced(&mut p).unwrap();
        p.move_up(ids[1]);
        let (_, b2) = announced(&mut p).unwrap();
        let loads = [a, b.unwrap().0, c.unwrap().0, b2.unwrap().0];
        assert!(loads.windows(2).all(|w| w[0] < w[1]), "{loads:?}");
        // A user load after them is newer still.
        let (d, _) = load_of(&p.play_entry(ids[2]));
        assert!(d > loads[3]);
    }

    #[test]
    fn a_new_load_without_a_next_entry_announces_nothing() {
        let (mut p, ids) = player_with(&["A", "B"]);
        start(&mut p, ids[1]);
        assert_eq!(announced(&mut p), None, "the backend holds none anyway");
        start(&mut p, ids[0]);
        assert_eq!(next_title(&mut p), some("B"));
    }

    #[test]
    fn the_next_entry_follows_the_repeat_mode() {
        let (mut p, ids) = player_with(&["A", "B"]);
        start(&mut p, ids[1]);
        assert_eq!(announced(&mut p), None);
        // All: the first entry follows the last.
        p.set_repeat(RepeatMode::All);
        assert_eq!(next_title(&mut p), some("A"));
        // One: the queue's next until the track has really played, so an
        // instant-EOF file is never repeated…
        p.set_repeat(RepeatMode::One);
        assert_eq!(next_title(&mut p), Some(None));
        p.play_entry(ids[0]);
        let load = p.current_load().unwrap();
        p.handle_event(PlaybackEvent::Started { load });
        assert_eq!(next_title(&mut p), some("B"));
        // …then the track itself.
        p.handle_event(PlaybackEvent::Position(PROVEN_PLAYBACK));
        assert_eq!(next_title(&mut p), some("A"));
        // Off again: the queue's next.
        p.set_repeat(RepeatMode::Off);
        assert_eq!(next_title(&mut p), some("B"));
        // A single entry under All repeats itself.
        let (mut p, ids) = player_with(&["A"]);
        p.set_repeat(RepeatMode::All);
        start(&mut p, ids[0]);
        assert_eq!(next_title(&mut p), some("A"));
    }

    #[test]
    fn shuffle_changes_the_next_entry() {
        let (mut p, ids) = player_with(&["A", "B", "C", "D", "E", "F"]);
        start(&mut p, ids[0]);
        assert_eq!(next_title(&mut p), some("B"));
        let mut rng = Rng::from_seed(11);
        p.shuffle_upcoming(&mut rng);
        let expected = p.queue().id_after(ids[0]).unwrap();
        let title = p.queue().get(expected).unwrap().track.title.clone();
        if title == "B" {
            assert_eq!(announced(&mut p), None);
        } else {
            assert_eq!(next_title(&mut p), some(&title));
        }
    }

    /// Plays `ids[i]`, announces its next and advances to it gaplessly.
    fn advance(p: &mut Player) -> (LoadId, LoadId, Vec<PlayerCommand>) {
        let from = p.current_load().unwrap();
        let (_, next) = announced(p).expect("a next entry");
        let to = next.expect("a next entry").0;
        let commands = p.handle_event(PlaybackEvent::Advanced { from, to });
        (from, to, commands)
    }

    #[test]
    fn a_gapless_advance_moves_the_queue_on_without_a_load() {
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        let a = play_through(&mut p, ids[0]);
        p.handle_event(PlaybackEvent::Duration(Some(Duration::from_secs(3))));
        p.handle_event(PlaybackEvent::Position(Duration::from_millis(2900)));
        let (from, b, commands) = advance(&mut p);
        assert_eq!(from, a);
        assert!(commands.is_empty(), "{commands:?}");
        assert_eq!(p.queue().current(), Some(ids[1]));
        assert_eq!(p.current_load(), Some(b));
        // A is still heard until B is.
        assert_eq!(p.status(), PlaybackStatus::Playing);
        assert_eq!(p.playing_entry().unwrap().track.title, "A");
        assert_eq!(p.audible_load(), Some(a));
        assert_eq!(p.position(), Some(Duration::from_millis(2900)));
        assert_eq!(p.duration(), Some(Duration::from_secs(3)));
        assert_eq!(p.handover_from(b), Some(a));
        // Seeks wait for B (a seek now would land in B and cut A's tail).
        assert!(p.seek_relative(5.0).is_empty());
        // B's position before it is heard is ignored.
        p.handle_event(PlaybackEvent::Position(Duration::ZERO));
        assert_eq!(p.position(), Some(Duration::from_millis(2900)));
        // B is heard.
        p.handle_event(PlaybackEvent::Started { load: b });
        assert_eq!(p.playing_entry().unwrap().track.title, "B");
        assert_eq!(p.audible_load(), Some(b));
        assert_eq!(p.position(), None);
        assert_eq!(p.handover_from(b), None);
        assert_eq!(p.status(), PlaybackStatus::Playing);
        // What follows B is announced.
        assert_eq!(next_title(&mut p), some("C"));
        // Old A events are stale.
        assert!(p.handle_event(ended(a, EndReason::Eof)).is_empty());
        assert_eq!(p.current_load(), Some(b));
    }

    #[test]
    fn pausing_while_the_previous_track_is_heard() {
        let (mut p, ids) = player_with(&["A", "B"]);
        play_through(&mut p, ids[0]);
        advance(&mut p);
        assert_eq!(p.toggle_pause(), [PlayerCommand::SetPause(true)]);
        assert_eq!(p.status(), PlaybackStatus::Paused);
    }

    #[test]
    fn an_advance_from_or_to_a_load_that_is_not_current_is_ignored() {
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        let a = play_through(&mut p, ids[0]);
        let (_, next) = announced(&mut p).unwrap();
        let b = next.unwrap().0;
        // The user picked C meanwhile: an advance of A is stale.
        let (c, _) = load_of(&p.play_entry(ids[2]));
        assert!(
            p.handle_event(PlaybackEvent::Advanced { from: a, to: b })
                .is_empty()
        );
        assert_eq!(p.current_load(), Some(c));
        assert_eq!(p.queue().current(), Some(ids[2]));
        // And B's start is stale too.
        p.handle_event(PlaybackEvent::Started { load: b });
        assert_eq!(p.status(), PlaybackStatus::Loading);
    }

    #[test]
    fn an_advance_to_an_entry_no_longer_next_is_corrected() {
        // The user removed B while mpv was already starting it.
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        let a = play_through(&mut p, ids[0]);
        let (_, next) = announced(&mut p).unwrap();
        let b = next.unwrap().0;
        assert!(p.remove(ids[1]).is_empty());
        let (_, next) = announced(&mut p).unwrap();
        assert_eq!(next.unwrap().1, "C");
        let commands = p.handle_event(PlaybackEvent::Advanced { from: a, to: b });
        let (c, path) = load_of(&commands);
        assert_eq!(path, PathBuf::from("/m/C.flac"));
        assert_eq!(p.current_load(), Some(c));
        assert_eq!(p.queue().current(), Some(ids[2]));
        // B's events change nothing.
        p.handle_event(PlaybackEvent::Started { load: b });
        assert_eq!(p.status(), PlaybackStatus::Loading);
        assert!(p.handle_event(ended(b, EndReason::Stopped)).is_empty());
        assert_eq!(p.current_load(), Some(c));

        // Nothing left to play: the stale entry is stopped.
        let (mut p, ids) = player_with(&["A", "B"]);
        let a = play_through(&mut p, ids[0]);
        let (_, next) = announced(&mut p).unwrap();
        let b = next.unwrap().0;
        p.remove(ids[1]);
        assert_eq!(next_title(&mut p), Some(None));
        assert_eq!(
            p.handle_event(PlaybackEvent::Advanced { from: a, to: b }),
            [PlayerCommand::Stop]
        );
        assert_eq!(p.status(), PlaybackStatus::Stopped);
        assert_eq!(p.queue().current(), Some(ids[0]));

        // Under repeat one, a change of mode in the window is corrected
        // the same way: the track repeats instead of B.
        let (mut p, ids) = player_with(&["A", "B"]);
        let a = play_through(&mut p, ids[0]);
        let (_, next) = announced(&mut p).unwrap();
        let b = next.unwrap().0;
        p.set_repeat(RepeatMode::One);
        assert_eq!(next_title(&mut p), some("A"));
        let (_, path) = load_of(&p.handle_event(PlaybackEvent::Advanced { from: a, to: b }));
        assert_eq!(path, PathBuf::from("/m/A.flac"));
    }

    #[test]
    fn a_failing_next_track_advances_like_any_failure() {
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        play_through(&mut p, ids[0]);
        let (_, b, _) = advance(&mut p);
        let commands = p.handle_event(ended(b, EndReason::Error("loading failed".into())));
        let (_, path) = load_of(&commands);
        assert_eq!(path, PathBuf::from("/m/C.flac"));
        assert_eq!(p.take_messages(), ["Could not play B: loading failed"]);
        assert_eq!(p.playing_entry().unwrap().track.title, "C");
    }

    /// Every track starts, reports at most `position` and goes on without
    /// a gap to the announced next one. Returns the number of tracks
    /// started before playback stopped.
    fn instant_gapless(n: usize, repeat: RepeatMode, position: Duration) -> (usize, Player) {
        let names: Vec<String> = (0..n).map(|i| format!("T{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let (mut p, _) = player_with(&refs);
        p.set_repeat(repeat);
        let mut commands = p.toggle_pause();
        let mut plays = 1;
        loop {
            assert!(plays <= 10 * n + 10, "{repeat:?}: did not terminate");
            assert!(
                commands
                    .iter()
                    .all(|c| matches!(c, PlayerCommand::Load { .. })),
                "{commands:?}"
            );
            let load = p.current_load().unwrap();
            p.handle_event(PlaybackEvent::Started { load });
            p.handle_event(PlaybackEvent::Position(position));
            commands = match announced(&mut p) {
                Some((_, Some((to, _)))) => {
                    p.handle_event(PlaybackEvent::Advanced { from: load, to })
                }
                _ => p.handle_event(ended(load, EndReason::Eof)),
            };
            if p.current_load().is_none() {
                return (plays, p);
            }
            plays += 1;
        }
    }

    #[test]
    fn instant_tracks_never_loop_when_advances_are_gapless() {
        let almost = PROVEN_PLAYBACK - Duration::from_millis(1);
        for repeat in [RepeatMode::Off, RepeatMode::One, RepeatMode::All] {
            let (plays, mut p) = instant_gapless(4, repeat, almost);
            assert_eq!(plays, 4, "{repeat:?}");
            assert_eq!(p.status(), PlaybackStatus::Stopped, "{repeat:?}");
            assert_eq!(
                p.take_messages().last().map(String::as_str) == Some(NO_PLAYABLE_TRACKS),
                repeat == RepeatMode::All,
                "{repeat:?}"
            );
        }
        // A single instant entry plays once under every mode.
        for repeat in [RepeatMode::Off, RepeatMode::One, RepeatMode::All] {
            let (plays, _) = instant_gapless(1, repeat, Duration::ZERO);
            assert_eq!(plays, 1, "{repeat:?}");
        }
    }

    #[test]
    fn proven_tracks_repeat_gaplessly() {
        // One: the same entry again and again, each play its own load.
        let (mut p, ids) = player_with(&["A", "B"]);
        p.set_repeat(RepeatMode::One);
        play_through(&mut p, ids[0]);
        let mut loads = vec![p.current_load().unwrap()];
        for _ in 0..3 {
            let (_, to, commands) = advance(&mut p);
            assert!(commands.is_empty());
            p.handle_event(PlaybackEvent::Started { load: to });
            p.handle_event(PlaybackEvent::Position(PROVEN_PLAYBACK));
            loads.push(to);
        }
        assert!(loads.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(p.queue().current(), Some(ids[0]));
        // All: wraps around.
        let (mut p, ids) = player_with(&["A", "B"]);
        p.set_repeat(RepeatMode::All);
        play_through(&mut p, ids[1]);
        advance(&mut p);
        assert_eq!(p.queue().current(), Some(ids[0]));
    }

    #[test]
    fn failures_in_a_row_count_across_gapless_advances() {
        // A, B, C fail or end at once in a row under All: one round, then
        // stop, mixing errors (which never go on gaplessly) and advances.
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        p.set_repeat(RepeatMode::All);
        let a = start(&mut p, ids[0]);
        let (_, next) = announced(&mut p).unwrap();
        let b = next.unwrap().0;
        assert!(
            p.handle_event(PlaybackEvent::Advanced { from: a, to: b })
                .is_empty()
        );
        let (c, _) = load_of(&p.handle_event(ended(b, EndReason::Error("x".into()))));
        p.handle_event(PlaybackEvent::Started { load: c });
        let (_, next) = announced(&mut p).unwrap();
        let a2 = next.unwrap().0;
        assert_eq!(
            p.handle_event(PlaybackEvent::Advanced { from: c, to: a2 }),
            [PlayerCommand::Stop]
        );
        assert_eq!(
            p.take_messages().last().map(String::as_str),
            Some(NO_PLAYABLE_TRACKS)
        );
    }

    #[test]
    fn a_client_change_announces_a_remote_next_entry_again() {
        let mut p = Player::new();
        let a = p.enqueue(t("A")).unwrap();
        p.enqueue(remote("r1"));
        start(&mut p, a);
        let (_, first) = announced(&mut p).unwrap();
        assert!(p.client_changed().is_empty());
        let (_, again) = announced(&mut p).unwrap();
        assert_eq!(again.as_ref().unwrap().1, "r1");
        assert!(again.unwrap().0 > first.unwrap().0, "a fresh load id");
        // A local next entry is kept as it is.
        let (mut p, ids) = player_with(&["A", "B"]);
        start(&mut p, ids[0]);
        announced(&mut p);
        assert!(p.client_changed().is_empty());
        assert_eq!(announced(&mut p), None);
    }

    #[test]
    fn nothing_is_announced_once_playback_is_unavailable() {
        let (mut p, ids) = player_with(&["A", "B"]);
        start(&mut p, ids[0]);
        announced(&mut p);
        p.handle_event(PlaybackEvent::BackendExited("gone".into()));
        assert_eq!(announced(&mut p), None);
    }

    /// `names` queued; the entry at `from` plays (proven, at `position`)
    /// and the backend advanced to the next one, which is not heard yet.
    fn in_overlap(names: &[&str], from: usize, position: Duration) -> (Player, Vec<QueueEntryId>) {
        let (mut p, ids) = player_with(names);
        play_through(&mut p, ids[from]);
        p.handle_event(PlaybackEvent::Position(position));
        advance(&mut p);
        assert!(p.handover_from(p.current_load().unwrap()).is_some());
        (p, ids)
    }

    #[test]
    fn next_during_the_overlap_loads_the_track_not_heard_yet() {
        let (mut p, ids) = in_overlap(&["A", "B", "C"], 0, Duration::from_secs(100));
        let b = p.current_load().unwrap();
        let (load, path) = load_of(&p.play_next());
        assert_eq!(path, PathBuf::from("/m/B.flac"), "not C: B was never heard");
        assert!(load > b);
        assert_eq!(p.queue().current(), Some(ids[1]));
        assert_eq!(p.playing_entry().unwrap().track.title, "B");
        assert_eq!(p.status(), PlaybackStatus::Loading);
    }

    #[test]
    fn previous_during_the_overlap_follows_the_track_heard() {
        // Past the threshold: the heard track (A) restarts, by a load.
        let (mut p, ids) = in_overlap(&["Z", "A", "B"], 1, Duration::from_secs(100));
        let (_, path) = load_of(&p.play_previous());
        assert_eq!(path, PathBuf::from("/m/A.flac"));
        assert_eq!(p.queue().current(), Some(ids[1]));
        // Early in it: the entry before the heard track.
        let (mut p, ids) = in_overlap(&["Z", "A", "B"], 1, Duration::from_secs(2));
        let (_, path) = load_of(&p.play_previous());
        assert_eq!(path, PathBuf::from("/m/Z.flac"));
        assert_eq!(p.queue().current(), Some(ids[0]));
        // The first entry: it restarts.
        let (mut p, _) = in_overlap(&["A", "B"], 0, Duration::from_secs(2));
        let (_, path) = load_of(&p.play_previous());
        assert_eq!(path, PathBuf::from("/m/A.flac"));
    }

    #[test]
    fn pause_and_seek_during_the_overlap() {
        let (mut p, _) = in_overlap(&["A", "B"], 0, Duration::from_secs(100));
        assert!(p.seek_relative(5.0).is_empty(), "it would land in B");
        assert_eq!(p.toggle_pause(), [PlayerCommand::SetPause(true)]);
        p.handle_event(PlaybackEvent::Paused(true));
        assert_eq!(p.status(), PlaybackStatus::Paused);
        assert_eq!(p.toggle_pause(), [PlayerCommand::SetPause(false)]);
    }

    #[test]
    fn an_advance_to_an_earlier_copy_of_the_same_local_entry_plays_on() {
        // B moved away and back while mpv was already starting the copy
        // announced first: the same file, so no correction.
        let (mut p, ids) = player_with(&["A", "B", "C"]);
        let a = play_through(&mut p, ids[0]);
        let (_, first) = announced(&mut p).unwrap();
        let first = first.unwrap().0;
        p.move_down(ids[1]);
        announced(&mut p).unwrap();
        p.move_up(ids[1]);
        let (_, again) = announced(&mut p).unwrap();
        assert_ne!(again.unwrap().0, first);
        let commands = p.handle_event(PlaybackEvent::Advanced { from: a, to: first });
        assert!(commands.is_empty(), "{commands:?}");
        assert_eq!(p.current_load(), Some(first));
        assert_eq!(p.queue().current(), Some(ids[1]));
        assert_eq!(p.playing_entry().unwrap().track.title, "A");

        // A remote entry is always corrected: its first copy may have been
        // signed by another client.
        let mut p = Player::new();
        let a_id = p.enqueue(t("A")).unwrap();
        let r = p.enqueue(remote("r1")).unwrap();
        p.enqueue(t("C"));
        let a = play_through(&mut p, a_id);
        let (_, first) = announced(&mut p).unwrap();
        let first = first.unwrap().0;
        p.move_down(r);
        announced(&mut p).unwrap();
        p.move_up(r);
        announced(&mut p).unwrap();
        let commands = p.handle_event(PlaybackEvent::Advanced { from: a, to: first });
        assert!(
            matches!(commands.as_slice(), [PlayerCommand::Load { .. }]),
            "{commands:?}"
        );
    }

    #[test]
    fn a_client_change_reloads_a_remote_track_not_heard_yet() {
        let mut p = Player::new();
        let a = p.enqueue(t("A")).unwrap();
        p.enqueue(remote("r1"));
        play_through(&mut p, a);
        let (_, r1, _) = advance(&mut p);
        let commands = p.client_changed();
        let [PlayerCommand::Load { load, source }] = commands.as_slice() else {
            panic!("{commands:?}");
        };
        assert!(*load > r1);
        assert_eq!(
            *source,
            TrackSource::Navidrome {
                song_id: "r1".into()
            }
        );
        assert_eq!(p.current_load(), Some(*load));
        // A local track not heard yet plays on.
        let (mut p, _) = in_overlap(&["A", "B"], 0, Duration::from_secs(2));
        assert!(p.client_changed().is_empty());
    }

    #[test]
    fn a_reload_after_a_client_change_keeps_the_player_paused() {
        let mut p = Player::new();
        let a = p.enqueue(t("A")).unwrap();
        p.enqueue(remote("r1"));
        play_through(&mut p, a);
        advance(&mut p);
        assert_eq!(p.toggle_pause(), [PlayerCommand::SetPause(true)]);
        p.handle_event(PlaybackEvent::Paused(true));
        let commands = p.client_changed();
        let [
            PlayerCommand::Load { load, .. },
            PlayerCommand::SetPause(true),
        ] = commands.as_slice()
        else {
            panic!("{commands:?}");
        };
        p.handle_event(PlaybackEvent::Started { load: *load });
        assert_eq!(p.status(), PlaybackStatus::Paused);
    }
}
