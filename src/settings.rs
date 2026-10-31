//! The in-app Settings overlay (`,`) as pure state: [`Input`]s and driver
//! [`Response`]s go in, [`Request`]s (ping, save, disconnect) and
//! [`Change`]s (a new Navidrome client, or none) come out. No I/O and no
//! Ratatui: the app's event loop performs the requests
//! (`crate::app`), `crate::ui::settings` draws the overlay.
//!
//! The overlay shows the music folder and the Navidrome status. Who owns
//! a setting decides what can be done with it:
//! - Not configured: **Connect** (`c`) opens the server form.
//! - Written by Coda (`password_keychain = true`, no `CODA_NAVIDROME_*`
//!   variable): **Edit** (`e`) opens the form prefilled (except the
//!   password); **Disconnect** (`d`, twice) removes `[navidrome]` and the
//!   keychain entry. It is refused unless the config file keeps a music
//!   folder (one given only on the command line or in `CODA_MUSIC_DIR`
//!   does not count), so the file always configures a source.
//! - Set outside Coda (environment, or a hand-written `password` or
//!   `password_command`): shown with where it comes from (names, never
//!   values) and left alone.
//!
//! The form is setup's [`ServerForm`]. A submit pings the server; once
//! that succeeds the password is stored and the config written
//! ([`Request::Save`]); only then does the app switch to the new client.
//! While `CODA_NAVIDROME_URL` or `CODA_NAVIDROME_USER` is set without
//! `CODA_NAVIDROME_PASSWORD`, nothing is saved (startup would look the
//! password up under another account; see
//! [`crate::onboarding::navidrome_blocked_by`]).
//!
//! Every request has an id; an answer that is not the one awaited (stale,
//! or abandoned with Esc while connecting) is ignored.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use url::Url;

use crate::config::{MusicDirArg, NAVIDROME_PASSWORD_ENV, NavidromeConfig, PasswordSource};
use crate::line_input::{LineInput, MAX_QUERY_CHARS};
use crate::navidrome;
use crate::onboarding::{
    FormEvent, Input, Server, ServerForm, navidrome_blocked_by, navidrome_blocked_message,
};
use crate::secret_store;

/// A second `d` within this time disconnects.
pub const DISCONNECT_CONFIRM: Duration = Duration::from_secs(3);
/// Shown after the first `d`.
pub const DISCONNECT_PROMPT: &str = "Press d again to disconnect from Navidrome";
/// Disconnect refused: it would leave no source.
pub const KEEP_A_SOURCE: &str =
    "Navidrome is the only source, so it stays; run coda --setup to start over";
/// Disconnect refused: the folder of this run is not in the config file,
/// which would then configure no source.
pub const KEEP_A_SOURCE_IN_FILE: &str = "A source must stay in the config file, which has no music folder; add one with coda --setup first";
/// Connect or edit refused: there is nowhere to write the config.
pub const NO_CONFIG_LOCATION: &str =
    "There is no config file location; set CODA_CONFIG or HOME to save settings";

pub type RequestId = u64;

/// Who set up Navidrome, and so whether Coda may change it.
#[derive(Clone)]
pub enum Navidrome {
    NotConfigured,
    /// Written by Coda: the password is in the keychain under `account`.
    Coda {
        url: Url,
        username: String,
        account: String,
    },
    /// Set outside Coda; `by` names the variables or file keys (never
    /// their values).
    Outside {
        url: Url,
        username: String,
        by: Vec<&'static str>,
    },
}

impl std::fmt::Debug for Navidrome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => f.write_str("NotConfigured"),
            Self::Coda { .. } => f.write_str("Coda(<redacted>)"),
            Self::Outside { by, .. } => f.debug_struct("Outside").field("by", by).finish(),
        }
    }
}

