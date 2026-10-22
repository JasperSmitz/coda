//! The help overlay: a centred box listing the keymap.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

/// Key column, description column.
pub const BINDINGS: &[(&str, &str)] = &[
    ("j/k  ↓/↑", "Move selection"),
    ("Enter", "Open directory / play now"),
    ("h  Backspace", "Parent directory (library)"),
    ("a", "Add track or directory (library)"),
    ("d", "Remove entry (queue)"),
    ("Space", "Pause / resume / start"),
    ("n / p", "Next / previous"),
    ("← / →", "Seek -5 s / +5 s"),
    ("+ (=) / -", "Volume up / down"),
    ("Tab", "Switch panel"),
    ("?  Esc", "Close help"),
    ("q  Ctrl-C", "Quit"),
];

const KEY_WIDTH: usize = 14;

pub fn draw(frame: &mut Frame, area: Rect) {
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
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Help ")),
        popup,
    );
}
