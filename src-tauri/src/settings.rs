//! The Settings view and first-run onboarding. Coda's `config.toml` and
//! keychain are written exactly like the TUI's settings and setup
//! (`coda_core::setup_actions`, `config_writer`, `secret_store`): the
//! password first, then the config; unknown settings are kept, a file
//! with settings Coda did not write is backed up first, and an unreadable
//! file is never replaced. Only once a change is saved does the engine
//! apply it live ([`SettingsChange`]).
//!
//! Who owns the server decides what can be changed, as in the TUI: a
//! server Coda wrote (keychain password) can be edited or disconnected;
//! one set outside Coda (environment variables, a hand-written `password`
//! or `password_command`) is shown and left alone.
//!
//! Writes run one at a time on blocking threads; the quit sequence waits
//! (bounded) for a running one. The password goes from the form to the
//! keychain and the client only: never back to the webview, never logged.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use coda_core::config::{self, Env, MUSIC_DIR_ENV};
use coda_core::config_writer::{self, Preferences};
use coda_core::navidrome::{self, Password};
use coda_core::onboarding::{self, SaveError, Server};
use coda_core::secret_store::{SecretError, SecretStore};
use coda_core::setup_actions::{self, SaveNavidrome};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tokio::sync::{Mutex, OwnedMutexGuard, oneshot};

use crate::engine::{EngineHandle, SettingsChange};

/// Bound on waiting for a running settings write at exit.
pub const WRITE_GRACE: Duration = Duration::from_secs(3);

const NO_CONFIG_LOCATION: &str =
    "There is no config file location; set CODA_CONFIG or HOME to save settings";
const OUTSIDE: &str = "This server is set up outside Coda; change it where it is set";
const NOTHING_CHOSEN: &str = "Choose a music folder or connect a server";
const SAVED_FOR_NEXT_START: &str = "Saved; the change applies when Coda is started again";

const QUITTING: &str = "Coda is quitting; nothing was saved";
const CHANGED_MEANWHILE: &str = "The settings were changed meanwhile (by Coda in a terminal?); nothing was saved. \
     Restart Coda to use them.";

/// One settings write (keychain, then config) at a time. Held only for
/// the write itself (never across a network ping). Once quitting starts,
/// [`SettingsLock::finish`] takes it for good, so no write can start
/// while the app shuts down.
#[derive(Default)]
pub struct SettingsLock {
    lock: Arc<Mutex<()>>,
    quitting: AtomicBool,
}

impl SettingsLock {
    /// The lock for one write; refused once quitting has started.
    async fn acquire(&self) -> Result<OwnedMutexGuard<()>, String> {
        if self.quitting.load(Ordering::SeqCst) {
            return Err(QUITTING.into());
        }
        let guard = self.lock.clone().lock_owned().await;
        if self.quitting.load(Ordering::SeqCst) {
            return Err(QUITTING.into());
        }
        Ok(guard)
    }

    /// Stops new writes and waits up to [`WRITE_GRACE`] for a running one,
    /// so the app does not quit between the keychain write and the config
    /// write. The lock is then kept until the process exits.
    pub async fn finish(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        match tokio::time::timeout(WRITE_GRACE, self.lock.clone().lock_owned()).await {
            Ok(guard) => std::mem::forget(guard),
            Err(_) => tracing::warn!("a settings save did not finish before exit"),
        }
    }
}

/// The server as the settings show it (never a password).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ServerSettingsDto {
    NotConfigured,
    /// Written by Coda (keychain): can be edited and disconnected.
    Coda {
        url: String,
        username: String,
    },
    /// Set outside Coda; `by` names the variables or keys (never values).
    Outside {
        url: Option<String>,
        username: Option<String>,
        by: Vec<String>,
    },
}

