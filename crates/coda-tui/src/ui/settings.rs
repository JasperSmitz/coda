//! Draws the Settings overlay ([`Settings`]): the overview (music folder,
//! Navidrome status and what can be done) or the server form, in the same
//! centred panel as setup. Keys are mapped with setup's
//! [`crate::ui::onboarding::map_key`].
//!
//! The URL and username may be shown (the user typed them); the password
//! only ever masked. Error texts come without credentials or hosts.

use ratatui::Frame;
use ratatui::style::{Color, Style};

use crate::app::Connection;
use crate::config::MusicDirArg;
use crate::secret_store::STORE_NAME;
use crate::settings::{Busy, FormKind, Navidrome, Settings};
use crate::ui::onboarding::{Panel, draw_panel, server_form};

/// Draws the overlay over the whole frame. `connection` is the current
/// client's last observed state; `spinner` any counter that advances.
pub fn draw(frame: &mut Frame, settings: &Settings, connection: &Connection, spinner: usize) {
    draw_panel(frame, |width| match settings.form() {
        Some((kind, _)) => form(settings, kind, spinner, width),
        None => overview(settings, connection, spinner, width),
    });
}

fn overview(settings: &Settings, connection: &Connection, spinner: usize, width: usize) -> Panel {
    let mut p = Panel::new("Settings", width);
    let context = settings.context();
    let note = Style::default().fg(Color::Yellow);
    match &context.music_dir {
        Some(dir) => p.field("Music folder", &dir.to_string_lossy()),
        None => p.field("Music folder", "none"),
    }
    match context.music_origin {
        Some(MusicDirArg::Env) => p.note("Set by CODA_MUSIC_DIR, outside Coda."),
        Some(MusicDirArg::Argument) => p.note("Given on the command line for this run."),
        None => p.hint("Change it with coda --setup."),
    }
    p.blank();
    let navidrome = settings.navidrome();
    match (navidrome.account_label(), connection) {
        (None, _) => p.field("Navidrome", "Not configured"),
        (Some(label), Connection::Connected) => {
            p.field("Navidrome", &format!("Connected as {label}"));
        }
        (Some(label), Connection::Connecting) => {
            p.field("Navidrome", &format!("Connecting as {label}…"));
        }
        (Some(label), Connection::Failed(error)) => {
            p.field("Navidrome", &label);
            p.error(&format!("Not connected: {error}"));
        }
    }
    if let Navidrome::Outside { by, .. } = navidrome {
        let text = if by.is_empty() {
            "Configured outside Coda.".to_owned()
        } else {
            format!(
                "Configured outside Coda ({}); change it there.",
                by.join(", ")
            )
        };
        p.wrapped(&text, note);
    }
    p.blank();
    if settings.busy() == Some(Busy::Disconnecting) {
        // No hints: a save cannot be interrupted.
        p.spinner(spinner, "Disconnecting…");
        return p;
    }
    match settings.notice() {
        Some(notice) => p.wrapped(notice, note),
        None => p.blank(),
    }
    p.blank();
    p.hint(match navidrome {
        Navidrome::NotConfigured => "c connect to Navidrome · Esc close · q quit",
        Navidrome::Coda { .. } => "e edit · d disconnect · Esc close · q quit",
        Navidrome::Outside { .. } => "Esc close · q quit",
    });
    p
}

