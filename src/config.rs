//! Startup configuration: music directory and Navidrome server, from the
//! command line, the environment and an optional TOML file (ADR 0003).
//!
//! - Config file: `$CODA_CONFIG`, else `$XDG_CONFIG_HOME/coda/config.toml`,
//!   else `~/.config/coda/config.toml`. A missing default file is fine; a
//!   missing `$CODA_CONFIG` file is an error. Unknown keys are rejected.
//! - Music directory: CLI argument > `CODA_MUSIC_DIR` > `music_dir`.
//! - Navidrome: `CODA_NAVIDROME_URL/USER/PASSWORD` override the file's
//!   `[navidrome]` values. All of url, username and a password source are
//!   required once any is given. The file names at most one password
//!   source: `password`, `password_command` or `password_keychain` (the OS
//!   credential store, ADR 0004).
//! - At least one source (music directory or Navidrome) is required.
//!
//! Error messages name files, fields and variables, never their values.

use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read};

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

use serde::{Deserialize, Deserializer};
use url::Url;

use crate::navidrome::Password;
use crate::navidrome::client::{UrlError, parse_base_url};
use crate::secret_store::{self, STORE_NAME, SecretError, SecretStore};

pub const MUSIC_DIR_ENV: &str = "CODA_MUSIC_DIR";
pub const CONFIG_ENV: &str = "CODA_CONFIG";
pub const NAVIDROME_URL_ENV: &str = "CODA_NAVIDROME_URL";
pub const NAVIDROME_USER_ENV: &str = "CODA_NAVIDROME_USER";
pub const NAVIDROME_PASSWORD_ENV: &str = "CODA_NAVIDROME_PASSWORD";
/// Upper bound on `password_command` output; more is an error.
pub const PASSWORD_COMMAND_MAX_OUTPUT: usize = 64 * 1024;
/// How long `password_command` may run before it is killed.
pub const PASSWORD_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Command line. `clap` reads `CODA_MUSIC_DIR` when the argument is absent.
/// The value is an `OsString` because clap's path parser rejects empty
/// values with a generic error; an empty value counts as unset.
#[derive(Debug, Default, clap::Parser)]
#[command(
    name = "coda",
    version,
    about = "A keyboard-first terminal music player"
)]
pub struct Cli {
    /// Music directory to browse [default: $CODA_MUSIC_DIR, else
    /// `music_dir` in the config file]
    #[arg(value_name = "MUSIC_DIR", env = MUSIC_DIR_ENV, hide_env = true)]
    pub music_dir: Option<OsString>,
    /// Run the setup again to change the music folder or Navidrome server
    #[arg(long)]
    pub setup: bool,
}

/// Where a music directory given to the command comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicDirArg {
    /// The `MUSIC_DIR` argument.
    Argument,
    /// `CODA_MUSIC_DIR` (clap reads it into the argument).
    Env,
}

impl Cli {
    /// Parses `args` like [`clap::Parser::try_parse_from`], and also
    /// reports where a (non-empty) music directory came from, so setup can
    /// say what overrides the file.
    pub fn try_parse_with_source<I, T>(args: I) -> Result<(Self, Option<MusicDirArg>), clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        use clap::{CommandFactory, FromArgMatches, parser::ValueSource};
        let matches = Self::command().try_get_matches_from(args)?;
        let cli = Self::from_arg_matches(&matches)?;
        let source = match matches.value_source("music_dir") {
            _ if cli.music_dir.as_ref().is_none_or(|d| d.is_empty()) => None,
            Some(ValueSource::CommandLine) => Some(MusicDirArg::Argument),
            Some(ValueSource::EnvVariable) => Some(MusicDirArg::Env),
            _ => None,
        };
        Ok((cli, source))
    }
}

/// Environment inputs, read once so tests never touch the process
/// environment. No `Debug`: it may hold a password.
#[derive(Default, Clone)]
pub struct Env {
    pub config_path: Option<OsString>,
    pub xdg_config_home: Option<OsString>,
    pub home: Option<OsString>,
    pub navidrome_url: Option<OsString>,
    pub navidrome_user: Option<OsString>,
    pub navidrome_password: Option<OsString>,
}

impl Env {
    pub fn from_process() -> Self {
        Self {
            config_path: std::env::var_os(CONFIG_ENV),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME"),
            home: std::env::var_os("HOME"),
            navidrome_url: std::env::var_os(NAVIDROME_URL_ENV),
            navidrome_user: std::env::var_os(NAVIDROME_USER_ENV),
            navidrome_password: std::env::var_os(NAVIDROME_PASSWORD_ENV),
        }
    }
}

#[derive(Debug)]
pub struct Config {
    /// Canonicalized, absolute music directory, if one is configured.
    pub music_dir: Option<PathBuf>,
    pub navidrome: Option<NavidromeConfig>,
}

/// A complete Navidrome configuration. `Debug` hides every value.
pub struct NavidromeConfig {
    /// Validated base URL whose path ends in `/`.
    pub url: Url,
    pub username: String,
    pub password: PasswordSource,
    /// Report plays to the server (`scrobble`, default `true`).
    pub scrobble: bool,
}

impl fmt::Debug for NavidromeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NavidromeConfig(<redacted>)")
    }
}

/// Where the password comes from. `Debug` hides the password, argv and
/// account.
pub enum PasswordSource {
    Value(Password),
    /// argv, run without a shell by [`PasswordSource::resolve`].
    Command(Vec<String>),
    /// The OS credential store entry for this account (see
    /// [`secret_store::account`]). `config_path` names the file that asked
    /// for it, for error messages.
    Keychain {
        account: String,
        config_path: Option<PathBuf>,
    },
}

impl fmt::Debug for PasswordSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(_) => f.write_str("Value(<redacted>)"),
            Self::Command(_) => f.write_str("Command(<redacted>)"),
            Self::Keychain { .. } => f.write_str("Keychain(<redacted>)"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(
        "no music source configured.\n\nUsage: coda [MUSIC_DIR]\n\
         Or set {MUSIC_DIR_ENV}, for example: {MUSIC_DIR_ENV}=~/Music coda\n\
         Or set music_dir and/or a [navidrome] section in {}",
        .config_path.as_deref().map_or_else(|| "the config file".into(), |p| p.display().to_string())
    )]
    NoSource { config_path: Option<PathBuf> },
    #[error("music directory does not exist: {}", .0.display())]
    NotFound(PathBuf),
    #[error("music directory is not a directory: {}", .0.display())]
    NotADirectory(PathBuf),
    #[error("music directory is not readable: {}: {source}", .path.display())]
    Unreadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("config file not found: {} (from {CONFIG_ENV})", .0.display())]
    ConfigMissing(PathBuf),
    #[error("cannot read config file {}: {source}", .path.display())]
    ConfigUnreadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid config file {}: line {line}, column {column}: {message}", .path.display())]
    ConfigParse {
        path: PathBuf,
        line: usize,
        column: usize,
        message: String,
    },
    #[error(
        "{} contains a password but is readable by other users; run `chmod 600 {}`",
        .0.display(), .0.display()
    )]
    InsecurePermissions(PathBuf),
    #[error(
        "{} sets password_command but is writable by other users; run `chmod go-w {}`",
        .0.display(), .0.display()
    )]
    WritableCommandConfig(PathBuf),
    #[error("music_dir in {} must be absolute or start with ~/", .0.display())]
    RelativeMusicDir(PathBuf),
    #[error("{0} is not valid UTF-8")]
    NotUnicode(&'static str),
    #[error(
        "Navidrome is partly configured: {field} is missing \
         (set it in the [navidrome] section of the config file or with {env})"
    )]
    MissingNavidromeField {
        field: &'static str,
        env: &'static str,
    },
    #[error(
        "[navidrome] sets more than one of password, password_command and \
         password_keychain; keep one"
    )]
    ConflictingPassword,
    #[error("[navidrome] password_command must not be empty")]
    EmptyPasswordCommand,
    #[error("invalid Navidrome url: {0}")]
    InvalidUrl(#[from] UrlError),
    #[error("password_command failed: {0}")]
    PasswordCommand(String),
    #[error("{}", keychain_message(*.error, .config_path.as_deref()))]
    Keychain {
        error: SecretError,
        config_path: Option<PathBuf>,
    },
    #[error("no config file location is known; set {CONFIG_ENV}, XDG_CONFIG_HOME or HOME")]
    NoConfigLocation,
}

