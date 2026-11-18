//! The mix that owns the queue (decisions 6 and 7, ADR 0008).
//!
//! Starting a mix replaces the queue with [`START_TRACKS`] songs and plays
//! the first. The mix then remembers the queue's
//! [`context`](crate::queue::Queue::context); anything that replaces or
//! clears the queue changes it, and the mix no longer owns the queue
//! (adding, reordering, removing and skipping keep it). While it owns the
//! queue, [`ActiveMix::wants_top_up`] asks for [`TOP_UP_TRACKS`] more
//! whenever [`TOP_UP_WHEN_REMAINING`] or fewer entries remain after the
//! current one; they are appended, oldest played entries removed first
//! when the queue nears its cap. The repeat mode is left alone: the top-up
//! comes before the end, so Repeat All never wraps.
//!
//! Appending never interrupts playback. Only when playback stopped because
//! it reached the end of the queue on its own
//! ([`Player::stopped_at_end`]: the user skipped past the last entries, or
//! a Navidrome refill came late) does it play on, with the entry after the
//! current one (what `>` would play, so songs the user queued meanwhile come
//! first; else the first appended song); not
//! after a stop, a pause, an error, a restore or a removed last entry.
//!
//! The player stays pure and has no callback: each frontend checks
//! [`ActiveMix::wants_top_up`] after it applies events and intents, and
//! dispatches the commands of a top-up (none unless it resumed playback)
//! so the next track is announced again (ADR 0005).

use super::{Ingredient, Mix, Picker, SessionMix};
use crate::model::Track;
use crate::player::{Player, PlayerCommand};
use crate::queue::{MAX_QUEUE_LEN, Placement, Rng};

/// Songs queued when a mix starts.
pub const START_TRACKS: usize = 25;
/// A top-up is due when this many entries or fewer follow the current one.
pub const TOP_UP_WHEN_REMAINING: usize = 5;
/// Songs added per top-up.
pub const TOP_UP_TRACKS: usize = 20;

/// What [`ActiveMix::append`] or [`ActiveMix::top_up`] did.
#[derive(Debug, Clone, PartialEq, Default)]
#[must_use = "dispatch the commands"]
pub struct Appended {
    /// How many songs were added.
    pub added: usize,
    /// The commands to dispatch: when playback had stopped at the end of the
    /// queue, a load of the entry after the current one (else the first added
    /// song); otherwise none.
    pub commands: Vec<PlayerCommand>,
}

/// Why a mix did not start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StartError {
    #[error("No songs found for this mix")]
    NoSongs,
    /// The player refused to replace the queue (playback unavailable; the
    /// player queued its own message).
    #[error("Playback unavailable")]
    Refused,
    /// The queue was replaced or cleared after the mix was made and
    /// before it started ([`ActiveMix::has_ended`]): the user chose
    /// something else to play.
    #[error("Something else is playing")]
    Ended,
}

/// A playing mix: its recipe, its picker and the queue it filled.
#[derive(Debug, Clone)]
pub struct ActiveMix {
    mix: Mix,
    saved: bool,
    picker: Picker,
    /// The queue context it owns; `None` until it starts or adopts one.
    queue_context: Option<u64>,
    /// The queue context when it was made: replaced before the mix
    /// starts, the mix has ended without playing.
    created_context: u64,
}

impl ActiveMix {
    /// `saved`: the mix is in the mixes file (else a quick mix). `picker`
    /// holds one pool per ingredient, in order (or will, as a Navidrome
    /// mix's fetches return). Records `player`'s queue context: once the
    /// queue is replaced or cleared, even before the mix starts, the mix
    /// has ended ([`ActiveMix::has_ended`]).
    pub fn new(mix: Mix, saved: bool, picker: Picker, player: &Player) -> Self {
        Self {
            mix,
            saved,
            picker,
            queue_context: None,
            created_context: player.queue_context(),
        }
    }

    pub fn mix(&self) -> &Mix {
        &self.mix
    }

    pub fn is_saved(&self) -> bool {
        self.saved
    }

    pub fn picker(&self) -> &Picker {
        &self.picker
    }

    /// For setting pools as they arrive ([`Picker::set_pool`]).
    pub fn picker_mut(&mut self) -> &mut Picker {
        &mut self.picker
    }

    /// The ingredients that found no songs (decision 3), for
    /// [`super::no_songs_message`].
    pub fn empty_ingredients(&self) -> Vec<&Ingredient> {
        self.picker
            .empty()
            .into_iter()
            .filter_map(|i| self.mix.ingredients.get(i))
            .collect()
    }

