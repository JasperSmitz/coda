//! The Now playing screen (`f`): a calm display of the track heard, in
//! place of the panels and the now-playing bar.
//!
//! - **Side by side** (when it fits): a square cover on the left, about
//!   55 % of the height (35–55 %), and beside it a left-aligned column,
//!   centred against the cover: the title in bold, album · artist (dim)
//!   with the repeat mark and a passing volume right-aligned on its row, a
//!   blank row, the timeline and the visualizer's bars, both as wide as
//!   the column, and a dim Next up line when it is due
//!   ([`crate::now_playing`]). Cover and column form one block, centred
//!   on the screen.
//! - **Stacked** (narrow or tall windows, or no cover): the cover, the
//!   title with album · artist under it, the visualizer, the timeline and
//!   Next up, each centred.
//!
//! Only the cover, the title and the timeline are not dim grey; the
//! timeline's played part is cyan as in the bar, and it is all dim unless
//! playing or loading.
//!
//! [`screen_layout`] is pure. With too little room, side by side the
//! cover first shrinks (down to [`MIN_COVER_SHARE`] of the height), the
//! bars go next to a cover under ten rows, and then the screen is
//! stacked; stacked, the cover goes first, then the visualizer. The text
//! and the timeline always fit.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::player::{PlaybackStatus, RepeatMode};
use crate::ui::cover::{ArtFrame, ArtPainter, DIACRITICS};
use crate::ui::render::{
    TIMELINE_LEFT, TIMELINE_PLAYED, VISUALIZER_MIN_SCREEN, draw_visualizer, fit, format_time,
};

/// A cover smaller than this many rows is left out (the bar already shows
/// one that size).
pub const MIN_COVER_ROWS: u16 = 6;
/// Side by side, the cover takes at most this share of the height
/// (percent), so it does not fill the screen.
pub const COVER_SHARE: u16 = 55;
/// Side by side, the cover shrinks down to this share of the height
/// (percent) to leave the text column its minimum width, before the
/// screen is stacked. At 35 % the cover is still about two thirds of its
/// full size and clearly larger than the bar's; smaller than that, a
/// stacked screen shows a much larger cover, which reads better.
pub const MIN_COVER_SHARE: u16 = 35;
/// Side by side: columns between the cover and the text column.
pub const COLUMN_GAP: u16 = 4;
/// Side by side: the text column's width range. Narrower than the minimum
/// next to the cover, the screen is stacked.
pub const MIN_COLUMN: u16 = 30;
pub const MAX_COLUMN: u16 = 64;
/// Side by side: the visualizer's height in the column, at most.
pub const MAX_COLUMN_VISUALIZER_ROWS: u16 = 8;
/// The visualizer is left out rather than drawn lower than this.
pub const MIN_VISUALIZER_ROWS: u16 = 3;
/// Stacked: the visualizer's height below a cover: a fifth of the screen,
/// within these.
pub const MAX_VISUALIZER_ROWS: u16 = 10;
/// Stacked without a cover the visualizer takes its room: a third of the
/// screen, up to this.
pub const MAX_VISUALIZER_ROWS_ALONE: u16 = 16;
/// Stacked: the timeline is never wider than this.
pub const MAX_TIMELINE_WIDTH: u16 = 64;
/// Below this width the bar is left out and the timeline is just times.
const MIN_BAR: u16 = 4;
/// Side by side: album · artist keeps at least this many columns (or all
/// it needs, if less) beside the extras on its row.
const MIN_HEADER: u16 = 16;
/// Columns kept free left and right of the screen's content.
const SIDE: u16 = 2;
/// Stacked: room kept on both sides of a wide timeline for the extras
/// beside it (`⟳ 1  Vol 100%` and a two-column gap), so it does not move
/// when they come and go.
const EXTRAS_ROOM: u16 = 15;
/// Stacked: a timeline narrower than this gives up the centring room.
const MIN_CENTRED_TIMELINE: u16 = 24;
/// Stacked text rows: the title, album · artist, a blank row, the
/// timeline and Next up.
const TEXT_ROWS: u16 = 5;
const DIM: Style = Style::new().fg(Color::DarkGray);

