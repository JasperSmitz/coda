//! Startup: config, logging, the Navidrome client, mpv and the saved
//! session. Nothing here may crash the app: a missing config, password or
//! mpv becomes a [`ProblemDto`] the UI shows as a banner.
//!
//! The GUI shares Coda's `config.toml` (music folder, Navidrome account)
//! and cover cache, but keeps its own session and log under
//! `<Coda's state dir>/coda-app/`, so the TUI and GUI never overwrite
//! each other's queue.

use std::ffi::OsString;
use std::path::PathBuf;

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
        let dir = config::state_dir(xdg_state_home, home.clone()).map(|d| d.join("coda-app"));
        Self {
            session: dir.as_ref().map(|d| d.join("session.json")),
            log: dir.map(|d| d.join("coda-app.log")),
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
    let config = match Config::load(Cli::default(), &Env::from_process()) {
        Ok(config) => Some(config),
        Err(ConfigError::NoSource { .. }) => {
            tracing::info!("no music source configured");
            problems.push(ProblemDto {
                kind: ProblemKind::NoConfig,
                message: "No music source is set up yet. Set music_dir or a [navidrome] \
                          section in Coda's config.toml (or run `coda --setup`), then restart."
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

/// Reads the GUI's saved session on a blocking thread.
pub async fn restore(path: Option<PathBuf>) -> Restored {
    tokio::task::spawn_blocking(move || session::restore(path))
        .await
        .unwrap_or_else(|_| Restored {
            session: Default::default(),
            save_path: None,
            notice: Some("Saved queue could not be read".into()),
        })
}

/// All of startup, mpv and the password concurrently.
pub async fn start(paths: &Paths) -> Startup {
    let (sources, playback, restored) =
        tokio::join!(sources(), playback(), restore(paths.session.clone()));
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
    }
}
