//! The `theme = …` setting of Ghostty's config, for the "follow Ghostty"
//! mode: `theme = Name` or `theme = light:A,dark:B` (either order); a
//! name may be an absolute path to a theme file (a relative path is looked
//! up by its file name). The last `theme` wins, as in Ghostty, including
//! in `config-file = [?]path` includes (read after the including file,
//! relative to it; at most 4 levels and 16 files). Only these keys are
//! read.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::fsio;
use super::sources::config_dir;

/// Ghostty's config files are not read past this size.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
/// `config-file` includes: nesting depth and files read in all.
const MAX_INCLUDE_DEPTH: usize = 4;
const MAX_CONFIG_FILES: usize = 16;

/// The theme(s) Ghostty is set to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhosttyTheme {
    Single(String),
    Pair { light: String, dark: String },
}

impl GhosttyTheme {
    pub fn light(&self) -> &str {
        match self {
            Self::Single(name) => name,
            Self::Pair { light, .. } => light,
        }
    }

    pub fn dark(&self) -> &str {
        match self {
            Self::Single(name) => name,
            Self::Pair { dark, .. } => dark,
        }
    }
}

/// Ghostty's config files, in the order Ghostty reads them (later wins).
pub fn config_paths(home: Option<OsString>, xdg_config_home: Option<OsString>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(config) = config_dir(home.clone(), xdg_config_home) {
        paths.push(config.join("ghostty").join("config"));
    }
    if cfg!(target_os = "macos")
        && let Some(home) = home.filter(|h| !h.is_empty())
    {
        paths.push(
            PathBuf::from(home).join("Library/Application Support/com.mitchellh.ghostty/config"),
        );
    }
    paths
}

/// The `theme` value of one config file's text, if it sets one.
pub fn parse_config(text: &str) -> Option<GhosttyTheme> {
    scan(text).0.flatten()
}

fn unquote(value: &str) -> &str {
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
        .trim()
}

/// One file's `theme` (`None` = not set, `Some(None)` = reset) and its
/// `config-file` includes in order.
fn scan(text: &str) -> (Option<Option<GhosttyTheme>>, Vec<String>) {
    let mut found = None;
    let mut includes = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = unquote(value);
        match key.trim() {
            // An empty value resets the theme.
            "theme" => {
                found = Some(if value.is_empty() {
                    None
                } else {
                    parse_value(value)
                });
            }
            "config-file" if !value.is_empty() => includes.push(value.to_owned()),
            _ => {}
        }
    }
    (found, includes)
}

fn parse_value(value: &str) -> Option<GhosttyTheme> {
    if !value.contains("light:") && !value.contains("dark:") {
        return Some(GhosttyTheme::Single(theme_name(value)));
    }
    let mut light = None;
    let mut dark = None;
    for part in value.split(',') {
        let part = part.trim();
        if let Some(name) = part.strip_prefix("light:") {
            light = Some(theme_name(name.trim()));
        } else if let Some(name) = part.strip_prefix("dark:") {
            dark = Some(theme_name(name.trim()));
        }
    }
    match (light, dark) {
        (Some(light), Some(dark)) => Some(GhosttyTheme::Pair { light, dark }),
        (Some(one), None) | (None, Some(one)) => Some(GhosttyTheme::Single(one)),
        (None, None) => None,
    }
}

/// An absolute path is kept (read as that file); another path is looked
/// up by its file name.
fn theme_name(value: &str) -> String {
    if value.contains('/') && !Path::new(value).is_absolute() {
        Path::new(value)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        value.to_owned()
    }
}

/// The theme Ghostty's config files set (later files win), if any.
pub fn read(paths: &[PathBuf]) -> Option<GhosttyTheme> {
    let mut found = None;
    let mut budget = MAX_CONFIG_FILES;
    for path in paths {
        load(path, 0, &mut budget, &mut found);
    }
    found
}

