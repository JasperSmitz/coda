//! Cover art on a real pseudo-terminal: the startup query never leaks a
//! reply into the key events, and every exit path deletes the Kitty image
//! and restores the terminal.
//!
//! The terminal side is scripted here (the "terminal" answers, or not, on
//! the pty's master). Code that must own a terminal runs in a child: this
//! test binary run again with `CODA_PTY_CHILD` set ([`pty_child`]), or the
//! real `coda` binary (those tests need mpv, so they are ignored by
//! default; `cargo test -- --ignored` runs them).

use std::fs;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Ghostty's replies to Coda's query.
const GHOSTTY: &[u8] = b"\x1b_Gi=31;OK\x1b\\\x1b[6;34;16t\x1b[?62;22;52c";
/// The start of Coda's query.
const QUERY: &[u8] = b"\x1b_Gi=31,";
/// The child is in its key loop.
const READY: &[u8] = b"\x1b]9999;READY\x07";

// ----- a pty --------------------------------------------------------------

/// A child on a new pty, with everything it writes collected.
struct Pty {
    master: Arc<fs::File>,
    output: Arc<Mutex<Vec<u8>>>,
    child: Child,
}

impl Pty {
    fn spawn(mut command: Command) -> Pty {
        let mut master = -1;
        let mut slave = -1;
        let size = libc::winsize {
            ws_row: 30,
            ws_col: 100,
            ws_xpixel: 1600,
            ws_ypixel: 1020,
        };
        // SAFETY: valid out-pointers and window size; no name buffer.
        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &size as *const libc::winsize as *mut libc::winsize,
            )
        };
        assert_eq!(result, 0, "openpty: {}", io::Error::last_os_error());
        // No child may inherit either end (tests run in parallel): one
        // holding a master keeps its own terminal from ever hanging up.
        for fd in [master, slave] {
            // SAFETY: `fcntl` on a descriptor just opened here.
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
                0
            );
        }
        // SAFETY: both were just opened and are owned here alone.
        let (master, slave) =
            unsafe { (fs::File::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        command
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                #[allow(clippy::cast_lossless, clippy::useless_conversion)]
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        let master = Arc::new(master);
        let output = Arc::new(Mutex::new(Vec::new()));
        let (reader, sink) = (master.clone(), output.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 65536];
            loop {
                // SAFETY: `buf` is valid for its length.
                let n =
                    unsafe { libc::read(reader.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                if n <= 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&buf[..n as usize]);
            }
        });
        Pty {
            master,
            output,
            child,
        }
    }

    fn write(&self, bytes: &[u8]) {
        (&*self.master).write_all(bytes).unwrap();
    }

    fn output(&self) -> Vec<u8> {
        self.output.lock().unwrap().clone()
    }

    /// Waits until the output contains `needle`; its end offset.
    fn wait_for(&self, needle: &[u8], timeout: Duration) -> usize {
        let end = Instant::now() + timeout;
        loop {
            if let Some(at) = find(&self.output(), needle) {
                return at + needle.len();
            }
            assert!(
                Instant::now() < end,
                "timed out waiting for {:?}; got {:?}",
                String::from_utf8_lossy(needle),
                String::from_utf8_lossy(&self.output())
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Waits for the child to exit; its exit code (`None`: by a signal).
    fn wait(&mut self, timeout: Duration) -> Option<i32> {
        let end = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                // Let the reader catch up with the last bytes.
                std::thread::sleep(Duration::from_millis(100));
                return status.code();
            }
            if Instant::now() > end {
                let _ = self.child.kill();
                panic!(
                    "the child did not exit; got {:?}",
                    String::from_utf8_lossy(&self.output())
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn signal(&self, signal: libc::c_int) {
        // SAFETY: a plain kill(2) of our own child.
        assert_eq!(
            unsafe { libc::kill(self.child.id() as libc::pid_t, signal) },
            0
        );
    }
}

impl Drop for Pty {
    /// A test that failed early must not leave its child running: it is
    /// asked to quit (its output is still read meanwhile), then killed.
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        // SAFETY: a plain kill(2) of our own child.
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

// ----- the in-process child ---------------------------------------------

/// This test binary as a pty child running `mode`.
fn child(mode: &str, term: &str, result: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "pty_child", "--nocapture", "--test-threads=1"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("TERM", term)
        .env("CODA_PTY_CHILD", mode)
        .env("CODA_PTY_RESULT", result);
    // To check that the late-reply tests can fail: run them with this set.
    if let Some(off) = std::env::var_os("CODA_PTY_NO_FILTER") {
        command.env("CODA_PTY_NO_FILTER", off);
    }
    command
}

/// Runs a child mode when this binary was started by [`child`]; a no-op
/// in a normal test run.
#[test]
fn pty_child() {
    let Ok(mode) = std::env::var("CODA_PTY_CHILD") else {
        return;
    };
    let result = PathBuf::from(std::env::var_os("CODA_PTY_RESULT").unwrap());
    match mode.as_str() {
        "keys" => keys_child(&result),
        "panic" => panic_child(),
        other => panic!("unknown child mode {other}"),
    }
}

/// As the app starts: terminal set up, detection (and the clear), then a
/// key reader with the late-reply filter (unless `CODA_PTY_NO_FILTER`),
/// recording keys until `y` or 4 s.
fn keys_child(result: &Path) {
    use coda_tui::ui::cover::LateReplyFilter;
    use crossterm::event::{self, Event, KeyCode};
    let previous = std::panic::take_hook();
    let mut terminal = ratatui::try_init().unwrap();
    coda_tui::app::install_panic_hook(previous);
    let detection = coda_tui::app::detect_graphics(&mut terminal);
    let mut filter = LateReplyFilter::new(detection.late, Instant::now());
    let mut stdout = io::stdout();
    stdout.write_all(READY).unwrap();
    stdout.flush().unwrap();
    let mut keys = Vec::new();
    let end = Instant::now() + Duration::from_secs(4);
    while Instant::now() < end {
        if !event::poll(Duration::from_millis(20)).unwrap() {
            continue;
        }
        if let Event::Key(key) = event::read().unwrap() {
            if std::env::var_os("CODA_PTY_NO_FILTER").is_none()
                && filter.swallow(&key, Instant::now())
            {
                continue;
            }
            keys.push(format!(
                "{:?}{}",
                key.code,
                if key.modifiers.is_empty() { "" } else { "+mod" }
            ));
            if key.code == KeyCode::Char('y') {
                break;
            }
        }
    }
    drop(terminal);
    ratatui::restore();
    fs::write(
        result,
        format!(
            "graphics={:?}\nkeys={}\n",
            detection.graphics,
            keys.join(" ")
        ),
    )
    .unwrap();
}

/// Shows a Kitty image as the app does, then panics on the UI thread.
fn panic_child() {
    use coda_tui::cover::CoverImage;
    use coda_tui::ui::cover::{ArtFrame, ArtPainter, Graphics};
    let previous = std::panic::take_hook();
    let _terminal = ratatui::try_init().unwrap();
    coda_tui::app::install_panic_hook(previous);
    let mut painter = ArtPainter::new(Graphics::Kitty { cell: (16, 34) }, 0x0B_0000);
    let image = CoverImage::new(4, 4, vec![200; 48]).unwrap();
    painter.prepare(Some(ArtFrame {
        picture: (image.fingerprint(), false),
        image: Arc::new(image),
        cols: 9,
        rows: 4,
    }));
    let mut stdout = io::stdout();
    stdout.write_all(&painter.take_output()).unwrap();
    stdout.write_all(READY).unwrap();
    stdout.flush().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    panic!("simulated panic on the UI thread");
}

#[derive(Debug, Clone, Copy)]
enum Reply {
    /// Ghostty's replies at once.
    Prompt,
    /// After 400 ms, within the deadline.
    Slow,
    /// Never.
    Absent,
    /// After 1.5 s: past the deadline, once the key reader runs.
    Late,
    /// Half the Kitty reply before the deadline, the rest only after
    /// detection gave up (past the cap for a started reply).
    Split,
    /// The Kitty reply and part of DA1 at once, DA1's tail after 1.5 s:
    /// past the first deadline, within the cap for a started reply.
    Da1Split,
    /// As `Da1Split`, but DA1's tail only after detection gave up.
    Da1PastCap,
    /// Nothing in time; then the reply with its `ESC` alone in one read.
    EscSplit,
    /// Nothing at all; the user types `Alt-_` and then `jk`.
    UserAltUnderscore,
}

fn run_keys(term: &str, reply: Reply) -> (String, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("result");
    let mut pty = Pty::spawn(child("keys", term, &result));
    let queried = term == "xterm-kitty";
    if queried {
        pty.wait_for(QUERY, Duration::from_secs(10));
        match reply {
            Reply::Prompt => pty.write(GHOSTTY),
            Reply::Slow => {
                std::thread::sleep(Duration::from_millis(400));
                pty.write(GHOSTTY);
            }
            Reply::Absent => {}
            Reply::Late => {
                pty.wait_for(READY, Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(300));
                pty.write(GHOSTTY);
            }
            Reply::Split => {
                pty.write(&GHOSTTY[..7]);
                pty.wait_for(READY, Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(300));
                pty.write(&GHOSTTY[7..]);
            }
            Reply::Da1Split => {
                let cut = find(GHOSTTY, b"\x1b[?").unwrap() + 5;
                pty.write(&GHOSTTY[..cut]);
                std::thread::sleep(Duration::from_millis(1500));
                pty.write(&GHOSTTY[cut..]);
            }
            Reply::Da1PastCap => {
                let cut = find(GHOSTTY, b"\x1b[?").unwrap() + 5;
                pty.write(&GHOSTTY[..cut]);
                pty.wait_for(READY, Duration::from_secs(10));
                std::thread::sleep(Duration::from_millis(300));
                pty.write(&GHOSTTY[cut..]);
            }
            Reply::EscSplit => {
                pty.wait_for(READY, Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(300));
                pty.write(&GHOSTTY[..1]);
                std::thread::sleep(Duration::from_millis(200));
                pty.write(&GHOSTTY[1..]);
            }
            Reply::UserAltUnderscore => {
                pty.wait_for(READY, Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(300));
                pty.write(b"\x1b_");
                std::thread::sleep(Duration::from_millis(100));
                pty.write(b"j");
                std::thread::sleep(Duration::from_millis(100));
                pty.write(b"k");
            }
        }
    }
    pty.wait_for(READY, Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(300));
    pty.write(b"x");
    std::thread::sleep(Duration::from_millis(100));
    pty.write(b"y");
    assert_eq!(pty.wait(Duration::from_secs(10)), Some(0));
    (fs::read_to_string(&result).unwrap(), pty.output())
}

#[test]
fn a_prompt_reply_means_kitty_and_no_stray_keys() {
    let (result, output) = run_keys("xterm-kitty", Reply::Prompt);
    assert!(
        result.contains("graphics=Kitty { cell: (16, 34) }"),
        "{result}"
    );
    assert!(result.contains("keys=Char('x') Char('y')\n"), "{result}");
    // The query was cleared off the screen: a full clear follows it.
    let query = find(&output, QUERY).unwrap();
    assert!(find(&output[query..], b"\x1b[2J").is_some());
}

#[test]
fn a_slow_reply_within_the_deadline_is_waited_for() {
    let (result, _) = run_keys("xterm-kitty", Reply::Slow);
    assert!(result.contains("graphics=Kitty"), "{result}");
    assert!(result.contains("keys=Char('x') Char('y')\n"), "{result}");
}

#[test]
fn no_reply_means_half_blocks_after_the_deadline() {
    let started = Instant::now();
    let (result, _) = run_keys("xterm-kitty", Reply::Absent);
    assert!(result.contains("graphics=HalfBlocks"), "{result}");
    assert!(result.contains("keys=Char('x') Char('y')\n"), "{result}");
    assert!(started.elapsed() > Duration::from_secs(1));
}

#[test]
fn a_late_reply_never_reaches_the_keys() {
    let (result, _) = run_keys("xterm-kitty", Reply::Late);
    assert!(result.contains("graphics=HalfBlocks"), "{result}");
    assert!(result.contains("keys=Char('x') Char('y')\n"), "{result}");
}

#[test]
fn a_reply_cut_by_the_deadline_never_reaches_the_keys() {
    let (result, _) = run_keys("xterm-kitty", Reply::Split);
    assert!(result.contains("graphics=HalfBlocks"), "{result}");
    assert!(result.contains("keys=Char('x') Char('y')\n"), "{result}");
}

#[test]
fn a_deadline_inside_da1_waits_for_the_rest() {
    let started = Instant::now();
    let (result, _) = run_keys("xterm-kitty", Reply::Da1Split);
    assert!(result.contains("graphics=Kitty"), "{result}");
    assert!(result.contains("keys=Char('x') Char('y')\n"), "{result}");
    assert!(started.elapsed() > Duration::from_millis(1500));
}

#[test]
fn the_tail_of_da1_after_detection_gave_up_never_reaches_the_keys() {
    let (result, _) = run_keys("xterm-kitty", Reply::Da1PastCap);
    // The Kitty answer itself was complete.
    assert!(result.contains("graphics=Kitty"), "{result}");
    assert!(result.contains("keys=Char('x') Char('y')\n"), "{result}");
}

#[test]
fn a_late_reply_whose_escape_came_alone_never_reaches_the_keys() {
    let (result, _) = run_keys("xterm-kitty", Reply::EscSplit);
    assert!(result.contains("graphics=HalfBlocks"), "{result}");
    // Only the lone `ESC` gets through, as `Esc`: nothing can tell it
    // from the key until the next byte.
    assert!(
        result.contains("keys=Esc Char('x') Char('y')\n"),
        "{result}"
    );
}

#[test]
fn a_users_alt_underscore_is_not_followed_by_lost_keys() {
    let (result, _) = run_keys("xterm-kitty", Reply::UserAltUnderscore);
    assert!(result.contains("graphics=HalfBlocks"), "{result}");
    assert!(
        result.contains("keys=Char('j') Char('k') Char('x') Char('y')\n"),
        "{result}"
    );
}

#[test]
fn other_terminals_are_not_queried() {
    for term in ["xterm-256color", "alacritty", "tmux-256color"] {
        let (result, output) = run_keys(term, Reply::Prompt);
        assert!(result.contains("graphics=HalfBlocks"), "{term}: {result}");
        assert!(find(&output, QUERY).is_none(), "{term}");
        assert!(result.contains("keys=Char('x') Char('y')\n"), "{result}");
    }
}

/// The Kitty image id in the first transmission of `output`.
fn transmitted_id(output: &[u8]) -> Option<String> {
    let start = find(output, b"\x1b_Ga=T,")?;
    let text =
        String::from_utf8_lossy(&output[start..(start + 200).min(output.len())]).into_owned();
    let id = text.split(",i=").nth(1)?.split(',').next()?;
    Some(id.to_owned())
}

/// Asserts the image was deleted after it was sent, and the alternate
/// screen left after that.
fn assert_cleaned_up(output: &[u8]) {
    let id = transmitted_id(output).expect("an image was sent");
    let sent = find(output, b"\x1b_Ga=T,").unwrap();
    let delete = format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\");
    let deleted = rfind(output, delete.as_bytes())
        .unwrap_or_else(|| panic!("no delete of {id}: {:?}", String::from_utf8_lossy(output)));
    assert!(deleted > sent);
    let left = rfind(output, b"\x1b[?1049l").expect("left the alternate screen");
    assert!(left > deleted, "the delete comes before leaving");
}

#[test]
fn a_panic_deletes_the_image_and_restores_the_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let mut pty = Pty::spawn(child("panic", "xterm-kitty", &dir.path().join("unused")));
    pty.wait_for(READY, Duration::from_secs(10));
    let code = pty.wait(Duration::from_secs(10));
    assert_ne!(code, Some(0), "the test child failed by panicking");
    let output = pty.output();
    assert_cleaned_up(&output);
    assert!(find(&output, b"\x1b[?25h").is_some(), "the cursor is shown");
}

// ----- the real binary (needs mpv) -----------------------------------------

struct Session {
    dir: tempfile::TempDir,
}

impl Session {
    /// A music folder with one long WAV and a folder cover, restored as
    /// the current entry (not playing), and a config with `extra`.
    fn new(extra: &str) -> Session {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let album = root.join("music").join("Album");
        fs::create_dir_all(&album).unwrap();
        let song = album.join("song.wav");
        write_wav(&song, 120);
        fs::write(album.join("Cover.PNG"), png(64)).unwrap();
        let state = root.join("state").join("coda");
        fs::create_dir_all(&state).unwrap();
        let session = serde_json::json!({
            "version": 1, "volume": 50, "repeat": "off", "current": 0,
            "queue": [{ "source": { "local": song }, "title": "Song" }]
        });
        fs::write(state.join("state.json"), session.to_string()).unwrap();
        let config = root.join("config.toml");
        let music = root.join("music").canonicalize().unwrap();
        fs::write(
            &config,
            format!("music_dir = {:?}\n{extra}", music.to_str().unwrap()),
        )
        .unwrap();
        Session { dir }
    }

    fn command(&self, term: &str) -> Command {
        let root = self.dir.path();
        let mut command = Command::new(env!("CARGO_BIN_EXE_coda"));
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("TERM", term)
            .env("HOME", root.join("home"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("CODA_CONFIG", root.join("config.toml"))
            .env("CODA_LOG", "debug");
        command
    }

    fn log(&self) -> String {
        fs::read_to_string(self.dir.path().join("state/coda/coda.log")).unwrap_or_default()
    }
}

fn write_wav(path: &Path, seconds: u32) {
    let data = vec![0u8; 8000 * 2 * seconds as usize];
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((36 + data.len() as u32).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(8000u32.to_le_bytes());
    bytes.extend(16000u32.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend((data.len() as u32).to_le_bytes());
    bytes.extend(&data);
    fs::write(path, bytes).unwrap();
}

fn png(size: u32) -> Vec<u8> {
    use image::ImageEncoder;
    let pixels = image::RgbImage::from_fn(size, size, |x, y| {
        image::Rgb([x as u8 * 4, y as u8 * 4, 90])
    });
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&pixels, size, size, image::ExtendedColorType::Rgb8)
        .unwrap();
    out
}

/// Starts coda as Kitty (answering its query) and waits for the cover.
fn start_kitty(session: &Session) -> Pty {
    let pty = Pty::spawn(session.command("xterm-kitty"));
    pty.wait_for(QUERY, Duration::from_secs(10));
    pty.write(GHOSTTY);
    pty.wait_for(b"\x1b_Ga=T,", Duration::from_secs(10));
    // The frame with the placeholders follows.
    pty.wait_for("\u{10EEEE}".as_bytes(), Duration::from_secs(5));
    pty
}

#[test]
#[ignore = "needs mpv"]
fn quitting_with_q_deletes_the_image() {
    let session = Session::new("");
    let mut pty = start_kitty(&session);
    pty.write(b"q");
    assert_eq!(pty.wait(Duration::from_secs(10)), Some(0));
    assert_cleaned_up(&pty.output());
    let log = session.log();
    assert!(log.contains("cover art: kitty graphics"), "{log}");
    // No path, file name or title in the log.
    for secret in ["Album", "song.wav", "Cover.PNG", "Song"] {
        assert!(!log.contains(secret), "{secret} in {log}");
    }
}

#[test]
#[ignore = "needs mpv"]
fn sigterm_deletes_the_image() {
    let session = Session::new("");
    let mut pty = start_kitty(&session);
    pty.signal(libc::SIGTERM);
    assert_eq!(pty.wait(Duration::from_secs(10)), Some(0));
    assert_cleaned_up(&pty.output());
}

#[test]
#[ignore = "needs mpv"]
fn sighup_deletes_the_image() {
    let session = Session::new("");
    let mut pty = start_kitty(&session);
    pty.signal(libc::SIGHUP);
    assert_eq!(pty.wait(Duration::from_secs(10)), Some(0));
    assert_cleaned_up(&pty.output());
}

#[test]
#[ignore = "needs mpv"]
fn half_blocks_in_a_plain_terminal_and_nothing_when_turned_off() {
    // Not Kitty: no query, half blocks.
    let session = Session::new("");
    let mut pty = Pty::spawn(session.command("xterm-256color"));
    pty.wait_for("▀".as_bytes(), Duration::from_secs(10));
    pty.write(b"q");
    assert_eq!(pty.wait(Duration::from_secs(10)), Some(0));
    let output = pty.output();
    assert!(find(&output, QUERY).is_none());
    assert!(find(&output, b"\x1b_G").is_none());
    // Local covers only: no cache folder is made.
    assert!(!session.dir.path().join("cache").exists());
    // Turned off, even in Kitty: no query, no image, no art.
    let session = Session::new("\n[ui]\ncover_art = false\n");
    let mut pty = Pty::spawn(session.command("xterm-kitty"));
    pty.wait_for("Now Playing".as_bytes(), Duration::from_secs(10));
    std::thread::sleep(Duration::from_millis(500));
    pty.write(b"q");
    assert_eq!(pty.wait(Duration::from_secs(10)), Some(0));
    let output = pty.output();
    assert!(find(&output, b"\x1b_G").is_none());
    assert!(find(&output, "▀".as_bytes()).is_none());
    assert!(
        !session.dir.path().join("cache").exists(),
        "no cache either"
    );
}
