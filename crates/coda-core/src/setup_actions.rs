//! The UI-free actions of the setup and the in-app settings: the drop
//! guard [`Reply`] (every spawned request answers exactly once), the
//! folder check, the ping with a temporary client, and the saves
//! (keychain first, then config; [`save`] for the setup, [`save_navidrome`]
//! for the settings). Blocking functions run on blocking threads; the
//! callers (the frontends' setup and settings code, and `cover`) spawn
//! them.
//!
//! The log records only kinds and outcomes: never the URL, host,
//! username, folder or password.

use std::io;
use std::path::Path;

use tokio::sync::mpsc;

use crate::config::{self, ConfigError, Env};
use crate::config_writer::{self, NavidromeSection, SetupConfig, SetupNavidrome};
use crate::navidrome;
use crate::onboarding::{SaveError, Server};
use crate::secret_store::{self, SecretError, SecretStore};

/// Sends `fallback` unless [`Reply::send`] was called: a request whose
/// task panics or is cancelled still answers (LESSONS.md, "Every spawned
/// request must answer exactly once"). Also used by the app's settings.
pub(crate) struct Reply<R> {
    tx: mpsc::UnboundedSender<R>,
    fallback: Option<R>,
}

impl<R> Reply<R> {
    /// Built before spawning, so a task dropped before its first poll
    /// still answers.
    pub(crate) fn new(tx: &mpsc::UnboundedSender<R>, fallback: R) -> Self {
        Self {
            tx: tx.clone(),
            fallback: Some(fallback),
        }
    }

    pub(crate) fn send(mut self, response: R) {
        self.fallback = None;
        let _ = self.tx.send(response);
    }
}

impl<R> Drop for Reply<R> {
    fn drop(&mut self) {
        if let Some(response) = self.fallback.take() {
            let _ = self.tx.send(response);
        }
    }
}

/// The answer to a request whose task died (see [`Reply`]).
pub const INTERRUPTED: &str = "Something went wrong; please try again";

/// Validates a typed folder like `music_dir` in the config file.
pub fn check_folder(typed: &str, env: &Env) -> Result<(), String> {
    let Some(path) = config::expand_music_dir(typed, env) else {
        return Err("Use an absolute path or one starting with ~/".into());
    };
    match config::validate_music_dir(path) {
        Ok(_) => Ok(()),
        Err(ConfigError::NotFound(_)) => Err("That folder does not exist".into()),
        Err(ConfigError::NotADirectory(_)) => Err("That is not a folder".into()),
        Err(_) => Err("That folder cannot be read".into()),
    }
}

/// Pings with a temporary client. The message is the client's short error
/// text; the log gets only its detail (no URL, host or response data).
pub async fn ping(server: Server) -> Result<(), String> {
    let result = match navidrome::Client::new(server.url, server.username, server.password) {
        Ok(client) => client.ping().await,
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => {
            tracing::info!("setup: navidrome ping ok");
            Ok(())
        }
        Err(error) => {
            tracing::info!(detail = %error.detail(), "setup: navidrome ping failed");
            Err(ping_message(&error))
        }
    }
}

