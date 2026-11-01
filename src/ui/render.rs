//! Draws the [`App`] with Ratatui: Library and Queue panels, the
//! now-playing bar, the status line, the help overlay and the Settings
//! overlay (`crate::ui::settings`).
//!
//! Untrusted names (tags, file names, server responses, the search input)
//! are sanitised and truncated by display width, so control characters and
//! wide characters cannot break the layout.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, LineGauge, List, ListItem, ListState, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::{
    App, Connection, Focus, LocalSearch, NAVIDROME_NOT_CONFIGURED, NO_LOCAL_LIBRARY,
    PRESS_ENTER_TO_CONNECT, ScanState, Source, count, thousands,
};
use crate::library::browse::{Row as LocalRow, View as LocalView};
use crate::library::{Library, LocalPlaylist};
use crate::model::Track;
use crate::navidrome::browse::{Browser, Load, More, Resource, Row};
use crate::player::{PlaybackStatus, RepeatMode};
use crate::ui::{help, settings};

/// Below this width only the focused panel is shown.
pub const NARROW_WIDTH: u16 = 60;
const HIGHLIGHT_SYMBOL: &str = "> ";
const NOW_PLAYING_HEIGHT: u16 = 4;
/// Marks tracks streamed from Navidrome in the queue and now-playing bar.
/// U+2601 has no emoji presentation by default, so terminals draw it as a
/// one-column text glyph (and `unicode-width` agrees).
pub const REMOTE_MARKER: &str = "☁";
const SEARCH_PROMPT: &str = "Search: ";
/// Shown right-aligned on the search input line when it fits.
const SEARCH_HINT: &str = "Enter search · Esc cancel";

/// Widget state that persists between frames: the first visible row of
/// each panel, so the view does not jump between frames.
#[derive(Debug, Default)]
pub struct ViewState {
    library_offset: usize,
    local_search_offset: usize,
    remote_offset: usize,
    queue_offset: usize,
    /// Navidrome list rows shown in the last frame (0 if none).
    pub remote_rows: usize,
    /// The largest useful help scroll for the last frame's size.
    pub help_max_scroll: usize,
    /// Advances the settings spinner, one step per frame.
    spinner: usize,
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
        view.help_max_scroll = help::draw(frame, frame.area(), app.help_scroll());
    }
    if app.settings_open() {
        if app.settings().busy().is_some() {
            view.spinner = view.spinner.wrapping_add(1);
        }
        settings::draw(frame, app.settings(), app.connection(), view.spinner);
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
    if let (Source::Navidrome, Some(browser)) = (app.source(), app.browser()) {
        draw_remote(frame, app, browser, view, area, focused);
        return;
    }
    if app.navidrome_prompt() {
        let block = panel_block("Navidrome".to_owned(), focused, area.width);
        let body = block.inner(area);
        frame.render_widget(block, area);
        let width = usize::from(body.width);
        let mut lines = vec![Line::raw(fit(NAVIDROME_NOT_CONFIGURED, width))];
        if focused {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                fit(PRESS_ENTER_TO_CONNECT, width),
                Style::default().fg(Color::Cyan),
            ));
        }
        frame.render_widget(Paragraph::new(lines), body);
        return;
    }
    if app.scan_state() == ScanState::NoLibrary {
        let block = panel_block("Local".to_owned(), focused, area.width);
        let body = block.inner(area);
        frame.render_widget(block, area);
        let line = fit(NO_LOCAL_LIBRARY, usize::from(body.width));
        frame.render_widget(Paragraph::new(line), body);
        return;
    }
    let browser = app.local_browser();
    let library = app.library();
    let title = match app.local_search() {
        Some(search) => local_search_title(search),
        None => {
            let prefix = match app.scan_state() {
                ScanState::Scanning { .. } => "Local (scanning…)",
                _ => "Local",
            };
            path_title(prefix, &browser.path_titles(library), area.width)
        }
    };
    let block = panel_block(title, focused, area.width);
    let mut body = block.inner(area);
    frame.render_widget(block, area);
    if let Some(query) = app.local_input() {
        let [input_area, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(body);
        draw_search_input(frame, query, input_area);
        body = rest;
    }
    if let Some(search) = app.local_search() {
        draw_local_search(frame, search, view, body, focused);
        return;
    }

    let view_shown = browser.current();
    if browser.is_empty(library) {
        let scanning = matches!(app.scan_state(), ScanState::Scanning { .. });
        let message = match view_shown {
            _ if scanning => "Scanning…".to_owned(),
            LocalView::Playlists => "No playlists (.m3u or .m3u8 files)".to_owned(),
            LocalView::Playlist(id) => match library.playlist(*id).missing {
                0 => "Empty playlist".to_owned(),
                n => format!("No tracks found ({} missing)", thousands(n)),
            },
            _ => format!("No music found in {}", app.music_dir().to_string_lossy()),
        };
        let line = Line::styled(
            fit(&message, usize::from(body.width)),
            Style::default().fg(Color::DarkGray),
        );
        frame.render_widget(Paragraph::new(line), body);
        return;
    }

    let width = usize::from(body.width).saturating_sub(HIGHLIGHT_SYMBOL.width());
    let height = usize::from(body.height);
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
            ListItem::new(local_row(library, view_shown, row, width))
        })
        .collect();
    let mut state =
        ListState::default().with_selected(Some(app.library_selected().saturating_sub(offset)));
    frame.render_stateful_widget(list_widget(items, Block::new(), focused), body, &mut state);
}

/// One row of a local view.
fn local_row(
    library: &Library,
    view: &LocalView,
    row: LocalRow<'_>,
    width: usize,
) -> Line<'static> {
    match row {
        LocalRow::Home(entry) => Line::raw(fit(entry.label(), width)),
        LocalRow::Dir(dir) => Line::styled(
            fit(&format!("{}/", dir.name()), width),
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        ),
        LocalRow::Track(_, track) => {
            let numbered = |text: String| match track.track_no {
                Some(no) => format!("{no:02} {text}"),
                None => text,
            };
            let left = match view {
                LocalView::Playlist(_) => track.display_name(),
                // A compilation: name the track's own artist.
                LocalView::Album(id) => {
                    let album = library.tags().album(*id);
                    let album_artist = library.tags().artist(album.artist()).name();
                    match &track.artist {
                        Some(artist) if artist.to_lowercase() != album_artist.to_lowercase() => {
                            numbered(track.display_name())
                        }
                        _ => numbered(track.title.clone()),
                    }
                }
                _ => numbered(track.title.clone()),
            };
            row_line(&left, track.duration.map(format_time), width)
        }
        LocalRow::Artist(artist) => row_line(
            artist.name(),
            Some(count(artist.albums().len(), "album")),
            width,
        ),
        LocalRow::Album(album, artist) => {
            let left = match view {
                LocalView::Albums(_) => format!("{} — {}", album.name(), artist.name()),
                _ => album.name().to_owned(),
            };
            row_line(&left, Some(count(album.tracks().len(), "track")), width)
        }
        LocalRow::Playlist(playlist) => {
            row_line(&playlist.name, Some(playlist_counts(playlist)), width)
        }
    }
}

/// `12 tracks`, or `12 tracks, 3 missing`.
fn playlist_counts(playlist: &LocalPlaylist) -> String {
    let tracks = count(playlist.tracks.len(), "track");
    match playlist.missing {
        0 => tracks,
        n => format!("{tracks}, {} missing", thousands(n)),
    }
}

