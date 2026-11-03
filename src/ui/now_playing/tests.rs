use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use super::*;
use crate::app::{Action, Focus};
use crate::cover::{CoverImage, CoverResponse, Outcome};
use crate::library::scan::ScanEvent;
use crate::model::{Track, TrackMetadata};
use crate::playback::{LoadId, PlaybackCommand, PlaybackEvent};
use crate::ui::cover::Graphics;
use crate::ui::render::{ViewState, draw};

const HALF: Graphics = Graphics::HalfBlocks;
const GHOSTTY: Graphics = Graphics::Kitty { cell: (9, 19) };

fn painter(graphics: Graphics) -> ArtPainter {
    ArtPainter::new(graphics, 0x0A_0000)
}

fn area(width: u16, height: u16) -> Rect {
    Rect::new(0, 0, width, height)
}

fn rows(rect: Option<Rect>) -> Option<(u16, u16)> {
    rect.map(|r| (r.y, r.height))
}

// ----- layout ---------------------------------------------------------

#[test]
fn at_80x24_the_cover_visualizer_and_text_share_the_height() {
    // 80x24 less the status row.
    let half = painter(HALF);
    let layout = screen_layout(area(80, 23), Some(&half), true);
    assert_eq!(rows(layout.header), Some((1, 1)));
    assert_eq!(layout.cover, Some(Rect::new(31, 3, 18, 9)), "centred");
    assert_eq!(
        rows(layout.title),
        Some((13, 1)),
        "a blank row on each side"
    );
    let visualizer = layout.visualizer.unwrap();
    assert_eq!((visualizer.y, visualizer.height), (15, 4));
    assert_eq!((visualizer.x, visualizer.width), (8, 64), "side margins");
    assert_eq!(rows(layout.timeline), Some((20, 1)));
    assert_eq!(rows(layout.next), Some((21, 1)));
    assert_eq!(layout.title.unwrap().x, 2);
    assert_eq!(layout.title.unwrap().width, 76);
}

#[test]
fn the_cover_grows_with_the_screen_and_the_visualizer_up_to_ten_rows() {
    let kitty = painter(GHOSTTY);
    // A 50-row Ghostty window: a fifth for the visualizer, the rest to
    // the cover, square-looking (9 × 19 px cells).
    let layout = screen_layout(area(160, 49), Some(&kitty), true);
    assert_eq!(layout.visualizer.unwrap().height, 9);
    assert_eq!(layout.cover.unwrap().height, 49 - 2 - 8 - 9);
    assert_eq!(layout.cover.unwrap().width, 63);
    let layout = screen_layout(area(200, 80), Some(&kitty), true);
    assert_eq!(layout.visualizer.unwrap().height, MAX_VISUALIZER_ROWS);
    assert_eq!(layout.cover.unwrap().height, 80 - 2 - 8 - 10);
    // Placeholders run out at 297 rows and columns.
    let layout = screen_layout(area(2000, 1000), Some(&kitty), false);
    assert!(layout.cover.unwrap().height <= DIACRITICS.len() as u16);
    assert!(layout.cover.unwrap().width <= DIACRITICS.len() as u16);
}

#[test]
fn a_cover_at_the_diacritics_limit_stays_square() {
    // 300 × 1000 with 16 × 34 px cells: tall enough for far more than 297
    // columns of square cover; the rows are cut so the columns fit.
    let kitty = ArtPainter::new(Graphics::Kitty { cell: (16, 34) }, 1);
    let layout = screen_layout(area(300, 1000), Some(&kitty), true);
    let cover = layout.cover.unwrap();
    assert!(cover.width <= DIACRITICS.len() as u16);
    assert_eq!((cover.width, cover.height), (295, 139));
    // Square on screen: width and height in pixels within a cell.
    let (width, height) = (u32::from(cover.width) * 16, u32::from(cover.height) * 34);
    assert!(width.abs_diff(height) <= 16, "{width} × {height}");
    // Just below the limit, and with half blocks.
    let half = painter(HALF);
    let layout = screen_layout(area(300, 1000), Some(&half), false);
    let cover = layout.cover.unwrap();
    assert_eq!(cover.width, cover.height * 2);
    assert!(cover.width <= DIACRITICS.len() as u16);
}

