//! Startup: config, logging, the Navidrome client, mpv and the saved
//! session. Nothing here may crash the app: a missing config, password or
//! mpv becomes a [`ProblemDto`] the UI shows as a banner.
//!
//! The GUI shares Coda's `config.toml` (music folder, Navidrome account)
//! and cover cache, but keeps its own session and log under
//! `<Coda's state dir>/coda-app/`, so the TUI and GUI never overwrite
//! each other's queue.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use coda_core::config::{self, Cli, Config, ConfigError, Env};
use coda_core::navidrome::Client;
use coda_core::playback::{self, PlaybackHandle};
use coda_core::session::{self, Restored};

use crate::dto::{ProblemDto, ProblemKind};

/// Where the GUI keeps its own files.
#[derive(Debug, Clone, Default)]
pub struct Paths {
    /// `<state>/coda-app/session.json`; `None` disables saving.
    pub session: Option<PathBuf>,
    /// `<state>/coda-app/history.json` (the play history for Home).
    pub history: Option<PathBuf>,
    /// `<state>/coda-app/coda-app.log`.
    pub log: Option<PathBuf>,
    /// Coda's cover cache folder (shared with the TUI).
    pub cover_cache: Option<PathBuf>,
    /// The TUI's session (`<state>/state.json`), imported once when the
    /// GUI has none yet. Only ever read.
    pub tui_session: Option<PathBuf>,
}

impl Paths {
    pub fn from_process() -> Self {
        Self::new(
            std::env::var_os("XDG_STATE_HOME"),
            std::env::var_os("XDG_CACHE_HOME"),
            std::env::var_os("HOME"),
        )
    }

    pub fn new(
        xdg_state_home: Option<OsString>,
        xdg_cache_home: Option<OsString>,
        home: Option<OsString>,
    ) -> Self {
        let dir =
            config::state_dir(xdg_state_home.clone(), home.clone()).map(|d| d.join("coda-app"));
        Self {
            session: dir.as_ref().map(|d| d.join("session.json")),
            history: dir.as_ref().map(|d| d.join("history.json")),
            log: dir.map(|d| d.join("coda-app.log")),
            tui_session: config::session_file_path(xdg_state_home, home.clone()),
            cover_cache: coda_core::cover::cache::cache_dir(xdg_cache_home, home),
        }
    }
}

/// Sets up the file log (never stdout). Call once, before anything logs.
pub fn init_logging(paths: &Paths) {
    if let Some(path) = &paths.log {
        coda_core::logging::init_logging_at(path);
    }
}

/// The music sources from the config, or the problem that stopped them.
pub struct Sources {
    pub music_dir: Option<PathBuf>,
    pub client: Option<Client>,
    pub scrobble: bool,
    pub cover_art: bool,
    pub problems: Vec<ProblemDto>,
}

/// Everything the engine starts from.
pub struct Startup {
    pub sources: Sources,
    /// mpv, or why it could not start.
    pub playback: Result<PlaybackHandle, ProblemDto>,
    pub restored: Restored,
    pub history: crate::history::Loaded,
}

/// Loads the config and builds the Navidrome client (the password may
/// run `password_command` or read the keychain, which can prompt).
pub async fn sources() -> Sources {
    let mut problems = Vec::new();
    let config = match Config::load(cli_from_process(), &Env::from_process()) {
        Ok(config) => Some(config),
        Err(ConfigError::NoSource { .. }) => {
            tracing::info!("no music source configured");
            problems.push(ProblemDto {
                kind: ProblemKind::NoConfig,
                message: "No music source is set up yet: choose a music folder or connect \
                          a Navidrome server."
                    .into(),
            });
            None
        }
        Err(error) => {
            tracing::warn!(%error, "config could not be loaded");
            problems.push(ProblemDto {
                kind: ProblemKind::Config,
                message: error.to_string(),
            });
            None
        }
    };
    let Some(config) = config else {
        return Sources {
            music_dir: None,
            client: None,
            scrobble: false,
            cover_art: true,
            problems,
        };
    };
    let scrobble = config.navidrome.as_ref().is_some_and(|n| n.scrobble);
    let client = match config::navidrome_client(config.navidrome).await {
        Ok(client) => client,
        Err(error) => {
            problems.push(ProblemDto {
                kind: ProblemKind::Navidrome,
                message: error.to_string(),
            });
            None
        }
    };
    Sources {
        music_dir: config.music_dir,
        scrobble: scrobble && client.is_some(),
        client,
        cover_art: config.cover_art,
        problems,
    }
}