impl Navidrome {
    /// Ownership of a loaded configuration. `overrides` are the
    /// `CODA_NAVIDROME_*` variables that are set
    /// ([`crate::config::navidrome_env_overrides`]).
    pub fn from_config(config: Option<&NavidromeConfig>, overrides: &[&'static str]) -> Self {
        let Some(config) = config else {
            return Self::NotConfigured;
        };
        let url = config.url.clone();
        let username = config.username.clone();
        let mut by = overrides.to_vec();
        match &config.password {
            PasswordSource::Keychain { account, .. } if by.is_empty() => {
                return Self::Coda {
                    url,
                    username,
                    account: account.clone(),
                };
            }
            PasswordSource::Keychain { .. } => {}
            PasswordSource::Command(_) => by.push("password_command"),
            // From the file unless the variable supplied it.
            PasswordSource::Value(_) if !overrides.contains(&NAVIDROME_PASSWORD_ENV) => {
                by.push("password");
            }
            PasswordSource::Value(_) => {}
        }
        Self::Outside { url, username, by }
    }

    /// `user @ host[:port]` for the screen, if configured.
    pub fn account_label(&self) -> Option<String> {
        match self {
            Self::NotConfigured => None,
            Self::Coda { url, username, .. } | Self::Outside { url, username, .. } => {
                Some(account_label(url, username))
            }
        }
    }
}

/// `user @ host[:port]`, for the screen only (never logs or errors).
pub fn account_label(url: &Url, username: &str) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{username} @ {host}:{port}"),
        None => format!("{username} @ {host}"),
    }
}

/// What the settings start from. `Debug` hides the values.
#[derive(Clone)]
pub struct Context {
    /// The config file the settings write; `None` if there is no location
    /// (no `CODA_CONFIG`, `XDG_CONFIG_HOME` or `HOME`).
    pub config_path: Option<PathBuf>,
    /// The music folder of this run, as shown.
    pub music_dir: Option<PathBuf>,
    /// Where the folder came from when not from the config file.
    pub music_origin: Option<MusicDirArg>,
    /// The config file has a `music_dir` (even if this run's folder came
    /// from elsewhere). Disconnect needs it.
    pub file_has_music_dir: bool,
    pub navidrome: Navidrome,
    /// The `CODA_NAVIDROME_*` variables that are set.
    pub env_overrides: Vec<&'static str>,
}

impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("config_path", &self.config_path.is_some())
            .field("music_dir", &self.music_dir.is_some())
            .field("music_origin", &self.music_origin)
            .field("file_has_music_dir", &self.file_has_music_dir)
            .field("navidrome", &self.navidrome)
            .field("env_overrides", &self.env_overrides)
            .finish()
    }
}

impl Context {
    /// Settings for an app built without a context (tests): nothing can
    /// be saved, and a client counts as set outside Coda.
    pub fn unsaved(music_dir: Option<PathBuf>, navidrome: Option<&navidrome::Client>) -> Self {
        Self {
            config_path: None,
            file_has_music_dir: music_dir.is_some(),
            music_dir,
            music_origin: None,
            navidrome: match navidrome {
                Some(client) => Navidrome::Outside {
                    url: client.base_url().clone(),
                    username: client.username().to_owned(),
                    by: Vec::new(),
                },
                None => Navidrome::NotConfigured,
            },
            env_overrides: Vec::new(),
        }
    }
}

/// Work for the event loop. Every request is answered by exactly one
/// [`Response`] with the same id.
pub enum Request {
    /// Connect with these credentials.
    Ping { id: RequestId, server: Server },
    /// Store the password, write `[navidrome]` (keeping the folder and
    /// `scrobble`), then delete `previous_account`'s entry if the account
    /// changed; build the new client.
    Save {
        id: RequestId,
        server: Server,
        previous_account: Option<String>,
    },
    /// Remove `[navidrome]`, then `previous_account`'s entry.
    Disconnect {
        id: RequestId,
        previous_account: Option<String>,
    },
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ping { id, .. } => write!(f, "Ping({id})"),
            Self::Save { id, .. } => write!(f, "Save({id})"),
            Self::Disconnect { id, .. } => write!(f, "Disconnect({id})"),
        }
    }
}

/// A saved server: its client and the file's `scrobble` setting.
#[derive(Debug, Clone)]
pub struct Saved {
    pub client: navidrome::Client,
    pub scrobble: bool,
}

/// A finished request. Error texts are short, free of the password,
/// account and host, and shown as they are.
#[derive(Debug)]
pub enum Response {
    Pinged {
        id: RequestId,
        result: Result<(), String>,
    },
    Saved {
        id: RequestId,
        result: Result<Saved, String>,
    },
    Disconnected {
        id: RequestId,
        result: Result<(), String>,
    },
}

impl Response {
    fn id(&self) -> RequestId {
        match self {
            Self::Pinged { id, .. } | Self::Saved { id, .. } | Self::Disconnected { id, .. } => *id,
        }
    }
}

