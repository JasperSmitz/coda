//! The file log: `init_logging` sets up a `tracing` subscriber that
//! writes to `coda.log` in the state directory (never to the terminal),
//! with the HTTP/TLS and D-Bus crates clamped to `warn` (ADR 0003).

use tracing::{Level, Subscriber, level_filters::LevelFilter};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, filter, filter::FilterExt};

/// Logs to a file only, truncated at each launch. Level from `CODA_LOG`
/// (EnvFilter syntax), default `info`. If the file cannot be opened,
/// continue without logging rather than writing to the terminal.
///
/// Never add a `log` → `tracing` bridge (`tracing-log`) without clamping
/// the keyring crates first: `keyring-core` and its stores log the
/// keychain account (username and host) at debug through `log`, which
/// today goes nowhere.
pub fn init_logging() {
    let Some(path) =
        crate::config::log_file_path(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"))
    else {
        return;
    };
    init_logging_at(&path);
}

/// [`init_logging`] to the file at `path` (its folder is created).
pub fn init_logging_at(path: &std::path::Path) {
    let file = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| open_log_file(path));
    let Ok(file) = file else {
        return;
    };
    let spec = std::env::var("CODA_LOG").unwrap_or_default();
    let _ = tracing_subscriber::registry()
        .with(log_layer(&spec, file))
        .try_init();
}

/// Creates (or truncates) the log file, readable by the owner only: even
/// without library content the log names the server's behaviour.
/// The mode applies when the file is created; an older, wider file keeps its mode.
fn open_log_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
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
    #[test]
    fn the_log_file_is_created_private_and_truncated() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coda.log");
        drop(super::open_log_file(&path).unwrap());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::write(&path, "old").unwrap();
        drop(super::open_log_file(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"");
    }
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
            tracing::info!(target: "coda_core::app", "app info");
            tracing::debug!(target: "coda_core::app", "app debug");
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
        assert!(!is_clamped("coda_core"));
    }
}
