//! Draws the [`App`] with Ratatui: Library and Queue panels, the
//! now-playing bar, the status line, and the help overlay.
//!
//! Untrusted names (tags, file names) are sanitised and truncated by
//! display width, so control characters and wide characters cannot break
//! the layout.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, LineGauge, List, ListItem, ListState, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus, LibraryRow, ScanState, count, thousands};
use crate::player::PlaybackStatus;
use crate::ui::help;

/// Below this width only the focused panel is shown.
pub const NARROW_WIDTH: u16 = 60;
const HIGHLIGHT_SYMBOL: &str = "> ";
const NOW_PLAYING_HEIGHT: u16 = 4;

/// Widget state that persists between frames: the first visible row of
/// each panel, so the view does not jump between frames.
#[derive(Debug, Default)]
pub struct ViewState {
    library_offset: usize,
    queue_offset: usize,
}

/// The first visible row for a list of `len` rows in a viewport of
/// `height` rows, starting from the previous `offset`.
///
/// Scrolls only as far as needed to keep `selected` visible, and keeps the
/// result valid when the list shrinks or the viewport changes: the window
/// never starts past the last page, and an empty list or zero height gives
/// 0. (With a height of 1 the selection is the one visible row.)
fn scroll_offset(offset: usize, selected: usize, len: usize, height: usize) -> usize {
    if len == 0 || height == 0 {
        return 0;
    }
    let selected = selected.min(len - 1);
    let mut offset = offset.min(len - height.min(len));
    if selected < offset {
        offset = selected;
    } else if selected >= offset + height {
        offset = selected + 1 - height;
    }
    offset
}

#[cfg(test)]
thread_local! {
    /// Rows built by the panels, to check work scales with the viewport.
    static ROWS_BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn note_row_built() {
    #[cfg(test)]
    ROWS_BUILT.with(|n| n.set(n.get() + 1));
}

pub fn draw(frame: &mut Frame, app: &App, view: &mut ViewState, now: Instant) {
    let [panels, now_playing, status] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(NOW_PLAYING_HEIGHT),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    if panels.width < NARROW_WIDTH {
        match app.focus() {
            Focus::Library => draw_library(frame, app, view, panels, true),
            Focus::Queue => draw_queue(frame, app, view, panels, true),
        }
    } else {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(panels);
        draw_library(frame, app, view, left, app.focus() == Focus::Library);
        draw_queue(frame, app, view, right, app.focus() == Focus::Queue);
    }
    draw_now_playing(frame, app, now_playing);
    draw_status(frame, app, status, now);
    if app.help_open() {
        help::draw(frame, frame.area());
    }
}

fn panel_block(title: String, focused: bool, width: u16) -> Block<'static> {
    let style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let title = fit(&title, usize::from(width.saturating_sub(4)));
    Block::bordered().border_style(style).title(Span::styled(
        format!(" {title} "),
        Style::default().add_modifier(Modifier::BOLD),
    ))
}

fn list_widget<'a>(items: Vec<ListItem<'a>>, block: Block<'a>, focused: bool) -> List<'a> {
    let highlight = if focused {
        Style::default().bg(Color::Cyan).fg(Color::Black)
    } else {
        Style::default().add_modifier(Modifier::REVERSED)
    };
    List::new(items)
        .block(block)
        .highlight_symbol(HIGHLIGHT_SYMBOL)
        .highlight_style(highlight)
}

/// Inner width available for row text (borders and highlight symbol).
fn row_width(area: Rect) -> usize {
    usize::from(area.width.saturating_sub(2)).saturating_sub(HIGHLIGHT_SYMBOL.width())
}

