//! Runs the setup ([`Onboarding`]), on first run or for `coda --setup`, in
//! its own short TUI session, before mpv is started, and performs its
//! requests: folder checks and keychain/config writes on blocking threads,
//! the ping on a Tokio task. Nothing here blocks the input loop.
//!
//! On a redo, the keychain entry Coda wrote for the previous account is
//! deleted (best effort) once the new config has been written, if the
//! account changed or Navidrome was removed; an unchanged account's entry
//! is overwritten in place.
//!
//! The terminal lifecycle mirrors `app::run`: the same panic hook (the
//! previous hook is put back afterwards, so the player's session installs
//! its own on a clean slate), a `ManuallyDrop` terminal released
//! explicitly, and restoration on every exit path, including signals.
//!
//! The log records only kinds and outcomes: never the URL, host,
//! username, folder or password.

use std::io;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{EnableBracketedPaste, Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::app::{self, PanicHook};
use crate::config::{self, ConfigError, Env, MusicDirArg, SetupStart};
use crate::config_writer::{self, NavidromeSection, SetupConfig, SetupNavidrome};
use crate::navidrome;
use crate::onboarding::{
    Busy, Input, Onboarding, Outcome, Redo, Request, Response, SaveError, Server,
};
use crate::secret_store::{self, STORE_NAME, SecretError, SecretStore};
use crate::ui::onboarding::{self as view, View};

/// How often the spinner moves while something is in progress.
const SPINNER_TICK: Duration = Duration::from_millis(100);
/// How long a terminating signal waits for a running save.
const SAVE_WAIT: Duration = Duration::from_secs(5);

/// Where setup writes, and how it reaches the credential store.
pub struct Context {
    /// The config file to write (the one `Config::load` reads).
    pub config_path: PathBuf,
    pub env: Env,
    /// Opens the credential store (on a blocking thread).
    pub open_store: fn() -> Result<SecretStore, SecretError>,
    /// Set for `coda --setup`.
    pub redo: Option<RedoContext>,
}

/// What `coda --setup` starts from. `Debug` hides the account.
pub struct RedoContext {
    pub prefill: Redo,
    /// The keychain entry Coda wrote for the current config, if any.
    pub previous_account: Option<String>,
}

impl RedoContext {
    /// From what [`config::read_for_setup`] found, how the music folder
    /// was given to this run, and the environment.
    pub fn from_start(start: &SetupStart, music_override: Option<MusicDirArg>, env: &Env) -> Self {
        Self {
            prefill: Redo {
                music_dir: start.music_dir.clone(),
                url: start.url.clone(),
                username: start.username.clone(),
                music_override,
                navidrome_overrides: config::navidrome_env_overrides(env),
                hand_written_password: start.hand_written_password,
                has_navidrome: start.has_navidrome,
                url_had_credentials: start.url_had_credentials,
                url_withheld: start.url_withheld,
                file_has_content: start.has_content,
            },
            previous_account: start.keychain_account.clone(),
        }
    }
}

impl std::fmt::Debug for RedoContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedoContext")
            .field("prefill", &self.prefill)
            .field("previous_account", &self.previous_account.is_some())
            .finish()
    }
}

impl Context {
    /// First-run setup with the real credential store.
    pub fn new(config_path: PathBuf, env: Env) -> Self {
        Self {
            config_path,
            env,
            open_store: SecretStore::system,
            redo: None,
        }
    }

    /// `coda --setup` with the real credential store.
    pub fn redo(config_path: PathBuf, env: Env, redo: RedoContext) -> Self {
        Self {
            redo: Some(redo),
            ..Self::new(config_path, env)
        }
    }

    /// `~/Music`, if that folder exists.
    fn music_prefill(&self) -> Option<&'static str> {
        let home = self.env.home.as_ref().map(PathBuf::from)?;
        (home.is_absolute() && home.join("Music").is_dir()).then_some("~/Music")
    }
}