/// The message for a failed `password_keychain` lookup, with a remedy.
fn keychain_message(error: SecretError, config_path: Option<&Path>) -> String {
    let path = config_path.map_or_else(|| "the config file".into(), |p| p.display().to_string());
    match error {
        SecretError::Missing => format!(
            "the Navidrome password was not found in the {STORE_NAME}.\n\
             Run `coda --setup` to enter it again, or set {NAVIDROME_PASSWORD_ENV}."
        ),
        error => format!(
            "could not read the Navidrome password from the {STORE_NAME}: {error}.\n\
             Run `coda --setup` to set it up again, set {NAVIDROME_PASSWORD_ENV}, \
             or use password_command in {path} instead."
        ),
    }
}

impl ConfigError {
    /// Exit status: 2 for usage and configuration errors, 1 for an
    /// unusable directory or file, insecure permissions or a failing
    /// password command.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::NoSource { .. }
            | Self::ConfigMissing(_)
            | Self::ConfigParse { .. }
            | Self::RelativeMusicDir(_)
            | Self::NotUnicode(_)
            | Self::MissingNavidromeField { .. }
            | Self::ConflictingPassword
            | Self::EmptyPasswordCommand
            | Self::InvalidUrl(_)
            | Self::NoConfigLocation => 2,
            Self::NotFound(_)
            | Self::NotADirectory(_)
            | Self::Unreadable { .. }
            | Self::ConfigUnreadable { .. }
            | Self::InsecurePermissions(_)
            | Self::WritableCommandConfig(_)
            | Self::PasswordCommand(_)
            | Self::Keychain { .. } => 1,
        }
    }
}

/// The config file's contents. Unknown keys are errors (typos).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    music_dir: Option<String>,
    navidrome: Option<FileNavidrome>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileNavidrome {
    url: Option<String>,
    username: Option<String>,
    password: Option<FilePassword>,
    password_command: Option<Vec<String>>,
    password_keychain: Option<bool>,
    scrobble: Option<bool>,
}

/// A password string from the file. Its deserializer never echoes the
/// value in an error (serde's default "invalid type" message would show
/// e.g. an unquoted numeric password).
struct FilePassword(String);

impl fmt::Debug for FilePassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl<'de> Deserialize<'de> for FilePassword {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = FilePassword;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<FilePassword, E> {
                Ok(FilePassword(v.to_owned()))
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<FilePassword, E> {
                Err(E::custom("password must be a quoted string"))
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<FilePassword, E> {
                Err(E::custom("password must be a quoted string"))
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<FilePassword, E> {
                Err(E::custom("password must be a quoted string"))
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<FilePassword, E> {
                Err(E::custom("password must be a quoted string"))
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}

/// The config file location: `CODA_CONFIG` (explicit), else
/// `$XDG_CONFIG_HOME/coda/config.toml`, else `~/.config/coda/config.toml`.
/// Empty or relative XDG/HOME values are ignored. Returns the path and
/// whether it was given explicitly.
pub fn config_file_path(env: &Env) -> Option<(PathBuf, bool)> {
    if let Some(path) = env.config_path.as_ref().filter(|p| !p.is_empty()) {
        return Some((PathBuf::from(path), true));
    }
    let absolute = |v: &Option<OsString>| v.as_ref().map(PathBuf::from).filter(|p| p.is_absolute());
    let base = absolute(&env.xdg_config_home)
        .or_else(|| absolute(&env.home).map(|h| h.join(".config")))?;
    Some((base.join("coda").join("config.toml"), false))
}

impl Config {
    /// Merges the CLI, environment and config file, and validates the
    /// result. Does not run `password_command` (see
    /// [`PasswordSource::resolve`]).
    pub fn load(cli: Cli, env: &Env) -> Result<Self, ConfigError> {
        let location = config_file_path(env);
        let file = match &location {
            Some((path, explicit)) => read_config_file(path, *explicit)?.map(|(file, _)| file),
            None => None,
        };
        let config_path = location.map(|(path, _)| path);
        let file = file.unwrap_or_default();

        let navidrome = merge_navidrome(file.navidrome, env, config_path.as_deref())?;

        let cli_dir = cli.music_dir.filter(|p| !p.is_empty()).map(PathBuf::from);
        let file_dir = match file.music_dir.filter(|s| !s.is_empty()) {
            Some(dir) => Some(expand_home(&dir, env).ok_or_else(|| {
                ConfigError::RelativeMusicDir(config_path.clone().unwrap_or_default())
            })?),
            None => None,
        };
        let music_dir = match cli_dir.or(file_dir) {
            Some(dir) => Some(validate_music_dir(dir)?),
            None => None,
        };
        if music_dir.is_none() && navidrome.is_none() {
            return Err(ConfigError::NoSource { config_path });
        }
        Ok(Self {
            music_dir,
            navidrome,
        })
    }
}

/// What `coda --setup` starts from: the config file's values as written
/// (e.g. `~/Music` stays `~/Music`). `Debug` hides every value.
#[derive(Default)]
pub struct SetupStart {
    /// The file setup writes (it may not exist yet).
    pub config_path: PathBuf,
    pub music_dir: Option<String>,
    pub url: Option<String>,
    pub username: Option<String>,
    /// `password` or `password_command`, if the file names one: a
    /// password set outside Coda, which saving replaces.
    pub hand_written_password: Option<&'static str>,
    /// The keychain entry Coda wrote for this file: set when the file has
    /// `password_keychain = true` and a usable url and username.
    pub keychain_account: Option<String>,
    /// `true` if `[navidrome]` configures anything besides `scrobble`.
    pub has_navidrome: bool,
    /// The file's URL held `user[:password]@`, which `url` leaves out.
    pub url_had_credentials: bool,
    /// The file's URL does not parse and contains `@`, so it may hold
    /// credentials that cannot be separated reliably: `url` is `None`.
    pub url_withheld: bool,
    /// The file holds more than whitespace and comments, so saving backs
    /// it up first (the config writer's rule).
    pub has_content: bool,
}

impl fmt::Debug for SetupStart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SetupStart(<redacted>)")
    }
}

/// Reads the config file for `coda --setup`. Like [`Config::load`], a file
/// that cannot be read or parsed, or has unsafe permissions for the secret
/// it holds, is an error (setup does not repair broken files). Everything
/// else is taken as it is, to be corrected in setup: a partial
/// `[navidrome]`, an invalid URL, a folder that no longer exists, or more
/// than one password key. Nothing is resolved: no keychain read, no
/// `password_command`.
pub fn read_for_setup(env: &Env) -> Result<SetupStart, ConfigError> {
    let (config_path, explicit) = config_file_path(env).ok_or(ConfigError::NoConfigLocation)?;
    let (file, has_content) = match read_config_file(&config_path, explicit)? {
        Some((file, text)) => (file, crate::config_writer::has_content(&text)),
        None => (FileConfig::default(), false),
    };
    let non_empty = |s: Option<String>| s.filter(|s| !s.is_empty());
    let navidrome = file.navidrome.unwrap_or_default();
    let hand_written_password = if navidrome.password.is_some() {
        Some("password")
    } else if navidrome.password_command.is_some() {
        Some("password_command")
    } else {
        None
    };
    let keychain = navidrome.password_keychain == Some(true);
    let has_navidrome = navidrome.url.is_some()
        || navidrome.username.is_some()
        || hand_written_password.is_some()
        || keychain;
    let raw_url = non_empty(navidrome.url);
    let username = non_empty(navidrome.username);
    // Never prefill (and so show) credentials embedded in the URL.
    let prefill = raw_url.as_deref().map(prefill_url);
    let url_had_credentials = matches!(prefill, Some(UrlPrefill::Stripped(_)));
    let url_withheld = matches!(prefill, Some(UrlPrefill::Withheld));
    // From the URL as written, as `Config::load` would.
    let keychain_account = match (&raw_url, &username) {
        (Some(url), Some(username)) if keychain => parse_base_url(url)
            .ok()
            .map(|url| secret_store::account(&url, username)),
        _ => None,
    };
    let url = match prefill {
        Some(UrlPrefill::AsWritten) => raw_url,
        Some(UrlPrefill::Stripped(url)) => Some(url),
        Some(UrlPrefill::Withheld) | None => None,
    };
    Ok(SetupStart {
        config_path,
        music_dir: non_empty(file.music_dir),
        url,
        username,
        hand_written_password,
        keychain_account,
        has_navidrome,
        url_had_credentials,
        url_withheld,
        has_content,
    })
}

/// How a URL from the file may be prefilled.
#[derive(Debug, PartialEq, Eq)]
enum UrlPrefill {
    /// No credentials: shown as written.
    AsWritten,
    /// Parsed, with its username and password removed.
    Stripped(String),
    /// Does not parse (as a URL with a host) and contains `@`: it may hold
    /// credentials that text rules would cut wrongly, so it is not shown.
    Withheld,
}

/// Coda's own files never hold credentials in the URL (`parse_base_url`
/// rejects them); a hand-written one might.
fn prefill_url(raw: &str) -> UrlPrefill {
    match Url::parse(raw) {
        Ok(mut url) if url.has_host() => {
            if url.username().is_empty() && url.password().is_none() {
                return UrlPrefill::AsWritten;
            }
            // Cannot fail for a URL with a host.
            let cleared = url.set_username("").is_ok() && url.set_password(None).is_ok();
            if cleared {
                UrlPrefill::Stripped(url.into())
            } else {
                UrlPrefill::Withheld
            }
        }
        _ if raw.contains('@') => UrlPrefill::Withheld,
        _ => UrlPrefill::AsWritten,
    }
}

/// The `CODA_NAVIDROME_*` variables that are set (non-empty) and so
/// override the file's `[navidrome]` values. Names only.
pub fn navidrome_env_overrides(env: &Env) -> Vec<&'static str> {
    let set = |v: &Option<OsString>| v.as_ref().is_some_and(|v| !v.is_empty());
    [
        (&env.navidrome_url, NAVIDROME_URL_ENV),
        (&env.navidrome_user, NAVIDROME_USER_ENV),
        (&env.navidrome_password, NAVIDROME_PASSWORD_ENV),
    ]
    .into_iter()
    .filter_map(|(value, name)| set(value).then_some(name))
    .collect()
}

/// Reads and parses the file. `None` if an implicit (default) path does
/// not exist. The permission check uses the opened file's metadata.
/// Also returns the text, for [`read_for_setup`].
fn read_config_file(
    path: &Path,
    explicit: bool,
) -> Result<Option<(FileConfig, String)>, ConfigError> {
    let unreadable = |source| ConfigError::ConfigUnreadable {
        path: path.to_owned(),
        source,
    };
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound && !explicit => return Ok(None),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(ConfigError::ConfigMissing(path.to_owned()));
        }
        Err(e) => return Err(unreadable(e)),
    };
    let mode = file.metadata().map_err(unreadable)?.permissions().mode();
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(unreadable)?;
    let config: FileConfig = toml::from_str(&text).map_err(|e| {
        let (line, column) = e
            .span()
            .map_or((1, 1), |span| line_column(&text, span.start));
        ConfigError::ConfigParse {
            path: path.to_owned(),
            line,
            column,
            message: e.message().trim().to_owned(),
        }
    })?;
    let navidrome = config.navidrome.as_ref();
    if navidrome.is_some_and(|n| n.password.is_some()) && mode & 0o077 != 0 {
        return Err(ConfigError::InsecurePermissions(path.to_owned()));
    }
    // Anyone who can edit the file could make Coda run their command.
    if navidrome.is_some_and(|n| n.password_command.is_some()) && mode & 0o022 != 0 {
        return Err(ConfigError::WritableCommandConfig(path.to_owned()));
    }
    Ok(Some((config, text)))
}

