use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;

use super::*;
use crate::app::{Action, Focus};
use crate::ui::cover::Graphics;
use crate::ui::render::{ViewState, draw};
use coda_core::cover::{CoverImage, CoverResponse, Outcome};
use coda_core::library::scan::ScanEvent;
use coda_core::model::{Track, TrackMetadata};
use coda_core::playback::{LoadId, PlaybackCommand, PlaybackEvent};

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
fn side_by_side_at_120x36_the_cover_and_column_form_a_centred_block() {
    // 120x36 less the status row; half blocks.
    let half = painter(HALF);
    let layout = screen_layout(area(120, 35), Some(&half), true);
    assert!(layout.side_by_side);
    // 55 % of 35 rows; square in half blocks: 19 rows, 38 columns.
    assert_eq!(layout.cover, Some(Rect::new(7, 8, 38, 19)));
    // The column: 4 columns on, 64 wide, centred against the cover.
    let column = |rect: Option<Rect>| rect.map(|r| (r.x, r.y, r.width, r.height));
    assert_eq!(column(layout.title), Some((49, 10, 64, 1)));
    assert_eq!(column(layout.header), Some((49, 11, 64, 1)));
    assert_eq!(column(layout.timeline), Some((49, 13, 64, 1)));
    assert_eq!(column(layout.visualizer), Some((49, 15, 64, 8)));
    assert_eq!(column(layout.next), Some((49, 24, 64, 1)));
    // The block (7..113) is centred in 120 columns.
    assert_eq!(7 + 38 + COLUMN_GAP + 64 + 7, 120);
    // Without the visualizer the column is shorter, still centred.
    let layout = screen_layout(area(120, 35), Some(&half), false);
    assert_eq!(layout.visualizer, None);
    assert_eq!(rows(layout.title), Some((14, 1)));
    assert_eq!(rows(layout.next), Some((19, 1)));
}

#[test]
fn side_by_side_shrinks_the_cover_before_stacking() {
    let half = painter(HALF);
    // 35 rows: a 19-row cover (55 %) and 38 columns, so 2 + 38 + 4 + 30
    // + 2 = 76 columns hold it at full size.
    let layout = |width| screen_layout(area(width, 35), Some(&half), true);
    let cover = |width| layout(width).cover.map(|c| (c.width, c.height));
    assert!(layout(76).side_by_side);
    assert_eq!(cover(76), Some((38, 19)));
    assert_eq!(layout(76).title.unwrap().width, MIN_COLUMN);
    // Narrower: the cover shrinks so the column keeps 30 columns...
    assert!(layout(75).side_by_side);
    assert_eq!(cover(75), Some((36, 18)));
    assert_eq!(
        layout(75).title.unwrap().width,
        31,
        "half blocks: even widths"
    );
    // ...down to 35 % of the height (12 rows), with the bars beside it.
    assert!(layout(62).side_by_side);
    assert_eq!(cover(62), Some((24, 12)));
    assert_eq!(layout(62).visualizer.map(|v| v.height), Some(5));
    // Then it is stacked.
    assert!(!layout(61).side_by_side, "narrower: stacked");
    assert!(layout(61).cover.is_some());
    // The column stops growing at 64.
    assert_eq!(layout(400).title.unwrap().width, MAX_COLUMN);
    // Kitty in a 50-row Ghostty window (9 × 19 px cells).
    let kitty = painter(GHOSTTY);
    let layout = screen_layout(area(160, 49), Some(&kitty), true);
    assert!(layout.side_by_side);
    assert_eq!(layout.cover.map(|c| (c.width, c.height)), Some((55, 26)));
    assert_eq!(
        layout.visualizer.unwrap().height,
        MAX_COLUMN_VISUALIZER_ROWS
    );
    assert_eq!(
        layout.visualizer.unwrap().width,
        layout.timeline.unwrap().width
    );
    // 80x30 with 16 × 34 px cells: a 15-row cover beside a 40-column
    // column.
    let kitty = ArtPainter::new(Graphics::Kitty { cell: (16, 34) }, 1);
    let layout = screen_layout(area(80, 29), Some(&kitty), true);
    assert!(layout.side_by_side);
    assert_eq!(layout.cover.map(|c| (c.width, c.height)), Some((32, 15)));
    assert_eq!(layout.title.unwrap().width, 40);
    assert_eq!(layout.visualizer.map(|v| v.height), Some(8));
}

