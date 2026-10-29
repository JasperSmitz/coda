//! `coda` binary. Startup order: parse and validate the config, init
//! logging, obtain the Navidrome password (possibly running
//! `password_command`, which may prompt) and build the client, spawn mpv
//! and connect, then hand over to [`app::run`], which enters the TUI,
//! starts the scan and pings the server. Errors before the TUI are printed
//! to stderr; the TUI never writes to stdout/stderr.

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use coda_player::app;
use coda_player::config::{self, Cli, Config, Env, PASSWORD_COMMAND_TIMEOUT};
use coda_player::navidrome;
use coda_player::playback;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = match Config::load(cli, &Env::from_process()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("coda: {error}");
            return ExitCode::from(error.exit_code());
        }
    };
    init_logging();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "coda starting");

    let navidrome = match config.navidrome {
        Some(settings) => {
            let password = match settings.password.resolve(PASSWORD_COMMAND_TIMEOUT).await {
                Ok(password) => password,
                Err(error) => {
                    tracing::error!(%error, "could not obtain the Navidrome password");
                    eprintln!("coda: {error}");
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
        "sources configured"
    );

    let handle = match playback::spawn_backend().await {
        Ok(handle) => handle,
        Err(error) => {
            tracing::error!(%error, "could not start playback backend");
            eprintln!("coda: {error}");
            return ExitCode::FAILURE;
        }
    };

    match app::run(handle, config.music_dir, navidrome).await {
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

/// Logs to a file only, truncated at each launch. Level from `CODA_LOG`
/// (EnvFilter syntax), default `info`. If the file cannot be opened,
/// continue without logging rather than writing to the terminal.
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
    let filter = tracing_subscriber::EnvFilter::builder()
        .with_env_var("CODA_LOG")
        .with_default_directive(tracing::level_filters::LevelFilter::INFO.into())
        .from_env_lossy();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(file)
        .with_ansi(false)
        .try_init();
}