/// What the Settings view shows (the `settings_get` command).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    /// The config file, if a location is known (shown to the user).
    pub config_path: Option<String>,
    /// `config.toml` is a symbolic link (e.g. managed dotfiles): a save
    /// replaces the link with a regular file and leaves its target alone.
    pub config_is_link: bool,
    /// `music_dir` as written in the file (`~/Music` stays `~/Music`).
    pub music_dir: Option<String>,
    /// `CODA_MUSIC_DIR` is set and used instead of the file's folder.
    pub music_dir_env: bool,
    pub server: ServerSettingsDto,
    /// Variables that keep a server from being saved here (names only).
    pub server_blocked_by: Vec<String>,
    pub scrobble: bool,
    pub cover_art: bool,
    /// Why the file could not be read (nothing can be saved then).
    pub problem: Option<String>,
}

/// A server as typed in the form. `Debug` hides everything.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInput {
    pub url: String,
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for ServerInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServerInput(<redacted>)")
    }
}

impl ServerInput {
    fn into_server(self) -> Result<Server, String> {
        let url = onboarding::normalize_url(&self.url).map_err(|e| capitalize(&e.to_string()))?;
        let username = self.username.trim().to_owned();
        if username.is_empty() {
            return Err("Enter a username".into());
        }
        if self.password.is_empty() {
            return Err("Enter the password".into());
        }
        Ok(Server {
            url,
            username,
            password: Password::new(self.password),
        })
    }
}

/// The current settings and what owns them, read from the file.
struct Current {
    config_path: Option<PathBuf>,
    dto: SettingsDto,
    /// The keychain entry Coda wrote for the current server.
    previous_account: Option<String>,
}

fn read_current(env: &Env) -> Current {
    let overrides = config::navidrome_env_overrides(env);
    let blocked: Vec<String> = onboarding::navidrome_blocked_by(&overrides)
        .into_iter()
        .map(str::to_owned)
        .collect();
    let music_dir_env = std::env::var_os(MUSIC_DIR_ENV).is_some_and(|d| !d.is_empty());
    let mut dto = SettingsDto {
        config_path: None,
        config_is_link: false,
        music_dir: None,
        music_dir_env,
        server: ServerSettingsDto::NotConfigured,
        server_blocked_by: blocked,
        scrobble: true,
        cover_art: true,
        problem: None,
    };
    let start = match config::read_for_setup(env) {
        Ok(start) => start,
        Err(error) => {
            dto.problem = Some(match error {
                config::ConfigError::NoConfigLocation => NO_CONFIG_LOCATION.into(),
                error => capitalize(&error.to_string()),
            });
            return Current {
                config_path: config::config_file_path(env).map(|(path, _)| path),
                dto,
                previous_account: None,
            };
        }
    };
    dto.config_path = Some(start.config_path.display().to_string());
    dto.config_is_link =
        std::fs::symlink_metadata(&start.config_path).is_ok_and(|m| m.file_type().is_symlink());
    dto.music_dir = start.music_dir.clone();
    match config_writer::read_preferences(&start.config_path) {
        Ok(prefs) => {
            dto.scrobble = prefs.scrobble;
            dto.cover_art = prefs.cover_art;
        }
        Err(_) => dto.problem = Some("The config file could not be read".into()),
    }
    let mut by: Vec<String> = overrides.iter().map(|s| (*s).to_owned()).collect();
    if let Some(key) = start.hand_written_password {
        by.push(key.to_owned());
    }
    dto.server = if !start.has_navidrome && overrides.is_empty() {
        ServerSettingsDto::NotConfigured
    } else if !by.is_empty() || start.url_withheld {
        ServerSettingsDto::Outside {
            url: start.url.clone(),
            username: start.username.clone(),
            by,
        }
    } else {
        ServerSettingsDto::Coda {
            url: start.url.clone().unwrap_or_default(),
            username: start.username.clone().unwrap_or_default(),
        }
    };
    Current {
        config_path: Some(start.config_path),
        dto,
        previous_account: start.keychain_account,
    }
}

impl Current {
    /// The file to write, unless the settings cannot be saved.
    fn writable(&self) -> Result<PathBuf, String> {
        if let Some(problem) = &self.dto.problem {
            return Err(problem.clone());
        }
        self.config_path
            .clone()
            .ok_or_else(|| NO_CONFIG_LOCATION.into())
    }

