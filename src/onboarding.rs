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
//! `coda --setup` runs the same screens as a redo ([`Onboarding::redo`]):
//! the folder, URL and username start from the config file (the password
//! is asked again), settings made outside Coda are reported for the screens
//! they affect, and skipping a source removes it. Skipping both is refused
//! there ("keep at least one source") instead of ending. While
//! `CODA_NAVIDROME_URL` or `CODA_NAVIDROME_USER` is set without
//! `CODA_NAVIDROME_PASSWORD`, startup would look the password up under the
//! account those variables name, so a redo cannot save a server: it offers
//! keeping `[navidrome]` as it is, or removing it.
//!
//! The password is held as a [`Password`] in requests and only ever shown
//! masked. The server URL may be shown (the user typed it); nothing here
//! logs it.

use url::Url;

use crate::config::{MusicDirArg, NAVIDROME_PASSWORD_ENV, NAVIDROME_URL_ENV, NAVIDROME_USER_ENV};
use crate::line_input::{LineInput, MAX_PATH_CHARS, MAX_QUERY_CHARS};
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
    /// With `keep_navidrome` (and no `server`), `[navidrome]` is kept as it
    /// is in the file.
    Save {
        id: RequestId,
        music_dir: Option<String>,
        server: Option<Server>,
        keep_navidrome: bool,
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

/// What `coda --setup` starts from: the config file's values and the
/// settings made outside Coda (names only). `Debug` hides the values.
#[derive(Clone, Default)]
pub struct Redo {
    pub music_dir: Option<String>,
    pub url: Option<String>,
    pub username: Option<String>,
    /// A folder given on the command line or in `CODA_MUSIC_DIR`, which
    /// overrides the file's.
    pub music_override: Option<MusicDirArg>,
    /// The `CODA_NAVIDROME_*` variables that are set.
    pub navidrome_overrides: Vec<&'static str>,
    /// `password` or `password_command` in the file, which saving
    /// Navidrome replaces with the keychain.
    pub hand_written_password: Option<&'static str>,
    /// The file configures Navidrome (so skipping it removes it).
    pub has_navidrome: bool,
    /// The file's URL held credentials, which were left out of `url`.
    pub url_had_credentials: bool,
    /// The file's URL was not prefilled because it may hold credentials
    /// (see `config::SetupStart::url_withheld`).
    pub url_withheld: bool,
    /// Saving backs the current file up (it holds settings).
    pub file_has_content: bool,
}

impl Redo {
    /// `true` if the file has no folder, URL or username to start from.
    pub fn is_empty(&self) -> bool {
        self.music_dir.is_none() && self.url.is_none() && self.username.is_none()
    }

    /// The variables that make saving a server pointless: the URL or user
    /// from the environment, without its password. Startup would then look
    /// the stored password up under another account than the one entered.
    pub fn navidrome_blocked_by(&self) -> Vec<&'static str> {
        navidrome_blocked_by(&self.navidrome_overrides)
    }
}

/// Of the set `CODA_NAVIDROME_*` variables (`overrides`), those that stop
/// Coda from saving a server: the URL or user from the environment,
/// without its password. Startup would then look the stored password up
/// under another account than the one entered (LESSONS.md, "Setup and
/// startup must derive a secret's key from the same inputs"). Used by
/// `coda --setup` and the in-app settings alike.
pub fn navidrome_blocked_by(overrides: &[&'static str]) -> Vec<&'static str> {
    if overrides.contains(&NAVIDROME_PASSWORD_ENV) {
        return Vec::new();
    }
    overrides
        .iter()
        .copied()
        .filter(|name| [NAVIDROME_URL_ENV, NAVIDROME_USER_ENV].contains(name))
        .collect()
}

/// Explains why a redo cannot save a server. Names only.
pub fn navidrome_blocked_message(names: &[&str]) -> String {
    match names {
        [name] => {
            format!("{name} is set and would be used instead; unset it to change the server here.")
        }
        names => format!(
            "{} are set and would be used instead; unset them to change the server here.",
            names.join(" and ")
        ),
    }
}

/// Warns that removing Navidrome leaves these variables behind. Names
/// only.
pub fn leftover_overrides_message(names: &[&str]) -> String {
    let (verb, pronoun) = if names.len() == 1 {
        ("is", "it")
    } else {
        ("are", "them")
    };
    format!(
        "{} {verb} still set; unset {pronoun} too, or Coda will report Navidrome as partly configured.",
        names.join(" and ")
    )
}

/// Prefilled values that were left out because the input could not hold
/// them exactly (too long, or with control characters).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NotPrefilled {
    pub music_dir: bool,
    pub url: bool,
    pub username: bool,
}

impl std::fmt::Debug for Redo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redo")
            .field("music_override", &self.music_override)
            .field("navidrome_overrides", &self.navidrome_overrides)
            .field("hand_written_password", &self.hand_written_password)
            .field("has_navidrome", &self.has_navidrome)
            .field("url_had_credentials", &self.url_had_credentials)
            .field("url_withheld", &self.url_withheld)
            .field("file_has_content", &self.file_has_content)
            .finish_non_exhaustive()
    }
}

