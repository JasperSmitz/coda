//! The play queue: an ordered list of entries plus an optional current
//! entry. Pure data; no I/O.
//!
//! Entries are addressed by [`QueueEntryId`], never by an index captured
//! earlier, so concurrent edits cannot make an operation hit the wrong
//! entry. The same track may appear several times as distinct entries.
//!
//! Every change to the entries or to `current` bumps [`Queue::revision`],
//! so callers can tell cheaply whether the queue needs saving.

use crate::model::Track;

/// Upper bound on queue length, enforced where tracks are added (the
/// player) and on restore.
pub const MAX_QUEUE_LEN: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QueueEntryId(u64);

impl std::fmt::Display for QueueEntryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "q{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEntry {
    pub id: QueueEntryId,
    pub track: Track,
}

#[derive(Debug, Clone, Default)]
pub struct Queue {
    entries: Vec<QueueEntry>,
    current: Option<QueueEntryId>,
    next_id: u64,
    revision: u64,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &[QueueEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn current(&self) -> Option<QueueEntryId> {
        self.current
    }

    /// Increases with every change to the entries, their order or
    /// `current`. Never decreases.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn changed(&mut self) {
        self.revision += 1;
    }

    pub fn current_entry(&self) -> Option<&QueueEntry> {
        self.current.and_then(|id| self.get(id))
    }

    pub fn get(&self, id: QueueEntryId) -> Option<&QueueEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn position(&self, id: QueueEntryId) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }

    pub fn first(&self) -> Option<QueueEntryId> {
        self.entries.first().map(|e| e.id)
    }

    /// The entry after `id`, if any.
    pub fn id_after(&self, id: QueueEntryId) -> Option<QueueEntryId> {
        let pos = self.position(id)?;
        self.entries.get(pos + 1).map(|e| e.id)
    }

    /// The entry before `id`, if any.
    pub fn id_before(&self, id: QueueEntryId) -> Option<QueueEntryId> {
        let pos = self.position(id)?;
        pos.checked_sub(1).map(|p| self.entries[p].id)
    }

    /// Appends a new entry at the end. Does not change `current`.
    pub fn append(&mut self, track: Track) -> QueueEntryId {
        let entry = self.new_entry(track);
        let id = entry.id;
        self.entries.push(entry);
        self.changed();
        id
    }

    /// Inserts a new entry directly after the current one, or appends it
    /// when there is no current entry. Does not change `current`.
    pub fn insert_after_current(&mut self, track: Track) -> QueueEntryId {
        let Some(pos) = self.current.and_then(|id| self.position(id)) else {
            return self.append(track);
        };
        let entry = self.new_entry(track);
        let id = entry.id;
        self.entries.insert(pos + 1, entry);
        self.changed();
        id
    }

    /// Makes `id` current. Returns `false` (and changes nothing) if `id` is
    /// not in the queue.
    pub fn set_current(&mut self, id: QueueEntryId) -> bool {
        if self.position(id).is_some() {
            if self.current != Some(id) {
                self.current = Some(id);
                self.changed();
            }
            true
        } else {
            false
        }
    }

    /// Moves `current` to the following entry and returns it. At the end of
    /// the queue, or with no current entry, returns `None` and leaves
    /// `current` unchanged.
    pub fn move_to_next(&mut self) -> Option<QueueEntryId> {
        let next = self.id_after(self.current?)?;
        self.current = Some(next);
        self.changed();
        Some(next)
    }

    /// Moves `current` to the preceding entry and returns it. At the first
    /// entry, or with no current entry, returns `None` and leaves `current`
    /// unchanged.
    pub fn move_to_previous(&mut self) -> Option<QueueEntryId> {
        let prev = self.id_before(self.current?)?;
        self.current = Some(prev);
        self.changed();
        Some(prev)
    }

    /// Removes an entry. Removing a non-current entry never changes
    /// `current`. If the current entry is removed (the player decides what
    /// that means for playback first), `current` moves to the entry that
    /// took its place, else the one before it, else `None`.
    pub fn remove(&mut self, id: QueueEntryId) -> Option<QueueEntry> {
        let pos = self.position(id)?;
        let entry = self.entries.remove(pos);
        self.changed();
        if self.current == Some(id) {
            self.current = self
                .entries
                .get(pos)
                .or_else(|| pos.checked_sub(1).and_then(|p| self.entries.get(p)))
                .map(|e| e.id);
        }
        Some(entry)
    }

    /// Swaps entry `id` with the one before it. Returns `false` (and changes
    /// nothing) if `id` is unknown or already first. `current` keeps
    /// pointing at the same entry.
    pub fn move_up(&mut self, id: QueueEntryId) -> bool {
        match self.position(id) {
            Some(pos) if pos > 0 => {
                self.entries.swap(pos - 1, pos);
                self.changed();
                true
            }
            _ => false,
        }
    }

    /// Swaps entry `id` with the one after it. Returns `false` (and changes
    /// nothing) if `id` is unknown or already last.
    pub fn move_down(&mut self, id: QueueEntryId) -> bool {
        match self.position(id) {
            Some(pos) if pos + 1 < self.entries.len() => {
                self.entries.swap(pos, pos + 1);
                self.changed();
                true
            }
            _ => false,
        }
    }

    /// Removes every entry. Ids are never reused afterwards.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.current = None;
        self.changed();
    }

    /// Shuffles the entries after the current one (all entries when there
    /// is no current one) with a Fisher–Yates shuffle. `random_below(n)`
    /// must return a value in `0..n`; out-of-range values are reduced
    /// modulo `n`. The current entry and everything before it stay in
    /// place. Returns the number of entries shuffled.
    pub fn shuffle_after_current(&mut self, mut random_below: impl FnMut(usize) -> usize) -> usize {
        let start = match self.current.and_then(|id| self.position(id)) {
            Some(pos) => pos + 1,
            None => 0,
        };
        let upcoming = &mut self.entries[start..];
        for i in (1..upcoming.len()).rev() {
            let j = random_below(i + 1) % (i + 1);
            upcoming.swap(i, j);
        }
        let shuffled = upcoming.len();
        if shuffled > 1 {
            self.changed();
        }
        shuffled
    }

    fn new_entry(&mut self, track: Track) -> QueueEntry {
        self.next_id += 1;
        QueueEntry {
            id: QueueEntryId(self.next_id),
            track,
        }
    }
}