#[test]
fn without_the_visualizer_the_cover_takes_its_room() {
    let half = painter(HALF);
    let layout = screen_layout(area(80, 23), Some(&half), false);
    assert_eq!(layout.visualizer, None);
    assert_eq!(layout.cover.map(|c| (c.width, c.height)), Some((28, 14)));
    assert_eq!(rows(layout.timeline), Some((20, 1)));
}

#[test]
fn a_narrow_screen_narrows_the_cover() {
    let kitty = painter(GHOSTTY);
    let layout = screen_layout(area(30, 40), Some(&kitty), true);
    let cover = layout.cover.unwrap();
    assert_eq!((cover.width, cover.height), (25, 12));
    assert!(cover.width <= 30 - 4);
}

#[test]
fn without_a_cover_the_visualizer_takes_its_room_and_the_text_is_centred() {
    let layout = screen_layout(area(80, 23), None, true);
    assert_eq!(layout.cover, None);
    let visualizer = layout.visualizer.unwrap();
    assert_eq!(visualizer.height, 7);
    // 14 rows in all, centred in 23.
    assert_eq!(rows(layout.header), Some((4, 1)));
    assert_eq!(rows(layout.title), Some((6, 1)));
    assert_eq!(visualizer.y, 8);
    assert_eq!(rows(layout.timeline), Some((16, 1)));
    assert_eq!(rows(layout.next), Some((17, 1)));
    // Without either: just the text, centred.
    let layout = screen_layout(area(80, 23), None, false);
    assert_eq!(rows(layout.header), Some((8, 1)));
    assert_eq!(rows(layout.title), Some((10, 1)));
    assert_eq!(rows(layout.timeline), Some((12, 1)));
    assert_eq!(rows(layout.next), Some((13, 1)));
}

#[test]
fn small_screens_drop_the_cover_first_then_the_visualizer() {
    let half = painter(HALF);
    let parts = |height| {
        let layout = screen_layout(area(80, height), Some(&half), true);
        (
            layout.cover.map(|c| c.height),
            layout.visualizer.map(|v| v.height),
        )
    };
    assert_eq!(parts(23), (Some(9), Some(4)));
    assert_eq!(parts(20), (Some(6), Some(4)));
    // The smallest cover with the visualizer: 2 margins, 8 text rows, 6
    // and 3.
    assert_eq!(parts(19), (Some(6), Some(3)));
    assert_eq!(parts(18), (None, Some(6)), "the cover goes first");
    assert_eq!(parts(12), (None, Some(MIN_VISUALIZER_ROWS)));
    assert_eq!(parts(11), (None, None), "then the visualizer");
    // Without the visualizer the cover keeps a smaller screen.
    let layout = screen_layout(area(80, 15), Some(&half), false);
    assert_eq!(layout.cover.map(|c| c.height), Some(6));
    let layout = screen_layout(area(80, 14), Some(&half), false);
    assert_eq!(layout.cover, None);
    // Too narrow for the visualizer.
    let layout = screen_layout(area(VISUALIZER_MIN_SCREEN.0 - 1, 23), None, true);
    assert_eq!(layout.visualizer, None);
}

