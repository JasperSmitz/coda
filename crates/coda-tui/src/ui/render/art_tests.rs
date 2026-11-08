//! Cover art in the now-playing bar: layout, and the bytes a real
//! (crossterm) backend writes in each kind of redraw.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::{Backend, ClearType, CrosstermBackend, TestBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Rect, Size};

use super::*;
use crate::app::Action;
use crate::ui::cover::{Graphics, PLACEHOLDER, RESIZE_SETTLE};
use coda_core::cover::{CoverImage, CoverResponse, Outcome};
use coda_core::library::scan::ScanEvent;
use coda_core::model::{Track, TrackMetadata};
use coda_core::playback::{PlaybackCommand, PlaybackEvent};

const GHOSTTY: Graphics = Graphics::Kitty { cell: (16, 34) };

/// Everything the backend writes.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A crossterm backend writing into a [`Capture`], with a set size and
/// no tty queries, as a fullscreen terminal (like the app's).
struct CaptureBackend {
    inner: CrosstermBackend<Capture>,
    size: Size,
}

impl Write for CaptureBackend {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.inner)
    }
}

impl Backend for CaptureBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }
    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(Position::ORIGIN)
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }
    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }
    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }
    fn size(&self) -> io::Result<Size> {
        Ok(self.size)
    }
    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: self.size,
            pixels: Size::default(),
        })
    }
    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

type CaptureTerminal = Terminal<CaptureBackend>;

fn terminal(width: u16, height: u16) -> (CaptureTerminal, Capture) {
    let capture = Capture::default();
    let backend = CaptureBackend {
        inner: CrosstermBackend::new(capture.clone()),
        size: Size::new(width, height),
    };
    (Terminal::new(backend).unwrap(), capture)
}

fn view(graphics: Graphics) -> ViewState {
    ViewState {
        art: Some(ArtPainter::new(graphics, 0x0A_0000)),
        ..ViewState::default()
    }
}

/// Kitty graphics commands (their keys) in `bytes`.
fn kitty_commands(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .split("\x1b_G")
        .skip(1)
        .map(|c| {
            let c = c.split("\x1b\\").next().unwrap();
            c.split(';').next().unwrap().to_owned()
        })
        .collect()
}

fn placeholders_in(bytes: &[u8]) -> usize {
    String::from_utf8_lossy(bytes).matches(PLACEHOLDER).count()
}

fn track(rel: &str, title: &str) -> Track {
    Track::local(
        format!("/music/{rel}").into(),
        TrackMetadata {
            title: Some(title.into()),
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            duration: Some(Duration::from_secs(100)),
            ..TrackMetadata::default()
        },
    )
}

/// An app with cover art on, playing the first of two tracks (the second
/// queued for a gapless change).
fn playing(now: Instant) -> (App, LoadId, LoadId) {
    let mut app = App::new("/music".into());
    app.set_cover_art(true);
    app.handle_scan_event(
        ScanEvent::Batch(vec![track("1.flac", "First"), track("2.flac", "Second")]),
        now,
    );
    app.handle_action(Action::Down, now);
    app.handle_action(Action::Down, now);
    app.handle_action(Action::Activate, now);
    let commands = app.handle_action(Action::Activate, now);
    let (load, next) = match commands.as_slice() {
        [
            PlaybackCommand::Load { load, .. },
            PlaybackCommand::SetNext {
                next: Some((next, _)),
                ..
            },
        ] => (*load, *next),
        other => panic!("{other:?}"),
    };
    app.handle_playback_event(PlaybackEvent::Started { load }, now);
    (app, load, next)
}

use coda_core::playback::LoadId;

fn cover(shade: u8) -> Arc<CoverImage> {
    Arc::new(CoverImage::new(8, 8, [shade, 40, 200].repeat(64)).unwrap())
}

/// Answers the app's cover request (if any) with `outcome`.
fn answer_cover(app: &mut App, outcome: Outcome) {
    let requests = app.take_cover_requests();
    answer(app, requests, outcome);
}

fn answer(app: &mut App, requests: Vec<coda_core::cover::CoverRequest>, outcome: Outcome) {
    for request in requests {
        app.handle_cover_response(CoverResponse {
            id: request.id,
            generation: request.generation,
            key: request.key,
            size: request.size,
            outcome: outcome.clone(),
        });
    }
}

fn full(terminal: &mut CaptureTerminal, app: &mut App, view: &mut ViewState, now: Instant) {
    // As the loop does: cover requests are taken before every frame.
    assert!(app.take_cover_requests().is_empty(), "answer covers first");
    draw_full(terminal, app, view, now).unwrap();
}