/// What the TUI's command line would get without arguments: only
/// `CODA_MUSIC_DIR` (the GUI takes no arguments).
pub fn cli_from_process() -> Cli {
    Cli {
        music_dir: std::env::var_os(config::MUSIC_DIR_ENV).filter(|dir| !dir.is_empty()),
        ..Cli::default()
    }
}

/// Starts mpv. A failure is a problem, not an error: the app runs without
/// playback.
pub async fn playback() -> Result<PlaybackHandle, ProblemDto> {
    let spawned = playback::spawn_backend().await;
    if let Ok(handle) = &spawned
        && let Some(pid) = handle.pid()
    {
        MPV_PID.store(pid, Ordering::SeqCst);
    }
    spawned.map_err(|error| {
        tracing::error!(%error, "mpv could not be started");
        ProblemDto {
            kind: if error.is_not_found() {
                ProblemKind::MpvMissing
            } else {
                ProblemKind::Playback
            },
            message: error.to_string(),
        }
    })
}

/// The running mpv's pid (0: none), for [`kill_recorded_mpv`]: the last
/// resort when the process ends without the normal shutdown (a panic,
/// which aborts in release builds).
static MPV_PID: AtomicU32 = AtomicU32::new(0);

/// mpv was shut down (or never ran): nothing to kill any more, and its pid
/// may be reused.
pub fn forget_mpv_pid() {
    MPV_PID.store(0, Ordering::SeqCst);
}

/// Sends SIGTERM to the recorded mpv, if any. Async-signal-safe enough for
/// a panic hook: one atomic load and one `kill`.
pub fn kill_recorded_mpv() {
    let pid = MPV_PID.swap(0, Ordering::SeqCst);
    if let Ok(pid) = libc::pid_t::try_from(pid)
        && pid > 0
    {
        // SAFETY: `kill` has no memory effects.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
}

/// Next to the GUI's session: the TUI's session was looked at once (and
/// imported if it was valid). Never again after that, even if the GUI's
/// own session is later set aside.
const IMPORT_MARKER: &str = ".tui-imported";

/// Copies the TUI's session file to `gui` once, if the GUI has none yet,
/// so the first run starts with the TUI's queue. The TUI's file is only
/// read (non-blocking, regular files only, see `session::read_valid`)
/// and copied only if it is a valid session, so the GUI never sets aside
/// a copy of it. Whatever the outcome, a marker records that the import
/// was tried. Returns whether a copy was made.
pub fn import_tui_session(gui: &Path, tui: &Path) -> bool {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let Some(dir) = gui.parent() else {
        return false;
    };
    let marker = dir.join(IMPORT_MARKER);
    let exists = |path: &Path| std::fs::symlink_metadata(path).is_ok();
    if exists(gui) || exists(&marker) || !exists(tui) {
        return false;
    }
    let bytes = session::read_valid(tui);
    let temp = gui.with_extension("json.import");
    let result = (|| {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        if let Some(bytes) = &bytes {
            // A stale temp from an interrupted import is removed; the new
            // one is created fresh, never through a link.
            let _ = std::fs::remove_file(&temp);
            let mut out = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temp)?;
            out.write_all(bytes)?;
            out.sync_all()?;
            std::fs::rename(&temp, gui)?;
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&marker)
            .map(drop)
    })();
    match (result, bytes.is_some()) {
        (Ok(()), true) => {
            tracing::info!("imported the TUI's session");
            true
        }
        (Ok(()), false) => {
            tracing::info!("the TUI's session is not usable; not imported");
            false
        }
        (Err(error), _) => {
            tracing::warn!(error = %error.kind(), "could not import the TUI's session");
            let _ = std::fs::remove_file(&temp);
            false
        }
    }
}

/// Reads the GUI's saved session on a blocking thread, importing the
/// TUI's once if the GUI has none.
pub async fn restore(path: Option<PathBuf>, tui: Option<PathBuf>) -> Restored {
    tokio::task::spawn_blocking(move || {
        if let (Some(gui), Some(tui)) = (&path, &tui) {
            import_tui_session(gui, tui);
        }
        session::restore(path)
    })
    .await
    .unwrap_or_else(|_| Restored {
        session: Default::default(),
        save_path: None,
        notice: Some("Saved queue could not be read".into()),
    })
}

