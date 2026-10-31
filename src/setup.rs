//! Runs the first-run setup ([`Onboarding`]) in its own short TUI session,
//! before mpv is started, and performs its requests: folder checks and
//! keychain/config writes on blocking threads, the ping on a Tokio task.
//! Nothing here blocks the input loop.
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
use crate::config::{self, ConfigError, Env};
use crate::config_writer::{self, SetupConfig, SetupNavidrome};
use crate::navidrome;
use crate::onboarding::{Busy, Input, Onboarding, Outcome, Request, Response, SaveError, Server};
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
}

impl Context {
    /// The real config path and credential store.
    pub fn new(config_path: PathBuf, env: Env) -> Self {
        Self {
            config_path,
            env,
            open_store: SecretStore::system,
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

    tracing::info!("first-run setup started");
    let mut setup = Onboarding::new(context.music_prefill());
    let result = event_loop(&mut terminal, &mut setup, &context, &mut signals).await;
    match &result {
        Ok(outcome) => tracing::info!(?outcome, "first-run setup finished"),
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
            tokio::task::spawn_blocking(move || {
                let result = save(&path, music_dir, server, open_store);
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

/// Stores the password, then writes the config. Blocking.
/// If the config write fails after the password was stored, the entry is
/// removed again (best effort), so a failed setup leaves nothing behind.
fn save(
    path: &Path,
    music_dir: Option<String>,
    server: Option<Server>,
    open_store: fn() -> Result<SecretStore, SecretError>,
) -> Result<(), SaveError> {
    let mut stored = None;
    let navidrome = match server {
        Some(server) => {
            let account = secret_store::account(&server.url, &server.username);
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
            Some(SetupNavidrome {
                url: server.url,
                username: server.username,
            })
        }
        None => None,
    };
    let config = SetupConfig {
        music_dir,
        navidrome,
    };
    match config_writer::write(path, &config) {
        Ok(backup) => {
            tracing::info!(backup = backup.is_some(), "setup: config written");
            Ok(())
        }
        Err(error) => {
            tracing::warn!(error = %error.kind(), "setup: could not write the config");
            if let Some((store, account)) = stored {
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
            Some(server("http://localhost:4533", "s3cret")),
            store_a,
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("s3cret"), "{text}");
        let env = Env {
            config_path: Some(path.clone().into()),
            ..Env::default()
        };
        let nd = Config::load(Cli { music_dir: None }, &env)
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
            Some(server("https://h", "p")),
            unavailable,
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
        let error = save(&path, Some("/m".into()), None, unavailable).unwrap_err();
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
        let error = save(&blocker.join("config.toml"), None, Some(server), store_b).unwrap_err();
        assert!(matches!(error, SaveError::Config(_)), "{error:?}");
        assert_eq!(
            store_b().unwrap().get(&account).unwrap_err(),
            SecretError::Missing,
            "no orphaned entry"
        );
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