fn text_of(buffer: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn the_bar_grows_beside_the_art_and_keeps_its_height_without_it() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    let mut view = view(Graphics::HalfBlocks);
    // No cover yet: today's 4-row bar.
    let (mut terminal, _) = terminal(100, 24);
    let requests = app.take_cover_requests();
    let buffer = draw_full(&mut terminal, &app, &mut view, now)
        .unwrap()
        .buffer
        .clone();
    let text = text_of(&buffer);
    let bar_top = text
        .lines()
        .position(|l| l.contains("Now Playing"))
        .unwrap();
    assert_eq!(bar_top, 24 - 1 - 4, "{text}");
    // With a cover: 6 rows, art 4 rows × 8 columns, text beside it.
    answer(&mut app, requests, Outcome::Art(cover(1)));
    let buffer = draw_full(&mut terminal, &app, &mut view, now)
        .unwrap()
        .buffer
        .clone();
    let text = text_of(&buffer);
    let lines: Vec<&str> = text.lines().collect();
    let bar_top = lines
        .iter()
        .position(|l| l.contains("Now Playing"))
        .unwrap();
    assert_eq!(bar_top, 24 - 1 - ART_BAR_HEIGHT as usize, "{text}");
    for row in 0..4 {
        for col in 0..8 {
            assert_eq!(buffer[(1 + col, bar_top as u16 + 1 + row)].symbol(), "▀");
        }
    }
    assert_eq!(buffer[(9, bar_top as u16 + 1)].symbol(), " ", "a gap");
    assert!(lines[bar_top + 1][..].contains("[Playing] First"), "{text}");
    assert!(lines[bar_top + 2].contains("Artist · Album"), "{text}");
    assert!(lines[bar_top + 4].contains("0:00 / 1:40"), "{text}");
    assert!(lines[bar_top + 1].find("[Playing]").unwrap() > 10);
}

#[test]
fn beside_the_visualizer_the_art_takes_all_five_rows() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    answer_cover(&mut app, Outcome::Art(cover(1)));
    app.handle_action(Action::ToggleVisualizer, now);
    let screen = Rect::new(0, 0, 100, 24);
    let layout = art_layout(screen, &app, Some(&ArtPainter::new(GHOSTTY, 1))).unwrap();
    assert_eq!(layout, ArtLayout { cols: 11, rows: 5 });
    let mut view = view(Graphics::HalfBlocks);
    let (mut terminal, _) = terminal(100, 24);
    let buffer = draw_full(&mut terminal, &app, &mut view, now)
        .unwrap()
        .buffer
        .clone();
    let text = text_of(&buffer);
    let lines: Vec<&str> = text.lines().collect();
    let bar_top = lines
        .iter()
        .position(|l| l.contains("Now Playing"))
        .unwrap();
    assert_eq!(bar_top, 24 - 1 - 7, "the visualizer's bar height: {text}");
    for row in 1..=5 {
        assert!(lines[bar_top + row].starts_with("│▀▀▀▀▀▀▀▀▀▀ "), "{text}");
    }
    assert!(
        lines[bar_top + 1].contains("[Playing] First — Artist"),
        "{text}"
    );
    // The visualizer is drawn beside the art.
    let area = view.visualizer_area.unwrap();
    assert!(area.x > 11, "{area:?}");
}

#[test]
fn the_art_hides_on_small_screens_and_without_a_cover() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    let painter = ArtPainter::new(GHOSTTY, 1);
    let requests = app.take_cover_requests();
    assert_eq!(
        art_layout(Rect::new(0, 0, 100, 24), &app, Some(&painter)),
        None
    );
    answer(&mut app, requests, Outcome::Art(cover(1)));
    let layout = |w, h| art_layout(Rect::new(0, 0, w, h), &app, Some(&painter));
    assert_eq!(layout(100, 24), Some(ArtLayout { cols: 9, rows: 4 }));
    assert_eq!(
        layout(NARROW_WIDTH, VISUALIZER_MIN_SCREEN.1).map(|l| l.rows),
        Some(4)
    );
    assert_eq!(layout(NARROW_WIDTH - 1, 24), None);
    assert_eq!(layout(100, VISUALIZER_MIN_SCREEN.1 - 1), None);
    // Turned off: no painter, no art.
    assert_eq!(art_layout(Rect::new(0, 0, 100, 24), &app, None), None);
}

