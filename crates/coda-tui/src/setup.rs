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
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{EnableBracketedPaste, Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::app::{self, PanicHook};
use crate::ui::onboarding::{self as view, View};
use coda_core::config::{self, Env, MusicDirArg, SetupStart};
use coda_core::onboarding::{Busy, Input, Onboarding, Outcome, Redo, Request, Response, SaveError};
use coda_core::secret_store::{STORE_NAME, SecretError, SecretStore};
use coda_core::setup_actions::{INTERRUPTED, Reply, SaveNavidrome, check_folder, ping, save};

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

#[cfg(test)]
mod tests {
    use super::*;

    use coda_core::navidrome::Password;
    use coda_core::navidrome::client::parse_base_url;
    use coda_core::onboarding::Server;
    use coda_core::secret_store;
    use std::path::Path;
    use std::sync::OnceLock;

    fn server(url: &str, password: &str) -> Server {
        Server {
            url: parse_base_url(url).unwrap(),
            username: "me".into(),
            password: Password::new(password.into()),
        }
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
        assert_eq!(store.get(&account).unwrap(), Password::new("new-pw".into()));
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
}