/// Runs setup until it saves, the user leaves, or a signal arrives (which
/// counts as leaving). Must be awaited to completion (see `app::run`).
pub async fn run(context: Context) -> io::Result<Outcome> {
    let mut signals = Signals::new()?;
    // Kept so it can be put back when setup ends.
    let previous: Arc<PanicHook> = Arc::new(std::panic::take_hook());
    let terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            reinstall(previous);
            app::restore_terminal();
            return Err(error);
        }
    };
    let chained = Arc::clone(&previous);
    app::install_panic_hook(Box::new(move |info| chained(info)));
    if let Err(error) = crossterm::execute!(io::stdout(), EnableBracketedPaste) {
        tracing::warn!(%error, "could not enable bracketed paste");
    }
    // Released explicitly below; see `app::run` and LESSONS.md.
    let mut terminal = ManuallyDrop::new(terminal);

    let redo = context.redo.is_some();
    tracing::info!(redo, "setup started");
    let mut setup = match &context.redo {
        Some(redo) => Onboarding::redo(redo.prefill.clone(), context.music_prefill()),
        None => Onboarding::new(context.music_prefill()),
    };
    let result = event_loop(&mut terminal, &mut setup, &context, &mut signals).await;
    match &result {
        Ok(outcome) => tracing::info!(?outcome, redo, "setup finished"),
        Err(error) => tracing::error!(%error, "terminal error during setup"),
    }
    app::release_terminal(ManuallyDrop::into_inner(terminal));
    app::restore_terminal();
    reinstall(previous);
    result
}

/// Puts the hook that was active before setup back.
fn reinstall(previous: Arc<PanicHook>) {
    std::panic::set_hook(Box::new(move |info| previous(info)));
}

struct Signals {
    terminate: tokio::signal::unix::Signal,
    hangup: tokio::signal::unix::Signal,
    interrupt: tokio::signal::unix::Signal,
}

impl Signals {
    fn new() -> io::Result<Self> {
        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            hangup: signal(SignalKind::hangup())?,
            interrupt: signal(SignalKind::interrupt())?,
        })
    }
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    setup: &mut Onboarding,
    context: &Context,
    signals: &mut Signals,
) -> io::Result<Outcome> {
    let mut input = EventStream::new();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut tick = tokio::time::interval(SPINNER_TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut view = View {
        config_path: context.config_path.display().to_string(),
        store_name: STORE_NAME,
        spinner: 0,
    };
    let mut dirty = true;
    loop {
        for request in setup.take_requests() {
            spawn_request(request, context, &tx);
        }
        if let Some(outcome) = setup.outcome() {
            return Ok(outcome);
        }
        if dirty {
            terminal.draw(|frame| view::draw(frame, setup, &view))?;
            dirty = false;
        }
        tokio::select! {
            event = input.next() => match event {
                Some(Ok(Event::Key(key))) => {
                    if key.kind != KeyEventKind::Release
                        && let Some(input) = view::map_key(key)
                    {
                        setup.handle_input(input);
                        dirty = true;
                    }
                }
                Some(Ok(Event::Paste(text))) => {
                    setup.handle_input(Input::Paste(text));
                    dirty = true;
                }
                Some(Ok(Event::Resize(..))) => dirty = true,
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(error),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "terminal input closed",
                    ));
                }
            },
            // The loop holds a sender, so this never yields `None`.
            Some(response) = rx.recv() => {
                setup.handle_response(response);
                dirty = true;
            },
            _ = tick.tick(), if setup.busy().is_some() => {
                view.spinner = view.spinner.wrapping_add(1);
                dirty = true;
            },
            _ = signals.terminate.recv() => {
                return Ok(leave_on_signal("SIGTERM", setup, &mut rx, SAVE_WAIT).await);
            },
            _ = signals.hangup.recv() => {
                return Ok(leave_on_signal("SIGHUP", setup, &mut rx, SAVE_WAIT).await);
            },
            _ = signals.interrupt.recv() => {
                return Ok(leave_on_signal("SIGINT", setup, &mut rx, SAVE_WAIT).await);
            },
        }
    }
}

/// Leaves setup after a terminating signal. A running save is first given
/// up to `wait` ([`SAVE_WAIT`]) to answer, so the process does not exit between the
/// keychain write and the config write (or the cleanup after a failed one).
async fn leave_on_signal(
    name: &'static str,
    setup: &mut Onboarding,
    rx: &mut mpsc::UnboundedReceiver<Response>,
    wait: Duration,
) -> Outcome {
    tracing::info!(signal = name, "signal received during setup");
    if setup.busy() == Some(Busy::Saving) {
        let finish = async {
            while setup.busy() == Some(Busy::Saving) {
                match rx.recv().await {
                    Some(response) => setup.handle_response(response),
                    None => break,
                }
            }
        };
        if tokio::time::timeout(wait, finish).await.is_err() {
            tracing::warn!("the save did not finish before exit");
        }
    }
    Outcome::Exit
}