#[test]
fn turned_off_the_screen_is_exactly_as_before() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    answer_cover(&mut app, Outcome::Art(cover(1)));
    let mut off = App::new("/music".into());
    off.handle_scan_event(
        ScanEvent::Batch(vec![track("1.flac", "First"), track("2.flac", "Second")]),
        now,
    );
    off.handle_action(Action::Down, now);
    off.handle_action(Action::Down, now);
    off.handle_action(Action::Activate, now);
    let commands = off.handle_action(Action::Activate, now);
    let Some(PlaybackCommand::Load { load, .. }) = commands.first() else {
        panic!()
    };
    off.handle_playback_event(PlaybackEvent::Started { load: *load }, now);
    assert!(off.take_cover_requests().is_empty(), "off: nothing asked");
    let draw_with = |app: &App, view: &mut ViewState| {
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|f| draw(f, app, view, now)).unwrap();
        terminal.backend().buffer().clone()
    };
    // A loaded cover but no painter (`cover_art = false` has none) draws
    // the same as an app that never had cover art.
    assert_eq!(
        draw_with(&app, &mut ViewState::default()),
        draw_with(&off, &mut ViewState::default())
    );
}

#[test]
fn kitty_art_is_sent_once_before_its_placeholders() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    answer_cover(&mut app, Outcome::Art(cover(1)));
    let mut view = view(GHOSTTY);
    let (mut terminal, capture) = terminal(100, 24);
    full(&mut terminal, &mut app, &mut view, now);
    let bytes = capture.take();
    let commands = kitty_commands(&bytes);
    assert!(
        // The 8 px cover: never more pixels than the picture has.
        commands[0].starts_with("a=T,U=1,q=2,f=24,i=655360,s=8,v=7,c=9,r=4"),
        "{commands:?}"
    );
    assert!(commands.iter().all(|c| c.contains("q=2")), "never answered");
    // The transmission comes before any cell is written.
    let text = String::from_utf8_lossy(&bytes);
    let last_apc = text.rfind("\x1b_G").unwrap();
    assert!(text.find(PLACEHOLDER).unwrap() > last_apc);
    assert_eq!(placeholders_in(&bytes), 9 * 4);
    // Nothing changed: a second frame writes no art at all.
    full(&mut terminal, &mut app, &mut view, now);
    let bytes = capture.take();
    assert!(kitty_commands(&bytes).is_empty());
    assert_eq!(placeholders_in(&bytes), 0);
}

#[test]
fn visualizer_only_redraws_write_no_image_bytes() {
    let now = Instant::now();
    let (mut app, load, _) = playing(now);
    answer_cover(&mut app, Outcome::Art(cover(1)));
    app.handle_action(Action::ToggleVisualizer, now);
    let mut view = view(GHOSTTY);
    let (mut terminal, capture) = terminal(100, 24);
    let completed = draw_full(&mut terminal, &app, &mut view, now).unwrap();
    let mut last = completed.buffer.clone();
    assert!(placeholders_in(&capture.take()) > 0);
    let area = view.visualizer_area.unwrap();
    for i in 0..30 {
        let level = (i % 10) as f32 / 10.0;
        let frame = coda_core::playback::VisualizerFrame {
            levels: [level, level],
            bands: [Some(level); coda_core::playback::VISUALIZER_BANDS],
        };
        app.handle_visualizer(load, &frame, now + Duration::from_millis(33 * i));
        redraw_visualizer(&mut terminal, &mut last, &app, area).unwrap();
    }
    let bytes = capture.take();
    assert!(!bytes.is_empty(), "the bars moved");
    assert!(kitty_commands(&bytes).is_empty());
    assert_eq!(placeholders_in(&bytes), 0);
}

