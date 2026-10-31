//! `coda` binary. Startup order: parse and validate the config (when no
//! source is configured and stdin/stdout are terminals, or with `--setup`,
//! run the setup and load the config it wrote), init logging, obtain the Navidrome
//! password (possibly running `password_command`, which may prompt, or
//! reading the OS keychain) and build the client, load the
//! saved session, spawn mpv and connect, then hand over to [`app::run`],
//! which restores the session, enters the TUI, starts the scan and pings
//! the server. Errors before the TUI are printed
//! to stderr; the TUI never writes to stdout/stderr.

use std::io::{IsTerminal, Write};
use std::panic::AssertUnwindSafe;
use std::process::ExitCode;
use std::time::Duration;

use coda_player::app;
use coda_player::config::{self, Cli, Config, ConfigError, Env, PASSWORD_COMMAND_TIMEOUT};
use coda_player::navidrome;
use coda_player::onboarding::Outcome;
use coda_player::playback;
use coda_player::session;
use coda_player::settings;
use coda_player::setup;
use tracing::{Level, Subscriber, level_filters::LevelFilter};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, filter, filter::FilterExt};

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
    let navidrome = match config.navidrome {
        Some(settings) => {
            let password = match settings.password.resolve(PASSWORD_COMMAND_TIMEOUT).await {
                Ok(password) => password,
                Err(error) => {
                    tracing::error!(%error, "could not obtain the Navidrome password");
                    let _ = writeln!(std::io::stderr(), "coda: {error}");
                    return ExitCode::from(error.exit_code());
                }
            };
            match navidrome::Client::new(settings.url, settings.username, password) {
                Ok(client) => Some(client),
                Err(error) => {
                    tracing::error!(%error, "could not set up the Navidrome client");
                    eprintln!("coda: Navidrome: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
        None => None,
    };
    tracing::info!(
        local = config.music_dir.is_some(),
        navidrome = navidrome.is_some(),
        scrobble,
        "sources configured"
    );

    let restored = session::restore(config::session_file_path(
        std::env::var_os("XDG_STATE_HOME"),
        std::env::var_os("HOME"),
    ));

    let handle = match playback::spawn_backend().await {
        Ok(handle) => handle,
        Err(error) => {
            tracing::error!(%error, "could not start playback backend");
            eprintln!("coda: {error}");
            return ExitCode::FAILURE;
        }
    };

    match app::run(
        handle,
        config.music_dir,
        navidrome,
        scrobble,
        restored,
        settings,
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

/// Logs to a file only, truncated at each launch. Level from `CODA_LOG`
/// (EnvFilter syntax), default `info`. If the file cannot be opened,
/// continue without logging rather than writing to the terminal.
///
/// Never add a `log` → `tracing` bridge (`tracing-log`) without clamping
/// the keyring crates first: `keyring-core` and its stores log the
/// keychain account (username and host) at debug through `log`, which
/// today goes nowhere.
fn init_logging() {
    let Some(path) =
        config::log_file_path(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"))
    else {
        return;
    };
    let file = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::File::create(&path));
    let Ok(file) = file else {
        return;
    };
    let spec = std::env::var("CODA_LOG").unwrap_or_default();
    let _ = tracing_subscriber::registry()
        .with(log_layer(&spec, file))
        .try_init();
}

/// HTTP/TLS crates that log server hostnames and addresses below `warn`
/// (e.g. hyper-util's `connecting to <addr>`), and zbus, whose D-Bus
/// messages to the Secret Service carry the keychain account (username and
/// host). ADR 0003: the log never records them, whatever `CODA_LOG` says.
const CLAMPED_TARGETS: [&str; 6] = ["hyper_util", "hyper", "h2", "reqwest", "rustls", "zbus"];

fn is_clamped(target: &str) -> bool {
    CLAMPED_TARGETS.iter().any(|crate_name| {
        target
            .strip_prefix(crate_name)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
    })
}

/// The `CODA_LOG` filter (default `info`) with the HTTP/TLS crates
/// clamped to `warn`. The clamp directives come last, so they replace a
/// user directive for the same crate.
fn log_filter(spec: &str) -> EnvFilter {
    let mut filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .parse_lossy(spec);
    for crate_name in CLAMPED_TARGETS {
        if let Ok(directive) = format!("{crate_name}=warn").parse() {
            filter = filter.add_directive(directive);
        }
    }
    filter
}

/// The file log layer. A more specific user directive (e.g.
/// `hyper_util::client=trace`) would outrank the clamp directives inside
/// the `EnvFilter`, so a second filter enforces the clamp by target.
fn log_layer<S, W>(spec: &str, writer: W) -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let clamp =
        filter::filter_fn(|meta| !is_clamped(meta.target()) || *meta.level() <= Level::WARN);
    tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_filter(log_filter(spec).and(clamp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn logged(spec: &str) -> String {
        let capture = Capture::default();
        let writer = capture.clone();
        let subscriber =
            tracing_subscriber::registry().with(log_layer(spec, move || writer.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!(target: "hyper_util::client::legacy::connect::http", "connecting to secret-host:443");
            tracing::trace!(target: "hyper_util", "trace secret-host");
            tracing::info!(target: "reqwest::connect", "info secret-host");
            tracing::debug!(target: "rustls::client", "tls secret-host");
            tracing::warn!(target: "hyper_util::client", "warn kept");
            tracing::info!(target: "coda_player::app", "app info");
            tracing::debug!(target: "coda_player::app", "app debug");
        });
        String::from_utf8(capture.0.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn http_crates_are_clamped_to_warn_whatever_coda_log_says() {
        for spec in [
            "",
            "trace",
            "hyper_util=trace",
            "hyper_util::client::legacy=trace,reqwest=debug,rustls=trace",
            "debug,hyper_util=trace",
        ] {
            let log = logged(spec);
            assert!(!log.contains("secret-host"), "{spec:?}: {log}");
            assert!(log.contains("warn kept"), "{spec:?}: {log}");
        }
        // EnvFilter semantics otherwise: the default is `info`, and a
        // target-only spec enables only that target.
        assert!(logged("").contains("app info"));
        assert!(logged("debug,hyper_util=trace").contains("app info"));
        assert!(!logged("hyper_util=trace").contains("app info"));
        // The user's level still applies elsewhere.
        assert!(!logged("").contains("app debug"));
        let debug = logged("debug");
        assert!(debug.contains("app debug"), "{debug}");
    }

    #[test]
    fn clamped_targets_match_whole_crate_names() {
        assert!(is_clamped("hyper"));
        assert!(is_clamped("hyper::proto"));
        assert!(is_clamped("hyper_util::client"));
        assert!(is_clamped("h2"));
        assert!(is_clamped("zbus::connection"));
        assert!(!is_clamped("hyperion"));
        assert!(!is_clamped("h2o::x"));
        assert!(!is_clamped("coda_player"));
    }
}
