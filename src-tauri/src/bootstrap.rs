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
    playback::spawn_backend().await.map_err(|error| {
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

/// Largest TUI session file imported (the session loader's own cap).
const MAX_IMPORT_BYTES: u64 = 16 * 1024 * 1024;

/// Copies the TUI's session file to `gui` if the GUI has none yet, so the
/// first run starts with the TUI's queue. The TUI's file is only read:
/// the copy is what the loader validates (and sets aside if it is
/// invalid). Returns whether a copy was made.
pub fn import_tui_session(gui: &Path, tui: &Path) -> bool {
    use std::io::{Read, Write};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    match std::fs::symlink_metadata(gui) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        _ => return false,
    }
    let Ok(file) = std::fs::File::open(tui) else {
        return false;
    };
    // A regular file only (never block on a FIFO), within the cap.
    match file.metadata() {
        Ok(m) if m.is_file() && m.len() <= MAX_IMPORT_BYTES => {}
        _ => return false,
    }
    let mut bytes = Vec::new();
    if file
        .take(MAX_IMPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 > MAX_IMPORT_BYTES
    {
        return false;
    }
    let Some(dir) = gui.parent() else {
        return false;
    };
    let temp = gui.with_extension("json.import");
    let result = (|| {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)?;
        out.write_all(&bytes)?;
        out.sync_all()?;
        std::fs::rename(&temp, gui)
    })();
    match result {
        Ok(()) => {
            tracing::info!("imported the TUI's session");
            true
        }
        Err(error) => {
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
    let (sources, playback, restored) = tokio::join!(
        sources(),
        playback(),
        restore(paths.session.clone(), paths.tui_session.clone())
    );
    Startup {
        sources,
        playback,
        restored,
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
            paths.tui_session,
            Some(PathBuf::from("/xdg/state/coda/state.json"))
        );
    }

    #[test]
    fn the_tui_session_is_imported_once_and_never_changed() {
        let dir = tempfile::tempdir().unwrap();
        let tui = dir.path().join("coda/state.json");
        let gui = dir.path().join("coda/coda-app/session.json");
        std::fs::create_dir_all(tui.parent().unwrap()).unwrap();
        let text = r#"{"version":1,"tracks":[{"source":{"kind":"local","path":"/m/a.flac"},"title":"A"}],"current":0,"volume":40,"repeat":"off"}"#;
        std::fs::write(&tui, text).unwrap();
        assert!(import_tui_session(&gui, &tui));
        assert_eq!(std::fs::read_to_string(&gui).unwrap(), text);
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&gui).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        // Once the GUI has its own, the TUI's is never imported again.
        std::fs::write(&tui, "{}").unwrap();
        assert!(!import_tui_session(&gui, &tui));
        assert_eq!(std::fs::read_to_string(&gui).unwrap(), text);
        // An invalid TUI file: the GUI's copy is set aside by the loader,
        // the TUI's own file stays exactly as it was.
        std::fs::remove_file(&gui).unwrap();
        std::fs::write(&tui, "not json").unwrap();
        assert!(import_tui_session(&gui, &tui));
        let restored = session::restore(Some(gui.clone()));
        assert!(restored.session.tracks.is_empty());
        assert_eq!(std::fs::read_to_string(&tui).unwrap(), "not json");
        // No TUI file: nothing to import.
        let other = dir.path().join("x/session.json");
        assert!(!import_tui_session(&other, &dir.path().join("none.json")));
        assert!(!other.exists());
    }
}