#[test]
fn the_text_and_timeline_always_fit() {
    let half = painter(HALF);
    for height in 0..=12 {
        for width in [1, 8, 20, 80] {
            let layout = screen_layout(area(width, height), Some(&half), true);
            let parts = [layout.header, layout.title, layout.timeline, layout.next];
            for part in parts.into_iter().flatten() {
                assert!(
                    part.bottom() <= height && part.right() <= width,
                    "{width}x{height}"
                );
            }
            if height >= 2 {
                assert!(layout.title.is_some() && layout.timeline.is_some());
            }
            if height >= 4 {
                assert!(parts.iter().all(Option::is_some), "{width}x{height}");
            }
        }
    }
    // Lower than the text with its blank rows: no blank rows, in order.
    let layout = screen_layout(area(80, 5), None, true);
    assert_eq!(rows(layout.header), Some((0, 1)));
    assert_eq!(rows(layout.title), Some((1, 1)));
    assert_eq!(rows(layout.timeline), Some((2, 1)));
    assert_eq!(rows(layout.next), Some((3, 1)));
    let layout = screen_layout(area(80, 2), None, true);
    assert_eq!((layout.header, layout.next), (None, None));
    assert_eq!(rows(layout.title), Some((0, 1)));
    assert_eq!(rows(layout.timeline), Some((1, 1)));
}

// ----- drawing --------------------------------------------------------

fn track(rel: &str, title: &str, artist: Option<&str>, album: Option<&str>) -> Track {
    Track::local(
        format!("/music/{rel}").into(),
        TrackMetadata {
            title: Some(title.into()),
            artist: artist.map(str::to_owned),
            album: album.map(str::to_owned),
            duration: Some(Duration::from_secs(100)),
            ..TrackMetadata::default()
        },
    )
}

/// An app with cover art on, playing the first of `tracks`, on the Now
/// playing screen.
fn playing(tracks: Vec<Track>, now: Instant) -> (App, LoadId) {
    let mut app = App::new("/music".into());
    app.set_cover_art(true);
    app.handle_scan_event(ScanEvent::Batch(tracks), now);
    // Home → Folders → the first track.
    app.handle_action(Action::Down, now);
    app.handle_action(Action::Down, now);
    app.handle_action(Action::Activate, now);
    let commands = app.handle_action(Action::Activate, now);
    let Some(PlaybackCommand::Load { load, .. }) = commands.first() else {
        panic!("{commands:?}");
    };
    let load = *load;
    app.handle_playback_event(PlaybackEvent::Started { load }, now);
    app.handle_action(Action::ToggleNowPlaying, now);
    app.follow_now_playing(now);
    (app, load)
}

fn two_tracks(now: Instant) -> (App, LoadId) {
    playing(
        vec![
            track("1.flac", "First", Some("Artist"), Some("Album")),
            track("2.flac", "Second", Some("Other"), None),
        ],
        now,
    )
}

fn answer_covers(app: &mut App, image: Arc<CoverImage>) {
    for request in app.take_cover_requests() {
        app.handle_cover_response(CoverResponse {
            id: request.id,
            generation: request.generation,
            key: request.key,
            size: request.size,
            outcome: Outcome::Art(image.clone()),
        });
    }
}

fn cover(side: u32) -> Arc<CoverImage> {
    Arc::new(CoverImage::new(side, side, [200, 40, 9].repeat((side * side) as usize)).unwrap())
}

fn render(app: &App, view: &mut ViewState, width: u16, height: u16, now: Instant) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    if let Some(painter) = view.art.as_mut() {
        let screen = Rect::new(0, 0, width, height);
        painter.prepare(crate::ui::render::art_frame(screen, app, painter));
        painter.take_output();
    }
    terminal.draw(|frame| draw(frame, app, view, now)).unwrap();
    terminal.backend().buffer().clone()
}

fn lines(buffer: &Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

fn line_with<'a>(lines: &'a [String], text: &str) -> (usize, &'a str) {
    lines
        .iter()
        .enumerate()
        .find(|(_, l)| l.contains(text))
        .map(|(i, l)| (i, l.as_str()))
        .unwrap_or_else(|| panic!("{text:?} not in {lines:#?}"))
}

