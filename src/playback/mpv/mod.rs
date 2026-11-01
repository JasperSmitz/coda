//! mpv backend: an async driver task that wires the pure [`state`] machine
//! to mpv's JSON IPC socket and child process. All socket and process I/O
//! for playback lives here and in [`process`].

pub mod process;
pub mod protocol;
pub mod state;
pub mod visualizer;

use std::ffi::OsString;
use std::path::Path;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, oneshot};
use tokio::time::MissedTickBehavior;

pub use process::SpawnError;

use self::process::{MpvProcess, QUIT_GRACE};
use self::state::{MpvState, OutgoingRequest, Output, POSITION_INTERVAL, STOPPED_RESPONDING};
use crate::playback::{
    EVENT_CHANNEL_CAPACITY, PlaybackCommand, PlaybackEvent, PlaybackHandle, PlaybackTarget,
    ShutdownOutcome,
};

/// Internal backend parameters, public only for Coda's own tests. None of
/// them is user configuration; the app always uses the default.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct MpvOptions {
    /// Program to execute. Tests point it at a nonexistent binary.
    pub program: OsString,
    /// How long a request may go without a reply before mpv is considered
    /// hung and killed. Tests shorten it.
    pub reply_timeout: Duration,
    /// Run mpv with `--ao=null`: playback is timed by mpv's clock instead
    /// of an audio device. Tests whose timing must not depend on the
    /// device's buffering use it.
    pub null_audio: bool,
    /// With `null_audio`, `--ao-null-buffer=<seconds>`: how much audio the
    /// null output takes up front, like a real device's buffer.
    pub null_audio_buffer: Option<u32>,
    /// The visualizer adds this graph instead of [`visualizer::GRAPH`].
    /// Tests use a broken one.
    pub visualizer_graph: Option<String>,
    /// More mpv arguments, after the others (tests render to a file with
    /// `--ao=pcm`).
    pub extra_args: Vec<String>,
}

impl Default for MpvOptions {
    fn default() -> Self {
        Self {
            program: process::DEFAULT_PROGRAM.into(),
            reply_timeout: state::REPLY_TIMEOUT,
            null_audio: false,
            null_audio_buffer: None,
            visualizer_graph: None,
            extra_args: Vec::new(),
        }
    }
}

/// The mpv arguments `options` add to [`process::mpv_args`].
fn extra_args(options: &MpvOptions) -> Vec<String> {
    let mut args = Vec::new();
    if options.null_audio {
        args.push("--ao=null".to_owned());
        if let Some(seconds) = options.null_audio_buffer {
            args.push(format!("--ao-null-buffer={seconds}"));
        }
    }
    args.extend(options.extra_args.iter().cloned());
    args
}

