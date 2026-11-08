//! Navidrome playlist edits as small sequential jobs. No Ratatui.
//!
//! The browser decides what to change and emits an [`EditRequest`] as
//! data; the event loop runs it with [`spawn_edit`], and the
//! [`EditResponse`] comes back to the browser. One edit runs at a time,
//! and the requests of one edit run in order, one after the other.
//!
//! Rules:
//! - A removal is sent as a replace of the whole list without that entry,
//!   never as `updatePlaylist`'s `songIndexToRemove`: Navidrome's
//!   `getPlaylist` hides entries whose file is missing, but its indexes
//!   count them, so an index from the shown list can name another entry.
//!   A replace drops those hidden entries on the server, and an empty
//!   replace is ignored by Navidrome, so the last song cannot be removed.
//! - Removing and replacing the order depend on the playlist the
//!   user saw. Both first refetch the playlist and compare its entries
//!   (song ids in order, nothing skipped or cut); on any difference
//!   nothing is written and the result is [`EditResult::Stale`].
//! - Song ids are split into requests that fit the server's transport
//!   ([`Client::id_batches`]). A replace is never split (a partial
//!   replace would lose songs): if it does not fit one request it is
//!   refused before anything is sent.
//! - A multi-request edit that fails part-way reports what was done
//!   ([`EditResult::Partial`]); the browser then reloads the playlist.
//! - Every spawned edit answers exactly once: if its task is dropped or
//!   panics first, an [`Error::Aborted`] failure is sent instead.
//! - Song ids, names and errors may be logged by kind; request URLs and
//!   bodies never are (the client does not log them).

use tokio::sync::mpsc;

use super::browse::Generation;
use super::{Client, Error};

/// Identifies an edit within one browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EditId(pub u64);

/// A change to the user's playlists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOp {
    /// `createPlaylist` named `name` with the first songs that fit, then
    /// `updatePlaylist` adds for the rest, in order.
    Create {
        name: String,
        songs: Vec<String>,
    },
    /// `updatePlaylist` adds, in order, in as many requests as needed.
    Add {
        playlist_id: String,
        songs: Vec<String>,
    },
    Rename {
        playlist_id: String,
        name: String,
    },
    Delete {
        playlist_id: String,
    },
    /// Removes the entry at `index`, if the playlist still holds exactly
    /// `expected`, by replacing the list (see the module docs).
    Remove {
        playlist_id: String,
        index: usize,
        expected: Vec<String>,
        /// Entries the server hides that the user agreed to lose
        /// ([`super::Hidden::bound`]); more on the refetch is stale.
        hidden: usize,
    },
    /// Replaces the songs with `songs`, if the playlist still holds
    /// exactly `expected` (and at most `hidden` hidden entries).
    Replace {
        playlist_id: String,
        expected: Vec<String>,
        songs: Vec<String>,
        hidden: usize,
    },
}

