//! The help overlay: a centred box listing the keymap.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::ui::render::REMOTE_MARKER;

/// Key column, description column.
pub const BINDINGS: &[(&str, &str)] = &[
    ("j/k  ↓/↑", "Move selection"),
    ("Enter", "Open / play now / play entry"),
    ("h  Backspace", "Go up (library)"),
    ("a", "Add track, folder, album, artist"),
    ("1 / 2", "Local / Navidrome library"),
    ("/", "Search Navidrome (library)"),
    ("  Enter / Esc", "Run search / cancel"),
    ("r", "Refresh Navidrome view"),
    ("d", "Remove entry (queue)"),
    ("J / K", "Move entry down / up (queue)"),
    ("g", "Select current entry (queue)"),
    ("s", "Shuffle upcoming (queue)"),
    ("C", "Clear queue (queue)"),
    ("R", "Repeat: off / all / one"),
    ("Space", "Pause / resume / start"),
    ("n / p", "Next / previous"),
    ("← / →", "Seek -5 s / +5 s"),
    ("+ (=) / -", "Volume up / down"),
    ("Tab", "Switch panel"),
    ("?  Esc", "Close help"),
    ("q  Ctrl-C", "Quit"),
    (REMOTE_MARKER, "Streamed from Navidrome"),
];

const KEY_WIDTH: usize = 14;

/// Draws the help box with its lines starting at `scroll` (clamped).
/// When not every line fits, the title says how to scroll. Returns the
/// largest scroll that still shows a full box of lines.
pub fn draw(frame: &mut Frame, area: Rect, scroll: usize) -> usize {
    let lines: Vec<Line> = BINDINGS
        .iter()
        .map(|(key, what)| {
            Line::from(vec![
                Span::styled(
                    format!(" {key:<KEY_WIDTH$}"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(*what),
            ])
        })
        .collect();
    let width = (area.width).min(52);
    let height = (area.height).min(lines.len() as u16 + 2);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    let visible = usize::from(height.saturating_sub(2));
    let max_scroll = lines.len().saturating_sub(visible);
    let scroll = scroll.min(max_scroll);
    let title = if max_scroll > 0 {
        format!(" Help {}/{} · j/k scroll ", scroll + visible, lines.len())
    } else {
        " Help ".to_owned()
    };
    let shown: Vec<Line> = lines.into_iter().skip(scroll).take(visible).collect();
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(shown).block(Block::bordered().title(title)),
        popup,
    );
    max_scroll
}
