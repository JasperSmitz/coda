//! What the Navidrome playlist editor ([`crate::playlists`]) needs from
//! the engine: the queue's songs for "Save queue as playlist", and a way
//! to show an edit's outcome as a notice. The edits themselves run
//! outside the engine (one at a time, on tasks), so a slow server never
//! blocks the loop.

use super::*;

pub enum PlaylistMsg {
    /// Show `text` as a notice.
    Notice(String),
    /// The queue's Navidrome song ids in order, and the number of local
    /// tracks left out.
    QueueSongs(oneshot::Sender<(Vec<String>, usize)>),
}

impl EngineHandle {
    /// Shows `text` as a notice (dropped while quitting).
    pub async fn notify(&self, text: String) {
        let _ = self.tx.send(Msg::Playlist(PlaylistMsg::Notice(text))).await;
    }

    /// The queue's Navidrome songs, in queue order, and how many local
    /// tracks it holds.
    pub async fn queue_songs(&self) -> Result<(Vec<String>, usize), EngineGone> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Msg::Playlist(PlaylistMsg::QueueSongs(tx)))
            .await
            .map_err(|_| EngineGone)?;
        rx.await.map_err(|_| EngineGone)
    }
}

impl Engine {
    pub(super) fn handle_playlist_msg(&mut self, msg: PlaylistMsg, now: Instant) {
        match msg {
            PlaylistMsg::Notice(text) => self.notify(text, now),
            PlaylistMsg::QueueSongs(reply) => {
                let _ = reply.send(self.queue_songs());
            }
        }
    }
}