#[test]
fn an_overlay_replaces_the_placeholders_it_covers() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    answer_cover(&mut app, Outcome::Art(cover(1)));
    let mut view = view(GHOSTTY);
    // Narrow enough for the help box to reach over the art.
    let (mut terminal, capture) = terminal(64, 24);
    full(&mut terminal, &mut app, &mut view, now);
    capture.take();
    app.handle_action(Action::ToggleHelp, now);
    let completed = draw_full(&mut terminal, &app, &mut view, now).unwrap();
    let buffer = completed.buffer.clone();
    let bytes = capture.take();
    assert!(kitty_commands(&bytes).is_empty(), "the image itself stays");
    let art_top = 24 - 1 - ART_BAR_HEIGHT + 1;
    let mut covered = 0;
    let mut shown = 0;
    for y in art_top..art_top + 4 {
        for x in 1..10 {
            if buffer[(x, y)].symbol().starts_with(PLACEHOLDER) {
                shown += 1;
            } else {
                covered += 1;
            }
        }
    }
    assert!(covered > 0 && shown > 0, "{covered} {shown}");
    // The help box's own cells hold no placeholder anywhere.
    let help_left = (64 - 52) / 2;
    for y in art_top..art_top + 4 {
        for x in help_left..10 {
            assert!(
                !buffer[(x, y)].symbol().starts_with(PLACEHOLDER),
                "({x}, {y})"
            );
        }
    }
    // Closed again: the covered cells come back, still without a new
    // transmission.
    app.handle_action(Action::ToggleHelp, now);
    full(&mut terminal, &mut app, &mut view, now);
    let bytes = capture.take();
    assert!(kitty_commands(&bytes).is_empty());
    assert_eq!(placeholders_in(&bytes), covered);
}

#[test]
fn a_clear_redraws_placeholders_without_sending_the_image_again() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    answer_cover(&mut app, Outcome::Art(cover(1)));
    let mut view = view(GHOSTTY);
    let (mut terminal, capture) = terminal(100, 24);
    full(&mut terminal, &mut app, &mut view, now);
    capture.take();
    terminal.clear().unwrap();
    full(&mut terminal, &mut app, &mut view, now);
    let bytes = capture.take();
    assert!(kitty_commands(&bytes).is_empty());
    assert_eq!(placeholders_in(&bytes), 9 * 4);
}

#[test]
fn a_gapless_change_swaps_the_image_when_the_next_track_is_heard() {
    let now = Instant::now();
    let (mut app, load, next) = playing(now);
    answer_cover(&mut app, Outcome::Art(cover(1)));
    let mut view = view(GHOSTTY);
    let (mut terminal, capture) = terminal(100, 24);
    full(&mut terminal, &mut app, &mut view, now);
    capture.take();
    // mpv moved on, but the first track is still heard: no change.
    app.handle_playback_event(
        PlaybackEvent::Advanced {
            from: load,
            to: next,
        },
        now,
    );
    assert!(app.take_cover_requests().is_empty());
    full(&mut terminal, &mut app, &mut view, now);
    assert!(kitty_commands(&capture.take()).is_empty());
    // Heard: the second track's cover is asked for; the first stays on
    // screen until it arrives, then is replaced.
    app.handle_playback_event(PlaybackEvent::Started { load: next }, now);
    let requests = app.take_cover_requests();
    assert_eq!(requests.len(), 1);
    full(&mut terminal, &mut app, &mut view, now);
    assert!(
        kitty_commands(&capture.take()).is_empty(),
        "no blank meanwhile"
    );
    answer(&mut app, requests, Outcome::Art(cover(2)));
    full(&mut terminal, &mut app, &mut view, now);
    let bytes = capture.take();
    let commands = kitty_commands(&bytes);
    assert_eq!(commands[0], "a=d,d=I,i=655360,q=2");
    assert!(
        commands[1].starts_with("a=T,U=1,q=2,f=24,i=655361,"),
        "{commands:?}"
    );
    assert_eq!(placeholders_in(&bytes), 9 * 4);
}

#[test]
fn the_same_cover_across_a_change_is_neither_deleted_nor_sent_again() {
    let now = Instant::now();
    let (mut app, load, next) = playing(now);
    answer_cover(&mut app, Outcome::Art(cover(1)));
    let mut view = view(GHOSTTY);
    let (mut terminal, capture) = terminal(100, 24);
    full(&mut terminal, &mut app, &mut view, now);
    capture.take();
    app.handle_playback_event(
        PlaybackEvent::Advanced {
            from: load,
            to: next,
        },
        now,
    );
    app.handle_playback_event(PlaybackEvent::Started { load: next }, now);
    let requests = app.take_cover_requests();
    full(&mut terminal, &mut app, &mut view, now);
    // The next track's file has the same picture (another image object).
    answer(&mut app, requests, Outcome::Art(cover(1)));
    full(&mut terminal, &mut app, &mut view, now);
    let bytes = capture.take();
    assert!(
        kitty_commands(&bytes).is_empty(),
        "no delete, no transmission"
    );
    assert_eq!(placeholders_in(&bytes), 0, "the cells never changed");
}

