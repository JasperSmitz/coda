//! Writes the config file chosen by the setup (first run or `coda --setup`)
//! or by the in-app settings, which change only `[navidrome]`
//! ([`write_navidrome`]).
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
//! Setup decides every setting the file can hold except `scrobble`, which
//! is kept, and `[navidrome]` as a whole when setup keeps it
//! ([`NavidromeSection::Keep`]): its keys are then written back by value.
//! The in-app settings keep `music_dir` the same way. Replacing the file
//! therefore loses only comments and the
//! settings setup replaced (e.g. a hand-written `password_command`); a
//! file with anything but whitespace and comments is first hard-linked to
//! `config.toml.bak.<secs>`, unless it is exactly what Coda wrote.

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
    pub navidrome: NavidromeSection,
}

/// What happens to `[navidrome]`. `scrobble` is kept in every case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavidromeSection {
    /// No server.
    Remove,
    /// Every key as it is in the existing file (including a hand-written
    /// `password` or `password_command`).
    Keep,
    /// This server, with `password_keychain = true`.
    Write(SetupNavidrome),
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

const HEADER: &str = "# Coda configuration, written by Coda's setup.\n\
                      # You may edit this file; see the README for all settings.\n";

/// An existing file, read for what setup keeps. Strict like the loader
/// (unknown keys are errors), so a file changed during setup into
/// something Coda would not load is never carried over.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Existing {
    music_dir: Option<String>,
    navidrome: Option<ExistingNavidrome>,
}

/// What happens to `music_dir`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MusicDirSection {
    /// This folder (as typed), or none.
    Set(Option<String>),
    /// As it is in the existing file (the in-app settings never change it).
    Keep,
}

/// Why [`write_navidrome`] refused to remove Navidrome.
pub const NO_SOURCE_LEFT: &str = "the config file would have no music source left";

/// The result of [`write_navidrome`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// The old file's backup, if it was backed up.
    pub backup: Option<PathBuf>,
    /// The `scrobble` setting the written file has (default `true`).
    pub scrobble: bool,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExistingNavidrome {
    url: Option<String>,
    username: Option<String>,
    password: Option<String>,
    password_command: Option<Vec<String>>,
    password_keychain: Option<bool>,
    scrobble: Option<bool>,
}

/// Writes `config` to `path`. Returns the backup's path if the old file
/// was backed up.
pub fn write(path: &Path, config: &SetupConfig) -> io::Result<Option<PathBuf>> {
    let music_dir = MusicDirSection::Set(config.music_dir.clone());
    write_sections(path, &music_dir, &config.navidrome).map(|written| written.backup)
}

/// Writes `navidrome` to `path`, keeping `music_dir` and `scrobble` as
/// they are in the existing file (the in-app settings). An existing file
/// that cannot be parsed is not replaced, so a folder is never lost.
/// Removing Navidrome from a file without a `music_dir` is refused
/// ([`NO_SOURCE_LEFT`]): the file would configure no source.
pub fn write_navidrome(path: &Path, navidrome: &NavidromeSection) -> io::Result<Written> {
    write_sections(path, &MusicDirSection::Keep, navidrome)
}