fn draw_library(frame: &mut Frame, app: &App, view: &mut ViewState, area: Rect, focused: bool) {
    let location = if app.library_path().is_empty() {
        "Library".to_owned()
    } else {
        format!("Library: {}", app.library_path().join("/"))
    };
    let block = panel_block(location, focused, area.width);

    if app.tree().is_empty() {
        let message = match app.scan_state() {
            ScanState::Scanning { .. } => "Scanning…".to_owned(),
            _ => format!("No music found in {}", app.music_dir().to_string_lossy()),
        };
        let inner = usize::from(area.width.saturating_sub(2));
        frame.render_widget(Paragraph::new(fit(&message, inner)).block(block), area);
        return;
    }

    let width = row_width(area);
    let height = usize::from(block.inner(area).height);
    let offset = scroll_offset(
        view.library_offset,
        app.library_selected(),
        app.library_row_count(),
        height,
    );
    view.library_offset = offset;
    let items: Vec<ListItem> = app
        .library_rows_window(offset, height)
        .into_iter()
        .map(|row| {
            note_row_built();
            match row {
                LibraryRow::Dir(dir) => ListItem::new(Line::styled(
                    fit(&format!("{}/", dir.name()), width),
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                )),
                LibraryRow::Track(track) => {
                    let left = match track.track_no {
                        Some(no) => format!("{no:02} {}", track.title),
                        None => track.title.clone(),
                    };
                    ListItem::new(row_line(&left, track.duration.map(format_time), width))
                }
            }
        })
        .collect();
    let mut state =
        ListState::default().with_selected(Some(app.library_selected().saturating_sub(offset)));
    frame.render_stateful_widget(list_widget(items, block, focused), area, &mut state);
}

fn draw_queue(frame: &mut Frame, app: &App, view: &mut ViewState, area: Rect, focused: bool) {
    let queue = app.player().queue();
    let block = panel_block(
        format!("Queue ({})", thousands(queue.len())),
        focused,
        area.width,
    );
    if queue.is_empty() {
        let inner = usize::from(area.width.saturating_sub(2));
        frame.render_widget(
            Paragraph::new(fit("Queue is empty. Press a in the library to add.", inner))
                .block(block),
            area,
        );
        return;
    }
    let width = row_width(area).saturating_sub(2);
    let height = usize::from(block.inner(area).height);
    let offset = scroll_offset(view.queue_offset, app.queue_selected(), queue.len(), height);
    view.queue_offset = offset;
    let current = queue.current();
    let items: Vec<ListItem> = queue
        .entries()
        .iter()
        .skip(offset)
        .take(height)
        .map(|entry| {
            note_row_built();
            let is_current = current == Some(entry.id);
            let marker = if is_current { "▶ " } else { "  " };
            let mut line = row_line(
                &entry.track.display_name(),
                entry.track.duration.map(format_time),
                width,
            );
            line.spans.insert(0, Span::raw(marker));
            if is_current {
                line = line.style(
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                );
            }
            ListItem::new(line)
        })
        .collect();
    let mut state =
        ListState::default().with_selected(Some(app.queue_selected().saturating_sub(offset)));
    frame.render_stateful_widget(list_widget(items, block, focused), area, &mut state);
}

fn status_label(status: PlaybackStatus) -> (&'static str, Color) {
    match status {
        PlaybackStatus::Stopped => ("Stopped", Color::DarkGray),
        PlaybackStatus::Loading => ("Loading", Color::Yellow),
        PlaybackStatus::Playing => ("Playing", Color::Green),
        PlaybackStatus::Paused => ("Paused", Color::Yellow),
        PlaybackStatus::Unavailable => ("Unavailable", Color::Red),
    }
}