#[test]
fn no_art_or_a_slow_cover_removes_the_previous_one() {
    for slow in [false, true] {
        let now = Instant::now();
        let (mut app, load, next) = playing(now);
        answer_cover(&mut app, Outcome::Art(cover(1)));
        let mut view = view(GHOSTTY);
        let (mut terminal, capture) = terminal(100, 24);
        full(&mut terminal, &mut app, &mut view, now);
        capture.take();
        app.handle_playback_event(
            PlaybackEvent::Advanced {
                from: load,
                to: next,
            },
            now,
        );
        app.handle_playback_event(PlaybackEvent::Started { load: next }, now);
        let requests = app.take_cover_requests();
        full(&mut terminal, &mut app, &mut view, now);
        assert!(kitty_commands(&capture.take()).is_empty());
        if slow {
            app.tick(Instant::now() + coda_core::cover::LINGER);
        } else {
            answer(&mut app, requests, Outcome::NoArt);
        }
        full(&mut terminal, &mut app, &mut view, now);
        let bytes = capture.take();
        assert_eq!(
            kitty_commands(&bytes),
            ["a=d,d=I,i=655360,q=2"],
            "slow: {slow}"
        );
        assert_eq!(placeholders_in(&bytes), 0);
    }
}

#[test]
fn a_track_change_under_an_open_overlay_still_transmits() {
    let now = Instant::now();
    let (mut app, load, next) = playing(now);
    answer_cover(&mut app, Outcome::Art(cover(1)));
    let mut view = view(GHOSTTY);
    let (mut terminal, capture) = terminal(100, 24);
    full(&mut terminal, &mut app, &mut view, now);
    app.handle_action(Action::ToggleHelp, now);
    full(&mut terminal, &mut app, &mut view, now);
    capture.take();
    app.handle_playback_event(
        PlaybackEvent::Advanced {
            from: load,
            to: next,
        },
        now,
    );
    app.handle_playback_event(PlaybackEvent::Started { load: next }, now);
    answer_cover(&mut app, Outcome::Art(cover(2)));
    full(&mut terminal, &mut app, &mut view, now);
    let commands = kitty_commands(&capture.take());
    assert_eq!(commands[0], "a=d,d=I,i=655360,q=2");
    assert!(
        commands[1].starts_with("a=T,U=1,q=2,f=24,i=655361,"),
        "{commands:?}"
    );
    // Closing the overlay needs no new transmission.
    app.handle_action(Action::ToggleHelp, now);
    full(&mut terminal, &mut app, &mut view, now);
    assert!(kitty_commands(&capture.take()).is_empty());
}

#[test]
fn the_image_is_deleted_when_the_art_hides() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    answer_cover(&mut app, Outcome::Art(cover(1)));
    let mut view = view(GHOSTTY);
    let (mut terminal, capture) = terminal(100, 24);
    full(&mut terminal, &mut app, &mut view, now);
    capture.take();
    // The screen shrinks below the art's minimum.
    terminal.backend_mut().size = Size::new(50, 24);
    full(&mut terminal, &mut app, &mut view, now);
    let bytes = capture.take();
    assert_eq!(kitty_commands(&bytes), ["a=d,d=I,i=655360,q=2"]);
    assert_eq!(placeholders_in(&bytes), 0);
    // And at exit, nothing is left to delete.
    assert!(view.art.as_mut().unwrap().finish().is_empty());
}

#[test]
fn half_block_art_writes_no_escape_sequences() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    answer_cover(&mut app, Outcome::Art(cover(1)));
    let mut view = view(Graphics::HalfBlocks);
    let (mut terminal, capture) = terminal(100, 24);
    full(&mut terminal, &mut app, &mut view, now);
    let bytes = capture.take();
    assert!(kitty_commands(&bytes).is_empty());
    assert_eq!(placeholders_in(&bytes), 0);
    assert!(String::from_utf8_lossy(&bytes).matches('▀').count() >= 8 * 4);
}

// ----- the Now playing screen -------------------------------------------

fn big_cover(side: u32) -> Arc<CoverImage> {
    Arc::new(CoverImage::new(side, side, [9, 140, 70].repeat((side * side) as usize)).unwrap())
}

const MIN_COLUMN_FOR_TEST: u16 = crate::ui::now_playing::MIN_COLUMN;

/// The Kitty commands that start something (a transmission's first chunk
/// or a delete), without the chunks that continue a transmission.
fn heads(bytes: &[u8]) -> Vec<String> {
    kitty_commands(bytes)
        .into_iter()
        .filter(|c| c.starts_with("a="))
        .collect()
}

