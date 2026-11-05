//! Queue edits beyond single entries (M3): moving a selection to a drop
//! gap, removing or "play next"-ing a selection, shuffling the upcoming
//! entries, and the queue's Navidrome songs for "Save queue as playlist".
//! Like every intent they go through `Player`; the caller runs
//! `resolve::dispatch` afterwards, which re-announces the gapless next.

use std::collections::HashSet;

use super::*;

impl Engine {
    /// The ids of the entries named by `keys`, in queue order (unknown
    /// keys are skipped, duplicates count once).
    pub(super) fn entry_ids(&self, keys: &[String]) -> Vec<QueueEntryId> {
        let keys: HashSet<&str> = keys.iter().map(String::as_str).collect();
        self.player
            .queue()
            .entries()
            .iter()
            .filter(|e| keys.contains(e.id.to_string().as_str()))
            .map(|e| e.id)
            .collect()
    }

    /// Moves the entries `keys` as one block to the gap `to` (before the
    /// entry now at index `to`; the queue's length is the end).
    pub(super) fn move_entries(
        &mut self,
        keys: &[String],
        before: Option<&str>,
    ) -> Vec<PlayerCommand> {
        let queue = self.player.queue();
        let to = match before {
            None => queue.len(),
            Some(key) => match queue.entries().iter().position(|e| e.id.to_string() == key) {
                Some(pos) => pos,
                // The drop target is gone: do not guess.
                None => return Vec::new(),
            },
        };
        let ids = self.entry_ids(keys);
        self.player.move_entries(&ids, to);
        Vec::new()
    }

    /// Removes the entries `keys`. The current entry goes last, so when it
    /// is playing, playback moves on to an entry that stays.
    pub(super) fn remove_entries(&mut self, keys: &[String]) -> Vec<PlayerCommand> {
        let ids = self.entry_ids(keys);
        let current = self.player.queue().current();
        let mut commands = Vec::new();
        for id in ids.iter().filter(|id| Some(**id) != current) {
            commands.extend(self.player.remove(*id));
        }
        if let Some(current) = current.filter(|c| ids.contains(c)) {
            commands.extend(self.player.remove(current));
        }
        commands
    }

    /// Moves the entries `keys` right after the current entry, in queue
    /// order (to the start without a current entry). The current entry
    /// itself stays where it is.
    pub(super) fn play_next_entries(
        &mut self,
        keys: &[String],
        now: Instant,
    ) -> Vec<PlayerCommand> {
        let current = self.player.queue().current();
        let ids: Vec<QueueEntryId> = self
            .entry_ids(keys)
            .into_iter()
            .filter(|id| Some(*id) != current)
            .collect();
        if ids.is_empty() {
            return Vec::new();
        }
        let queue = self.player.queue();
        let to = current
            .and_then(|id| queue.position(id))
            .map_or(0, |pos| pos + 1);
        self.player.move_entries(&ids, to);
        self.notify(format!("Playing next: {}", count(ids.len(), "track")), now);
        Vec::new()
    }

    /// Shuffles the entries after the current one, as Coda's `s` in the
    /// queue.
    pub(super) fn shuffle_upcoming(&mut self, now: Instant) -> Vec<PlayerCommand> {
        let shuffled = self.player.shuffle_upcoming(&mut self.rng);
        let text = if shuffled > 1 {
            format!(
                "Shuffled {} upcoming entries",
                coda_core::model::thousands(shuffled)
            )
        } else {
            "Nothing to shuffle".to_owned()
        };
        self.notify(text, now);
        Vec::new()
    }

    /// The queue's Navidrome songs in queue order, and how many local
    /// tracks were left out (Coda's `save_queue_as_playlist`).
    pub fn queue_songs(&self) -> (Vec<String>, usize) {
        let entries = self.player.queue().entries();
        let songs: Vec<String> = entries
            .iter()
            .filter_map(|e| e.track.navidrome_id().map(str::to_owned))
            .collect();
        let local = entries.len() - songs.len();
        (songs, local)
    }
}
