//! Draws the first-run setup ([`Onboarding`]) and maps keys to its
//! [`Input`]s. One compact centred panel per screen.
//!
//! Typed text is sanitised and truncated like every other untrusted
//! string (the end of an input stays visible); the password is only ever
//! drawn masked.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::onboarding::{Busy, Field, Input, Onboarding, SaveError, Screen};
use crate::ui::render::{fit, fit_tail};

const MAX_WIDTH: u16 = 76;
const MAX_HEIGHT: u16 = 16;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Width of the form labels, including the gap after them.
const LABEL_WIDTH: usize = 10;

/// Things the panel shows that the state machine does not know.
#[derive(Debug, Clone)]
pub struct View {
    /// Where the config will be written, as shown.
    pub config_path: String,
    /// Where the password will be stored, e.g. "macOS Keychain".
    pub store_name: &'static str,
    /// Advances the spinner; any counter will do.
    pub spinner: usize,
}

/// Maps a key press. Ctrl-C quits; other Ctrl/Alt combinations and
/// control characters are ignored.
pub fn map_key(key: KeyEvent) -> Option<Input> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return matches!(key.code, KeyCode::Char('c' | 'C')).then_some(Input::Quit);
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        return None;
    }
    match key.code {
        KeyCode::Char(c) if !c.is_control() => Some(Input::Char(c)),
        KeyCode::Backspace => Some(Input::Backspace),
        KeyCode::Enter => Some(Input::Enter),
        KeyCode::Esc => Some(Input::Esc),
        KeyCode::Tab => Some(Input::Tab),
        KeyCode::BackTab => Some(Input::BackTab),
        KeyCode::Up => Some(Input::Up),
        KeyCode::Down => Some(Input::Down),
        _ => None,
    }
}

/// Lines of the panel plus where the cursor goes (line, column).
struct Panel {
    title: &'static str,
    lines: Vec<Line<'static>>,
    cursor: Option<(usize, usize)>,
    width: usize,
}

impl Panel {
    fn new(title: &'static str, width: usize) -> Self {
        Self {
            title,
            lines: Vec::new(),
            cursor: None,
            width,
        }
    }

    fn text(&mut self, text: &str) {
        self.lines.push(Line::raw(fit(text, self.width)));
    }

    fn blank(&mut self) {
        self.lines.push(Line::raw(""));
    }

    fn styled(&mut self, text: &str, style: Style) {
        self.lines
            .push(Line::from(Span::styled(fit(text, self.width), style)));
    }

    fn error(&mut self, text: &str) {
        self.styled(text, Style::default().fg(Color::Red));
    }

    fn hint(&mut self, text: &str) {
        self.styled(text, Style::default().fg(Color::DarkGray));
    }

    fn spinner(&mut self, frame: usize, text: &str) {
        let symbol = SPINNER[frame % SPINNER.len()];
        self.styled(
            &format!("{symbol} {text}"),
            Style::default().fg(Color::Cyan),
        );
    }