/// The `s=`, `v=`, `c=` and `r=` of a transmission.
fn sizes(command: &str) -> Vec<String> {
    command
        .split(',')
        .filter(|kv| ["s=", "v=", "c=", "r="].iter().any(|k| kv.starts_with(k)))
        .map(str::to_owned)
        .collect()
}

#[test]
fn opening_and_closing_the_screen_sends_the_cover_again_at_each_size() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    // Large enough not to limit the pixels sent (the app is not told that
    // a large cover is useful, so none is asked for).
    answer_cover(&mut app, Outcome::Art(big_cover(1024)));
    let mut view = view(GHOSTTY);
    let (mut terminal, capture) = terminal(100, 30);
    full(&mut terminal, &mut app, &mut view, now);
    let bar = kitty_commands(&capture.take());
    assert_eq!(sizes(&bar[0]), ["s=144", "v=136", "c=9", "r=4"]);
    // Open: the bar's image is deleted and the cover sent at the screen's
    // size, at once.
    app.handle_action(Action::ToggleNowPlaying, now);
    full(&mut terminal, &mut app, &mut view, now);
    let bytes = capture.take();
    let screen = kitty_commands(&bytes);
    assert_eq!(screen[0], "a=d,d=I,i=655360,q=2");
    assert!(screen[1].starts_with("a=T,U=1,q=2,f=24,i=655361,"));
    // Side by side: 55 % of 29 rows, 15 rows of 34 px, 32 columns of 16.
    assert_eq!(sizes(&screen[1]), ["s=512", "v=510", "c=32", "r=15"]);
    assert_eq!(placeholders_in(&bytes), 32 * 15);
    // Nothing changed: nothing sent.
    full(&mut terminal, &mut app, &mut view, now);
    let bytes = capture.take();
    assert!(kitty_commands(&bytes).is_empty());
    assert_eq!(placeholders_in(&bytes), 0);
    // Closed: back to the bar's size, the screen's image deleted.
    app.handle_action(Action::ToggleNowPlaying, now);
    full(&mut terminal, &mut app, &mut view, now);
    let back = kitty_commands(&capture.take());
    assert_eq!(back[0], "a=d,d=I,i=655361,q=2");
    assert_eq!(sizes(&back[1]), ["s=144", "v=136", "c=9", "r=4"]);
}

#[test]
fn the_large_cover_replaces_the_small_one_once() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    app.handle_action(Action::ToggleNowPlaying, now);
    let small = app.take_cover_requests();
    assert_eq!(small[0].size, coda_core::cover::CoverSize::Small);
    answer(&mut app, small, Outcome::Art(big_cover(256)));
    let mut view = view(GHOSTTY);
    let (mut terminal, capture) = terminal(100, 30);
    // The small cover shows meanwhile, never a blank: sent at its own
    // 256 px, in the area's shape.
    full(&mut terminal, &mut app, &mut view, now);
    let first = kitty_commands(&capture.take());
    let sent: Vec<&String> = first.iter().filter(|c| c.starts_with("a=T")).collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(sizes(sent[0]), ["s=256", "v=255", "c=32", "r=15"]);
    // The frame found it worth a large cover (512 px of area, Kitty), as
    // the loop then tells the app.
    assert!(view.large_cover_useful);
    app.set_large_cover_useful(view.large_cover_useful);
    let large = app.take_cover_requests();
    assert_eq!(large[0].size, coda_core::cover::CoverSize::Large);
    answer(&mut app, large, Outcome::Art(big_cover(1024)));
    full(&mut terminal, &mut app, &mut view, now);
    let bytes = capture.take();
    let change = kitty_commands(&bytes);
    assert_eq!(change[0], "a=d,d=I,i=655360,q=2");
    assert!(change[1].starts_with("a=T,U=1,q=2,f=24,i=655361,"));
    // Same cells; no more after that.
    assert_eq!(sizes(&change[1]), ["s=512", "v=510", "c=32", "r=15"]);
    full(&mut terminal, &mut app, &mut view, now);
    assert!(kitty_commands(&capture.take()).is_empty());
}