/// The colour and modifiers of the first cell of `text` in `line`.
fn style_at(buffer: &Buffer, line: usize, text: &str) -> Style {
    let row: String = (0..buffer.area.width)
        .map(|x| buffer[(x, line as u16)].symbol())
        .collect();
    let byte = row.find(text).unwrap();
    let col = row[..byte].chars().count() as u16;
    let cell = &buffer[(col, line as u16)];
    let mut style = Style::new().add_modifier(cell.modifier);
    if cell.fg != Color::Reset {
        style = style.fg(cell.fg);
    }
    style
}

#[test]
fn the_screen_shows_the_track_in_place_of_the_panels() {
    let now = Instant::now();
    let (mut app, _) = two_tracks(now);
    answer_covers(&mut app, cover(256));
    let mut view = ViewState::default();
    view.art = Some(painter(HALF));
    let buffer = render(&app, &mut view, 80, 24, now);
    let lines = lines(&buffer);
    let text = lines.join("\n");
    assert!(!text.contains("Now Playing"), "no bar: {text}");
    assert!(!text.contains("Queue"), "no panels: {text}");
    let (header, line) = line_with(&lines, "Album · Artist");
    assert_eq!(header, 1);
    assert_eq!(line.trim().width(), "Album · Artist".width());
    assert_eq!(style_at(&buffer, header, "Album"), DIM);
    // The cover in half blocks, then the title, bold, with blank rows.
    assert!(lines[3].contains("▀▀▀▀"), "{text}");
    let (title, _) = line_with(&lines, "First");
    // The visualizer is off: the cover takes its 14 rows.
    assert!(lines[16].contains("▀▀▀▀"), "{text}");
    assert_eq!(title, 18);
    assert!(lines[17].trim().is_empty() && lines[19].trim().is_empty());
    assert!(
        style_at(&buffer, title, "First")
            .add_modifier
            .contains(Modifier::BOLD)
    );
    // The timeline, centred and bounded.
    let (timeline, line) = line_with(&lines, "0:00 ");
    assert_eq!(timeline, 20);
    assert!(line.contains(" 1:40"));
    assert!(line.contains('─'));
    assert!(line.trim().width() <= usize::from(MAX_TIMELINE_WIDTH));
    // The status row is still there.
    assert!(lines[23].contains("Playing Artist - First"), "{text}");
    // Nothing about the next track yet (50 s from the end of 1:40).
    assert!(!text.contains("Next:"), "{text}");
}

#[test]
fn the_header_shows_what_it_has() {
    let now = Instant::now();
    let header = |artist, album| {
        let (app, _) = playing(vec![track("1.flac", "T", artist, album)], now);
        let lines = lines(&render(&app, &mut ViewState::default(), 80, 24, now));
        lines[layout_for(area(80, 23), &app, None).header.unwrap().y as usize]
            .trim()
            .to_owned()
    };
    assert_eq!(header(Some("Ar"), Some("Al")), "Al · Ar");
    assert_eq!(header(Some("Ar"), None), "Ar");
    assert_eq!(header(None, Some("Al")), "Al");
    assert_eq!(header(None, None), "");
}

#[test]
fn long_names_are_cut_with_an_ellipsis() {
    let now = Instant::now();
    let long = "Very long title ".repeat(10);
    let (app, _) = playing(
        vec![track("1.flac", &long, Some(&"Artist ".repeat(20)), None)],
        now,
    );
    let lines = lines(&render(&app, &mut ViewState::default(), 40, 24, now));
    let (_, title) = line_with(&lines, "Very long title");
    assert!(title.trim_end().ends_with('…'), "{title}");
    assert!(title.width() <= 40);
    let (_, header) = line_with(&lines, "Artist Artist");
    assert!(header.trim_end().ends_with('…'));
}

#[test]
fn nothing_playing_says_so_and_nothing_else() {
    let now = Instant::now();
    let mut app = App::new("/music".into());
    app.handle_action(Action::ToggleNowPlaying, now);
    let mut view = ViewState::default();
    let buffer = render(&app, &mut view, 80, 24, now);
    let lines = lines(&buffer);
    let (row, _) = line_with(&lines, "Nothing playing");
    assert_eq!(row, 11);
    assert_eq!(style_at(&buffer, row, "Nothing"), DIM);
    let others: String = lines[..23]
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != row)
        .map(|(_, l)| l.trim())
        .collect();
    assert!(others.is_empty(), "{lines:#?}");
    assert_eq!(view.visualizer_area, None);
}