/// Where each part of the screen goes; a part without room (or without
/// anything to show) is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScreenLayout {
    /// Side by side: the text is left-aligned in a column beside the
    /// cover; else (stacked) everything is centred.
    pub side_by_side: bool,
    pub header: Option<Rect>,
    pub cover: Option<Rect>,
    pub title: Option<Rect>,
    pub visualizer: Option<Rect>,
    pub timeline: Option<Rect>,
    pub next: Option<Rect>,
}

/// The screen's parts in `area`. `cover` (the painter, given only when a
/// cover is to be shown) sizes a square-looking cover; `visualizer` asks
/// for the visualizer. Side by side when [`side_by_side`] fits, else
/// [`stacked`].
pub fn screen_layout(area: Rect, cover: Option<&ArtPainter>, visualizer: bool) -> ScreenLayout {
    cover
        .and_then(|painter| side_by_side(area, painter, visualizer))
        .unwrap_or_else(|| stacked(area, cover, visualizer))
}

/// The side-by-side layout, if it fits: a cover of [`COVER_SHARE`] of the
/// height, shrunk if needed so the column keeps [`MIN_COLUMN`] columns,
/// but not below [`MIN_COVER_SHARE`] of the height or [`MIN_COVER_ROWS`]
/// (else `None`: stacked); then [`COLUMN_GAP`] columns, then
/// a column of [`MIN_COLUMN`] to [`MAX_COLUMN`] columns, all within the
/// width less two columns a side. The column: title, album · artist, a
/// blank row, the timeline, a blank row, the visualizer (as many rows as
/// the cover leaves, up to [`MAX_COLUMN_VISUALIZER_ROWS`]; left out under
/// [`MIN_VISUALIZER_ROWS`]) and a blank row, then Next up. The column is
/// centred against the cover, the two together on the screen.
pub fn side_by_side(area: Rect, painter: &ArtPainter, visualizer: bool) -> Option<ScreenLayout> {
    let (width, height) = (area.width, area.height);
    let share = |percent: u16| (u32::from(height) * u32::from(percent) / 100) as u16;
    // The widest cover that leaves the column its minimum width.
    let room = width.saturating_sub(2 * SIDE + COLUMN_GAP + MIN_COLUMN);
    let rows = largest_fitting(painter, share(COVER_SHARE), room);
    // The floor counts the width only: at the diacritics' limit the
    // cover is as large as it gets anyway.
    let largest = largest_fitting(painter, share(COVER_SHARE), u16::MAX);
    if rows < MIN_COVER_ROWS.max(share(MIN_COVER_SHARE).min(largest)) {
        return None;
    }
    let cols = painter.columns(rows);
    let column = width
        .saturating_sub(2 * SIDE + cols + COLUMN_GAP)
        .min(MAX_COLUMN);
    if column < MIN_COLUMN {
        return None;
    }
    let bars = if visualizer && column >= VISUALIZER_MIN_SCREEN.0 {
        rows.saturating_sub(7).min(MAX_COLUMN_VISUALIZER_ROWS)
    } else {
        0
    };
    let bars = if bars >= MIN_VISUALIZER_ROWS { bars } else { 0 };
    // Title, album · artist, blank, timeline, blank, [bars, blank], next.
    let column_rows = 6 + if bars > 0 { bars + 1 } else { 0 };
    let block_rows = rows.max(column_rows);
    let block_cols = cols + COLUMN_GAP + column;
    let top = area.y + height.saturating_sub(block_rows) / 2;
    let left = area.x + width.saturating_sub(block_cols) / 2;
    let x = left + cols + COLUMN_GAP;
    let mut y = top + (block_rows - column_rows) / 2;
    let mut row = |rows: u16, gap: u16| {
        let rect = Rect::new(x, y, column, rows);
        y += rows + gap;
        rect
    };
    let title = row(1, 0);
    let header = row(1, 1);
    let timeline = row(1, 1);
    let visualizer = (bars > 0).then(|| row(bars, 1));
    let next = row(1, 0);
    Some(ScreenLayout {
        side_by_side: true,
        header: Some(header),
        cover: Some(Rect::new(left, top + (block_rows - rows) / 2, cols, rows)),
        title: Some(title),
        visualizer,
        timeline: Some(timeline),
        next: Some(next),
    })
}