#[test]
fn no_large_cover_is_useful_with_half_blocks_or_a_small_area() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    answer_cover(&mut app, Outcome::Art(big_cover(256)));
    app.handle_action(Action::ToggleNowPlaying, now);
    let mut half = view(Graphics::HalfBlocks);
    let (mut terminal, _) = terminal(100, 30);
    full(&mut terminal, &mut app, &mut half, now);
    assert!(!half.large_cover_useful);
    // Kitty, but the cover takes at most 256 px: 7 rows of 34 px (55 %
    // of 14 rows).
    let mut kitty = view(GHOSTTY);
    let (mut terminal, _) = self::terminal(100, 15);
    full(&mut terminal, &mut app, &mut kitty, now);
    assert!(!kitty.large_cover_useful);
    let (mut terminal, _) = self::terminal(100, 16);
    full(&mut terminal, &mut app, &mut kitty, now);
    assert!(kitty.large_cover_useful, "8 rows: 272 px");
    // Closed: never.
    app.handle_action(Action::ToggleNowPlaying, now);
    full(&mut terminal, &mut app, &mut kitty, now);
    assert!(!kitty.large_cover_useful);
}

#[test]
fn a_resize_sends_the_screen_cover_again_once_the_size_settles() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    answer_cover(&mut app, Outcome::Art(big_cover(1024)));
    app.handle_action(Action::ToggleNowPlaying, now);
    let mut view = view(GHOSTTY);
    let (mut terminal, capture) = terminal(100, 30);
    full(&mut terminal, &mut app, &mut view, now);
    capture.take();
    let resize = |terminal: &mut CaptureTerminal, width, height| {
        terminal.backend_mut().size = Size::new(width, height);
        terminal
            .resize(Rect::new(0, 0, width, height))
            .expect("the test backend resizes");
    };
    // A drag: two sizes in quick succession. The old image goes at once,
    // the area stays blank, and nothing is sent yet.
    resize(&mut terminal, 110, 35);
    full(&mut terminal, &mut app, &mut view, now);
    let bytes = capture.take();
    assert_eq!(kitty_commands(&bytes), ["a=d,d=I,i=655360,q=2"]);
    assert_eq!(placeholders_in(&bytes), 0);
    assert!(view.art.as_ref().unwrap().waiting());
    let later = now + Duration::from_millis(100);
    resize(&mut terminal, 120, 40);
    full(&mut terminal, &mut app, &mut view, later);
    assert!(kitty_commands(&capture.take()).is_empty());
    // Still within the settle time of the last change: still nothing.
    let settling = later + RESIZE_SETTLE - Duration::from_millis(1);
    full(&mut terminal, &mut app, &mut view, settling);
    assert!(kitty_commands(&capture.take()).is_empty());
    // Held still (the tick draws again): sent once, at the new size.
    let settled = later + RESIZE_SETTLE;
    full(&mut terminal, &mut app, &mut view, settled);
    let bytes = capture.take();
    let commands = heads(&bytes);
    assert_eq!(commands.len(), 1, "{commands:?}");
    // Side by side: 55 % of 39 rows, 21 × 34 px by 45 × 16 px.
    assert_eq!(sizes(&commands[0]), ["s=720", "v=714", "c=45", "r=21"]);
    assert_eq!(placeholders_in(&bytes), 45 * 21);
    full(&mut terminal, &mut app, &mut view, settled);
    assert!(kitty_commands(&capture.take()).is_empty());
}

#[test]
fn opening_the_screen_or_a_new_track_during_a_resize_wait_only_for_a_new_size() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    answer_cover(&mut app, Outcome::Art(big_cover(1024)));
    let mut view = view(GHOSTTY);
    let (mut terminal, capture) = terminal(100, 30);
    full(&mut terminal, &mut app, &mut view, now);
    capture.take();
    // A resize that leaves the bar's art the same size: nothing to wait for.
    terminal.backend_mut().size = Size::new(120, 30);
    terminal.resize(Rect::new(0, 0, 120, 30)).unwrap();
    full(&mut terminal, &mut app, &mut view, now);
    assert!(kitty_commands(&capture.take()).is_empty());
    assert!(!view.art.as_ref().unwrap().waiting());
    // Opening the screen right after it is a new size during the settle
    // time; once settled it is sent.
    app.handle_action(Action::ToggleNowPlaying, now);
    full(&mut terminal, &mut app, &mut view, now + RESIZE_SETTLE);
    let commands = heads(&capture.take());
    assert_eq!(commands.len(), 2, "{commands:?}");
    // Without a resize, opening and closing are immediate.
    let later = now + Duration::from_secs(1);
    app.handle_action(Action::ToggleNowPlaying, now);
    full(&mut terminal, &mut app, &mut view, later);
    let commands = kitty_commands(&capture.take());
    assert!(commands[1].starts_with("a=T"), "{commands:?}");
}

