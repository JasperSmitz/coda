//! The Now playing screen (`f`): a calm display of the track heard, in
//! place of the panels and the now-playing bar. Top to bottom: album ·
//! artist (dim), a large square cover, the title in bold with a blank row
//! on either side, the visualizer, the timeline with the repeat mark and a
//! passing volume beside it (dim), and a dim Next up line when it is due
//! ([`crate::now_playing`]). Only the cover, the title and the timeline
//! are not dim grey; the timeline is dim too unless playing or loading.
//!
//! [`screen_layout`] is pure: with too little room the cover goes first,
//! then the visualizer; the text and the timeline always fit.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::player::{PlaybackStatus, RepeatMode};
use crate::ui::cover::{ArtFrame, ArtPainter, DIACRITICS};
use crate::ui::render::{VISUALIZER_MIN_SCREEN, draw_visualizer, fit, format_time};

/// A cover smaller than this many rows is left out (the bar already shows
/// one that size).
pub const MIN_COVER_ROWS: u16 = 6;
/// The visualizer is left out rather than drawn lower than this.
pub const MIN_VISUALIZER_ROWS: u16 = 3;
/// The visualizer's height beside a cover: a fifth of the screen, within
/// these.
pub const MAX_VISUALIZER_ROWS: u16 = 10;
/// Without a cover the visualizer takes its room: a third of the screen,
/// up to this.
pub const MAX_VISUALIZER_ROWS_ALONE: u16 = 16;
/// The timeline is never wider than this.
pub const MAX_TIMELINE_WIDTH: u16 = 64;
/// Below this width the bar is left out and the timeline is just times.
const MIN_BAR: u16 = 4;
/// Columns kept free left and right of the text.
const SIDE: u16 = 2;
/// Room kept on both sides of a wide timeline for the extras beside it
/// (`⟳1  Vol 100%` and a two-column gap), so it does not move when they
/// come and go.
const EXTRAS_ROOM: u16 = 14;
/// A timeline narrower than this gives up the centring room above.
const MIN_CENTRED_TIMELINE: u16 = 24;
/// The text rows: album · artist, title, timeline and Next up, with a
/// blank row after the first and on either side of the title.
const TEXT_ROWS: u16 = 6;
const DIM: Style = Style::new().fg(Color::DarkGray);

/// Where each part of the screen goes; a part without room (or without
/// anything to show) is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScreenLayout {
    pub header: Option<Rect>,
    pub cover: Option<Rect>,
    pub title: Option<Rect>,
    pub visualizer: Option<Rect>,
    pub timeline: Option<Rect>,
    pub next: Option<Rect>,
}

