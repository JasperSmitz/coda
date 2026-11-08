//! The play queue: an ordered list of entries plus an optional current
//! entry. Pure data; no I/O.
//!
//! Entries are addressed by [`QueueEntryId`], never by an index captured
//! earlier, so concurrent edits cannot make an operation hit the wrong
//! entry. The same track may appear several times as distinct entries.
//!
//! Every change to the entries or to `current` bumps [`Queue::revision`],
//! so callers can tell cheaply whether the queue needs saving.
//!
//! "Play next" ([`Queue::insert_next`]) remembers its insertion point, so
//! consecutive inserts form a run after the current entry in the order
//! they were made. A change of the current entry, a shuffle or a clear
//! forgets it (see [`Queue::next_anchor`]).

use crate::model::{Track, count, thousands};

/// Upper bound on queue length, enforced where tracks are added (the
/// player) and on restore.
pub const MAX_QUEUE_LEN: usize = 10_000;

/// Where added tracks go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    /// `a`: at the end of the queue.
    #[default]
    End,
    /// `A`: after the current entry, behind earlier "play next" tracks
    /// ([`Queue::insert_next`]).
    Next,
}

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
    /// The last entry added by [`Queue::insert_next`] since `current`
    /// last changed; the next one goes after it. Not saved.
    next_anchor: Option<QueueEntryId>,
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

    /// Where the next [`Queue::insert_next`] goes after: the last entry it
    /// added. It is forgotten when `current` changes (playback moved on,
    /// or another entry was played), when the current entry is removed,
    /// and on shuffle and clear (which also covers replacing the queue). Appending, moving and removing
    /// other entries keep it: if the anchor entry is gone
    /// or now comes before the current one, inserts go right after the
    /// current entry again.
    pub fn next_anchor(&self) -> Option<QueueEntryId> {
        self.next_anchor
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

    /// "Play next": inserts a new entry after the current one, behind any
    /// entries inserted this way since the current entry last changed (see
    /// [`Queue::next_anchor`]), so consecutive inserts keep their order.
    /// Without a current entry it appends. Does not change `current`.
    pub fn insert_next(&mut self, track: Track) -> QueueEntryId {
        self.insert_all_next([track])
            .pop()
            .expect("one track inserts one entry")
    }

    /// [`Queue::insert_next`] for several tracks, in order, as one splice.
    /// Returns their ids.
    pub fn insert_all_next(
        &mut self,
        tracks: impl IntoIterator<Item = Track>,
    ) -> Vec<QueueEntryId> {
        let entries: Vec<QueueEntry> = tracks.into_iter().map(|t| self.new_entry(t)).collect();
        let ids: Vec<QueueEntryId> = entries.iter().map(|e| e.id).collect();
        if entries.is_empty() {
            return ids;
        }
        match self.current.and_then(|id| self.position(id)) {
            Some(current) => {
                let at = self
                    .next_anchor
                    .and_then(|id| self.position(id))
                    .filter(|&anchor| anchor > current)
                    .unwrap_or(current)
                    + 1;
                self.entries.splice(at..at, entries);
                self.next_anchor = ids.last().copied();
            }
            None => self.entries.extend(entries),
        }
        self.changed();
        ids
    }

    /// Makes `id` current. Returns `false` (and changes nothing) if `id` is
    /// not in the queue.
    pub fn set_current(&mut self, id: QueueEntryId) -> bool {
        if self.position(id).is_some() {
            if self.current != Some(id) {
                self.current = Some(id);
                self.next_anchor = None;
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
        self.next_anchor = None;
        self.changed();
        Some(next)
    }

    /// Moves `current` to the preceding entry and returns it. At the first
    /// entry, or with no current entry, returns `None` and leaves `current`
    /// unchanged.
    pub fn move_to_previous(&mut self) -> Option<QueueEntryId> {
        let prev = self.id_before(self.current?)?;
        self.current = Some(prev);
        self.next_anchor = None;
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
            // A new current entry: the "play next" run belonged to the old.
            self.next_anchor = None;
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

    /// Moves the entries `ids` (unknown ones are ignored; duplicates count
    /// once) as one block, in their queue order, to the gap `to`: the
    /// place before the entry now at index `to` (`len` = the end), as a
    /// drop between two rows names it. Returns `false` (and changes
    /// nothing) if no known entry is given or the order stays the same.
    /// `current` keeps pointing at the same entry.
    pub fn move_entries(&mut self, ids: &[QueueEntryId], to: usize) -> bool {
        let to = to.min(self.entries.len());
        let ids: std::collections::HashSet<QueueEntryId> = ids.iter().copied().collect();
        let moving: Vec<bool> = self.entries.iter().map(|e| ids.contains(&e.id)).collect();
        if !moving.contains(&true) {
            return false;
        }
        // The gap, counted among the entries that stay.
        let at = moving[..to].iter().filter(|m| !**m).count();
        let old: Vec<QueueEntryId> = self.entries.iter().map(|e| e.id).collect();
        let (mut block, mut rest) = (Vec::new(), Vec::new());
        for (entry, moved) in self.entries.drain(..).zip(moving) {
            if moved {
                block.push(entry);
            } else {
                rest.push(entry);
            }
        }
        let tail = rest.split_off(at);
        rest.extend(block);
        rest.extend(tail);
        self.entries = rest;
        let same = self.entries.iter().map(|e| e.id).eq(old.iter().copied());
        if !same {
            self.changed();
        }
        !same
    }

    /// Removes every entry. Ids are never reused afterwards.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.current = None;
        self.next_anchor = None;
        self.changed();
    }

    /// Shuffles the entries after the current one (all entries when there
    /// is no current one) with a Fisher–Yates shuffle. `random_below(n)`
    /// must return a value in `0..n`; out-of-range values are reduced
    /// modulo `n`. The current entry and everything before it stay in
    /// place. Returns the number of entries shuffled.
    pub fn shuffle_after_current(&mut self, mut random_below: impl FnMut(usize) -> usize) -> usize {
        self.next_anchor = None;
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

/// Shuffles `items` for "play shuffled" (`s` in the library) with a
/// Fisher–Yates shuffle. With `first` (an index into `items`), that item
/// goes to the front and the rest are shuffled behind it; an index out of
/// range is ignored.
pub fn shuffle_with_first<T>(items: &mut [T], first: Option<usize>, rng: &mut Rng) {
    let rest = match first.filter(|&i| i < items.len()) {
        Some(i) => {
            items.swap(0, i);
            &mut items[1..]
        }
        None => items,
    };
    for i in (1..rest.len()).rev() {
        let j = rng.below(i + 1);
        rest.swap(i, j);
    }
}

/// Keeps a uniformly random sample of `k` items (all of them, if there
/// are fewer), in random order (a partial Fisher–Yates shuffle).
pub fn sample<T>(items: &mut Vec<T>, k: usize, rng: &mut Rng) {
    let k = k.min(items.len());
    for i in 0..k {
        let j = i + rng.below(items.len() - i);
        items.swap(i, j);
    }
    items.truncate(k);
}

/// "Shuffle all": every track of `all` in random order; from a larger
/// set, a uniformly random sample of [`MAX_QUEUE_LEN`] (the queue cap).
pub fn shuffle_all(all: &[Track], rng: &mut Rng) -> Vec<Track> {
    let mut picked: Vec<usize> = (0..all.len()).collect();
    sample(&mut picked, MAX_QUEUE_LEN, rng);
    picked.into_iter().map(|i| all[i].clone()).collect()
}

/// The status after "Shuffle all" queued `picked` tracks out of `total`:
/// `Shuffling all (10,000 of 12,345 tracks)`. `partial`: the library was
/// not completely scanned, so `total` is what was scanned so far.
pub fn shuffle_all_message(picked: usize, total: usize, partial: bool) -> String {
    let counts = if picked < total {
        format!("{} of {}", thousands(picked), count(total, "track"))
    } else {
        count(picked, "track")
    };
    let scope = if partial { " scanned so far" } else { "" };
    format!("Shuffling all ({counts}{scope})")
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

    #[test]
    fn shuffle_all_keeps_every_track_once_in_random_order() {
        let all: Vec<Track> = (0..50).map(|i| t(&format!("{i:02}"))).collect();
        let mut rng = Rng::from_seed(3);
        let shuffled = shuffle_all(&all, &mut rng);
        let names = |tracks: &[Track]| tracks.iter().map(|t| t.title.clone()).collect::<Vec<_>>();
        assert_ne!(names(&shuffled), names(&all), "shuffled");
        let mut sorted = names(&shuffled);
        sorted.sort();
        assert_eq!(sorted, names(&all), "every track once");
        assert!(shuffle_all(&[], &mut rng).is_empty());
    }

    #[test]
    fn shuffle_all_samples_a_library_over_the_queue_cap() {
        let total = MAX_QUEUE_LEN + 5;
        let all: Vec<Track> = (0..total).map(|i| t(&format!("{i:05}"))).collect();
        let picked = shuffle_all(&all, &mut Rng::from_seed(11));
        assert_eq!(picked.len(), MAX_QUEUE_LEN);
        let mut names: Vec<String> = picked.iter().map(|t| t.title.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), MAX_QUEUE_LEN, "no track twice");
        let first_n: Vec<String> = (0..MAX_QUEUE_LEN).map(|i| format!("{i:05}")).collect();
        assert_ne!(names, first_n, "a random sample, not the first tracks");
    }

    #[test]
    fn shuffle_all_message_counts_the_sample_and_a_partial_scan() {
        assert_eq!(
            shuffle_all_message(MAX_QUEUE_LEN, MAX_QUEUE_LEN + 5, false),
            "Shuffling all (10,000 of 10,005 tracks)"
        );
        assert_eq!(
            shuffle_all_message(2, 2, true),
            "Shuffling all (2 tracks scanned so far)"
        );
        assert_eq!(shuffle_all_message(1, 1, false), "Shuffling all (1 track)");
        assert_eq!(
            shuffle_all_message(3, 7, true),
            "Shuffling all (3 of 7 tracks scanned so far)"
        );
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
    fn move_entries_moves_a_block_to_a_gap() {
        let mut q = Queue::new();
        let a = q.append(t("A"));
        let b = q.append(t("B"));
        let c = q.append(t("C"));
        let d = q.append(t("D"));
        q.set_current(b);
        let rev = q.revision();
        // One entry down: the gap after C (index 3).
        assert!(q.move_entries(&[a], 3));
        assert_eq!(titles(&q), ["B*", "C", "A", "D"]);
        assert!(q.revision() > rev);
        // To the end.
        assert!(q.move_entries(&[b], 4));
        assert_eq!(titles(&q), ["C", "A", "D", "B*"]);
        assert_eq!(q.current(), Some(b));
        // To the start; a gap past the end is the end.
        assert!(q.move_entries(&[b], 0));
        assert_eq!(titles(&q), ["B*", "C", "A", "D"]);
        assert!(q.move_entries(&[c], 99));
        assert_eq!(titles(&q), ["B*", "A", "D", "C"]);
        // A non-contiguous selection becomes one block in queue order,
        // whatever order the ids were given in.
        assert!(q.move_entries(&[c, a, c], 1));
        assert_eq!(titles(&q), ["B*", "A", "C", "D"]);
        assert!(q.move_entries(&[d, b], 2));
        assert_eq!(titles(&q), ["A", "B*", "D", "C"]);
        let _ = d;
    }

    #[test]
    fn move_entries_without_a_change_does_nothing() {
        let mut q = Queue::new();
        let a = q.append(t("A"));
        let b = q.append(t("B"));
        let c = q.append(t("C"));
        let rev = q.revision();
        // Gaps around the entry itself leave it where it is.
        assert!(!q.move_entries(&[b], 1));
        assert!(!q.move_entries(&[b], 2));
        assert!(!q.move_entries(&[a, b], 0));
        assert!(!q.move_entries(&[a, b, c], 3));
        assert!(!q.move_entries(&[], 0));
        q.remove(c);
        let rev2 = q.revision();
        assert!(rev2 > rev);
        assert!(!q.move_entries(&[c], 0), "unknown id");
        assert_eq!(q.revision(), rev2);
        assert_eq!(titles(&q), ["A", "B"]);
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
    fn consecutive_play_next_inserts_keep_their_order() {
        let mut q = Queue::new();
        let a = q.append(t("A"));
        q.append(t("B"));
        q.set_current(a);
        q.insert_next(t("X"));
        q.insert_next(t("Y"));
        q.insert_next(t("Z"));
        assert_eq!(titles(&q), ["A*", "X", "Y", "Z", "B"]);
        // Appending does not break the run.
        q.append(t("E"));
        q.insert_next(t("W"));
        assert_eq!(titles(&q), ["A*", "X", "Y", "Z", "W", "B", "E"]);
    }

    #[test]
    fn play_next_without_current_appends_and_remembers_nothing() {
        let mut q = Queue::new();
        q.insert_next(t("X"));
        q.append(t("A"));
        q.insert_next(t("Y"));
        assert_eq!(titles(&q), ["X", "A", "Y"]);
        assert_eq!(q.next_anchor(), None);
    }

    #[test]
    fn the_play_next_point_resets_when_current_moves_or_the_order_changes() {
        // Each case starts as A* X(anchor) B and then disturbs the run;
        // the next insert goes right after the current entry again.
        type Disturb = fn(&mut Queue, [QueueEntryId; 3]);
        let cases: [(&str, Disturb, &[&str]); 3] = [
            (
                "next",
                |q, _| {
                    q.move_to_next();
                },
                &["A", "X*", "Y", "B"],
            ),
            (
                "set current",
                |q, [_, _, b]| {
                    q.set_current(b);
                },
                &["A", "X", "B*", "Y"],
            ),
            (
                "shuffle",
                |q, _| {
                    q.shuffle_after_current(|n| n - 1);
                },
                &["A*", "Y", "X", "B"],
            ),
        ];
        for (name, disturb, expected) in cases {
            let mut q = Queue::new();
            let a = q.append(t("A"));
            let b = q.append(t("B"));
            q.set_current(a);
            let x = q.insert_next(t("X"));
            assert_eq!(q.next_anchor(), Some(x), "{name}");
            disturb(&mut q, [a, x, b]);
            assert_eq!(q.next_anchor(), None, "{name}");
            q.insert_next(t("Y"));
            assert_eq!(titles(&q), expected, "{name}");
        }
        // Going back: A B* X(anchor), then `<`.
        let mut q = Queue::new();
        q.append(t("A"));
        let b = q.append(t("B"));
        q.set_current(b);
        q.insert_next(t("X"));
        q.move_to_previous();
        assert_eq!(q.next_anchor(), None);
        q.insert_next(t("Y"));
        assert_eq!(titles(&q), ["A*", "Y", "B", "X"]);
        q.clear();
        assert_eq!(q.next_anchor(), None);
    }

    #[test]
    fn moving_and_removing_keep_the_play_next_point_where_it_is_still_valid() {
        // A* X(anchor) B: removing B keeps the run.
        let mut q = Queue::new();
        let a = q.append(t("A"));
        let b = q.append(t("B"));
        q.set_current(a);
        let x = q.insert_next(t("X"));
        q.remove(b);
        assert_eq!(q.next_anchor(), Some(x));
        q.insert_next(t("Y"));
        assert_eq!(titles(&q), ["A*", "X", "Y"]);
        // Moving the anchor further down: inserts follow it.
        let mut q = Queue::new();
        let a = q.append(t("A"));
        q.append(t("B"));
        q.set_current(a);
        let x = q.insert_next(t("X"));
        q.move_down(x);
        q.insert_next(t("Y"));
        assert_eq!(titles(&q), ["A*", "B", "X", "Y"]);
        // Moved above the current entry, or removed: right after current.
        let mut q = Queue::new();
        let a = q.append(t("A"));
        q.set_current(a);
        let x = q.insert_next(t("X"));
        q.move_up(x);
        q.insert_next(t("Y"));
        assert_eq!(titles(&q), ["X", "A*", "Y"]);
        let y = q.next_anchor().unwrap();
        q.remove(y);
        q.insert_next(t("Z"));
        assert_eq!(titles(&q), ["X", "A*", "Z"]);
    }

    #[test]
    fn removing_the_current_entry_forgets_the_play_next_point() {
        // A* X(anchor) B: removing A makes X current; Y goes right after.
        let mut q = Queue::new();
        let a = q.append(t("A"));
        q.append(t("B"));
        q.set_current(a);
        q.insert_next(t("X"));
        q.remove(a);
        assert_eq!(q.next_anchor(), None);
        q.insert_next(t("Y"));
        assert_eq!(titles(&q), ["X*", "Y", "B"]);
    }

    #[test]
    fn several_tracks_go_next_as_one_run() {
        let mut q = Queue::new();
        let a = q.append(t("A"));
        q.append(t("B"));
        q.set_current(a);
        let ids = q.insert_all_next([t("X"), t("Y")]);
        assert_eq!(ids.len(), 2);
        assert_eq!(q.next_anchor(), Some(ids[1]));
        q.insert_all_next([t("Z")]);
        assert!(q.insert_all_next([]).is_empty());
        assert_eq!(titles(&q), ["A*", "X", "Y", "Z", "B"]);
    }

    #[test]
    fn replaying_the_current_entry_keeps_the_play_next_point() {
        let mut q = Queue::new();
        let a = q.append(t("A"));
        q.set_current(a);
        let x = q.insert_next(t("X"));
        // Repeat-one reloads make the same entry current again.
        assert!(q.set_current(a));
        assert_eq!(q.next_anchor(), Some(x));
        q.insert_next(t("Y"));
        assert_eq!(titles(&q), ["A*", "X", "Y"]);
    }

    #[test]
    fn shuffle_with_first_puts_it_first_and_keeps_the_multiset() {
        let original: Vec<u32> = (0..50).collect();
        for first in [None, Some(0), Some(17), Some(49), Some(50)] {
            let mut items = original.clone();
            shuffle_with_first(&mut items, first, &mut Rng::from_seed(9));
            if let Some(i) = first.filter(|&i| i < 50) {
                assert_eq!(items[0], original[i], "{first:?}");
            }
            let mut sorted = items.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, original, "{first:?}");
            assert_ne!(items, original, "{first:?} shuffled");
            // The same seed gives the same order.
            let mut again = original.clone();
            shuffle_with_first(&mut again, first, &mut Rng::from_seed(9));
            assert_eq!(items, again, "{first:?}");
        }
        let mut empty: Vec<u32> = Vec::new();
        shuffle_with_first(&mut empty, Some(0), &mut Rng::from_seed(1));
        assert!(empty.is_empty());
    }

    #[test]
    fn sample_keeps_k_distinct_items_and_reaches_every_one() {
        let original: Vec<u32> = (0..100).collect();
        // Fewer than `k`: all of them, shuffled.
        let mut all = original.clone();
        sample(&mut all, 1_000, &mut Rng::from_seed(3));
        let mut sorted = all.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, original);
        assert_ne!(all, original);
        // More than `k`: `k` distinct items, not just the first `k`.
        let mut seen = [false; 100];
        for seed in 0..50 {
            let mut items = original.clone();
            sample(&mut items, 10, &mut Rng::from_seed(seed));
            assert_eq!(items.len(), 10);
            let mut distinct = items.clone();
            distinct.sort_unstable();
            distinct.dedup();
            assert_eq!(distinct.len(), 10, "{seed}");
            for i in items {
                seen[i as usize] = true;
            }
        }
        assert!(seen[90..].iter().any(|s| *s), "the tail is sampled too");
        assert!(seen.iter().filter(|s| **s).count() > 90);
        let mut empty: Vec<u32> = Vec::new();
        sample(&mut empty, 5, &mut Rng::from_seed(1));
        assert!(empty.is_empty());
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
