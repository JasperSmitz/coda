//! The playback boundary: [`PlaybackCommand`]s in, [`PlaybackEvent`]s out,
//! over Tokio channels. One concrete backend (mpv) sits behind it; nothing
//! outside `playback::mpv` knows which backend is in use.
//!
//! Contracts every backend upholds:
//! - `Load` always starts unpaused.
//! - `Load` of a [`PlaybackTarget::File`] that does not exist yields
//!   `TrackEnded { reason: Error("file not found") }` without starting the
//!   player. Streams are not pre-checked; a failing stream ends with
//!   `TrackEnded { reason: Error(..) }` from the player itself.
//! - The backend never sees track sources or credentials: the app resolves
//!   a queue entry's source into a [`PlaybackTarget`] right before sending.
//! - `Started`/`TrackEnded` carry the [`LoadId`] of the `Load` they belong
//!   to, so the coordinator can discard events for superseded loads.

pub mod mpv;

use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub use mpv::SpawnError as BackendError;

/// Identifies one `Load`. Monotonic, allocated by the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LoadId(pub u64);

impl std::fmt::Display for LoadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "load#{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    /// The track played to its end.
    Eof,
    /// The track could not be played (or failed mid-way).
    Error(String),
    /// Playback was stopped, replaced, or the player quit.
    Stopped,
}

/// A signed `http`/`https` stream URL. It carries an authentication token,
/// so `Debug` and `Display` are redacted and only the mpv protocol encoder
/// reads the raw value. Built only by the Navidrome client.
#[derive(Clone, PartialEq, Eq)]
pub struct StreamUrl(url::Url);

impl StreamUrl {
    /// Wraps `url` if its scheme is `http` or `https`.
    pub(crate) fn new(url: url::Url) -> Option<Self> {
        matches!(url.scheme(), "http" | "https").then_some(Self(url))
    }

    /// The raw URL, including credentials. Never log or display it.
    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Debug for StreamUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StreamUrl(<redacted>)")
    }
}

impl std::fmt::Display for StreamUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// What the backend should open.
#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackTarget {
    /// A local file; pre-checked to be an existing regular file.
    File(PathBuf),
    /// A remote stream.
    Stream(StreamUrl),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackCommand {
    Load {
        load: LoadId,
        target: PlaybackTarget,
    },
    SetPause(bool),
    Stop,
    SeekRelative(f64),
    SeekAbsolute(f64),
    SetVolume(u8),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackEvent {
    Started {
        load: LoadId,
    },
    TrackEnded {
        load: LoadId,
        reason: EndReason,
    },
    Position(Duration),
    Duration(Option<Duration>),
    Paused(bool),
    Volume(u8),
    /// The backend refused `SetVolume(volume)`; its volume is unchanged by
    /// that request. `reason` is user-presentable.
    VolumeRejected {
        volume: u8,
        reason: String,
    },
    /// A non-fatal backend problem worth showing to the user.
    Error(String),
    /// The backend is gone (crashed, its connection closed, or it stopped
    /// answering requests and was killed). Terminal.
    BackendExited(String),
}

/// Capacity of the event channel. `Position` is coalesced by the backend,
/// so this only fills up if the consumer stalls.
pub const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Reason given when the backend task ends without reporting why.
pub const BACKEND_TASK_ENDED: &str = "backend task ended";

/// How a backend shutdown ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
    /// The player quit on request.
    Clean,
    /// The player did not quit in time and was killed.
    Killed,
    /// The backend had already exited (or its task failed).
    AlreadyExited,
    /// The backend task did not finish within the timeout and was aborted;
    /// the child is killed on drop.
    TimedOut,
}

/// Owner-side handle to a running backend task.
///
/// Commands go over an unbounded channel so callers never await. Dropping
/// the handle closes the command channel, which makes the backend shut
/// down; if the runtime itself goes away, the child process is killed on
/// drop.
pub struct PlaybackHandle {
    commands: mpsc::UnboundedSender<PlaybackCommand>,
    events: mpsc::Receiver<PlaybackEvent>,
    task: Option<JoinHandle<ShutdownOutcome>>,
    pid: Option<u32>,
    exit_reported: bool,
}

impl PlaybackHandle {
    pub(crate) fn new(
        commands: mpsc::UnboundedSender<PlaybackCommand>,
        events: mpsc::Receiver<PlaybackEvent>,
        task: JoinHandle<ShutdownOutcome>,
        pid: Option<u32>,
    ) -> Self {
        Self {
            commands,
            events,
            task: Some(task),
            pid,
            exit_reported: false,
        }
    }