impl EditOp {
    /// Short name for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create",
            Self::Add { .. } => "add",
            Self::Rename { .. } => "rename",
            Self::Delete { .. } => "delete",
            Self::Remove { .. } => "remove",
            Self::Replace { .. } => "replace",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRequest {
    pub generation: Generation,
    pub id: EditId,
    pub op: EditOp,
}

#[derive(Debug)]
pub struct EditResponse {
    pub generation: Generation,
    pub id: EditId,
    pub result: EditResult,
}

/// What an edit did.
#[derive(Debug)]
pub enum EditResult {
    /// Every request succeeded. `created` is the new playlist's id, for a
    /// create whose server sent it back.
    Done { created: Option<String> },
    /// Some requests succeeded, then one failed: `added` songs were added
    /// (to the playlist `created`, for a create, which then exists).
    Partial {
        created: Option<String>,
        added: usize,
        error: Error,
    },
    /// The playlist changed on the server since the user saw it; nothing
    /// was written.
    Stale,
    /// Nothing was changed (as far as Coda knows: a request that timed out
    /// may still have been applied).
    Failed(Error),
}

/// Runs `op`'s requests in order.
pub async fn run(client: &Client, op: &EditOp) -> EditResult {
    match op {
        EditOp::Create { name, songs } => create(client, name, songs).await,
        EditOp::Add { playlist_id, songs } => add(client, playlist_id, songs, 0, None).await,
        EditOp::Rename { playlist_id, name } => {
            single(client.rename_playlist(playlist_id, name).await)
        }
        EditOp::Delete { playlist_id } => single(client.delete_playlist(playlist_id).await),
        EditOp::Remove {
            playlist_id,
            index,
            expected,
            hidden,
        } => {
            if *index >= expected.len() {
                return EditResult::Stale;
            }
            let mut songs = expected.clone();
            songs.remove(*index);
            replace(client, playlist_id, expected, &songs, *hidden).await
        }
        EditOp::Replace {
            playlist_id,
            expected,
            songs,
            hidden,
        } => replace(client, playlist_id, expected, songs, *hidden).await,
    }
}

/// Replaces the songs of `playlist_id` with `songs` in one request, if it
/// still holds exactly `expected`.
async fn replace(
    client: &Client,
    playlist_id: &str,
    expected: &[String],
    songs: &[String],
    hidden: usize,
) -> EditResult {
    if songs.is_empty() {
        return EditResult::Failed(Error::EmptyReplace);
    }
    // Refused before the refetch if it can never be sent.
    match client
        .id_batches(
            "createPlaylist",
            &[("playlistId", playlist_id)],
            "songId",
            songs,
        )
        .await
    {
        Ok(batches) if batches.len() == 1 => {}
        Ok(_) => return EditResult::Failed(Error::RequestTooLarge),
        Err(error) => return EditResult::Failed(error),
    }
    if let Err(result) = check_unchanged(client, playlist_id, expected, hidden).await {
        return result;
    }
    single(client.replace_playlist(playlist_id, songs).await)
}

fn single(result: Result<(), Error>) -> EditResult {
    match result {
        Ok(()) => EditResult::Done { created: None },
        Err(error) => EditResult::Failed(error),
    }
}

/// `Ok` if the playlist on the server holds exactly `expected`, in order,
/// with every entry readable and at most `hidden` entries hidden.
async fn check_unchanged(
    client: &Client,
    playlist_id: &str,
    expected: &[String],
    hidden: usize,
) -> Result<(), EditResult> {
    match client.playlist(playlist_id).await {
        // More hidden entries than the user agreed to lose is a change too:
        // a replace never drops more than was confirmed.
        Ok(fresh)
            if fresh.is_complete()
                && fresh.song_ids() == expected
                && fresh.hidden_entries().bound() <= hidden =>
        {
            Ok(())
        }
        Ok(_) => Err(EditResult::Stale),
        Err(error) => Err(EditResult::Failed(error)),
    }
}

async fn create(client: &Client, name: &str, songs: &[String]) -> EditResult {
    let batches = match client
        .id_batches("createPlaylist", &[("name", name)], "songId", songs)
        .await
    {
        Ok(batches) => batches,
        Err(error) => return EditResult::Failed(error),
    };
    let first = batches.first().cloned().unwrap_or(0..0);
    let created = match client.create_playlist(name, &songs[first.clone()]).await {
        Ok(created) => created,
        Err(error) => return EditResult::Failed(error),
    };
    if first.end == songs.len() {
        return EditResult::Done { created };
    }
    let Some(id) = created.clone() else {
        // An old server that does not return the id: the rest cannot be
        // added to it.
        return EditResult::Partial {
            created: None,
            added: first.len(),
            error: Error::Decode("createPlaylist: no playlist id in the reply".to_owned()),
        };
    };
    add(client, &id, songs, first.end, created).await
}

/// Adds `songs[start..]` to `playlist_id` in batches; `start` songs (and
/// the playlist itself, with `created`) already count as done.
async fn add(
    client: &Client,
    playlist_id: &str,
    songs: &[String],
    start: usize,
    created: Option<String>,
) -> EditResult {
    let rest = &songs[start.min(songs.len())..];
    if rest.is_empty() {
        return EditResult::Done { created };
    }
    let failed = |created: Option<String>, added: usize, error: Error| {
        if added == 0 && created.is_none() {
            EditResult::Failed(error)
        } else {
            EditResult::Partial {
                created,
                added,
                error,
            }
        }
    };
    let batches = match client
        .id_batches(
            "updatePlaylist",
            &[("playlistId", playlist_id)],
            "songIdToAdd",
            rest,
        )
        .await
    {
        Ok(batches) => batches,
        Err(error) => return failed(created, start, error),
    };
    let mut added = start;
    for batch in batches {
        let len = batch.len();
        if let Err(error) = client.add_to_playlist(playlist_id, &rest[batch]).await {
            return failed(created, added, error);
        }
        added += len;
    }
    EditResult::Done { created }
}

/// Runs `request` on a Tokio task and sends its response to `responses`.
/// Never blocks the caller; answers exactly once.
pub fn spawn_edit(
    client: Client,
    request: EditRequest,
    responses: mpsc::UnboundedSender<EditResponse>,
) -> tokio::task::JoinHandle<()> {
    // Built before spawning, so even a task dropped before its first poll
    // answers.
    let reply = Reply {
        responses,
        generation: request.generation,
        id: request.id,
        sent: false,
    };
    tokio::spawn(async move {
        let result = run(&client, &request.op).await;
        match &result {
            EditResult::Failed(error) | EditResult::Partial { error, .. } => {
                // Only the detail: `Display` can carry untrusted server
                // text, and `Debug` of network errors the server address.
                tracing::warn!(
                    edit = request.op.kind(),
                    detail = %error.detail(),
                    "navidrome playlist edit failed"
                );
            }
            EditResult::Stale => {
                tracing::info!(edit = request.op.kind(), "playlist changed on the server");
            }
            EditResult::Done { .. } => {
                tracing::debug!(edit = request.op.kind(), "navidrome playlist edit done");
            }
        }
        reply.send(result);
    })
}

/// Sends an edit's response exactly once; dropped without sending, it
/// sends [`Error::Aborted`].
struct Reply {
    responses: mpsc::UnboundedSender<EditResponse>,
    generation: Generation,
    id: EditId,
    sent: bool,
}

impl Reply {
    fn send(mut self, result: EditResult) {
        self.send_once(result);
    }

    fn send_once(&mut self, result: EditResult) {
        if !self.sent {
            self.sent = true;
            let _ = self.responses.send(EditResponse {
                generation: self.generation,
                id: self.id,
                result,
            });
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if self.sent || self.responses.is_closed() {
            return;
        }
        tracing::warn!(
            id = self.id.0,
            "navidrome playlist edit ended without a result"
        );
        self.send_once(EditResult::Failed(Error::Aborted));
    }
}