/// 1-based line and column (in chars) of byte offset `at`.
fn line_column(text: &str, at: usize) -> (usize, usize) {
    let before = text.get(..at).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

/// Expands a leading `~` or `~/` with `$HOME`. `None` if the result is not
/// absolute.
fn expand_home(dir: &str, env: &Env) -> Option<PathBuf> {
    let home = || {
        env.home
            .as_ref()
            .map(PathBuf::from)
            .filter(|h| h.is_absolute())
    };
    let path = if dir == "~" {
        home()?
    } else if let Some(rest) = dir.strip_prefix("~/") {
        home()?.join(rest)
    } else {
        PathBuf::from(dir)
    };
    path.is_absolute().then_some(path)
}

fn env_string(value: &Option<OsString>, name: &'static str) -> Result<Option<String>, ConfigError> {
    match value {
        None => Ok(None),
        Some(v) if v.is_empty() => Ok(None),
        Some(v) => v
            .to_str()
            .map(|s| Some(s.to_owned()))
            .ok_or(ConfigError::NotUnicode(name)),
    }
}

/// The password source before the URL is known.
enum PasswordChoice {
    Ready(PasswordSource),
    Keychain,
}

/// Env values (non-empty) override file values. Returns `None` when
/// nothing Navidrome-related is set anywhere.
fn merge_navidrome(
    file: Option<FileNavidrome>,
    env: &Env,
    config_path: Option<&Path>,
) -> Result<Option<NavidromeConfig>, ConfigError> {
    let file = file.unwrap_or_default();
    let keychain = file.password_keychain == Some(true);
    let sources = usize::from(file.password.is_some())
        + usize::from(file.password_command.is_some())
        + usize::from(keychain);
    if sources > 1 {
        return Err(ConfigError::ConflictingPassword);
    }
    let scrobble = file.scrobble.unwrap_or(true);
    let non_empty = |s: Option<String>| s.filter(|s| !s.is_empty());
    let url = env_string(&env.navidrome_url, NAVIDROME_URL_ENV)?.or(non_empty(file.url));
    let username =
        env_string(&env.navidrome_user, NAVIDROME_USER_ENV)?.or(non_empty(file.username));
    let password = match env_string(&env.navidrome_password, NAVIDROME_PASSWORD_ENV)? {
        Some(password) => Some(PasswordChoice::Ready(PasswordSource::Value(Password::new(
            password,
        )))),
        None => match (file.password, file.password_command) {
            (Some(FilePassword(p)), _) if !p.is_empty() => Some(PasswordChoice::Ready(
                PasswordSource::Value(Password::new(p)),
            )),
            (_, Some(argv)) if argv.is_empty() || argv[0].is_empty() => {
                return Err(ConfigError::EmptyPasswordCommand);
            }
            (_, Some(argv)) => Some(PasswordChoice::Ready(PasswordSource::Command(argv))),
            _ if keychain => Some(PasswordChoice::Keychain),
            _ => None,
        },
    };
    let (url, username, password) = match (url, username, password) {
        (None, None, None) => return Ok(None),
        (Some(url), Some(username), Some(password)) => (url, username, password),
        (url, username, _) => {
            let (field, env) = if url.is_none() {
                ("url", NAVIDROME_URL_ENV)
            } else if username.is_none() {
                ("username", NAVIDROME_USER_ENV)
            } else {
                (
                    "password (or password_command or password_keychain)",
                    NAVIDROME_PASSWORD_ENV,
                )
            };
            return Err(ConfigError::MissingNavidromeField { field, env });
        }
    };
    let url = parse_base_url(&url)?;
    let password = match password {
        PasswordChoice::Ready(source) => source,
        PasswordChoice::Keychain => PasswordSource::Keychain {
            account: secret_store::account(&url, &username),
            config_path: config_path.map(Path::to_owned),
        },
    };
    Ok(Some(NavidromeConfig {
        url,
        username,
        password,
        scrobble,
    }))
}

/// Expands a typed music directory (`~` or `~/…` with `$HOME`). `None` if
/// the result is not absolute.
pub fn expand_music_dir(typed: &str, env: &Env) -> Option<PathBuf> {
    expand_home(typed, env)
}

/// Validates the directory: it must exist, be a directory and be
/// listable. Returns the canonical path.
pub fn validate_music_dir(path: PathBuf) -> Result<PathBuf, ConfigError> {
    let unreadable = |source| ConfigError::Unreadable {
        path: path.clone(),
        source,
    };
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(ConfigError::NotFound(path));
        }
        Err(e) => return Err(unreadable(e)),
    };
    if !metadata.is_dir() {
        return Err(ConfigError::NotADirectory(path));
    }
    std::fs::read_dir(&path).map_err(unreadable)?;
    std::fs::canonicalize(&path).map_err(unreadable)
}

