//! Writes the config file chosen by the first-run setup.
//!
//! The file is small and fixed in shape, so it is written by hand (with
//! TOML basic-string escaping) rather than through a TOML editor. The
//! write is atomic, like the session file: a 0600 temp file in the same
//! directory, `fsync`, rename. A missing parent directory is created with
//! mode 0700.
//!
//! If `config.toml` is a symlink, the link itself is replaced by a regular
//! file (rename semantics); nothing is written through it, and its target
//! is left as it was.
//!
//! Setup only runs when the existing file (if any) configures no source,
//! so replacing it loses nothing but a `scrobble` setting, which is kept,
//! and comments. A file with anything but whitespace and comments is
//! first hard-linked to `config.toml.bak.<secs>`.

use std::fs::{self, File, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use url::Url;

/// What setup decided. No password: that goes to the credential store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupConfig {
    /// As the user typed it (`~/Music` stays `~/Music`).
    pub music_dir: Option<String>,
    pub navidrome: Option<SetupNavidrome>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SetupNavidrome {
    pub url: Url,
    pub username: String,
}

impl std::fmt::Debug for SetupNavidrome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SetupNavidrome(<redacted>)")
    }
}

const HEADER: &str = "# Coda configuration, written by the first-run setup.\n\
                      # You may edit this file; see the README for all settings.\n";

/// The parts of an existing file that setup keeps. Lenient: anything else
/// is ignored here (and was validated by `Config::load` before setup).
#[derive(Default, Deserialize)]
struct Existing {
    navidrome: Option<ExistingNavidrome>,
}

#[derive(Default, Deserialize)]
struct ExistingNavidrome {
    scrobble: Option<bool>,
}

/// Writes `config` to `path`. Returns the backup's path if the old file
/// was backed up.
pub fn write(path: &Path, config: &SetupConfig) -> io::Result<Option<PathBuf>> {
    let dir = parent_dir(path)?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;

    let existing = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let scrobble = existing
        .as_deref()
        .and_then(|text| toml::from_str::<Existing>(text).ok())
        .and_then(|e| e.navidrome)
        .and_then(|n| n.scrobble);
    let backup = match &existing {
        Some(text) if has_content(text) => Some(back_up(path)?),
        _ => None,
    };

    let text = render(config, scrobble);
    let file_name = path.file_name().map_or_else(
        || "config.toml".into(),
        |n| n.to_string_lossy().into_owned(),
    );
    let mut temp = tempfile::Builder::new()
        .prefix(&format!(".{file_name}."))
        .suffix(".tmp")
        .permissions(Permissions::from_mode(0o600))
        .tempfile_in(dir)?;
    temp.write_all(text.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
    Ok(backup)
}

/// The directory `path` lives in; `.` for a bare file name (a relative
/// `CODA_CONFIG`).
fn parent_dir(path: &Path) -> io::Result<&Path> {
    match path.parent() {
        Some(dir) if dir.as_os_str().is_empty() => Ok(Path::new(".")),
        Some(dir) => Ok(dir),
        None => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no parent directory",
        )),
    }
}

/// `true` if `text` has anything besides whitespace and `#` comments.
fn has_content(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with('#')
    })
}

/// Hard-links `path` to `<path>.bak.<secs>` (adding `.1`, `.2`, … if that
/// exists), never replacing an existing file.
fn back_up(path: &Path) -> io::Result<PathBuf> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Built from the OsString: `display()` is lossy for non-UTF-8 paths.
    let mut base = path.as_os_str().to_owned();
    base.push(format!(".bak.{secs}"));
    for n in 0..100 {
        let mut candidate = base.clone();
        if n > 0 {
            candidate.push(format!(".{n}"));
        }
        let candidate = PathBuf::from(candidate);
        match fs::hard_link(path, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "too many config backups",
    ))
}

fn render(config: &SetupConfig, scrobble: Option<bool>) -> String {
    let mut out = String::from(HEADER);
    if let Some(dir) = &config.music_dir {
        out.push_str(&format!("\nmusic_dir = {}\n", toml_string(dir)));
    }
    if config.navidrome.is_some() || scrobble.is_some() {
        out.push_str("\n[navidrome]\n");
    }
    if let Some(navidrome) = &config.navidrome {
        out.push_str(&format!(
            "url = {}\nusername = {}\npassword_keychain = true\n",
            toml_string(navidrome.url.as_str()),
            toml_string(&navidrome.username),
        ));
    }
    if let Some(scrobble) = scrobble {
        out.push_str(&format!("scrobble = {scrobble}\n"));
    }
    out
}