/// The screen's parts in `area`. `cover` (the painter, given only when a
/// cover is to be shown) sizes a square-looking cover; `visualizer` asks
/// for the visualizer.
///
/// - Margins: one row above and below, when there is room for the text
///   rows and both.
/// - With a cover: the visualizer takes a fifth of the height
///   ([`MIN_VISUALIZER_ROWS`] to [`MAX_VISUALIZER_ROWS`]) and the cover
///   the rest, as wide as it looks square, narrowed to fit the width; a
///   cover under [`MIN_COVER_ROWS`] is left out.
/// - Without one, the visualizer takes a third of the height (up to
///   [`MAX_VISUALIZER_ROWS_ALONE`]), else it is left out.
/// - The text alone needs six rows; lower than that the blank rows go,
///   then Next up, album · artist and the timeline, in that order; the
///   title goes last.
///
/// Everything is centred vertically, the cover horizontally; the text
/// rows keep two columns free on either side, the visualizer a tenth
/// of the width (at least two columns).
pub fn screen_layout(area: Rect, cover: Option<&ArtPainter>, visualizer: bool) -> ScreenLayout {
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
        let rank = [2u16, 0, 1, 3]; // header, title, timeline, next
        let kept = rank.map(|rank| rank < height);
        let mut y = area.y;
        for (index, slot) in [
            &mut layout.header,
            &mut layout.title,
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
    layout.header = Some(Rect {
        y: take(1, 1),
        height: 1,
        ..text
    });
    if cover_rows > 0 {
        layout.cover = Some(Rect {
            y: take(cover_rows, 1),
            height: cover_rows,
            ..side(cover_cols)
        });
    }
    layout.title = Some(Rect {
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
    let cover =
        painter.filter(|_| app.covers().reserve() && app.player().playing_entry().is_some());
    screen_layout(area, cover, app.visualizer_shown())
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

/// Whether `area` has room for the visualizer, shown or not.
pub fn visualizer_room(area: Rect) -> bool {
    screen_layout(area, None, true).visualizer.is_some()
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
    let track = &entry.track;
    if let Some(rect) = layout.header {
        let header = [track.album.as_deref(), track.artist.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        let header = fit(&header, usize::from(rect.width));
        frame.render_widget(Line::styled(header, DIM).centered(), rect);
    }
    if let (Some(rect), Some(painter)) = (layout.cover, painter) {
        painter.draw(frame.buffer_mut(), rect);
    }
    if let Some(rect) = layout.title {
        let title = fit(&track.title, usize::from(rect.width));
        frame.render_widget(
            Line::styled(title, Style::new().add_modifier(Modifier::BOLD)).centered(),
            rect,
        );
    }
    if let Some(rect) = layout.visualizer {
        draw_visualizer(frame.buffer_mut(), app, rect);
    }
    if let Some(rect) = layout.timeline {
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
    if let (Some(rect), Some(next)) = (layout.next, app.now_playing().up_next(player, now)) {
        let mut line = format!("Next: {}", next.track.title);
        if let Some(artist) = &next.track.artist {
            line.push_str(" — ");
            line.push_str(artist);
        }
        frame.render_widget(
            Line::styled(fit(&line, usize::from(rect.width)), DIM).centered(),
            rect,
        );
    }
    layout.visualizer
}

/// The repeat mark and the passing volume, for beside the timeline.
fn extras(app: &App, now: Instant) -> String {
    let player = app.player();
    let repeat = match player.repeat() {
        RepeatMode::Off => None,
        RepeatMode::All => Some("⟳".to_owned()),
        RepeatMode::One => Some("⟳1".to_owned()),
    };
    let volume = app
        .now_playing()
        .volume_shown(now)
        .then(|| format!("Vol {}%", player.volume()));
    [repeat, volume]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("  ")
}

/// Elapsed time, a bar and the duration, centred in `row` (the screen's
/// width) and at most [`MAX_TIMELINE_WIDTH`] wide, with the extras to its
/// right. Dim unless playing or loading.
fn draw_timeline(frame: &mut Frame, app: &App, row: Rect, now: Instant) {
    let player = app.player();
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
    // Paused, stopped (a restored session, the end of the queue) or
    // unavailable: nothing moves, so it is dim.
    let still = !matches!(
        player.status(),
        PlaybackStatus::Playing | PlaybackStatus::Loading
    );
    let (text, filled, unfilled) = if still {
        (DIM, DIM, DIM)
    } else {
        (Style::new(), Style::new().fg(Color::Cyan), DIM)
    };
    let entry = player.playing_entry();
    let duration = player
        .duration()
        .or_else(|| entry.and_then(|e| e.track.duration));
    let position = player.position().unwrap_or_default();
    let elapsed = format_time(position);
    let total = duration.map_or_else(|| "--:--".to_owned(), format_time);
    let times = (elapsed.width() + total.width() + 2) as u16;
    let mut spans = Vec::new();
    if width >= times + MIN_BAR {
        let bar = width - times;
        let ratio = match duration {
            Some(total) if !total.is_zero() => {
                (position.as_secs_f64() / total.as_secs_f64()).clamp(0.0, 1.0)
            }
            _ => 0.0,
        };
        let done = ((ratio * f64::from(bar)).round() as u16).min(bar);
        spans.push(Span::styled(format!("{elapsed} "), text));
        spans.push(Span::styled("━".repeat(usize::from(done)), filled));
        spans.push(Span::styled("─".repeat(usize::from(bar - done)), unfilled));
        spans.push(Span::styled(format!(" {total}"), text));
    } else {
        spans.push(Span::styled(
            fit(&format!("{elapsed} / {total}"), usize::from(width)),
            text,
        ));
    }
    let line = Line::from(spans);
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
