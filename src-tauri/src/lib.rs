//! Coda's desktop app: the Tauri backend. One engine actor ([`engine`])
//! owns playback and library state; commands ([`commands`]) send it
//! intents, and it emits `snapshot`, `progress` and `visualizer` events
//! ([`dto`]). Covers are served by the `coda` URI scheme ([`cover`]).
//! [`settings`] writes Coda's config and applies it live; [`media`] is
//! the OS media controls; [`theme`] the colour themes (applied before
//! the window's first paint by an initialization script).

pub mod bootstrap;
pub mod browse;
pub mod commands;
pub mod cover;
pub mod dto;
pub mod engine;
pub mod media;
pub mod playlists;
pub mod settings;
pub mod theme;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use tauri::{AppHandle, Emitter, Manager, RunEvent};

use crate::bootstrap::Paths;
use crate::browse::BrowseHandle;
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

/// Starts the quit sequence once: a running settings save, then the
/// engine's shutdown (final session save, then mpv), then `exit`. Later calls do nothing.
fn begin_quit(app: &AppHandle) {
    if app.state::<Quit>().started.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let engine = app.state::<EngineHandle>().inner().clone();
    tauri::async_runtime::spawn(async move {
        // A settings save in progress finishes first (keychain, then
        // config), bounded.
        app.state::<settings::SettingsLock>().finish().await;
        // A running playlist edit answers first (bounded), like the TUI.
        app.state::<playlists::PlaylistEditor>()
            .finish(playlists::EDIT_GRACE)
            .await;
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

/// Tells the webview when the window is minimised or restored (the
/// Now Playing view stops visualizer readings then): WebKitGTK does not
/// reliably turn `document.visibilityState` hidden on minimise.
fn report_minimized(window: &tauri::Window, event: &tauri::WindowEvent) {
    static MINIMIZED: AtomicBool = AtomicBool::new(false);
    if !matches!(
        event,
        tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Focused(_)
    ) {
        return;
    }
    let minimized = window.is_minimized().unwrap_or(false);
    if MINIMIZED.swap(minimized, Ordering::SeqCst) != minimized {
        let _ = window.emit("window-minimized", minimized);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let paths = Paths::from_process();
    bootstrap::init_logging(&paths);
    install_panic_hook();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "coda-app starting");
    let covers = Arc::new(CoverService::new(paths.cover_cache.clone()));
    let protocol_covers = covers.clone();
    let browse = BrowseHandle::new(covers.clone());

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
            // The main window is created here (not from the config) so the
            // chosen theme's colours are set before its first paint.
            let appearance = theme::Appearance::from_process();
            let window_config = app
                .config()
                .app
                .windows
                .iter()
                .find(|w| w.label == "main")
                .cloned()
                .ok_or("no main window in tauri.conf.json")?;
            tauri::WebviewWindowBuilder::from_config(app.handle(), &window_config)?
                .initialization_script(appearance.init_script())
                .build()?;
            app.manage(appearance);
            let media = Arc::new(OnceLock::new());
            let sink = TauriSink(app.handle().clone(), media.clone());
            let startup_paths = paths.clone();
            let engine = engine::spawn(sink, covers.clone(), browse.clone(), async move {
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
            app.manage(browse);
            app.manage(settings::SettingsLock::default());
            app.manage(playlists::PlaylistEditor::default());
            app.manage(Quit::default());
            watch_signals(app.handle().clone());
            Ok(())
        })
        .on_window_event(report_minimized)
        .invoke_handler(tauri::generate_handler![
            commands::intent,
            commands::snapshot,
            browse::commands::local_artists,
            browse::commands::local_albums,
            browse::commands::local_artist,
            browse::commands::local_album,
            browse::commands::local_folder,
            browse::commands::local_playlists,
            browse::commands::local_playlist,
            browse::commands::local_search,
            browse::commands::navidrome_artists,
            browse::commands::navidrome_artist,
            browse::commands::navidrome_artist_tracks,
            browse::commands::navidrome_album,
            browse::commands::navidrome_albums,
            browse::commands::navidrome_playlists,
            browse::commands::navidrome_playlist,
            browse::commands::navidrome_random_songs,
            browse::commands::navidrome_search,
            playlists::commands::playlist_create,
            playlists::commands::playlist_save_queue,
            playlists::commands::playlist_add,
            playlists::commands::playlist_rename,
            playlists::commands::playlist_delete,
            playlists::commands::playlist_remove,
            playlists::commands::playlist_reorder,
            settings::settings_get,
            settings::settings_test_server,
            settings::settings_save_server,
            settings::settings_disconnect,
            settings::settings_pick_folder,
            settings::settings_set_music_dir,
            settings::settings_set_options,
            settings::onboarding_finish,
            theme::commands::appearance_get,
            theme::commands::appearance_set,
            theme::commands::theme_list,
            theme::commands::theme_resolve,
            theme::commands::theme_save_custom,
            theme::commands::theme_rename,
            theme::commands::theme_duplicate,
            theme::commands::theme_delete,
            theme::commands::theme_reveal_folder
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
