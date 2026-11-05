//! Navidrome playlist edits (M3): create, save the queue as a playlist,
//! add, rename, delete, remove entries and reorder, through
//! `coda_core::navidrome::edit` (Coda's rules: removals and reorders are
//! a replace of the whole list after a refetch that must match what the
//! user saw; stale and partial results are reported).
//!
//! - One edit runs at a time, like Coda's `EditRunner`: a [`PlaylistEditor`]
//!   (managed state) holds a lock for the edit's whole run, so a second
//!   edit waits for the first. The quit sequence waits for it, bounded.
//! - Edits use the browse handle's current client. A result that arrives
//!   after the client changed is reported as such (the old server's change
//!   completed or not; nothing is retried with the new one).
//! - Every outcome becomes a notice (through the engine) and is returned
//!   to the caller, which reloads the affected views.
//! - Song ids, names and errors may be logged by kind; nothing else is
//!   (the client never logs URLs or credentials).

pub mod commands;
pub mod dto;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use coda_core::model::thousands;
use coda_core::navidrome::api::MAX_PLAYLIST_ENTRIES;
use coda_core::navidrome::edit::{self, EditOp, EditResult};
use coda_core::navidrome::{Client, Error};
use tokio::sync::Mutex;

use crate::browse::dto::HiddenDto;
use dto::{EditOutcomeDto, EditStatus};

/// Longest playlist name accepted (characters, after trimming).
pub const MAX_NAME_CHARS: usize = 256;
/// Longest song id accepted.
const MAX_ID_CHARS: usize = 256;
/// Bound on waiting for a running edit at exit.
pub const EDIT_GRACE: Duration = Duration::from_secs(3);

/// Navidrome ignores an empty replace (Coda's `LAST_SONG`).
pub const LAST_SONG: &str = "Cannot remove every song; delete the playlist instead";
pub const STALE_PLAYLIST: &str = "The playlist changed on the server; reloaded";
pub const CLIENT_CHANGED: &str = "The server connection changed during the change; reloaded";
pub const NO_SERVER: &str = "No Navidrome server is configured";
pub const NO_REMOTE_IN_QUEUE: &str = "The queue has no Navidrome tracks to save";

/// Serializes playlist edits; see the module docs.
#[derive(Default)]
pub struct PlaylistEditor {
    running: Mutex<()>,
    next_id: AtomicU64,
}

/// One validated edit and what its message needs.
#[derive(Debug, Clone)]
pub struct Edit {
    pub op: EditOp,
    /// The playlist's name (the new name, for a create or rename).
    pub playlist: String,
    /// The old name of a renamed playlist.
    pub old_name: String,
    /// For a save of the queue: local tracks left out.
    pub left_out: usize,
}

impl PlaylistEditor {
    /// Runs `edit` with `client` once every earlier edit has answered.
    /// `still_current` is asked afterwards whether the client is still the
    /// one in use.
    pub async fn run(
        &self,
        client: &Client,
        edit: Edit,
        still_current: impl FnOnce() -> bool,
    ) -> EditOutcomeDto {
        let _running = self.running.lock().await;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(id, edit = edit.op.kind(), "navidrome playlist edit started");
        let result = edit::run(client, &edit.op).await;
        log_result(&edit.op, &result);
        let mut outcome = outcome(&edit, &result);
        if !still_current() {
            outcome.status = EditStatus::Stale;
            outcome.message = CLIENT_CHANGED.to_owned();
        }
        outcome
    }

    /// Waits up to `wait` for a running edit (at exit).
    pub async fn finish(&self, wait: Duration) {
        if tokio::time::timeout(wait, self.running.lock())
            .await
            .is_err()
        {
            tracing::warn!("a playlist change did not finish before exit");
        }
    }
}

fn log_result(op: &EditOp, result: &EditResult) {
    match result {
        EditResult::Failed(error) | EditResult::Partial { error, .. } => {
            // Only the detail: `Display` can carry untrusted server text.
            tracing::warn!(
                edit = op.kind(),
                detail = %error.detail(),
                "navidrome playlist edit failed"
            );
        }
        EditResult::Stale => tracing::info!(edit = op.kind(), "playlist changed on the server"),
        EditResult::Done { .. } => {
            tracing::debug!(edit = op.kind(), "navidrome playlist edit done")
        }
    }
}