    /// Sends a command without waiting. Returns `false` if the backend has
    /// already stopped.
    pub fn send(&self, command: PlaybackCommand) -> bool {
        self.commands.send(command).is_ok()
    }

    /// Sends commands in order. Returns `false` if the backend has stopped.
    pub fn send_all(&self, commands: impl IntoIterator<Item = PlaybackCommand>) -> bool {
        commands.into_iter().all(|c| self.send(c))
    }

    /// Waits for the next event. If the backend ends without reporting
    /// `BackendExited`, one synthetic `BackendExited` is returned, so the
    /// end is never silent. After that, `None`: stop polling.
    pub async fn recv(&mut self) -> Option<PlaybackEvent> {
        match self.events.recv().await {
            Some(event) => {
                if matches!(event, PlaybackEvent::BackendExited(_)) {
                    self.exit_reported = true;
                }
                Some(event)
            }
            None if !self.exit_reported => {
                self.exit_reported = true;
                Some(PlaybackEvent::BackendExited(BACKEND_TASK_ENDED.into()))
            }
            None => None,
        }
    }

    /// Process id of the backend child, for diagnostics and tests.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Asks the backend to shut down and waits up to `timeout` for it.
    ///
    /// The event receiver is closed first, so a backend blocked delivering
    /// events to a consumer that stopped reading still takes the graceful
    /// path. If the wait times out the task is aborted, which kills the
    /// child on drop.
    pub async fn shutdown(mut self, timeout: Duration) -> ShutdownOutcome {
        self.events.close();
        let _ = self.commands.send(PlaybackCommand::Shutdown);
        let Some(mut task) = self.task.take() else {
            return ShutdownOutcome::AlreadyExited;
        };
        match tokio::time::timeout(timeout, &mut task).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => ShutdownOutcome::AlreadyExited,
            Err(_) => {
                task.abort();
                let _ = task.await;
                ShutdownOutcome::TimedOut
            }
        }
    }
}

/// Starts the default backend (mpv from `PATH`) and connects to it.
pub async fn spawn_backend() -> Result<PlaybackHandle, BackendError> {
    mpv::spawn(mpv::MpvOptions::default()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> (PlaybackHandle, mpsc::Sender<PlaybackEvent>) {
        let (command_tx, _command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::channel(4);
        let task = tokio::spawn(async { ShutdownOutcome::AlreadyExited });
        (
            PlaybackHandle::new(command_tx, event_rx, task, None),
            event_tx,
        )
    }

    #[test]
    fn stream_url_is_redacted_and_http_only() {
        let url = url::Url::parse("https://h.example/rest/stream?id=1&t=tok&s=salt").unwrap();
        let stream = StreamUrl::new(url).unwrap();
        assert_eq!(format!("{stream:?}"), "StreamUrl(<redacted>)");
        assert_eq!(stream.to_string(), "<redacted>");
        let target = PlaybackTarget::Stream(stream.clone());
        let command = PlaybackCommand::Load {
            load: LoadId(1),
            target,
        };
        let debug = format!("{command:?}");
        assert!(!debug.contains("tok") && !debug.contains("salt"), "{debug}");
        assert!(stream.expose().contains("t=tok"));
        for bad in ["file:///etc/passwd", "ftp://h/x", "data:,x"] {
            assert!(
                StreamUrl::new(url::Url::parse(bad).unwrap()).is_none(),
                "{bad}"
            );
        }
    }

    #[tokio::test]
    async fn silent_channel_close_yields_one_synthetic_backend_exited() {
        let (mut handle, events) = handle();
        events.send(PlaybackEvent::Paused(true)).await.unwrap();
        drop(events);
        assert_eq!(handle.recv().await, Some(PlaybackEvent::Paused(true)));
        assert_eq!(
            handle.recv().await,
            Some(PlaybackEvent::BackendExited(BACKEND_TASK_ENDED.into()))
        );
        assert_eq!(handle.recv().await, None);
    }

    #[tokio::test]
    async fn real_backend_exited_is_not_duplicated() {
        let (mut handle, events) = handle();
        events
            .send(PlaybackEvent::BackendExited("mpv exited".into()))
            .await
            .unwrap();
        drop(events);
        assert_eq!(
            handle.recv().await,
            Some(PlaybackEvent::BackendExited("mpv exited".into()))
        );
        assert_eq!(handle.recv().await, None);
    }
}