    /// `label` then `value` (already masked if secret), with the cursor at
    /// its end when `focused`. Shows the end of a long value.
    fn input(&mut self, label: &str, value: &str, focused: bool) {
        let label = format!("{label:<LABEL_WIDTH$}");
        let room = self.width.saturating_sub(label.width() + 1);
        let shown = fit_tail(value, room);
        let column = label.width() + shown.width();
        let label_style = if focused {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        if focused {
            self.cursor = Some((self.lines.len(), column));
        }
        self.lines.push(Line::from(vec![
            Span::styled(label, label_style),
            Span::raw(shown),
        ]));
    }

    /// A `label: value` summary line.
    fn field(&mut self, label: &str, value: &str) {
        let label = format!("{label:<14}");
        let room = self.width.saturating_sub(label.width());
        self.lines.push(Line::from(vec![
            Span::styled(label, Style::default().fg(Color::DarkGray)),
            Span::raw(fit(value, room)),
        ]));
    }
}

pub fn draw(frame: &mut Frame, setup: &Onboarding, view: &View) {
    let area = frame.area();
    let outer = area.centered(
        Constraint::Length(MAX_WIDTH.min(area.width)),
        Constraint::Length(MAX_HEIGHT.min(area.height)),
    );
    // Inside the border and one column of padding on each side.
    let width = usize::from(outer.width.saturating_sub(4));
    let panel = build(setup, view, width);

    let block = Block::bordered()
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            format!(" {} ", fit(panel.title, width)),
            Style::default().add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(outer);
    let inner = Rect {
        x: inner.x.saturating_add(1),
        width: inner.width.saturating_sub(2),
        ..inner
    };
    frame.render_widget(Clear, outer);
    frame.render_widget(block, outer);
    frame.render_widget(Paragraph::new(panel.lines), inner);
    if let Some((row, column)) = panel.cursor
        && row < usize::from(inner.height)
        && inner.width > 0
    {
        let x = inner.x + (column as u16).min(inner.width - 1);
        frame.set_cursor_position((x, inner.y + row as u16));
    }
}

fn build(setup: &Onboarding, view: &View, width: usize) -> Panel {
    match setup.screen() {
        Screen::Welcome => {
            let mut p = Panel::new("Welcome to Coda", width);
            p.text("Coda plays music from a folder on this computer,");
            p.text("from a Navidrome server, or both.");
            p.blank();
            p.text("Let's set up where your music comes from.");
            p.blank();
            p.hint("Enter continue · Esc or Ctrl-C quit");
            p
        }
        Screen::Music => {
            let mut p = Panel::new("Local music", width);
            p.text("Do you have a local music folder?");
            p.blank();
            let checking = setup.busy() == Some(Busy::CheckingFolder);
            p.input("Folder", setup.music_input(), !checking);
            p.blank();
            if checking {
                p.spinner(view.spinner, "Checking the folder…");
                p.blank();
                p.hint("Esc stop waiting · Ctrl-C quit");
            } else {
                match setup.music_error() {
                    Some(error) => p.error(error),
                    None => p.blank(),
                }
                p.blank();
                p.hint("Enter use this folder · Esc skip · Ctrl-C quit");
            }
            p
        }
        Screen::AskNavidrome => {
            let mut p = Panel::new("Navidrome", width);
            p.text("Do you have a Navidrome server?");
            p.blank();
            match setup.music_dir() {
                Some(dir) => p.field("Music folder", dir),
                None => p.field("Music folder", "none"),
            }
            p.blank();
            p.hint("y set it up · n skip · Esc back · Ctrl-C quit");
            p
        }
        Screen::Server => {
            let mut p = Panel::new("Navidrome server", width);
            let connecting = setup.busy() == Some(Busy::Connecting);
            let focus = (!connecting).then_some(setup.focus());
            p.input("URL", setup.url_input(), focus == Some(Field::Url));
            p.input(
                "Username",
                setup.username_input(),
                focus == Some(Field::Username),
            );
            p.input(
                "Password",
                &setup.password_masked(),
                focus == Some(Field::Password),
            );
            p.blank();
            if let Some(url) = setup.connecting_to().filter(|_| connecting) {
                p.spinner(view.spinner, &format!("Connecting to {url}"));
                p.blank();
                p.hint("Esc stop waiting · Ctrl-C quit");
            } else {
                match setup.server_error() {
                    Some(error) => p.error(error),
                    None => p.hint("Without http:// or https://, https:// is used."),
                }
                p.blank();
                p.hint("Enter next/connect · Tab field · Esc skip Navidrome · Ctrl-C quit");
            }
            p
        }
        Screen::Ready => {
            let mut p = Panel::new("Ready", width);
            p.field("Music folder", setup.music_dir().unwrap_or("none"));
            match setup.connected() {
                Some((url, username)) => {
                    p.field("Navidrome", &format!("Connected as {username}"));
                    p.field("", url.as_str());
                    if setup.connected_insecurely() {
                        p.error("Plain http: others on the network can observe your login.");
                    }
                }
                None => p.field("Navidrome", "not set up"),
            }
            p.blank();
            p.field("Settings", &view.config_path);
            if setup.connected().is_some() {
                p.field("Password", view.store_name);
            }
            p.blank();
            if setup.busy() == Some(Busy::Saving) {
                // No hints: a save cannot be interrupted.
                p.spinner(view.spinner, "Saving…");
            } else {
                match setup.save_error() {
                    Some(SaveError::Keychain(message) | SaveError::Config(message)) => {
                        p.error(message);
                    }
                    None => p.blank(),
                }
                p.blank();
                let hint = match (setup.save_error(), setup.connected().is_some()) {
                    (Some(_), true) => "Enter retry · s skip Navidrome · Esc back · Ctrl-C quit",
                    (Some(_), false) => "Enter retry · Esc back · Ctrl-C quit",
                    (None, true) => {
                        "Enter save and start · s skip Navidrome · Esc back · Ctrl-C quit"
                    }
                    (None, false) => "Enter save and start · Esc back · Ctrl-C quit",
                };
                p.hint(hint);
            }
            p
        }
        Screen::Nothing => {
            let mut p = Panel::new("Nothing to play yet", width);
            p.text("Nothing to play yet. Coda will ask again next time.");
            p.blank();
            p.hint("Press any key to exit");
            p
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onboarding::{Request, Response};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn view() -> View {
        View {
            config_path: "/home/u/.config/coda/config.toml".into(),
            store_name: "macOS Keychain",
            spinner: 0,
        }
    }

    fn render(setup: &Onboarding, width: u16, height: u16) -> (String, Option<(u16, u16)>) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                draw(frame, setup, &view());
            })
            .unwrap();
        let position = terminal.get_cursor_position().unwrap();
        let cursor = Some((position.x, position.y));
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            let mut x = 0;
            while x < buffer.area.width {
                let symbol = buffer[(x, y)].symbol();
                out.push_str(symbol);
                x += symbol.width().max(1) as u16;
            }
            out.push('\n');
        }
        (out, cursor)
    }

    fn screen(setup: &Onboarding) -> String {
        render(setup, 80, 20).0
    }

    fn send(setup: &mut Onboarding, inputs: &[Input]) {
        for input in inputs {
            setup.handle_input(input.clone());
        }
    }

    fn typed(setup: &mut Onboarding, text: &str) {
        for c in text.chars() {
            setup.handle_input(Input::Char(c));
        }
    }

    fn answer(setup: &mut Onboarding, ok: bool, message: &str) {
        let request = setup.take_requests().pop().unwrap();
        let response = match request {
            Request::CheckFolder { id, .. } => Response::FolderChecked {
                id,
                result: if ok { Ok(()) } else { Err(message.into()) },
            },
            Request::Ping { id, .. } => Response::Pinged {
                id,
                result: if ok { Ok(()) } else { Err(message.into()) },
            },
            Request::Save { id, .. } => Response::Saved {
                id,
                result: if ok {
                    Ok(())
                } else {
                    Err(SaveError::Keychain(message.into()))
                },
            },
        };
        setup.handle_response(response);
    }

    #[test]
    fn keys_map_to_inputs() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(map_key(key(KeyCode::Char('q'))), Some(Input::Char('q')));
        assert_eq!(map_key(key(KeyCode::Enter)), Some(Input::Enter));
        assert_eq!(map_key(key(KeyCode::BackTab)), Some(Input::BackTab));
        assert_eq!(map_key(key(KeyCode::F(1))), None);
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(map_key(ctrl('c')), Some(Input::Quit));
        assert_eq!(map_key(ctrl('u')), None);
        let alt = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT);
        assert_eq!(map_key(alt), None);
    }

    #[test]
    fn welcome_and_music_screens() {
        let mut setup = Onboarding::new(Some("~/Music"));
        let text = screen(&setup);
        assert!(text.contains("Welcome to Coda"), "{text}");
        // Every hint fits an 80-column terminal.
        assert!(!text.contains('…'), "{text}");
        assert!(
            text.contains("Enter continue · Esc or Ctrl-C quit"),
            "{text}"
        );

        setup.handle_input(Input::Enter);
        let (text, cursor) = render(&setup, 80, 20);
        assert!(text.contains("Do you have a local music folder?"), "{text}");
        assert!(text.contains("Folder    ~/Music"), "{text}");
        assert!(text.contains("Esc skip"), "{text}");
        let (x, y) = cursor.unwrap();
        let line: String = text.lines().nth(usize::from(y)).unwrap().to_owned();
        let before: String = line.chars().take(usize::from(x)).collect();
        assert!(before.ends_with("~/Music"), "{line:?} at {x}");

        setup.handle_input(Input::Enter);
        let text = screen(&setup);
        assert!(text.contains("Checking the folder"), "{text}");
        answer(&mut setup, false, "That folder does not exist");
        let text = screen(&setup);
        assert!(text.contains("That folder does not exist"), "{text}");
    }

    #[test]
    fn server_form_masks_the_password_and_shows_connecting() {
        let mut setup = Onboarding::new(None);
        send(&mut setup, &[Input::Enter, Input::Esc]);
        let text = screen(&setup);
        assert!(text.contains("Do you have a Navidrome server?"), "{text}");
        assert!(
            text.contains("y set it up · n skip · Esc back · Ctrl-C quit"),
            "{text}"
        );

        setup.handle_input(Input::Char('y'));
        typed(&mut setup, "music.example.org");
        setup.handle_input(Input::Enter);
        typed(&mut setup, "me");
        setup.handle_input(Input::Enter);
        setup.handle_input(Input::Paste("hunter2-secret".into()));
        let text = screen(&setup);
        assert!(text.contains("Password  ••••••••••••••"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");

        setup.handle_input(Input::Enter);
        let text = screen(&setup);
        assert!(
            text.contains("Connecting to https://music.example.org/"),
            "{text}"
        );
        assert!(text.contains("Esc stop waiting"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");

        answer(&mut setup, false, "Authentication failed");
        let text = screen(&setup);
        assert!(text.contains("Authentication failed"), "{text}");
        assert!(
            text.contains("URL       https://music.example.org/"),
            "{text}"
        );
        assert!(!text.contains("hunter2"), "{text}");
    }

    #[test]
    fn ready_saving_and_errors() {
        let mut setup = Onboarding::new(None);
        send(&mut setup, &[Input::Enter, Input::Esc, Input::Char('y')]);
        typed(&mut setup, "http://nd.example:4533");
        setup.handle_input(Input::Enter);
        typed(&mut setup, "me");
        setup.handle_input(Input::Enter);
        typed(&mut setup, "pw-secret");
        setup.handle_input(Input::Enter);
        answer(&mut setup, true, "");
        let text = screen(&setup);
        assert!(text.contains("Connected as me"), "{text}");
        assert!(text.contains("http://nd.example:4533/"), "{text}");
        assert!(text.contains("Plain http"), "{text}");
        assert!(text.contains("/home/u/.config/coda/config.toml"), "{text}");
        assert!(text.contains("macOS Keychain"), "{text}");
        assert!(
            text.contains("Enter save and start · s skip Navidrome"),
            "{text}"
        );

        setup.handle_input(Input::Enter);
        let text = screen(&setup);
        assert!(text.contains("Saving…"), "{text}");
        assert!(
            !text.contains("Ctrl-C"),
            "no quit hint while saving: {text}"
        );
        answer(&mut setup, false, "Could not store the password: locked");
        let text = screen(&setup);
        assert!(
            text.contains("Could not store the password: locked"),
            "{text}"
        );
        assert!(text.contains("Enter retry · s skip Navidrome"), "{text}");
        assert!(text.contains("Ctrl-C quit"), "{text}");
        assert!(!text.contains("pw-secret"), "{text}");

        setup.handle_input(Input::Char('s'));
        let text = screen(&setup);
        assert!(
            text.contains("Nothing to play yet. Coda will ask again"),
            "{text}"
        );
    }

    #[test]
    fn hostile_input_is_sanitised_and_tiny_terminals_do_not_panic() {
        let mut setup = Onboarding::new(None);
        setup.handle_input(Input::Enter);
        typed(&mut setup, "/a\u{202E}b");
        setup.handle_input(Input::Paste("x".repeat(5000)));
        let text = screen(&setup);
        assert!(!text.contains('\u{202E}'), "{text}");
        assert!(text.contains('…'), "{text}");
        for (w, h) in [(0, 0), (1, 1), (5, 3), (20, 4), (200, 60)] {
            render(&setup, w, h);
        }
    }
}

#[cfg(test)]
mod hint_tests {
    use super::*;
    use crate::onboarding::Request;

    /// Every non-saving screen offers Ctrl-C, and no hint is cut off in
    /// an 80-column terminal.
    #[test]
    fn every_screen_but_saving_shows_ctrl_c_untruncated() {
        let width = usize::from(MAX_WIDTH - 4);
        let view = View {
            config_path: "/c".into(),
            store_name: "store",
            spinner: 0,
        };
        let hints = |setup: &Onboarding| -> String {
            let panel = build(setup, &view, width);
            let last = panel.lines.last().unwrap().to_string();
            assert!(!last.contains('…'), "truncated: {last}");
            last
        };
        let mut setup = Onboarding::new(None);
        let mut seen = vec![hints(&setup)];
        for input in [Input::Enter, Input::Esc, Input::Char('y')] {
            setup.handle_input(input);
            seen.push(hints(&setup));
        }
        for c in "h".chars() {
            setup.handle_input(Input::Char(c));
        }
        setup.handle_input(Input::Tab);
        setup.handle_input(Input::Char('u'));
        setup.handle_input(Input::Tab);
        setup.handle_input(Input::Char('p'));
        setup.handle_input(Input::Enter);
        seen.push(hints(&setup));
        let Some(Request::Ping { id, .. }) = setup.take_requests().pop() else {
            panic!()
        };
        setup.handle_response(crate::onboarding::Response::Pinged { id, result: Ok(()) });
        seen.push(hints(&setup));
        for hint in &seen {
            assert!(hint.contains("Ctrl-C"), "{hint}");
        }
    }
}
