//! Coda's desktop app: the Tauri backend. One engine actor ([`engine`])
//! owns playback and library state; commands ([`commands`]) send it
//! intents, and it emits `snapshot`, `progress` and `visualizer` events
//! ([`dto`]). Covers are served by the `coda` URI scheme ([`cover`]).
//! [`settings`] writes Coda's config and applies it live; [`media`] is
//! the OS media controls.

pub mod bootstrap;
pub mod commands;
pub mod cover;
pub mod dto;
pub mod engine;
pub mod media;
pub mod settings;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use tauri::{AppHandle, Emitter, Manager, RunEvent};

use crate::bootstrap::Paths;
use crate::cover::CoverService;
use crate::dto::{ProgressDto, Snapshot, VisualizerDto};
use crate::engine::{EngineHandle, Sink};

/// Emits the engine's output as Tauri events to the webview, and feeds
/// the OS media controls (set once the engine handle exists).
struct TauriSink(AppHandle, Arc<OnceLock<media::Media>>);

impl Sink for TauriSink {
    fn snapshot(&self, snapshot: &Snapshot) {
        if let Err(error) = self.0.emit("snapshot", snapshot) {
            tracing::warn!(%error, "could not emit a snapshot");
        }
        if let Some(media) = self.1.get() {
            media.snapshot(snapshot);
        }
    }

    fn progress(&self, progress: &ProgressDto) {
        let _ = self.0.emit("progress", progress);
        if let Some(media) = self.1.get() {
            media.progress(progress);
        }
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let paths = Paths::from_process();
    bootstrap::init_logging(&paths);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "coda-app starting");
    let covers = Arc::new(CoverService::new(paths.cover_cache.clone()));
    let protocol_covers = covers.clone();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
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
            let media = Arc::new(OnceLock::new());
            let sink = TauriSink(app.handle().clone(), media.clone());
            let startup_paths = paths.clone();
            let engine = engine::spawn(sink, covers.clone(), async move {
                bootstrap::start(&startup_paths).await
            });
            let art_dir = paths
                .session
                .as_ref()
                .and_then(|s| s.parent())
                .map(|dir| dir.join("media"));
            let _ = media.set(media::Media::start(
                app.handle(),
                engine.clone(),
                covers,
                art_dir,
            ));
            app.manage(engine);
            app.manage(settings::SettingsLock::default());
            app.manage(Quit::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::intent,
            commands::snapshot,
            settings::settings_get,
            settings::settings_test_server,
            settings::settings_save_server,
            settings::settings_disconnect,
            settings::settings_pick_folder,
            settings::settings_set_music_dir,
            settings::settings_set_options,
            settings::onboarding_finish
        ])
        .build(tauri::generate_context!())
        .expect("error while building the Coda app");

    app.run(|app, event| match event {
        RunEvent::ExitRequested { api, .. } => {
            let quit = app.state::<Quit>();
            if quit.done.load(Ordering::SeqCst) {
                return;
            }
            api.prevent_exit();
            if quit.started.swap(true, Ordering::SeqCst) {
                return;
            }
            let app = app.clone();
            let engine = app.state::<EngineHandle>().inner().clone();
            tauri::async_runtime::spawn(async move {
                // A settings save in progress finishes first (keychain,
                // then config).
                app.state::<settings::SettingsLock>().finish().await;
                engine.shutdown().await;
                tracing::info!("coda-app quitting");
                app.state::<Quit>().done.store(true, Ordering::SeqCst);
                app.exit(0);
            });
        }
        // A quit path that skipped `ExitRequested` (or raced it): shut down
        // here, blocking the event loop, so mpv never outlives the app. The
        // engine runs on the async runtime's threads, not this one.
        RunEvent::Exit => {
            media::Media::stop();
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
