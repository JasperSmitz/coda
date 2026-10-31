//! First-run setup as a pure state machine: [`Input`]s and driver
//! [`Response`]s go in, [`Request`]s (check a folder, ping a server,
//! save) come out. No I/O, no Ratatui; the driver is `crate::setup`, the
//! drawing `crate::ui::onboarding`.
//!
//! Screens, in order: welcome, local music folder (skippable), "Do you
//! have a Navidrome server?" (Esc goes back to the folder), the server form
//! (URL, username, password; skippable), then a summary that saves. Quit
//! (Ctrl-C) works everywhere except while saving. Skipping both ends on a
//! "nothing to play" screen that writes nothing.
//!
//! The password is held as a [`Password`] in requests and only ever shown
//! masked. The server URL may be shown (the user typed it); nothing here
//! logs it.

use url::Url;

use crate::line_input::{LineInput, MAX_QUERY_CHARS};
use crate::navidrome::Password;
use crate::navidrome::client::{UrlError, is_insecure, parse_base_url};

/// Abstract key input, mapped from terminal events by the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Char(char),
    Paste(String),
    Backspace,
    Enter,
    Esc,
    Tab,
    BackTab,
    Up,
    Down,
    /// Ctrl-C: leave setup at once.
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Welcome,
    Music,
    AskNavidrome,
    Server,
    Ready,
    Nothing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Url,
    Username,
    Password,
}

/// How setup ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The config (and keychain entry) were written: start the player.
    Saved,
    /// Nothing was written: exit.
    Exit,
}

pub type RequestId = u64;

/// Server details as entered. `Debug` hides everything.
#[derive(Clone)]
pub struct Server {
    pub url: Url,
    pub username: String,
    pub password: Password,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Server(<redacted>)")
    }
}

/// Work for the driver. Every request is answered by exactly one
/// [`Response`] with the same id; stale answers are ignored.
#[derive(Debug)]
pub enum Request {
    /// Expand `~/` and check that the folder exists and can be listed.
    CheckFolder { id: RequestId, typed: String },
    /// Connect with these credentials.
    Ping { id: RequestId, server: Server },
    /// Store the password (if any) in the keychain, then write the config.
    Save {
        id: RequestId,
        music_dir: Option<String>,
        server: Option<Server>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    /// The keychain refused: retry, or skip Navidrome.
    Keychain(String),
    /// The config file could not be written: retry.
    Config(String),
}

/// A finished request. Error texts are short and shown as they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    FolderChecked {
        id: RequestId,
        result: Result<(), String>,
    },
    Pinged {
        id: RequestId,
        result: Result<(), String>,
    },
    Saved {
        id: RequestId,
        result: Result<(), SaveError>,
    },
}

/// What the driver is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    CheckingFolder,
    Connecting,
    Saving,
}

const MISSING_FOLDER: &str = "Type a folder, or press Esc to skip";
const MISSING_USERNAME: &str = "Enter your username";
const MISSING_PASSWORD: &str = "Enter your password";

/// The setup state. See the module docs. `Debug` shows no input.
pub struct Onboarding {
    screen: Screen,
    music: LineInput,
    music_error: Option<String>,
    /// The accepted folder, as typed.
    music_dir: Option<String>,
    url: LineInput,
    username: LineInput,
    password: LineInput,
    focus: Field,
    server_error: Option<String>,
    /// The server being pinged.
    connecting: Option<Server>,
    /// The connected server, once the ping succeeded.
    server: Option<Server>,
    save_error: Option<SaveError>,
    pending: Option<(RequestId, Busy)>,
    next_id: RequestId,
    requests: Vec<Request>,
    outcome: Option<Outcome>,
}

impl std::fmt::Debug for Onboarding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Onboarding")
            .field("screen", &self.screen)
            .field("busy", &self.busy())
            .field("outcome", &self.outcome)
            .finish_non_exhaustive()
    }
}

/// A typed server URL with `https://` added when it has no scheme.
pub fn normalize_url(typed: &str) -> Result<Url, UrlError> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Err(UrlError::Invalid);
    }
    if typed.contains("://") {
        parse_base_url(typed)
    } else {
        parse_base_url(&format!("https://{typed}"))
    }
}

