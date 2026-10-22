//! The play queue: an ordered list of entries plus an optional current
//! entry. Pure data; no I/O.
//!
//! Entries are addressed by [`QueueEntryId`], never by an index captured
//! earlier, so concurrent edits cannot make an operation hit the wrong
//! entry. The same track may appear several times as distinct entries.

use crate::model::Track;

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
        id
    }

    /// Makes `id` current. Returns `false` (and changes nothing) if `id` is
    /// not in the queue.
    pub fn set_current(&mut self, id: QueueEntryId) -> bool {
        if self.position(id).is_some() {
            self.current = Some(id);
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
        Some(next)
    }

    /// Moves `current` to the preceding entry and returns it. At the first
    /// entry, or with no current entry, returns `None` and leaves `current`
    /// unchanged.
    pub fn move_to_previous(&mut self) -> Option<QueueEntryId> {
        let prev = self.id_before(self.current?)?;
        self.current = Some(prev);
        Some(prev)
    }

    /// Removes an entry. Removing a non-current entry never changes
    /// `current`. If the current entry is removed (the player decides what
    /// that means for playback first), `current` moves to the entry that
    /// took its place, else the one before it, else `None`.
    pub fn remove(&mut self, id: QueueEntryId) -> Option<QueueEntry> {
        let pos = self.position(id)?;
        let entry = self.entries.remove(pos);
        if self.current == Some(id) {
            self.current = self
                .entries
                .get(pos)
                .or_else(|| pos.checked_sub(1).and_then(|p| self.entries.get(p)))
                .map(|e| e.id);
        }
        Some(entry)
    }

    fn new_entry(&mut self, track: Track) -> QueueEntry {
        self.next_id += 1;
        QueueEntry {
            id: QueueEntryId(self.next_id),
            track,
        }
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
}