/// The stacked layout in `area`:
///
/// - Margins: one row above and below, when there is room for the text
///   rows and both.
/// - With a cover: the visualizer takes a fifth of the height
///   ([`MIN_VISUALIZER_ROWS`] to [`MAX_VISUALIZER_ROWS`]) and the cover
///   the rest, as wide as it looks square, narrowed to fit the width; a
///   cover under [`MIN_COVER_ROWS`] is left out.
/// - Without one, the visualizer takes a third of the height (up to
///   [`MAX_VISUALIZER_ROWS_ALONE`]), else it is left out.
/// - Top to bottom: the cover, a blank row, the title, album · artist, a
///   blank row, the visualizer and a blank row, the timeline, Next up. The
///   text alone needs five rows; lower than that the blank row goes, then
///   Next up, album · artist and the timeline, in that order; the title
///   goes last.
///
/// Everything is centred; the text rows keep two columns free on either
/// side, the visualizer a tenth of the width (at least two columns).
pub fn stacked(area: Rect, cover: Option<&ArtPainter>, visualizer: bool) -> ScreenLayout {
    let (width, height) = (area.width, area.height);
    let margins = if height >= TEXT_ROWS + 2 { 2 } else { 0 };
    let room = height - margins;
    let text_width = width.saturating_sub(2 * SIDE).max(width.min(1));
    let visualizer = visualizer && width >= VISUALIZER_MIN_SCREEN.0;
    // Rows besides the cover's and the visualizer's (each brings a blank
    // row below it).
    let fixed = |cover: bool, rows: u16| TEXT_ROWS + u16::from(cover) + rows + u16::from(rows > 0);

    let mut cover_rows = 0;
    let mut cover_cols = 0;
    let mut visualizer_rows = 0;
    if let Some(painter) = cover {
        let wanted = if visualizer {
            (height / 5).clamp(MIN_VISUALIZER_ROWS, MAX_VISUALIZER_ROWS)
        } else {
            0
        };
        let rows = largest_fitting(
            painter,
            room.saturating_sub(fixed(true, wanted)),
            text_width,
        );
        if rows >= MIN_COVER_ROWS {
            cover_rows = rows;
            cover_cols = painter.columns(rows);
            visualizer_rows = wanted;
        }
    }
    if cover_rows == 0 && visualizer {
        let rows = room
            .saturating_sub(fixed(false, 0) + 1)
            .min((height / 3).clamp(MIN_VISUALIZER_ROWS, MAX_VISUALIZER_ROWS_ALONE));
        if rows >= MIN_VISUALIZER_ROWS {
            visualizer_rows = rows;
        }
    }

    let side = |w: u16| Rect {
        x: area.x + (width - w.min(width)) / 2,
        width: w.min(width),
        ..area
    };
    let text = side(text_width);
    let mut layout = ScreenLayout::default();
    if room < TEXT_ROWS {
        // Compact: no blank rows, the most important text first: the
        // title, the timeline, album · artist, Next up.
        let rank = [0u16, 2, 1, 3]; // title, header, timeline, next
        let kept = rank.map(|rank| rank < height);
        let mut y = area.y;
        for (index, slot) in [
            &mut layout.title,
            &mut layout.header,
            &mut layout.timeline,
            &mut layout.next,
        ]
        .into_iter()
        .enumerate()
        {
            if kept[index] {
                *slot = Some(Rect {
                    y,
                    height: 1,
                    ..text
                });
                y += 1;
            }
        }
        return layout;
    }
    let total = fixed(cover_rows > 0, visualizer_rows) + cover_rows;
    let mut y = area.y + height.saturating_sub(total) / 2;
    let mut take = |rows: u16, gap: u16| {
        let top = y;
        y += rows + gap;
        top
    };
    if cover_rows > 0 {
        layout.cover = Some(Rect {
            y: take(cover_rows, 1),
            height: cover_rows,
            ..side(cover_cols)
        });
    }
    layout.title = Some(Rect {
        y: take(1, 0),
        height: 1,
        ..text
    });
    layout.header = Some(Rect {
        y: take(1, 1),
        height: 1,
        ..text
    });
    if visualizer_rows > 0 {
        let margin = (width / 10).max(2);
        layout.visualizer = Some(Rect {
            y: take(visualizer_rows, 1),
            height: visualizer_rows,
            ..side(width.saturating_sub(2 * margin))
        });
    }
    layout.timeline = Some(Rect {
        y: take(1, 0),
        height: 1,
        ..text
    });
    layout.next = Some(Rect {
        y: take(1, 0),
        height: 1,
        ..text
    });
    layout
}

