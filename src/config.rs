//! Music-directory resolution and validation, plus the log file location.
//!
//! The music directory comes from the positional argument, else
//! `CODA_MUSIC_DIR` (the argument wins). There is no config file.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

pub const MUSIC_DIR_ENV: &str = "CODA_MUSIC_DIR";

/// Command line. `clap` reads `CODA_MUSIC_DIR` when the argument is absent.
/// The value is an `OsString` because clap's path parser rejects empty
/// values with a generic error; an empty value is reported as unset by
/// [`Config::from_cli`] instead.
#[derive(Debug, clap::Parser)]
#[command(
    name = "coda",
    version,
    about = "A keyboard-first terminal music player"
)]
pub struct Cli {
    /// Music directory to browse [default: $CODA_MUSIC_DIR]
    #[arg(value_name = "MUSIC_DIR", env = MUSIC_DIR_ENV, hide_env = true)]
    pub music_dir: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Canonicalized, absolute music directory.
    pub music_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(
        "no music directory given.\n\nUsage: coda [MUSIC_DIR]\n\
         Or set {MUSIC_DIR_ENV}, for example: {MUSIC_DIR_ENV}=~/Music coda"
    )]
    Unset,
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
}

impl ConfigError {
    /// Exit status: 2 for usage errors, 1 for an unusable directory.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Unset => 2,
            _ => 1,
        }
    }
}

impl Config {
    pub fn from_cli(cli: Cli) -> Result<Self, ConfigError> {
        Self::from_music_dir(cli.music_dir.map(PathBuf::from))
    }

    /// Validates the directory: it must exist, be a directory and be
    /// listable. An empty value counts as unset.
    pub fn from_music_dir(music_dir: Option<PathBuf>) -> Result<Self, ConfigError> {
        let path = music_dir
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or(ConfigError::Unset)?;
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
        let music_dir = std::fs::canonicalize(&path).map_err(unreadable)?;
        Ok(Self { music_dir })
    }
}

/// `$XDG_STATE_HOME/coda/coda.log`, else `~/.local/state/coda/coda.log`.
/// Relative or empty values are ignored; `None` if no location is known.
pub fn log_file_path(xdg_state_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let absolute = |v: Option<OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    let state = absolute(xdg_state_home)
        .or_else(|| absolute(home).map(|h| h.join(".local").join("state")))?;
    Some(state.join("coda").join("coda.log"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn unset_or_empty_is_a_usage_error() {
        let err = Config::from_music_dir(None).unwrap_err();
        assert!(matches!(err, ConfigError::Unset));
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("CODA_MUSIC_DIR"));
        assert!(matches!(
            Config::from_music_dir(Some(PathBuf::new())),
            Err(ConfigError::Unset)
        ));
    }

    #[test]
    fn nonexistent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let err = Config::from_music_dir(Some(dir.path().join("nope"))).unwrap_err();
        assert!(matches!(err, ConfigError::NotFound(_)));
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn file_is_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.mp3");
        std::fs::write(&file, b"x").unwrap();
        let err = Config::from_music_dir(Some(file)).unwrap_err();
        assert!(matches!(err, ConfigError::NotADirectory(_)));
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn unreadable_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = Config::from_music_dir(Some(locked.clone()));
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
        let config = Config::from_music_dir(Some(nested.join("..").join("a"))).unwrap();
        assert!(config.music_dir.is_absolute());
        assert_eq!(config.music_dir, std::fs::canonicalize(&nested).unwrap());
    }

    #[test]
    fn positional_argument_parses() {
        let cli = Cli::try_parse_from(["coda", "/some/dir"]).unwrap();
        assert_eq!(cli.music_dir, Some(OsString::from("/some/dir")));
    }

    #[test]
    fn empty_argument_is_unset_not_a_clap_error() {
        let cli = Cli::try_parse_from(["coda", ""]).unwrap();
        let err = Config::from_cli(cli).unwrap_err();
        assert!(matches!(err, ConfigError::Unset));
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
    }
}