#[test]
fn up_next_names_the_next_track_near_the_end() {
    let now = Instant::now();
    let (mut app, _) = two_tracks(now);
    app.handle_playback_event(PlaybackEvent::Position(Duration::from_secs(85)), now);
    let buffer = render(&app, &mut ViewState::default(), 80, 24, now);
    let lines = lines(&buffer);
    let (row, _) = line_with(&lines, "Next: Second — Other");
    assert_eq!(style_at(&buffer, row, "Next"), DIM);
    // With repeat one it never shows.
    app.handle_action(Action::CycleRepeat, now);
    app.handle_action(Action::CycleRepeat, now);
    let lines = self::lines(&render(&app, &mut ViewState::default(), 80, 24, now));
    assert!(!lines.join("").contains("Next:"));
}

#[test]
fn a_queue_change_shows_up_next_until_the_tick_after_four_seconds() {
    let now = Instant::now();
    let (mut app, _) = two_tracks(now);
    let shows = |app: &App, at| {
        lines(&render(app, &mut ViewState::default(), 80, 24, at))
            .join("")
            .contains("Next: First")
    };
    // Repeat all: the first track follows the last.
    app.handle_playback_event(PlaybackEvent::Position(Duration::from_secs(10)), now);
    app.handle_action(Action::Next, now);
    app.follow_now_playing(now);
    assert!(!shows(&app, now), "nothing follows the last track");
    app.handle_action(Action::CycleRepeat, now);
    app.follow_now_playing(now);
    assert!(shows(&app, now));
    let later = now + crate::now_playing::UP_NEXT_FLASH;
    assert!(app.tick(later), "the tick that hides it redraws");
    assert!(!shows(&app, later));
}

#[test]
fn the_volume_and_repeat_sit_dim_beside_the_timeline() {
    let now = Instant::now();
    let (mut app, _) = two_tracks(now);
    app.handle_action(Action::VolumeUp, now);
    app.handle_playback_event(PlaybackEvent::Volume(55), now);
    app.handle_action(Action::CycleRepeat, now);
    let buffer = render(&app, &mut ViewState::default(), 100, 24, now);
    let lines = lines(&buffer);
    let (row, line) = line_with(&lines, "1:40");
    assert!(line.contains("1:40  ⟳  Vol 55%"), "{line}");
    assert_eq!(style_at(&buffer, row, "⟳"), DIM);
    assert_eq!(style_at(&buffer, row, "Vol"), DIM);
    // The timeline does not move when the volume goes.
    let start = line.find("0:00").unwrap();
    let gone = now + crate::now_playing::VOLUME_SHOWN;
    app.tick(gone);
    let lines = self::lines(&render(&app, &mut ViewState::default(), 100, 24, gone));
    let (_, line) = line_with(&lines, "1:40");
    assert!(!line.contains("Vol"), "{line}");
    assert!(line.contains("1:40  ⟳"));
    assert_eq!(line.find("0:00").unwrap(), start);
    // Repeat one.
    app.handle_action(Action::CycleRepeat, now);
    let lines = self::lines(&render(&app, &mut ViewState::default(), 100, 24, gone));
    assert!(line_with(&lines, "1:40").1.contains("⟳1"));
    assert_eq!("⟳1".width(), 2);
}