/// The most rows, at most `rows`, of a cover whose square width fits
/// `width` and the placeholder diacritics. Compared unclamped:
/// [`ArtPainter::columns`] stops at the diacritics, which would squash a
/// cover taller than that.
fn largest_fitting(painter: &ArtPainter, rows: u16, width: u16) -> u16 {
    let limit = u32::from(width).min(DIACRITICS.len() as u32);
    let mut rows = rows.min(DIACRITICS.len() as u16);
    while rows > 0 && painter.square_columns(rows) > limit {
        rows -= 1;
    }
    rows
}

/// The screen's layout for the app's state in `area`: a cover when a
/// painter is given (cover art is on), a track is heard and it has a cover
/// (or one is loading after a track that had one).
pub fn layout_for(area: Rect, app: &App, painter: Option<&ArtPainter>) -> ScreenLayout {
    screen_layout(area, cover_for(app, painter), app.visualizer_shown())
}

/// The painter, when the screen shows a cover with it.
fn cover_for<'a>(app: &App, painter: Option<&'a ArtPainter>) -> Option<&'a ArtPainter> {
    painter.filter(|_| app.covers().reserve() && app.player().playing_entry().is_some())
}

/// What the cover area shows: the large cover once loaded, else the small
/// one, where [`layout_for`] puts it in `area`.
pub fn art_frame(area: Rect, app: &App, painter: &ArtPainter) -> Option<ArtFrame> {
    let cover = layout_for(area, app, Some(painter)).cover?;
    let (image, picture) = app.covers().large_picture()?;
    Some(ArtFrame {
        image: image.clone(),
        picture,
        cols: cover.width,
        rows: cover.height,
    })
}

/// Whether the screen's layout in `area` has room for the visualizer,
/// shown or not (the backend is asked for readings only then).
pub fn visualizer_room(area: Rect, app: &App, painter: Option<&ArtPainter>) -> bool {
    // The layout that draws, with the visualizer asked for: the same
    // cover decision, so side by side next to a small cover (no room for
    // the bars) counts as no room.
    screen_layout(area, cover_for(app, painter), true)
        .visualizer
        .is_some()
}