/// What the app must apply after a response.
#[derive(Debug)]
pub enum Change {
    /// Switch to this client.
    Connected(Saved),
    /// Drop the client.
    Disconnected,
}

/// What the overlay is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    Connecting,
    Saving,
    Disconnecting,
}

/// Which form is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormKind {
    Connect,
    Edit,
}

/// The overlay state. See the module docs. `Debug` shows no values.
pub struct Settings {
    context: Context,
    open: bool,
    form: Option<(FormKind, ServerForm)>,
    /// The server being pinged or saved.
    server: Option<Server>,
    pending: Option<(RequestId, Busy)>,
    /// Until when a second `d` disconnects.
    disconnect_armed_until: Option<Instant>,
    /// A refusal, prompt or error shown on the overview.
    notice: Option<String>,
    next_id: RequestId,
    requests: Vec<Request>,
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings")
            .field("context", &self.context)
            .field("open", &self.open)
            .field("form", &self.form.as_ref().map(|(kind, _)| kind))
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

impl Settings {
    pub fn new(context: Context) -> Self {
        Self {
            context,
            open: false,
            form: None,
            server: None,
            pending: None,
            disconnect_armed_until: None,
            notice: None,
            next_id: 0,
            requests: Vec::new(),
        }
    }

    // ----- read access ----------------------------------------------------

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn context(&self) -> &Context {
        &self.context
    }

    pub fn navidrome(&self) -> &Navidrome {
        &self.context.navidrome
    }

    /// The open form, if any.
    pub fn form(&self) -> Option<(FormKind, &ServerForm)> {
        self.form.as_ref().map(|(kind, form)| (*kind, form))
    }

    pub fn busy(&self) -> Option<Busy> {
        self.pending.map(|(_, busy)| busy)
    }

    /// The URL being connected to or saved.
    pub fn server_url(&self) -> Option<&Url> {
        self.server.as_ref().map(|s| &s.url)
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// The `CODA_NAVIDROME_*` variables that stop a server from being
    /// saved here.
    pub fn blocked_by(&self) -> Vec<&'static str> {
        navidrome_blocked_by(&self.context.env_overrides)
    }

    /// Requests for the event loop, oldest first.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    // ----- transitions ----------------------------------------------------

    /// Opens the overview.
    pub fn open(&mut self) {
        self.open = true;
        self.notice = None;
        self.disconnect_armed_until = None;
    }

    /// Opens the overlay straight on the Connect form (`2` then Enter
    /// without Navidrome). A refusal is shown on the overview instead.
    pub fn open_connect(&mut self) {
        self.open();
        if matches!(self.context.navidrome, Navidrome::NotConfigured) {
            self.start_form(FormKind::Connect);
        }
    }

    /// Closes the overlay, unless a save is running. A ping in flight is
    /// abandoned.
    fn close(&mut self) {
        if matches!(self.busy(), Some(Busy::Saving | Busy::Disconnecting)) {
            return;
        }
        self.open = false;
        self.form = None;
        self.server = None;
        self.pending = None;
        self.notice = None;
        self.disconnect_armed_until = None;
    }

    /// Applies a key while the overlay is open. Quitting is the app's.
    pub fn handle_input(&mut self, input: Input, now: Instant) {
        if !self.open || input == Input::Quit {
            return;
        }
        match self.busy() {
            // Nothing interrupts a save between the keychain and the file.
            Some(Busy::Saving | Busy::Disconnecting) => return,
            Some(Busy::Connecting) => {
                if input == Input::Esc {
                    // Stop waiting; the ping's answer will be ignored.
                    self.pending = None;
                    self.server = None;
                }
                return;
            }
            None => {}
        }
        if let Some((_, form)) = self.form.as_mut() {
            match form.handle_input(input) {
                FormEvent::None => {}
                FormEvent::Cancel => self.form = None,
                FormEvent::Submit(server) => self.submit(server),
            }
            return;
        }
        // Any other key cancels a pending `d`.
        let armed = self.disconnect_armed_until.take();
        match input {
            Input::Esc | Input::Char(',') => self.close(),
            Input::Char('c') if matches!(self.context.navidrome, Navidrome::NotConfigured) => {
                self.start_form(FormKind::Connect);
            }
            Input::Char('e') if matches!(self.context.navidrome, Navidrome::Coda { .. }) => {
                self.start_form(FormKind::Edit);
            }
            Input::Char('d') => self.disconnect(armed, now),
            _ => {}
        }
    }