#[test]
fn tall_narrow_and_low_windows_are_stacked() {
    let half = painter(HALF);
    // 60x40: a 21-row cover would need 80 columns beside its column.
    let layout = screen_layout(area(60, 39), Some(&half), true);
    assert!(!layout.side_by_side);
    assert_eq!(layout.cover, Some(Rect::new(7, 1, 46, 23)));
    assert_eq!(rows(layout.title), Some((25, 1)));
    assert_eq!(rows(layout.header), Some((26, 1)), "under the title");
    assert_eq!(rows(layout.visualizer), Some((28, 7)));
    assert_eq!(rows(layout.timeline), Some((36, 1)));
    assert_eq!(rows(layout.next), Some((37, 1)));
    // Wide but low: 55 % of 14 rows is a 7-row cover, beside a column
    // without the visualizer (it needs 7 + 3 rows).
    let layout = screen_layout(area(200, 14), Some(&half), true);
    assert!(layout.side_by_side);
    assert_eq!(layout.cover.unwrap().height, 7);
    assert_eq!(layout.visualizer, None);
    // 10 rows: under 6 for the cover side by side; stacked, the text alone.
    let layout = screen_layout(area(200, 10), Some(&half), true);
    assert!(!layout.side_by_side);
    assert_eq!((layout.cover, layout.visualizer), (None, None));
}

#[test]
fn a_cover_at_the_diacritics_limit_stays_square() {
    // 300 × 1000 with 16 × 34 px cells: tall enough for far more than 297
    // columns of square cover; the rows are cut so the columns fit.
    let kitty = ArtPainter::new(Graphics::Kitty { cell: (16, 34) }, 1);
    let layout = screen_layout(area(300, 1000), Some(&kitty), true);
    assert!(!layout.side_by_side);
    let cover = layout.cover.unwrap();
    assert!(cover.width <= DIACRITICS.len() as u16);
    assert_eq!((cover.width, cover.height), (295, 139));
    // Square on screen: width and height in pixels within a cell.
    let (width, height) = (u32::from(cover.width) * 16, u32::from(cover.height) * 34);
    assert!(width.abs_diff(height) <= 16, "{width} × {height}");
    // Side by side at the limit too.
    let layout = screen_layout(area(2000, 1000), Some(&kitty), true);
    assert!(layout.side_by_side);
    assert_eq!(layout.cover.map(|c| (c.width, c.height)), Some((295, 139)));
    // And with half blocks.
    let half = painter(HALF);
    let layout = screen_layout(area(300, 1000), Some(&half), false);
    let cover = layout.cover.unwrap();
    assert_eq!(cover.width, cover.height * 2);
    assert!(cover.width <= DIACRITICS.len() as u16);
}

#[test]
fn stacked_without_the_visualizer_the_cover_takes_its_room() {
    let half = painter(HALF);
    let layout = screen_layout(area(40, 23), Some(&half), false);
    assert!(!layout.side_by_side);
    assert_eq!(layout.visualizer, None);
    assert_eq!(layout.cover.map(|c| (c.width, c.height)), Some((30, 15)));
    assert_eq!(rows(layout.timeline), Some((20, 1)));
}

#[test]
fn a_narrow_stacked_screen_narrows_the_cover() {
    let kitty = painter(GHOSTTY);
    let layout = screen_layout(area(30, 40), Some(&kitty), true);
    let cover = layout.cover.unwrap();
    assert_eq!((cover.width, cover.height), (25, 12));
    assert!(cover.width <= 30 - 4);
}

#[test]
fn without_a_cover_the_visualizer_takes_its_room_and_the_text_is_centred() {
    let layout = screen_layout(area(80, 23), None, true);
    assert!(!layout.side_by_side);
    assert_eq!(layout.cover, None);
    let visualizer = layout.visualizer.unwrap();
    assert_eq!(visualizer.height, 7);
    // 13 rows in all, centred in 23.
    assert_eq!(rows(layout.title), Some((5, 1)));
    assert_eq!(rows(layout.header), Some((6, 1)));
    assert_eq!(visualizer.y, 8);
    assert_eq!(rows(layout.timeline), Some((16, 1)));
    assert_eq!(rows(layout.next), Some((17, 1)));
    // Without either: just the text, centred.
    let layout = screen_layout(area(80, 23), None, false);
    assert_eq!(rows(layout.title), Some((9, 1)));
    assert_eq!(rows(layout.header), Some((10, 1)));
    assert_eq!(rows(layout.timeline), Some((12, 1)));
    assert_eq!(rows(layout.next), Some((13, 1)));
}