/// A small seeded PRNG (SplitMix64) for shuffling. Not cryptographic.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn from_seed(seed: u64) -> Self {
        Self(seed)
    }

    /// Seeded from the OS; falls back to the clock if that fails.
    pub fn from_entropy() -> Self {
        let seed = getrandom::u64().unwrap_or_else(|_| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos() as u64)
        });
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..n` (`0` when `n` is `0`).
    pub fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next_u64()) * n as u128) >> 64) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackMetadata;

    fn t(name: &str) -> Track {
        Track::local(format!("/m/{name}.flac").into(), TrackMetadata::default())
    }

    fn titles(q: &Queue) -> Vec<String> {
        q.entries()
            .iter()
            .map(|e| {
                let mark = if q.current() == Some(e.id) { "*" } else { "" };
                format!("{}{mark}", e.track.title)
            })
            .collect()
    }

    #[test]
    fn append_on_empty_and_non_empty() {
        let mut q = Queue::new();
        assert!(q.is_empty());
        let a = q.append(t("A"));
        let b = q.append(t("B"));
        assert_ne!(a, b);
        assert_eq!(titles(&q), ["A", "B"]);
        assert_eq!(q.current(), None);
    }

    #[test]
    fn insert_after_current_on_empty_queue() {
        let mut q = Queue::new();
        q.insert_after_current(t("X"));
        assert_eq!(titles(&q), ["X"]);
    }

    #[test]
    fn insert_after_current_without_current_appends() {
        let mut q = Queue::new();
        q.append(t("A"));
        q.append(t("B"));
        q.insert_after_current(t("X"));
        assert_eq!(titles(&q), ["A", "B", "X"]);
    }

    #[test]
    fn insert_after_current_in_middle_and_at_end() {
        let mut q = Queue::new();
        q.append(t("A"));
        let b = q.append(t("B"));
        q.append(t("C"));
        q.set_current(b);
        q.insert_after_current(t("X"));
        assert_eq!(titles(&q), ["A", "B*", "X", "C"]);

        let mut q = Queue::new();
        q.append(t("A"));
        let b = q.append(t("B"));
        q.set_current(b);
        q.insert_after_current(t("X"));
        assert_eq!(titles(&q), ["A", "B*", "X"]);
    }

    #[test]
    fn duplicates_have_distinct_ids() {
        let mut q = Queue::new();
        let a1 = q.append(t("A"));
        let a2 = q.append(t("A"));
        assert_ne!(a1, a2);
        q.remove(a1);
        assert_eq!(q.len(), 1);
        assert!(q.get(a2).is_some());
    }

    #[test]
    fn set_current_rejects_unknown_id() {
        let mut q = Queue::new();
        let a = q.append(t("A"));
        q.remove(a);
        assert!(!q.set_current(a));
        assert_eq!(q.current(), None);
    }

    #[test]
    fn next_and_previous_at_bounds() {
        let mut q = Queue::new();
        assert_eq!(q.move_to_next(), None);
        assert_eq!(q.move_to_previous(), None);
        let a = q.append(t("A"));
        let b = q.append(t("B"));
        // No current: neither moves.
        assert_eq!(q.move_to_next(), None);
        assert_eq!(q.move_to_previous(), None);
        q.set_current(a);
        assert_eq!(q.move_to_previous(), None);
        assert_eq!(q.current(), Some(a));
        assert_eq!(q.move_to_next(), Some(b));
        assert_eq!(q.move_to_next(), None);
        assert_eq!(q.current(), Some(b));
        assert_eq!(q.move_to_previous(), Some(a));
    }

    #[test]
    fn remove_before_and_after_current_keeps_current() {
        let mut q = Queue::new();
        let a = q.append(t("A"));
        let b = q.append(t("B"));
        let c = q.append(t("C"));
        q.set_current(b);
        q.remove(a);
        assert_eq!(q.current(), Some(b));
        q.remove(c);
        assert_eq!(q.current(), Some(b));
        assert_eq!(titles(&q), ["B*"]);
    }

    #[test]
    fn remove_current_moves_to_neighbour() {
        let mut q = Queue::new();
        let a = q.append(t("A"));
        let b = q.append(t("B"));
        let c = q.append(t("C"));
        q.set_current(b);
        q.remove(b);
        assert_eq!(q.current(), Some(c));
        q.remove(c);
        assert_eq!(q.current(), Some(a));
        q.remove(a);
        assert_eq!(q.current(), None);
        assert!(q.remove(a).is_none());
    }

    #[test]
    fn move_up_and_down_keep_current_and_respect_bounds() {
        let mut q = Queue::new();
        let a = q.append(t("A"));
        let b = q.append(t("B"));
        let c = q.append(t("C"));
        q.set_current(b);
        assert!(q.move_up(b));
        assert_eq!(titles(&q), ["B*", "A", "C"]);
        assert!(!q.move_up(b), "already first");
        assert!(q.move_down(a));
        assert_eq!(titles(&q), ["B*", "C", "A"]);
        assert!(!q.move_down(a), "already last");
        assert!(q.move_down(b));
        assert_eq!(titles(&q), ["C", "B*", "A"]);
        assert_eq!(q.current(), Some(b));
        q.remove(c);
        assert!(!q.move_up(c), "unknown id");
        assert!(!q.move_down(c));
        assert_eq!(titles(&q), ["B*", "A"]);
    }

    #[test]
    fn clear_empties_and_never_reuses_ids() {
        let mut q = Queue::new();
        let a = q.append(t("A"));
        q.set_current(a);
        q.clear();
        assert!(q.is_empty());
        assert_eq!(q.current(), None);
        let b = q.append(t("B"));
        assert_ne!(a, b);
        assert!(!q.set_current(a));
    }

    /// Always picks the lowest index: Fisher–Yates then rotates left.
    fn lowest(_: usize) -> usize {
        0
    }

    #[test]
    fn shuffle_after_current_leaves_current_and_earlier_entries() {
        let mut q = Queue::new();
        let ids: Vec<_> = ["A", "B", "C", "D", "E"]
            .iter()
            .map(|n| q.append(t(n)))
            .collect();
        q.set_current(ids[1]);
        assert_eq!(q.shuffle_after_current(lowest), 3);
        assert_eq!(titles(&q), ["A", "B*", "D", "E", "C"]);
        assert_eq!(q.current(), Some(ids[1]));
        // Picking the highest index each time is the identity.
        q.shuffle_after_current(|n| n - 1);
        assert_eq!(titles(&q), ["A", "B*", "D", "E", "C"]);
        // Out-of-range picks are reduced, never panic.
        q.shuffle_after_current(|_| usize::MAX);
        assert_eq!(q.len(), 5);
    }

    #[test]
    fn shuffle_without_current_shuffles_everything() {
        let mut q = Queue::new();
        for n in ["A", "B", "C"] {
            q.append(t(n));
        }
        assert_eq!(q.shuffle_after_current(lowest), 3);
        assert_eq!(titles(&q), ["B", "C", "A"]);
    }

    #[test]
    fn shuffle_of_short_tails_is_a_no_op() {
        let mut q = Queue::new();
        assert_eq!(q.shuffle_after_current(lowest), 0);
        let a = q.append(t("A"));
        q.set_current(a);
        assert_eq!(q.shuffle_after_current(lowest), 0);
        q.append(t("B"));
        assert_eq!(q.shuffle_after_current(lowest), 1);
        assert_eq!(titles(&q), ["A*", "B"]);
    }

    #[test]
    fn shuffle_is_a_permutation_with_a_seeded_rng() {
        let mut q = Queue::new();
        for i in 0..50 {
            q.append(t(&format!("T{i}")));
        }
        let before: Vec<_> = q.entries().iter().map(|e| e.id).collect();
        let mut rng = Rng::from_seed(7);
        q.shuffle_after_current(|n| rng.below(n));
        let mut after: Vec<_> = q.entries().iter().map(|e| e.id).collect();
        assert_ne!(after, before, "50 entries stay in order only by chance");
        after.sort();
        assert_eq!(after, before);
    }

    #[test]
    fn rng_is_deterministic_and_in_range() {
        let mut a = Rng::from_seed(1);
        let mut b = Rng::from_seed(1);
        for n in 1..1000 {
            let x = a.below(n);
            assert_eq!(x, b.below(n));
            assert!(x < n);
        }
        assert_eq!(a.below(0), 0);
    }
}