impl Onboarding {
    /// `music_prefill` is shown in the folder input (e.g. `~/Music` when
    /// that folder exists).
    pub fn new(music_prefill: Option<&str>) -> Self {
        Self {
            screen: Screen::Welcome,
            music: music_prefill.map(LineInput::with_text).unwrap_or_default(),
            music_error: None,
            music_dir: None,
            url: LineInput::new(),
            username: LineInput::new(),
            password: LineInput::new(),
            focus: Field::Url,
            server_error: None,
            connecting: None,
            server: None,
            save_error: None,
            pending: None,
            next_id: 0,
            requests: Vec::new(),
            outcome: None,
        }
    }

    // ----- read access for the UI and driver ------------------------------

    pub fn screen(&self) -> Screen {
        self.screen
    }

    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }

    pub fn busy(&self) -> Option<Busy> {
        self.pending.map(|(_, busy)| busy)
    }

    pub fn music_input(&self) -> &str {
        self.music.as_str()
    }

    pub fn music_error(&self) -> Option<&str> {
        self.music_error.as_deref()
    }

    pub fn music_dir(&self) -> Option<&str> {
        self.music_dir.as_deref()
    }

    pub fn url_input(&self) -> &str {
        self.url.as_str()
    }

    pub fn username_input(&self) -> &str {
        self.username.as_str()
    }

    /// The password as shown: masked.
    pub fn password_masked(&self) -> String {
        self.password.masked()
    }

    pub fn focus(&self) -> Field {
        self.focus
    }

    pub fn server_error(&self) -> Option<&str> {
        self.server_error.as_deref()
    }

    /// The (normalised) URL being connected to.
    pub fn connecting_to(&self) -> Option<&Url> {
        self.connecting.as_ref().map(|s| &s.url)
    }

    /// The connected server's URL and username.
    pub fn connected(&self) -> Option<(&Url, &str)> {
        self.server.as_ref().map(|s| (&s.url, s.username.as_str()))
    }

    /// `true` if the connected server is reached over plain http.
    pub fn connected_insecurely(&self) -> bool {
        self.server.as_ref().is_some_and(|s| is_insecure(&s.url))
    }

    pub fn save_error(&self) -> Option<&SaveError> {
        self.save_error.as_ref()
    }

    /// Requests for the driver, oldest first.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    // ----- transitions ----------------------------------------------------

    pub fn handle_input(&mut self, input: Input) {
        if self.outcome.is_some() {
            return;
        }
        if input == Input::Quit {
            // A save is never abandoned half-way (keychain written, config
            // not yet): quitting waits until it has answered.
            if self.busy() != Some(Busy::Saving) {
                self.finish(Outcome::Exit);
            }
            return;
        }
        match self.screen {
            Screen::Welcome => match input {
                Input::Enter => self.screen = Screen::Music,
                Input::Esc | Input::Char('q') => self.finish(Outcome::Exit),
                _ => {}
            },
            Screen::Music => self.music_input_key(input),
            Screen::AskNavidrome => match input {
                Input::Char('y' | 'Y') => {
                    self.screen = Screen::Server;
                    self.focus = Field::Url;
                }
                Input::Char('n' | 'N') => self.skip_navidrome(),
                Input::Esc => self.screen = Screen::Music,
                _ => {}
            },
            Screen::Server => self.server_key(input),
            Screen::Ready => self.ready_key(input),
            Screen::Nothing => self.finish(Outcome::Exit),
        }
    }

    pub fn handle_response(&mut self, response: Response) {
        let id = match &response {
            Response::FolderChecked { id, .. }
            | Response::Pinged { id, .. }
            | Response::Saved { id, .. } => *id,
        };
        if self.pending.map(|(pending, _)| pending) != Some(id) || self.outcome.is_some() {
            return;
        }
        self.pending = None;
        match response {
            Response::FolderChecked { result, .. } => match result {
                Ok(()) => {
                    self.music_dir = Some(self.music.as_str().trim().to_owned());
                    self.screen = Screen::AskNavidrome;
                }
                Err(message) => self.music_error = Some(message),
            },
            Response::Pinged { result, .. } => {
                let server = self.connecting.take();
                match (result, server) {
                    (Ok(()), Some(server)) => {
                        self.server = Some(server);
                        self.save_error = None;
                        self.screen = Screen::Ready;
                    }
                    (Err(message), _) => self.server_error = Some(message),
                    (Ok(()), None) => {}
                }
            }
            Response::Saved { result, .. } => match result {
                Ok(()) => self.finish(Outcome::Saved),
                Err(error) => self.save_error = Some(error),
            },
        }
    }

    fn finish(&mut self, outcome: Outcome) {
        self.outcome = Some(outcome);
        self.pending = None;
    }

    fn request(&mut self, busy: Busy, make: impl FnOnce(RequestId) -> Request) {
        self.next_id += 1;
        let id = self.next_id;
        self.pending = Some((id, busy));
        self.requests.push(make(id));
    }

    fn music_input_key(&mut self, input: Input) {
        if self.pending.is_some() {
            // Esc abandons a slow check; its answer is then ignored.
            if input == Input::Esc {
                self.pending = None;
            }
            return;
        }
        match input {
            Input::Char(c) => {
                self.music.push_char(c);
                self.music_error = None;
            }
            Input::Paste(text) => {
                self.music.push_str(trim_line_end(&text));
                self.music_error = None;
            }
            Input::Backspace => {
                self.music.backspace();
                self.music_error = None;
            }
            Input::Enter => {
                let typed = self.music.as_str().trim().to_owned();
                if typed.is_empty() {
                    self.music_error = Some(MISSING_FOLDER.into());
                } else {
                    self.music_error = None;
                    self.request(Busy::CheckingFolder, |id| Request::CheckFolder {
                        id,
                        typed,
                    });
                }
            }
            Input::Esc => {
                self.music_dir = None;
                self.music_error = None;
                self.screen = Screen::AskNavidrome;
            }
            _ => {}
        }
    }

    fn skip_navidrome(&mut self) {
        self.server = None;
        self.connecting = None;
        self.server_error = None;
        self.save_error = None;
        self.screen = if self.music_dir.is_some() {
            Screen::Ready
        } else {
            Screen::Nothing
        };
    }

    fn field(&mut self) -> &mut LineInput {
        match self.focus {
            Field::Url => &mut self.url,
            Field::Username => &mut self.username,
            Field::Password => &mut self.password,
        }
    }

    fn next_field(&mut self) {
        self.focus = match self.focus {
            Field::Url => Field::Username,
            Field::Username => Field::Password,
            Field::Password => Field::Url,
        };
    }

    fn previous_field(&mut self) {
        self.focus = match self.focus {
            Field::Url => Field::Password,
            Field::Username => Field::Url,
            Field::Password => Field::Username,
        };
    }

    fn server_key(&mut self, input: Input) {
        if self.pending.is_some() {
            if input == Input::Esc {
                // Stop waiting; the ping's answer will be ignored.
                self.pending = None;
                self.connecting = None;
            }
            return;
        }
        match input {
            Input::Char(c) => {
                self.field().push_char(c);
                self.server_error = None;
            }
            Input::Paste(text) => {
                self.field().push_str(trim_line_end(&text));
                self.server_error = None;
            }
            Input::Backspace => {
                self.field().backspace();
                self.server_error = None;
            }
            Input::Tab | Input::Down => self.next_field(),
            Input::BackTab | Input::Up => self.previous_field(),
            // Enter moves on from a filled field, and submits from the last
            // field or once every field is filled (e.g. when retrying).
            Input::Enter => {
                let all_filled =
                    !self.url.is_empty() && !self.username.is_empty() && !self.password.is_empty();
                let current_filled = match self.focus {
                    Field::Url => !self.url.is_empty(),
                    Field::Username => !self.username.is_empty(),
                    Field::Password => false,
                };
                if current_filled && !all_filled {
                    self.next_field();
                } else {
                    self.submit_server();
                }
            }
            Input::Esc => self.skip_navidrome(),
            Input::Quit => {}
        }
    }

    fn submit_server(&mut self) {
        let url = match normalize_url(self.url.as_str()) {
            Ok(url) => url,
            Err(error) => {
                self.server_error = Some(format!("Server URL: {error}"));
                self.focus = Field::Url;
                return;
            }
        };
        // Show what will actually be contacted, unless the input could not
        // hold all of it (it would then be cut off and submit another URL).
        if url.as_str().chars().count() <= MAX_QUERY_CHARS {
            self.url = LineInput::with_text(url.as_str());
        }
        if self.username.is_empty() {
            self.server_error = Some(MISSING_USERNAME.into());
            self.focus = Field::Username;
            return;
        }
        if self.password.is_empty() {
            self.server_error = Some(MISSING_PASSWORD.into());
            self.focus = Field::Password;
            return;
        }
        self.server_error = None;
        let server = Server {
            url,
            username: self.username.as_str().to_owned(),
            password: Password::new(self.password.as_str().to_owned()),
        };
        self.connecting = Some(server.clone());
        self.request(Busy::Connecting, |id| Request::Ping { id, server });
    }

    fn ready_key(&mut self, input: Input) {
        if self.pending.is_some() {
            return;
        }
        match input {
            Input::Enter => {
                self.save_error = None;
                let music_dir = self.music_dir.clone();
                let server = self.server.clone();
                self.request(Busy::Saving, |id| Request::Save {
                    id,
                    music_dir,
                    server,
                });
            }
            Input::Char('s') if self.server.is_some() => self.skip_navidrome(),
            Input::Esc => {
                self.save_error = None;
                if self.server.take().is_some() {
                    self.screen = Screen::Server;
                } else {
                    self.screen = Screen::AskNavidrome;
                }
            }
            _ => {}
        }
    }
}