fn draw_now_playing(frame: &mut Frame, app: &App, area: Rect) {
    let player = app.player();
    let block = Block::bordered()
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Now Playing ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [info, progress] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);

    let (label, color) = status_label(player.status());
    let label = format!("[{label}] ");
    let entry = player.queue().current_entry();
    let mut spans = vec![Span::styled(
        label.clone(),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )];
    let room = usize::from(info.width).saturating_sub(label.width());
    match entry {
        Some(entry) => {
            let title = fit(&entry.track.title, room);
            let rest = room.saturating_sub(title.width());
            spans.push(Span::styled(
                title,
                Style::default().add_modifier(Modifier::BOLD),
            ));
            if let Some(artist) = &entry.track.artist {
                spans.push(Span::raw(fit(&format!(" — {artist}"), rest)));
            }
        }
        None => spans.push(Span::styled(
            fit("Nothing playing", room),
            Style::default().fg(Color::DarkGray),
        )),
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), info);

    let volume = format!(" Vol {:>3}%", player.volume());
    let [gauge_area, volume_area] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(volume.width() as u16),
    ])
    .areas(progress);
    let duration = player
        .duration()
        .or_else(|| entry.and_then(|e| e.track.duration))
        .filter(|_| entry.is_some());
    let position = player.position().unwrap_or_default();
    let ratio = match duration {
        Some(total) if !total.is_zero() => {
            (position.as_secs_f64() / total.as_secs_f64()).clamp(0.0, 1.0)
        }
        _ => 0.0,
    };
    let time = format!(
        "{} / {}",
        format_time(position),
        duration.map_or_else(|| "--:--".to_owned(), format_time)
    );
    frame.render_widget(
        LineGauge::default()
            .ratio(ratio)
            .label(time)
            .filled_style(Style::default().fg(Color::Cyan))
            .unfilled_style(Style::default().fg(Color::DarkGray)),
        gauge_area,
    );
    frame.render_widget(Paragraph::new(volume), volume_area);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect, now: Instant) {
    let right = match app.scan_state() {
        ScanState::Scanning { tracks } => format!("Scanning… {} ", count(tracks, "track")),
        ScanState::Failed { .. } => "Scan incomplete ".to_owned(),
        ScanState::Finished { .. } => "? help ".to_owned(),
    };
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right.width() as u16)])
            .areas(area);
    let width = usize::from(left_area.width);
    let left = match (app.status(now), app.player().unavailable_reason()) {
        (Some(message), _) => Span::raw(fit(&format!(" {message}"), width)),
        (None, Some(reason)) => Span::styled(
            fit(
                &format!(" Playback unavailable: {reason} (restart Coda)"),
                width,
            ),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        (None, None) => Span::raw(""),
    };
    frame.render_widget(Paragraph::new(left), left_area);
    frame.render_widget(
        Paragraph::new(Span::styled(right, Style::default().fg(Color::DarkGray))),
        right_area,
    );
}

/// `left` truncated, with `right` right-aligned, within `width` columns.
fn row_line(left: &str, right: Option<String>, width: usize) -> Line<'static> {
    let right = right.unwrap_or_default();
    let right_width = right.width();
    if right_width == 0 || right_width + 2 > width {
        return Line::raw(fit(left, width));
    }
    // Saturating throughout: width arithmetic must never underflow, even
    // if a measured width disagrees with expectations.
    let left = fit(left, width.saturating_sub(right_width + 1));
    let pad = width.saturating_sub(right_width + left.width());
    Line::from(vec![
        Span::raw(left),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, Style::default().fg(Color::DarkGray)),
    ])
}

/// Upper bound on the chars `fit` looks at. No row is anywhere near this
/// wide; anything beyond it counts as truncated.
const MAX_FIT_CHARS: usize = 1024;

/// Replaces control and bidi-override characters and truncates to `width`
/// display columns, ending in `…` when shortened.
///
/// Prefixes are measured as strings, not as sums of per-char widths:
/// sequences such as `❤️` (U+2764 U+FE0F) are wider than their chars
/// summed. Work is bounded for hostile input (e.g. thousands of combining
/// marks): at most [`MAX_FIT_CHARS`] chars are considered, and the cut is
/// found by binary search, so the cost is O(n log n) in that bound. Only
/// measured prefixes are returned, so the result never exceeds `width`
/// even where string width is not monotonic.
pub fn fit(text: &str, width: usize) -> String {
    let mut chars = text.chars();
    let clean: String = chars
        .by_ref()
        .take(MAX_FIT_CHARS)
        .map(|c| if is_unsafe(c) { ' ' } else { c })
        .collect();
    let cut_off = chars.next().is_some();
    if !cut_off && clean.width() <= width {
        return clean;
    }
    if width == 0 {
        return String::new();
    }
    let budget = width - 1;
    let ends: Vec<usize> = clean
        .char_indices()
        .map(|(i, _)| i)
        .skip(1)
        .chain([clean.len()])
        .collect();
    // Largest k whose prefix (k chars) fits; k = 0 always fits.
    let (mut lo, mut hi) = (0, ends.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if clean[..ends[mid - 1]].width() <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let mut out = match lo {
        0 => String::new(),
        k => clean[..ends[k - 1]].to_owned(),
    };
    out.push('…');
    out
}

/// Characters that must not reach the terminal from untrusted names:
/// control characters, and bidi controls that could reorder the
/// surrounding layout.
fn is_unsafe(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}'
        )
}