/// Draws the screen into `area`. Returns the visualizer's area, if drawn.
pub fn draw(
    frame: &mut Frame,
    app: &App,
    painter: Option<&ArtPainter>,
    area: Rect,
    now: Instant,
) -> Option<Rect> {
    let player = app.player();
    let Some(entry) = player.playing_entry() else {
        let middle = Rect {
            y: area.y + area.height / 2,
            height: area.height.min(1),
            ..area
        };
        frame.render_widget(
            Line::styled(fit("Nothing playing", usize::from(area.width)), DIM).centered(),
            middle,
        );
        return None;
    };
    let layout = layout_for(area, app, painter);
    // Side by side the text is left-aligned in its column.
    let place = |line: Line<'static>| {
        if layout.side_by_side {
            line
        } else {
            line.centered()
        }
    };
    let track = &entry.track;
    if let Some(rect) = layout.header {
        let header = [track.album.as_deref(), track.artist.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        if layout.side_by_side {
            draw_column_header(frame, app, rect, &header, now);
        } else {
            let header = fit(&header, usize::from(rect.width));
            frame.render_widget(place(Line::styled(header, DIM)), rect);
        }
    }
    if let (Some(rect), Some(painter)) = (layout.cover, painter) {
        painter.draw(frame.buffer_mut(), rect);
    }
    if let Some(rect) = layout.title {
        let title = fit(&track.title, usize::from(rect.width));
        frame.render_widget(
            place(Line::styled(
                title,
                Style::new().add_modifier(Modifier::BOLD),
            )),
            rect,
        );
    }
    if let Some(rect) = layout.visualizer {
        draw_visualizer(frame.buffer_mut(), app, rect);
    }
    if let Some(rect) = layout.timeline {
        if layout.side_by_side {
            draw_column_timeline(frame, app, rect);
        } else {
            draw_timeline(
                frame,
                app,
                Rect {
                    x: area.x,
                    width: area.width,
                    ..rect
                },
                now,
            );
        }
    }
    if let (Some(rect), Some(next)) = (layout.next, app.now_playing().up_next(player, now)) {
        let mut line = format!("Next: {}", next.track.title);
        if let Some(artist) = &next.track.artist {
            line.push_str(" — ");
            line.push_str(artist);
        }
        frame.render_widget(
            place(Line::styled(fit(&line, usize::from(rect.width)), DIM)),
            rect,
        );
    }
    layout.visualizer
}

/// The repeat mark and the passing volume, for beside the timeline.
/// Repeat one is `⟳ 1`: fonts draw `⟳` wider than its cell, so a digit
/// right after it would run into it.
fn extras(app: &App, now: Instant) -> String {
    let (repeat, volume) = extra_parts(app, now);
    join_extras(repeat, volume)
}

/// The repeat mark and the passing volume, each if shown.
fn extra_parts(app: &App, now: Instant) -> (Option<String>, Option<String>) {
    let player = app.player();
    let repeat = match player.repeat() {
        RepeatMode::Off => None,
        RepeatMode::All => Some("⟳".to_owned()),
        RepeatMode::One => Some("⟳ 1".to_owned()),
    };
    let volume = app
        .now_playing()
        .volume_shown(now)
        .then(|| format!("Vol {}%", player.volume()));
    (repeat, volume)
}

fn join_extras(repeat: Option<String>, volume: Option<String>) -> String {
    [repeat, volume]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("  ")
}

/// The styles of the timeline's text, played part and remainder: as the
/// bar's progress line (cyan and dim grey); all dim unless playing or
/// loading (paused, stopped after a restart or at the end of the queue,
/// unavailable).
fn timeline_styles(app: &App) -> (Style, Style, Style) {
    let still = !matches!(
        app.player().status(),
        PlaybackStatus::Playing | PlaybackStatus::Loading
    );
    if still {
        (DIM, DIM, DIM)
    } else {
        (Style::new(), TIMELINE_PLAYED, TIMELINE_LEFT)
    }
}

/// Elapsed time, a bar and the duration in at most `width` columns.
fn timeline_line(app: &App, width: u16) -> Line<'static> {
    let player = app.player();
    let (text, filled, unfilled) = timeline_styles(app);
    let entry = player.playing_entry();
    let duration = player
        .duration()
        .or_else(|| entry.and_then(|e| e.track.duration));
    let position = player.position().unwrap_or_default();
    let elapsed = format_time(position);
    let total = duration.map_or_else(|| "--:--".to_owned(), format_time);
    let times = (elapsed.width() + total.width() + 2) as u16;
    if width < times + MIN_BAR {
        return Line::styled(
            fit(&format!("{elapsed} / {total}"), usize::from(width)),
            text,
        );
    }
    let bar = width - times;
    let ratio = match duration {
        Some(total) if !total.is_zero() => {
            (position.as_secs_f64() / total.as_secs_f64()).clamp(0.0, 1.0)
        }
        _ => 0.0,
    };
    let done = ((ratio * f64::from(bar)).round() as u16).min(bar);
    Line::from(vec![
        Span::styled(format!("{elapsed} "), text),
        Span::styled("━".repeat(usize::from(done)), filled),
        Span::styled("─".repeat(usize::from(bar - done)), unfilled),
        Span::styled(format!(" {total}"), text),
    ])
}