fn form(settings: &Settings, kind: FormKind, spinner: usize, width: usize) -> Panel {
    let title = match kind {
        FormKind::Connect => "Connect to Navidrome",
        FormKind::Edit => "Edit Navidrome",
    };
    let mut p = Panel::new(title, width);
    let Some((_, server)) = settings.form() else {
        return p;
    };
    let busy = settings.busy();
    server_form(&mut p, server, busy.is_none());
    p.blank();
    p.hint(&format!("The password is stored in the {STORE_NAME}."));
    p.blank();
    match busy {
        Some(Busy::Connecting) => {
            let url = settings.server_url().map(|u| u.as_str()).unwrap_or("");
            p.spinner(spinner, &format!("Connecting to {url}"));
            p.blank();
            p.hint("Esc stop waiting");
        }
        Some(Busy::Saving | Busy::Disconnecting) => {
            // No hints: a save cannot be interrupted.
            p.spinner(spinner, "Saving…");
        }
        None => {
            match server.error() {
                Some(error) => p.error(error),
                None => p.hint("Without http:// or https://, https:// is used."),
            }
            p.blank();
            p.hint("Enter next/connect · Tab field · Esc back · Ctrl-C quit");
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::navidrome::client::parse_base_url;
    use crate::onboarding::Input;
    use crate::settings::{Context, Request, Response};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::time::Instant;
    use unicode_width::UnicodeWidthStr;

    fn screen(settings: &Settings, connection: &Connection) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 22)).unwrap();
        terminal
            .draw(|frame| draw(frame, settings, connection, 0))
            .unwrap();
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
        out
    }

    fn context(navidrome: Navidrome) -> Context {
        Context {
            config_path: Some("/cfg/config.toml".into()),
            music_dir: Some("/home/me/Music".into()),
            music_origin: None,
            file_has_music_dir: true,
            navidrome,
            env_overrides: Vec::new(),
        }
    }

    fn coda() -> Navidrome {
        Navidrome::Coda {
            url: parse_base_url("https://music.example:4533").unwrap(),
            username: "me".into(),
            account: "me@music.example:4533".into(),
        }
    }

    fn opened(navidrome: Navidrome) -> Settings {
        let mut settings = Settings::new(context(navidrome));
        settings.open();
        settings
    }

    fn input(settings: &mut Settings, inputs: &[Input]) {
        for i in inputs {
            settings.handle_input(i.clone(), Instant::now());
        }
    }

    fn typed(settings: &mut Settings, text: &str) {
        for c in text.chars() {
            settings.handle_input(Input::Char(c), Instant::now());
        }
    }

    #[test]
    fn overview_for_each_ownership_case() {
        let screen_of = |navidrome, connection| screen(&opened(navidrome), &connection);

        let s = screen_of(Navidrome::NotConfigured, Connection::Connecting);
        assert!(s.contains(" Settings "), "{s}");
        assert!(s.contains("Music folder  /home/me/Music"), "{s}");
        assert!(s.contains("Change it with coda --setup."), "{s}");
        assert!(s.contains("Navidrome     Not configured"), "{s}");
        assert!(s.contains("c connect to Navidrome"), "{s}");

        let s = screen_of(coda(), Connection::Connected);
        assert!(s.contains("Connected as me @ music.example:4533"), "{s}");
        assert!(s.contains("e edit · d disconnect"), "{s}");
        assert!(!s.contains("outside"), "{s}");

        let s = screen_of(coda(), Connection::Failed("authentication failed".into()));
        assert!(s.contains("me @ music.example:4533"), "{s}");
        assert!(s.contains("Not connected: authentication failed"), "{s}");

        let s = screen_of(coda(), Connection::Connecting);
        assert!(s.contains("Connecting as me @ music.example:4533…"), "{s}");

        let outside = Navidrome::Outside {
            url: parse_base_url("https://music.example").unwrap(),
            username: "me".into(),
            by: vec!["CODA_NAVIDROME_URL", "password_command"],
        };
        let s = screen_of(outside, Connection::Connected);
        assert!(
            s.contains("Configured outside Coda (CODA_NAVIDROME_URL, password_command)"),
            "{s}"
        );
        assert!(!s.contains("e edit"), "{s}");
        assert!(!s.contains("c connect"), "{s}");
        assert!(s.contains("Esc close"), "{s}");
    }

    #[test]
    fn music_folder_notes_name_where_it_comes_from() {
        for (origin, note) in [
            (MusicDirArg::Env, "Set by CODA_MUSIC_DIR, outside Coda."),
            (
                MusicDirArg::Argument,
                "Given on the command line for this run.",
            ),
        ] {
            let mut settings = Settings::new(Context {
                music_origin: Some(origin),
                ..context(coda())
            });
            settings.open();
            let s = screen(&settings, &Connection::Connected);
            assert!(s.contains(note), "{s}");
        }
        let mut settings = Settings::new(Context {
            music_dir: None,
            ..context(coda())
        });
        settings.open();
        input(&mut settings, &[Input::Char('d'), Input::Char('d')]);
        let s = screen(&settings, &Connection::Connected);
        assert!(s.contains("Music folder  none"), "{s}");
        assert!(s.contains("Navidrome is the only source"), "{s}");
    }

    #[test]
    fn notices_prompts_and_blocked_environment() {
        let mut settings = opened(coda());
        input(&mut settings, &[Input::Char('d')]);
        let s = screen(&settings, &Connection::Connected);
        assert!(s.contains("Press d again to disconnect"), "{s}");
        input(&mut settings, &[Input::Char('d')]);
        let s = screen(&settings, &Connection::Connected);
        assert!(s.contains("Disconnecting…"), "{s}");
        assert!(!s.contains("Esc close"), "no hints while saving: {s}");

        let mut settings = Settings::new(Context {
            env_overrides: vec!["CODA_NAVIDROME_USER"],
            ..context(Navidrome::NotConfigured)
        });
        settings.open();
        input(&mut settings, &[Input::Char('c')]);
        let s = screen(&settings, &Connection::Connecting);
        assert!(s.contains("CODA_NAVIDROME_USER is set"), "{s}");
    }

    #[test]
    fn the_form_masks_the_password_and_shows_progress_and_errors() {
        let mut settings = opened(coda());
        input(&mut settings, &[Input::Char('e')]);
        typed(&mut settings, "hunter2");
        let s = screen(&settings, &Connection::Connected);
        assert!(s.contains(" Edit Navidrome "), "{s}");
        assert!(s.contains("https://music.example:4533/"), "{s}");
        assert!(s.contains("me"), "{s}");
        assert!(s.contains("•••••••"), "{s}");
        assert!(!s.contains("hunter2"), "{s}");
        assert!(
            s.contains(&format!("The password is stored in the {STORE_NAME}.")),
            "{s}"
        );
        assert!(s.contains("Esc back"), "{s}");

        input(&mut settings, &[Input::Enter]);
        let s = screen(&settings, &Connection::Connected);
        assert!(
            s.contains("Connecting to https://music.example:4533/"),
            "{s}"
        );
        assert!(s.contains("Esc stop waiting"), "{s}");
        let Some(Request::Ping { id, .. }) = settings.take_requests().pop() else {
            panic!()
        };
        settings.handle_response(Response::Pinged {
            id,
            result: Err("Authentication failed".into()),
        });
        let s = screen(&settings, &Connection::Connected);
        assert!(s.contains("Authentication failed"), "{s}");
        assert!(!s.contains("hunter2"), "{s}");

        input(&mut settings, &[Input::Enter]);
        let Some(Request::Ping { id, .. }) = settings.take_requests().pop() else {
            panic!()
        };
        settings.handle_response(Response::Pinged { id, result: Ok(()) });
        let s = screen(&settings, &Connection::Connected);
        assert!(s.contains("Saving…"), "{s}");
        assert!(!s.contains("Esc"), "no hints while saving: {s}");
    }

    #[test]
    fn connect_form_title_and_tiny_terminals() {
        let mut settings = opened(Navidrome::NotConfigured);
        input(&mut settings, &[Input::Char('c')]);
        let s = screen(&settings, &Connection::Connecting);
        assert!(s.contains(" Connect to Navidrome "), "{s}");
        for (w, h) in [(10, 5), (1, 1), (0, 0)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal
                .draw(|frame| draw(frame, &settings, &Connection::Connecting, 3))
                .unwrap();
        }
    }
}