pub const KEEP_ONE_SOURCE: &str =
    "Keep at least one source: y sets up Navidrome, Esc goes to the folder";
/// The same while a server cannot be saved (env overrides), when the file
/// has one to keep.
pub const KEEP_ONE_SOURCE_BLOCKED: &str =
    "Keep at least one source: k keeps Navidrome, Esc goes to the folder";
/// The same while a server cannot be saved and there is none to keep.
pub const KEEP_ONE_SOURCE_FOLDER: &str = "Keep at least one source: Esc goes to the folder";
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
    form: ServerForm,
    /// The server being pinged.
    connecting: Option<Server>,
    /// The connected server, once the ping succeeded.
    server: Option<Server>,
    save_error: Option<SaveError>,
    /// Shown on the Navidrome question when a redo would remove both
    /// sources.
    source_error: Option<&'static str>,
    /// Set for `coda --setup`.
    redo: Option<Redo>,
    /// Redo values that could not be prefilled.
    not_prefilled: NotPrefilled,
    /// A redo keeps `[navidrome]` as it is (see the module docs).
    keep_navidrome: bool,
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
            music: LineInput::with_limit_and_text(
                MAX_PATH_CHARS,
                music_prefill.unwrap_or_default(),
            ),
            music_error: None,
            music_dir: None,
            form: ServerForm::new("", ""),
            connecting: None,
            server: None,
            save_error: None,
            source_error: None,
            redo: None,
            not_prefilled: NotPrefilled::default(),
            keep_navidrome: false,
            pending: None,
            next_id: 0,
            requests: Vec::new(),
            outcome: None,
        }
    }

    /// `coda --setup`: the folder, URL and username start from `redo`; the
    /// password starts empty.
    ///
    /// A value the input cannot hold exactly is left out (and reported by
    /// [`Onboarding::not_prefilled`]). With nothing to start from,
    /// `suggestion` fills the folder as on first run.
    pub fn redo(redo: Redo, suggestion: Option<&str>) -> Self {
        fn fits(value: &Option<String>, limit: usize) -> Option<&str> {
            value
                .as_deref()
                .filter(|v| LineInput::holds_exactly(limit, v))
        }
        let music = fits(&redo.music_dir, MAX_PATH_CHARS);
        let url = fits(&redo.url, MAX_QUERY_CHARS);
        let username = fits(&redo.username, MAX_QUERY_CHARS);
        let not_prefilled = NotPrefilled {
            music_dir: redo.music_dir.is_some() && music.is_none(),
            url: redo.url_withheld || (redo.url.is_some() && url.is_none()),
            username: redo.username.is_some() && username.is_none(),
        };
        let music = if redo.is_empty() { suggestion } else { music };
        let mut setup = Self::new(music);
        setup.form = ServerForm::new(url.unwrap_or_default(), username.unwrap_or_default());
        setup.not_prefilled = not_prefilled;
        setup.redo = Some(redo);
        setup
    }

    // ----- read access for the UI and driver ------------------------------

    /// What a redo started from; `None` on first run.
    pub fn redo_info(&self) -> Option<&Redo> {
        self.redo.as_ref()
    }

    /// Redo values that could not be prefilled.
    pub fn not_prefilled(&self) -> NotPrefilled {
        self.not_prefilled
    }

    /// The variables that stop a redo from saving a server; empty if none.
    pub fn navidrome_blocked_by(&self) -> Vec<&'static str> {
        self.redo
            .as_ref()
            .map(Redo::navidrome_blocked_by)
            .unwrap_or_default()
    }

    /// The `CODA_NAVIDROME_*` variables that would leave Navidrome partly
    /// configured once this redo removes it from the file: some, but not
    /// all three, are set.
    pub fn leftover_overrides(&self) -> Vec<&'static str> {
        match &self.redo {
            Some(redo)
                if self.server.is_none()
                    && !self.keep_navidrome
                    && redo.navidrome_overrides.len() < 3 =>
            {
                redo.navidrome_overrides.clone()
            }
            _ => Vec::new(),
        }
    }

    /// The redo keeps `[navidrome]` as it is.
    pub fn keeps_navidrome(&self) -> bool {
        self.keep_navidrome
    }

    /// Keeping `[navidrome]` is offered: the server cannot be saved, and
    /// the file has one to keep.
    pub fn can_keep_navidrome(&self) -> bool {
        self.redo.as_ref().is_some_and(|r| r.has_navidrome)
            && !self.navidrome_blocked_by().is_empty()
    }

    /// Why skipping Navidrome was refused, if it was.
    pub fn source_error(&self) -> Option<&'static str> {
        self.source_error
    }

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

    /// The server form (URL, username, password).
    pub fn form(&self) -> &ServerForm {
        &self.form
    }

    pub fn url_input(&self) -> &str {
        self.form.url_input()
    }

    pub fn username_input(&self) -> &str {
        self.form.username_input()
    }

    /// The password as shown: masked.
    pub fn password_masked(&self) -> String {
        self.form.password_masked()
    }

    pub fn focus(&self) -> Field {
        self.form.focus()
    }

    pub fn server_error(&self) -> Option<&str> {
        self.form.error()
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
                // The server cannot be saved; the screen says why.
                Input::Char('y' | 'Y') if !self.navidrome_blocked_by().is_empty() => {}
                Input::Char('k' | 'K') if self.can_keep_navidrome() => {
                    self.source_error = None;
                    self.server = None;
                    self.keep_navidrome = true;
                    self.save_error = None;
                    self.screen = Screen::Ready;
                }
                Input::Char('y' | 'Y') => {
                    self.source_error = None;
                    self.screen = Screen::Server;
                    // The first empty field: the password on a redo.
                    self.form.focus_first_empty();
                }
                Input::Char('n' | 'N') => self.skip_navidrome(),
                Input::Esc => {
                    self.source_error = None;
                    self.screen = Screen::Music;
                }
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
                    self.source_error = None;
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
                    (Err(message), _) => self.form.set_error(message),
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
                self.source_error = None;
                self.screen = Screen::AskNavidrome;
            }
            _ => {}
        }
    }

    fn skip_navidrome(&mut self) {
        self.keep_navidrome = false;
        self.server = None;
        self.connecting = None;
        self.form.clear_error();
        self.save_error = None;
        self.screen = if self.music_dir.is_some() {
            Screen::Ready
        } else if self.redo.is_some() {
            // A redo must leave a source; leaving setup changes nothing.
            self.source_error = Some(if self.navidrome_blocked_by().is_empty() {
                KEEP_ONE_SOURCE
            } else if self.can_keep_navidrome() {
                KEEP_ONE_SOURCE_BLOCKED
            } else {
                KEEP_ONE_SOURCE_FOLDER
            });
            Screen::AskNavidrome
        } else {
            Screen::Nothing
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
        match self.form.handle_input(input) {
            FormEvent::None => {}
            FormEvent::Cancel => self.skip_navidrome(),
            FormEvent::Submit(server) => {
                if !self.navidrome_blocked_by().is_empty() {
                    // Not reachable through the screens; never ping or store.
                    return;
                }
                self.connecting = Some(server.clone());
                self.request(Busy::Connecting, |id| Request::Ping { id, server });
            }
        }
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
                let keep_navidrome = self.keep_navidrome;
                self.request(Busy::Saving, |id| Request::Save {
                    id,
                    music_dir,
                    server,
                    keep_navidrome,
                });
            }
            Input::Char('s') if self.server.is_some() => self.skip_navidrome(),
            Input::Esc => {
                self.save_error = None;
                self.keep_navidrome = false;
                if self.server.take().is_some() {
                    self.screen = Screen::Server;
                } else {
                    self.source_error = None;
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

/// What a key did to the [`ServerForm`].
#[derive(Debug)]
pub enum FormEvent {
    None,
    /// Esc: leave the form.
    Cancel,
    /// Every field is filled and the URL is valid: connect to this server.
    Submit(Server),
}

/// The Navidrome form: URL, username and password, with inline errors.
/// Shared by setup and the in-app settings, which decide what happens on
/// submit and cancel. Pure; `Debug` shows no input.
pub struct ServerForm {
    url: LineInput,
    username: LineInput,
    password: LineInput,
    focus: Field,
    error: Option<String>,
}

impl std::fmt::Debug for ServerForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerForm")
            .field("focus", &self.focus)
            .finish_non_exhaustive()
    }
}

impl ServerForm {
    /// A form with `url` and `username` filled in (they may be empty) and
    /// an empty password, focused on the first empty field.
    pub fn new(url: &str, username: &str) -> Self {
        let mut form = Self {
            url: LineInput::with_text(url),
            username: LineInput::with_text(username),
            password: LineInput::new(),
            focus: Field::Url,
            error: None,
        };
        form.focus_first_empty();
        form
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

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Shows `message` under the form (e.g. a failed ping).
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
    }

    pub fn clear_error(&mut self) {
        self.error = None;
    }

    /// Focuses the first empty field (the password when the rest is
    /// prefilled).
    pub fn focus_first_empty(&mut self) {
        self.focus = if self.url.is_empty() {
            Field::Url
        } else if self.username.is_empty() {
            Field::Username
        } else {
            Field::Password
        };
    }

    /// Applies a key. Text and paste go into the focused field; Tab and
    /// the arrows move between fields; Enter moves on from a filled field
    /// and submits from the last field or once every field is filled (e.g.
    /// when retrying); Esc cancels. `Quit` is the caller's.
    pub fn handle_input(&mut self, input: Input) -> FormEvent {
        match input {
            Input::Char(c) => {
                self.field().push_char(c);
                self.error = None;
            }
            Input::Paste(text) => {
                self.field().push_str(trim_line_end(&text));
                self.error = None;
            }
            Input::Backspace => {
                self.field().backspace();
                self.error = None;
            }
            Input::Tab | Input::Down => self.next_field(),
            Input::BackTab | Input::Up => self.previous_field(),
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
                } else if let Some(server) = self.submit() {
                    return FormEvent::Submit(server);
                }
            }
            Input::Esc => return FormEvent::Cancel,
            Input::Quit => {}
        }
        FormEvent::None
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

    /// Validates the fields: the server to connect to, or `None` with an
    /// inline error and the faulty field focused.
    fn submit(&mut self) -> Option<Server> {
        let url = match normalize_url(self.url.as_str()) {
            Ok(url) => url,
            Err(error) => {
                self.error = Some(format!("Server URL: {error}"));
                self.focus = Field::Url;
                return None;
            }
        };
        // Show what will actually be contacted, unless the input could not
        // hold all of it (it would then be cut off and submit another URL).
        if url.as_str().chars().count() <= MAX_QUERY_CHARS {
            self.url = LineInput::with_text(url.as_str());
        }
        if self.username.is_empty() {
            self.error = Some(MISSING_USERNAME.into());
            self.focus = Field::Username;
            return None;
        }
        if self.password.is_empty() {
            self.error = Some(MISSING_PASSWORD.into());
            self.focus = Field::Password;
            return None;
        }
        self.error = None;
        Some(Server {
            url,
            username: self.username.as_str().to_owned(),
            password: Password::new(self.password.as_str().to_owned()),
        })
    }
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
            ..
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

    fn redo(music_dir: Option<&str>, navidrome: bool) -> Onboarding {
        Onboarding::redo(
            Redo {
                music_dir: music_dir.map(Into::into),
                url: navidrome.then(|| "https://h.example/".into()),
                username: navidrome.then(|| "me".into()),
                has_navidrome: navidrome,
                ..Redo::default()
            },
            None,
        )
    }

    fn save_request(setup: &mut Onboarding) -> (RequestId, Option<String>, Option<Server>) {
        match one_request(setup) {
            Request::Save {
                id,
                music_dir,
                server,
                ..
            } => (id, music_dir, server),
            other => panic!("expected a save, got {other:?}"),
        }
    }

    #[test]
    fn redo_prefills_folder_url_and_username_but_not_the_password() {
        let mut setup = redo(Some("~/Music"), true);
        assert!(setup.redo_info().is_some());
        setup.handle_input(Input::Enter);
        assert_eq!(setup.music_input(), "~/Music");
        setup.handle_input(Input::Enter);
        let Request::CheckFolder { id, typed: folder } = one_request(&mut setup) else {
            panic!()
        };
        assert_eq!(folder, "~/Music");
        setup.handle_response(Response::FolderChecked { id, result: Ok(()) });
        setup.handle_input(Input::Char('y'));
        assert_eq!(setup.screen(), Screen::Server);
        assert_eq!(setup.url_input(), "https://h.example/");
        assert_eq!(setup.username_input(), "me");
        assert_eq!(setup.password_masked(), "");
        assert_eq!(setup.focus(), Field::Password, "the empty field");
        typed(&mut setup, "pw");
        setup.handle_input(Input::Enter);
        let (id, server) = ping(&mut setup);
        assert_eq!(server.url.as_str(), "https://h.example/");
        assert_eq!(server.username, "me");
        setup.handle_response(Response::Pinged { id, result: Ok(()) });
        setup.handle_input(Input::Enter);
        let (_, music_dir, server) = save_request(&mut setup);
        assert_eq!(music_dir.as_deref(), Some("~/Music"));
        assert_eq!(server.unwrap().password.expose(), "pw");

        // A partial setup focuses the first missing field.
        let mut setup = Onboarding::redo(
            Redo {
                url: Some("h.example".into()),
                has_navidrome: true,
                ..Redo::default()
            },
            None,
        );
        send_all(&mut setup, &[Input::Enter, Input::Esc, Input::Char('y')]);
        assert_eq!(setup.focus(), Field::Username);
        assert_eq!(setup.url_input(), "h.example");
    }

    fn send_all(setup: &mut Onboarding, inputs: &[Input]) {
        for input in inputs {
            setup.handle_input(input.clone());
        }
    }

    #[test]
    fn redo_skipping_a_source_removes_it() {
        // Skipping the folder: saved without one.
        let mut setup = redo(Some("/m"), true);
        send_all(&mut setup, &[Input::Enter, Input::Esc, Input::Char('y')]);
        typed(&mut setup, "pw");
        setup.handle_input(Input::Enter);
        let (id, _) = ping(&mut setup);
        setup.handle_response(Response::Pinged { id, result: Ok(()) });
        setup.handle_input(Input::Enter);
        let (_, music_dir, server) = save_request(&mut setup);
        assert_eq!(music_dir, None);
        assert!(server.is_some());

        // Skipping Navidrome: saved without it.
        let mut setup = redo(Some("/m"), true);
        setup.handle_input(Input::Enter);
        choose_folder(&mut setup, "");
        setup.handle_input(Input::Char('n'));
        assert_eq!(setup.screen(), Screen::Ready);
        setup.handle_input(Input::Enter);
        let (_, music_dir, server) = save_request(&mut setup);
        assert_eq!(music_dir.as_deref(), Some("/m"));
        assert!(server.is_none());
    }

    #[test]
    fn redo_refuses_to_skip_both_but_first_run_exits() {
        let mut setup = redo(Some("/m"), true);
        send_all(&mut setup, &[Input::Enter, Input::Esc, Input::Char('n')]);
        assert_eq!(setup.screen(), Screen::AskNavidrome);
        assert_eq!(setup.source_error(), Some(KEEP_ONE_SOURCE));
        assert_eq!(setup.outcome(), None);
        assert!(setup.take_requests().is_empty());
        // Esc in the form is refused the same way, keeping what was typed.
        setup.handle_input(Input::Char('y'));
        assert_eq!(setup.source_error(), None);
        typed(&mut setup, "pw");
        setup.handle_input(Input::Esc);
        assert_eq!(setup.screen(), Screen::AskNavidrome);
        assert_eq!(setup.source_error(), Some(KEEP_ONE_SOURCE));
        setup.handle_input(Input::Char('y'));
        assert_eq!(setup.password_masked(), "••");
        // So is "s" after a failed save.
        setup.handle_input(Input::Enter);
        let (id, _) = ping(&mut setup);
        setup.handle_response(Response::Pinged { id, result: Ok(()) });
        setup.handle_input(Input::Enter);
        let (id, _, _) = save_request(&mut setup);
        setup.handle_response(Response::Saved {
            id,
            result: Err(SaveError::Keychain("locked".into())),
        });
        setup.handle_input(Input::Char('s'));
        assert_eq!(setup.screen(), Screen::AskNavidrome);
        assert_eq!(setup.source_error(), Some(KEEP_ONE_SOURCE));
        // Going back for a folder clears the message; leaving changes nothing.
        setup.handle_input(Input::Esc);
        assert_eq!(setup.screen(), Screen::Music);
        assert_eq!(setup.source_error(), None);
        setup.handle_input(Input::Quit);
        assert_eq!(setup.outcome(), Some(Outcome::Exit));
        assert!(setup.take_requests().is_empty());

        // First run: skipping both still ends.
        let mut setup = Onboarding::new(None);
        send_all(&mut setup, &[Input::Enter, Input::Esc, Input::Char('n')]);
        assert_eq!(setup.screen(), Screen::Nothing);
        assert_eq!(setup.source_error(), None);
    }

    fn blocked(overrides: Vec<&'static str>, has_navidrome: bool) -> Onboarding {
        Onboarding::redo(
            Redo {
                music_dir: Some("/m".into()),
                url: has_navidrome.then(|| "https://h.example/".into()),
                username: has_navidrome.then(|| "me".into()),
                navidrome_overrides: overrides,
                has_navidrome,
                ..Redo::default()
            },
            None,
        )
    }

    /// Welcome → folder accepted → the Navidrome question.
    fn at_question(setup: &mut Onboarding) {
        setup.handle_input(Input::Enter);
        choose_folder(setup, "");
    }

    #[test]
    fn env_url_or_user_without_password_blocks_saving_a_server() {
        use crate::config::{NAVIDROME_PASSWORD_ENV, NAVIDROME_URL_ENV, NAVIDROME_USER_ENV};
        let mut setup = blocked(vec![NAVIDROME_USER_ENV], true);
        assert_eq!(setup.navidrome_blocked_by(), [NAVIDROME_USER_ENV]);
        assert!(setup.can_keep_navidrome());
        at_question(&mut setup);
        // The form never opens, so nothing is pinged or stored.
        setup.handle_input(Input::Char('y'));
        assert_eq!(setup.screen(), Screen::AskNavidrome);
        assert!(setup.take_requests().is_empty());
        // Changing only the folder keeps [navidrome] as it is.
        setup.handle_input(Input::Char('k'));
        assert_eq!(setup.screen(), Screen::Ready);
        assert!(setup.keeps_navidrome());
        setup.handle_input(Input::Enter);
        let Request::Save {
            music_dir,
            server,
            keep_navidrome,
            ..
        } = one_request(&mut setup)
        else {
            panic!()
        };
        assert_eq!(music_dir.as_deref(), Some("/m"));
        assert!(server.is_none());
        assert!(keep_navidrome);

        // Esc on Ready drops the choice; removing still works.
        let mut setup = blocked(vec![NAVIDROME_URL_ENV, NAVIDROME_USER_ENV], true);
        at_question(&mut setup);
        setup.handle_input(Input::Char('k'));
        setup.handle_input(Input::Esc);
        assert_eq!(setup.screen(), Screen::AskNavidrome);
        assert!(!setup.keeps_navidrome());
        setup.handle_input(Input::Char('n'));
        assert_eq!(setup.screen(), Screen::Ready);
        setup.handle_input(Input::Enter);
        let Request::Save {
            server,
            keep_navidrome,
            ..
        } = one_request(&mut setup)
        else {
            panic!()
        };
        assert!(server.is_none() && !keep_navidrome);

        assert_eq!(
            setup.leftover_overrides(),
            [NAVIDROME_URL_ENV, NAVIDROME_USER_ENV]
        );

        // Removing the last source names only what works here.
        let mut no_folder = blocked(vec![NAVIDROME_USER_ENV], true);
        no_folder.handle_input(Input::Enter);
        no_folder.handle_input(Input::Esc);
        no_folder.handle_input(Input::Char('n'));
        assert_eq!(no_folder.source_error(), Some(KEEP_ONE_SOURCE_BLOCKED));
        let mut nothing = blocked(vec![NAVIDROME_USER_ENV], false);
        nothing.handle_input(Input::Enter);
        nothing.handle_input(Input::Esc);
        nothing.handle_input(Input::Char('n'));
        assert_eq!(nothing.source_error(), Some(KEEP_ONE_SOURCE_FOLDER));

        // Nothing to keep without [navidrome] in the file.
        let mut setup = blocked(vec![NAVIDROME_URL_ENV], false);
        assert!(!setup.can_keep_navidrome());
        at_question(&mut setup);
        setup.handle_input(Input::Char('k'));
        assert_eq!(setup.screen(), Screen::AskNavidrome);

        // With the password from the environment too, the stored entry is
        // never looked up, so nothing is blocked.
        let setup = blocked(
            vec![
                NAVIDROME_URL_ENV,
                NAVIDROME_USER_ENV,
                NAVIDROME_PASSWORD_ENV,
            ],
            true,
        );
        assert!(setup.navidrome_blocked_by().is_empty());
        assert!(!setup.can_keep_navidrome());
        // All three set: the environment alone configures Navidrome.
        assert!(setup.leftover_overrides().is_empty());
        // First run never has overrides (they would be a config error).
        assert!(Onboarding::new(None).navidrome_blocked_by().is_empty());
        assert_eq!(
            navidrome_blocked_message(&[NAVIDROME_USER_ENV]),
            "CODA_NAVIDROME_USER is set and would be used instead; unset it to change the server here."
        );
    }

    #[test]
    fn values_an_input_cannot_hold_exactly_are_not_prefilled() {
        let long_dir = format!("/{}", "d".repeat(1000));
        let mut setup = Onboarding::redo(
            Redo {
                music_dir: Some(long_dir.clone()),
                url: Some(format!("https://{}", "h".repeat(300))),
                username: Some("me\u{1b}[31m".into()),
                has_navidrome: true,
                ..Redo::default()
            },
            Some("~/Music"),
        );
        // A long path fits the folder input (4096 chars).
        assert_eq!(setup.music_input(), long_dir);
        assert_eq!(
            setup.not_prefilled(),
            NotPrefilled {
                music_dir: false,
                url: true,
                username: true
            }
        );
        assert_eq!(setup.url_input(), "");
        assert_eq!(setup.username_input(), "");
        setup.handle_input(Input::Enter);
        setup.handle_input(Input::Paste("x".repeat(5000)));
        assert_eq!(setup.music_input().chars().count(), MAX_PATH_CHARS);

        let setup = Onboarding::redo(
            Redo {
                music_dir: Some(format!("/{}", "d".repeat(MAX_PATH_CHARS))),
                ..Redo::default()
            },
            None,
        );
        assert!(setup.not_prefilled().music_dir);
        assert_eq!(setup.music_input(), "");
    }

    #[test]
    fn a_withheld_url_is_reported_as_not_prefilled() {
        let setup = Onboarding::redo(
            Redo {
                url_withheld: true,
                username: Some("me".into()),
                has_navidrome: true,
                ..Redo::default()
            },
            None,
        );
        assert!(setup.not_prefilled().url);
        assert_eq!(setup.url_input(), "");
    }

    #[test]
    fn an_empty_redo_suggests_the_music_folder_like_first_run() {
        let mut setup = Onboarding::redo(Redo::default(), Some("~/Music"));
        setup.handle_input(Input::Enter);
        assert_eq!(setup.music_input(), "~/Music");
        // With something to start from, the file wins.
        let mut setup = Onboarding::redo(
            Redo {
                url: Some("h".into()),
                ..Redo::default()
            },
            Some("~/Music"),
        );
        setup.handle_input(Input::Enter);
        assert_eq!(setup.music_input(), "");
    }

    #[test]
    fn redo_debug_hides_the_prefilled_values() {
        let info = Redo {
            music_dir: Some("/secret-dir".into()),
            url: Some("https://secret-host".into()),
            username: Some("secret-user".into()),
            navidrome_overrides: vec!["CODA_NAVIDROME_URL"],
            ..Redo::default()
        };
        let text = format!("{info:?}");
        assert!(!text.contains("secret"), "{text}");
        assert!(text.contains("CODA_NAVIDROME_URL"), "{text}");
        let text = format!("{:?}", Onboarding::redo(info, None));
        assert!(!text.contains("secret"), "{text}");
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
