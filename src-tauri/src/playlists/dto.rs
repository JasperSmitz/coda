//! Playlist edit DTOs (camelCase JSON), mirrored in `src/lib/types.ts`
//! (the "Playlist edits" block).

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EditStatus {
    /// Every request succeeded.
    Done,
    /// Some requests succeeded, then one failed; the playlist changed.
    Partial,
    /// The playlist (or the server connection) changed since it was
    /// shown; nothing was written.
    Stale,
    /// Nothing was changed (as far as known: a timed-out request may
    /// still have been applied).
    Failed,
    /// Refused before anything was sent (invalid input, no server).
    Refused,
}

/// A playlist edit's outcome. `message` is also shown as a notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditOutcomeDto {
    pub status: EditStatus,
    pub message: String,
    /// The new playlist's id, for a create whose server sent it.
    pub created: Option<String>,
}