fn ping_message(error: &navidrome::Error) -> String {
    let text = match error {
        navidrome::Error::Decode(_) => "Not a Subsonic server (unexpected response)".to_owned(),
        navidrome::Error::Http(404) => {
            "Server returned HTTP 404; is this the Navidrome URL?".to_owned()
        }
        error => error.to_string(),
    };
    capitalize(&text)
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// What a save does with Navidrome.
pub enum SaveNavidrome {
    Remove,
    /// Keep `[navidrome]` as it is; the keychain is not touched.
    Keep,
    Server(Server),
}

/// Stores the password, then writes the config with `music_dir`. Blocking.
/// See [`store_then_write`].
pub fn save(
    path: &Path,
    music_dir: Option<String>,
    navidrome: SaveNavidrome,
    open_store: fn() -> Result<SecretStore, SecretError>,
    previous: Option<&str>,
) -> Result<(), SaveError> {
    store_then_write(path, navidrome, open_store, previous, |navidrome| {
        let config = SetupConfig {
            music_dir,
            navidrome,
        };
        let backup = config_writer::write(path, &config)?;
        tracing::info!(backup = backup.is_some(), "setup: config written");
        Ok(())
    })
}

/// The in-app settings' save: stores the password (for a server), then
/// writes `[navidrome]`, keeping `music_dir` and `scrobble` as they are in
/// the file. Returns the file's `scrobble` setting. Blocking. See
/// [`store_then_write`].
pub fn save_navidrome(
    path: &Path,
    navidrome: SaveNavidrome,
    open_store: fn() -> Result<SecretStore, SecretError>,
    previous: Option<&str>,
) -> Result<bool, SaveError> {
    store_then_write(path, navidrome, open_store, previous, |navidrome| {
        let written = config_writer::write_navidrome(path, &navidrome)?;
        tracing::info!(
            backup = written.backup.is_some(),
            "settings: config written"
        );
        Ok(written.scrobble)
    })
}

/// Stores the password (for a server), then runs `write` with the
/// resulting `[navidrome]` section. Blocking.
///
/// If the config write fails after the password was stored, the new entry
/// is removed again (best effort), so a failed setup leaves nothing
/// behind, unless it is `previous`'s, which the old config still uses.
/// Once the config is written, `previous` (the entry Coda wrote for the
/// old config) is deleted best effort if it is no longer used: the account
/// changed or Navidrome was removed (not when it is kept).
fn store_then_write<T>(
    path: &Path,
    navidrome: SaveNavidrome,
    open_store: fn() -> Result<SecretStore, SecretError>,
    previous: Option<&str>,
    write: impl FnOnce(NavidromeSection) -> io::Result<T>,
) -> Result<T, SaveError> {
    let mut stored = None;
    let mut new_account = None;
    let keep = matches!(navidrome, SaveNavidrome::Keep);
    let navidrome = match navidrome {
        SaveNavidrome::Server(server) => {
            let account = secret_store::account(&server.url, &server.username);
            new_account = Some(account.clone());
            let store = open_store()
                .and_then(|store| store.set(&account, &server.password).map(|()| store));
            match store {
                Ok(store) => stored = Some((store, account)),
                Err(error) => {
                    tracing::warn!(kind = error.kind(), "setup: could not store the password");
                    return Err(SaveError::Keychain(format!(
                        "Could not store the password: {error}"
                    )));
                }
            }
            tracing::info!("setup: password stored");
            NavidromeSection::Write(SetupNavidrome {
                url: server.url,
                username: server.username,
            })
        }
        SaveNavidrome::Keep => NavidromeSection::Keep,
        SaveNavidrome::Remove => NavidromeSection::Remove,
    };
    match write(navidrome) {
        Ok(written) => {
            if let Some(previous) = previous.filter(|p| !keep && new_account.as_deref() != Some(*p))
            {
                delete_previous(
                    previous,
                    stored.as_ref().map(|(store, _)| store),
                    open_store,
                );
            }
            Ok(written)
        }
        Err(error) => {
            tracing::warn!(error = %error.kind(), "setup: could not write the config");
            if let Some((store, account)) = stored.filter(|(_, a)| Some(a.as_str()) != previous) {
                match store.delete(&account) {
                    Ok(()) => tracing::info!("setup: stored password removed again"),
                    Err(error) => tracing::warn!(
                        kind = error.kind(),
                        "setup: could not remove the stored password"
                    ),
                }
            }
            Err(SaveError::Config(format!(
                "Could not write {}: {error}",
                path.display()
            )))
        }
    }
}

/// Deletes the entry for the previous account, best effort: a failure is
/// logged by kind only and does not fail the save (the config no longer
/// refers to it).
fn delete_previous(
    account: &str,
    store: Option<&SecretStore>,
    open_store: fn() -> Result<SecretStore, SecretError>,
) {
    let result = match store {
        Some(store) => store.delete(account),
        None => open_store().and_then(|store| store.delete(account)),
    };
    match result {
        Ok(()) => tracing::info!("setup: the previous account's password was removed"),
        Err(error) => tracing::warn!(
            kind = error.kind(),
            "setup: could not remove the previous account's password"
        ),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::{Cli, Config, PasswordSource};
    use crate::navidrome::Password;
    use crate::navidrome::client::parse_base_url;
    use crate::onboarding::Response;
    use std::path::PathBuf;
    use std::sync::OnceLock;
    use std::time::Duration;

    pub(crate) fn server(url: &str, password: &str) -> Server {
        Server {
            url: parse_base_url(url).unwrap(),
            username: "me".into(),
            password: Password::new(password.into()),
        }
    }

    #[test]
    fn folder_checks_follow_the_config_rules() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join("Music")).unwrap();
        std::fs::write(home.join("file"), b"x").unwrap();
        let env = Env {
            home: Some(home.clone().into()),
            ..Env::default()
        };
        assert_eq!(check_folder("~/Music", &env), Ok(()));
        assert_eq!(
            check_folder(home.join("Music").to_str().unwrap(), &env),
            Ok(())
        );
        assert_eq!(
            check_folder("Music", &env),
            Err("Use an absolute path or one starting with ~/".into())
        );
        assert_eq!(
            check_folder("~/Nope", &env),
            Err("That folder does not exist".into())
        );
        assert_eq!(
            check_folder("~/file", &env),
            Err("That is not a folder".into())
        );
    }

    #[test]
    fn ping_messages_are_short_and_capitalised() {
        assert_eq!(
            ping_message(&navidrome::Error::Auth),
            "Authentication failed"
        );
        assert_eq!(
            ping_message(&navidrome::Error::Decode("x".into())),
            "Not a Subsonic server (unexpected response)"
        );
        assert_eq!(
            ping_message(&navidrome::Error::Http(500)),
            "Server returned HTTP 500"
        );
    }

    /// One mock store per test, shared by the `fn` pointers below.
    fn store_a() -> Result<SecretStore, SecretError> {
        static STORE: OnceLock<SecretStore> = OnceLock::new();
        Ok(STORE.get_or_init(SecretStore::mock).clone())
    }

    fn unavailable() -> Result<SecretStore, SecretError> {
        Err(SecretError::Unavailable)
    }

    #[tokio::test]
    async fn save_stores_the_password_then_writes_a_loadable_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coda").join("config.toml");
        save(
            &path,
            None,
            SaveNavidrome::Server(server("http://localhost:4533", "s3cret")),
            store_a,
            None,
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("s3cret"), "{text}");
        let env = Env {
            config_path: Some(path.clone().into()),
            ..Env::default()
        };
        let nd = Config::load(Cli::default(), &env)
            .unwrap()
            .navidrome
            .unwrap();
        assert!(matches!(nd.password, PasswordSource::Keychain { .. }));
        let password = nd
            .password
            .resolve_with(Duration::from_secs(1), store_a)
            .await
            .unwrap();
        assert_eq!(password.expose(), "s3cret");
    }

    #[test]
    fn keychain_failure_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let error = save(
            &path,
            Some("/m".into()),
            SaveNavidrome::Server(server("https://h", "p")),
            unavailable,
            None,
        )
        .unwrap_err();
        let SaveError::Keychain(message) = error else {
            panic!("{error:?}")
        };
        assert!(
            message.starts_with("Could not store the password: "),
            "{message}"
        );
        assert!(!path.exists());
    }

    #[test]
    fn config_failure_is_reported_with_the_path() {
        let dir = tempfile::tempdir().unwrap();
        // The parent is a file, so the directory cannot be created.
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"").unwrap();
        let path = blocker.join("config.toml");
        let error = save(
            &path,
            Some("/m".into()),
            SaveNavidrome::Remove,
            unavailable,
            None,
        )
        .unwrap_err();
        let SaveError::Config(message) = error else {
            panic!("{error:?}")
        };
        assert!(message.contains("blocker/config.toml"), "{message}");
    }

    async fn ping_against(body: serde_json::Value, password: &str) -> Result<(), String> {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/ping"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&mock)
            .await;
        let result = ping(server(&mock.uri(), password)).await;
        // The password itself never travels.
        for request in mock.received_requests().await.unwrap() {
            assert!(!request.url.as_str().contains(password));
        }
        result
    }

    #[tokio::test]
    async fn ping_success_and_failures_against_a_fake_server() {
        use serde_json::json;
        let ok = json!({ "subsonic-response": { "status": "ok", "version": "1.16.1" } });
        assert_eq!(ping_against(ok, "right-pass").await, Ok(()));
        let auth = json!({ "subsonic-response": {
            "status": "failed", "version": "1.16.1",
            "error": { "code": 40, "message": "Wrong username or password" }
        } });
        assert_eq!(
            ping_against(auth, "wrong-pass").await,
            Err("Authentication failed".into())
        );
        let html = json!({ "hello": "world" });
        assert_eq!(
            ping_against(html, "pw").await,
            Err("Not a Subsonic server (unexpected response)".into())
        );
        // Nothing listening.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        assert_eq!(
            ping(server(&url, "pw")).await,
            Err("Could not reach server".into())
        );
    }

    fn store_b() -> Result<SecretStore, SecretError> {
        static STORE: OnceLock<SecretStore> = OnceLock::new();
        Ok(STORE.get_or_init(SecretStore::mock).clone())
    }

    #[test]
    fn a_failed_config_write_removes_the_stored_password() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"").unwrap();
        let server = server("https://h.example", "pw");
        let account = secret_store::account(&server.url, &server.username);
        let error = save(
            &blocker.join("config.toml"),
            None,
            SaveNavidrome::Server(server),
            store_b,
            None,
        )
        .unwrap_err();
        assert!(matches!(error, SaveError::Config(_)), "{error:?}");
        assert_eq!(
            store_b().unwrap().get(&account).unwrap_err(),
            SecretError::Missing,
            "no orphaned entry"
        );
    }

    /// A mock store of its own for each `fn` pointer.
    macro_rules! own_store {
        ($name:ident) => {
            fn $name() -> Result<SecretStore, SecretError> {
                static STORE: OnceLock<SecretStore> = OnceLock::new();
                Ok(STORE.get_or_init(SecretStore::mock).clone())
            }
        };
    }

    own_store!(store_change);
    own_store!(store_change_fails);
    own_store!(store_same);
    own_store!(store_same_fails);
    own_store!(store_removed);
    own_store!(store_delete_denied);
    own_store!(store_command);
    own_store!(store_keep);
    own_store!(store_drop_folder);

    pub(crate) const OLD: &str = "me@old.example";

    pub(crate) fn with_old_entry(open: fn() -> Result<SecretStore, SecretError>) -> SecretStore {
        let store = open().unwrap();
        store.set(OLD, &Password::new("old-pw".into())).unwrap();
        store
    }

    /// A config written by Coda for the old account, plus `extra`.
    pub(crate) fn old_config(dir: &Path, extra: &str) -> PathBuf {
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            format!(
                "[navidrome]\nurl = \"https://old.example\"\nusername = \"me\"\n\
                 password_keychain = true\n{extra}"
            ),
        )
        .unwrap();
        path
    }

    fn blocked_path(dir: &Path) -> PathBuf {
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"").unwrap();
        blocker.join("config.toml")
    }

    #[test]
    fn an_account_change_deletes_the_old_entry_after_the_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = old_config(dir.path(), "scrobble = false\n");
        let store = with_old_entry(store_change);
        let new = server("https://new.example", "new-pw");
        save(
            &path,
            None,
            SaveNavidrome::Server(new.clone()),
            store_change,
            Some(OLD),
        )
        .unwrap();
        assert_eq!(store.get(OLD).unwrap_err(), SecretError::Missing);
        let account = secret_store::account(&new.url, &new.username);
        assert_eq!(store.get(&account).unwrap().expose(), "new-pw");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("new.example"), "{text}");
        assert!(text.contains("scrobble = false"), "kept: {text}");

        // A failed write keeps the old entry, which the old config still
        // uses, and removes the new one.
        let store = with_old_entry(store_change_fails);
        let error = save(
            &blocked_path(dir.path()),
            None,
            SaveNavidrome::Server(new),
            store_change_fails,
            Some(OLD),
        )
        .unwrap_err();
        assert!(matches!(error, SaveError::Config(_)), "{error:?}");
        assert_eq!(store.get(OLD).unwrap().expose(), "old-pw");
        assert_eq!(store.get(&account).unwrap_err(), SecretError::Missing);
    }

    #[test]
    fn an_unchanged_account_is_overwritten_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = old_config(dir.path(), "");
        let store = with_old_entry(store_same);
        // Same user and host; a different path does not change the account.
        let same = server("https://old.example/nd", "new-pw");
        save(
            &path,
            None,
            SaveNavidrome::Server(same.clone()),
            store_same,
            Some(OLD),
        )
        .unwrap();
        assert_eq!(store.get(OLD).unwrap().expose(), "new-pw");

        // A failed write never deletes the entry the old config uses.
        let store = with_old_entry(store_same_fails);
        save(
            &blocked_path(dir.path()),
            None,
            SaveNavidrome::Server(same),
            store_same_fails,
            Some(OLD),
        )
        .unwrap_err();
        assert_eq!(store.get(OLD).unwrap().expose(), "new-pw");
    }

    #[test]
    fn removing_navidrome_deletes_the_entry_and_keeps_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = old_config(dir.path(), "scrobble = false\n");
        let store = with_old_entry(store_removed);
        save(
            &path,
            Some("/m".into()),
            SaveNavidrome::Remove,
            store_removed,
            Some(OLD),
        )
        .unwrap();
        assert_eq!(store.get(OLD).unwrap_err(), SecretError::Missing);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("url"), "{text}");
        assert!(text.contains("music_dir = \"/m\""), "{text}");
        assert!(text.contains("[navidrome]\nscrobble = false\n"), "{text}");
    }

    #[test]
    fn a_failed_delete_of_the_old_entry_does_not_fail_the_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = old_config(dir.path(), "");
        let store = with_old_entry(store_delete_denied);
        store.fail_next(OLD, SecretError::Denied);
        save(
            &path,
            Some("/m".into()),
            SaveNavidrome::Remove,
            store_delete_denied,
            Some(OLD),
        )
        .unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("/m"));
        // Best effort: the entry is left over.
        assert_eq!(store.get(OLD).unwrap().expose(), "old-pw");
    }

    #[tokio::test]
    async fn a_password_command_config_moves_to_the_keychain_with_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let old = "[navidrome]\nurl = \"http://localhost:4533\"\nusername = \"me\"\n\
                   password_command = [\"pass\", \"show\", \"nd\"]\nscrobble = false\n";
        std::fs::write(&path, old).unwrap();
        save(
            &path,
            None,
            SaveNavidrome::Server(server("http://localhost:4533", "s3cret")),
            store_command,
            // Nothing in the keychain belonged to the old config.
            None,
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("password_keychain = true"), "{text}");
        assert!(!text.contains("password_command"), "{text}");
        assert!(text.contains("scrobble = false"), "{text}");
        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().contains(".bak."))
            .collect();
        assert_eq!(backups.len(), 1, "{backups:?}");
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), old);
        let env = Env {
            config_path: Some(path.into()),
            ..Env::default()
        };
        let nd = Config::load(Cli::default(), &env)
            .unwrap()
            .navidrome
            .unwrap();
        assert!(!nd.scrobble);
        let password = nd
            .password
            .resolve_with(Duration::from_secs(1), store_command)
            .await
            .unwrap();
        assert_eq!(password.expose(), "s3cret");
    }

    #[tokio::test]
    async fn keeping_navidrome_under_env_overrides_changes_only_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("new-music");
        std::fs::create_dir(&music).unwrap();
        let path = dir.path().join("config.toml");
        let old = "music_dir = \"/old\"\n[navidrome]\nurl = \"https://h.example\"\n\
                   username = \"me\"\npassword_command = [\"printf\", \"%s\", \"cmd \\\"pw\\\"\"]\n\
                   scrobble = false\n";
        std::fs::write(&path, old).unwrap();
        let store = with_old_entry(store_keep);
        save(
            &path,
            Some(music.to_str().unwrap().into()),
            SaveNavidrome::Keep,
            store_keep,
            Some(OLD),
        )
        .unwrap();
        assert_eq!(
            store.get(OLD).unwrap().expose(),
            "old-pw",
            "kept: not deleted"
        );
        // Startup with the override that blocked saving a server.
        let env = Env {
            config_path: Some(path.clone().into()),
            navidrome_user: Some("env-user".into()),
            ..Env::default()
        };
        let config = Config::load(Cli::default(), &env).unwrap();
        assert_eq!(
            config.music_dir,
            Some(std::fs::canonicalize(&music).unwrap())
        );
        let nd = config.navidrome.unwrap();
        assert_eq!(nd.username, "env-user");
        assert_eq!(nd.url.as_str(), "https://h.example/");
        assert!(!nd.scrobble);
        let password = nd
            .password
            .resolve_with(Duration::from_secs(5), unavailable)
            .await
            .unwrap();
        assert_eq!(
            password.expose(),
            "cmd \"pw\"",
            "the command was kept exactly"
        );
    }

    #[test]
    fn keeping_navidrome_from_an_unreadable_file_fails_instead_of_dropping_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[navidrome\n").unwrap();
        let error = save(
            &path,
            Some("/m".into()),
            SaveNavidrome::Keep,
            unavailable,
            None,
        )
        .unwrap_err();
        assert!(matches!(error, SaveError::Config(_)), "{error:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[navidrome\n");
    }

    #[test]
    fn a_save_can_drop_the_folder_and_keep_navidrome() {
        let dir = tempfile::tempdir().unwrap();
        // A new server without a folder.
        let path = old_config(dir.path(), "");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("music_dir = \"/m\"\n{text}")).unwrap();
        let server = server("https://old.example", "pw");
        save(
            &path,
            None,
            SaveNavidrome::Server(server),
            store_drop_folder,
            Some(OLD),
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("music_dir"), "{text}");
        assert!(text.contains("url = \"https://old.example/\""), "{text}");
        // Kept as it is, without a folder.
        std::fs::write(&path, format!("music_dir = \"/m\"\n{text}")).unwrap();
        save(
            &path,
            None,
            SaveNavidrome::Keep,
            store_drop_folder,
            Some(OLD),
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("music_dir"), "{text}");
        assert!(text.contains("password_keychain = true"), "{text}");
        let env = Env {
            config_path: Some(path.into()),
            ..Env::default()
        };
        let config = Config::load(Cli::default(), &env).unwrap();
        assert!(config.music_dir.is_none() && config.navidrome.is_some());
    }

    #[tokio::test]
    async fn requests_always_answer_even_if_the_task_dies() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let reply = Reply::new(
            &tx,
            Response::Pinged {
                id: 7,
                result: Err(INTERRUPTED.into()),
            },
        );
        let task = tokio::spawn(async move {
            let _reply = reply;
            std::future::pending::<()>().await;
        });
        task.abort();
        let _ = task.await;
        assert_eq!(
            rx.recv().await,
            Some(Response::Pinged {
                id: 7,
                result: Err(INTERRUPTED.into())
            })
        );
    }
}