/// `Local: 12 results for "blue"`; with more matches than are kept,
/// `1,000 of 2,345 results`; while the scan was still running, the title
/// says only the tracks scanned so far were searched. The query comes
/// last, so a narrow panel cuts it rather than the counts.
fn local_search_title(search: &LocalSearch) -> String {
    let results = &search.results;
    let shown = results.tracks.len();
    let counts = if shown < results.total {
        format!("{} of {}", thousands(shown), count(results.total, "result"))
    } else {
        count(results.total, "result")
    };
    let scope = if search.partial {
        " (scanned so far)"
    } else {
        ""
    };
    format!("Local{scope}: {counts} for \"{}\"", results.query)
}

fn draw_local_search(
    frame: &mut Frame,
    search: &LocalSearch,
    view: &mut ViewState,
    area: Rect,
    focused: bool,
) {
    let tracks = &search.results.tracks;
    if tracks.is_empty() {
        let line = Line::styled(
            fit("No matches (h or Esc to go back)", usize::from(area.width)),
            Style::default().fg(Color::DarkGray),
        );
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    let width = usize::from(area.width).saturating_sub(HIGHLIGHT_SYMBOL.width());
    let height = usize::from(area.height);
    let offset = scroll_offset(
        view.local_search_offset,
        search.selected,
        tracks.len(),
        height,
    );
    view.local_search_offset = offset;
    let items: Vec<ListItem> = tracks
        .iter()
        .skip(offset)
        .take(height)
        .map(|track| {
            note_row_built();
            ListItem::new(row_line(
                &track.display_name(),
                track.duration.map(format_time),
                width,
            ))
        })
        .collect();
    let mut state =
        ListState::default().with_selected(Some(search.selected.saturating_sub(offset)));
    frame.render_stateful_widget(list_widget(items, Block::new(), focused), area, &mut state);
}

/// Separates the views of the path in a library panel title.
pub const PATH_SEPARATOR: &str = " › ";

/// `prefix › view › view`, fitted to a panel of `width` columns; just
/// `prefix` at Home. When too long, the start of the path is cut so the
/// shown view stays visible.
fn path_title(prefix: &str, path: &[String], width: u16) -> String {
    if path.is_empty() {
        return prefix.to_owned();
    }
    let prefix = format!("{prefix}{PATH_SEPARATOR}");
    let room = usize::from(width.saturating_sub(4)).saturating_sub(prefix.width());
    format!("{prefix}{}", fit_tail(&path.join(PATH_SEPARATOR), room))
}

/// `Navidrome (<state>) › Albums (newest) › <album>`, fitted to a panel
/// of `width` columns; Home is the bare `Navidrome (<state>)`. When too
/// long, the start of the path is cut so the shown view stays visible.
fn remote_title(connection: &Connection, browser: &Browser, width: u16) -> String {
    let state = match connection {
        Connection::Connecting => "connecting…",
        Connection::Connected => "connected",
        Connection::Failed(_) => "not connected",
    };
    let path: Vec<String> = browser
        .stack()
        .iter()
        .filter(|v| v.resource != Resource::Home)
        .map(|v| browser.title(v))
        .collect();
    path_title(&format!("Navidrome ({state})"), &path, width)
}

fn draw_remote(
    frame: &mut Frame,
    app: &App,
    browser: &Browser,
    view: &mut ViewState,
    area: Rect,
    focused: bool,
) {
    let title = remote_title(app.connection(), browser, area.width);
    let block = panel_block(title, focused, area.width);
    let mut body = block.inner(area);
    frame.render_widget(block, area);
    if let Some(query) = browser.input() {
        let [input_area, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(body);
        draw_search_input(frame, query, input_area);
        body = rest;
    }
    let message = |frame: &mut Frame, text: &str, style: Style| {
        let line = Line::styled(fit(text, usize::from(body.width)), style);
        frame.render_widget(Paragraph::new(line), body);
    };
    view.remote_rows = 0;
    let Some(current) = browser.current() else {
        return;
    };
    match &current.load {
        Load::Loading(_) => message(frame, "Loading…", Style::default().fg(Color::Yellow)),
        Load::Failed(error) => message(
            frame,
            &format!("Error: {error} (r to retry)"),
            Style::default().fg(Color::Red),
        ),
        Load::Ready => {
            let Some(content) = browser.content(current) else {
                return;
            };
            if content.is_empty() {
                let text = match current.resource {
                    Resource::Home => "",
                    Resource::Artists => "No artists",
                    Resource::Artist(_) | Resource::Albums(_) | Resource::AlbumPage { .. } => {
                        "No albums"
                    }
                    Resource::Album(_) | Resource::Playlist(_) => "No songs",
                    Resource::Search(_) => "No results",
                    Resource::Playlists => "No playlists",
                };
                message(frame, text, Style::default().fg(Color::DarkGray));
                return;
            }
            let footer = match browser.more(current) {
                More::Idle => None,
                More::Loading => Some(Line::styled(
                    fit("Loading more…", usize::from(body.width)),
                    Style::default().fg(Color::Yellow),
                )),
                More::Failed(_) => Some(Line::styled(
                    fit("Could not load more — r to retry", usize::from(body.width)),
                    Style::default().fg(Color::Red),
                )),
            };
            if let Some(footer) = footer
                && body.height > 1
            {
                let [list, footer_area] =
                    Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(body);
                frame.render_widget(Paragraph::new(footer), footer_area);
                body = list;
            }
            let style = RowStyle::of(&current.resource);
            let width = usize::from(body.width).saturating_sub(HIGHLIGHT_SYMBOL.width());
            let height = usize::from(body.height);
            view.remote_rows = height;
            let offset = scroll_offset(view.remote_offset, current.selected, content.len(), height);
            view.remote_offset = offset;
            let items: Vec<ListItem> = content
                .rows(offset, height)
                .into_iter()
                .map(|row| {
                    note_row_built();
                    ListItem::new(remote_row(row, style, width))
                })
                .collect();
            let mut state =
                ListState::default().with_selected(Some(current.selected.saturating_sub(offset)));
            frame.render_stateful_widget(
                list_widget(items, Block::new(), focused),
                body,
                &mut state,
            );
        }
    }
}

/// How rows of a remote view show their item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowStyle {
    /// Albums with their artist (lists across artists).
    album_artist: bool,
    /// Songs as `artist - title` rather than `NN title` (lists that are
    /// not one album).
    song_artist: bool,
}

impl RowStyle {
    fn of(resource: &Resource) -> Self {
        Self {
            album_artist: matches!(
                resource,
                Resource::Search(_) | Resource::Albums(_) | Resource::AlbumPage { .. }
            ),
            song_artist: matches!(resource, Resource::Search(_) | Resource::Playlist(_)),
        }
    }
}

fn remote_row(row: Row<'_>, style: RowStyle, width: usize) -> Line<'static> {
    match row {
        Row::Header(section) => Line::styled(
            fit(section.label(), width),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Row::Home(entry) => Line::raw(fit(entry.label(), width)),
        Row::Playlist(playlist) => {
            let songs = playlist.song_count.map(|n| count(n as usize, "song"));
            let right = match (songs, playlist.duration.map(format_time)) {
                (Some(songs), Some(time)) => Some(format!("{songs} · {time}")),
                (songs, time) => songs.or(time),
            };
            row_line(&playlist.name, right, width)
        }
        Row::Artist(artist) => row_line(
            &artist.name,
            artist.album_count.map(|n| count(n as usize, "album")),
            width,
        ),
        Row::Album(album) => {
            let left = match (&album.artist, style.album_artist) {
                (Some(artist), true) => format!("{} — {artist}", album.name),
                _ => album.name.clone(),
            };
            row_line(&left, album.year.map(|y| y.to_string()), width)
        }
        Row::Song(track) => {
            let left = if style.song_artist {
                track.display_name()
            } else {
                match track.track_no {
                    Some(no) => format!("{no:02} {}", track.title),
                    None => track.title.clone(),
                }
            };
            row_line(&left, track.duration.map(format_time), width)
        }
    }
}

/// The one-line search input: the prompt and the end of the query (the
/// part being typed), with the terminal cursor after it.
fn draw_search_input(frame: &mut Frame, query: &str, area: Rect) {
    let room = usize::from(area.width).saturating_sub(SEARCH_PROMPT.width() + 1);
    let shown = fit_tail(query, room);
    let cursor_x = SEARCH_PROMPT.width() + shown.width();
    let mut spans = vec![
        Span::styled(SEARCH_PROMPT, Style::default().fg(Color::Cyan)),
        Span::raw(shown),
    ];
    // The hint only appears with at least two spare columns before it.
    let spare = usize::from(area.width).saturating_sub(cursor_x + 1);
    if spare >= SEARCH_HINT.width() + 2 {
        spans.push(Span::raw(" ".repeat(spare - SEARCH_HINT.width())));
        spans.push(Span::styled(
            SEARCH_HINT,
            Style::default().fg(Color::DarkGray),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    if area.width > 0 && area.height > 0 {
        let x = area.x + (cursor_x as u16).min(area.width - 1);
        frame.set_cursor_position((x, area.y));
    }
}

/// Like [`fit`], but keeps the end of `text`, starting with `…` when
/// shortened. Bounded the same way: only the last [`MAX_FIT_CHARS`] chars
/// are considered and the cut is found by binary search over measured
/// suffixes.
pub fn fit_tail(text: &str, width: usize) -> String {
    let skip = text.chars().count().saturating_sub(MAX_FIT_CHARS);
    let clean: String = text
        .chars()
        .skip(skip)
        .map(|c| if is_unsafe(c) { ' ' } else { c })
        .collect();
    if skip == 0 && clean.width() <= width {
        return clean;
    }
    if width == 0 {
        return String::new();
    }
    let budget = width - 1;
    let starts: Vec<usize> = clean
        .char_indices()
        .map(|(i, _)| i)
        .chain([clean.len()])
        .collect();
    // Smallest k whose suffix (from starts[k]) fits; the empty suffix
    // always fits.
    let (mut lo, mut hi) = (0, starts.len() - 1);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if clean[starts[mid]..].width() <= budget {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    format!("…{}", &clean[starts[lo]..])
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
                queue_right(&entry.track),
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

/// The right column of a queue row: the remote marker and the duration.
fn queue_right(track: &Track) -> Option<String> {
    let time = track.duration.map(format_time);
    if !track.is_remote() {
        return time;
    }
    Some(match time {
        Some(time) => format!("{REMOTE_MARKER} {time}"),
        None => REMOTE_MARKER.to_owned(),
    })
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
    let mut room = usize::from(info.width).saturating_sub(label.width());
    if entry.is_some_and(|e| e.track.is_remote()) {
        let marker = fit(&format!("{REMOTE_MARKER} "), room);
        room = room.saturating_sub(marker.width());
        spans.push(Span::styled(marker, Style::default().fg(Color::Cyan)));
    }
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

    let repeat = match player.repeat() {
        RepeatMode::Off => String::new(),
        mode => format!(" Repeat {}", mode.label()),
    };
    let volume = format!("{repeat} Vol {:>3}%", player.volume());
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
        ScanState::Finished { .. } | ScanState::NoLibrary => "? help ".to_owned(),
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
    use crate::ui::help;
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

    /// From the local Home (as a new app starts), opens Folders.
    fn open_folders(app: &mut App, now: Instant) {
        app.handle_action(Action::Down, now);
        app.handle_action(Action::Activate, now);
        assert_eq!(app.library_path(), [] as [String; 0]);
        assert!(matches!(
            app.local_browser().current(),
            crate::library::browse::View::Folder(_)
        ));
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
    fn navidrome_only_shows_no_local_library_and_ping_status() {
        let now = Instant::now();
        let client = crate::navidrome::Client::new(
            crate::navidrome::client::parse_base_url("https://h.example").unwrap(),
            "u".into(),
            crate::navidrome::Password::new("p".into()),
        )
        .unwrap();
        let mut app = App::with_sources(None, Some(client));
        let screen = render(&app, 100, 20, now);
        assert!(screen.contains("Navidrome (connecting…) "), "{screen}");
        assert!(!screen.contains("Scanning"), "{screen}");
        assert!(screen.contains("? help"), "{screen}");
        app.handle_action(Action::SelectLocal, now);
        let screen = render(&app, 100, 20, now);
        assert!(
            screen.contains("No local music directory configured"),
            "{screen}"
        );
        app.handle_navidrome_ping(Ok(()), now);
        let screen = render(&app, 100, 20, now);
        assert!(screen.contains("Navidrome connected"), "{screen}");
        app.handle_navidrome_ping(Err(crate::navidrome::Error::Auth), now);
        let screen = render(&app, 100, 20, now);
        assert!(
            screen.contains("Navidrome: authentication failed"),
            "{screen}"
        );
    }

    #[test]
    fn navidrome_prompt_and_settings_overlay() {
        let now = Instant::now();
        let mut app = App::new("/music".into());
        app.handle_action(Action::SelectNavidrome, now);
        let screen = render(&app, 100, 20, now);
        assert!(screen.contains("Navidrome is not configured"), "{screen}");
        assert!(screen.contains("Press Enter to connect"), "{screen}");
        // The prompt is only offered while the panel is focused.
        app.handle_action(Action::SwitchFocus, now);
        let screen = render(&app, 100, 20, now);
        assert!(screen.contains("Navidrome is not configured"), "{screen}");
        assert!(!screen.contains("Press Enter"), "{screen}");
        app.handle_action(Action::OpenSettings, now);
        let screen = render(&app, 100, 24, now);
        assert!(screen.contains(" Settings "), "{screen}");
        assert!(screen.contains("Music folder  /music"), "{screen}");
        // Help lists the key.
        let mut app = App::new("/music".into());
        app.handle_action(Action::ToggleHelp, now);
        let screen = render(&app, 80, 24, now);
        assert!(screen.contains("Settings (Navidrome)"), "{screen}");
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
        assert!(screen.contains("│> Playlists"), "Home first: {screen}");
        open_folders(&mut app, now);
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
        assert!(screen.contains("┌ Local (scanning…) "), "{screen}");
        open_folders(&mut app, now);
        let screen = render(&app, 100, 20, now);
        assert!(screen.contains("│Scanning…"), "{screen}");
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
        open_folders(&mut app, now);
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
        for text in [
            "Move entry down / up",
            "Select current entry",
            "Shuffle upcoming",
            "Clear queue",
            "Repeat: off / all / one",
            "Streamed from Navidrome",
        ] {
            assert!(screen.contains(text), "{text}: {screen}");
        }
        // Also fits a tiny terminal without panicking.
        render(&app, 10, 5, now);
    }

    #[test]
    fn every_help_binding_is_reachable_at_80x24_and_smaller() {
        let now = Instant::now();
        for (width, height) in [(80, 24), (80, 12), (60, 8)] {
            let mut app = App::new("/music".into());
            app.handle_action(Action::ToggleHelp, now);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut view = ViewState::default();
            let mut seen = String::new();
            for _ in 0..help::BINDINGS.len() {
                terminal
                    .draw(|frame| draw(frame, &app, &mut view, now))
                    .unwrap();
                app.clamp_help_scroll(view.help_max_scroll);
                seen.push_str(&buffer_text(&terminal));
                app.handle_action(Action::Down, now);
            }
            for (key, what) in help::BINDINGS {
                assert!(seen.contains(what), "{width}x{height}: {key} {what}");
            }
            let fits = view.help_max_scroll == 0;
            assert_eq!(fits, height == 24, "{width}x{height}");
        }
    }

    #[test]
    fn now_playing_shows_the_repeat_mode() {
        let now = Instant::now();
        let mut app = App::new("/music".into());
        let screen = render(&app, 100, 20, now);
        assert!(!screen.contains("Repeat"), "off is not shown: {screen}");
        assert!(screen.contains("Vol 100%"), "{screen}");
        app.handle_action(Action::CycleRepeat, now);
        let screen = render(&app, 100, 20, now);
        assert!(screen.contains("Repeat all Vol 100%"), "{screen}");
        assert!(screen.contains("Repeat all"), "status line: {screen}");
        app.handle_action(Action::CycleRepeat, now);
        let screen = render(&app, 100, 20, now);
        assert!(screen.contains("Repeat one Vol 100%"), "{screen}");
        // Narrow terminals do not break the layout.
        render(&app, 12, 8, now);
        app.handle_action(Action::CycleRepeat, now);
        assert!(!render(&app, 100, 20, now).contains("Repeat one"));
    }

    #[test]
    fn narrow_layout_shows_only_the_focused_panel() {
        let now = Instant::now();
        let mut app = App::new("/music".into());
        app.handle_scan_event(ScanEvent::Batch(vec![tagged("a.flac", "A", "B", 1)]), now);
        let wide = render(&app, 80, 20, now);
        assert!(
            wide.contains("Local") && wide.contains("Queue (0)"),
            "{wide}"
        );
        let narrow = render(&app, 59, 20, now);
        assert!(narrow.contains("Local"), "{narrow}");
        assert!(!narrow.contains("Queue (0)"), "{narrow}");
        app.handle_action(Action::SwitchFocus, now);
        let narrow = render(&app, 59, 20, now);
        assert!(narrow.contains("Queue (0)"), "{narrow}");
        assert!(!narrow.contains("Local"), "{narrow}");
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
    fn fit_tail_keeps_the_end_and_never_exceeds_the_width() {
        assert_eq!(fit_tail("hello", 5), "hello");
        assert_eq!(fit_tail("hello", 4), "…llo");
        assert_eq!(fit_tail("hello", 0), "");
        assert_eq!(fit_tail("雨雨雨", 4), "…雨");
        for text in TRICKY {
            for width in 0..=40 {
                let out = fit_tail(text, width);
                assert!(out.width() <= width, "{text:?} at {width}: {out:?}");
                assert!(!out.chars().any(is_unsafe), "{out:?}");
            }
        }
        for text in hostile_inputs() {
            for width in [0, 1, 10, 200] {
                let started = Instant::now();
                let out = fit_tail(&text, width);
                assert!(out.width() <= width);
                assert!(started.elapsed() < Duration::from_secs(1));
            }
        }
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
        open_folders(&mut app, now);
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
        open_folders(&mut app, now);
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
        let mut app = queue_app(10_000, |i| format!("song{i:05}"), now);
        let mut view = ViewState::default();
        // 14 rows - now playing (4) - status (1) = 9 panel rows, 7 inner.
        let mut terminal = Terminal::new(TestBackend::new(100, 14)).unwrap();
        draw_to(&mut terminal, &app, &mut view, now);
        let screen = buffer_text(&terminal);
        assert!(screen.contains("▶ x - song00000"), "{screen}");

        for _ in 0..9_995 {
            app.handle_action(Action::Down, now);
        }
        let built = rows_built_by(|| draw_to(&mut terminal, &app, &mut view, now));
        let screen = buffer_text(&terminal);
        // Selection is the last visible row; the window ends there.
        assert_eq!(view.queue_offset, 9_995 + 1 - 7);
        assert!(screen.contains("x - song09989"), "{screen}");
        assert!(
            screen.contains("> ") && screen.contains("x - song09995"),
            "{screen}"
        );
        assert!(!screen.contains("x - song09996"), "{screen}");
        assert!(!screen.contains("x - song00000"), "{screen}");
        assert!(!screen.contains('▶'), "{screen}");
        // Work scales with the viewport: queue rows plus library rows.
        assert!(built <= 2 * 7, "built {built} rows");

        // Selecting the playing entry again shows its marker.
        for _ in 0..9_995 {
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
        let mut app = queue_app(10_000, |i| hostile[i % hostile.len()].clone(), now);
        for _ in 0..9_990 {
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
        open_folders(&mut app, now);
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

    // ----- local search ------------------------------------------------

    fn search_local(app: &mut App, query: &str, now: Instant) {
        app.handle_action(Action::OpenSearch, now);
        for c in query.chars() {
            app.handle_action(Action::InputChar(c), now);
        }
        app.handle_action(Action::InputSubmit, now);
    }

    fn finish_scan(app: &mut App, tracks: usize, now: Instant) {
        app.handle_scan_event(
            ScanEvent::Finished {
                tracks,
                skipped: 0,
                errors: 0,
            },
            now,
        );
    }

    #[test]
    fn local_search_input_and_results() {
        let now = Instant::now();
        let mut app = App::new("/music".into());
        app.handle_scan_event(
            ScanEvent::Batch(vec![
                tagged("a/1.flac", "Blue in Green", "Miles Davis", 337),
                tagged("a/2.flac", "So What", "Miles Davis", 562),
                tagged(
                    "b/3.flac",
                    "\u{202E}Blue\u{1b}[31m Evil",
                    "❤️".repeat(40).as_str(),
                    60,
                ),
            ]),
            now,
        );
        open_folders(&mut app, now);
        // The input line sits at the top of the Local panel, with its hint.
        app.handle_action(Action::OpenSearch, now);
        for c in "blue".chars() {
            app.handle_action(Action::InputChar(c), now);
        }
        let mut terminal = Terminal::new(TestBackend::new(100, 14)).unwrap();
        let mut view = ViewState::default();
        draw_to(&mut terminal, &app, &mut view, now);
        let screen = buffer_text(&terminal);
        assert!(screen.contains("│Search: blue"), "{screen}");
        assert!(screen.contains("Enter search · Esc cancel"), "{screen}");
        assert!(screen.contains("a/"), "the tree stays visible: {screen}");
        terminal
            .backend_mut()
            .assert_cursor_position((1 + 8 + 4, 1));
        app.handle_action(Action::InputSubmit, now);
        let screen = render(&app, 100, 14, now);
        assert!(!screen.contains("Search: "), "{screen}");
        assert!(
            screen.contains("Local (scanned so far): 2 results for \"blue\""),
            "{screen}"
        );
        assert!(screen.contains("> Miles Davis - Blue in Green"), "{screen}");
        assert!(screen.contains("5:37"), "{screen}");
        assert!(!screen.contains("So What"), "{screen}");
        assert!(
            !screen.contains('\u{1b}') && !screen.contains('\u{202E}'),
            "{screen}"
        );
        for width in [20, 59, 100] {
            assert_full_width(&render(&app, width, 14, now), width);
        }
        // After the scan, the title no longer qualifies the results.
        finish_scan(&mut app, 3, now);
        search_local(&mut app, "nothing here", now);
        let screen = render(&app, 100, 14, now);
        assert!(
            screen.contains("Local: 0 results for \"nothing here\""),
            "{screen}"
        );
        assert!(
            screen.contains("No matches (h or Esc to go back)"),
            "{screen}"
        );
        // Back to the tree.
        app.handle_action(Action::Back, now);
        let screen = render(&app, 100, 14, now);
        assert!(screen.contains("┌ Local › Folders ─"), "{screen}");
        assert!(screen.contains("> a/"), "{screen}");
    }

    #[test]
    fn local_search_results_are_capped_and_scroll_with_the_selection() {
        let now = Instant::now();
        let mut app = App::new("/music".into());
        let tracks: Vec<Track> = (0..1234)
            .map(|i| tagged(&format!("d/{i:04}.flac"), &format!("Song {i:04}"), "x", 1))
            .collect();
        app.handle_scan_event(ScanEvent::Batch(tracks), now);
        finish_scan(&mut app, 1234, now);
        search_local(&mut app, "song", now);
        let mut terminal = Terminal::new(TestBackend::new(100, 14)).unwrap();
        let mut view = ViewState::default();
        let built = rows_built_by(|| draw_to(&mut terminal, &app, &mut view, now));
        let screen = buffer_text(&terminal);
        assert!(
            screen.contains("Local: 1,000 of 1,234 results for \"song\""),
            "{screen}"
        );
        assert!(built < 30, "only visible rows are built: {built}");
        for _ in 0..1500 {
            app.handle_action(Action::Down, now);
        }
        draw_to(&mut terminal, &app, &mut view, now);
        let screen = buffer_text(&terminal);
        assert!(screen.contains("> x - Song 0999"), "{screen}");
        assert!(!screen.contains("Song 1000"), "{screen}");
    }

    // ----- Navidrome ---------------------------------------------------

    use crate::navidrome::browse::{Content, FetchRequest, FetchResponse};
    use crate::navidrome::{Album, Artist, SearchResults};

    fn nav_client() -> crate::navidrome::Client {
        crate::navidrome::Client::new(
            crate::navidrome::client::parse_base_url("https://secret-host.example/nd").unwrap(),
            "u".into(),
            crate::navidrome::Password::new("p".into()),
        )
        .unwrap()
    }

    /// Navidrome only, with Artists opened from Home; its request is
    /// returned unanswered.
    fn nav_app() -> (App, FetchRequest) {
        let mut app = App::with_sources(None, Some(nav_client()));
        let now = Instant::now();
        app.handle_action(Action::Down, now);
        app.handle_action(Action::Activate, now);
        let request = app.take_fetch_requests().remove(0);
        (app, request)
    }

    fn answer(
        app: &mut App,
        request: &FetchRequest,
        result: Result<Content, crate::navidrome::Error>,
    ) {
        app.handle_fetch_response(
            FetchResponse {
                generation: Default::default(),
                id: request.id,
                resource: request.resource.clone(),
                result,
            },
            Instant::now(),
        );
    }

    fn nav_artist(id: &str, name: &str) -> Artist {
        Artist {
            id: id.into(),
            name: name.into(),
            album_count: Some(3),
        }
    }

    fn nav_track(id: &str, title: &str, artist: &str, secs: u64) -> Track {
        Track::navidrome(
            id.into(),
            TrackMetadata {
                title: Some(title.into()),
                artist: Some(artist.into()),
                duration: Some(Duration::from_secs(secs)),
                ..TrackMetadata::default()
            },
        )
    }

    fn assert_full_width(screen: &str, width: u16) {
        for line in screen.lines() {
            assert_eq!(line.width(), usize::from(width), "{screen}");
        }
    }

    #[test]
    fn navidrome_loading_and_artists() {
        let now = Instant::now();
        let (mut app, request) = nav_app();
        let screen = render(&app, 100, 14, now);
        assert!(screen.contains("Loading…"), "{screen}");
        app.handle_navidrome_ping(Ok(()), now);
        answer(
            &mut app,
            &request,
            Ok(Content::Artists(vec![
                nav_artist("1", "Miles Davis"),
                nav_artist("2", "\u{202E}Evil\u{1b}[31m"),
            ])),
        );
        let screen = render(&app, 100, 14, now);
        assert!(
            screen.contains("Navidrome (connected) › Artists"),
            "{screen}"
        );
        assert!(screen.contains("> Miles Davis"), "{screen}");
        assert!(screen.contains("3 albums"), "{screen}");
        assert!(
            !screen.contains('\u{1b}') && !screen.contains('\u{202E}'),
            "{screen}"
        );
        assert!(!screen.contains("Loading…"), "{screen}");
        assert_full_width(&screen, 100);
        // Albums of an artist: breadcrumb title and loading.
        app.handle_action(Action::Activate, now);
        let screen = render(&app, 100, 14, now);
        assert!(
            screen.contains("Navidrome (connected) › Artists › Miles Davis"),
            "{screen}"
        );
        assert!(screen.contains("Loading…"), "{screen}");
        let albums = app.take_fetch_requests().remove(0);
        answer(
            &mut app,
            &albums,
            Ok(Content::Albums(vec![Album {
                id: "k".into(),
                name: "Kind of Blue".into(),
                artist: Some("Miles Davis".into()),
                year: Some(1959),
                song_count: None,
                duration: None,
            }])),
        );
        let screen = render(&app, 100, 14, now);
        assert!(screen.contains("> Kind of Blue"), "{screen}");
        assert!(screen.contains("1959"), "{screen}");
    }

    #[test]
    fn navidrome_errors_render_inline_without_urls() {
        let now = Instant::now();
        let (mut app, request) = nav_app();
        answer(&mut app, &request, Err(crate::navidrome::Error::Auth));
        app.handle_navidrome_ping(Err(crate::navidrome::Error::Auth), now);
        let screen = render(&app, 100, 14, now);
        assert!(
            screen.contains("Error: authentication failed (r to retry)"),
            "{screen}"
        );
        assert!(screen.contains("Navidrome (not connected)"), "{screen}");
        assert!(!screen.contains("secret-host"), "{screen}");
        // Hostile server message: sanitised and truncated.
        app.handle_action(Action::Refresh, now);
        let retry = app.take_fetch_requests().remove(0);
        answer(
            &mut app,
            &retry,
            Err(crate::navidrome::Error::Server {
                code: 0,
                message: format!("\u{1b}[2J{}", "❤️".repeat(200)),
            }),
        );
        for width in [40, 100] {
            let screen = render(&app, width, 14, now);
            assert!(screen.contains("Error: server error 0:"), "{screen}");
            assert!(!screen.contains('\u{1b}'), "{screen}");
            assert_full_width(&screen, width);
        }
    }

    #[test]
    fn search_input_and_results() {
        let now = Instant::now();
        let (mut app, request) = nav_app();
        answer(&mut app, &request, Ok(Content::Artists(Vec::new())));
        let screen = render(&app, 100, 14, now);
        assert!(screen.contains("No artists"), "{screen}");
        app.handle_action(Action::OpenSearch, now);
        for c in "blue\u{202E}x".chars() {
            app.handle_action(Action::InputChar(c), now);
        }
        let mut terminal = Terminal::new(TestBackend::new(100, 14)).unwrap();
        let mut view = ViewState::default();
        draw_to(&mut terminal, &app, &mut view, now);
        let screen = buffer_text(&terminal);
        assert!(screen.contains("│Search: blue x"), "{screen}");
        assert!(screen.contains("No artists"), "{screen}");
        // The cursor sits after the query (border + prompt + 6 chars).
        terminal
            .backend_mut()
            .assert_cursor_position((1 + 8 + 6, 1));
        assert!(screen.contains("Enter search · Esc cancel"), "{screen}");
        // Too narrow for the hint: only the query.
        let narrow = render(&app, 30, 14, now);
        assert!(!narrow.contains("Esc cancel"), "{narrow}");
        assert_full_width(&narrow, 30);
        // A long query shows its end.
        for _ in 0..200 {
            app.handle_action(Action::InputChar('z'), now);
        }
        app.handle_action(Action::InputChar('!'), now);
        for width in [30, 100] {
            let screen = render(&app, width, 14, now);
            assert!(screen.contains("Search: …zzz"), "{screen}");
            assert!(screen.contains("z!"), "{screen}");
            assert_full_width(&screen, width);
        }
        for _ in 0..201 {
            app.handle_action(Action::InputBackspace, now);
        }
        app.handle_action(Action::InputSubmit, now);
        let search = app.take_fetch_requests().remove(0);
        let screen = render(&app, 140, 14, now);
        assert!(!screen.contains("Search: "), "{screen}");
        assert!(
            screen.contains("Navidrome (connected) › Artists › Search \"blue x\""),
            "{screen}"
        );
        // A narrow panel cuts the start of the path, not the shown view.
        let screen = render(&app, 40, 14, now);
        assert!(
            screen.contains("(connected) › …") && screen.contains("\"blue x\" ┐"),
            "{screen}"
        );
        answer(
            &mut app,
            &search,
            Ok(Content::Search(SearchResults {
                artists: vec![nav_artist("1", "Blue Note Band")],
                albums: vec![Album {
                    id: "b".into(),
                    name: "Blue Train".into(),
                    artist: Some("John Coltrane".into()),
                    year: None,
                    song_count: None,
                    duration: None,
                }],
                tracks: vec![nav_track("s", "Blue in Green", "Miles Davis", 337)],
            })),
        );
        let screen = render(&app, 100, 14, now);
        for expected in [
            "Artists",
            "> Blue Note Band",
            "Albums",
            "Blue Train — John Coltrane",
            "Songs",
            "Miles Davis - Blue in Green",
            "5:37",
        ] {
            assert!(screen.contains(expected), "{expected}:\n{screen}");
        }
        let screen = render(&app, 20, 14, now);
        assert_full_width(&screen, 20);
    }

    #[test]
    fn remote_tracks_are_marked_in_the_queue_and_now_playing_bar() {
        let now = Instant::now();
        assert_eq!(REMOTE_MARKER.width(), 1);
        assert_eq!(REMOTE_MARKER.chars().count(), 1);
        let mut app = App::with_sources(Some("/music".into()), Some(nav_client()));
        app.handle_scan_event(
            ScanEvent::Batch(vec![tagged("l.flac", "Local Song", "Band", 61)]),
            now,
        );
        open_folders(&mut app, now);
        app.handle_action(Action::Enqueue, now);
        app.handle_action(Action::SelectNavidrome, now);
        app.handle_action(Action::Down, now);
        app.handle_action(Action::Activate, now);
        let request = app.take_fetch_requests().remove(0);
        answer(
            &mut app,
            &request,
            Ok(Content::Artists(vec![nav_artist("1", "A")])),
        );
        app.handle_action(Action::Activate, now);
        let albums = app.take_fetch_requests().remove(0);
        answer(
            &mut app,
            &albums,
            Ok(Content::Albums(vec![Album {
                id: "x".into(),
                name: "X".into(),
                artist: None,
                year: None,
                song_count: None,
                duration: None,
            }])),
        );
        app.handle_action(Action::Activate, now);
        let songs = app.take_fetch_requests().remove(0);
        let mut no_duration = nav_track("r2", "No Length", "Remote", 0);
        no_duration.duration = None;
        answer(
            &mut app,
            &songs,
            Ok(Content::Songs(vec![
                nav_track("r1", "Remote Song", "Cloud", 125),
                no_duration,
            ])),
        );
        app.handle_action(Action::Down, now);
        app.handle_action(Action::Enqueue, now);
        app.handle_action(Action::Up, now);
        let commands = app.handle_action(Action::Activate, now);
        let Some(PlaybackCommand::Load { load, .. }) = commands.first() else {
            panic!("{commands:?}");
        };
        app.handle_playback_event(PlaybackEvent::Started { load: *load }, now);
        let screen = render(&app, 100, 14, now);
        assert!(
            screen.contains("[Playing] ☁ Remote Song — Cloud"),
            "{screen}"
        );
        let lines: Vec<&str> = screen.lines().collect();
        let row = |text: &str| {
            *lines
                .iter()
                .find(|l| l.contains(text))
                .unwrap_or_else(|| panic!("{text}:\n{screen}"))
        };
        assert!(row("Band - Local Song").contains("1:01"), "{screen}");
        assert!(!row("Band - Local Song").contains('☁'), "{screen}");
        assert!(row("▶ Cloud - Remote Song").contains("☁ 2:05"), "{screen}");
        assert!(
            row("Remote - No Length")
                .trim_end_matches('│')
                .trim_end()
                .ends_with('☁')
        );
        // The marker never breaks the layout, at any width.
        for width in [10, 20, 40, 59, 61, 100] {
            for focus_queue in [false, true] {
                if focus_queue != (app.focus() == Focus::Queue) {
                    app.handle_action(Action::SwitchFocus, now);
                }
                let screen = render(&app, width, 14, now);
                assert_full_width(&screen, width);
            }
        }
    }

    // ----- local Home, tag lists and playlists ---------------------------

    fn album_track(rel: &str, title: &str, artist: &str, album_artist: &str, album: &str) -> Track {
        let some = |s: &str| (!s.is_empty()).then(|| s.to_owned());
        Track::local(
            format!("/music/{rel}").into(),
            TrackMetadata {
                title: Some(title.into()),
                artist: some(artist),
                album_artist: some(album_artist),
                album: some(album),
                track_no: rel
                    .rsplit('/')
                    .next()
                    .and_then(|f| f.get(..2)?.parse().ok()),
                duration: Some(Duration::from_secs(90)),
                ..TrackMetadata::default()
            },
        )
    }

    fn local_press(app: &mut App, actions: &[Action], now: Instant) {
        for action in actions {
            app.handle_action(*action, now);
        }
    }

    #[test]
    fn local_home_lists_and_playlists_render() {
        use crate::library::m3u::PlaylistFile;
        let now = Instant::now();
        let mut app = App::new("/music".into());
        app.handle_scan_event(
            ScanEvent::Batch(vec![
                album_track("k/01.flac", "So What", "Miles Davis", "", "Kind of Blue"),
                album_track("k/02.flac", "Freddie", "Miles Davis", "", "Kind of Blue"),
                album_track("h/01.flac", "Hit", "Guest", "Various Artists", "Hits"),
                album_track(
                    "h/02.flac",
                    "Other",
                    "Miles Davis",
                    "Various Artists",
                    "Hits",
                ),
                album_track("loose.flac", "Loose", "", "", ""),
            ]),
            now,
        );
        // Home, with the scanning note in the title.
        let screen = render(&app, 140, 16, now);
        assert!(screen.contains("┌ Local (scanning…) ─"), "{screen}");
        for (i, entry) in ["Playlists", "Folders", "Artists", "Albums"]
            .iter()
            .enumerate()
        {
            let mark = if i == 0 { "> " } else { "  " };
            assert!(screen.contains(&format!("│{mark}{entry} ")), "{screen}");
        }
        // Artists, with album counts.
        local_press(
            &mut app,
            &[Action::Down, Action::Down, Action::Activate],
            now,
        );
        let screen = render(&app, 140, 16, now);
        assert!(
            screen.contains("┌ Local (scanning…) › Artists ─"),
            "{screen}"
        );
        assert!(screen.contains("> Miles Davis"), "{screen}");
        assert!(screen.contains("Various Artists"), "{screen}");
        assert!(screen.contains("Unknown artist"), "{screen}");
        assert!(screen.contains("1 album"), "{screen}");
        // A compilation names each track's artist.
        local_press(
            &mut app,
            &[Action::Down, Action::Activate, Action::Activate],
            now,
        );
        let screen = render(&app, 140, 16, now);
        assert!(
            screen.contains("Local (scanning…) › Artists › Various Artists › Hits"),
            "{screen}"
        );
        assert!(screen.contains("> 01 Guest - Hit"), "{screen}");
        assert!(screen.contains("02 Miles Davis - Other"), "{screen}");
        // The note goes once the scan finishes; playlists come before.
        app.handle_scan_event(
            ScanEvent::Playlist(PlaylistFile {
                path: "/music/Road.m3u".into(),
                name: "Road Trip".into(),
                entries: vec!["/music/k/02.flac".into(), "/music/gone.flac".into()],
                unresolved: 2,
                cut: false,
            }),
            now,
        );
        app.handle_scan_event(
            ScanEvent::Playlist(PlaylistFile {
                path: "/music/All.m3u".into(),
                name: "All good".into(),
                entries: vec!["/music/k/01.flac".into()],
                unresolved: 0,
                cut: true,
            }),
            now,
        );
        app.handle_scan_event(
            ScanEvent::Finished {
                tracks: 5,
                skipped: 0,
                errors: 0,
            },
            now,
        );
        let screen = render(&app, 140, 16, now);
        assert!(
            screen.contains("┌ Local › Artists › Various Artists › Hits ─"),
            "{screen}"
        );
        // Albums A–Z and by artist, each row with its artist.
        for _ in 0..4 {
            app.handle_action(Action::Parent, now);
        }
        local_press(&mut app, &[Action::Down, Action::Activate], now);
        let screen = render(&app, 140, 16, now);
        assert!(screen.contains("┌ Local › Albums (A–Z) ─"), "{screen}");
        let lines: Vec<&str> = screen.lines().collect();
        let at = |text: &str| lines.iter().position(|l| l.contains(text)).unwrap();
        assert!(at("Hits — Various Artists") < at("Kind of Blue — Miles Davis"));
        assert!(at("Kind of Blue — Miles Davis") < at("Unknown album — Unknown artist"));
        assert!(lines[at("Kind of Blue")].contains("2 tracks"), "{screen}");
        app.handle_action(Action::CycleSort, now);
        let screen = render(&app, 140, 16, now);
        assert!(
            screen.contains("┌ Local › Albums (by artist) ─"),
            "{screen}"
        );
        let lines: Vec<&str> = screen.lines().collect();
        let at = |text: &str| lines.iter().position(|l| l.contains(text)).unwrap();
        assert!(at("Kind of Blue — Miles Davis") < at("Hits — Various Artists"));
        // Playlists with their counts, A–Z.
        app.handle_action(Action::Parent, now);
        local_press(
            &mut app,
            &[Action::Up, Action::Up, Action::Up, Action::Activate],
            now,
        );
        let screen = render(&app, 140, 16, now);
        assert!(screen.contains("┌ Local › Playlists ─"), "{screen}");
        let lines: Vec<&str> = screen.lines().collect();
        let all = lines.iter().position(|l| l.contains("All good")).unwrap();
        let road = lines.iter().position(|l| l.contains("Road Trip")).unwrap();
        assert!(all < road, "{screen}");
        assert!(lines[all].contains("1 track│"), "{screen}");
        assert!(lines[road].contains("1 track, 3 missing"), "{screen}");
        // A playlist cut at the cap says so; its tracks show the artist.
        app.handle_action(Action::Activate, now);
        let screen = render(&app, 140, 16, now);
        assert!(
            screen.contains("Local › Playlists › All good (cut: file too long)"),
            "{screen}"
        );
        assert!(screen.contains("> Miles Davis - So What"), "{screen}");
        for width in [20, 59, 100] {
            assert_full_width(&render(&app, width, 16, now), width);
        }
    }

    #[test]
    fn empty_local_lists_explain_themselves() {
        use crate::library::m3u::PlaylistFile;
        let now = Instant::now();
        let mut app = App::new("/music".into());
        local_press(&mut app, &[Action::Activate], now);
        let screen = render(&app, 100, 14, now);
        assert!(screen.contains("│Scanning…"), "{screen}");
        app.handle_scan_event(
            ScanEvent::Playlist(PlaylistFile {
                path: "/music/x.m3u".into(),
                name: "Gone".into(),
                entries: Vec::new(),
                unresolved: 4,
                cut: false,
            }),
            now,
        );
        app.handle_scan_event(
            ScanEvent::Finished {
                tracks: 0,
                skipped: 0,
                errors: 0,
            },
            now,
        );
        app.handle_action(Action::Activate, now);
        let screen = render(&app, 100, 14, now);
        assert!(screen.contains("No tracks found (4 missing)"), "{screen}");
        app.handle_action(Action::Parent, now);
        app.handle_action(Action::Parent, now);
        local_press(
            &mut app,
            &[Action::Down, Action::Down, Action::Activate],
            now,
        );
        let screen = render(&app, 100, 14, now);
        assert!(screen.contains("No music found in /music"), "{screen}");
        let mut app = App::new("/music".into());
        app.handle_scan_event(
            ScanEvent::Finished {
                tracks: 0,
                skipped: 0,
                errors: 0,
            },
            now,
        );
        local_press(&mut app, &[Action::Activate], now);
        let screen = render(&app, 100, 14, now);
        assert!(
            screen.contains("No playlists (.m3u or .m3u8 files)"),
            "{screen}"
        );
    }

    #[test]
    fn a_large_local_album_list_renders_only_visible_rows() {
        let now = Instant::now();
        let mut app = App::new("/music".into());
        let tracks: Vec<Track> = (0..20_000)
            .map(|i| {
                album_track(
                    &format!("d/{i:05}.flac"),
                    "t",
                    &format!("Artist {}", i % 997),
                    "",
                    &format!("Album {i}"),
                )
            })
            .collect();
        app.handle_scan_event(ScanEvent::Batch(tracks), now);
        local_press(
            &mut app,
            &[Action::Down, Action::Down, Action::Down, Action::Activate],
            now,
        );
        assert_eq!(app.library_row_count(), 20_000);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut view = ViewState::default();
        let built = rows_built_by(|| draw_to(&mut terminal, &app, &mut view, now));
        assert!(built <= 19, "built {built} rows");
    }

    // ----- Navidrome Home, playlists and album lists ---------------------

    use crate::navidrome::browse::Resource as Res;
    use crate::navidrome::{AlbumPage, AlbumSort, Playlist};

    /// Navidrome only, connected, on Home.
    fn nav_home() -> App {
        let mut app = App::with_sources(None, Some(nav_client()));
        app.handle_navidrome_ping(Ok(()), Instant::now());
        app
    }

    fn press(app: &mut App, actions: &[Action]) {
        for action in actions {
            app.handle_action(*action, Instant::now());
        }
    }

    fn nav_album(id: &str, name: &str, artist: &str) -> Album {
        Album {
            id: id.into(),
            name: name.into(),
            artist: Some(artist.into()),
            year: Some(1959),
            song_count: None,
            duration: None,
        }
    }

    #[test]
    fn home_and_playlists_render() {
        let now = Instant::now();
        let mut app = nav_home();
        let screen = render(&app, 100, 14, now);
        for expected in [
            "┌ Navidrome (connected) ─",
            "> Playlists",
            "Artists",
            "Albums",
        ] {
            assert!(screen.contains(expected), "{expected}:\n{screen}");
        }
        assert!(!screen.contains("›"), "Home has no path: {screen}");
        press(&mut app, &[Action::Activate]);
        let request = app.take_fetch_requests().remove(0);
        assert_eq!(request.resource, Res::Playlists);
        answer(
            &mut app,
            &request,
            Ok(Content::Playlists(vec![
                Playlist {
                    id: "p1".into(),
                    name: "Road Trip".into(),
                    song_count: Some(12),
                    duration: Some(Duration::from_secs(2712)),
                },
                Playlist {
                    id: "p2".into(),
                    name: "\u{202E}Huge\u{1b}[2J".into(),
                    song_count: Some(u32::MAX),
                    duration: Some(Duration::MAX),
                },
                Playlist {
                    id: "p3".into(),
                    name: "Bare".into(),
                    song_count: None,
                    duration: None,
                },
            ])),
        );
        let screen = render(&app, 100, 14, now);
        for expected in [
            "Navidrome (connected) › Playlists",
            "> Road Trip",
            "12 songs · 45:12",
            "4,294,967,295 songs",
            "Bare",
        ] {
            assert!(screen.contains(expected), "{expected}:\n{screen}");
        }
        assert!(!screen.contains('\u{1b}') && !screen.contains('\u{202E}'));
        assert_full_width(&screen, 100);
        assert_full_width(&render(&app, 30, 14, now), 30);
        // A playlist's songs show their artist.
        press(&mut app, &[Action::Activate]);
        let songs = app.take_fetch_requests().remove(0);
        answer(
            &mut app,
            &songs,
            Ok(Content::Songs(vec![
                nav_track("s1", "So What", "Miles Davis", 562),
                nav_track("s1", "So What", "Miles Davis", 562),
            ])),
        );
        let screen = render(&app, 100, 14, now);
        assert!(
            screen.contains("Navidrome (connected) › Playlists › Road Trip"),
            "{screen}"
        );
        assert_eq!(
            screen.matches("Miles Davis - So What").count(),
            2,
            "{screen}"
        );
        assert!(screen.contains("9:22"), "{screen}");
    }

    #[test]
    fn album_lists_show_the_sort_and_a_paging_footer() {
        let now = Instant::now();
        let mut app = nav_home();
        press(&mut app, &[Action::Down, Action::Down, Action::Activate]);
        let first = app.take_fetch_requests().remove(0);
        let screen = render(&app, 100, 14, now);
        assert!(
            screen.contains("Navidrome (connected) › Albums (A–Z)"),
            "{screen}"
        );
        assert!(screen.contains("Loading…"), "{screen}");
        let albums: Vec<Album> = (0..500)
            .map(|i| nav_album(&format!("al{i}"), &format!("Record {i:03}"), "Band"))
            .collect();
        answer(
            &mut app,
            &first,
            Ok(Content::AlbumPage(AlbumPage {
                albums,
                served: 500,
            })),
        );
        let mut terminal = Terminal::new(TestBackend::new(100, 14)).unwrap();
        let mut view = ViewState::default();
        draw_to(&mut terminal, &app, &mut view, now);
        let screen = buffer_text(&terminal);
        assert!(screen.contains("> Record 000 — Band"), "{screen}");
        assert!(screen.contains("1959"), "{screen}");
        assert!(!screen.contains("Loading more"), "{screen}");
        // 14 rows - 5 (now playing, status) - 2 borders.
        assert_eq!(view.remote_rows, 7);
        app.set_remote_rows(view.remote_rows);
        for _ in 0..493 {
            press(&mut app, &[Action::Down]);
        }
        let more = app.take_fetch_requests().remove(0);
        assert_eq!(
            more.resource,
            Res::AlbumPage {
                sort: AlbumSort::Name,
                offset: 500
            }
        );
        draw_to(&mut terminal, &app, &mut view, now);
        let screen = buffer_text(&terminal);
        assert!(screen.contains("> Record 493 — Band"), "{screen}");
        assert!(screen.contains("Loading more…"), "{screen}");
        assert_eq!(view.remote_rows, 6, "the footer takes a row");
        answer(&mut app, &more, Err(crate::navidrome::Error::Http(500)));
        draw_to(&mut terminal, &app, &mut view, now);
        let screen = buffer_text(&terminal);
        assert!(
            screen.contains("Could not load more — r to retry"),
            "{screen}"
        );
        assert!(!screen.contains("secret-host"), "{screen}");
        // The sort shows in the title.
        press(&mut app, &[Action::CycleSort, Action::CycleSort]);
        let screen = render(&app, 100, 14, now);
        assert!(
            screen.contains("Navidrome (connected) › Albums (newest)"),
            "{screen}"
        );
    }

    #[test]
    fn a_huge_playlist_renders_only_visible_rows() {
        let now = Instant::now();
        let mut app = nav_home();
        press(&mut app, &[Action::Activate]);
        let lists = app.take_fetch_requests().remove(0);
        answer(
            &mut app,
            &lists,
            Ok(Content::Playlists(vec![Playlist {
                id: "big".into(),
                name: "Big".into(),
                song_count: Some(10_000),
                duration: None,
            }])),
        );
        press(&mut app, &[Action::Activate]);
        let songs = app.take_fetch_requests().remove(0);
        let tracks: Vec<Track> = (0..10_000)
            .map(|i| nav_track(&format!("s{i}"), &format!("Song {i:05}"), "x", 60))
            .collect();
        answer(&mut app, &songs, Ok(Content::Songs(tracks)));
        for _ in 0..9_990 {
            press(&mut app, &[Action::Down]);
        }
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut view = ViewState::default();
        let started = Instant::now();
        let built = rows_built_by(|| draw_to(&mut terminal, &app, &mut view, now));
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(1), "render took {elapsed:?}");
        assert!(built <= 2 * 19, "built {built} rows");
        let screen = buffer_text(&terminal);
        assert!(screen.contains("> x - Song 09990"), "{screen}");
    }

    #[test]
    fn help_lists_play_next_and_sort() {
        let keys: Vec<&str> = help::BINDINGS.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"a / A"), "{keys:?}");
        assert!(keys.contains(&"r / o"), "{keys:?}");
    }
}