    /// Bracketed paste: into the open form's focused field.
    pub fn handle_paste(&mut self, text: &str) {
        if self.busy().is_some() {
            return;
        }
        if let Some((_, form)) = self.form.as_mut() {
            // A paste never submits or cancels.
            form.handle_input(Input::Paste(text.to_owned()));
        }
    }

    /// Applies a finished request. Returns the change the app must make
    /// once a save or disconnect is done.
    pub fn handle_response(&mut self, response: Response) -> Option<Change> {
        if self.pending.map(|(id, _)| id) != Some(response.id()) {
            return None;
        }
        self.pending = None;
        match response {
            Response::Pinged { result, .. } => {
                let server = self.server.clone();
                match (result, server) {
                    (Ok(()), Some(server)) => {
                        let previous_account = self.coda_account();
                        self.request(Busy::Saving, |id| Request::Save {
                            id,
                            server,
                            previous_account,
                        });
                    }
                    (Err(message), _) => {
                        self.server = None;
                        self.set_form_error(message);
                    }
                    (Ok(()), None) => {}
                }
                None
            }
            Response::Saved { result, .. } => {
                let server = self.server.take();
                match (result, server) {
                    (Ok(saved), Some(server)) => {
                        let account = secret_store::account(&server.url, &server.username);
                        self.context.navidrome = Navidrome::Coda {
                            url: server.url,
                            username: server.username,
                            account,
                        };
                        self.form = None;
                        self.notice = None;
                        Some(Change::Connected(saved))
                    }
                    (Err(message), _) => {
                        self.set_form_error(message);
                        None
                    }
                    (Ok(_), None) => None,
                }
            }
            Response::Disconnected { result, .. } => match result {
                Ok(()) => {
                    self.context.navidrome = Navidrome::NotConfigured;
                    self.notice = Some("Disconnected from Navidrome".into());
                    Some(Change::Disconnected)
                }
                Err(message) => {
                    self.notice = Some(message);
                    None
                }
            },
        }
    }

    fn coda_account(&self) -> Option<String> {
        match &self.context.navidrome {
            Navidrome::Coda { account, .. } => Some(account.clone()),
            _ => None,
        }
    }

    fn set_form_error(&mut self, message: String) {
        match self.form.as_mut() {
            Some((_, form)) => form.set_error(message),
            None => self.notice = Some(message),
        }
    }

    fn request(&mut self, busy: Busy, make: impl FnOnce(RequestId) -> Request) {
        self.next_id += 1;
        let id = self.next_id;
        self.pending = Some((id, busy));
        self.requests.push(make(id));
    }

    /// Why a server cannot be saved here, if it cannot.
    fn refusal(&self) -> Option<String> {
        let blocked = self.blocked_by();
        if !blocked.is_empty() {
            return Some(navidrome_blocked_message(&blocked));
        }
        if self.context.config_path.is_none() {
            return Some(NO_CONFIG_LOCATION.into());
        }
        None
    }

    fn start_form(&mut self, kind: FormKind) {
        self.notice = None;
        if let Some(reason) = self.refusal() {
            self.notice = Some(reason);
            return;
        }
        // Prefill only what the input can hold exactly.
        let fits = |value: &str| LineInput::holds_exactly(MAX_QUERY_CHARS, value);
        let form = match (&self.context.navidrome, kind) {
            (Navidrome::Coda { url, username, .. }, FormKind::Edit) => ServerForm::new(
                Some(url.as_str()).filter(|u| fits(u)).unwrap_or_default(),
                Some(username.as_str())
                    .filter(|u| fits(u))
                    .unwrap_or_default(),
            ),
            _ => ServerForm::new("", ""),
        };
        self.form = Some((kind, form));
    }

    fn submit(&mut self, server: Server) {
        if let Some(reason) = self.refusal() {
            // Not reachable through the overlay; never ping or store.
            self.set_form_error(reason);
            return;
        }
        self.server = Some(server.clone());
        self.request(Busy::Connecting, |id| Request::Ping { id, server });
    }