/// Sends `fallback` unless [`Reply::send`] was called: a request whose
/// task panics or is cancelled still answers (LESSONS.md, "Every spawned
/// request must answer exactly once").
struct Reply {
    tx: mpsc::UnboundedSender<Response>,
    fallback: Option<Response>,
}

impl Reply {
    fn new(tx: &mpsc::UnboundedSender<Response>, fallback: Response) -> Self {
        Self {
            tx: tx.clone(),
            fallback: Some(fallback),
        }
    }

    fn send(mut self, response: Response) {
        self.fallback = None;
        let _ = self.tx.send(response);
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if let Some(response) = self.fallback.take() {
            let _ = self.tx.send(response);
        }
    }
}

const INTERRUPTED: &str = "Something went wrong; please try again";

fn spawn_request(request: Request, context: &Context, tx: &mpsc::UnboundedSender<Response>) {
    match request {
        Request::CheckFolder { id, typed } => {
            let reply = Reply::new(
                tx,
                Response::FolderChecked {
                    id,
                    result: Err(INTERRUPTED.into()),
                },
            );
            let env = context.env.clone();
            tokio::task::spawn_blocking(move || {
                let result = check_folder(&typed, &env);
                reply.send(Response::FolderChecked { id, result });
            });
        }
        Request::Ping { id, server } => {
            let reply = Reply::new(
                tx,
                Response::Pinged {
                    id,
                    result: Err(INTERRUPTED.into()),
                },
            );
            tokio::spawn(async move {
                let result = ping(server).await;
                reply.send(Response::Pinged { id, result });
            });
        }
        Request::Save {
            id,
            music_dir,
            server,
            keep_navidrome,
        } => {
            let reply = Reply::new(
                tx,
                Response::Saved {
                    id,
                    result: Err(SaveError::Config(INTERRUPTED.into())),
                },
            );
            let path = context.config_path.clone();
            let open_store = context.open_store;
            let previous = context
                .redo
                .as_ref()
                .and_then(|redo| redo.previous_account.clone());
            tokio::task::spawn_blocking(move || {
                let navidrome = match server {
                    Some(server) => SaveNavidrome::Server(server),
                    None if keep_navidrome => SaveNavidrome::Keep,
                    None => SaveNavidrome::Remove,
                };
                let result = save(&path, music_dir, navidrome, open_store, previous.as_deref());
                reply.send(Response::Saved { id, result });
            });
        }
    }
}

/// Validates a typed folder like `music_dir` in the config file.
pub fn check_folder(typed: &str, env: &Env) -> Result<(), String> {
    let Some(path) = config::expand_music_dir(typed, env) else {
        return Err("Use an absolute path or one starting with ~/".into());
    };
    match config::validate_music_dir(path) {
        Ok(_) => Ok(()),
        Err(ConfigError::NotFound(_)) => Err("That folder does not exist".into()),
        Err(ConfigError::NotADirectory(_)) => Err("That is not a folder".into()),
        Err(_) => Err("That folder cannot be read".into()),
    }
}

/// Pings with a temporary client. The message is the client's short error
/// text; the log gets only its detail (no URL, host or response data).
pub async fn ping(server: Server) -> Result<(), String> {
    let result = match navidrome::Client::new(server.url, server.username, server.password) {
        Ok(client) => client.ping().await,
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => {
            tracing::info!("setup: navidrome ping ok");
            Ok(())
        }
        Err(error) => {
            tracing::info!(detail = %error.detail(), "setup: navidrome ping failed");
            Err(ping_message(&error))
        }
    }
}