/// Spawns mpv, connects to it and starts the driver task.
pub async fn spawn(options: MpvOptions) -> Result<PlaybackHandle, SpawnError> {
    let extra_args = extra_args(&options);
    let extra_args: Vec<&str> = extra_args.iter().map(String::as_str).collect();
    let (process, stream) = MpvProcess::launch(&options.program, &extra_args).await?;
    let pid = process.id();
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
    let mut state = MpvState::with_reply_timeout(options.reply_timeout);
    if let Some(graph) = options.visualizer_graph {
        state = state.with_visualizer_graph(graph);
    }
    let task = tokio::spawn(run(process, stream, state, command_rx, event_tx));
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
            if request.command.is_frequent() {
                tracing::trace!(
                    request_id = request.request_id,
                    command = %request.command.describe(),
                    "mpv <-"
                );
            } else {
                tracing::debug!(
                    request_id = request.request_id,
                    command = %request.command.describe(),
                    "mpv <-"
                );
            }
            let line = request.encode();
            if !request.command.is_sensitive() {
                tracing::trace!(line = %line.trim_end(), "mpv <- raw");
            }
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

/// The driver loop. Every iteration first checks the reply watchdog; the
/// tick guarantees an iteration at least every [`POSITION_INTERVAL`] while
/// the loop waits, which is the cadence the watchdog relies on to tell
/// listening time from time spent blocked elsewhere. While the visualizer
/// is active, its deadline wakes the loop as well (at most about 30 times
/// a second); otherwise it costs nothing.
async fn run(
    mut process: MpvProcess,
    stream: UnixStream,
    mut state: MpvState,
    mut commands: mpsc::UnboundedReceiver<PlaybackCommand>,
    events: mpsc::Sender<PlaybackEvent>,
) -> ShutdownOutcome {
    let (read, write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let mut writer = Writer::spawn(write);
    let mut tick = tokio::time::interval(POSITION_INTERVAL);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let mut pending = state.startup();
    let exit = loop {
        let now = Instant::now();
        if let Some((request_id, waited)) = state.overdue_reply(now) {
            tracing::error!(request_id, ?waited, "mpv did not reply in time");
            break Exit::BackendGone(STOPPED_RESPONDING.to_owned());
        }
        state.requests_sent(&pending.requests, now);
        writer.send(&pending.requests);
        if !deliver(&events, std::mem::take(&mut pending.events)).await {
            break Exit::Graceful;
        }
        let visualizer = state.visualizer_deadline(Instant::now());
        // Not polled when `None`; the value is only a placeholder then.
        let visualizer_due =
            tokio::time::Instant::from_std(visualizer.unwrap_or_else(Instant::now));
        pending = tokio::select! {
            command = commands.recv() => match command {
                None | Some(PlaybackCommand::Shutdown) => break Exit::Graceful,
                Some(PlaybackCommand::Load { load, target: PlaybackTarget::File(path) }) => {
                    // Contract: only existing regular files reach mpv.
                    // Streams are signed http(s) URLs and are not checked.
                    match precheck(&path).await {
                        Ok(()) => state.handle_command(PlaybackCommand::Load {
                            load,
                            target: PlaybackTarget::File(path),
                        }),
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
            _ = tokio::time::sleep_until(visualizer_due), if visualizer.is_some() => {
                state.visualizer_tick(Instant::now())
            }
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

    #[test]
    fn the_visualizer_graph_is_never_a_spawn_argument() {
        let options = [
            MpvOptions::default(),
            MpvOptions {
                null_audio: true,
                null_audio_buffer: Some(3),
                visualizer_graph: Some("@viz:lavfi=[anull]".into()),
                ..MpvOptions::default()
            },
        ];
        for options in options {
            let extra = extra_args(&options);
            let extra: Vec<&str> = extra.iter().map(String::as_str).collect();
            let args = process::mpv_args(Path::new("/s/mpv.sock"), &extra);
            let args: Vec<&str> = args.iter().map(|a| a.to_str().unwrap()).collect();
            assert!(args.contains(&"--audio-channels=stereo"), "{args:?}");
            for arg in &args {
                assert!(!arg.starts_with("--af"), "{arg}");
                assert!(!arg.contains("lavfi") && !arg.contains("@viz"), "{arg}");
                assert!(!arg.contains(visualizer::GRAPH), "{arg}");
            }
        }
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

    /// `true` if a process with this pid exists (zombies included).
    fn pid_exists(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run kill")
            .success()
    }

    struct FakeMpv {
        commands: mpsc::UnboundedSender<PlaybackCommand>,
        events: mpsc::Receiver<PlaybackEvent>,
        task: tokio::task::JoinHandle<ShutdownOutcome>,
        pid: u32,
    }

    /// Runs the driver against a stand-in for mpv: a `sleep` child as the
    /// process, and `peer` (returned to the test) as mpv's end of the IPC
    /// connection.
    fn fake_mpv(reply_timeout: Duration) -> (FakeMpv, UnixStream) {
        let child = tokio::process::Command::new("sleep")
            .arg("60")
            .kill_on_drop(true)
            .spawn()
            .expect("spawn sleep");
        let pid = child.id().unwrap();
        let (ours, peer) = UnixStream::pair().unwrap();
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let task = tokio::spawn(run(
            MpvProcess::from_child(child),
            ours,
            MpvState::with_reply_timeout(reply_timeout),
            command_rx,
            event_tx,
        ));
        let fake = FakeMpv {
            commands,
            events,
            task,
            pid,
        };
        (fake, peer)
    }

    #[tokio::test]
    async fn mpv_that_never_replies_is_reported_and_killed() {
        let timeout = Duration::from_millis(400);
        let started = Instant::now();
        // The peer stays open and never answers (or reads) anything.
        let (mut fake, _peer) = fake_mpv(timeout);
        assert!(pid_exists(fake.pid));
        let event = tokio::time::timeout(Duration::from_secs(5), fake.events.recv())
            .await
            .expect("reported in time");
        let elapsed = started.elapsed();
        assert_eq!(
            event,
            Some(PlaybackEvent::BackendExited(STOPPED_RESPONDING.into()))
        );
        assert!(elapsed >= timeout, "{elapsed:?}");
        // Generous: a loaded CI machine may schedule the checks late.
        assert!(elapsed < timeout + Duration::from_secs(2), "{elapsed:?}");
        assert_eq!(fake.task.await.unwrap(), ShutdownOutcome::AlreadyExited);
        // Killed and reaped.
        assert!(!pid_exists(fake.pid), "stand-in mpv still exists");
        assert!(fake.commands.send(PlaybackCommand::Stop).is_err());
    }

    #[tokio::test]
    async fn mpv_that_stops_replying_later_is_reported() {
        // Startup is answered; then one command goes unanswered.
        let timeout = Duration::from_millis(400);
        let (mut fake, peer) = fake_mpv(timeout);
        let (read, mut write) = peer.into_split();
        let mut lines = BufReader::new(read).lines();
        for _ in state::OBSERVED_PROPERTIES {
            let line = lines.next_line().await.unwrap().unwrap();
            let id = serde_json::from_str::<serde_json::Value>(&line).unwrap()["request_id"]
                .as_u64()
                .unwrap();
            let reply = format!("{{\"request_id\":{id},\"error\":\"success\"}}\n");
            write.write_all(reply.as_bytes()).await.unwrap();
        }
        // Answered startup does not trip the watchdog.
        let quiet = tokio::time::timeout(timeout * 3, fake.events.recv()).await;
        assert!(quiet.is_err(), "unexpected {quiet:?}");
        let sent = Instant::now();
        fake.commands.send(PlaybackCommand::SetPause(true)).unwrap();
        let event = tokio::time::timeout(Duration::from_secs(5), fake.events.recv())
            .await
            .expect("reported in time");
        assert_eq!(
            event,
            Some(PlaybackEvent::BackendExited(STOPPED_RESPONDING.into()))
        );
        assert!(sent.elapsed() >= timeout);
        assert_eq!(fake.task.await.unwrap(), ShutdownOutcome::AlreadyExited);
        assert!(!pid_exists(fake.pid));
    }

    #[tokio::test]
    async fn responsive_mpv_with_a_stalled_consumer_is_not_killed() {
        // A fake mpv answers every request and also floods property
        // changes. Nobody reads the events for well past the timeout, so
        // the driver blocks delivering them and stops reading from mpv;
        // that time must not count as mpv failing to reply.
        let timeout = Duration::from_millis(400);
        let (mut fake, peer) = fake_mpv(timeout);
        let responder = tokio::spawn(async move {
            let (read, mut write) = peer.into_split();
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let id = serde_json::from_str::<serde_json::Value>(&line).unwrap()["request_id"]
                    .as_u64()
                    .unwrap();
                let mut out = format!("{{\"request_id\":{id},\"error\":\"success\"}}\n");
                for _ in 0..4 {
                    out.push_str(
                        "{\"event\":\"property-change\",\"id\":3,\"name\":\"pause\",\"data\":true}\n",
                    );
                }
                if write.write_all(out.as_bytes()).await.is_err() {
                    break;
                }
            }
        });
        for i in 0..EVENT_CHANNEL_CAPACITY {
            fake.commands
                .send(PlaybackCommand::SetPause(i % 2 == 0))
                .unwrap();
        }
        tokio::time::sleep(timeout * 4).await;
        // Drain everything: no BackendExited among it.
        fake.commands
            .send(PlaybackCommand::SetPause(false))
            .unwrap();
        let mut drained = 0;
        while let Ok(Some(event)) = tokio::time::timeout(timeout * 2, fake.events.recv()).await {
            assert!(
                !matches!(event, PlaybackEvent::BackendExited(_)),
                "{event:?} after {drained} events"
            );
            drained += 1;
        }
        assert!(drained > EVENT_CHANNEL_CAPACITY, "{drained}");
        assert!(pid_exists(fake.pid));
        drop(fake.events);
        drop(fake.commands);
        // Graceful path; `sleep` ignores `quit`, so it is killed.
        assert_eq!(fake.task.await.unwrap(), ShutdownOutcome::Killed);
        responder.abort();
    }
}
