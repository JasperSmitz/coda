//! The main window's full-screen state and its saved frame.
//!
//! Tauri's `is_fullscreen` returns the value the app last set, not the
//! window manager's: on Linux full screen entered with a WM key is
//! invisible to it (LESSONS). So on Linux the real GDK state is read, and
//! its changes are sent as the `window-fullscreen` event (the resize that
//! goes with them can arrive before GTK's state changes).
//!
//! The frame (position, size, maximised) is ours, not
//! `tauri-plugin-window-state`'s: that plugin saves the full-screen frame
//! as the window size, and a maximised window's earlier position only
//! through its own move tracking. [`Tracker`] follows the window's moves
//! and resizes while it is windowed (not full screen or minimised; while
//! maximised only that flag), [`save`] writes the last windowed frame on
//! close and quit (`<state>/coda-app/window.json`), and [`saved`] gives it
//! back for the window builder (position and size set before the window
//! first shows; maximised after). Every launch is windowed. Frames are in
//! logical units (points), which mean the same on every screen; physical
//! pixels depend on each screen's scale.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{Manager, Runtime};

use crate::theme::fsio;

/// The saved file stays tiny; anything bigger is not ours.
const MAX_FILE_BYTES: u64 = 4096;
/// Sizes outside this range (logical units) are ignored.
const SIZE_RANGE: std::ops::RangeInclusive<f64> = 200.0..=20_000.0;
/// Positions beyond this (logical units) are ignored.
const POSITION_LIMIT: f64 = 100_000.0;
/// The file format; a file without it (or another) is ignored.
const VERSION: u32 = 1;

/// The window's windowed frame, in logical units.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Maximised (macOS: zoomed) when last windowed; `x`..`height` are then
    /// the frame it had before.
    pub maximized: bool,
}

impl Frame {
    fn valid(&self) -> bool {
        SIZE_RANGE.contains(&self.width)
            && SIZE_RANGE.contains(&self.height)
            && self.x.abs() <= POSITION_LIMIT
            && self.y.abs() <= POSITION_LIMIT
    }
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    #[serde(flatten)]
    frame: Frame,
}

/// What the window looks like at one event (logical units).
#[derive(Clone, Copy, Debug)]
pub struct Observed {
    pub frame: Frame,
    pub minimized: bool,
    pub fullscreen: bool,
}

/// The last windowed frame, kept up to date from window events (main
/// thread) and read at save time.
#[derive(Default)]
pub struct Tracker {
    state: Mutex<TrackerState>,
}

#[derive(Default)]
struct TrackerState {
    last: Option<Frame>,
    /// When `last` came; `None`: settled (restored or undone).
    changed: Option<Instant>,
    /// The last frame that stayed for at least [`SETTLE`]: what a
    /// maximised or full-screen state falls back to, since the resizes
    /// leading to it (GTK may report the screen-sized one first; a macOS
    /// zoom animates) are not the window's own frame.
    settled: Option<Frame>,
    /// The frame last written (or restored), so an unchanged one is not
    /// written again.
    written: Option<Frame>,
}

/// How long a frame has to stay to count as settled.
const SETTLE: Duration = Duration::from_millis(500);

impl TrackerState {
    /// The settled frame now: `last` once it has stayed long enough.
    fn settled_now(&self) -> Option<Frame> {
        match self.changed {
            Some(at) if at.elapsed() < SETTLE => self.settled,
            _ => self.last,
        }
    }

    /// Back to the settled frame (a maximised or full-screen state came).
    fn undo_to_settled(&mut self) {
        if let Some(settled) = self.settled_now() {
            self.last = Some(settled);
        }
        self.changed = None;
    }
}