#[test]
fn small_stacked_screens_drop_the_cover_first_then_the_visualizer() {
    let half = painter(HALF);
    // 40 columns: never wide enough for side by side.
    let parts = |height| {
        let layout = screen_layout(area(40, height), Some(&half), true);
        assert!(!layout.side_by_side);
        (
            layout.cover.map(|c| c.height),
            layout.visualizer.map(|v| v.height),
        )
    };
    assert_eq!(parts(23), (Some(10), Some(4)));
    assert_eq!(parts(18), (Some(6), Some(3)));
    assert_eq!(parts(17), (None, Some(5)), "the cover goes first");
    assert_eq!(parts(11), (None, Some(MIN_VISUALIZER_ROWS)));
    assert_eq!(parts(10), (None, None), "then the visualizer");
    // Without the visualizer the cover keeps a smaller screen.
    let layout = screen_layout(area(40, 14), Some(&half), false);
    assert_eq!(layout.cover.map(|c| c.height), Some(6));
    let layout = screen_layout(area(40, 13), Some(&half), false);
    assert_eq!(layout.cover, None);
    // Too narrow for the visualizer.
    let layout = screen_layout(area(VISUALIZER_MIN_SCREEN.0 - 1, 23), None, true);
    assert_eq!(layout.visualizer, None);
}