/// Side by side: the timeline across the whole column `row`, as wide as
/// the bars below it, whatever the extras do.
fn draw_column_timeline(frame: &mut Frame, app: &App, row: Rect) {
    let line = timeline_line(app, row.width);
    let width = (line.width() as u16).min(row.width);
    frame.render_widget(line, Rect { width, ..row });
}

/// Side by side: album · artist left-aligned in the column's `row`, and
/// the extras (repeat mark, passing volume) right-aligned on the same row,
/// above the end of the timeline. Album · artist is cut with `…` to keep
/// two columns before the extras; when both do not fit (album · artist
/// keeping at least [`MIN_HEADER`] columns, or all it has if less), the
/// repeat mark goes first, then the volume.
fn draw_column_header(frame: &mut Frame, app: &App, row: Rect, header: &str, now: Instant) {
    let (repeat, volume) = extra_parts(app, now);
    let header_needs = (header.width() as u16).min(MIN_HEADER);
    let extras = [
        join_extras(repeat, volume.clone()),
        join_extras(None, volume),
        String::new(),
    ]
    .into_iter()
    .find(|extras| {
        let width = extras.width() as u16;
        extras.is_empty()
            || (header_needs == 0 && width <= row.width)
            || width + 2 + header_needs <= row.width
    })
    .unwrap_or_default();
    let extras_width = extras.width() as u16;
    let room = if extras.is_empty() {
        row.width
    } else {
        row.width.saturating_sub(extras_width + 2)
    };
    let header = fit(header, usize::from(room));
    frame.render_widget(Line::styled(header, DIM), Rect { width: room, ..row });
    if !extras.is_empty() {
        frame.render_widget(
            Span::styled(extras, DIM),
            Rect {
                x: row.right() - extras_width,
                width: extras_width,
                ..row
            },
        );
    }
}

/// Stacked: the timeline centred in `row` (the screen's width) and at
/// most [`MAX_TIMELINE_WIDTH`] wide, with the extras to its right.
fn draw_timeline(frame: &mut Frame, app: &App, row: Rect, now: Instant) {
    let extras = extras(app, now);
    let extras_width = extras.width() as u16;
    let inner = row.width.saturating_sub(2 * SIDE);
    let (x, width) = if inner >= 2 * EXTRAS_ROOM + MIN_CENTRED_TIMELINE {
        let width = (inner - 2 * EXTRAS_ROOM).min(MAX_TIMELINE_WIDTH);
        (row.x + (row.width - width) / 2, width)
    } else {
        // Narrow: the timeline and the extras are centred together.
        let beside = if extras_width > 0 {
            extras_width + 2
        } else {
            0
        };
        let width = inner.saturating_sub(beside).min(MAX_TIMELINE_WIDTH);
        let x = row.x + SIDE + inner.saturating_sub(width + beside) / 2;
        (x, width)
    };
    let line = timeline_line(app, width);
    let line_width = line.width() as u16;
    let timeline = Rect {
        x: x + width.saturating_sub(line_width) / 2,
        width: line_width.min(width),
        ..row
    };
    frame.render_widget(line, timeline);
    if extras_width > 0 {
        let start = x + width + 2;
        let room = (row.x + row.width).saturating_sub(start);
        let extras = fit(&extras, usize::from(room));
        let area = Rect {
            x: start.min(row.x + row.width),
            width: (extras.width() as u16).min(room),
            ..row
        };
        frame.render_widget(Span::styled(extras, DIM), area);
    }
}

#[cfg(test)]
mod tests;