// ----- validation -----------------------------------------------------------

/// A name as typed: trimmed, control characters dropped; `Err` when
/// blank or too long.
pub fn clean_name(name: &str) -> Result<String, String> {
    let name: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let name = name.trim();
    if name.is_empty() {
        return Err("The name cannot be empty".to_owned());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "The name is too long (at most {MAX_NAME_CHARS} characters)"
        ));
    }
    Ok(name.to_owned())
}

/// A playlist name for messages only (from the UI; never sent).
pub fn shown_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_CHARS)
        .collect();
    let name = name.trim();
    if name.is_empty() {
        "the playlist".to_owned()
    } else {
        name.to_owned()
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().count() <= MAX_ID_CHARS && !id.chars().any(char::is_control)
}

/// Song or playlist ids from the UI: each non-empty and bounded, at most
/// a playlist's worth.
pub fn check_ids(ids: &[String], what: &str) -> Result<(), String> {
    if ids.len() > MAX_PLAYLIST_ENTRIES {
        return Err(format!(
            "Too many songs (at most {})",
            thousands(MAX_PLAYLIST_ENTRIES)
        ));
    }
    if ids.iter().any(|id| !valid_id(id)) {
        return Err(format!("Not a valid {what}"));
    }
    Ok(())
}

pub fn check_playlist_id(id: &str) -> Result<(), String> {
    if valid_id(id) {
        Ok(())
    } else {
        Err("Not a valid playlist".to_owned())
    }
}

/// The hidden entries the user agreed to lose ([`coda_core::navidrome::Hidden::bound`]).
pub fn hidden_bound(hidden: HiddenDto) -> usize {
    match hidden {
        HiddenDto::None => 0,
        HiddenDto::Some { count } => count,
        HiddenDto::Suspect => usize::MAX,
    }
}

/// The remove edit for `indices` (into `expected`, the songs as shown):
/// a single removal, or a replace without them.
pub fn removal(
    playlist_id: String,
    expected: Vec<String>,
    indices: &[usize],
    hidden: usize,
) -> Result<EditOp, String> {
    let mut drop = vec![false; expected.len()];
    for &i in indices {
        match drop.get_mut(i) {
            Some(slot) => *slot = true,
            None => return Err(STALE_PLAYLIST.to_owned()),
        }
    }
    let removed = drop.iter().filter(|d| **d).count();
    if removed == 0 {
        return Err("Nothing to remove".to_owned());
    }
    if removed == expected.len() {
        return Err(LAST_SONG.to_owned());
    }
    if removed == 1 {
        let index = drop.iter().position(|d| *d).unwrap_or(0);
        return Ok(EditOp::Remove {
            playlist_id,
            index,
            expected,
            hidden,
        });
    }
    let songs = expected
        .iter()
        .zip(&drop)
        .filter(|(_, d)| !**d)
        .map(|(s, _)| s.clone())
        .collect();
    Ok(EditOp::Replace {
        playlist_id,
        expected,
        songs,
        hidden,
    })
}

/// The reorder edit: `order` must be a permutation of `expected`'s
/// indices. `Ok(None)` when it is the shown order.
pub fn reorder(
    playlist_id: String,
    expected: Vec<String>,
    order: &[usize],
    hidden: usize,
) -> Result<Option<EditOp>, String> {
    let mut seen = vec![false; expected.len()];
    if order.len() != expected.len() {
        return Err(STALE_PLAYLIST.to_owned());
    }
    for &i in order {
        match seen.get_mut(i) {
            Some(slot) if !*slot => *slot = true,
            _ => return Err(STALE_PLAYLIST.to_owned()),
        }
    }
    if order.iter().enumerate().all(|(at, &i)| at == i) {
        return Ok(None);
    }
    let songs = order.iter().map(|&i| expected[i].clone()).collect();
    Ok(Some(EditOp::Replace {
        playlist_id,
        expected,
        songs,
        hidden,
    }))
}

