//! mpv backend: an async driver task that wires the pure [`state`] machine
//! to mpv's JSON IPC socket and child process. All socket and process I/O
//! for playback lives here and in [`process`].

pub mod process;
pub mod protocol;
pub mod state;

use std::ffi::OsString;
use std::path::Path;
use std::time::Instant;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, oneshot};
use tokio::time::MissedTickBehavior;

pub use process::SpawnError;

use self::process::{MpvProcess, QUIT_GRACE};
use self::state::{MpvState, OutgoingRequest, Output, POSITION_INTERVAL};
use crate::playback::{
    EVENT_CHANNEL_CAPACITY, PlaybackCommand, PlaybackEvent, PlaybackHandle, ShutdownOutcome,
};

#[derive(Debug, Clone)]
pub struct MpvOptions {
    /// Program to execute. Internal: tests point it at a nonexistent
    /// binary. Not user configuration.
    pub program: OsString,
}

impl Default for MpvOptions {
    fn default() -> Self {
        Self {
            program: process::DEFAULT_PROGRAM.into(),
        }
    }
}

/// Spawns mpv, connects to it and starts the driver task.
pub async fn spawn(options: MpvOptions) -> Result<PlaybackHandle, SpawnError> {
    let (process, stream) = MpvProcess::launch(&options.program).await?;
    let pid = process.id();
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
    let task = tokio::spawn(run(process, stream, command_rx, event_tx));
    Ok(PlaybackHandle::new(command_tx, event_rx, task, pid))
}

/// Why the main loop ended.
enum Exit {
    /// Shutdown requested, or the owner went away: quit mpv gracefully.
    Graceful,
    /// mpv or its connection is gone.
    BackendGone(String),
}

/// Requests are written by a separate task so the main loop never awaits
/// socket writes. Otherwise a deadlock is possible: mpv blocks writing
/// replies while the driver is not reading, stops reading requests, and a
/// driver blocked writing a request never gets back to reading.
struct Writer {
    lines: mpsc::UnboundedSender<String>,
    quit: Option<oneshot::Sender<String>>,
    task: tokio::task::JoinHandle<Result<(), String>>,
}

impl Writer {
    fn spawn(write: OwnedWriteHalf) -> Self {
        let (lines, rx) = mpsc::unbounded_channel();
        let (quit, quit_rx) = oneshot::channel();
        let task = tokio::spawn(write_loop(write, rx, quit_rx));
        Self {
            lines,
            quit: Some(quit),
            task,
        }
    }

    fn send(&self, requests: &[OutgoingRequest]) {
        for request in requests {
            tracing::debug!(
                request_id = request.request_id,
                command = %request.command.describe(),
                "mpv <-"
            );
            let line = request.encode();
            tracing::trace!(line = %line.trim_end(), "mpv <- raw");
            // A closed writer is noticed through its task handle.
            let _ = self.lines.send(line);
        }
    }