/// `m:ss`, or `h:mm:ss` from an hour.
pub fn format_time(duration: Duration) -> String {
    let secs = duration.as_secs();
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::library::scan::ScanEvent;
    use crate::model::{Track, TrackMetadata};
    use crate::playback::{PlaybackCommand, PlaybackEvent};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(app: &App, width: u16, height: u16, now: Instant) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut view = ViewState::default();
        terminal
            .draw(|frame| draw(frame, app, &mut view, now))
            .unwrap();
        buffer_text(&terminal)
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            let mut x = 0;
            while x < buffer.area.width {
                // A wide character's trailing cell is a placeholder.
                let symbol = buffer[(x, y)].symbol();
                out.push_str(symbol);
                x += symbol.width().max(1) as u16;
            }
            out.push('\n');
        }
        out
    }

    fn tagged(rel: &str, title: &str, artist: &str, secs: u64) -> Track {
        Track::local(
            format!("/music/{rel}").into(),
            TrackMetadata {
                title: Some(title.into()),
                artist: Some(artist.into()),
                duration: Some(Duration::from_secs(secs)),
                ..TrackMetadata::default()
            },
        )
    }

    #[test]
    fn empty_library_after_scan() {
        let now = Instant::now();
        let mut app = App::new("/music".into());
        app.handle_scan_event(
            ScanEvent::Finished {
                tracks: 0,
                skipped: 0,
                errors: 0,
            },
            now,
        );
        let screen = render(&app, 100, 20, now);
        assert!(screen.contains("No music found in /music"), "{screen}");
        assert!(screen.contains("Queue is empty"), "{screen}");
        assert!(screen.contains("[Stopped]"), "{screen}");
        assert!(screen.contains("Nothing playing"), "{screen}");
        assert!(!screen.contains("Scanning"), "{screen}");
    }

    #[test]
    fn scanning_shows_progress() {
        let now = Instant::now();
        let mut app = App::new("/music".into());
        let screen = render(&app, 100, 20, now);
        assert!(screen.contains("Scanning…"), "{screen}");
        let tracks: Vec<Track> = (0..1234)
            .map(|i| tagged(&format!("a/{i:04}.flac"), &format!("t{i}"), "x", 1))
            .collect();
        app.handle_scan_event(ScanEvent::Batch(tracks), now);
        let screen = render(&app, 100, 20, now);
        assert!(screen.contains("Scanning… 1,234 tracks"), "{screen}");
        assert!(screen.contains("a/"), "{screen}");
    }

    #[test]
    fn now_playing_shows_track_state_progress_and_volume() {
        let now = Instant::now();
        let mut app = App::new("/music".into());
        app.handle_scan_event(
            ScanEvent::Batch(vec![tagged(
                "song.flac",
                "Blue Train",
                "John Coltrane",
                643,
            )]),
            now,
        );
        let commands = app.handle_action(Action::Activate, now);
        let Some(PlaybackCommand::Load { load, .. }) = commands.first() else {
            panic!("{commands:?}");
        };
        let screen = render(&app, 100, 20, now);
        assert!(screen.contains("[Loading]"), "{screen}");
        app.handle_playback_event(PlaybackEvent::Started { load: *load }, now);
        app.handle_playback_event(PlaybackEvent::Position(Duration::from_secs(83)), now);
        app.handle_playback_event(PlaybackEvent::Volume(80), now);
        let screen = render(&app, 100, 20, now);
        assert!(
            screen.contains("[Playing] Blue Train — John Coltrane"),
            "{screen}"
        );
        assert!(screen.contains("1:23 / 10:43"), "{screen}");
        assert!(screen.contains("Vol  80%"), "{screen}");
        assert!(screen.contains("▶ John Coltrane - Blue Train"), "{screen}");
        app.handle_playback_event(PlaybackEvent::Paused(true), now);
        assert!(render(&app, 100, 20, now).contains("[Paused]"));
    }

    #[test]
    fn help_overlay_lists_keys() {
        let now = Instant::now();
        let mut app = App::new("/music".into());
        app.handle_action(Action::ToggleHelp, now);
        let screen = render(&app, 80, 24, now);
        assert!(screen.contains(" Help "), "{screen}");
        assert!(screen.contains("Switch panel"), "{screen}");
        assert!(screen.contains("Ctrl-C"), "{screen}");
        // Also fits a tiny terminal without panicking.
        render(&app, 10, 5, now);
    }

    #[test]
    fn narrow_layout_shows_only_the_focused_panel() {
        let now = Instant::now();
        let mut app = App::new("/music".into());
        app.handle_scan_event(ScanEvent::Batch(vec![tagged("a.flac", "A", "B", 1)]), now);
        let wide = render(&app, 80, 20, now);
        assert!(
            wide.contains("Library") && wide.contains("Queue (0)"),
            "{wide}"
        );
        let narrow = render(&app, 59, 20, now);
        assert!(narrow.contains("Library"), "{narrow}");
        assert!(!narrow.contains("Queue (0)"), "{narrow}");
        app.handle_action(Action::SwitchFocus, now);
        let narrow = render(&app, 59, 20, now);
        assert!(narrow.contains("Queue (0)"), "{narrow}");
        assert!(!narrow.contains("Library"), "{narrow}");
    }

    #[test]
    fn long_and_wide_names_truncate_without_breaking_the_layout() {
        let now = Instant::now();
        let mut app = App::new("/music".into());
        let long = "雨".repeat(80);
        app.handle_scan_event(
            ScanEvent::Batch(vec![tagged("x.flac", &long, "Ärtist\u{1b}[31m", 100)]),
            now,
        );
        app.handle_action(Action::Enqueue, now);
        for width in [40, 61, 100] {
            let screen = render(&app, width, 12, now);
            for line in screen.lines() {
                assert_eq!(line.width(), usize::from(width), "{screen}");
            }
            assert!(screen.contains('…'), "{screen}");
            assert!(!screen.contains('\u{1b}'), "{screen}");
        }
    }

    #[test]
    fn persistent_backend_error_shows_after_messages_expire() {
        let now = Instant::now();
        let mut app = App::new("/music".into());
        app.handle_playback_event(PlaybackEvent::BackendExited("mpv exited".into()), now);
        let later = now + crate::app::STATUS_TTL * 2;
        let screen = render(&app, 100, 12, later);
        assert!(
            screen.contains("Playback unavailable: mpv exited"),
            "{screen}"
        );
        assert!(screen.contains("[Unavailable]"), "{screen}");
    }

    /// Strings whose width differs between per-char and string measuring,
    /// plus other hostile input.
    const TRICKY: &[&str] = &[
        "",
        "❤️❤️❤️❤️❤️❤️❤️❤️❤️❤️❤️❤️❤️❤️❤️❤️❤️❤️❤️❤️",
        "👨‍👩‍👧‍👦👨‍👩‍👧‍👦👨‍👩‍👧‍👦 family",
        "👍🏽👍🏿 thumbs 👍🏻",
        "e\u{301}e\u{301}e\u{301} combining a\u{308}\u{308}",
        "雨雨雨雨雨雨雨雨雨雨雨雨雨雨雨",
        "🇳🇱🇯🇵🇺🇸🇧🇷 flags",
        "tab\there\nnew\u{1b}[31mline\u{7f}",
        "\u{202E}evil\u{202C} \u{2066}iso\u{2069} \u{200E}\u{200F}\u{061C}x",
        "mixed ❤️ 雨 e\u{301} 👨‍👩‍👧 🇳🇱 \u{202E}end",
    ];

    #[test]
    fn fit_never_exceeds_the_width() {
        for text in TRICKY {
            for width in 0..=40 {
                let out = fit(text, width);
                assert!(
                    out.width() <= width,
                    "fit({text:?}, {width}) = {out:?} has width {}",
                    out.width()
                );
                assert!(!out.chars().any(is_unsafe), "{out:?}");
            }
        }
    }

    #[test]
    fn row_line_never_exceeds_the_width() {
        for text in TRICKY {
            for width in 0..=40 {
                let line = row_line(text, Some("12:34".into()), width);
                assert!(
                    line.width() <= width,
                    "{text:?} at {width}: {}",
                    line.width()
                );
            }
        }
    }

    fn hostile_inputs() -> Vec<String> {
        let combining = format!("a{}{}", "\u{301}".repeat(100_000), "b".repeat(200));
        let mut mixed = String::new();
        while mixed.len() < 1_000_000 {
            mixed.push_str(TRICKY[(mixed.len() / 7) % TRICKY.len()]);
            mixed.push_str("\u{301}\u{301}\u{FE0F}\u{200D}");
        }
        vec![combining, mixed]
    }

    #[test]
    fn fit_is_bounded_on_hostile_input() {
        for text in hostile_inputs() {
            for width in [0, 1, 2, 10, 40, 200] {
                let started = Instant::now();
                let out = fit(&text, width);
                let elapsed = started.elapsed();
                assert!(out.width() <= width, "{width}: {}", out.width());
                assert!(out.chars().count() <= MAX_FIT_CHARS + 1);
                // Only guards against a quadratic regression; real cost is microseconds.
                assert!(
                    elapsed < Duration::from_secs(1),
                    "fit of {} bytes at width {width} took {elapsed:?}",
                    text.len()
                );
            }
        }
    }

    #[test]
    fn hostile_names_render_quickly() {
        let now = Instant::now();
        let mut app = App::new("/music".into());
        // Bypasses the ingestion cap on purpose: render must cope alone.
        let tracks = hostile_inputs()
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let mut t = tagged(&format!("d/{i}.wav"), "x", "y", 60);
                t.title = name.clone();
                t.artist = Some(name);
                t
            })
            .collect();
        app.handle_scan_event(ScanEvent::Batch(tracks), now);
        app.handle_action(Action::Activate, now);
        app.handle_action(Action::Enqueue, now);
        let commands = app.handle_action(Action::Activate, now);
        assert!(!commands.is_empty());
        for width in [40, 120] {
            let started = Instant::now();
            let screen = render(&app, width, 14, now);
            let elapsed = started.elapsed();
            for line in screen.lines() {
                assert_eq!(line.width(), usize::from(width), "{screen}");
            }
            // Only guards against a quadratic regression; real cost is microseconds.
            assert!(elapsed < Duration::from_secs(1), "render took {elapsed:?}");
        }
    }

    #[test]
    fn scroll_follows_the_selection_minimally() {
        // Within the window: unchanged.
        assert_eq!(scroll_offset(0, 3, 100, 10), 0);
        assert_eq!(scroll_offset(5, 14, 100, 10), 5);
        // Past the bottom: the selection becomes the last visible row.
        assert_eq!(scroll_offset(0, 10, 100, 10), 1);
        assert_eq!(scroll_offset(5, 15, 100, 10), 6);
        // Past the top: the selection becomes the first visible row.
        assert_eq!(scroll_offset(20, 19, 100, 10), 19);
        // Jump to the end and back.
        assert_eq!(scroll_offset(0, 99, 100, 10), 90);
        assert_eq!(scroll_offset(90, 0, 100, 10), 0);
    }

    #[test]
    fn scroll_stays_valid_when_the_list_shrinks() {
        // Removing the last entries with the selection clamped to the end.
        assert_eq!(scroll_offset(90, 94, 95, 10), 85);
        assert_eq!(scroll_offset(85, 89, 90, 10), 80);
        // The list shrinks below the offset, or to fewer rows than a page.
        assert_eq!(scroll_offset(90, 3, 5, 10), 0);
        assert_eq!(scroll_offset(90, 4, 50, 10), 4);
        assert_eq!(scroll_offset(40, 45, 50, 10), 40);
        // A stale out-of-range selection is clamped.
        assert_eq!(scroll_offset(0, 500, 20, 10), 10);
    }

    #[test]
    fn scroll_handles_viewport_changes_and_degenerate_sizes() {
        // Shrinking keeps the selection visible, growing uses the room.
        assert_eq!(scroll_offset(10, 19, 100, 10), 10);
        assert_eq!(scroll_offset(10, 19, 100, 5), 15);
        assert_eq!(scroll_offset(15, 19, 100, 20), 15);
        assert_eq!(scroll_offset(95, 97, 100, 20), 80);
        // Empty list and zero height.
        assert_eq!(scroll_offset(7, 0, 0, 10), 0);
        assert_eq!(scroll_offset(7, 3, 100, 0), 0);
        // Height one: the selection is the only row.
        assert_eq!(scroll_offset(0, 0, 100, 1), 0);
        assert_eq!(scroll_offset(0, 42, 100, 1), 42);
        assert_eq!(scroll_offset(42, 41, 100, 1), 41);
    }

    /// An app whose queue holds `n` tracks from one folder, focused on the
    /// queue with entry 0 playing.
    fn queue_app(n: usize, title: impl Fn(usize) -> String, now: Instant) -> App {
        let mut app = App::new("/music".into());
        let tracks = (0..n)
            .map(|i| {
                let mut t = tagged(&format!("d/{i:05}.flac"), "x", "x", 60);
                t.title = title(i);
                t
            })
            .collect();
        app.handle_scan_event(ScanEvent::Batch(tracks), now);
        app.handle_action(Action::Enqueue, now); // the folder, recursively
        app.handle_action(Action::SwitchFocus, now);
        app.handle_action(Action::Activate, now);
        assert_eq!(app.player().queue().len(), n);
        app
    }

    fn rows_built_by(f: impl FnOnce()) -> usize {
        ROWS_BUILT.with(|n| n.set(0));
        f();
        ROWS_BUILT.with(|n| n.get())
    }

    fn draw_to(
        terminal: &mut Terminal<TestBackend>,
        app: &App,
        view: &mut ViewState,
        now: Instant,
    ) {
        terminal.draw(|f| draw(f, app, view, now)).unwrap();
    }

    #[test]
    fn large_queue_shows_the_selected_window_and_markers() {
        let now = Instant::now();
        let mut app = queue_app(50_000, |i| format!("song{i:05}"), now);
        let mut view = ViewState::default();
        // 14 rows - now playing (4) - status (1) = 9 panel rows, 7 inner.
        let mut terminal = Terminal::new(TestBackend::new(100, 14)).unwrap();
        draw_to(&mut terminal, &app, &mut view, now);
        let screen = buffer_text(&terminal);
        assert!(screen.contains("▶ x - song00000"), "{screen}");

        for _ in 0..49_995 {
            app.handle_action(Action::Down, now);
        }
        let built = rows_built_by(|| draw_to(&mut terminal, &app, &mut view, now));
        let screen = buffer_text(&terminal);
        // Selection is the last visible row; the window ends there.
        assert_eq!(view.queue_offset, 49_995 + 1 - 7);
        assert!(screen.contains("x - song49989"), "{screen}");
        assert!(
            screen.contains("> ") && screen.contains("x - song49995"),
            "{screen}"
        );
        assert!(!screen.contains("x - song49996"), "{screen}");
        assert!(!screen.contains("x - song00000"), "{screen}");
        assert!(!screen.contains('▶'), "{screen}");
        // Work scales with the viewport: queue rows plus library rows.
        assert!(built <= 2 * 7, "built {built} rows");

        // Selecting the playing entry again shows its marker.
        for _ in 0..49_995 {
            app.handle_action(Action::Up, now);
        }
        draw_to(&mut terminal, &app, &mut view, now);
        assert_eq!(view.queue_offset, 0);
        assert!(buffer_text(&terminal).contains("▶ x - song00000"));
    }

    #[test]
    fn offset_persists_and_follows_removal_and_resize() {
        let now = Instant::now();
        let mut app = queue_app(30, |i| format!("song{i:02}"), now);
        let mut view = ViewState::default();
        let mut terminal = Terminal::new(TestBackend::new(100, 14)).unwrap();
        for _ in 0..12 {
            app.handle_action(Action::Down, now);
        }
        draw_to(&mut terminal, &app, &mut view, now);
        assert_eq!(view.queue_offset, 6);
        // Moving up within the window does not scroll.
        app.handle_action(Action::Up, now);
        app.handle_action(Action::Up, now);
        draw_to(&mut terminal, &app, &mut view, now);
        assert_eq!(view.queue_offset, 6);
        // Growing the terminal keeps the selection visible.
        terminal.backend_mut().resize(100, 40);
        terminal
            .resize(ratatui::layout::Rect::new(0, 0, 100, 40))
            .unwrap();
        draw_to(&mut terminal, &app, &mut view, now);
        assert!(buffer_text(&terminal).contains("song10"));
        assert_eq!(view.queue_offset, 0);
        // Shrinking too (down to a one-row panel).
        terminal.backend_mut().resize(100, 7);
        terminal
            .resize(ratatui::layout::Rect::new(0, 0, 100, 7))
            .unwrap();
        draw_to(&mut terminal, &app, &mut view, now);
        assert!(buffer_text(&terminal).contains("song10"));
        // Removing entries from the end keeps the last rows on screen.
        terminal.backend_mut().resize(100, 14);
        terminal
            .resize(ratatui::layout::Rect::new(0, 0, 100, 14))
            .unwrap();
        for _ in 0..30 {
            app.handle_action(Action::Down, now);
        }
        draw_to(&mut terminal, &app, &mut view, now);
        for _ in 0..10 {
            app.handle_action(Action::Remove, now);
            draw_to(&mut terminal, &app, &mut view, now);
            let screen = buffer_text(&terminal);
            let last = app.player().queue().len() - 1;
            assert!(screen.contains(&format!("x - song{last:02}")), "{screen}");
        }
        assert_eq!(app.player().queue().len(), 20);
        assert_eq!(view.queue_offset, 13);
    }

    #[test]
    fn hostile_large_queue_renders_quickly() {
        let now = Instant::now();
        // Maximum-length (as capped at ingestion), mostly zero-width names.
        let hostile: Vec<String> = hostile_inputs()
            .into_iter()
            .map(|s| s.chars().take(crate::model::MAX_METADATA_CHARS).collect())
            .collect();
        let mut app = queue_app(20_000, |i| hostile[i % hostile.len()].clone(), now);
        for _ in 0..19_990 {
            app.handle_action(Action::Down, now);
        }
        let mut view = ViewState::default();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let started = Instant::now();
        let built = rows_built_by(|| draw_to(&mut terminal, &app, &mut view, now));
        let elapsed = started.elapsed();
        // Only guards against a regression to per-entry work.
        assert!(elapsed < Duration::from_secs(1), "render took {elapsed:?}");
        assert!(built <= 2 * 19, "built {built} rows");
    }

    #[test]
    fn bidi_controls_are_neutralised() {
        assert_eq!(fit("a\u{202E}b\u{2067}c\u{200F}d\u{061C}", 20), "a b c d ");
    }

    #[test]
    fn emoji_names_render_in_every_panel_without_breaking_the_layout() {
        let now = Instant::now();
        let mut app = App::new("/music".into());
        let heart = "❤️".repeat(60);
        let family = "👨‍👩‍👧‍👦".repeat(30);
        app.handle_scan_event(
            ScanEvent::Batch(vec![
                tagged(&format!("{heart}/{heart}.wav"), &heart, &family, 61),
                tagged(&format!("{heart}/b.wav"), &family, &heart, 62),
                tagged("x/c.wav", "🇳🇱👍🏽e\u{301}", "\u{202E}rtl", 63),
            ]),
            now,
        );
        // Open the emoji folder, enqueue both and play the first (fills
        // the library, queue and now-playing bar).
        app.handle_action(Action::Activate, now);
        app.handle_action(Action::Enqueue, now);
        app.handle_action(Action::Down, now);
        app.handle_action(Action::Enqueue, now);
        let commands = app.handle_action(Action::Up, now);
        assert!(commands.is_empty());
        let commands = app.handle_action(Action::Activate, now);
        let Some(PlaybackCommand::Load { load, .. }) = commands.first() else {
            panic!("{commands:?}");
        };
        app.handle_playback_event(PlaybackEvent::Started { load: *load }, now);
        for focus_queue in [false, true] {
            for width in [20, 40, 59, 60, 61, 80, 133] {
                let screen = render(&app, width, 14, now);
                for line in screen.lines() {
                    assert_eq!(line.width(), usize::from(width), "{width}:\n{screen}");
                }
                assert!(screen.contains('…'), "{screen}");
            }
            if !focus_queue {
                app.handle_action(Action::SwitchFocus, now);
            }
        }
    }

    #[test]
    fn fit_truncates_by_display_width() {
        assert_eq!(fit("hello", 5), "hello");
        assert_eq!(fit("hello", 4), "hel…");
        assert_eq!(fit("hello", 0), "");
        assert_eq!(fit("雨雨雨", 5), "雨雨…");
        assert_eq!(fit("雨雨雨", 4), "雨…");
        assert_eq!(fit("a\nb", 3), "a b");
    }

    #[test]
    fn time_formatting() {
        assert_eq!(format_time(Duration::from_secs(0)), "0:00");
        assert_eq!(format_time(Duration::from_secs(83)), "1:23");
        assert_eq!(format_time(Duration::from_secs(3725)), "1:02:05");
    }
}