impl Tracker {
    /// The frame the window was created with (`written`: it is already
    /// in the file).
    pub fn start(&self, frame: Option<Frame>, written: bool) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.last = frame;
        state.changed = None;
        state.settled = frame;
        state.written = if written { frame } else { None };
    }

    /// Follows a move or resize. A full-screen state undoes the resizes
    /// just before it; a maximised one keeps the settled frame before it;
    /// minimised frames are not the window's.
    pub fn observe(&self, now: Observed) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if now.fullscreen {
            if state.changed.is_some() {
                state.undo_to_settled();
            }
            return;
        }
        if now.minimized {
            return;
        }
        let next = if now.frame.maximized {
            if state.last.is_some_and(|last| last.maximized) {
                return;
            }
            let Some(base) = state.settled_now() else {
                return;
            };
            state.undo_to_settled();
            Frame {
                maximized: true,
                ..base
            }
        } else {
            now.frame
        };
        if state.last == Some(next) {
            return;
        }
        // The frame being replaced stayed long enough: it is settled.
        state.settled = state.settled_now();
        state.last = Some(next);
        state.changed = (!next.maximized).then(Instant::now);
    }

    /// The window just went full screen (Linux: GTK's state, which may
    /// come after the resize to the screen's size).
    pub fn entered_fullscreen(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.changed.is_some() {
            state.undo_to_settled();
        }
    }

    pub fn last(&self) -> Option<Frame> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).last
    }

    /// The last frame if it differs from the one written, marked written.
    fn take_unwritten(&self) -> Option<Frame> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let last = state.last?;
        if state.written == Some(last) {
            return None;
        }
        state.written = Some(last);
        Some(last)
    }
}

/// The saved frame at `path`, if there is a valid one.
pub fn load(path: &Path) -> Option<Frame> {
    let text = fsio::read_regular(path, MAX_FILE_BYTES).ok()??;
    let file: File = serde_json::from_str(&text).ok()?;
    (file.version == VERSION && file.frame.valid()).then_some(file.frame)
}

/// Writes `frame` to `path` (atomically).
pub fn write(path: &Path, frame: &Frame) -> std::io::Result<()> {
    let file = File {
        version: VERSION,
        frame: *frame,
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(std::io::Error::other)?;
    fsio::write_atomic(path, &bytes)
}

/// Where the frame is saved; managed as app state.
pub struct FramePath(pub Option<PathBuf>);

/// A screen in logical units: `(x, y, width, height)`.
type Rect = (f64, f64, f64, f64);

/// The saved frame to create the window with: its position only while a
/// screen still shows at least 100×100 points of it (else `None` there,
/// and the system places it). Starts the tracker.
pub fn saved<R: Runtime>(app: &tauri::AppHandle<R>) -> Option<(Frame, bool)> {
    let path = app.state::<FramePath>();
    let frame = path.0.as_deref().and_then(load);
    app.state::<Tracker>().start(frame, true);
    let frame = frame?;
    let screens: Vec<Rect> = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let scale = m.scale_factor();
            let position = m.position().to_logical::<f64>(scale);
            let size = m.size().to_logical::<f64>(scale);
            (position.x, position.y, size.width, size.height)
        })
        .collect();
    Some((frame, on_screen(&frame, &screens)))
}

fn on_screen(frame: &Frame, screens: &[Rect]) -> bool {
    screens.iter().any(|&screen| {
        overlap((frame.x, frame.y, frame.width, frame.height), screen) >= 100.0 * 100.0
    })
}

/// The overlapping area of two rectangles.
fn overlap(a: Rect, b: Rect) -> f64 {
    let span = |a0: f64, a_len: f64, b0: f64, b_len: f64| {
        ((a0 + a_len).min(b0 + b_len) - a0.max(b0)).max(0.0)
    };
    span(a.0, a.2, b.0, b.2) * span(a.1, a.3, b.1, b.3)
}

/// Follows the window's moves and resizes (window events, main thread).
pub fn track<R: Runtime>(window: &tauri::Window<R>, event: &tauri::WindowEvent) {
    if !matches!(
        event,
        tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_)
    ) {
        return;
    }
    let Some(tracker) = window.try_state::<Tracker>() else {
        return;
    };
    let (Ok(scale), Ok(position), Ok(size)) = (
        window.scale_factor(),
        window.outer_position(),
        window.inner_size(),
    ) else {
        return;
    };
    let position = position.to_logical::<f64>(scale);
    let size = size.to_logical::<f64>(scale);
    tracker.observe(Observed {
        frame: Frame {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
            maximized: window.is_maximized().unwrap_or(false),
        },
        minimized: window.is_minimized().unwrap_or(false),
        fullscreen: is_fullscreen(window),
    });
}