/// All of startup, mpv and the password concurrently.
pub async fn start(paths: &Paths) -> Startup {
    let history_path = paths.history.clone();
    let (sources, playback, restored, history) = tokio::join!(
        sources(),
        playback(),
        restore(paths.session.clone(), paths.tui_session.clone()),
        async move {
            tokio::task::spawn_blocking(move || crate::history::load(history_path))
                .await
                .unwrap_or_default()
        }
    );
    Startup {
        sources,
        playback,
        restored,
        history,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_files_live_in_their_own_folder_next_to_coda() {
        let paths = Paths::new(None, None, Some("/home/u".into()));
        assert_eq!(
            paths.session,
            Some(PathBuf::from(
                "/home/u/.local/state/coda/coda-app/session.json"
            ))
        );
        assert_eq!(
            paths.log,
            Some(PathBuf::from(
                "/home/u/.local/state/coda/coda-app/coda-app.log"
            ))
        );
        // Never Coda's own session file.
        assert_ne!(
            paths.session,
            config::session_file_path(None, Some("/home/u".into()))
        );
        assert!(paths.cover_cache.is_some());
        let paths = Paths::new(Some("/xdg/state".into()), None, Some("/home/u".into()));
        assert_eq!(
            paths.session,
            Some(PathBuf::from("/xdg/state/coda/coda-app/session.json"))
        );
        assert!(Paths::new(None, None, None).session.is_none());
        assert_eq!(
            paths.history,
            Some(PathBuf::from("/xdg/state/coda/coda-app/history.json"))
        );
        assert_eq!(
            paths.tui_session,
            Some(PathBuf::from("/xdg/state/coda/state.json"))
        );
    }

    #[test]
    fn the_tui_session_is_imported_once_and_never_changed() {
        let dir = tempfile::tempdir().unwrap();
        let tui = dir.path().join("coda/state.json");
        let gui = dir.path().join("coda/coda-app/session.json");
        let marker = gui.parent().unwrap().join(IMPORT_MARKER);
        std::fs::create_dir_all(tui.parent().unwrap()).unwrap();
        session::save(&tui, &session::Session::default()).unwrap();
        let text = std::fs::read(&tui).unwrap();
        assert!(import_tui_session(&gui, &tui));
        assert_eq!(std::fs::read(&gui).unwrap(), text);
        assert!(marker.exists());
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&gui).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        // Never again: not while the GUI has its own, and not after the
        // GUI's own was set aside either.
        assert!(!import_tui_session(&gui, &tui));
        std::fs::remove_file(&gui).unwrap();
        assert!(!import_tui_session(&gui, &tui));
        assert!(!gui.exists());
        assert_eq!(std::fs::read(&tui).unwrap(), text, "TUI file untouched");
    }

    #[test]
    fn an_invalid_tui_session_is_not_copied_nor_retried() {
        let dir = tempfile::tempdir().unwrap();
        let tui = dir.path().join("coda/state.json");
        let gui = dir.path().join("coda/coda-app/session.json");
        std::fs::create_dir_all(tui.parent().unwrap()).unwrap();
        std::fs::write(&tui, "not json").unwrap();
        assert!(!import_tui_session(&gui, &tui));
        assert!(!gui.exists(), "nothing for the GUI to set aside");
        assert_eq!(std::fs::read_to_string(&tui).unwrap(), "not json");
        let names: Vec<_> = std::fs::read_dir(tui.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 2, "state.json and coda-app/ only: {names:?}");
        // Fixed later: still not imported (the attempt was made once).
        session::save(&tui, &session::Session::default()).unwrap();
        assert!(!import_tui_session(&gui, &tui));
        // A FIFO does not hang and is not imported.
        let other = dir.path().join("x/coda-app/session.json");
        let fifo = dir.path().join("fifo.json");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(!import_tui_session(&other, &fifo));
        assert!(!other.exists());
        // No TUI file: no marker either (a later TUI session can still be
        // imported).
        let none = dir.path().join("y/coda-app/session.json");
        assert!(!import_tui_session(&none, &dir.path().join("none.json")));
        assert!(!none.parent().unwrap().join(IMPORT_MARKER).exists());
    }
}