    /// Replaces the queue with [`START_TRACKS`] songs and plays the first;
    /// the mix then owns the queue. Refused once the mix has ended
    /// ([`StartError::Ended`]: the queue was replaced since it was made, or
    /// since it started). On an error the queue and the picker are as they
    /// were (only `rng` has moved on).
    pub fn start(
        &mut self,
        player: &mut Player,
        rng: &mut Rng,
    ) -> Result<Vec<PlayerCommand>, StartError> {
        if self.has_ended(player) {
            return Err(StartError::Ended);
        }
        let picker = self.picker.clone();
        let tracks = self.picker.next(START_TRACKS, rng);
        if tracks.is_empty() {
            self.picker = picker;
            return Err(StartError::NoSongs);
        }
        let before = player.queue_context();
        let commands = player.play_context(tracks, 0);
        if player.queue_context() == before {
            self.picker = picker;
            return Err(StartError::Refused);
        }
        self.queue_context = Some(player.queue_context());
        Ok(commands)
    }

    /// Takes the queue as it is as the mix's own: for a session restored
    /// with this mix active.
    pub fn adopt(&mut self, player: &Player) {
        self.queue_context = Some(player.queue_context());
    }

    /// The queue is still the one this mix filled. Once `false`, the mix
    /// has ended and the frontend drops it.
    pub fn owns_queue(&self, player: &Player) -> bool {
        self.queue_context == Some(player.queue_context())
    }

    /// The queue has been replaced or cleared since the mix started (or
    /// adopted it), or, before it starts, since the mix was made: the mix
    /// is over (decision 7), and the frontend drops it. Before that, a
    /// Navidrome mix's first fetches may land before the queue is filled.
    pub fn has_ended(&self, player: &Player) -> bool {
        let context = player.queue_context();
        match self.queue_context {
            Some(owned) => owned != context,
            None => self.created_context != context,
        }
    }

    /// How many songs to add now: [`TOP_UP_TRACKS`] when the mix owns the
    /// queue and [`TOP_UP_WHEN_REMAINING`] or fewer entries follow the
    /// current one; else `None`.
    pub fn wants_top_up(&self, player: &Player) -> Option<usize> {
        let low = player.queue().remaining_after_current() <= TOP_UP_WHEN_REMAINING;
        (self.owns_queue(player) && low).then_some(TOP_UP_TRACKS)
    }

    /// Adds the songs [`ActiveMix::wants_top_up`] asks for from the
    /// picker ([`ActiveMix::append`]). The caller dispatches the commands
    /// (none unless playback resumed) so the next track is announced.
    pub fn top_up(&mut self, player: &mut Player, rng: &mut Rng) -> Appended {
        match self.wants_top_up(player) {
            Some(n) => {
                let tracks = self.picker.next(n, rng);
                self.append(player, tracks)
            }
            None => Appended::default(),
        }
    }

    /// Appends `tracks` to the queue the mix owns (nothing if it no longer
    /// does), first removing the oldest played entries that would push it
    /// over [`MAX_QUEUE_LEN`]. Only when playback had stopped at the end
    /// of the queue ([`Player::stopped_at_end`]), plays on with what `>`
    /// would play: the entry after the ended one (a song the user queued
    /// meanwhile comes first), else the first of `tracks`.
    pub fn append(&mut self, player: &mut Player, tracks: Vec<Track>) -> Appended {
        if !self.owns_queue(player) || tracks.is_empty() {
            return Appended::default();
        }
        let over = (player.queue().len() + tracks.len()).saturating_sub(MAX_QUEUE_LEN);
        let resume = player.stopped_at_end();
        player.remove_played(over);
        let first = player.queue().len();
        let added = player.enqueue_all(tracks, Placement::End);
        let queue = player.queue();
        let next = queue
            .current()
            .and_then(|current| queue.id_after(current))
            .or_else(|| queue.entries().get(first).map(|entry| entry.id));
        let commands = match next {
            Some(id) if resume && added > 0 => player.play_entry(id),
            _ => Vec::new(),
        };
        Appended { added, commands }
    }

    /// What the session saves for this mix.
    pub fn session(&self) -> SessionMix {
        if self.saved {
            SessionMix::Saved(self.mix.id.clone())
        } else {
            SessionMix::Quick(self.mix.clone())
        }
    }
}