/// Writes the last windowed frame (close and quit; main thread) unless
/// it is already written. A full-screen frame is never what is saved.
pub fn save<R: Runtime, M: Manager<R>>(manager: &M) {
    let (Some(path), Some(tracker)) = (
        manager.try_state::<FramePath>(),
        manager.try_state::<Tracker>(),
    ) else {
        return;
    };
    let Some(path) = path.0.as_deref() else {
        return;
    };
    let Some(frame) = tracker.take_unwritten() else {
        return;
    };
    if let Err(error) = write(path, &frame) {
        tracing::warn!(%error, "could not save the window frame");
    }
}

/// Whether the window is full screen, however it got there. Main thread
/// only on Linux (GTK).
pub fn is_fullscreen<R: Runtime>(window: &tauri::Window<R>) -> bool {
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::WidgetExt;
        window
            .gtk_window()
            .ok()
            .and_then(|gtk_window| gtk_window.window())
            .is_some_and(|gdk_window| {
                gdk_window
                    .state()
                    .contains(gtk::gdk::WindowState::FULLSCREEN)
            })
    }
    #[cfg(not(target_os = "linux"))]
    {
        window.is_fullscreen().unwrap_or(false)
    }
}

/// Linux: emits `window-fullscreen` (the new state) when the window
/// manager changes it. Call on the main thread, once.
#[cfg(target_os = "linux")]
pub fn watch<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    use gtk::prelude::WidgetExt;
    use tauri::Emitter;
    let Ok(gtk_window) = window.gtk_window() else {
        return;
    };
    let window = window.clone();
    gtk_window.connect_window_state_event(move |_, event| {
        if event
            .changed_mask()
            .contains(gtk::gdk::WindowState::FULLSCREEN)
        {
            let on = event
                .new_window_state()
                .contains(gtk::gdk::WindowState::FULLSCREEN);
            if on {
                window.state::<Tracker>().entered_fullscreen();
            }
            let _ = window.emit("window-fullscreen", on);
        }
        gtk::glib::Propagation::Proceed
    });
}

pub mod commands {
    use tokio::sync::oneshot;

    /// Whether the window is full screen (see [`super::is_fullscreen`]),
    /// read on the main thread.
    #[tauri::command]
    pub async fn window_is_fullscreen(window: tauri::Window) -> Result<bool, String> {
        let (tx, rx) = oneshot::channel();
        let on_main = window.clone();
        window
            .run_on_main_thread(move || {
                let _ = tx.send(super::is_fullscreen(&on_main));
            })
            .map_err(|e| e.to_string())?;
        rx.await.map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: f64, width: f64, maximized: bool) -> Frame {
        Frame {
            x,
            y: 40.0,
            width,
            height: 700.0,
            maximized,
        }
    }

    fn seen(frame: Frame) -> Observed {
        Observed {
            frame,
            minimized: false,
            fullscreen: false,
        }
    }

    /// A tracker whose current frame `f` has settled.
    fn settled_at(f: Frame) -> Tracker {
        let t = Tracker::default();
        t.start(Some(f), false);
        t
    }

    /// Makes the last change old enough to count as settled.
    fn age(t: &Tracker) {
        let mut state = t.state.lock().unwrap();
        state.changed = state.changed.map(|_| Instant::now() - SETTLE * 2);
    }

    #[test]
    fn follows_the_windowed_frame_only() {
        let t = settled_at(frame(10.0, 1000.0, false));
        t.observe(seen(frame(20.0, 1100.0, false)));
        assert_eq!(t.last(), Some(frame(20.0, 1100.0, false)));
        t.observe(Observed {
            minimized: true,
            ..seen(frame(-32000.0, 200.0, false))
        });
        assert_eq!(t.last(), Some(frame(20.0, 1100.0, false)));
    }

    #[test]
    fn maximised_keeps_the_settled_frame_before_it() {
        let t = settled_at(frame(20.0, 1100.0, false));
        t.observe(seen(frame(0.0, 1800.0, true)));
        assert_eq!(t.last(), Some(frame(20.0, 1100.0, true)));
        // Full screen while maximised: still maximised, never the screen's frame.
        t.observe(Observed {
            fullscreen: true,
            ..seen(frame(0.0, 1920.0, true))
        });
        assert_eq!(t.last(), Some(frame(20.0, 1100.0, true)));
        t.observe(seen(frame(30.0, 1100.0, false)));
        assert_eq!(t.last(), Some(frame(30.0, 1100.0, false)));
    }