fn write_sections(
    path: &Path,
    music_dir: &MusicDirSection,
    navidrome: &NavidromeSection,
) -> io::Result<Written> {
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
    let keep = *navidrome == NavidromeSection::Keep;
    let keep_music_dir = *music_dir == MusicDirSection::Keep;
    let (kept_music_dir, parsed) = match existing.as_deref().map(toml::from_str::<Existing>) {
        Some(Ok(existing)) => (existing.music_dir, existing.navidrome),
        // Never drop a setting that was meant to be kept. The parser's
        // message is not passed on: it can quote the file.
        Some(Err(_)) if keep => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the current [navidrome] settings could not be read",
            ));
        }
        Some(Err(_)) if keep_music_dir => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the current settings could not be read",
            ));
        }
        Some(Err(_)) | None => (None, None),
    };
    let music_dir = match music_dir {
        MusicDirSection::Set(dir) => dir.clone(),
        MusicDirSection::Keep => kept_music_dir.filter(|dir| !dir.trim().is_empty()),
    };
    if keep_music_dir && *navidrome == NavidromeSection::Remove && music_dir.is_none() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, NO_SOURCE_LEFT));
    }
    if keep && parsed.is_none() {
        // The file changed (or went away) since setup read it.
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the [navidrome] settings to keep are no longer in the file",
        ));
    }
    let backup = match &existing {
        Some(text) if needs_backup(text) => Some(back_up(path)?),
        _ => None,
    };

    let parsed = parsed.unwrap_or_default();
    let scrobble = parsed.scrobble.unwrap_or(true);
    let text = render(music_dir.as_deref(), navidrome, parsed);
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
    Ok(Written { backup, scrobble })
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

/// `true` if `text` has anything besides whitespace and `#` comments: the
/// rule for backing a file up before it is replaced.
pub(crate) fn has_content(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with('#')
    })
}

/// `true` if `text` is backed up before it is replaced: it holds settings
/// and is not exactly what Coda itself writes. A file with other comments,
/// a hand-written key (`password`, `password_command`) or another layout
/// is backed up; one Coda wrote and nobody changed is not, so saves do not
/// pile up backups.
pub(crate) fn needs_backup(text: &str) -> bool {
    has_content(text) && !is_codas_own(text)
}