#[test]
fn the_text_and_timeline_always_fit() {
    let half = painter(HALF);
    for height in 0..=40 {
        for width in [1, 8, 20, 40, 80, 120] {
            for visualizer in [false, true] {
                let layout = screen_layout(area(width, height), Some(&half), visualizer);
                let parts = [
                    layout.header,
                    layout.title,
                    layout.timeline,
                    layout.next,
                    layout.cover,
                    layout.visualizer,
                ];
                for part in parts.into_iter().flatten() {
                    assert!(
                        part.bottom() <= height && part.right() <= width,
                        "{width}x{height} {part:?}"
                    );
                }
                if height >= 2 {
                    assert!(layout.title.is_some() && layout.timeline.is_some());
                }
                if height >= 4 {
                    assert!(parts[..4].iter().all(Option::is_some), "{width}x{height}");
                }
            }
        }
    }
    // Lower than the text with its blank row: no blank row, in order.
    let layout = screen_layout(area(80, 4), None, true);
    assert_eq!(rows(layout.title), Some((0, 1)));
    assert_eq!(rows(layout.header), Some((1, 1)));
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
    // Side by side: a 12-row cover (55 % of 23) at the left of a centred
    // block, a 48-column text column 4 columns to its right.
    for line in &lines[5..17] {
        let cover: String = line.chars().skip(2).take(24).collect();
        assert_eq!(cover, "▀".repeat(24), "{text}");
    }
    assert!(lines[4].trim().is_empty() && lines[17].trim().is_empty());
    let column = |row: usize| lines[row].chars().skip(30).collect::<String>();
    let (title, _) = line_with(&lines, "First");
    assert_eq!(title, 8);
    assert!(column(8).starts_with("First"), "left-aligned: {text}");
    assert!(
        style_at(&buffer, title, "First")
            .add_modifier
            .contains(Modifier::BOLD)
    );
    let (header, _) = line_with(&lines, "Album · Artist");
    assert_eq!(header, 9, "under the title");
    assert!(column(9).starts_with("Album · Artist"));
    assert_eq!(style_at(&buffer, header, "Album"), DIM);
    assert!(column(10).trim().is_empty());
    // The timeline fills the column.
    let (timeline, _) = line_with(&lines, "0:00 ");
    assert_eq!(timeline, 11);
    assert!(column(11).starts_with("0:00 ─"));
    assert_eq!(column(11).trim_end().width(), 48);
    assert!(column(11).trim_end().ends_with(" 1:40"));
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
    let later = now + coda_core::now_playing::UP_NEXT_FLASH;
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
    let gone = now + coda_core::now_playing::VOLUME_SHOWN;
    app.tick(gone);
    let lines = self::lines(&render(&app, &mut ViewState::default(), 100, 24, gone));
    let (_, line) = line_with(&lines, "1:40");
    assert!(!line.contains("Vol"), "{line}");
    assert!(line.contains("1:40  ⟳"));
    assert_eq!(line.find("0:00").unwrap(), start);
    // Repeat one.
    app.handle_action(Action::CycleRepeat, now);
    let lines = self::lines(&render(&app, &mut ViewState::default(), 100, 24, gone));
    assert!(line_with(&lines, "1:40").1.contains("1:40  ⟳ 1"));
    assert_eq!("⟳ 1".width(), 3);
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
        coda_core::session::Session {
            tracks: vec![track("1.flac", "Kept", Some("Artist"), None)],
            current: Some(0),
            ..coda_core::session::Session::default()
        },
        now,
    );
    app.handle_action(Action::ToggleNowPlaying, now);
    assert_eq!(
        app.player().status(),
        coda_core::player::PlaybackStatus::Stopped
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
    let frame = coda_core::playback::VisualizerFrame {
        levels: [1.0, 1.0],
        bands: [Some(1.0); coda_core::playback::VISUALIZER_BANDS],
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

/// Two tracks playing on the screen with a cover and the visualizer on,
/// every band at full level.
fn loud_screen(now: Instant) -> App {
    let (mut app, load) = two_tracks(now);
    answer_covers(&mut app, cover(256));
    app.handle_action(Action::ToggleVisualizer, now);
    let frame = coda_core::playback::VisualizerFrame {
        levels: [1.0, 1.0],
        bands: [Some(1.0); coda_core::playback::VISUALIZER_BANDS],
    };
    assert!(app.handle_visualizer(load, &frame, now));
    app
}

#[test]
fn the_screens_visualizer_is_bars_only_as_wide_as_the_timeline() {
    let now = Instant::now();
    let app = loud_screen(now);
    let mut view = ViewState::default();
    view.art = Some(painter(HALF));
    let buffer = render(&app, &mut view, 120, 36, now);
    let area = view.visualizer_area.expect("drawn");
    let layout = layout_for(Rect::new(0, 0, 120, 35), &app, view.art.as_ref());
    assert!(layout.side_by_side);
    assert_eq!(Some(area), layout.visualizer);
    assert_eq!((area.x, area.width), (49, 64));
    assert_eq!(area.width, layout.timeline.unwrap().width);
    // Full bars from the left edge to the right edge, no L/R meter.
    let bottom = area.bottom() - 1;
    let row: String = (area.x..area.right())
        .map(|x| buffer[(x, bottom)].symbol())
        .collect();
    assert!(row.starts_with('█') && row.ends_with('█'), "{row}");
    assert!(!row.contains('L') && !row.contains('R') && !row.contains('▏'));
    assert_eq!(buffer[(area.x, bottom)].fg, Color::DarkGray);
    // A visualizer-only redraw covers exactly that area and matches.
    let mut last = buffer.clone();
    crate::ui::render::draw_visualizer(&mut last, &app, area);
    assert_eq!(last, buffer);
}

/// Draws `app` at `width` × `height` with half blocks and checks the
/// visualizer's full bars run from its left edge to its right edge.
fn assert_bars_fill(app: &App, width: u16, height: u16, side_by_side: bool, now: Instant) -> Rect {
    let mut view = ViewState::default();
    view.art = Some(painter(HALF));
    let buffer = render(app, &mut view, width, height, now);
    let screen = Rect::new(0, 0, width, height - 1);
    assert_eq!(
        layout_for(screen, app, view.art.as_ref()).side_by_side,
        side_by_side,
        "{width}x{height}"
    );
    let area = view.visualizer_area.expect("drawn");
    let bottom = area.bottom() - 1;
    let row: String = (area.x..area.right())
        .map(|x| buffer[(x, bottom)].symbol())
        .collect();
    assert!(
        row.starts_with('█') && row.ends_with('█'),
        "{width}x{height}: {row:?}"
    );
    area
}

#[test]
fn narrow_visualizers_still_fill_their_width() {
    let now = Instant::now();
    let app = loud_screen(now);
    // Side by side with 30- and 41-column text columns.
    assert_eq!(assert_bars_fill(&app, 76, 36, true, now).width, 30);
    assert_eq!(assert_bars_fill(&app, 87, 36, true, now).width, 41);
    // A 40-column stacked screen.
    let area = assert_bars_fill(&app, 40, 30, false, now);
    assert!(area.width < 42, "{area:?}");
}

#[test]
fn the_visualizer_is_drawable_only_where_the_layout_draws_it() {
    let now = Instant::now();
    let (mut app, _) = two_tracks(now);
    answer_covers(&mut app, cover(256));
    let mut view = ViewState::default();
    view.art = Some(painter(HALF));
    // 80x14: side by side with a 7-row cover, too low for the bars.
    render(&app, &mut view, 80, 14, now);
    let screen = Rect::new(0, 0, 80, 13);
    let layout = screen_layout(screen, view.art.as_ref(), true);
    assert!(layout.side_by_side && layout.visualizer.is_none());
    assert!(!view.visualizer_drawable);
    // Without a cover the same window stacks and has room.
    render(&app, &mut ViewState::default(), 80, 14, now);
    let mut plain = ViewState::default();
    render(&app, &mut plain, 80, 14, now);
    assert!(plain.visualizer_drawable);
    // With a taller window the bars fit beside the cover.
    render(&app, &mut view, 80, 24, now);
    assert!(view.visualizer_drawable);
}

/// The column (side by side, half blocks) at `width` × 36 for `app`:
/// the text from the column's left edge on the album and timeline rows.
fn column_rows(app: &App, width: u16, now: Instant) -> (String, String, Rect) {
    let mut view = ViewState::default();
    view.art = Some(painter(HALF));
    let buffer = render(app, &mut view, width, 36, now);
    let layout = layout_for(Rect::new(0, 0, width, 35), app, view.art.as_ref());
    assert!(layout.side_by_side);
    let text = |rect: Rect| -> String {
        (rect.x..rect.right())
            .map(|x| buffer[(x, rect.y)].symbol())
            .collect()
    };
    let header = layout.header.unwrap();
    (text(header), text(layout.timeline.unwrap()), header)
}

fn with_extras(app: &mut App, volume: u8, now: Instant) {
    // Repeat one, and the volume just changed.
    app.handle_action(Action::CycleRepeat, now);
    app.handle_action(Action::CycleRepeat, now);
    app.handle_action(Action::VolumeUp, now);
    app.handle_playback_event(PlaybackEvent::Volume(volume), now);
}

#[test]
fn side_by_side_the_extras_sit_on_the_album_row_and_the_timeline_keeps_its_width() {
    let now = Instant::now();
    let (mut app, _) = two_tracks(now);
    answer_covers(&mut app, cover(256));
    let (header, timeline, _) = column_rows(&app, 120, now);
    assert_eq!(header.trim_end(), "Album · Artist");
    assert!(timeline.starts_with("0:00 ─") && timeline.ends_with(" 1:40"));
    assert_eq!(timeline.width(), 64, "the whole column");
    with_extras(&mut app, 55, now);
    let (header, with, rect) = column_rows(&app, 120, now);
    assert_eq!(with, timeline, "the timeline does not move");
    // Right-aligned to the column's edge, above the end of the timeline.
    assert!(header.starts_with("Album · Artist"));
    assert!(header.ends_with("  ⟳ 1  Vol 55%"), "{header:?}");
    let mut view = ViewState::default();
    view.art = Some(painter(HALF));
    let buffer = render(&app, &mut view, 120, 36, now);
    assert_eq!(buffer[(rect.right() - 1, rect.y)].symbol(), "%");
    assert!(
        buffer[(rect.right() - 1, rect.y)]
            .modifier
            .contains(Modifier::DIM)
    );
    assert_eq!(buffer[(rect.right() - 12, rect.y)].symbol(), "⟳");
}

#[test]
fn a_long_album_row_is_cut_before_the_extras() {
    let now = Instant::now();
    let (mut app, _) = playing(
        vec![track(
            "1.flac",
            "T",
            Some(&"Artist ".repeat(10)),
            Some(&"Album ".repeat(10)),
        )],
        now,
    );
    answer_covers(&mut app, cover(256));
    // No extras: the whole column.
    let (header, ..) = column_rows(&app, 120, now);
    assert_eq!(header.width(), 64);
    assert!(header.ends_with('…'));
    with_extras(&mut app, 55, now);
    let (header, ..) = column_rows(&app, 120, now);
    assert!(header.ends_with("…  ⟳ 1  Vol 55%"), "{header:?}");
}

#[test]
fn a_narrow_column_drops_the_repeat_mark_then_the_volume() {
    let now = Instant::now();
    let long = |volume| {
        let (mut app, _) = playing(
            vec![track(
                "1.flac",
                "T",
                Some("Some Artist"),
                Some("A Long Album"),
            )],
            now,
        );
        answer_covers(&mut app, cover(256));
        with_extras(&mut app, volume, now);
        app
    };
    // 76x36: a 30-column column. `⟳ 1  Vol 55%` and 16 columns of album
    // · artist just fit.
    let (header, timeline, rect) = column_rows(&long(55), 76, now);
    assert_eq!(rect.width, 30);
    assert!(header.ends_with("…  ⟳ 1  Vol 55%"), "{header:?}");
    assert_eq!(timeline.width(), 30);
    // `Vol 100%` is one wider: the repeat mark goes.
    let (header, ..) = column_rows(&long(100), 76, now);
    assert!(
        header.ends_with("  Vol 100%") && !header.contains('⟳'),
        "{header:?}"
    );
    // A short album · artist keeps everything.
    let (mut app, _) = playing(vec![track("1.flac", "T", Some("Ar"), Some("Al"))], now);
    answer_covers(&mut app, cover(256));
    with_extras(&mut app, 100, now);
    let (header, ..) = column_rows(&app, 76, now);
    assert!(header.starts_with("Al · Ar") && header.ends_with("⟳ 1  Vol 100%"));
}

#[test]
fn without_album_or_artist_the_extras_still_show() {
    let now = Instant::now();
    let (mut app, _) = playing(vec![track("1.flac", "T", None, None)], now);
    answer_covers(&mut app, cover(256));
    with_extras(&mut app, 55, now);
    let (header, ..) = column_rows(&app, 120, now);
    assert_eq!(header.trim_start(), "⟳ 1  Vol 55%");
    assert_eq!(header.width(), 64);
}

#[test]
fn the_bars_visualizer_keeps_its_meter() {
    let now = Instant::now();
    let mut app = loud_screen(now);
    app.handle_action(Action::ToggleNowPlaying, now);
    let mut view = ViewState::default();
    let buffer = render(&app, &mut view, 120, 36, now);
    let area = view.visualizer_area.expect("drawn in the bar");
    let text: String = (area.y..area.bottom())
        .flat_map(|y| (area.x..area.right()).map(move |x| (x, y)))
        .map(|p| buffer[p].symbol().to_owned())
        .collect();
    assert!(text.contains('L') && text.contains('R'));
}

#[test]
fn sketches_of_both_layouts() {
    // As the report shows them: half blocks at 120x36; Kitty with 16 × 34
    // px cells at 80x30 (placeholder cells shown as `#`).
    let now = Instant::now();
    let mut app = loud_screen(now);
    with_extras(&mut app, 55, now);
    let mut view = ViewState::default();
    view.art = Some(painter(HALF));
    let wide = lines(&render(&app, &mut view, 120, 36, now));
    assert!(wide[11].ends_with("⟳ 1  Vol 55%       "), "{:?}", wide[11]);
    let mut kitty = ViewState::default();
    kitty.art = Some(ArtPainter::new(Graphics::Kitty { cell: (16, 34) }, 1));
    let small: Vec<String> = lines(&render(&app, &mut kitty, 80, 30, now))
        .into_iter()
        .map(|line| {
            line.chars()
                .map(|c| {
                    if c == crate::ui::cover::PLACEHOLDER {
                        '#'
                    } else {
                        c
                    }
                })
                .filter(|c| unicode_width::UnicodeWidthChar::width(*c) != Some(0))
                .collect()
        })
        .collect();
    if std::env::var_os("CODA_PRINT_SKETCH").is_some() {
        eprintln!("{}\n\n{}", wide.join("\n"), small.join("\n"));
    }
    assert!(wide[10].contains("First") && wide[11].contains("Album · Artist"));
    let (title, line) = line_with(&small, "First");
    assert!(
        line.starts_with(&format!("{}{}", " ".repeat(2), "#".repeat(32))),
        "{line}"
    );
    assert!(small[title + 1].contains("Album · Artist"));
    assert_eq!(small[title].find("First"), small[title + 1].find("Album"));
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
    assert_eq!(small[0].size, coda_core::cover::CoverSize::Small);
    app.handle_cover_response(CoverResponse {
        id: small[0].id,
        generation: small[0].generation,
        key: small[0].key.clone(),
        size: small[0].size,
        outcome: Outcome::Art(cover(256)),
    });
    let frame = crate::ui::render::art_frame(screen, &app, &painter).unwrap();
    assert_eq!(frame.image.width(), 256);
    // Side by side: 55 % of 49 rows.
    assert_eq!((frame.cols, frame.rows), (55, 26));
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