/// A TOML basic string.
fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Cli, Config, Env, PasswordSource};
    use crate::navidrome::client::parse_base_url;

    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
            }
        }

        fn home(&self) -> PathBuf {
            self.dir.path().join("home")
        }

        fn path(&self) -> PathBuf {
            self.dir.path().join("xdg").join("coda").join("config.toml")
        }

        fn env(&self) -> Env {
            Env {
                xdg_config_home: Some(self.dir.path().join("xdg").into()),
                home: Some(self.home().into()),
                ..Env::default()
            }
        }

        fn load(&self) -> Config {
            Config::load(Cli { music_dir: None }, &self.env()).unwrap()
        }

        fn files(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.path().parent().unwrap())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    fn navidrome(url: &str, username: &str) -> SetupNavidrome {
        SetupNavidrome {
            url: parse_base_url(url).unwrap(),
            username: username.into(),
        }
    }

    #[test]
    fn round_trips_through_config_load_with_private_modes() {
        let f = Fixture::new();
        fs::create_dir_all(f.home().join("My \"Music\"")).unwrap();
        let config = SetupConfig {
            music_dir: Some("~/My \"Music\"".into()),
            navidrome: Some(navidrome("http://localhost:4533/nd", "me \\ \"you\" ü")),
        };
        assert_eq!(write(&f.path(), &config).unwrap(), None);

        let text = fs::read_to_string(f.path()).unwrap();
        assert!(text.starts_with("# Coda configuration"), "{text}");
        assert!(
            text.contains("music_dir = \"~/My \\\"Music\\\"\""),
            "{text}"
        );
        assert!(text.contains("password_keychain = true"), "{text}");
        assert!(!text.contains("scrobble"), "{text}");

        let loaded = f.load();
        assert_eq!(
            loaded.music_dir,
            Some(fs::canonicalize(f.home().join("My \"Music\"")).unwrap())
        );
        let nd = loaded.navidrome.unwrap();
        assert_eq!(nd.url.as_str(), "http://localhost:4533/nd/");
        assert_eq!(nd.username, "me \\ \"you\" ü");
        assert!(nd.scrobble);
        assert!(matches!(
            nd.password,
            PasswordSource::Keychain { ref account, .. } if account == "me \\ \"you\" ü@localhost:4533"
        ));

        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&f.path()), 0o600);
        // Both created directories are private.
        assert_eq!(mode(f.path().parent().unwrap()), 0o700);
        assert_eq!(mode(f.path().parent().unwrap().parent().unwrap()), 0o700);
        // No temp file is left behind.
        assert_eq!(f.files(), ["config.toml"]);
    }

    #[test]
    fn control_characters_are_escaped() {
        assert_eq!(toml_string("a\u{1}b\u{7f}\n"), "\"a\\u0001b\\u007F\\n\"");
        let f = Fixture::new();
        let config = SetupConfig {
            music_dir: None,
            navidrome: Some(navidrome("https://h", "a\tb\u{1b}c")),
        };
        write(&f.path(), &config).unwrap();
        assert_eq!(f.load().navidrome.unwrap().username, "a\tb\u{1b}c");
    }

    #[test]
    fn existing_parent_directory_keeps_its_mode() {
        let f = Fixture::new();
        let dir = f.path().parent().unwrap().to_owned();
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, Permissions::from_mode(0o755)).unwrap();
        let music = f.dir.path().join("m");
        fs::create_dir(&music).unwrap();
        let config = SetupConfig {
            music_dir: Some(music.to_str().unwrap().into()),
            navidrome: None,
        };
        write(&f.path(), &config).unwrap();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(f.load().music_dir, Some(fs::canonicalize(music).unwrap()));
        let text = fs::read_to_string(f.path()).unwrap();
        assert!(!text.contains("[navidrome]"), "{text}");
    }

    #[test]
    fn bare_file_names_live_in_the_current_directory() {
        assert_eq!(
            parent_dir(Path::new("config.toml")).unwrap(),
            Path::new(".")
        );
        assert_eq!(
            parent_dir(Path::new("a/config.toml")).unwrap(),
            Path::new("a")
        );
        assert_eq!(
            parent_dir(Path::new("/x/config.toml")).unwrap(),
            Path::new("/x")
        );
        assert!(parent_dir(Path::new("/")).is_err());
    }

    #[test]
    fn a_symlinked_config_is_replaced_and_its_target_left_alone() {
        let f = Fixture::new();
        fs::create_dir_all(f.path().parent().unwrap()).unwrap();
        let target = f.dir.path().join("dotfiles-config.toml");
        for old in [
            "# managed elsewhere\n",
            "# managed\n[navidrome]\nscrobble = false\n",
        ] {
            fs::write(&target, old).unwrap();
            let _ = fs::remove_file(f.path());
            std::os::unix::fs::symlink(&target, f.path()).unwrap();
            let config = SetupConfig {
                music_dir: None,
                navidrome: Some(navidrome("https://h", "u")),
            };
            write(&f.path(), &config).unwrap();
            let meta = fs::symlink_metadata(f.path()).unwrap();
            assert!(meta.file_type().is_file(), "the link became a regular file");
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
            assert_eq!(
                fs::read_to_string(&target).unwrap(),
                old,
                "target untouched"
            );
            assert!(
                fs::read_to_string(f.path())
                    .unwrap()
                    .contains("password_keychain = true")
            );
        }
        // The second (non-empty) one was backed up, and its scrobble kept.
        assert!(!f.load().navidrome.unwrap().scrobble);
        let backups: Vec<_> = f
            .files()
            .into_iter()
            .filter(|n| n.contains(".bak."))
            .collect();
        assert_eq!(backups.len(), 1, "{backups:?}");
        let backup = f.path().parent().unwrap().join(&backups[0]);
        assert_eq!(
            fs::read_to_string(backup).unwrap(),
            "# managed\n[navidrome]\nscrobble = false\n"
        );
    }

    #[test]
    fn backup_names_keep_non_utf8_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let f = Fixture::new();
        let dir = f.dir.path().join("cfg");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(std::ffi::OsStr::from_bytes(b"conf\xff.toml"));
        if fs::write(&path, "music_dir = \"/x\"\n").is_err() {
            // APFS rejects non-UTF-8 names (EILSEQ); Linux file systems
            // accept them.
            return;
        }
        let config = SetupConfig {
            music_dir: None,
            navidrome: Some(navidrome("https://h", "u")),
        };
        let backup = write(&path, &config).unwrap().unwrap();
        let name = backup.file_name().unwrap().as_bytes();
        assert!(name.starts_with(b"conf\xff.toml.bak."), "{name:?}");
        assert_eq!(fs::read_to_string(backup).unwrap(), "music_dir = \"/x\"\n");
    }

    #[test]
    fn comment_only_file_is_replaced_without_backup() {
        let f = Fixture::new();
        fs::create_dir_all(f.path().parent().unwrap()).unwrap();
        fs::write(f.path(), "# my notes\n\n   # more\n").unwrap();
        let config = SetupConfig {
            music_dir: None,
            navidrome: Some(navidrome("https://h", "u")),
        };
        assert_eq!(write(&f.path(), &config).unwrap(), None);
        assert_eq!(f.files(), ["config.toml"]);
    }

    #[test]
    fn file_with_settings_is_backed_up_and_scrobble_kept() {
        let f = Fixture::new();
        fs::create_dir_all(f.path().parent().unwrap()).unwrap();
        let old = "# mine\n[navidrome]\nscrobble = false\n";
        fs::write(f.path(), old).unwrap();
        let config = SetupConfig {
            music_dir: None,
            navidrome: Some(navidrome("https://h", "u")),
        };
        let backup = write(&f.path(), &config).unwrap().unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), old);
        let name = backup.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("config.toml.bak."), "{name}");
        assert!(!f.load().navidrome.unwrap().scrobble);

        // A second backup in the same second does not replace the first.
        let second = write(&f.path(), &config).unwrap().unwrap();
        assert_ne!(second, backup);
        assert_eq!(fs::read_to_string(&backup).unwrap(), old);
        assert_eq!(f.files().len(), 3, "{:?}", f.files());

        // A kept scrobble setting without Navidrome is still valid.
        let music = f.dir.path().join("m");
        fs::create_dir(&music).unwrap();
        let config = SetupConfig {
            music_dir: Some(music.to_str().unwrap().into()),
            navidrome: None,
        };
        write(&f.path(), &config).unwrap();
        let text = fs::read_to_string(f.path()).unwrap();
        assert!(text.contains("[navidrome]\nscrobble = false\n"), "{text}");
        let loaded = f.load();
        assert!(loaded.navidrome.is_none());
        assert!(loaded.music_dir.is_some());
    }
}