    /// The file to write a server to.
    fn server_writable(&self) -> Result<PathBuf, String> {
        let path = self.writable()?;
        if !self.dto.server_blocked_by.is_empty() {
            let names: Vec<&str> = self
                .dto
                .server_blocked_by
                .iter()
                .map(String::as_str)
                .collect();
            return Err(onboarding::navidrome_blocked_message(&names));
        }
        if matches!(self.dto.server, ServerSettingsDto::Outside { .. }) {
            return Err(OUTSIDE.into());
        }
        Ok(path)
    }
}

/// Runs blocking work off the async runtime.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| setup_actions::INTERRUPTED.to_owned())
}

async fn current() -> Result<Current, String> {
    blocking(|| read_current(&Env::from_process())).await
}

fn save_message(error: SaveError) -> String {
    match error {
        SaveError::Keychain(message) | SaveError::Config(message) => message,
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Applies a saved change; a refusal (still starting) means it applies at
/// the next start.
async fn apply(engine: &EngineHandle, change: SettingsChange) -> Result<(), String> {
    engine.apply_settings(change).await.map_err(|error| {
        tracing::info!(%error, "settings change saved but not applied");
        SAVED_FOR_NEXT_START.to_owned()
    })
}

/// Builds the client for `server` (before anything is written).
fn client_for(server: &Server) -> Result<navidrome::Client, String> {
    navidrome::Client::new(
        server.url.clone(),
        server.username.clone(),
        server.password.clone(),
    )
    .map_err(|error| {
        tracing::warn!(detail = %error.detail(), "settings: could not set up the client");
        "Could not set up the Navidrome client".to_owned()
    })
}

/// A typed folder, checked like `music_dir` in the file; returns the
/// canonical folder to scan.
fn check_folder(typed: &str) -> Result<PathBuf, String> {
    let env = Env::from_process();
    setup_actions::check_folder(typed, &env)?;
    config::expand_music_dir(typed, &env)
        .and_then(|path| config::validate_music_dir(path).ok())
        .ok_or_else(|| "That folder cannot be read".to_owned())
}

// ----- commands --------------------------------------------------------

#[tauri::command]
pub async fn settings_get() -> Result<SettingsDto, String> {
    Ok(current().await?.dto)
}

/// Pings a server with the typed credentials; nothing is saved.
#[tauri::command]
pub async fn settings_test_server(server: ServerInput) -> Result<(), String> {
    setup_actions::ping(server.into_server()?).await
}

/// Pings the server (before taking the write lock), stores the password,
/// writes `[navidrome]` (keeping the folder and `scrobble`), then switches
/// the engine to the new client.
#[tauri::command]
pub async fn settings_save_server(
    engine: State<'_, EngineHandle>,
    lock: State<'_, SettingsLock>,
    server: ServerInput,
) -> Result<(), String> {
    let server = server.into_server()?;
    // Refused at once if a server cannot be saved here; checked again
    // under the lock.
    current().await?.server_writable()?;
    setup_actions::ping(server.clone()).await?;
    let client = client_for(&server)?;
    let scrobble = {
        let _write = lock.acquire().await?;
        let current = current().await?;
        let path = current.server_writable()?;
        let previous = current.previous_account;
        blocking(move || {
            setup_actions::save_navidrome(
                &path,
                SaveNavidrome::Server(server),
                SecretStore::system,
                previous.as_deref(),
            )
        })
        .await?
        .map_err(save_message)?
    };
    apply(
        &engine,
        SettingsChange::Client {
            client: Some(client),
            scrobble,
        },
    )
    .await
}

/// Removes `[navidrome]` and the keychain entry Coda wrote. Refused when
/// the file would keep no music source.
#[tauri::command]
pub async fn settings_disconnect(
    engine: State<'_, EngineHandle>,
    lock: State<'_, SettingsLock>,
) -> Result<(), String> {
    {
        let _write = lock.acquire().await?;
        let current = current().await?;
        let path = current.server_writable()?;
        if current.dto.music_dir.is_none() {
            return Err(
                "Navidrome is the only source in the config file, so it stays; \
                        choose a music folder first"
                    .into(),
            );
        }
        let previous = current.previous_account;
        blocking(move || {
            setup_actions::save_navidrome(
                &path,
                SaveNavidrome::Remove,
                SecretStore::system,
                previous.as_deref(),
            )
        })
        .await?
        .map_err(save_message)?;
    }
    apply(
        &engine,
        SettingsChange::Client {
            client: None,
            scrobble: false,
        },
    )
    .await
}

/// The native folder picker. `None` if cancelled.
#[tauri::command]
pub async fn settings_pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = oneshot::channel();
    app.dialog()
        .file()
        .set_title("Choose your music folder")
        .pick_folder(move |folder| {
            let _ = tx.send(folder);
        });
    let folder = rx
        .await
        .map_err(|_| setup_actions::INTERRUPTED.to_owned())?;
    Ok(folder
        .and_then(|f| f.into_path().ok())
        .map(|p| p.display().to_string()))
}

/// Writes `music_dir` and rescans. Refused while `CODA_MUSIC_DIR` is set
/// (it would be used instead at the next start).
#[tauri::command]
pub async fn settings_set_music_dir(
    engine: State<'_, EngineHandle>,
    lock: State<'_, SettingsLock>,
    folder: String,
) -> Result<(), String> {
    let typed = folder.trim().to_owned();
    let dir = blocking({
        let typed = typed.clone();
        move || check_folder(&typed)
    })
    .await??;
    {
        let _write = lock.acquire().await?;
        let current = current().await?;
        let path = current.writable()?;
        if current.dto.music_dir_env {
            return Err(format!(
                "{MUSIC_DIR_ENV} is set and would be used instead; unset it to change the folder here."
            ));
        }
        let prefs = Preferences {
            music_dir: Some(typed),
            ..Preferences::default()
        };
        write_preferences(path, prefs).await?;
    }
    apply(&engine, SettingsChange::MusicDir(dir)).await
}

/// Turns cover art and/or scrobbling on or off.
#[tauri::command]
pub async fn settings_set_options(
    engine: State<'_, EngineHandle>,
    lock: State<'_, SettingsLock>,
    cover_art: Option<bool>,
    scrobble: Option<bool>,
) -> Result<(), String> {
    {
        let _write = lock.acquire().await?;
        let path = current().await?.writable()?;
        let prefs = Preferences {
            music_dir: None,
            scrobble,
            cover_art,
        };
        write_preferences(path, prefs).await?;
    }
    // Both are applied even if the first is refused (both are saved).
    let mut result = Ok(());
    if let Some(on) = cover_art {
        result = result.and(apply(&engine, SettingsChange::CoverArt(on)).await);
    }
    if let Some(on) = scrobble {
        let applied = apply(&engine, SettingsChange::Scrobble(on)).await;
        result = result.and(applied);
    }
    result
}

async fn write_preferences(path: PathBuf, prefs: Preferences) -> Result<(), String> {
    blocking(move || config_writer::write_preferences(&path, &prefs))
        .await?
        .map(|written| {
            tracing::info!(
                backup = written.backup.is_some(),
                "settings: config written"
            );
        })
        .map_err(|error| {
            tracing::warn!(error = %error.kind(), "settings: could not write the config");
            format!("Could not save the settings: {error}")
        })
}

/// The first-run save. Re-reads the file right before writing and refuses
/// unless it still configures no source (the TUI's `coda --setup` may have
/// written one meanwhile), so it never removes a `[navidrome]` section, a
/// keychain entry or a folder it did not create: with no server typed,
/// only a section without a server (e.g. `scrobble` alone, which is kept)
/// can be there, and no keychain entry is ever deleted. Blocking. Returns
/// the file's `scrobble` setting.
fn save_onboarding(
    env: &Env,
    music_dir: Option<String>,
    server: Option<Server>,
    open_store: fn() -> Result<SecretStore, SecretError>,
) -> Result<bool, String> {
    let current = read_current(env);
    let path = if server.is_some() {
        current.server_writable()?
    } else {
        current.writable()?
    };
    let sourceless =
        current.dto.music_dir.is_none() && current.dto.server == ServerSettingsDto::NotConfigured;
    if !sourceless {
        return Err(CHANGED_MEANWHILE.into());
    }
    let navidrome = match server {
        Some(server) => SaveNavidrome::Server(server),
        None => SaveNavidrome::Remove,
    };
    setup_actions::save(&path, music_dir, navidrome, open_store, None).map_err(save_message)?;
    Ok(config_writer::read_preferences(&path).map_or(true, |prefs| prefs.scrobble))
}

/// First run: saves a music folder and/or a server (pinged first, outside
/// the write lock) like Coda's setup, then starts using them.
#[tauri::command]
pub async fn onboarding_finish(
    engine: State<'_, EngineHandle>,
    lock: State<'_, SettingsLock>,
    music_dir: Option<String>,
    server: Option<ServerInput>,
) -> Result<(), String> {
    let music_dir = music_dir
        .map(|d| d.trim().to_owned())
        .filter(|d| !d.is_empty());
    let server = server.map(ServerInput::into_server).transpose()?;
    if music_dir.is_none() && server.is_none() {
        return Err(NOTHING_CHOSEN.into());
    }
    let dir = match &music_dir {
        Some(typed) => {
            let typed = typed.clone();
            Some(blocking(move || check_folder(&typed)).await??)
        }
        None => None,
    };
    let client = match &server {
        Some(server) => {
            setup_actions::ping(server.clone()).await?;
            Some(client_for(server)?)
        }
        None => None,
    };
    let scrobble = {
        let _write = lock.acquire().await?;
        blocking(move || {
            save_onboarding(&Env::from_process(), music_dir, server, SecretStore::system)
        })
        .await??
    };
    if let Some(dir) = dir {
        apply(&engine, SettingsChange::MusicDir(dir)).await?;
    }
    if client.is_some() {
        apply(&engine, SettingsChange::Client { client, scrobble }).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(dir: &std::path::Path) -> Env {
        Env {
            xdg_config_home: Some(dir.join("xdg").into()),
            home: Some(dir.join("home").into()),
            ..Env::default()
        }
    }

    fn write(dir: &std::path::Path, text: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("xdg/coda/config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    #[test]
    fn ownership_decides_what_can_be_changed() {
        let dir = tempfile::tempdir().unwrap();
        // No file: nothing configured, writable.
        let current = read_current(&env(dir.path()));
        assert_eq!(current.dto.server, ServerSettingsDto::NotConfigured);
        assert!(current.dto.scrobble && current.dto.cover_art);
        assert!(current.server_writable().is_ok());
        // Written by Coda.
        write(
            dir.path(),
            "music_dir = \"~/Music\"\n[navidrome]\nurl = \"https://h.example/\"\n\
             username = \"me\"\npassword_keychain = true\nscrobble = false\n\n\
             [ui]\ncover_art = false\n",
        );
        let current = read_current(&env(dir.path()));
        assert_eq!(
            current.dto.server,
            ServerSettingsDto::Coda {
                url: "https://h.example/".into(),
                username: "me".into()
            }
        );
        assert_eq!(current.dto.music_dir.as_deref(), Some("~/Music"));
        assert!(!current.dto.scrobble && !current.dto.cover_art);
        assert!(current.previous_account.is_some());
        assert!(current.server_writable().is_ok());
        // A hand-written password: shown, left alone.
        write(
            dir.path(),
            "[navidrome]\nurl = \"https://h.example/\"\nusername = \"me\"\n\
             password_command = [\"pass\"]\n",
        );
        let current = read_current(&env(dir.path()));
        assert!(matches!(
            &current.dto.server,
            ServerSettingsDto::Outside { by, .. } if by == &["password_command"]
        ));
        assert_eq!(current.server_writable().unwrap_err(), OUTSIDE);
        assert!(current.writable().is_ok(), "other settings can change");
        // The environment's server blocks saving one here.
        let mut e = env(dir.path());
        e.navidrome_url = Some("https://other.example".into());
        let current = read_current(&e);
        assert_eq!(current.dto.server_blocked_by, ["CODA_NAVIDROME_URL"]);
        assert!(current.server_writable().is_err());
        // An unreadable file is never written.
        write(dir.path(), "music_dir = [");
        let current = read_current(&env(dir.path()));
        assert!(current.dto.problem.is_some());
        assert!(current.writable().is_err());
    }

    #[test]
    fn a_symlinked_config_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles.toml");
        std::fs::write(&real, "").unwrap();
        let path = dir.path().join("xdg/coda/config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        assert!(!read_current(&env(dir.path())).dto.config_is_link);
        std::os::unix::fs::symlink(&real, &path).unwrap();
        assert!(read_current(&env(dir.path())).dto.config_is_link);
    }

    #[test]
    fn the_settings_never_carry_a_password() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "[navidrome]\nurl = \"https://h.example/\"\nusername = \"me\"\n\
             password = \"hunter2\"\n",
        );
        let json = serde_json::to_string(&read_current(&env(dir.path())).dto).unwrap();
        assert!(!json.contains("hunter2"), "{json}");
        assert!(json.contains("\"by\":[\"password\"]"), "{json}");
        let input = ServerInput {
            url: "h.example".into(),
            username: "me".into(),
            password: "hunter2".into(),
        };
        assert_eq!(format!("{input:?}"), "ServerInput(<redacted>)");
        let server = input.into_server().unwrap();
        assert_eq!(server.url.as_str(), "https://h.example/");
        assert!(!format!("{server:?}").contains("hunter2"));
    }

    fn no_store() -> Result<SecretStore, SecretError> {
        Err(SecretError::Unavailable)
    }

    #[test]
    fn onboarding_never_replaces_a_source_that_appeared_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir(&music).unwrap();
        let folder = Some(music.to_str().unwrap().to_owned());
        // `coda --setup` connected a server while onboarding was shown.
        let tui = "[navidrome]\nurl = \"https://h.example/\"\nusername = \"me\"\n\
                   password_keychain = true\n";
        let path = write(dir.path(), tui);
        let error = save_onboarding(&env(dir.path()), folder.clone(), None, no_store).unwrap_err();
        assert_eq!(error, CHANGED_MEANWHILE);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            tui,
            "kept as it was"
        );
        // Or a folder: a typed server must not drop it.
        let tui = "music_dir = \"~/Music\"\n";
        write(dir.path(), tui);
        let server = ServerInput {
            url: "h.example".into(),
            username: "me".into(),
            password: "pw".into(),
        }
        .into_server()
        .unwrap();
        let error = save_onboarding(&env(dir.path()), None, Some(server), no_store).unwrap_err();
        assert_eq!(error, CHANGED_MEANWHILE);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), tui);
    }

    #[test]
    fn onboarding_writes_a_sourceless_file_keeping_its_settings() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir(&music).unwrap();
        let path = write(
            dir.path(),
            "[navidrome]\nscrobble = false\n\n[ui]\ncover_art = false\n",
        );
        let folder = Some(music.to_str().unwrap().to_owned());
        // The store is never opened without a server.
        let scrobble = save_onboarding(&env(dir.path()), folder, None, no_store).unwrap();
        assert!(!scrobble);
        let prefs = config_writer::read_preferences(&path).unwrap();
        assert_eq!(prefs.music_dir.as_deref(), music.to_str());
        assert!(!prefs.scrobble && !prefs.cover_art);
    }

    #[tokio::test]
    async fn quitting_holds_the_write_lock_for_good() {
        let lock = SettingsLock::default();
        let write = lock.acquire().await.unwrap();
        let finish = tokio::spawn(async move {
            lock.finish().await;
            lock
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!finish.is_finished(), "waits for the running write");
        drop(write);
        let lock = finish.await.unwrap();
        assert_eq!(lock.acquire().await.unwrap_err(), QUITTING);
        assert!(lock.lock.try_lock().is_err(), "still held");
    }

    #[test]
    fn typed_servers_are_checked() {
        let input = |url: &str, user: &str, password: &str| ServerInput {
            url: url.into(),
            username: user.into(),
            password: password.into(),
        };
        assert!(input("", "me", "pw").into_server().is_err());
        assert!(input("h.example", " ", "pw").into_server().is_err());
        assert!(input("h.example", "me", "").into_server().is_err());
        assert!(input("ftp://h.example", "me", "pw").into_server().is_err());
    }
}