impl PasswordSource {
    /// The password, running `password_command` (see
    /// [`run_password_command`]) or reading the OS credential store if
    /// needed.
    pub async fn resolve(self, timeout: Duration) -> Result<Password, ConfigError> {
        self.resolve_with(timeout, SecretStore::system).await
    }

    /// [`PasswordSource::resolve`] with the credential store from
    /// `open_store`. The store is opened and read on a blocking thread;
    /// the read may wait for an OS prompt, so it has no timeout.
    pub async fn resolve_with(
        self,
        timeout: Duration,
        open_store: impl FnOnce() -> Result<SecretStore, SecretError> + Send + 'static,
    ) -> Result<Password, ConfigError> {
        match self {
            Self::Value(password) => Ok(password),
            Self::Command(argv) => run_password_command(&argv, timeout).await,
            Self::Keychain {
                account,
                config_path,
            } => {
                let read = tokio::task::spawn_blocking(move || open_store()?.get(&account));
                let error = match read.await {
                    Ok(Ok(password)) => return Ok(password),
                    Ok(Err(error)) => error,
                    // The read panicked; its message is not shown.
                    Err(_) => SecretError::Failed,
                };
                tracing::warn!(kind = error.kind(), "could not read the keychain");
                Err(ConfigError::Keychain { error, config_path })
            }
        }
    }
}

/// Runs `argv` directly (no shell) with stdin and stderr inherited, so
/// pinentry/gpg prompts work, and stdout captured. Killed after `timeout`.
/// A non-zero exit, non-UTF-8 or empty output is an error. One trailing
/// newline (`\n` or `\r\n`) is stripped. The output is never logged.
pub async fn run_password_command(
    argv: &[String],
    timeout: Duration,
) -> Result<Password, ConfigError> {
    let (program, args) = argv
        .split_first()
        .ok_or(ConfigError::EmptyPasswordCommand)?;
    let fail = |reason: String| ConfigError::PasswordCommand(format!("`{program}` {reason}"));
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ConfigError::PasswordCommand(format!("could not run `{program}`: {e}")))?;
    let Some(stdout) = child.stdout.take() else {
        return Err(fail("has no stdout".to_owned()));
    };
    let run = async {
        let mut output = Vec::new();
        stdout
            .take(PASSWORD_COMMAND_MAX_OUTPUT as u64 + 1)
            .read_to_end(&mut output)
            .await
            .map_err(|e| fail(format!("failed: {e}")))?;
        if output.len() > PASSWORD_COMMAND_MAX_OUTPUT {
            return Err(fail(format!(
                "printed more than {} KiB",
                PASSWORD_COMMAND_MAX_OUTPUT / 1024
            )));
        }
        let status = child
            .wait()
            .await
            .map_err(|e| fail(format!("failed: {e}")))?;
        Ok((status, output))
    };
    let result = match tokio::time::timeout(timeout, run).await {
        Ok(result) => result,
        Err(_) => Err(fail(format!(
            "did not finish within {} s",
            timeout.as_secs_f64()
        ))),
    };
    let (status, output) = match result {
        Ok(done) => done,
        Err(error) => {
            // Timeout or overflow: kill and reap before reporting.
            if let Err(e) = child.start_kill() {
                tracing::debug!(error = %e, "kill password_command");
            }
            let _ = child.wait().await;
            return Err(error);
        }
    };
    if !status.success() {
        return Err(fail(format!("exited with {status}")));
    }
    let mut password = String::from_utf8(output)
        .map_err(|_| ConfigError::PasswordCommand(format!("`{program}` printed invalid UTF-8")))?;
    if password.ends_with('\n') {
        password.pop();
        if password.ends_with('\r') {
            password.pop();
        }
    }
    if password.is_empty() {
        return Err(ConfigError::PasswordCommand(format!(
            "`{program}` printed nothing"
        )));
    }
    Ok(Password::new(password))
}

/// `$XDG_STATE_HOME/coda`, else `~/.local/state/coda`. Relative or empty
/// values are ignored; `None` if no location is known.
pub fn state_dir(xdg_state_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let absolute = |v: Option<OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    let state = absolute(xdg_state_home)
        .or_else(|| absolute(home).map(|h| h.join(".local").join("state")))?;
    Some(state.join("coda"))
}

/// `coda.log` in [`state_dir`].
pub fn log_file_path(xdg_state_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    Some(state_dir(xdg_state_home, home)?.join("coda.log"))
}