/// A pasted value without its trailing line break (a copied password
/// often ends in one).
fn trim_line_end(text: &str) -> &str {
    text.trim_end_matches(['\r', '\n'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(setup: &mut Onboarding, text: &str) {
        for c in text.chars() {
            setup.handle_input(Input::Char(c));
        }
    }

    fn one_request(setup: &mut Onboarding) -> Request {
        let mut requests = setup.take_requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        requests.pop().unwrap()
    }

    /// Welcome → Music.
    fn at_music(prefill: Option<&str>) -> Onboarding {
        let mut setup = Onboarding::new(prefill);
        assert_eq!(setup.screen(), Screen::Welcome);
        setup.handle_input(Input::Enter);
        assert_eq!(setup.screen(), Screen::Music);
        setup
    }

    /// Accepts `folder` as the music folder.
    fn choose_folder(setup: &mut Onboarding, folder: &str) {
        typed(setup, folder);
        setup.handle_input(Input::Enter);
        let Request::CheckFolder { id, typed } = one_request(setup) else {
            panic!("expected a folder check");
        };
        assert_eq!(typed, setup.music_input().trim());
        setup.handle_response(Response::FolderChecked { id, result: Ok(()) });
        assert_eq!(setup.screen(), Screen::AskNavidrome);
    }

    /// Folder skipped, Navidrome form open.
    fn at_server() -> Onboarding {
        let mut setup = at_music(None);
        setup.handle_input(Input::Esc);
        setup.handle_input(Input::Char('y'));
        assert_eq!(setup.screen(), Screen::Server);
        setup
    }

    fn fill_server(setup: &mut Onboarding, url: &str, user: &str, password: &str) {
        typed(setup, url);
        setup.handle_input(Input::Enter);
        typed(setup, user);
        setup.handle_input(Input::Enter);
        typed(setup, password);
        setup.handle_input(Input::Enter);
    }

    fn ping(setup: &mut Onboarding) -> (RequestId, Server) {
        match one_request(setup) {
            Request::Ping { id, server } => (id, server),
            other => panic!("expected a ping, got {other:?}"),
        }
    }

    #[test]
    fn welcome_quits_with_esc_or_q() {
        for input in [Input::Esc, Input::Char('q'), Input::Quit] {
            let mut setup = Onboarding::new(None);
            setup.handle_input(input);
            assert_eq!(setup.outcome(), Some(Outcome::Exit));
            assert!(setup.take_requests().is_empty());
        }
        let mut setup = Onboarding::new(None);
        setup.handle_input(Input::Char('x'));
        assert_eq!(setup.screen(), Screen::Welcome);
        assert_eq!(setup.outcome(), None);
    }

    #[test]
    fn skipping_both_ends_on_nothing_and_exits_on_any_key() {
        let mut setup = at_music(Some("~/Music"));
        assert_eq!(setup.music_input(), "~/Music");
        setup.handle_input(Input::Esc);
        assert_eq!(setup.screen(), Screen::AskNavidrome);
        setup.handle_input(Input::Char('n'));
        assert_eq!(setup.screen(), Screen::Nothing);
        assert_eq!(setup.outcome(), None);
        setup.handle_input(Input::Char('z'));
        assert_eq!(setup.outcome(), Some(Outcome::Exit));
        assert!(setup.take_requests().is_empty(), "nothing is saved");

        // Esc in the form skips Navidrome.
        let mut setup = at_server();
        setup.handle_input(Input::Esc);
        assert_eq!(setup.screen(), Screen::Nothing);
    }

    #[test]
    fn folder_errors_are_inline_and_the_input_stays_editable() {
        let mut setup = at_music(None);
        setup.handle_input(Input::Enter);
        assert_eq!(setup.music_error(), Some(MISSING_FOLDER));
        assert!(setup.take_requests().is_empty());

        typed(&mut setup, "~/Nope");
        assert_eq!(setup.music_error(), None, "typing clears the error");
        setup.handle_input(Input::Enter);
        let Request::CheckFolder { id, typed: folder } = one_request(&mut setup) else {
            panic!()
        };
        assert_eq!(folder, "~/Nope");
        assert_eq!(setup.busy(), Some(Busy::CheckingFolder));
        // Keys other than Esc wait for the answer.
        setup.handle_input(Input::Char('x'));
        assert_eq!(setup.music_input(), "~/Nope");
        setup.handle_response(Response::FolderChecked {
            id,
            result: Err("That folder does not exist".into()),
        });
        assert_eq!(setup.busy(), None);
        assert_eq!(setup.screen(), Screen::Music);
        assert_eq!(setup.music_error(), Some("That folder does not exist"));

        for _ in 0.."Nope".len() {
            setup.handle_input(Input::Backspace);
        }
        choose_folder(&mut setup, "Music ");
        assert_eq!(setup.music_dir(), Some("~/Music"));
    }

    #[test]
    fn stale_and_abandoned_folder_checks_are_ignored() {
        let mut setup = at_music(Some("/slow"));
        setup.handle_input(Input::Enter);
        let Request::CheckFolder { id, .. } = one_request(&mut setup) else {
            panic!()
        };
        setup.handle_input(Input::Esc);
        assert_eq!(setup.busy(), None);
        assert_eq!(
            setup.screen(),
            Screen::Music,
            "Esc first abandons the check"
        );
        setup.handle_response(Response::FolderChecked { id, result: Ok(()) });
        assert_eq!(setup.screen(), Screen::Music);
        assert_eq!(setup.music_dir(), None);
        // A wrong id is ignored too.
        setup.handle_input(Input::Enter);
        let Request::CheckFolder { id, .. } = one_request(&mut setup) else {
            panic!()
        };
        setup.handle_response(Response::FolderChecked {
            id: id + 1,
            result: Ok(()),
        });
        assert_eq!(setup.busy(), Some(Busy::CheckingFolder));
    }

    #[test]
    fn folder_only_saves_without_a_server() {
        let mut setup = at_music(None);
        choose_folder(&mut setup, "/music");
        setup.handle_input(Input::Char('n'));
        assert_eq!(setup.screen(), Screen::Ready);
        assert_eq!(setup.connected(), None);
        setup.handle_input(Input::Enter);
        let Request::Save {
            id,
            music_dir,
            server,
        } = one_request(&mut setup)
        else {
            panic!()
        };
        assert_eq!(music_dir.as_deref(), Some("/music"));
        assert!(server.is_none());
        assert_eq!(setup.busy(), Some(Busy::Saving));
        setup.handle_input(Input::Esc);
        assert_eq!(setup.screen(), Screen::Ready, "saving cannot be undone");
        setup.handle_response(Response::Saved { id, result: Ok(()) });
        assert_eq!(setup.outcome(), Some(Outcome::Saved));
    }

    #[test]
    fn ready_esc_goes_back() {
        let mut setup = at_music(None);
        choose_folder(&mut setup, "/music");
        setup.handle_input(Input::Char('n'));
        assert_eq!(setup.screen(), Screen::Ready);
        setup.handle_input(Input::Esc);
        assert_eq!(setup.screen(), Screen::AskNavidrome);
        setup.handle_input(Input::Char('y'));
        fill_server(&mut setup, "h", "u", "p");
        let (id, _) = ping(&mut setup);
        setup.handle_response(Response::Pinged { id, result: Ok(()) });
        assert_eq!(setup.screen(), Screen::Ready);
        setup.handle_input(Input::Esc);
        assert_eq!(setup.screen(), Screen::Server, "back to the form");
        assert_eq!(setup.connected(), None);
        assert_eq!(setup.username_input(), "u", "fields are kept");
    }

    #[test]
    fn url_is_normalised_and_errors_name_the_field() {
        assert_eq!(
            normalize_url("music.example.org/nd").unwrap().as_str(),
            "https://music.example.org/nd/"
        );
        assert_eq!(
            normalize_url(" http://localhost:4533 ").unwrap().as_str(),
            "http://localhost:4533/"
        );
        assert_eq!(normalize_url(""), Err(UrlError::Invalid));
        assert_eq!(normalize_url("ftp://h"), Err(UrlError::Scheme));
        assert_eq!(normalize_url("https://u:p@h"), Err(UrlError::Credentials));
        assert_eq!(normalize_url("h/?x=1"), Err(UrlError::QueryOrFragment));

        let mut setup = at_server();
        fill_server(&mut setup, "ftp://h", "u", "p");
        assert!(setup.take_requests().is_empty());
        assert_eq!(
            setup.server_error(),
            Some("Server URL: the scheme must be http or https")
        );
        assert_eq!(setup.focus(), Field::Url);
    }

    #[test]
    fn enter_moves_through_fields_and_missing_values_are_reported() {
        let mut setup = at_server();
        // Enter on an empty URL submits, which reports the URL.
        setup.handle_input(Input::Enter);
        assert!(setup.server_error().unwrap().starts_with("Server URL"));
        typed(&mut setup, "music.example.org");
        setup.handle_input(Input::Enter);
        assert_eq!(setup.focus(), Field::Username);
        setup.handle_input(Input::Enter);
        assert_eq!(setup.server_error(), Some(MISSING_USERNAME));
        assert_eq!(
            setup.url_input(),
            "https://music.example.org/",
            "shown normalised"
        );
        typed(&mut setup, "me");
        setup.handle_input(Input::Tab);
        assert_eq!(setup.focus(), Field::Password);
        setup.handle_input(Input::Enter);
        assert_eq!(setup.server_error(), Some(MISSING_PASSWORD));
        assert!(setup.take_requests().is_empty());
        // Focus wraps both ways.
        setup.handle_input(Input::Down);
        assert_eq!(setup.focus(), Field::Url);
        setup.handle_input(Input::Up);
        assert_eq!(setup.focus(), Field::Password);
        setup.handle_input(Input::BackTab);
        assert_eq!(setup.focus(), Field::Username);
    }

    #[test]
    fn paste_goes_into_the_focused_field_without_its_line_break() {
        let mut setup = at_server();
        setup.handle_input(Input::Paste("https://h.example\n".into()));
        assert_eq!(setup.url_input(), "https://h.example");
        setup.handle_input(Input::Tab);
        setup.handle_input(Input::Paste("me".into()));
        setup.handle_input(Input::Tab);
        setup.handle_input(Input::Paste("pa ss\r\n".into()));
        assert_eq!(setup.password_masked(), "•••••");
        assert_eq!(setup.username_input(), "me");
        setup.handle_input(Input::Enter);
        let (_, server) = ping(&mut setup);
        assert_eq!(server.password.expose(), "pa ss");
        assert_eq!(server.username, "me");

        let mut setup = at_music(None);
        setup.handle_input(Input::Paste("/a\nb\n".into()));
        assert_eq!(setup.music_input(), "/a b");
    }

    #[test]
    fn failed_ping_can_be_edited_and_retried() {
        let mut setup = at_server();
        fill_server(&mut setup, "h.example", "me", "wrong");
        let (id, server) = ping(&mut setup);
        assert_eq!(server.url.as_str(), "https://h.example/");
        assert_eq!(setup.busy(), Some(Busy::Connecting));
        assert_eq!(setup.connecting_to(), Some(&server.url));
        // Typing waits while connecting.
        setup.handle_input(Input::Char('x'));
        assert_eq!(setup.password_masked(), "•••••");
        setup.handle_response(Response::Pinged {
            id,
            result: Err("Authentication failed".into()),
        });
        assert_eq!(setup.screen(), Screen::Server);
        assert_eq!(setup.server_error(), Some("Authentication failed"));
        assert_eq!(setup.connecting_to(), None);

        for _ in 0..5 {
            setup.handle_input(Input::Backspace);
        }
        typed(&mut setup, "right");
        assert_eq!(setup.server_error(), None);
        // With every field filled, Enter submits from any field.
        setup.handle_input(Input::Tab);
        assert_eq!(setup.focus(), Field::Url);
        setup.handle_input(Input::Enter);
        let (id, server) = ping(&mut setup);
        assert_eq!(server.password.expose(), "right");
        setup.handle_response(Response::Pinged { id, result: Ok(()) });
        assert_eq!(setup.screen(), Screen::Ready);
        let (url, user) = setup.connected().unwrap();
        assert_eq!((url.as_str(), user), ("https://h.example/", "me"));
        assert!(!setup.connected_insecurely());
    }

    #[test]
    fn esc_while_connecting_stops_waiting() {
        let mut setup = at_server();
        fill_server(&mut setup, "http://h.example", "me", "pw");
        let (id, _) = ping(&mut setup);
        setup.handle_input(Input::Esc);
        assert_eq!(setup.busy(), None);
        assert_eq!(setup.screen(), Screen::Server, "first Esc only cancels");
        setup.handle_response(Response::Pinged { id, result: Ok(()) });
        assert_eq!(setup.screen(), Screen::Server, "the late answer is ignored");
        setup.handle_input(Input::Enter);
        let (id, _) = ping(&mut setup);
        setup.handle_response(Response::Pinged { id, result: Ok(()) });
        assert!(setup.connected_insecurely());
    }

    /// Navidrome connected without a folder, at the Ready screen.
    fn connected_without_folder() -> Onboarding {
        let mut setup = at_server();
        fill_server(&mut setup, "h", "me", "pw");
        let (id, _) = ping(&mut setup);
        setup.handle_response(Response::Pinged { id, result: Ok(()) });
        setup
    }

    #[test]
    fn keychain_failure_can_be_retried() {
        let mut setup = connected_without_folder();
        setup.handle_input(Input::Enter);
        let Request::Save { id, server, .. } = one_request(&mut setup) else {
            panic!()
        };
        assert_eq!(server.unwrap().password.expose(), "pw");
        let failure = SaveError::Keychain("the keychain is locked".into());
        setup.handle_response(Response::Saved {
            id,
            result: Err(failure.clone()),
        });
        assert_eq!(setup.save_error(), Some(&failure));
        assert_eq!(setup.outcome(), None);
        setup.handle_input(Input::Enter);
        let Request::Save { id, .. } = one_request(&mut setup) else {
            panic!()
        };
        assert_eq!(setup.save_error(), None);
        setup.handle_response(Response::Saved { id, result: Ok(()) });
        assert_eq!(setup.outcome(), Some(Outcome::Saved));
    }

    #[test]
    fn keychain_failure_can_skip_navidrome() {
        // Without a folder, skipping Navidrome leaves nothing.
        let mut setup = connected_without_folder();
        setup.handle_input(Input::Enter);
        let Request::Save { id, .. } = one_request(&mut setup) else {
            panic!()
        };
        setup.handle_response(Response::Saved {
            id,
            result: Err(SaveError::Keychain("x".into())),
        });
        setup.handle_input(Input::Char('s'));
        assert_eq!(setup.screen(), Screen::Nothing);
        assert_eq!(setup.save_error(), None);

        // With a folder, the folder alone is saved.
        let mut setup = at_music(None);
        choose_folder(&mut setup, "/m");
        setup.handle_input(Input::Char('y'));
        fill_server(&mut setup, "h", "me", "pw");
        let (id, _) = ping(&mut setup);
        setup.handle_response(Response::Pinged { id, result: Ok(()) });
        setup.handle_input(Input::Enter);
        let Request::Save { id, .. } = one_request(&mut setup) else {
            panic!()
        };
        setup.handle_response(Response::Saved {
            id,
            result: Err(SaveError::Keychain("x".into())),
        });
        setup.handle_input(Input::Char('s'));
        assert_eq!(setup.screen(), Screen::Ready);
        assert_eq!(setup.connected(), None);
        setup.handle_input(Input::Enter);
        let Request::Save {
            music_dir, server, ..
        } = one_request(&mut setup)
        else {
            panic!()
        };
        assert_eq!(music_dir.as_deref(), Some("/m"));
        assert!(server.is_none());
    }

    #[test]
    fn config_write_failure_can_be_retried() {
        let mut setup = at_music(None);
        choose_folder(&mut setup, "/m");
        setup.handle_input(Input::Char('n'));
        setup.handle_input(Input::Enter);
        let Request::Save { id, .. } = one_request(&mut setup) else {
            panic!()
        };
        setup.handle_response(Response::Saved {
            id,
            result: Err(SaveError::Config("read-only".into())),
        });
        // 's' means nothing without a server.
        setup.handle_input(Input::Char('s'));
        assert_eq!(setup.screen(), Screen::Ready);
        setup.handle_input(Input::Enter);
        assert_eq!(setup.take_requests().len(), 1);
    }

    #[test]
    fn esc_on_the_navidrome_question_goes_back_to_the_folder() {
        let mut setup = at_music(Some("~/Music"));
        choose_folder(&mut setup, "");
        setup.handle_input(Input::Esc);
        assert_eq!(setup.screen(), Screen::Music);
        assert_eq!(setup.music_input(), "~/Music", "the input is kept");
        // Skipping the folder and going back works the same way.
        setup.handle_input(Input::Esc);
        assert_eq!(setup.screen(), Screen::AskNavidrome);
        setup.handle_input(Input::Esc);
        assert_eq!(setup.screen(), Screen::Music);
    }

    #[test]
    fn a_normalised_url_too_long_for_the_input_is_not_written_back() {
        let mut setup = at_server();
        // 249 chars typed; with "https://" and the trailing "/" it is 258.
        let host = format!("{}.example", "a".repeat(241));
        setup.handle_input(Input::Paste(host.clone()));
        assert_eq!(setup.url_input().chars().count(), 249);
        setup.handle_input(Input::Tab);
        setup.handle_input(Input::Paste("me".into()));
        setup.handle_input(Input::Tab);
        setup.handle_input(Input::Paste("pw".into()));
        setup.handle_input(Input::Enter);
        let (_, server) = ping(&mut setup);
        assert_eq!(server.url.as_str(), format!("https://{host}/"));
        assert_eq!(setup.url_input(), host, "the typed text is kept");
        // A short one is shown normalised.
        let mut setup = at_server();
        fill_server(&mut setup, "h.example", "me", "pw");
        assert_eq!(setup.url_input(), "https://h.example/");
    }

    #[test]
    fn quit_is_ignored_while_saving_and_the_save_still_finishes() {
        let mut setup = connected_without_folder();
        setup.handle_input(Input::Enter);
        let Request::Save { id, .. } = one_request(&mut setup) else {
            panic!()
        };
        setup.handle_input(Input::Quit);
        assert_eq!(setup.outcome(), None, "quitting waits for the save");
        assert_eq!(setup.busy(), Some(Busy::Saving));
        setup.handle_response(Response::Saved { id, result: Ok(()) });
        assert_eq!(setup.outcome(), Some(Outcome::Saved));

        // After a failed save, Quit works again.
        let mut setup = connected_without_folder();
        setup.handle_input(Input::Enter);
        let Request::Save { id, .. } = one_request(&mut setup) else {
            panic!()
        };
        setup.handle_input(Input::Quit);
        setup.handle_response(Response::Saved {
            id,
            result: Err(SaveError::Config("x".into())),
        });
        assert_eq!(setup.outcome(), None);
        setup.handle_input(Input::Quit);
        assert_eq!(setup.outcome(), Some(Outcome::Exit));
    }

    #[test]
    fn quit_works_everywhere_including_while_busy() {
        let mut setup = at_server();
        fill_server(&mut setup, "h", "me", "pw");
        let (id, _) = ping(&mut setup);
        setup.handle_input(Input::Quit);
        assert_eq!(setup.outcome(), Some(Outcome::Exit));
        // Nothing changes after the outcome.
        setup.handle_response(Response::Pinged { id, result: Ok(()) });
        setup.handle_input(Input::Enter);
        assert_eq!(setup.outcome(), Some(Outcome::Exit));
        assert!(setup.take_requests().is_empty());
    }

    #[test]
    fn debug_output_hides_credentials() {
        let mut setup = at_server();
        fill_server(&mut setup, "secret-host.example", "secret-user", "hunter2");
        let request = setup.take_requests().pop().unwrap();
        let text = format!("{request:?}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(!text.contains("secret"), "{text}");
        let text = format!("{setup:?}");
        assert!(!text.contains("hunter2"), "{text}");
    }
}