/// `true` if `text` is byte for byte what [`write`] produces for its own
/// settings: the header, `music_dir`, and `[navidrome]` with `url`,
/// `username`, `password_keychain = true` and `scrobble`, or only
/// `scrobble`.
fn is_codas_own(text: &str) -> bool {
    let Ok(existing) = toml::from_str::<Existing>(text) else {
        return false;
    };
    let navidrome = existing.navidrome.unwrap_or_default();
    let section = match (
        &navidrome.url,
        &navidrome.username,
        navidrome.password_keychain,
        &navidrome.password,
        &navidrome.password_command,
    ) {
        (None, None, None, None, None) => NavidromeSection::Remove,
        (Some(url), Some(username), Some(true), None, None) => {
            match crate::navidrome::client::parse_base_url(url) {
                Ok(url) => NavidromeSection::Write(SetupNavidrome {
                    url,
                    username: username.clone(),
                }),
                Err(_) => return false,
            }
        }
        _ => return false,
    };
    render(existing.music_dir.as_deref(), &section, navidrome) == text
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

fn render(
    music_dir: Option<&str>,
    navidrome: &NavidromeSection,
    existing: ExistingNavidrome,
) -> String {
    let mut out = String::from(HEADER);
    if let Some(dir) = music_dir {
        out.push_str(&format!("\nmusic_dir = {}\n", toml_string(dir)));
    }
    let mut section = String::new();
    match navidrome {
        NavidromeSection::Remove => {}
        NavidromeSection::Write(navidrome) => section.push_str(&format!(
            "url = {}\nusername = {}\npassword_keychain = true\n",
            toml_string(navidrome.url.as_str()),
            toml_string(&navidrome.username),
        )),
        NavidromeSection::Keep => {
            let mut key = |name: &str, value: Option<String>| {
                if let Some(value) = value {
                    section.push_str(&format!("{name} = {value}\n"));
                }
            };
            key("url", existing.url.as_deref().map(toml_string));
            key("username", existing.username.as_deref().map(toml_string));
            key("password", existing.password.as_deref().map(toml_string));
            key(
                "password_command",
                existing.password_command.as_ref().map(|argv| {
                    let items: Vec<String> = argv.iter().map(|a| toml_string(a)).collect();
                    format!("[{}]", items.join(", "))
                }),
            );
            key(
                "password_keychain",
                existing.password_keychain.map(|b| b.to_string()),
            );
        }
    }
    if let Some(scrobble) = existing.scrobble {
        section.push_str(&format!("scrobble = {scrobble}\n"));
    }
    if !section.is_empty() {
        out.push_str("\n[navidrome]\n");
        out.push_str(&section);
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
            Config::load(Cli::default(), &self.env()).unwrap()
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
            navidrome: NavidromeSection::Write(navidrome(
                "http://localhost:4533/nd",
                "me \\ \"you\" ü",
            )),
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
            navidrome: NavidromeSection::Write(navidrome("https://h", "a\tb\u{1b}c")),
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
            navidrome: NavidromeSection::Remove,
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
                navidrome: NavidromeSection::Write(navidrome("https://h", "u")),
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
            navidrome: NavidromeSection::Write(navidrome("https://h", "u")),
        };
        let backup = write(&path, &config).unwrap().unwrap();
        let name = backup.file_name().unwrap().as_bytes();
        assert!(name.starts_with(b"conf\xff.toml.bak."), "{name:?}");
        assert_eq!(fs::read_to_string(backup).unwrap(), "music_dir = \"/x\"\n");
    }

    fn keep(music_dir: Option<&str>) -> SetupConfig {
        SetupConfig {
            music_dir: music_dir.map(Into::into),
            navidrome: NavidromeSection::Keep,
        }
    }

    #[test]
    fn keep_fails_when_the_section_is_gone_or_unreadable() {
        let f = Fixture::new();
        fs::create_dir_all(f.path().parent().unwrap()).unwrap();
        // No file, or a file without [navidrome] (changed during setup).
        let error = write(&f.path(), &keep(Some("/m"))).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!f.path().exists());
        for text in [
            "music_dir = \"/m\"\n",
            "[navidrome]\nurl = \"https://h\"\npasword = \"hunter2\"\n",
            "[navidrome]\nurl = \"https://h\"\n[other]\nx = 1\n",
        ] {
            fs::write(f.path(), text).unwrap();
            let error = write(&f.path(), &keep(Some("/m"))).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{text}");
            assert!(!error.to_string().contains("hunter2"), "{error}");
            assert_eq!(fs::read_to_string(f.path()).unwrap(), text, "untouched");
        }
        assert_eq!(f.files(), ["config.toml"], "no backup or temp file left");
    }

    #[test]
    fn keep_round_trips_a_hand_written_password_exactly() {
        let f = Fixture::new();
        fs::create_dir_all(f.path().parent().unwrap()).unwrap();
        // A multi-line literal string: backslashes, quotes, non-ASCII, a
        // line break and a tab, all taken as they are.
        let old = "music_dir = \"/old\"\n[navidrome]\nurl = \"https://h.example\"\n\
                   username = \"m\u{e9}\"\n\
                   password = '''a\\b \"q\" \u{fc}\u{1f3b5}\nline two\ttab'''\n\
                   scrobble = false\n";
        fs::write(f.path(), old).unwrap();
        fs::set_permissions(f.path(), Permissions::from_mode(0o600)).unwrap();
        let music = f.dir.path().join("m");
        fs::create_dir(&music).unwrap();
        write(&f.path(), &keep(Some(music.to_str().unwrap()))).unwrap();
        assert_eq!(
            fs::metadata(f.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let nd = f.load().navidrome.unwrap();
        assert_eq!(nd.username, "m\u{e9}");
        assert!(!nd.scrobble);
        let PasswordSource::Value(password) = nd.password else {
            panic!("expected the hand-written password")
        };
        assert_eq!(
            password.expose(),
            "a\\b \"q\" \u{fc}\u{1f3b5}\nline two\ttab"
        );
        // A control character from a basic-string escape survives too.
        fs::write(
            f.path(),
            "[navidrome]\nurl = \"https://h\"\nusername = \"u\"\npassword = \"x\\u0001y\"\n",
        )
        .unwrap();
        fs::set_permissions(f.path(), Permissions::from_mode(0o600)).unwrap();
        write(&f.path(), &keep(Some(music.to_str().unwrap()))).unwrap();
        let PasswordSource::Value(password) = f.load().navidrome.unwrap().password else {
            panic!("expected the hand-written password")
        };
        assert_eq!(password.expose(), "x\u{1}y");
    }

    #[test]
    fn comment_only_file_is_replaced_without_backup() {
        let f = Fixture::new();
        fs::create_dir_all(f.path().parent().unwrap()).unwrap();
        fs::write(f.path(), "# my notes\n\n   # more\n").unwrap();
        let config = SetupConfig {
            music_dir: None,
            navidrome: NavidromeSection::Write(navidrome("https://h", "u")),
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
            navidrome: NavidromeSection::Write(navidrome("https://h", "u")),
        };
        let backup = write(&f.path(), &config).unwrap().unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), old);
        let name = backup.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("config.toml.bak."), "{name}");
        assert!(!f.load().navidrome.unwrap().scrobble);

        // A second backup in the same second does not replace the first.
        fs::write(f.path(), old).unwrap();
        let second = write(&f.path(), &config).unwrap().unwrap();
        assert_ne!(second, backup);
        assert_eq!(fs::read_to_string(&backup).unwrap(), old);
        assert_eq!(f.files().len(), 3, "{:?}", f.files());

        // A kept scrobble setting without Navidrome is still valid.
        let music = f.dir.path().join("m");
        fs::create_dir(&music).unwrap();
        let config = SetupConfig {
            music_dir: Some(music.to_str().unwrap().into()),
            navidrome: NavidromeSection::Remove,
        };
        write(&f.path(), &config).unwrap();
        let text = fs::read_to_string(f.path()).unwrap();
        assert!(text.contains("[navidrome]\nscrobble = false\n"), "{text}");
        let loaded = f.load();
        assert!(loaded.navidrome.is_none());
        assert!(loaded.music_dir.is_some());
    }

    #[test]
    fn write_navidrome_keeps_the_folder_and_scrobble() {
        let f = Fixture::new();
        fs::create_dir_all(f.path().parent().unwrap()).unwrap();
        fs::create_dir_all(f.home().join("My Music")).unwrap();
        // The folder as typed (`~/` kept), and a hand-written password
        // that Connect replaces with the keychain.
        let old = "music_dir = \"~/My Music\"\n[navidrome]\nscrobble = false\n";
        fs::write(f.path(), old).unwrap();
        let section = NavidromeSection::Write(navidrome("https://h.example", "me"));
        let written = write_navidrome(&f.path(), &section).unwrap();
        assert!(!written.scrobble);
        assert!(written.backup.is_some(), "the old file had settings");
        let text = fs::read_to_string(f.path()).unwrap();
        assert!(text.contains("music_dir = \"~/My Music\""), "{text}");
        assert!(text.contains("scrobble = false"), "{text}");
        let config = f.load();
        assert_eq!(
            config.music_dir.unwrap(),
            f.home().join("My Music").canonicalize().unwrap()
        );
        assert!(matches!(
            config.navidrome.unwrap().password,
            PasswordSource::Keychain { .. }
        ));
        // Removing Navidrome keeps the folder too.
        let written = write_navidrome(&f.path(), &NavidromeSection::Remove).unwrap();
        assert!(!written.scrobble);
        let config = f.load();
        assert!(config.navidrome.is_none());
        assert!(config.music_dir.is_some());
    }

    #[test]
    fn write_navidrome_without_a_file_writes_only_navidrome() {
        let f = Fixture::new();
        let section = NavidromeSection::Write(navidrome("https://h.example", "me"));
        let written = write_navidrome(&f.path(), &section).unwrap();
        assert_eq!(written.backup, None);
        assert!(written.scrobble, "the default");
        let text = fs::read_to_string(f.path()).unwrap();
        assert!(!text.contains("music_dir"), "{text}");
        assert!(f.load().music_dir.is_none());
    }

    #[test]
    fn write_navidrome_never_replaces_a_file_it_cannot_read() {
        let f = Fixture::new();
        fs::create_dir_all(f.path().parent().unwrap()).unwrap();
        let text = "music_dir = \"/m\"\nunknown = 1\n";
        fs::write(f.path(), text).unwrap();
        let section = NavidromeSection::Write(navidrome("https://h.example", "me"));
        let error = write_navidrome(&f.path(), &section).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read_to_string(f.path()).unwrap(), text, "untouched");
        assert_eq!(f.files(), ["config.toml"], "no backup or temp file left");
    }

    #[test]
    fn removing_navidrome_from_a_file_without_a_folder_is_refused() {
        let f = Fixture::new();
        fs::create_dir_all(f.path().parent().unwrap()).unwrap();
        for text in [
            "[navidrome]\nurl = \"https://h\"\nusername = \"u\"\npassword_keychain = true\n",
            "music_dir = \"\"\n[navidrome]\nurl = \"https://h\"\nusername = \"u\"\npassword_keychain = true\n",
        ] {
            fs::write(f.path(), text).unwrap();
            let error = write_navidrome(&f.path(), &NavidromeSection::Remove).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(error.to_string(), NO_SOURCE_LEFT);
            assert_eq!(fs::read_to_string(f.path()).unwrap(), text, "untouched");
            assert_eq!(f.files(), ["config.toml"], "no backup or temp file left");
        }
        // No file at all: nothing to remove from, and no source either.
        fs::remove_file(f.path()).unwrap();
        assert!(write_navidrome(&f.path(), &NavidromeSection::Remove).is_err());
        assert!(!f.path().exists());
    }

    #[test]
    fn a_file_coda_wrote_is_not_backed_up_again() {
        let f = Fixture::new();
        let music = f.dir.path().join("m");
        fs::create_dir(&music).unwrap();
        let first = SetupConfig {
            music_dir: Some(music.to_str().unwrap().into()),
            navidrome: NavidromeSection::Write(navidrome("https://h.example/nd", "u")),
        };
        write(&f.path(), &first).unwrap();
        let written = fs::read_to_string(f.path()).unwrap();
        assert!(!needs_backup(&written), "{written}");
        // Saving over it again (setup or the in-app settings) adds none.
        assert_eq!(write(&f.path(), &first).unwrap(), None);
        let other = NavidromeSection::Write(navidrome("https://other.example", "v"));
        assert_eq!(write_navidrome(&f.path(), &other).unwrap().backup, None);
        assert_eq!(
            write_navidrome(&f.path(), &NavidromeSection::Remove)
                .unwrap()
                .backup,
            None
        );
        assert_eq!(f.files(), ["config.toml"]);
        // With scrobble kept, and with only a folder: still Coda's own.
        let text = fs::read_to_string(f.path()).unwrap();
        assert!(!needs_backup(&text), "{text}");
        assert!(!needs_backup(&format!(
            "{text}\n[navidrome]\nscrobble = false\n"
        )));

        // Anything changed by hand is backed up.
        for changed in [
            format!("{text}# my note\n"),
            text.replace("music_dir = ", "music_dir   = "),
            format!(
                "{HEADER}\n[navidrome]\nurl = \"https://h/\"\nusername = \"u\"\npassword = \"p\"\n"
            ),
            format!(
                "{HEADER}\n[navidrome]\nurl = \"https://h/\"\nusername = \"u\"\npassword_command = [\"x\"]\n"
            ),
            format!(
                "{HEADER}\n[navidrome]\nurl = \"https://h\"\nusername = \"u\"\npassword_keychain = true\n"
            ),
            "music_dir = \"/m\"\n".to_owned(),
        ] {
            assert!(needs_backup(&changed), "{changed}");
        }
        fs::write(f.path(), format!("{text}# my note\n")).unwrap();
        assert!(write(&f.path(), &first).unwrap().is_some());
    }
}
