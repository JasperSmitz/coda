//! `coda` binary. Startup order: parse and validate the config (when no
//! source is configured and stdin/stdout are terminals, or with `--setup`,
//! run the setup and load the config it wrote), init logging, obtain the Navidrome
//! password (possibly running `password_command`, which may prompt, or
//! reading the OS keychain) and build the client, load the
//! saved session, spawn mpv and connect, then hand over to [`app::run`],
//! which restores the session, enters the TUI (asking the terminal about
//! images when cover art is on), starts the scan and pings
//! the server. Errors before the TUI are printed
//! to stderr; the TUI never writes to stdout/stderr.

use std::io::{IsTerminal, Write};
use std::panic::AssertUnwindSafe;
use std::process::ExitCode;
use std::time::Duration;

use coda_core::config::{self, Cli, Config, ConfigError, Env};
use coda_core::logging::init_logging;
use coda_core::onboarding::Outcome;
use coda_core::playback;
use coda_core::session;
use coda_core::settings;
use coda_tui::app;
use coda_tui::setup;

/// Upper bound on waiting for leftover background tasks at exit, e.g. a
/// fetch stuck in a blocking DNS lookup, which would otherwise keep the
/// process alive after the TUI has gone.
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("coda: could not start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    // A panic is caught only to bound the runtime shutdown below, then
    // resumed, so the exit status stays that of a panic (101).
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| runtime.block_on(run())));
    // Cleanup (mpv shutdown, terminal restore) is done; do not wait
    // indefinitely for tasks that cannot be cancelled.
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
    match result {
        Ok(code) => code,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

async fn run() -> ExitCode {
    let (cli, music_arg) = match Cli::try_parse_with_source(std::env::args_os()) {
        Ok(parsed) => parsed,
        Err(error) => error.exit(),
    };
    let env = Env::from_process();
    let mut logging = false;
    // Kept for loading the config again after setup.
    let music_dir_arg = cli.music_dir.clone();
    let config = if cli.setup {
        // `coda --setup`: run the setup whatever is configured. A folder
        // given to this run is checked first, as on a normal start.
        if let Some(dir) = music_dir_arg.as_ref().filter(|d| !d.is_empty())
            && let Err(error) = config::validate_music_dir(dir.into())
        {
            eprintln!("coda: {error}");
            return ExitCode::from(error.exit_code());
        }
        if !is_interactive() {
            eprintln!("coda: --setup needs a terminal on stdin and stdout");
            return ExitCode::from(2);
        }
        let start = match config::read_for_setup(&env) {
            Ok(start) => start,
            Err(error) => {
                eprintln!("coda: {error}");
                return ExitCode::from(error.exit_code());
            }
        };
        init_logging();
        logging = true;
        tracing::info!(version = env!("CARGO_PKG_VERSION"), "coda starting setup");
        let redo = setup::RedoContext::from_start(&start, music_arg, &env);
        let context = setup::Context::redo(start.config_path, env.clone(), redo);
        match run_setup(context, music_dir_arg, &env).await {
            Ok(config) => config,
            Err(code) => return code,
        }
    } else {
        match Config::load(cli, &env) {
            Ok(config) => config,
            // Setup only fills in a missing source, and only interactively;
            // scripts and pipes keep the usage error.
            Err(ConfigError::NoSource {
                config_path: Some(path),
            }) if is_interactive() => {
                init_logging();
                logging = true;
                tracing::info!(version = env!("CARGO_PKG_VERSION"), "coda starting setup");
                match run_setup(setup::Context::new(path, env.clone()), None, &env).await {
                    Ok(config) => config,
                    Err(code) => return code,
                }
            }
            Err(error) => {
                eprintln!("coda: {error}");
                return ExitCode::from(error.exit_code());
            }
        }
    };
    if !logging {
        init_logging();
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "coda starting");

    let scrobble = config.navidrome.as_ref().is_some_and(|n| n.scrobble);
    // What the in-app settings show and may change: names and ownership
    // only, never a password.
    let music_origin = music_arg.filter(|_| config.music_dir.is_some());
    // A folder from the argument or `CODA_MUSIC_DIR` hides whether the
    // file has its own; read it (it just loaded, values only, nothing is
    // resolved). On any doubt, assume it has none: that only refuses
    // Disconnect.
    let file_has_music_dir = match music_origin {
        None => config.music_dir.is_some(),
        Some(_) => config::read_for_setup(&env).is_ok_and(|start| start.music_dir.is_some()),
    };
    let settings = settings::Context {
        config_path: config::config_file_path(&env).map(|(path, _)| path),
        music_dir: config.music_dir.clone(),
        music_origin,
        file_has_music_dir,
        navidrome: settings::Navidrome::from_config(
            config.navidrome.as_ref(),
            &config::navidrome_env_overrides(&env),
        ),
        env_overrides: config::navidrome_env_overrides(&env),
    };
    // Logs the failure itself; the message is `coda: <error>` as before
    // (`Navidrome: …` for a client error, exit status 1).
    let navidrome = match config::navidrome_client(config.navidrome).await {
        Ok(client) => client,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "coda: {error}");
            return ExitCode::from(error.exit_code());
        }
    };
    tracing::info!(
        local = config.music_dir.is_some(),
        navidrome = navidrome.is_some(),
        scrobble,
        cover_art = config.cover_art,
        "sources configured"
    );

    let restored = session::restore(config::session_file_path(
        std::env::var_os("XDG_STATE_HOME"),
        std::env::var_os("HOME"),
    ));

    // mpv from `PATH` only, as before: core's `spawn_backend` also looks in
    // well-known folders for the desktop app, whose launchers get a
    // minimal `PATH`; a terminal has the user's own.
    let handle = match playback::mpv::spawn(playback::mpv::MpvOptions::default()).await {
        Ok(handle) => handle,
        Err(error) => {
            tracing::error!(%error, "could not start playback backend");
            eprintln!("coda: {error}");
            return ExitCode::FAILURE;
        }
    };

    let covers = app::CoverConfig {
        enabled: config.cover_art,
        cache_dir: coda_core::cover::cache::cache_dir(
            std::env::var_os("XDG_CACHE_HOME"),
            std::env::var_os("HOME"),
        ),
    };
    match app::run(
        handle,
        config.music_dir,
        navidrome,
        scrobble,
        restored,
        settings,
        covers,
    )
    .await
    {
        Ok(()) => {
            tracing::info!("coda exiting");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(%error, "coda exiting after an error");
            // The terminal may be gone (hangup); never panic on a failed write.
            let _ = writeln!(std::io::stderr(), "coda: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Runs setup, then loads the config it wrote (with the same music
/// directory argument as this run). `Err` carries the exit status: success
/// when the user left setup without saving.
async fn run_setup(
    context: setup::Context,
    music_dir: Option<std::ffi::OsString>,
    env: &Env,
) -> Result<Config, ExitCode> {
    match setup::run(context).await {
        Ok(Outcome::Saved) => {}
        Ok(Outcome::Exit) => return Err(ExitCode::SUCCESS),
        Err(error) => {
            // The terminal may be gone (hangup); never panic here.
            let _ = writeln!(std::io::stderr(), "coda: setup failed: {error}");
            return Err(ExitCode::FAILURE);
        }
    }
    // Known gap: setup registered Tokio handlers for SIGTERM, SIGHUP and
    // SIGINT, which stay installed for the process. A signal arriving from
    // here until `app::run` listens again (keychain read, mpv start;
    // normally well under a second) is swallowed rather than ending the
    // process.
    let cli = Cli {
        music_dir,
        setup: false,
    };
    Config::load(cli, env).map_err(|error| {
        tracing::error!("the config written by setup did not load");
        let _ = writeln!(std::io::stderr(), "coda: {error}");
        ExitCode::from(error.exit_code())
    })
}

/// Setup needs a terminal to talk to: both stdin and stdout.
fn is_interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}