#[test]
fn help_over_the_screen_covers_the_cover_without_a_new_transmission() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    answer_cover(&mut app, Outcome::Art(cover(1)));
    app.handle_action(Action::ToggleNowPlaying, now);
    let mut view = view(GHOSTTY);
    let (mut terminal, capture) = terminal(140, 40);
    full(&mut terminal, &mut app, &mut view, now);
    capture.take();
    app.handle_action(Action::ToggleHelp, now);
    let buffer = draw_full(&mut terminal, &app, &mut view, now)
        .unwrap()
        .buffer
        .clone();
    assert!(kitty_commands(&capture.take()).is_empty());
    // The help box (52 wide, centred) holds no placeholder.
    let help = Rect::new((140 - 52) / 2, 8, 52, 24);
    for y in help.y..help.bottom() {
        for x in help.x..help.right() {
            assert!(
                !buffer[(x, y)].symbol().starts_with(PLACEHOLDER),
                "({x}, {y})"
            );
        }
    }
    // Some of the cover still shows beside it.
    let shown = (0..40)
        .flat_map(|y| (0..140).map(move |x| (x, y)))
        .filter(|&(x, y)| buffer[(x, y)].symbol().starts_with(PLACEHOLDER))
        .count();
    assert!(shown > 0 && shown < 64 * 30, "{shown}");
}

#[test]
fn visualizer_only_redraws_on_the_screen_write_no_image_bytes() {
    let now = Instant::now();
    let (mut app, load, _) = playing(now);
    answer_cover(&mut app, Outcome::Art(cover(1)));
    app.handle_action(Action::ToggleVisualizer, now);
    app.handle_action(Action::ToggleNowPlaying, now);
    let mut view = view(GHOSTTY);
    let (mut terminal, capture) = terminal(100, 30);
    let completed = draw_full(&mut terminal, &app, &mut view, now).unwrap();
    let mut last = completed.buffer.clone();
    assert!(placeholders_in(&capture.take()) > 0);
    let area = view.visualizer_area.expect("the screen's visualizer");
    assert!(
        area.height >= 3 && area.width >= MIN_COLUMN_FOR_TEST,
        "{area:?}"
    );
    for i in 0..30 {
        let level = (i % 10) as f32 / 10.0;
        let frame = coda_core::playback::VisualizerFrame {
            levels: [level, level],
            bands: [Some(level); coda_core::playback::VISUALIZER_BANDS],
        };
        app.handle_visualizer(load, &frame, now + Duration::from_millis(33 * i));
        redraw_visualizer(&mut terminal, &mut last, &app, area).unwrap();
    }
    let bytes = capture.take();
    assert!(!bytes.is_empty(), "the bars moved");
    assert!(kitty_commands(&bytes).is_empty());
    assert_eq!(placeholders_in(&bytes), 0);
}

#[test]
fn a_resize_from_side_by_side_to_stacked_is_debounced_too() {
    let now = Instant::now();
    let (mut app, _, _) = playing(now);
    answer_cover(&mut app, Outcome::Art(big_cover(1024)));
    app.handle_action(Action::ToggleNowPlaying, now);
    let mut view = view(GHOSTTY);
    let (mut terminal, capture) = terminal(120, 40);
    full(&mut terminal, &mut app, &mut view, now);
    let first = heads(&capture.take());
    assert_eq!(sizes(&first[0]), ["s=720", "v=714", "c=45", "r=21"]);
    // Narrowed to 60 columns: no room beside the cover, so stacked.
    terminal.backend_mut().size = Size::new(60, 40);
    terminal.resize(Rect::new(0, 0, 60, 40)).unwrap();
    let screen = Rect::new(0, 0, 60, 39);
    let layout = crate::ui::now_playing::layout_for(screen, &app, view.art.as_ref());
    assert!(!layout.side_by_side);
    full(&mut terminal, &mut app, &mut view, now);
    let bytes = capture.take();
    assert_eq!(heads(&bytes), ["a=d,d=I,i=655360,q=2"], "deleted, not sent");
    assert_eq!(placeholders_in(&bytes), 0);
    full(&mut terminal, &mut app, &mut view, now + RESIZE_SETTLE);
    let sent = heads(&capture.take());
    assert_eq!(sent.len(), 1, "{sent:?}");
    let cover = layout.cover.unwrap();
    assert!(sent[0].contains(&format!("c={},r={}", cover.width, cover.height)));
}