    #[test]
    fn resizes_just_before_a_maximised_state_are_undone() {
        // GTK: several configures (screen-sized) before the maximised state.
        let t = settled_at(frame(20.0, 1100.0, false));
        t.observe(seen(frame(0.0, 1700.0, false)));
        t.observe(seen(frame(0.0, 1800.0, false)));
        t.observe(seen(frame(0.0, 1800.0, true)));
        assert_eq!(t.last(), Some(frame(20.0, 1100.0, true)));
    }

    #[test]
    fn an_animated_zoom_falls_back_to_the_frame_before_it() {
        // A move that settled, then the zoom's animation frames.
        let t = settled_at(frame(20.0, 1100.0, false));
        t.observe(seen(frame(60.0, 1100.0, false)));
        age(&t);
        for step in [1200.0, 1400.0, 1600.0] {
            t.observe(seen(frame(40.0, step, false)));
        }
        t.observe(seen(frame(0.0, 1800.0, true)));
        assert_eq!(t.last(), Some(frame(60.0, 1100.0, true)));
    }

    #[test]
    fn resizes_just_before_full_screen_are_undone() {
        let t = settled_at(frame(20.0, 1100.0, false));
        t.observe(seen(frame(0.0, 1920.0, false)));
        t.entered_fullscreen();
        assert_eq!(t.last(), Some(frame(20.0, 1100.0, false)));
        // The same when the full-screen state comes with a resize.
        let t = settled_at(frame(20.0, 1100.0, false));
        t.observe(seen(frame(0.0, 1920.0, false)));
        t.observe(Observed {
            fullscreen: true,
            ..seen(frame(0.0, 1920.0, false))
        });
        assert_eq!(t.last(), Some(frame(20.0, 1100.0, false)));
    }

    #[test]
    fn a_settled_frame_is_kept_through_full_screen() {
        let t = settled_at(frame(20.0, 1100.0, false));
        t.observe(seen(frame(80.0, 1200.0, false)));
        age(&t);
        t.entered_fullscreen();
        assert_eq!(t.last(), Some(frame(80.0, 1200.0, false)));
    }

    #[test]
    fn nothing_to_keep_before_any_windowed_frame() {
        let t = Tracker::default();
        t.observe(seen(frame(0.0, 1800.0, true)));
        assert_eq!(t.last(), None);
    }

    #[test]
    fn an_unchanged_frame_is_written_once() {
        let t = Tracker::default();
        t.start(Some(frame(20.0, 1100.0, false)), true);
        assert_eq!(t.take_unwritten(), None);
        t.observe(seen(frame(30.0, 1100.0, false)));
        assert_eq!(t.take_unwritten(), Some(frame(30.0, 1100.0, false)));
        assert_eq!(t.take_unwritten(), None);
    }

    #[test]
    fn saved_frames_round_trip_and_bad_or_old_ones_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coda-app").join("window.json");
        assert_eq!(load(&path), None);
        write(&path, &frame(-1200.5, 1000.0, true)).unwrap();
        assert_eq!(load(&path), Some(frame(-1200.5, 1000.0, true)));
        for bad in [
            // No version: the earlier physical-pixel file.
            r#"{"x":0,"y":0,"width":1000,"height":700,"maximized":false}"#,
            r#"{"version":2,"x":0,"y":0,"width":1000,"height":700,"maximized":false}"#,
            r#"{"version":1,"x":0,"y":0,"width":5,"height":700,"maximized":false}"#,
            r#"{"version":1,"x":1e9,"y":0,"width":1000,"height":700,"maximized":false}"#,
            "not json",
        ] {
            std::fs::write(&path, bad).unwrap();
            assert_eq!(load(&path), None, "{bad}");
        }
    }

    #[test]
    fn a_frame_is_on_screen_when_enough_of_it_shows() {
        // Logical units: a 1440×900 screen and a scaled 1920×1080 one right of it.
        let screens = [(0.0, 0.0, 1440.0, 900.0), (1440.0, 0.0, 1920.0, 1080.0)];
        assert!(on_screen(&frame(2000.0, 1000.0, false), &screens));
        assert!(!on_screen(&frame(3350.0, 1000.0, false), &screens));
        assert!(!on_screen(&frame(-5000.0, 1000.0, false), &screens));
        assert_eq!(
            overlap((0.0, 0.0, 100.0, 100.0), (50.0, 50.0, 100.0, 100.0)),
            2500.0
        );
    }
}