#[test]
fn pausing_dims_the_timeline() {
    let now = Instant::now();
    let (mut app, _) = two_tracks(now);
    app.handle_playback_event(PlaybackEvent::Position(Duration::from_secs(50)), now);
    let buffer = render(&app, &mut ViewState::default(), 80, 24, now);
    let (row, _) = line_with(&lines(&buffer), "0:50");
    assert_eq!(style_at(&buffer, row, "0:50"), Style::new());
    assert_eq!(style_at(&buffer, row, "━"), Style::new().fg(Color::Cyan));
    app.handle_playback_event(PlaybackEvent::Paused(true), now);
    let buffer = render(&app, &mut ViewState::default(), 80, 24, now);
    assert_eq!(style_at(&buffer, row, "0:50"), DIM);
    assert_eq!(style_at(&buffer, row, "━"), DIM);
    // The title stays as it was.
    let (title, _) = line_with(&lines(&buffer), "First");
    assert!(
        style_at(&buffer, title, "First")
            .add_modifier
            .contains(Modifier::BOLD)
    );
}

#[test]
fn a_stopped_track_shows_with_a_dim_timeline() {
    // A restored session: the queue's current entry, not playing.
    let now = Instant::now();
    let mut app = App::new("/music".into());
    app.restore_session(
        crate::session::Session {
            tracks: vec![track("1.flac", "Kept", Some("Artist"), None)],
            current: Some(0),
            ..crate::session::Session::default()
        },
        now,
    );
    app.handle_action(Action::ToggleNowPlaying, now);
    assert_eq!(
        app.player().status(),
        crate::player::PlaybackStatus::Stopped
    );
    let buffer = render(&app, &mut ViewState::default(), 80, 24, now);
    let lines = lines(&buffer);
    assert!(!lines.join("").contains("Nothing playing"));
    let (title, _) = line_with(&lines, "Kept");
    assert!(
        style_at(&buffer, title, "Kept")
            .add_modifier
            .contains(Modifier::BOLD)
    );
    let (row, _) = line_with(&lines, "1:40");
    assert_eq!(style_at(&buffer, row, "0:00"), DIM);
    assert_eq!(style_at(&buffer, row, "─"), DIM);
}

#[test]
fn up_next_never_names_the_track_heard() {
    // Repeat all with a single entry: it would name itself.
    let now = Instant::now();
    let (mut app, _) = playing(vec![track("1.flac", "Only", None, None)], now);
    app.handle_action(Action::CycleRepeat, now);
    assert_eq!(app.player().up_next().unwrap().track.title, "Only");
    app.handle_playback_event(PlaybackEvent::Position(Duration::from_secs(95)), now);
    app.follow_now_playing(now);
    let text = lines(&render(&app, &mut ViewState::default(), 80, 24, now)).join("");
    assert!(!text.contains("Next:"), "{text}");
}

#[test]
fn the_visualizer_is_wide_tall_dim_and_redrawn_on_its_own() {
    let now = Instant::now();
    let (mut app, load) = two_tracks(now);
    app.handle_action(Action::ToggleVisualizer, now);
    let frame = crate::playback::VisualizerFrame {
        levels: [1.0, 1.0],
        bands: [Some(1.0); crate::playback::VISUALIZER_BANDS],
    };
    let mut view = ViewState::default();
    let buffer = render(&app, &mut view, 120, 40, now);
    let area = view.visualizer_area.expect("redrawn on its own");
    assert!(view.visualizer_drawable);
    assert_eq!(area.height, 13, "no cover: a third of the screen");
    assert!(area.width >= 90 && area.x >= 2, "{area:?}");
    assert!(app.handle_visualizer(load, &frame, now));
    let mut last = buffer.clone();
    crate::ui::render::draw_visualizer(&mut last, &app, area);
    let bar = last[(area.x, area.bottom() - 1)].clone();
    assert_eq!(bar.symbol(), "█");
    assert_eq!(bar.fg, Color::DarkGray, "dim grey on the screen");
    // Matches a full redraw.
    let full = render(&app, &mut view, 120, 40, now);
    assert_eq!(full, last);
    // Help covers it: not redrawn on its own then.
    app.handle_action(Action::ToggleHelp, now);
    render(&app, &mut view, 120, 40, now);
    assert_eq!(view.visualizer_area, None);
    assert!(!view.visualizer_drawable);
}

