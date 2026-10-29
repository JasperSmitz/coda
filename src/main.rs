//! `coda` binary. Startup order: parse and validate the config, init
//! logging, obtain the Navidrome password (possibly running
//! `password_command`, which may prompt) and build the client, spawn mpv
//! and connect, then hand over to [`app::run`], which enters the TUI,
//! starts the scan and pings the server. Errors before the TUI are printed
//! to stderr; the TUI never writes to stdout/stderr.

use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use coda_player::app;
use coda_player::config::{self, Cli, Config, Env, PASSWORD_COMMAND_TIMEOUT};
use coda_player::navidrome;
use coda_player::playback;
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
    let spec = std::env::var("CODA_LOG").unwrap_or_default();
    let _ = tracing_subscriber::registry()
        .with(log_layer(&spec, file))
        .try_init();
}

/// HTTP/TLS crates that log server hostnames and addresses below `warn`
/// (e.g. hyper-util's `connecting to <addr>`). ADR 0003: the log never
/// records them, whatever `CODA_LOG` says.
const CLAMPED_TARGETS: [&str; 5] = ["hyper_util", "hyper", "h2", "reqwest", "rustls"];

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
        assert!(!is_clamped("hyperion"));
        assert!(!is_clamped("h2o::x"));
        assert!(!is_clamped("coda_player"));
    }
}