    fn disconnect(&mut self, armed: Option<Instant>, now: Instant) {
        let Some(account) = self.coda_account() else {
            return;
        };
        if self.context.music_dir.is_none() {
            self.notice = Some(KEEP_A_SOURCE.into());
            return;
        }
        if !self.context.file_has_music_dir {
            self.notice = Some(KEEP_A_SOURCE_IN_FILE.into());
            return;
        }
        if self.context.config_path.is_none() {
            self.notice = Some(NO_CONFIG_LOCATION.into());
            return;
        }
        if armed.is_none_or(|until| now >= until) {
            self.disconnect_armed_until = Some(now + DISCONNECT_CONFIRM);
            self.notice = Some(DISCONNECT_PROMPT.into());
            return;
        }
        self.notice = None;
        self.request(Busy::Disconnecting, |id| Request::Disconnect {
            id,
            previous_account: Some(account),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{NAVIDROME_URL_ENV, NAVIDROME_USER_ENV};
    use crate::navidrome::Password;
    use crate::navidrome::client::parse_base_url;

    fn url(s: &str) -> Url {
        parse_base_url(s).unwrap()
    }

    fn coda(music: bool) -> Context {
        Context {
            config_path: Some("/cfg/config.toml".into()),
            music_dir: music.then(|| "/music".into()),
            music_origin: None,
            file_has_music_dir: music,
            navidrome: Navidrome::Coda {
                url: url("https://old.example/nd"),
                username: "me".into(),
                account: "me@old.example".into(),
            },
            env_overrides: Vec::new(),
        }
    }

    fn not_configured() -> Context {
        Context {
            navidrome: Navidrome::NotConfigured,
            ..coda(true)
        }
    }

    fn client() -> navidrome::Client {
        navidrome::Client::new(
            url("https://new.example"),
            "you".into(),
            Password::new("p".into()),
        )
        .unwrap()
    }

    fn typed(settings: &mut Settings, text: &str) {
        let now = Instant::now();
        for c in text.chars() {
            settings.handle_input(Input::Char(c), now);
        }
    }

    fn key(settings: &mut Settings, input: Input) {
        settings.handle_input(input, Instant::now());
    }

    fn one_request(settings: &mut Settings) -> Request {
        let mut requests = settings.take_requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        requests.pop().unwrap()
    }

    /// Fills the open form (after any prefill) and submits: a ping.
    fn submit(settings: &mut Settings, url: &str, user: &str, password: &str) -> RequestId {
        if !url.is_empty() {
            typed(settings, url);
            key(settings, Input::Enter);
        }
        if !user.is_empty() {
            typed(settings, user);
            key(settings, Input::Enter);
        }
        typed(settings, password);
        key(settings, Input::Enter);
        match one_request(settings) {
            Request::Ping { id, server } => {
                assert_eq!(server.password.expose(), password);
                id
            }
            other => panic!("expected a ping, got {other:?}"),
        }
    }

    fn connected(settings: &mut Settings) -> Option<Change> {
        let id = submit(settings, "new.example", "you", "pw");
        assert_eq!(settings.busy(), Some(Busy::Connecting));
        assert!(
            settings
                .handle_response(Response::Pinged { id, result: Ok(()) })
                .is_none()
        );
        let Request::Save {
            id,
            server,
            previous_account,
        } = one_request(settings)
        else {
            panic!("expected a save");
        };
        assert_eq!(server.url.as_str(), "https://new.example/");
        assert_eq!(previous_account, None);
        assert_eq!(settings.busy(), Some(Busy::Saving));
        settings.handle_response(Response::Saved {
            id,
            result: Ok(Saved {
                client: client(),
                scrobble: true,
            }),
        })
    }

    #[test]
    fn ownership_follows_the_password_source_and_the_environment() {
        let config = |password| NavidromeConfig {
            url: url("https://h.example"),
            username: "me".into(),
            password,
            scrobble: true,
        };
        let keychain = || PasswordSource::Keychain {
            account: "me@h.example".into(),
            config_path: None,
        };
        assert!(matches!(
            Navidrome::from_config(None, &[]),
            Navidrome::NotConfigured
        ));
        let owner = Navidrome::from_config(Some(&config(keychain())), &[]);
        assert!(matches!(&owner, Navidrome::Coda { account, .. } if account == "me@h.example"));
        assert_eq!(owner.account_label().unwrap(), "me @ h.example");
        let outside =
            |config: NavidromeConfig, overrides: &[&'static str]| match Navidrome::from_config(
                Some(&config),
                overrides,
            ) {
                Navidrome::Outside { by, .. } => by,
                other => panic!("{other:?}"),
            };
        assert_eq!(
            outside(config(keychain()), &[NAVIDROME_URL_ENV]),
            [NAVIDROME_URL_ENV]
        );
        assert_eq!(
            outside(config(keychain()), &[NAVIDROME_PASSWORD_ENV]),
            [NAVIDROME_PASSWORD_ENV]
        );
        assert_eq!(
            outside(config(PasswordSource::Command(vec!["pass".into()])), &[]),
            ["password_command"]
        );
        let value = || PasswordSource::Value(Password::new("p".into()));
        assert_eq!(outside(config(value()), &[]), ["password"]);
        assert_eq!(
            outside(config(value()), &[NAVIDROME_PASSWORD_ENV]),
            [NAVIDROME_PASSWORD_ENV]
        );
        let port = Navidrome::from_config(
            Some(&NavidromeConfig {
                url: url("http://h:4533"),
                ..config(keychain())
            }),
            &[],
        );
        assert_eq!(port.account_label().unwrap(), "me @ h:4533");
    }

    #[test]
    fn connect_pings_then_saves_then_reports_the_new_client() {
        let mut settings = Settings::new(not_configured());
        settings.open();
        // Edit and disconnect do nothing without a Coda-owned server.
        key(&mut settings, Input::Char('e'));
        key(&mut settings, Input::Char('d'));
        assert!(settings.form().is_none());
        key(&mut settings, Input::Char('c'));
        let (kind, form) = settings.form().unwrap();
        assert_eq!(kind, FormKind::Connect);
        assert_eq!(form.url_input(), "");
        let change = connected(&mut settings);
        let Some(Change::Connected(saved)) = change else {
            panic!("{change:?}");
        };
        assert!(saved.scrobble);
        assert!(settings.form().is_none());
        assert!(settings.is_open());
        match settings.navidrome() {
            Navidrome::Coda {
                url,
                username,
                account,
            } => {
                assert_eq!(url.as_str(), "https://new.example/");
                assert_eq!(username, "you");
                assert_eq!(account, "you@new.example");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn edit_prefills_all_but_the_password_and_replaces_the_old_account() {
        let mut settings = Settings::new(coda(true));
        settings.open();
        key(&mut settings, Input::Char('c'));
        assert!(settings.form().is_none(), "already configured");
        key(&mut settings, Input::Char('e'));
        let (kind, form) = settings.form().unwrap();
        assert_eq!(kind, FormKind::Edit);
        assert_eq!(form.url_input(), "https://old.example/nd/");
        assert_eq!(form.username_input(), "me");
        assert_eq!(form.password_masked(), "");
        assert_eq!(form.focus(), crate::onboarding::Field::Password);
        let id = submit(&mut settings, "", "", "secret");
        settings.handle_response(Response::Pinged { id, result: Ok(()) });
        let Request::Save {
            previous_account, ..
        } = one_request(&mut settings)
        else {
            panic!()
        };
        assert_eq!(previous_account.as_deref(), Some("me@old.example"));
    }

    #[test]
    fn failed_ping_and_failed_save_show_inline_and_can_be_retried() {
        let mut settings = Settings::new(not_configured());
        settings.open();
        key(&mut settings, Input::Char('c'));
        let id = submit(&mut settings, "h.example", "me", "wrong");
        assert!(
            settings
                .handle_response(Response::Pinged {
                    id,
                    result: Err("Authentication failed".into()),
                })
                .is_none()
        );
        assert_eq!(settings.busy(), None);
        let (_, form) = settings.form().unwrap();
        assert_eq!(form.error(), Some("Authentication failed"));
        // Every field is filled: Enter retries at once.
        key(&mut settings, Input::Enter);
        let Request::Ping { id, .. } = one_request(&mut settings) else {
            panic!()
        };
        settings.handle_response(Response::Pinged { id, result: Ok(()) });
        let Request::Save { id, .. } = one_request(&mut settings) else {
            panic!()
        };
        let message = "Could not store the password: the macOS Keychain is locked or denied access";
        assert!(
            settings
                .handle_response(Response::Saved {
                    id,
                    result: Err(message.into()),
                })
                .is_none()
        );
        assert!(matches!(settings.navidrome(), Navidrome::NotConfigured));
        assert_eq!(settings.form().unwrap().1.error(), Some(message));
        key(&mut settings, Input::Enter);
        assert!(matches!(one_request(&mut settings), Request::Ping { .. }));
    }

    #[test]
    fn stale_and_abandoned_answers_are_ignored() {
        let mut settings = Settings::new(not_configured());
        settings.open();
        key(&mut settings, Input::Char('c'));
        let id = submit(&mut settings, "h.example", "me", "pw");
        // Esc while connecting stops waiting; the form stays.
        key(&mut settings, Input::Esc);
        assert_eq!(settings.busy(), None);
        assert!(settings.form().is_some());
        settings.handle_response(Response::Pinged { id, result: Ok(()) });
        assert!(settings.take_requests().is_empty(), "no save");
        // An answer for a request never made.
        assert!(
            settings
                .handle_response(Response::Saved {
                    id: 99,
                    result: Ok(Saved {
                        client: client(),
                        scrobble: true,
                    }),
                })
                .is_none()
        );
        assert!(matches!(settings.navidrome(), Navidrome::NotConfigured));
    }

    #[test]
    fn saving_cannot_be_interrupted_or_closed() {
        let mut settings = Settings::new(not_configured());
        settings.open();
        key(&mut settings, Input::Char('c'));
        let id = submit(&mut settings, "h.example", "me", "pw");
        settings.handle_response(Response::Pinged { id, result: Ok(()) });
        settings.take_requests();
        for input in [Input::Esc, Input::Char(','), Input::Char('x'), Input::Enter] {
            key(&mut settings, input);
        }
        assert_eq!(settings.busy(), Some(Busy::Saving));
        assert!(settings.is_open());
        assert!(settings.form().is_some());
    }

    #[test]
    fn esc_leaves_the_form_then_the_overlay() {
        let mut settings = Settings::new(not_configured());
        settings.open();
        key(&mut settings, Input::Char('c'));
        typed(&mut settings, "abc");
        key(&mut settings, Input::Esc);
        assert!(settings.form().is_none());
        assert!(settings.is_open());
        key(&mut settings, Input::Char(','));
        assert!(!settings.is_open());
        settings.open();
        key(&mut settings, Input::Esc);
        assert!(!settings.is_open());
    }

    #[test]
    fn paste_goes_into_the_focused_field() {
        let mut settings = Settings::new(not_configured());
        settings.open();
        settings.handle_paste("ignored on the overview");
        key(&mut settings, Input::Char('c'));
        settings.handle_paste("https://h.example\n");
        assert_eq!(settings.form().unwrap().1.url_input(), "https://h.example");
    }

    #[test]
    fn env_url_or_user_without_password_refuses_connect_and_edit() {
        for overrides in [
            vec![NAVIDROME_URL_ENV],
            vec![NAVIDROME_USER_ENV],
            vec![NAVIDROME_URL_ENV, NAVIDROME_USER_ENV],
        ] {
            for context in [not_configured(), coda(true)] {
                let mut settings = Settings::new(Context {
                    env_overrides: overrides.clone(),
                    ..context
                });
                settings.open();
                key(&mut settings, Input::Char('c'));
                key(&mut settings, Input::Char('e'));
                assert!(settings.form().is_none());
                let notice = settings.notice().unwrap().to_owned();
                for name in &overrides {
                    assert!(notice.contains(name), "{notice}");
                }
                assert!(!notice.contains("example"), "{notice}");
                assert!(settings.take_requests().is_empty());
            }
        }
        // With the password variable too, nothing is blocked.
        let settings = Settings::new(Context {
            env_overrides: vec![NAVIDROME_URL_ENV, NAVIDROME_PASSWORD_ENV],
            ..not_configured()
        });
        assert!(settings.blocked_by().is_empty());
    }

    #[test]
    fn nothing_is_saved_without_a_config_location() {
        let mut settings = Settings::new(Context {
            config_path: None,
            ..not_configured()
        });
        settings.open_connect();
        assert!(settings.form().is_none());
        assert_eq!(settings.notice(), Some(NO_CONFIG_LOCATION));
    }

    #[test]
    fn outside_settings_offer_no_actions() {
        let mut settings = Settings::new(Context {
            navidrome: Navidrome::Outside {
                url: url("https://h.example"),
                username: "me".into(),
                by: vec!["password_command"],
            },
            ..coda(true)
        });
        settings.open();
        for c in ['c', 'e', 'd', 'd'] {
            key(&mut settings, Input::Char(c));
        }
        assert!(settings.form().is_none());
        assert!(settings.take_requests().is_empty());
        settings.open_connect();
        assert!(settings.form().is_none());
    }

    #[test]
    fn disconnect_needs_two_presses_soon_and_nothing_in_between() {
        let mut settings = Settings::new(coda(true));
        settings.open();
        let now = Instant::now();
        settings.handle_input(Input::Char('d'), now);
        assert_eq!(settings.notice(), Some(DISCONNECT_PROMPT));
        assert!(settings.take_requests().is_empty());
        // Too late.
        settings.handle_input(Input::Char('d'), now + DISCONNECT_CONFIRM);
        assert!(settings.take_requests().is_empty());
        // Something in between.
        let later = now + DISCONNECT_CONFIRM;
        settings.handle_input(Input::Char('x'), later);
        settings.handle_input(Input::Char('d'), later);
        assert!(settings.take_requests().is_empty());
        settings.handle_input(Input::Char('d'), later);
        let Request::Disconnect {
            id,
            previous_account,
        } = one_request(&mut settings)
        else {
            panic!()
        };
        assert_eq!(previous_account.as_deref(), Some("me@old.example"));
        assert_eq!(settings.busy(), Some(Busy::Disconnecting));
        let change = settings.handle_response(Response::Disconnected { id, result: Ok(()) });
        assert!(matches!(change, Some(Change::Disconnected)));
        assert!(matches!(settings.navidrome(), Navidrome::NotConfigured));
    }

    #[test]
    fn a_failed_disconnect_keeps_the_server() {
        let mut settings = Settings::new(coda(true));
        settings.open();
        key(&mut settings, Input::Char('d'));
        key(&mut settings, Input::Char('d'));
        let Request::Disconnect { id, .. } = one_request(&mut settings) else {
            panic!()
        };
        let change = settings.handle_response(Response::Disconnected {
            id,
            result: Err("Could not write /cfg/config.toml: denied".into()),
        });
        assert!(change.is_none());
        assert!(matches!(settings.navidrome(), Navidrome::Coda { .. }));
        assert!(settings.notice().unwrap().starts_with("Could not write"));
    }

    #[test]
    fn disconnecting_the_last_source_is_refused() {
        let mut settings = Settings::new(coda(false));
        settings.open();
        key(&mut settings, Input::Char('d'));
        key(&mut settings, Input::Char('d'));
        assert_eq!(settings.notice(), Some(KEEP_A_SOURCE));
        assert!(settings.take_requests().is_empty());
    }

    #[test]
    fn disconnect_is_refused_when_the_folder_is_not_in_the_file() {
        for origin in [MusicDirArg::Argument, MusicDirArg::Env] {
            let mut settings = Settings::new(Context {
                music_origin: Some(origin),
                file_has_music_dir: false,
                ..coda(true)
            });
            settings.open();
            key(&mut settings, Input::Char('d'));
            key(&mut settings, Input::Char('d'));
            assert_eq!(settings.notice(), Some(KEEP_A_SOURCE_IN_FILE));
            assert!(settings.take_requests().is_empty());
        }
        // Overridden for this run, but the file has its own folder too.
        let mut settings = Settings::new(Context {
            music_origin: Some(MusicDirArg::Env),
            file_has_music_dir: true,
            ..coda(true)
        });
        settings.open();
        key(&mut settings, Input::Char('d'));
        key(&mut settings, Input::Char('d'));
        assert!(matches!(
            one_request(&mut settings),
            Request::Disconnect { .. }
        ));
    }

    #[test]
    fn debug_output_hides_credentials_and_accounts() {
        let mut settings = Settings::new(coda(true));
        settings.open();
        key(&mut settings, Input::Char('e'));
        typed(&mut settings, "hunter2");
        key(&mut settings, Input::Enter);
        let requests = settings.take_requests();
        let text = format!("{settings:?} {requests:?} {:?}", coda(true));
        for secret in ["hunter2", "old.example", "me@", "/cfg", "/music"] {
            assert!(!text.contains(secret), "{secret}: {text}");
        }
    }
}