#[test]
fn the_screen_cover_is_the_large_one_once_it_has_loaded() {
    let now = Instant::now();
    let (mut app, _) = two_tracks(now);
    let painter = painter(GHOSTTY);
    let screen = Rect::new(0, 0, 160, 50);
    // Small first, shown meanwhile.
    let small = app.take_cover_requests();
    assert_eq!(small.len(), 1);
    assert_eq!(small[0].size, crate::cover::CoverSize::Small);
    app.handle_cover_response(CoverResponse {
        id: small[0].id,
        generation: small[0].generation,
        key: small[0].key.clone(),
        size: small[0].size,
        outcome: Outcome::Art(cover(256)),
    });
    let frame = crate::ui::render::art_frame(screen, &app, &painter).unwrap();
    assert_eq!(frame.image.width(), 256);
    // The visualizer is off: the cover takes its room.
    assert_eq!((frame.cols, frame.rows), (84, 40));
    // Then the large one, once a frame found it useful.
    app.set_large_cover_useful(true);
    answer_covers(&mut app, cover(1024));
    let frame = crate::ui::render::art_frame(screen, &app, &painter).unwrap();
    assert_eq!(frame.image.width(), 1024);
    // Back in the library the bar keeps the small one.
    app.handle_action(Action::ToggleNowPlaying, now);
    let frame = crate::ui::render::art_frame(screen, &app, &painter).unwrap();
    assert_eq!(frame.image.width(), 256);
    assert!(app.take_cover_requests().is_empty());
}

// ----- keys -----------------------------------------------------------

#[test]
fn list_actions_do_nothing_on_the_screen() {
    let now = Instant::now();
    let (mut app, _) = two_tracks(now);
    let selected = app.library_selected();
    let queued = app.player().queue().len();
    for action in [
        Action::Down,
        Action::Up,
        Action::Activate,
        Action::Enqueue,
        Action::EnqueueNext,
        Action::Remove,
        Action::ClearQueue,
        Action::OpenSearch,
        Action::SwitchFocus,
        Action::Parent,
        Action::Back,
    ] {
        assert!(app.handle_action(action, now).is_empty(), "{action:?}");
    }
    assert_eq!(app.library_selected(), selected);
    assert_eq!(app.player().queue().len(), queued);
    assert_eq!(app.focus(), Focus::Library);
    assert!(app.local_input().is_none());
    assert!(app.now_playing_open());
}

#[test]
fn global_keys_work_and_the_source_keys_go_back_first() {
    let now = Instant::now();
    let (mut app, _) = two_tracks(now);
    assert!(!app.handle_action(Action::TogglePause, now).is_empty());
    assert!(!app.handle_action(Action::Next, now).is_empty());
    app.handle_action(Action::ToggleHelp, now);
    assert!(app.help_open());
    // Esc (CloseHelp) closes help, not the screen.
    app.handle_action(Action::CloseHelp, now);
    assert!(app.now_playing_open());
    app.handle_action(Action::SwitchFocus, now);
    assert_eq!(app.focus(), Focus::Library, "Tab does nothing here");
    app.handle_action(Action::SelectNavidrome, now);
    assert!(!app.now_playing_open());
    assert_eq!(app.source(), crate::app::Source::Navidrome);
    // `f` from the queue panel opens it; `f` closes it again.
    app.handle_action(Action::SwitchFocus, now);
    app.handle_action(Action::ToggleNowPlaying, now);
    assert!(app.now_playing_open());
    app.handle_action(Action::ToggleNowPlaying, now);
    assert!(!app.now_playing_open());
    assert_eq!(app.focus(), Focus::Queue);
    app.handle_action(Action::ToggleNowPlaying, now);
    app.handle_action(Action::SelectLocal, now);
    assert!(!app.now_playing_open());
    assert_eq!(app.source(), crate::app::Source::Local);
}