fn load(path: &Path, depth: usize, budget: &mut usize, found: &mut Option<GhosttyTheme>) {
    if *budget == 0 {
        tracing::info!("too many Ghostty config files; the rest are skipped");
        return;
    }
    *budget -= 1;
    let text = match fsio::read_regular(path, MAX_CONFIG_BYTES) {
        Ok(Some(text)) => text,
        Ok(None) => return,
        Err(error) => {
            tracing::info!(path = %path.display(), %error, "skipping a Ghostty config file");
            return;
        }
    };
    let (theme, includes) = scan(&text);
    if let Some(theme) = theme {
        *found = theme;
    }
    if depth >= MAX_INCLUDE_DEPTH {
        return;
    }
    let base = path.parent().unwrap_or(Path::new("/"));
    for include in includes {
        // `?path`: optional (missing is fine; it is skipped either way).
        let include = include.strip_prefix('?').unwrap_or(&include);
        load(&base.join(include), depth + 1, budget, found);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single(name: &str) -> Option<GhosttyTheme> {
        Some(GhosttyTheme::Single(name.into()))
    }

    #[test]
    fn reads_a_single_theme() {
        assert_eq!(
            parse_config("theme = Kanagawabones\n"),
            single("Kanagawabones")
        );
        assert_eq!(
            parse_config("theme=\"Catppuccin Mocha\""),
            single("Catppuccin Mocha")
        );
        assert_eq!(
            parse_config("font-size = 13\n# theme = Old\ntheme = A\ntheme = B\n"),
            single("B")
        );
        assert_eq!(parse_config("font-size = 13\n"), None);
        assert_eq!(parse_config("theme = A\ntheme =\n"), None);
        assert_eq!(
            parse_config("theme = /home/u/themes/Mine"),
            single("/home/u/themes/Mine")
        );
        assert_eq!(parse_config("theme = themes/Mine"), single("Mine"));
        assert_eq!(parse_config("themes = A\n"), None);
    }

    #[test]
    fn reads_light_and_dark_pairs() {
        let want = Some(GhosttyTheme::Pair {
            light: "Kanso Pearl".into(),
            dark: "Kanso Zen".into(),
        });
        assert_eq!(
            parse_config("theme = light:Kanso Pearl,dark:Kanso Zen"),
            want
        );
        assert_eq!(
            parse_config("theme = dark:Kanso Zen, light:Kanso Pearl"),
            want
        );
        assert_eq!(
            parse_config("theme = \"light:Kanso Pearl,dark:Kanso Zen\""),
            want
        );
        assert_eq!(parse_config("theme = dark:Kanso Zen"), single("Kanso Zen"));
    }

    #[test]
    fn later_files_win() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, "theme = A\n").unwrap();
        std::fs::write(&b, "font-size = 12\n").unwrap();
        let missing = dir.path().join("missing");
        assert_eq!(read(&[a.clone(), b.clone(), missing]), single("A"));
        std::fs::write(&b, "theme = B\n").unwrap();
        assert_eq!(read(&[a, b]), single("B"));
    }

    #[test]
    fn includes_are_followed_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("config");
        std::fs::write(
            &main,
            "config-file = ?missing\nconfig-file = \"sub/extra\"\ntheme = A\n",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        // Includes are read after the file, so B wins over A.
        std::fs::write(
            dir.path().join("sub/extra"),
            "theme = B\nconfig-file = loop\n",
        )
        .unwrap();
        // A self-including file stops at the caps.
        std::fs::write(dir.path().join("sub/loop"), "config-file = loop\n").unwrap();
        assert_eq!(read(std::slice::from_ref(&main)), single("B"));
    }

    #[test]
    fn a_fifo_config_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("config");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert_eq!(read(&[fifo]), None);
    }

    #[test]
    fn config_paths_follow_xdg() {
        let paths = config_paths(Some("/home/u".into()), Some("/xdg".into()));
        assert_eq!(paths[0], PathBuf::from("/xdg/ghostty/config"));
    }
}