/// `state.json` (the saved session) in [`state_dir`].
pub fn session_file_path(
    xdg_state_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    Some(state_dir(xdg_state_home, home)?.join("state.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn cli(dir: Option<&Path>) -> Cli {
        Cli {
            music_dir: dir.map(|d| d.as_os_str().to_owned()),
            setup: false,
        }
    }

    /// An environment whose config file lives in a fresh temp dir.
    struct Fixture {
        dir: tempfile::TempDir,
        env: Env,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let env = Env {
                xdg_config_home: Some(dir.path().join("xdg").into()),
                home: Some(dir.path().join("home").into()),
                ..Env::default()
            };
            Self { dir, env }
        }

        fn config_path(&self) -> PathBuf {
            self.dir.path().join("xdg").join("coda").join("config.toml")
        }

        fn write(&self, text: &str, mode: u32) -> PathBuf {
            let path = self.config_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let _ = std::fs::remove_file(&path);
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            path
        }

        fn music(&self, name: &str) -> PathBuf {
            let path = self.dir.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::canonicalize(path).unwrap()
        }

        fn load(&self, cli: Cli) -> Result<Config, ConfigError> {
            Config::load(cli, &self.env)
        }
    }

    const NAVIDROME: &str = "[navidrome]\nurl = \"https://music.example.org/nd\"\n\
                             username = \"me\"\npassword = \"hunter2\"\n";

    fn expose(source: &PasswordSource) -> &str {
        match source {
            PasswordSource::Value(p) => p.expose(),
            PasswordSource::Command(_) | PasswordSource::Keychain { .. } => {
                panic!("expected a value")
            }
        }
    }

    #[test]
    fn no_source_is_a_usage_error() {
        let f = Fixture::new();
        let err = f.load(cli(None)).unwrap_err();
        assert!(matches!(err, ConfigError::NoSource { .. }), "{err:?}");
        assert_eq!(err.exit_code(), 2);
        let text = err.to_string();
        assert!(text.contains("CODA_MUSIC_DIR"), "{text}");
        assert!(
            text.contains(&f.config_path().display().to_string()),
            "{text}"
        );
        // Empty values count as unset.
        let err = f.load(cli(Some(Path::new("")))).unwrap_err();
        assert!(matches!(err, ConfigError::NoSource { .. }));
    }

    #[test]
    fn music_dir_precedence_cli_over_file() {
        let f = Fixture::new();
        let from_cli = f.music("cli");
        let from_file = f.music("file");
        f.write(
            &format!("music_dir = {:?}\n", from_file.to_str().unwrap()),
            0o644,
        );
        let config = f.load(cli(Some(&from_cli))).unwrap();
        assert_eq!(config.music_dir, Some(from_cli));
        let config = f.load(cli(None)).unwrap();
        assert_eq!(config.music_dir, Some(from_file));
        assert!(config.navidrome.is_none());
    }

    #[test]
    fn env_music_dir_wins_over_file_via_clap() {
        // clap fills the argument from CODA_MUSIC_DIR; check the wiring.
        let cmd = <Cli as clap::CommandFactory>::command();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_id() == "music_dir")
            .unwrap();
        assert_eq!(arg.get_env(), Some(std::ffi::OsStr::new(MUSIC_DIR_ENV)));
    }

    #[test]
    fn music_dir_tilde_expansion_and_relative_rejection() {
        let f = Fixture::new();
        let home = f.dir.path().join("home");
        std::fs::create_dir_all(home.join("Music")).unwrap();
        f.write("music_dir = \"~/Music\"\n", 0o644);
        let config = f.load(cli(None)).unwrap();
        assert_eq!(
            config.music_dir,
            Some(std::fs::canonicalize(home.join("Music")).unwrap())
        );
        f.write("music_dir = \"Music\"\n", 0o644);
        let err = f.load(cli(None)).unwrap_err();
        assert!(matches!(err, ConfigError::RelativeMusicDir(_)), "{err:?}");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn navidrome_only_is_enough() {
        let f = Fixture::new();
        f.write(NAVIDROME, 0o600);
        let config = f.load(cli(None)).unwrap();
        assert_eq!(config.music_dir, None);
        let nd = config.navidrome.unwrap();
        assert_eq!(nd.url.as_str(), "https://music.example.org/nd/");
        assert_eq!(nd.username, "me");
        assert_eq!(expose(&nd.password), "hunter2");
        assert!(!format!("{nd:?}").contains("hunter2"));
        assert!(!format!("{nd:?}").contains("example"));
    }

    #[test]
    fn scrobbling_defaults_to_on_and_can_be_turned_off() {
        let f = Fixture::new();
        f.write(NAVIDROME, 0o600);
        assert!(f.load(cli(None)).unwrap().navidrome.unwrap().scrobble);
        f.write(&format!("{NAVIDROME}scrobble = false\n"), 0o600);
        assert!(!f.load(cli(None)).unwrap().navidrome.unwrap().scrobble);
        f.write(&format!("{NAVIDROME}scrobble = true\n"), 0o600);
        assert!(f.load(cli(None)).unwrap().navidrome.unwrap().scrobble);
        // Not a boolean: a parse error naming the line.
        f.write(&format!("{NAVIDROME}scrobble = \"no\"\n"), 0o600);
        let err = f.load(cli(None)).unwrap_err();
        assert!(
            matches!(err, ConfigError::ConfigParse { line: 5, .. }),
            "{err:?}"
        );
        // On its own it does not configure Navidrome.
        let music = f.music("music");
        f.write("[navidrome]\nscrobble = false\n", 0o600);
        let config = f.load(cli(Some(&music))).unwrap();
        assert!(config.navidrome.is_none());
    }

    #[test]
    fn env_overrides_file_values() {
        let mut f = Fixture::new();
        f.write(NAVIDROME, 0o600);
        f.env.navidrome_url = Some("http://localhost:4533".into());
        f.env.navidrome_user = Some("other".into());
        f.env.navidrome_password = Some("from-env".into());
        let nd = f.load(cli(None)).unwrap().navidrome.unwrap();
        assert_eq!(nd.url.as_str(), "http://localhost:4533/");
        assert_eq!(nd.username, "other");
        assert_eq!(expose(&nd.password), "from-env");
    }

    #[test]
    fn env_only_navidrome_without_file() {
        let mut f = Fixture::new();
        f.env.navidrome_url = Some("https://h".into());
        f.env.navidrome_user = Some("u".into());
        f.env.navidrome_password = Some("p".into());
        let config = f.load(cli(None)).unwrap();
        assert!(config.navidrome.is_some());
        // An empty env value counts as unset.
        f.env.navidrome_password = Some("".into());
        let err = f.load(cli(None)).unwrap_err();
        assert!(
            matches!(err, ConfigError::MissingNavidromeField { field, .. } if field.starts_with("password")),
            "{err:?}"
        );
    }

    #[test]
    fn env_password_overrides_password_command() {
        let mut f = Fixture::new();
        f.write(
            "[navidrome]\nurl = \"https://h\"\nusername = \"u\"\n\
             password_command = [\"false\"]\n",
            0o644,
        );
        f.env.navidrome_password = Some("from-env".into());
        let nd = f.load(cli(None)).unwrap().navidrome.unwrap();
        assert_eq!(expose(&nd.password), "from-env");
    }

    #[test]
    fn partial_navidrome_names_the_missing_field_not_values() {
        let f = Fixture::new();
        for (text, missing) in [
            ("[navidrome]\nusername = \"secret-user\"\n", "url"),
            ("[navidrome]\nurl = \"https://secret-host\"\n", "username"),
            (
                "[navidrome]\nurl = \"https://secret-host\"\nusername = \"secret-user\"\n",
                "password",
            ),
        ] {
            f.write(text, 0o600);
            let err = f.load(cli(None)).unwrap_err();
            assert_eq!(err.exit_code(), 2);
            let text = err.to_string();
            assert!(text.contains(missing), "{text}");
            assert!(!text.contains("secret"), "{text}");
        }
    }

    #[test]
    fn invalid_url_is_a_config_error() {
        let f = Fixture::new();
        f.write(
            "[navidrome]\nurl = \"ftp://h\"\nusername = \"u\"\npassword = \"p\"\n",
            0o600,
        );
        let err = f.load(cli(None)).unwrap_err();
        assert!(
            matches!(err, ConfigError::InvalidUrl(UrlError::Scheme)),
            "{err:?}"
        );
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn unknown_keys_and_syntax_errors_report_line() {
        let f = Fixture::new();
        f.write(
            "music_dir = \"/x\"\n[navidrome]\npasword = \"hunter2\"\n",
            0o600,
        );
        let err = f.load(cli(None)).unwrap_err();
        let ConfigError::ConfigParse { line, message, .. } = &err else {
            panic!("{err:?}");
        };
        assert_eq!(*line, 3);
        assert!(message.contains("pasword"), "{message}");
        assert_eq!(err.exit_code(), 2);
        assert!(!err.to_string().contains("hunter2"), "{err}");

        f.write("music_dir = \n", 0o644);
        let err = f.load(cli(None)).unwrap_err();
        assert!(
            matches!(err, ConfigError::ConfigParse { line: 1, .. }),
            "{err:?}"
        );
        assert!(
            err.to_string()
                .contains(&f.config_path().display().to_string())
        );
    }

    #[test]
    fn unquoted_password_is_not_echoed() {
        let f = Fixture::new();
        f.write(
            "[navidrome]\nurl = \"https://h\"\nusername = \"u\"\npassword = 987654321\n",
            0o600,
        );
        let err = f.load(cli(None)).unwrap_err();
        assert!(
            matches!(err, ConfigError::ConfigParse { line: 4, .. }),
            "{err:?}"
        );
        assert!(!err.to_string().contains("987654321"), "{err}");
    }

    #[test]
    fn plaintext_password_needs_a_private_file() {
        let f = Fixture::new();
        for mode in [0o640, 0o604, 0o660, 0o644] {
            f.write(NAVIDROME, mode);
            let err = f.load(cli(None)).unwrap_err();
            assert!(
                matches!(err, ConfigError::InsecurePermissions(_)),
                "{mode:o}"
            );
            assert_eq!(err.exit_code(), 1);
            assert!(err.to_string().contains("chmod 600"));
        }
        f.write(NAVIDROME, 0o600);
        assert!(f.load(cli(None)).is_ok());
        f.write(NAVIDROME, 0o400);
        assert!(f.load(cli(None)).is_ok());
        // Without a password the mode does not matter.
        f.write(
            "[navidrome]\nurl = \"https://h\"\nusername = \"u\"\n\
             password_command = [\"true\"]\n",
            0o644,
        );
        assert!(f.load(cli(None)).is_ok());
    }

    #[test]
    fn password_and_command_conflict() {
        let f = Fixture::new();
        f.write(
            "[navidrome]\nurl = \"https://h\"\nusername = \"u\"\npassword = \"p\"\n\
             password_command = [\"x\"]\n",
            0o600,
        );
        let err = f.load(cli(None)).unwrap_err();
        assert!(matches!(err, ConfigError::ConflictingPassword), "{err:?}");
        f.write(
            "[navidrome]\nurl = \"https://h\"\nusername = \"u\"\npassword_command = []\n",
            0o600,
        );
        let err = f.load(cli(None)).unwrap_err();
        assert!(matches!(err, ConfigError::EmptyPasswordCommand), "{err:?}");
    }

    #[test]
    fn explicit_config_path() {
        let mut f = Fixture::new();
        let path = f.dir.path().join("custom.toml");
        f.env.config_path = Some(path.clone().into());
        let err = f.load(cli(None)).unwrap_err();
        assert!(matches!(err, ConfigError::ConfigMissing(_)), "{err:?}");
        assert_eq!(err.exit_code(), 2);
        let music = f.music("m");
        std::fs::write(
            &path,
            format!("music_dir = {:?}\n", music.to_str().unwrap()),
        )
        .unwrap();
        assert_eq!(f.load(cli(None)).unwrap().music_dir, Some(music));
    }

    #[test]
    fn config_path_resolution() {
        let env = |config: Option<&str>, xdg: Option<&str>, home: Option<&str>| Env {
            config_path: config.map(Into::into),
            xdg_config_home: xdg.map(Into::into),
            home: home.map(Into::into),
            ..Env::default()
        };
        assert_eq!(
            config_file_path(&env(Some("/c.toml"), Some("/x"), Some("/h"))),
            Some((PathBuf::from("/c.toml"), true))
        );
        assert_eq!(
            config_file_path(&env(Some(""), Some("/x"), Some("/h"))),
            Some((PathBuf::from("/x/coda/config.toml"), false))
        );
        assert_eq!(
            config_file_path(&env(None, Some("rel"), Some("/h"))),
            Some((PathBuf::from("/h/.config/coda/config.toml"), false))
        );
        assert_eq!(config_file_path(&env(None, None, None)), None);
    }

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_owned()).collect()
    }

    #[tokio::test]
    async fn password_command_success_strips_one_newline() {
        let run = |s: &'static str| async move {
            run_password_command(&argv(&["printf", s]), Duration::from_secs(5))
                .await
                .unwrap()
        };
        assert_eq!(run("pw\n").await.expose(), "pw");
        assert_eq!(run("pw\r\n").await.expose(), "pw");
        assert_eq!(run("pw\n\n").await.expose(), "pw\n");
        assert_eq!(run(" a b ").await.expose(), " a b ");
    }

    #[tokio::test]
    async fn password_command_failures() {
        let run = |args: &[&str]| {
            let args = argv(args);
            async move { run_password_command(&args, Duration::from_secs(5)).await }
        };
        let err = run(&["sh", "-c", "printf secret; exit 3"])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("exited"), "{err}");
        assert!(!err.to_string().contains("secret"), "{err}");
        assert_eq!(err.exit_code(), 1);
        let err = run(&["true"]).await.unwrap_err();
        assert!(err.to_string().contains("printed nothing"), "{err}");
        let err = run(&["printf", "\\n"]).await.unwrap_err();
        assert!(err.to_string().contains("printed nothing"), "{err}");
        let err = run(&["/nonexistent/coda-test-program"]).await.unwrap_err();
        assert!(err.to_string().contains("could not run"), "{err}");
        // No shell: metacharacters are plain arguments.
        let out = run(&["printf", "%s", "$(echo hi); `x`"]).await.unwrap();
        assert_eq!(out.expose(), "$(echo hi); `x`");
    }

    /// Runs `script` under `sh` with `$0` set to a pid file; the script
    /// must `exec` so the recorded pid is the child's own.
    async fn run_recording_pid(
        script: &str,
        timeout: Duration,
    ) -> (Result<Password, ConfigError>, u32) {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let pid_arg = pid_file.to_str().unwrap();
        let result = run_password_command(&argv(&["sh", "-c", script, pid_arg]), timeout).await;
        let pid = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        (result, pid)
    }

    /// `true` if `pid` exists (including as a zombie).
    fn process_exists(pid: u32) -> bool {
        let out = std::process::Command::new("ps")
            .args(["-o", "pid=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        !String::from_utf8_lossy(&out.stdout).trim().is_empty()
    }

    #[tokio::test]
    async fn password_command_timeout_kills_and_reaps_the_child() {
        let started = std::time::Instant::now();
        // Long enough that `sh` has written its pid before the kill, even
        // on a loaded machine (with 300 ms it sometimes had not).
        let (result, pid) =
            run_recording_pid("echo $$ > \"$0\"; exec sleep 30", Duration::from_secs(2)).await;
        let err = result.unwrap_err();
        assert!(err.to_string().contains("did not finish"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));
        // Killed and reaped before returning: not even a zombie remains.
        assert!(
            !process_exists(pid),
            "password_command (pid {pid}) survived"
        );
    }

    #[tokio::test]
    async fn password_command_output_is_bounded() {
        // Endless output: stops at the cap, then the child is killed.
        let (result, pid) =
            run_recording_pid("echo $$ > \"$0\"; exec yes", Duration::from_secs(10)).await;
        let err = result.unwrap_err();
        assert!(err.to_string().contains("more than 64 KiB"), "{err}");
        assert!(
            !process_exists(pid),
            "password_command (pid {pid}) survived"
        );
        // Exactly at the cap is accepted.
        let script = format!("head -c {PASSWORD_COMMAND_MAX_OUTPUT} /dev/zero | tr '\\0' x");
        let password = run_password_command(&argv(&["sh", "-c", &script]), Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(password.expose().len(), PASSWORD_COMMAND_MAX_OUTPUT);
    }

    #[test]
    fn password_command_needs_a_file_others_cannot_write() {
        let f = Fixture::new();
        let text = "[navidrome]\nurl = \"https://h\"\nusername = \"u\"\n\
                    password_command = [\"pass\", \"show\", \"nd\"]\n";
        for mode in [0o664, 0o646, 0o666] {
            f.write(text, mode);
            let err = f.load(cli(None)).unwrap_err();
            assert!(
                matches!(err, ConfigError::WritableCommandConfig(_)),
                "{mode:o}: {err:?}"
            );
            assert_eq!(err.exit_code(), 1);
            assert!(err.to_string().contains("chmod go-w"), "{err}");
        }
        for mode in [0o644, 0o600, 0o444] {
            f.write(text, mode);
            assert!(f.load(cli(None)).is_ok(), "{mode:o}");
        }
        // Without password_command a group-writable file is accepted.
        f.write("music_dir = \"/\"\n", 0o664);
        assert!(f.load(cli(None)).is_ok());
    }

    #[tokio::test]
    async fn password_source_resolution() {
        let value = PasswordSource::Value(Password::new("p".into()));
        assert_eq!(format!("{value:?}"), "Value(<redacted>)");
        assert_eq!(
            value
                .resolve(Duration::from_secs(1))
                .await
                .unwrap()
                .expose(),
            "p"
        );
        let command = PasswordSource::Command(argv(&["printf", "q"]));
        assert_eq!(format!("{command:?}"), "Command(<redacted>)");
        assert_eq!(
            command
                .resolve(Duration::from_secs(5))
                .await
                .unwrap()
                .expose(),
            "q"
        );
    }

    const KEYCHAIN: &str = "[navidrome]\nurl = \"https://music.example.org:8443/nd\"\n\
                            username = \"me\"\npassword_keychain = true\n";

    fn keychain_account(source: &PasswordSource) -> &str {
        match source {
            PasswordSource::Keychain { account, .. } => account,
            other => panic!("expected the keychain, got {other:?}"),
        }
    }

    #[test]
    fn password_keychain_parses_into_an_account() {
        let f = Fixture::new();
        // No secret in the file: any mode is fine.
        f.write(KEYCHAIN, 0o644);
        let nd = f.load(cli(None)).unwrap().navidrome.unwrap();
        assert_eq!(keychain_account(&nd.password), "me@music.example.org:8443");
        assert_eq!(format!("{:?}", nd.password), "Keychain(<redacted>)");
        // Env values change the account the entry is looked up under.
        let mut f = f;
        f.env.navidrome_user = Some("other".into());
        let nd = f.load(cli(None)).unwrap().navidrome.unwrap();
        assert_eq!(
            keychain_account(&nd.password),
            "other@music.example.org:8443"
        );
        // `false` is the same as leaving it out.
        f.env.navidrome_user = None;
        f.write(&KEYCHAIN.replace("true", "false"), 0o600);
        let err = f.load(cli(None)).unwrap_err();
        assert!(
            matches!(err, ConfigError::MissingNavidromeField { field, .. } if field.contains("password_keychain")),
            "{err:?}"
        );
        // Not a boolean: a parse error.
        f.write(&KEYCHAIN.replace("true", "\"yes\""), 0o600);
        let err = f.load(cli(None)).unwrap_err();
        assert!(
            matches!(err, ConfigError::ConfigParse { line: 4, .. }),
            "{err:?}"
        );
    }

    #[test]
    fn at_most_one_password_source_in_the_file() {
        let f = Fixture::new();
        for extra in ["password = \"p\"\n", "password_command = [\"pass\"]\n"] {
            f.write(&format!("{KEYCHAIN}{extra}"), 0o600);
            let err = f.load(cli(None)).unwrap_err();
            assert!(matches!(err, ConfigError::ConflictingPassword), "{err:?}");
            assert_eq!(err.exit_code(), 2);
            assert!(err.to_string().contains("password_keychain"), "{err}");
        }
        f.write(
            &format!("{KEYCHAIN}password = \"p\"\npassword_command = [\"x\"]\n"),
            0o600,
        );
        assert!(matches!(
            f.load(cli(None)).unwrap_err(),
            ConfigError::ConflictingPassword
        ));
        // The env password overrides the keychain, but not a conflict.
        let mut f = Fixture::new();
        f.env.navidrome_password = Some("from-env".into());
        f.write(KEYCHAIN, 0o600);
        let nd = f.load(cli(None)).unwrap().navidrome.unwrap();
        assert_eq!(expose(&nd.password), "from-env");
        f.write(&format!("{KEYCHAIN}password = \"p\"\n"), 0o600);
        assert!(matches!(
            f.load(cli(None)).unwrap_err(),
            ConfigError::ConflictingPassword
        ));
    }

    #[tokio::test]
    async fn keychain_resolution_through_the_mock_store() {
        let f = Fixture::new();
        f.write(KEYCHAIN, 0o600);
        let store = SecretStore::mock();
        let account = "me@music.example.org:8443";
        store
            .set(account, &Password::new("from-keychain".into()))
            .unwrap();
        let source = f.load(cli(None)).unwrap().navidrome.unwrap().password;
        let opened = store.clone();
        let password = source
            .resolve_with(Duration::from_secs(1), move || Ok(opened))
            .await
            .unwrap();
        assert_eq!(password.expose(), "from-keychain");

        // Missing entry: names the remedy and the file, not the account.
        store.delete(account).unwrap();
        let source = f.load(cli(None)).unwrap().navidrome.unwrap().password;
        let opened = store.clone();
        let err = source
            .resolve_with(Duration::from_secs(1), move || Ok(opened))
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::Keychain {
                    error: SecretError::Missing,
                    ..
                }
            ),
            "{err:?}"
        );
        assert_eq!(err.exit_code(), 1);
        let text = err.to_string();
        assert!(text.contains("not found"), "{text}");
        assert!(text.contains("Run `coda --setup`"), "{text}");
        assert!(text.contains(NAVIDROME_PASSWORD_ENV), "{text}");
        assert!(!text.contains("me@"), "{text}");
        assert!(!text.contains("music.example.org"), "{text}");

        // Unavailable store and denied access.
        let source = f.load(cli(None)).unwrap().navidrome.unwrap().password;
        let err = source
            .resolve_with(Duration::from_secs(1), || Err(SecretError::Unavailable))
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::Keychain {
                    error: SecretError::Unavailable,
                    ..
                }
            ),
            "{err:?}"
        );
        assert!(err.to_string().contains("could not read"), "{err}");
        assert!(err.to_string().contains("Run `coda --setup`"), "{err}");
        assert!(!err.to_string().contains("me@"), "{err}");
        assert!(!err.to_string().contains("music.example.org"), "{err}");
        assert_eq!(err.exit_code(), 1);

        store.set(account, &Password::new("x".into())).unwrap();
        store.fail_next(account, SecretError::Denied);
        let source = f.load(cli(None)).unwrap().navidrome.unwrap().password;
        let opened = store.clone();
        let err = source
            .resolve_with(Duration::from_secs(1), move || Ok(opened))
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::Keychain {
                    error: SecretError::Denied,
                    ..
                }
            ),
            "{err:?}"
        );
        assert!(!err.to_string().contains("from-keychain"), "{err}");
    }

    #[test]
    fn setup_start_takes_the_file_as_written() {
        let f = Fixture::new();
        // No file yet: nothing to prefill, but the path is known.
        let start = read_for_setup(&f.env).unwrap();
        assert_eq!(start.config_path, f.config_path());
        assert_eq!(start.music_dir, None);
        assert!(!start.has_navidrome);

        // Coda's own file; the folder need not exist (setup checks it).
        f.write(
            &format!("music_dir = \"~/Gone\"\n{KEYCHAIN}scrobble = false\n"),
            0o600,
        );
        let start = read_for_setup(&f.env).unwrap();
        assert_eq!(start.music_dir.as_deref(), Some("~/Gone"));
        assert_eq!(
            start.url.as_deref(),
            Some("https://music.example.org:8443/nd")
        );
        assert_eq!(start.username.as_deref(), Some("me"));
        assert_eq!(
            start.keychain_account.as_deref(),
            Some("me@music.example.org:8443")
        );
        assert_eq!(start.hand_written_password, None);
        assert!(start.has_navidrome);
        let text = format!("{start:?}");
        assert!(
            !text.contains("music.example") && !text.contains("me@"),
            "{text}"
        );

        // Env overrides do not change what the file holds or which entry
        // Coda wrote for it.
        let mut f = f;
        f.env.navidrome_user = Some("other".into());
        let start = read_for_setup(&f.env).unwrap();
        assert_eq!(start.username.as_deref(), Some("me"));
        assert_eq!(
            start.keychain_account.as_deref(),
            Some("me@music.example.org:8443")
        );
    }

    #[test]
    fn setup_start_accepts_partial_and_hand_written_navidrome() {
        let f = Fixture::new();
        // Partial: prefilled, no keychain entry to clean up.
        f.write(
            "[navidrome]\nurl = \"h.example\"\npassword_keychain = true\n",
            0o644,
        );
        let start = read_for_setup(&f.env).unwrap();
        assert_eq!(start.url.as_deref(), Some("h.example"));
        assert_eq!(start.username, None);
        assert_eq!(start.keychain_account, None);
        assert!(start.has_navidrome);
        // An invalid URL is prefilled as typed, without an account.
        f.write(
            "[navidrome]\nurl = \"ftp://h\"\nusername = \"u\"\npassword_keychain = true\n",
            0o644,
        );
        let start = read_for_setup(&f.env).unwrap();
        assert_eq!(start.url.as_deref(), Some("ftp://h"));
        assert_eq!(start.keychain_account, None);
        // Hand-written passwords are named, never read or run.
        f.write(NAVIDROME, 0o600);
        let start = read_for_setup(&f.env).unwrap();
        assert_eq!(start.hand_written_password, Some("password"));
        assert_eq!(start.keychain_account, None);
        f.write(
            "[navidrome]\nurl = \"https://h\"\nusername = \"u\"\n\
             password_command = [\"/nonexistent/should-not-run\"]\n",
            0o644,
        );
        let start = read_for_setup(&f.env).unwrap();
        assert_eq!(start.hand_written_password, Some("password_command"));
        // `scrobble` alone is not a Navidrome setup.
        f.write("[navidrome]\nscrobble = false\n", 0o644);
        assert!(!read_for_setup(&f.env).unwrap().has_navidrome);
    }

    #[test]
    fn setup_start_still_rejects_broken_files() {
        let f = Fixture::new();
        f.write("music_dir = \n", 0o644);
        let err = read_for_setup(&f.env).unwrap_err();
        assert!(matches!(err, ConfigError::ConfigParse { .. }), "{err:?}");
        f.write("[navidrome]\npasword = \"x\"\n", 0o600);
        assert!(matches!(
            read_for_setup(&f.env).unwrap_err(),
            ConfigError::ConfigParse { .. }
        ));
        f.write(NAVIDROME, 0o644);
        let err = read_for_setup(&f.env).unwrap_err();
        assert!(
            matches!(err, ConfigError::InsecurePermissions(_)),
            "{err:?}"
        );
        assert!(!err.to_string().contains("hunter2"), "{err}");
        f.write(
            "[navidrome]\nurl = \"https://h\"\nusername = \"u\"\npassword_command = [\"x\"]\n",
            0o666,
        );
        assert!(matches!(
            read_for_setup(&f.env).unwrap_err(),
            ConfigError::WritableCommandConfig(_)
        ));
        // An explicit CODA_CONFIG must exist, as for a normal start.
        let mut f = f;
        f.env.config_path = Some(f.dir.path().join("missing.toml").into());
        assert!(matches!(
            read_for_setup(&f.env).unwrap_err(),
            ConfigError::ConfigMissing(_)
        ));
        let err = read_for_setup(&Env::default()).unwrap_err();
        assert!(matches!(err, ConfigError::NoConfigLocation), "{err:?}");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn credentials_in_a_saved_url_are_never_prefilled() {
        use UrlPrefill::{AsWritten, Stripped, Withheld};
        for (raw, expected) in [
            (
                "https://u:hunter2@h.example/nd",
                Stripped("https://h.example/nd".into()),
            ),
            (
                "https://a@b:hunter2@h.example",
                Stripped("https://h.example/".into()),
            ),
            ("http://u@h:4533/", Stripped("http://h:4533/".into())),
            // A backslash ends the authority, as for startup: host `h`.
            ("https://h\\x@evil", AsWritten),
            // The `/` ends the authority, so text rules would show `ss`.
            ("https://me:pa/ss@h", Withheld),
            // Without a scheme it does not parse with a host.
            ("u:hunter2@h.example:4533", Withheld),
            ("https://h.example/x", AsWritten),
            ("h.example", AsWritten),
        ] {
            assert_eq!(prefill_url(raw), expected, "{raw}");
        }
        let url = Url::parse("https://h\\x@evil").unwrap();
        assert_eq!(url.host_str(), Some("h"));
        let f = Fixture::new();
        f.write(
            "[navidrome]\nurl = \"https://me:hunter2@h.example\"\nusername = \"me\"\n\
             password_keychain = true\n",
            0o600,
        );
        let start = read_for_setup(&f.env).unwrap();
        assert_eq!(start.url.as_deref(), Some("https://h.example/"));
        assert!(start.url_had_credentials && !start.url_withheld);
        // `parse_base_url` rejects such a URL, so Coda never wrote an entry.
        assert_eq!(start.keychain_account, None);
        f.write("[navidrome]\nurl = \"https://me:pa/ss@h\"\n", 0o600);
        let start = read_for_setup(&f.env).unwrap();
        assert_eq!(start.url, None, "withheld");
        assert!(start.url_withheld && !start.url_had_credentials);
    }

    #[test]
    fn setup_start_reports_whether_saving_backs_the_file_up() {
        let f = Fixture::new();
        assert!(!read_for_setup(&f.env).unwrap().has_content, "no file");
        f.write("# only notes\n\n", 0o644);
        assert!(!read_for_setup(&f.env).unwrap().has_content);
        f.write("[navidrome]\nscrobble = false\n", 0o644);
        assert!(read_for_setup(&f.env).unwrap().has_content);
    }

    #[test]
    fn navidrome_env_overrides_are_named_when_set() {
        let mut env = Env::default();
        assert!(navidrome_env_overrides(&env).is_empty());
        env.navidrome_url = Some("https://secret-host".into());
        env.navidrome_user = Some("".into());
        env.navidrome_password = Some("secret".into());
        assert_eq!(
            navidrome_env_overrides(&env),
            [NAVIDROME_URL_ENV, NAVIDROME_PASSWORD_ENV]
        );
    }

    #[test]
    fn setup_flag_and_music_dir_source() {
        let (cli, source) = Cli::try_parse_with_source(["coda", "--setup", "/m"]).unwrap();
        assert!(cli.setup);
        assert_eq!(cli.music_dir, Some(OsString::from("/m")));
        assert_eq!(source, Some(MusicDirArg::Argument));
        let (cli, source) = Cli::try_parse_with_source(["coda", ""]).unwrap();
        assert!(!cli.setup);
        assert_eq!(source, None, "an empty folder is no override");
        // The environment fallback (`CODA_MUSIC_DIR`) is not exercised
        // here: setting it would leak into the other tests in this process.
    }

    #[test]
    fn nonexistent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let err = validate_music_dir(dir.path().join("nope")).unwrap_err();
        assert!(matches!(err, ConfigError::NotFound(_)));
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn file_is_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.mp3");
        std::fs::write(&file, b"x").unwrap();
        let err = validate_music_dir(file).unwrap_err();
        assert!(matches!(err, ConfigError::NotADirectory(_)));
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn unreadable_directory() {
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = validate_music_dir(locked.clone());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        if std::fs::read_dir("/root").is_ok() {
            // Running as root: permissions are not enforced.
            return;
        }
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::Unreadable { .. }), "{err:?}");
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn valid_directory_is_canonicalized() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a");
        std::fs::create_dir(&nested).unwrap();
        let music_dir = validate_music_dir(nested.join("..").join("a")).unwrap();
        assert!(music_dir.is_absolute());
        assert_eq!(music_dir, std::fs::canonicalize(&nested).unwrap());
    }

    #[test]
    fn positional_argument_parses() {
        let cli = Cli::try_parse_from(["coda", "/some/dir"]).unwrap();
        assert_eq!(cli.music_dir, Some(OsString::from("/some/dir")));
    }

    #[test]
    fn empty_argument_is_unset_not_a_clap_error() {
        let cli = Cli::try_parse_from(["coda", ""]).unwrap();
        let err = Fixture::new().load(cli).unwrap_err();
        assert!(matches!(err, ConfigError::NoSource { .. }));
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn log_path_resolution() {
        assert_eq!(
            log_file_path(Some("/state".into()), Some("/home/u".into())),
            Some(PathBuf::from("/state/coda/coda.log"))
        );
        assert_eq!(
            log_file_path(None, Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.local/state/coda/coda.log"))
        );
        assert_eq!(
            log_file_path(Some("rel".into()), Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.local/state/coda/coda.log"))
        );
        assert_eq!(log_file_path(None, None), None);
        assert_eq!(
            session_file_path(Some("/state".into()), Some("/home/u".into())),
            Some(PathBuf::from("/state/coda/state.json"))
        );
        assert_eq!(
            session_file_path(Some("".into()), Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.local/state/coda/state.json"))
        );
        assert_eq!(session_file_path(None, None), None);
    }
}