// ----- outcomes ---------------------------------------------------------

fn songs_text(n: usize) -> String {
    format!("{} song{}", thousands(n), if n == 1 { "" } else { "s" })
}

/// What a finished edit tells the user (Coda's `edit_message`, with GUI
/// wording).
pub fn outcome(edit: &Edit, result: &EditResult) -> EditOutcomeDto {
    let playlist = &edit.playlist;
    let op = &edit.op;
    let (status, created) = match result {
        EditResult::Done { created } => (EditStatus::Done, created.clone()),
        EditResult::Partial { created, .. } => (EditStatus::Partial, created.clone()),
        EditResult::Stale => (EditStatus::Stale, None),
        EditResult::Failed(_) => (EditStatus::Failed, None),
    };
    let message = match result {
        EditResult::Stale => STALE_PLAYLIST.to_owned(),
        EditResult::Done { .. } => match op {
            EditOp::Create { songs, .. } if songs.is_empty() => format!("Created {playlist}"),
            EditOp::Create { songs, .. } => {
                let left_out = match edit.left_out {
                    0 => String::new(),
                    1 => " (1 local track left out)".to_owned(),
                    n => format!(" ({} local tracks left out)", thousands(n)),
                };
                format!(
                    "Created {playlist} with {}{left_out}",
                    songs_text(songs.len())
                )
            }
            EditOp::Add { songs, .. } => {
                format!("Added {} to {playlist}", songs_text(songs.len()))
            }
            EditOp::Rename { .. } => format!("Renamed {} to {playlist}", edit.old_name),
            EditOp::Delete { .. } => format!("Deleted {playlist}"),
            EditOp::Remove { .. } => format!("Removed 1 song from {playlist}"),
            EditOp::Replace {
                expected, songs, ..
            } if songs.len() < expected.len() => format!(
                "Removed {} from {playlist}",
                songs_text(expected.len() - songs.len())
            ),
            EditOp::Replace { .. } => format!("Saved the new order of {playlist}"),
        },
        EditResult::Failed(error) | EditResult::Partial { error, .. } => {
            failure(edit, result, error)
        }
    };
    EditOutcomeDto {
        status,
        message,
        created,
    }
}

fn failure(edit: &Edit, result: &EditResult, error: &Error) -> String {
    let playlist = &edit.playlist;
    match (&edit.op, result) {
        (EditOp::Create { songs, .. }, EditResult::Partial { added, .. }) => format!(
            "Created {playlist} with {} of {} songs; could not add the rest: {error}",
            thousands(*added),
            thousands(songs.len())
        ),
        (EditOp::Add { songs, .. }, EditResult::Partial { added, .. }) => format!(
            "Added {} of {} songs to {playlist}; could not add the rest: {error}",
            thousands(*added),
            thousands(songs.len())
        ),
        (EditOp::Create { .. }, _) => format!("Could not create {playlist}: {error}"),
        (EditOp::Add { .. }, _) => format!("Could not add to {playlist}: {error}"),
        (EditOp::Rename { .. }, _) => format!("Could not rename {}: {error}", edit.old_name),
        (EditOp::Delete { .. }, _) => format!("Could not delete {playlist}: {error}"),
        (
            EditOp::Remove { .. } | EditOp::Replace { .. },
            EditResult::Failed(Error::RequestTooLarge),
        ) => {
            format!("Could not change {playlist}: too many songs to change at once on this server")
        }
        (EditOp::Remove { .. }, _) => format!("Could not remove from {playlist}: {error}"),
        (
            EditOp::Replace {
                expected, songs, ..
            },
            _,
        ) if songs.len() < expected.len() => {
            format!("Could not remove from {playlist}: {error}")
        }
        (EditOp::Replace { .. }, _) => format!("Could not save the order of {playlist}: {error}"),
    }
}

/// An outcome for an edit refused before anything was sent.
pub fn refused(message: impl Into<String>) -> EditOutcomeDto {
    EditOutcomeDto {
        status: EditStatus::Refused,
        message: message.into(),
        created: None,
    }
}

#[cfg(test)]
mod tests;