fn ping_message(error: &navidrome::Error) -> String {
    let text = match error {
        navidrome::Error::Decode(_) => "Not a Subsonic server (unexpected response)".to_owned(),
        navidrome::Error::Http(404) => {
            "Server returned HTTP 404; is this the Navidrome URL?".to_owned()
        }
        error => error.to_string(),
    };
    capitalize(&text)
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// What a save does with Navidrome.
enum SaveNavidrome {
    Remove,
    /// Keep `[navidrome]` as it is; the keychain is not touched.
    Keep,
    Server(Server),
}

/// Stores the password, then writes the config. Blocking.
///
/// If the config write fails after the password was stored, the new entry
/// is removed again (best effort), so a failed setup leaves nothing
/// behind, unless it is `previous`'s, which the old config still uses.
/// Once the config is written, `previous` (the entry Coda wrote for the
/// old config) is deleted best effort if it is no longer used: the account
/// changed or Navidrome was removed (not when it is kept).
fn save(
    path: &Path,
    music_dir: Option<String>,
    navidrome: SaveNavidrome,
    open_store: fn() -> Result<SecretStore, SecretError>,
    previous: Option<&str>,
) -> Result<(), SaveError> {
    let mut stored = None;
    let mut new_account = None;
    let keep = matches!(navidrome, SaveNavidrome::Keep);
    let navidrome = match navidrome {
        SaveNavidrome::Server(server) => {
            let account = secret_store::account(&server.url, &server.username);
            new_account = Some(account.clone());
            let store = open_store()
                .and_then(|store| store.set(&account, &server.password).map(|()| store));
            match store {
                Ok(store) => stored = Some((store, account)),
                Err(error) => {
                    tracing::warn!(kind = error.kind(), "setup: could not store the password");
                    return Err(SaveError::Keychain(format!(
                        "Could not store the password: {error}"
                    )));
                }
            }
            tracing::info!("setup: password stored");
            NavidromeSection::Write(SetupNavidrome {
                url: server.url,
                username: server.username,
            })
        }
        SaveNavidrome::Keep => NavidromeSection::Keep,
        SaveNavidrome::Remove => NavidromeSection::Remove,
    };
    let config = SetupConfig {
        music_dir,
        navidrome,
    };
    match config_writer::write(path, &config) {
        Ok(backup) => {
            tracing::info!(backup = backup.is_some(), "setup: config written");
            if let Some(previous) = previous.filter(|p| !keep && new_account.as_deref() != Some(*p))
            {
                delete_previous(
                    previous,
                    stored.as_ref().map(|(store, _)| store),
                    open_store,
                );
            }
            Ok(())
        }
        Err(error) => {
            tracing::warn!(error = %error.kind(), "setup: could not write the config");
            if let Some((store, account)) = stored.filter(|(_, a)| Some(a.as_str()) != previous) {
                match store.delete(&account) {
                    Ok(()) => tracing::info!("setup: stored password removed again"),
                    Err(error) => tracing::warn!(
                        kind = error.kind(),
                        "setup: could not remove the stored password"
                    ),
                }
            }
            Err(SaveError::Config(format!(
                "Could not write {}: {error}",
                path.display()
            )))
        }
    }
}

/// Deletes the entry for the previous account, best effort: a failure is
/// logged by kind only and does not fail the save (the config no longer
/// refers to it).
fn delete_previous(
    account: &str,
    store: Option<&SecretStore>,
    open_store: fn() -> Result<SecretStore, SecretError>,
) {
    let result = match store {
        Some(store) => store.delete(account),
        None => open_store().and_then(|store| store.delete(account)),
    };
    match result {
        Ok(()) => tracing::info!("setup: the previous account's password was removed"),
        Err(error) => tracing::warn!(
            kind = error.kind(),
            "setup: could not remove the previous account's password"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Cli, Config, PasswordSource};
    use crate::navidrome::Password;
    use crate::navidrome::client::parse_base_url;
    use std::sync::OnceLock;

    fn server(url: &str, password: &str) -> Server {
        Server {
            url: parse_base_url(url).unwrap(),
            username: "me".into(),
            password: Password::new(password.into()),
        }
    }

    #[test]
    fn folder_checks_follow_the_config_rules() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join("Music")).unwrap();
        std::fs::write(home.join("file"), b"x").unwrap();
        let env = Env {
            home: Some(home.clone().into()),
            ..Env::default()
        };
        assert_eq!(check_folder("~/Music", &env), Ok(()));
        assert_eq!(
            check_folder(home.join("Music").to_str().unwrap(), &env),
            Ok(())
        );
        assert_eq!(
            check_folder("Music", &env),
            Err("Use an absolute path or one starting with ~/".into())
        );
        assert_eq!(
            check_folder("~/Nope", &env),
            Err("That folder does not exist".into())
        );
        assert_eq!(
            check_folder("~/file", &env),
            Err("That is not a folder".into())
        );
    }

    #[test]
    fn ping_messages_are_short_and_capitalised() {
        assert_eq!(
            ping_message(&navidrome::Error::Auth),
            "Authentication failed"
        );
        assert_eq!(
            ping_message(&navidrome::Error::Decode("x".into())),
            "Not a Subsonic server (unexpected response)"
        );
        assert_eq!(
            ping_message(&navidrome::Error::Http(500)),
            "Server returned HTTP 500"
        );
    }

    /// One mock store per test, shared by the `fn` pointers below.
    fn store_a() -> Result<SecretStore, SecretError> {
        static STORE: OnceLock<SecretStore> = OnceLock::new();
        Ok(STORE.get_or_init(SecretStore::mock).clone())
    }

    fn unavailable() -> Result<SecretStore, SecretError> {
        Err(SecretError::Unavailable)
    }

    #[tokio::test]
    async fn save_stores_the_password_then_writes_a_loadable_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coda").join("config.toml");
        save(
            &path,
            None,
            SaveNavidrome::Server(server("http://localhost:4533", "s3cret")),
            store_a,
            None,
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("s3cret"), "{text}");
        let env = Env {
            config_path: Some(path.clone().into()),
            ..Env::default()
        };
        let nd = Config::load(Cli::default(), &env)
            .unwrap()
            .navidrome
            .unwrap();
        assert!(matches!(nd.password, PasswordSource::Keychain { .. }));
        let password = nd
            .password
            .resolve_with(Duration::from_secs(1), store_a)
            .await
            .unwrap();
        assert_eq!(password.expose(), "s3cret");
    }

    #[test]
    fn keychain_failure_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let error = save(
            &path,
            Some("/m".into()),
            SaveNavidrome::Server(server("https://h", "p")),
            unavailable,
            None,
        )
        .unwrap_err();
        let SaveError::Keychain(message) = error else {
            panic!("{error:?}")
        };
        assert!(
            message.starts_with("Could not store the password: "),
            "{message}"
        );
        assert!(!path.exists());
    }

    #[test]
    fn config_failure_is_reported_with_the_path() {
        let dir = tempfile::tempdir().unwrap();
        // The parent is a file, so the directory cannot be created.
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"").unwrap();
        let path = blocker.join("config.toml");
        let error = save(
            &path,
            Some("/m".into()),
            SaveNavidrome::Remove,
            unavailable,
            None,
        )
        .unwrap_err();
        let SaveError::Config(message) = error else {
            panic!("{error:?}")
        };
        assert!(message.contains("blocker/config.toml"), "{message}");
    }

    async fn ping_against(body: serde_json::Value, password: &str) -> Result<(), String> {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/ping"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&mock)
            .await;
        let result = ping(server(&mock.uri(), password)).await;
        // The password itself never travels.
        for request in mock.received_requests().await.unwrap() {
            assert!(!request.url.as_str().contains(password));
        }
        result
    }

    #[tokio::test]
    async fn ping_success_and_failures_against_a_fake_server() {
        use serde_json::json;
        let ok = json!({ "subsonic-response": { "status": "ok", "version": "1.16.1" } });
        assert_eq!(ping_against(ok, "right-pass").await, Ok(()));
        let auth = json!({ "subsonic-response": {
            "status": "failed", "version": "1.16.1",
            "error": { "code": 40, "message": "Wrong username or password" }
        } });
        assert_eq!(
            ping_against(auth, "wrong-pass").await,
            Err("Authentication failed".into())
        );
        let html = json!({ "hello": "world" });
        assert_eq!(
            ping_against(html, "pw").await,
            Err("Not a Subsonic server (unexpected response)".into())
        );
        // Nothing listening.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        assert_eq!(
            ping(server(&url, "pw")).await,
            Err("Could not reach server".into())
        );
    }

    fn store_b() -> Result<SecretStore, SecretError> {
        static STORE: OnceLock<SecretStore> = OnceLock::new();
        Ok(STORE.get_or_init(SecretStore::mock).clone())
    }

    #[test]
    fn a_failed_config_write_removes_the_stored_password() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"").unwrap();
        let server = server("https://h.example", "pw");
        let account = secret_store::account(&server.url, &server.username);
        let error = save(
            &blocker.join("config.toml"),
            None,
            SaveNavidrome::Server(server),
            store_b,
            None,
        )
        .unwrap_err();
        assert!(matches!(error, SaveError::Config(_)), "{error:?}");
        assert_eq!(
            store_b().unwrap().get(&account).unwrap_err(),
            SecretError::Missing,
            "no orphaned entry"
        );
    }

    /// A mock store of its own for each `fn` pointer.
    macro_rules! own_store {
        ($name:ident) => {
            fn $name() -> Result<SecretStore, SecretError> {
                static STORE: OnceLock<SecretStore> = OnceLock::new();
                Ok(STORE.get_or_init(SecretStore::mock).clone())
            }
        };
    }

    own_store!(store_change);
    own_store!(store_change_fails);
    own_store!(store_same);
    own_store!(store_same_fails);
    own_store!(store_removed);
    own_store!(store_delete_denied);
    own_store!(store_command);
    own_store!(store_keep);
    own_store!(store_drop_folder);
    own_store!(store_wired);

    const OLD: &str = "me@old.example";

    fn with_old_entry(open: fn() -> Result<SecretStore, SecretError>) -> SecretStore {
        let store = open().unwrap();
        store.set(OLD, &Password::new("old-pw".into())).unwrap();
        store
    }

    /// A config written by Coda for the old account, plus `extra`.
    fn old_config(dir: &Path, extra: &str) -> PathBuf {
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            format!(
                "[navidrome]\nurl = \"https://old.example\"\nusername = \"me\"\n\
                 password_keychain = true\n{extra}"
            ),
        )
        .unwrap();
        path
    }

    fn blocked_path(dir: &Path) -> PathBuf {
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"").unwrap();
        blocker.join("config.toml")
    }

    #[test]
    fn redo_context_debug_hides_the_account() {
        let redo = RedoContext {
            prefill: Redo {
                username: Some("secret-user".into()),
                ..Redo::default()
            },
            previous_account: Some("secret-user@secret-host".into()),
        };
        let text = format!("{redo:?}");
        assert!(!text.contains("secret"), "{text}");
    }

    #[test]
    fn an_account_change_deletes_the_old_entry_after_the_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = old_config(dir.path(), "scrobble = false\n");
        let store = with_old_entry(store_change);
        let new = server("https://new.example", "new-pw");
        save(
            &path,
            None,
            SaveNavidrome::Server(new.clone()),
            store_change,
            Some(OLD),
        )
        .unwrap();
        assert_eq!(store.get(OLD).unwrap_err(), SecretError::Missing);
        let account = secret_store::account(&new.url, &new.username);
        assert_eq!(store.get(&account).unwrap().expose(), "new-pw");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("new.example"), "{text}");
        assert!(text.contains("scrobble = false"), "kept: {text}");

        // A failed write keeps the old entry, which the old config still
        // uses, and removes the new one.
        let store = with_old_entry(store_change_fails);
        let error = save(
            &blocked_path(dir.path()),
            None,
            SaveNavidrome::Server(new),
            store_change_fails,
            Some(OLD),
        )
        .unwrap_err();
        assert!(matches!(error, SaveError::Config(_)), "{error:?}");
        assert_eq!(store.get(OLD).unwrap().expose(), "old-pw");
        assert_eq!(store.get(&account).unwrap_err(), SecretError::Missing);
    }

    #[test]
    fn an_unchanged_account_is_overwritten_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = old_config(dir.path(), "");
        let store = with_old_entry(store_same);
        // Same user and host; a different path does not change the account.
        let same = server("https://old.example/nd", "new-pw");
        save(
            &path,
            None,
            SaveNavidrome::Server(same.clone()),
            store_same,
            Some(OLD),
        )
        .unwrap();
        assert_eq!(store.get(OLD).unwrap().expose(), "new-pw");

        // A failed write never deletes the entry the old config uses.
        let store = with_old_entry(store_same_fails);
        save(
            &blocked_path(dir.path()),
            None,
            SaveNavidrome::Server(same),
            store_same_fails,
            Some(OLD),
        )
        .unwrap_err();
        assert_eq!(store.get(OLD).unwrap().expose(), "new-pw");
    }

    #[test]
    fn removing_navidrome_deletes_the_entry_and_keeps_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = old_config(dir.path(), "scrobble = false\n");
        let store = with_old_entry(store_removed);
        save(
            &path,
            Some("/m".into()),
            SaveNavidrome::Remove,
            store_removed,
            Some(OLD),
        )
        .unwrap();
        assert_eq!(store.get(OLD).unwrap_err(), SecretError::Missing);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("url"), "{text}");
        assert!(text.contains("music_dir = \"/m\""), "{text}");
        assert!(text.contains("[navidrome]\nscrobble = false\n"), "{text}");
    }

    #[test]
    fn a_failed_delete_of_the_old_entry_does_not_fail_the_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = old_config(dir.path(), "");
        let store = with_old_entry(store_delete_denied);
        store.fail_next(OLD, SecretError::Denied);
        save(
            &path,
            Some("/m".into()),
            SaveNavidrome::Remove,
            store_delete_denied,
            Some(OLD),
        )
        .unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("/m"));
        // Best effort: the entry is left over.
        assert_eq!(store.get(OLD).unwrap().expose(), "old-pw");
    }

    #[tokio::test]
    async fn a_password_command_config_moves_to_the_keychain_with_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let old = "[navidrome]\nurl = \"http://localhost:4533\"\nusername = \"me\"\n\
                   password_command = [\"pass\", \"show\", \"nd\"]\nscrobble = false\n";
        std::fs::write(&path, old).unwrap();
        save(
            &path,
            None,
            SaveNavidrome::Server(server("http://localhost:4533", "s3cret")),
            store_command,
            // Nothing in the keychain belonged to the old config.
            None,
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("password_keychain = true"), "{text}");
        assert!(!text.contains("password_command"), "{text}");
        assert!(text.contains("scrobble = false"), "{text}");
        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().contains(".bak."))
            .collect();
        assert_eq!(backups.len(), 1, "{backups:?}");
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), old);
        let env = Env {
            config_path: Some(path.into()),
            ..Env::default()
        };
        let nd = Config::load(Cli::default(), &env)
            .unwrap()
            .navidrome
            .unwrap();
        assert!(!nd.scrobble);
        let password = nd
            .password
            .resolve_with(Duration::from_secs(1), store_command)
            .await
            .unwrap();
        assert_eq!(password.expose(), "s3cret");
    }

    #[tokio::test]
    async fn keeping_navidrome_under_env_overrides_changes_only_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("new-music");
        std::fs::create_dir(&music).unwrap();
        let path = dir.path().join("config.toml");
        let old = "music_dir = \"/old\"\n[navidrome]\nurl = \"https://h.example\"\n\
                   username = \"me\"\npassword_command = [\"printf\", \"%s\", \"cmd \\\"pw\\\"\"]\n\
                   scrobble = false\n";
        std::fs::write(&path, old).unwrap();
        let store = with_old_entry(store_keep);
        save(
            &path,
            Some(music.to_str().unwrap().into()),
            SaveNavidrome::Keep,
            store_keep,
            Some(OLD),
        )
        .unwrap();
        assert_eq!(
            store.get(OLD).unwrap().expose(),
            "old-pw",
            "kept: not deleted"
        );
        // Startup with the override that blocked saving a server.
        let env = Env {
            config_path: Some(path.clone().into()),
            navidrome_user: Some("env-user".into()),
            ..Env::default()
        };
        let config = Config::load(Cli::default(), &env).unwrap();
        assert_eq!(
            config.music_dir,
            Some(std::fs::canonicalize(&music).unwrap())
        );
        let nd = config.navidrome.unwrap();
        assert_eq!(nd.username, "env-user");
        assert_eq!(nd.url.as_str(), "https://h.example/");
        assert!(!nd.scrobble);
        let password = nd
            .password
            .resolve_with(Duration::from_secs(5), unavailable)
            .await
            .unwrap();
        assert_eq!(
            password.expose(),
            "cmd \"pw\"",
            "the command was kept exactly"
        );
    }

    #[test]
    fn keeping_navidrome_from_an_unreadable_file_fails_instead_of_dropping_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[navidrome\n").unwrap();
        let error = save(
            &path,
            Some("/m".into()),
            SaveNavidrome::Keep,
            unavailable,
            None,
        )
        .unwrap_err();
        assert!(matches!(error, SaveError::Config(_)), "{error:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[navidrome\n");
    }

    #[test]
    fn a_save_can_drop_the_folder_and_keep_navidrome() {
        let dir = tempfile::tempdir().unwrap();
        // A new server without a folder.
        let path = old_config(dir.path(), "");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("music_dir = \"/m\"\n{text}")).unwrap();
        let server = server("https://old.example", "pw");
        save(
            &path,
            None,
            SaveNavidrome::Server(server),
            store_drop_folder,
            Some(OLD),
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("music_dir"), "{text}");
        assert!(text.contains("url = \"https://old.example/\""), "{text}");
        // Kept as it is, without a folder.
        std::fs::write(&path, format!("music_dir = \"/m\"\n{text}")).unwrap();
        save(
            &path,
            None,
            SaveNavidrome::Keep,
            store_drop_folder,
            Some(OLD),
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("music_dir"), "{text}");
        assert!(text.contains("password_keychain = true"), "{text}");
        let env = Env {
            config_path: Some(path.into()),
            ..Env::default()
        };
        let config = Config::load(Cli::default(), &env).unwrap();
        assert!(config.music_dir.is_none() && config.navidrome.is_some());
    }

    /// The path `main` takes: the file's keychain account goes through
    /// `read_for_setup` and `RedoContext::from_start` to the save, which
    /// deletes it after writing the new account.
    #[tokio::test]
    async fn the_previous_account_flows_from_the_file_to_the_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = old_config(dir.path(), "");
        let env = Env {
            config_path: Some(path.clone().into()),
            ..Env::default()
        };
        let start = config::read_for_setup(&env).unwrap();
        let redo = RedoContext::from_start(&start, None, &env);
        assert_eq!(redo.previous_account.as_deref(), Some(OLD));
        let mut context = Context::redo(start.config_path, env, redo);
        context.open_store = store_wired;
        let store = with_old_entry(store_wired);

        let (tx, mut rx) = mpsc::unbounded_channel();
        let new = server("https://new.example", "new-pw");
        let account = secret_store::account(&new.url, &new.username);
        spawn_request(
            Request::Save {
                id: 1,
                music_dir: None,
                server: Some(new),
                keep_navidrome: false,
            },
            &context,
            &tx,
        );
        assert_eq!(
            rx.recv().await,
            Some(Response::Saved {
                id: 1,
                result: Ok(())
            })
        );
        assert_eq!(store.get(OLD).unwrap_err(), SecretError::Missing);
        assert_eq!(store.get(&account).unwrap().expose(), "new-pw");
    }

    #[tokio::test]
    async fn a_signal_while_saving_waits_for_the_save() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut setup = Onboarding::new(None);
        for input in [Input::Enter, Input::Esc, Input::Char('y')] {
            setup.handle_input(input);
        }
        for input in [
            Input::Paste("h".into()),
            Input::Tab,
            Input::Paste("u".into()),
        ] {
            setup.handle_input(input);
        }
        setup.handle_input(Input::Tab);
        setup.handle_input(Input::Paste("p".into()));
        setup.handle_input(Input::Enter);
        let Some(Request::Ping { id, .. }) = setup.take_requests().pop() else {
            panic!()
        };
        setup.handle_response(Response::Pinged { id, result: Ok(()) });
        setup.handle_input(Input::Enter);
        let Some(Request::Save { id, .. }) = setup.take_requests().pop() else {
            panic!()
        };
        let answered = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = tx.send(Response::Saved { id, result: Ok(()) });
            tx
        });
        let started = std::time::Instant::now();
        let outcome = leave_on_signal("SIGTERM", &mut setup, &mut rx, SAVE_WAIT).await;
        assert_eq!(outcome, Outcome::Exit);
        assert!(started.elapsed() >= Duration::from_millis(150), "it waited");
        assert_eq!(
            setup.outcome(),
            Some(Outcome::Saved),
            "the save was taken in"
        );
        let _tx = answered.await.unwrap();
    }

    #[tokio::test]
    async fn a_signal_waits_for_a_stuck_save_only_so_long() {
        let (_tx, mut rx) = mpsc::unbounded_channel();
        let mut setup = Onboarding::new(Some("/m"));
        setup.handle_input(Input::Enter);
        setup.handle_input(Input::Enter);
        let Some(Request::CheckFolder { id, .. }) = setup.take_requests().pop() else {
            panic!()
        };
        setup.handle_response(Response::FolderChecked { id, result: Ok(()) });
        setup.handle_input(Input::Char('n'));
        setup.handle_input(Input::Enter);
        assert_eq!(setup.busy(), Some(Busy::Saving));
        let started = std::time::Instant::now();
        let wait = Duration::from_millis(200);
        assert_eq!(
            leave_on_signal("SIGHUP", &mut setup, &mut rx, wait).await,
            Outcome::Exit
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed >= wait && elapsed < Duration::from_secs(3),
            "{elapsed:?}"
        );
    }

    #[tokio::test]
    async fn requests_always_answer_even_if_the_task_dies() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let reply = Reply::new(
            &tx,
            Response::Pinged {
                id: 7,
                result: Err(INTERRUPTED.into()),
            },
        );
        let task = tokio::spawn(async move {
            let _reply = reply;
            std::future::pending::<()>().await;
        });
        task.abort();
        let _ = task.await;
        assert_eq!(
            rx.recv().await,
            Some(Response::Pinged {
                id: 7,
                result: Err(INTERRUPTED.into())
            })
        );
    }
}