    /// Writes `quit` ahead of anything still queued.
    fn quit(&mut self, requests: &[OutgoingRequest]) {
        let line: String = requests.iter().map(OutgoingRequest::encode).collect();
        if let Some(quit) = self.quit.take() {
            let _ = quit.send(line);
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn write_loop(
    mut write: OwnedWriteHalf,
    mut lines: mpsc::UnboundedReceiver<String>,
    mut quit: oneshot::Receiver<String>,
) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("could not write to mpv: {e}");
    loop {
        tokio::select! {
            biased;
            line = &mut quit => {
                if let Ok(line) = line {
                    write.write_all(line.as_bytes()).await.map_err(fail)?;
                }
                return Ok(());
            }
            line = lines.recv() => match line {
                Some(line) => write.write_all(line.as_bytes()).await.map_err(fail)?,
                None => return Ok(()),
            },
        }
    }
}

async fn run(
    mut process: MpvProcess,
    stream: UnixStream,
    mut commands: mpsc::UnboundedReceiver<PlaybackCommand>,
    events: mpsc::Sender<PlaybackEvent>,
) -> ShutdownOutcome {
    let (read, write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let mut writer = Writer::spawn(write);
    let mut state = MpvState::new();
    let mut tick = tokio::time::interval(POSITION_INTERVAL);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let mut pending = state.startup();
    let exit = loop {
        writer.send(&pending.requests);
        if !deliver(&events, std::mem::take(&mut pending.events)).await {
            break Exit::Graceful;
        }
        pending = tokio::select! {
            command = commands.recv() => match command {
                None | Some(PlaybackCommand::Shutdown) => break Exit::Graceful,
                Some(PlaybackCommand::Load { load, path }) => {
                    // Contract: only existing regular files reach mpv.
                    match precheck(&path).await {
                        Ok(()) => state.handle_command(PlaybackCommand::Load { load, path }),
                        Err(reason) => state.load_rejected(load, &reason),
                    }
                }
                Some(command) => state.handle_command(command),
            },
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    tracing::trace!(%line, "mpv ->");
                    match state.handle_line(&line, Instant::now()) {
                        Ok(output) => output,
                        Err(error) => {
                            tracing::warn!(%error, "skipping malformed line from mpv");
                            Output::default()
                        }
                    }
                }
                Ok(None) => break Exit::BackendGone("mpv closed its IPC connection".into()),
                Err(error) => break Exit::BackendGone(format!("lost connection to mpv: {error}")),
            },
            written = &mut writer.task => {
                let reason = match written {
                    Ok(Err(reason)) => reason,
                    _ => "mpv request writer stopped".to_owned(),
                };
                break Exit::BackendGone(reason);
            }
            status = process.wait() => {
                let reason = match status {
                    Ok(status) => format!("mpv exited unexpectedly ({status})"),
                    Err(error) => format!("lost track of the mpv process: {error}"),
                };
                break Exit::BackendGone(reason);
            }
            _ = tick.tick() => state.tick(Instant::now()),
        };
    };

    match exit {
        Exit::Graceful => {
            tracing::debug!("shutting down mpv");
            let quit = state.handle_command(PlaybackCommand::Shutdown);
            writer.quit(&quit.requests);
            let clean = wait_for_exit(&mut process, &mut lines).await;
            if !clean {
                tracing::warn!("mpv did not quit in time; killing it");
            }
            // Kills only if still running; always reaps and removes the
            // socket dir.
            process.kill().await;
            if clean {
                ShutdownOutcome::Clean
            } else {
                ShutdownOutcome::Killed
            }
        }
        Exit::BackendGone(reason) => {
            tracing::error!(%reason, "mpv backend exited");
            process.kill().await;
            let _ = events.send(PlaybackEvent::BackendExited(reason)).await;
            ShutdownOutcome::AlreadyExited
        }
    }
}

/// Waits up to [`QUIT_GRACE`] for mpv to exit after `quit`, draining its
/// output meanwhile so mpv is never blocked writing and can read `quit`.
async fn wait_for_exit(
    process: &mut MpvProcess,
    lines: &mut Lines<BufReader<OwnedReadHalf>>,
) -> bool {
    let mut eof = false;
    tokio::time::timeout(QUIT_GRACE, async {
        loop {
            tokio::select! {
                _ = process.wait() => break,
                line = lines.next_line(), if !eof => {
                    if !matches!(line, Ok(Some(_))) {
                        eof = true;
                    }
                }
            }
        }
    })
    .await
    .is_ok()
}

/// Checks a load target before it reaches mpv: it must be an existing
/// regular file (symlinks are followed). The error never includes the path.
pub async fn precheck(path: &Path) -> Result<(), String> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(state::NOT_A_REGULAR_FILE.to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(state::FILE_NOT_FOUND.to_owned())
        }
        Err(error) => Err(format!("cannot read file: {error}")),
    }
}

/// Delivers events in order. Returns `false` if the receiver is gone.
async fn deliver(events: &mpsc::Sender<PlaybackEvent>, batch: Vec<PlaybackEvent>) -> bool {
    for event in batch {
        if events.send(event).await.is_err() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn precheck_requires_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.wav");
        std::fs::write(&file, b"x").unwrap();
        let link = dir.path().join("link.wav");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert_eq!(precheck(&file).await, Ok(()));
        assert_eq!(precheck(&link).await, Ok(()));
        assert_eq!(
            precheck(dir.path()).await,
            Err(state::NOT_A_REGULAR_FILE.to_owned())
        );
        assert_eq!(
            precheck(&dir.path().join("missing.wav")).await,
            Err(state::FILE_NOT_FOUND.to_owned())
        );
        let dangling = dir.path().join("dangling.wav");
        std::os::unix::fs::symlink(dir.path().join("nope"), &dangling).unwrap();
        assert_eq!(
            precheck(&dangling).await,
            Err(state::FILE_NOT_FOUND.to_owned())
        );
    }

    async fn read_all(mut peer: UnixStream) -> String {
        use tokio::io::AsyncReadExt;
        let mut out = String::new();
        peer.read_to_string(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn writer_writes_requests_in_fifo_order() {
        let (ours, peer) = UnixStream::pair().unwrap();
        let (_read, write) = ours.into_split();
        let (lines, rx) = mpsc::unbounded_channel();
        let (_quit, quit_rx) = oneshot::channel();
        for line in ["one\n", "two\n", "three\n"] {
            lines.send(line.to_owned()).unwrap();
        }
        drop(lines);
        write_loop(write, rx, quit_rx).await.unwrap();
        assert_eq!(read_all(peer).await, "one\ntwo\nthree\n");
    }

    #[tokio::test]
    async fn writer_writes_quit_before_unwritten_lines_without_splitting() {
        let (ours, peer) = UnixStream::pair().unwrap();
        let (_read, write) = ours.into_split();
        let (lines, rx) = mpsc::unbounded_channel();
        let (quit, quit_rx) = oneshot::channel();
        for line in ["one\n", "two\n"] {
            lines.send(line.to_owned()).unwrap();
        }
        quit.send("{\"command\":[\"quit\"]}\n".to_owned()).unwrap();
        // Both are ready before the task first runs; quit has priority and
        // the queued lines are not written.
        write_loop(write, rx, quit_rx).await.unwrap();
        let out = read_all(peer).await;
        assert_eq!(out, "{\"command\":[\"quit\"]}\n");
        drop(lines);
    }
}
