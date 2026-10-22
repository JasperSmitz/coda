//! `coda` binary. Slice 1: validate the configuration, check that the
//! playback backend can be started, then exit. The TUI arrives in slice 2.

use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use coda_player::config::{self, Cli, Config};
use coda_player::playback;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = match Config::from_cli(cli) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("coda: {error}");
            return ExitCode::from(error.exit_code());
        }
    };
    init_logging();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "coda starting");

    match playback::spawn_backend().await {
        Ok(handle) => {
            let outcome = handle.shutdown(SHUTDOWN_TIMEOUT).await;
            tracing::info!(?outcome, "playback backend shut down");
        }
        Err(error) => {
            tracing::error!(%error, "could not start playback backend");
            eprintln!("coda: {error}");
            return ExitCode::FAILURE;
        }
    }

    println!(
        "Music directory: {}\nPlayback backend OK. The terminal UI arrives in M1 slice 2.",
        config.music_dir.display()
    );
    ExitCode::SUCCESS
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
