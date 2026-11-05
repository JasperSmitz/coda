//! Coda's desktop app: the Tauri backend. One engine actor ([`engine`])
//! owns playback and library state; commands ([`commands`]) send it
//! intents, and it emits `snapshot`, `progress` and `visualizer` events
//! ([`dto`]). Covers are served by the `coda` URI scheme ([`cover`]).

pub mod bootstrap;
pub mod commands;
pub mod cover;
pub mod dto;
pub mod engine;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, Manager, RunEvent};

use crate::bootstrap::Paths;
use crate::cover::CoverService;
use crate::dto::{ProgressDto, Snapshot, VisualizerDto};
use crate::engine::{EngineHandle, Sink};

/// Emits the engine's output as Tauri events to the webview.
struct TauriSink(AppHandle);

impl Sink for TauriSink {
    fn snapshot(&self, snapshot: &Snapshot) {
        if let Err(error) = self.0.emit("snapshot", snapshot) {
            tracing::warn!(%error, "could not emit a snapshot");
        }
    }

    fn progress(&self, progress: &ProgressDto) {
        let _ = self.0.emit("progress", progress);
    }

    fn visualizer(&self, frame: &VisualizerDto) {
        let _ = self.0.emit("visualizer", frame);
    }
}

/// Upper bound for the shutdown run from `RunEvent::Exit` (startup grace,
/// final save and mpv shutdown are each bounded inside the engine).
const EXIT_SHUTDOWN_BOUND: std::time::Duration = std::time::Duration::from_secs(8);

/// Quit state: the first exit request runs the engine's shutdown (final
/// session save, then mpv) and exits when it is done.
#[derive(Default)]
struct Quit {
    started: AtomicBool,
    done: AtomicBool,
}

/// Starts the quit sequence once: the engine's shutdown (final session
/// save, then mpv), then `exit`. Later calls do nothing.
fn begin_quit(app: &AppHandle) {
    if app.state::<Quit>().started.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let engine = app.state::<EngineHandle>().inner().clone();
    tauri::async_runtime::spawn(async move {
        engine.shutdown().await;
        tracing::info!("coda-app quitting");
        app.state::<Quit>().done.store(true, Ordering::SeqCst);
        app.exit(0);
    });
}

/// SIGTERM (logout, `kill`), SIGINT (Ctrl-C in `tauri dev`) and SIGHUP
/// quit through [`begin_quit`], like closing the window, instead of
/// killing the process with mpv still playing. A second signal while
/// quitting stops at once (terminating mpv first).
fn watch_signals(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
            signal(SignalKind::hangup()),
        ) else {
            tracing::warn!("could not install signal handlers");
            return;
        };
        loop {
            let (name, number) = tokio::select! {
                _ = term.recv() => ("TERM", libc::SIGTERM),
                _ = int.recv() => ("INT", libc::SIGINT),
                _ = hup.recv() => ("HUP", libc::SIGHUP),
            };
            if app.state::<Quit>().started.load(Ordering::SeqCst) {
                tracing::warn!(signal = name, "second signal while quitting; exiting now");
                bootstrap::kill_recorded_mpv();
                std::process::exit(128 + number);
            }
            tracing::info!(signal = name, "quitting on signal");
            begin_quit(&app);
        }
    });
}

static MAIN_THREAD: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();

/// A panic that ends the process (every panic in a `panic = "abort"`
/// release build; one on the main thread otherwise) skips the shutdown, so
/// mpv is terminated from the hook first. Panics a task catches (unwind
/// builds) leave mpv alone.
fn install_panic_hook() {
    let _ = MAIN_THREAD.set(std::thread::current().id());
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let on_main = MAIN_THREAD.get() == Some(&std::thread::current().id());
        if cfg!(panic = "abort") || on_main {
            bootstrap::kill_recorded_mpv();
        }
        previous(info);
    }));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let paths = Paths::from_process();
    bootstrap::init_logging(&paths);
    install_panic_hook();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "coda-app starting");
    let covers = Arc::new(CoverService::new(paths.cover_cache.clone()));
    let protocol_covers = covers.clone();

    let app = tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol(
            cover::SCHEME,
            move |_ctx, request, responder| {
                let covers = protocol_covers.clone();
                tauri::async_runtime::spawn(async move {
                    responder.respond(covers.respond(request).await);
                });
            },
        )
        .setup(move |app| {
            let sink = TauriSink(app.handle().clone());
            let startup_paths = paths.clone();
            let engine = engine::spawn(sink, covers, async move {
                bootstrap::start(&startup_paths).await
            });
            app.manage(engine);
            app.manage(Quit::default());
            watch_signals(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::intent,
            commands::snapshot
        ])
        .build(tauri::generate_context!())
        .expect("error while building the Coda app");

    app.run(|app, event| match event {
        RunEvent::ExitRequested { api, .. } => {
            if !app.state::<Quit>().done.load(Ordering::SeqCst) {
                api.prevent_exit();
                begin_quit(app);
            }
        }
        // A quit path that skipped `ExitRequested` (or raced it): shut down
        // here, blocking the event loop, so mpv never outlives the app. The
        // engine runs on the async runtime's threads, not this one.
        RunEvent::Exit => {
            let quit = app.state::<Quit>();
            if !quit.done.load(Ordering::SeqCst) {
                tracing::warn!("exiting without the normal shutdown; shutting down now");
                let engine = app.state::<EngineHandle>().inner().clone();
                tauri::async_runtime::block_on(async move {
                    let _ = tokio::time::timeout(EXIT_SHUTDOWN_BOUND, engine.shutdown()).await;
                });
                quit.done.store(true, Ordering::SeqCst);
            }
        }
        _ => {}
    });
}
