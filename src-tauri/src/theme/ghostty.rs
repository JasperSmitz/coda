//! The `theme = …` setting of Ghostty's config, for the "follow Ghostty"
//! mode: `theme = Name` or `theme = light:A,dark:B` (either order). The
//! last `theme` line wins, as in Ghostty. Only this one key is read.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::sources::config_dir;

/// Ghostty's config files are not read past this size.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

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
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "theme" {
            continue;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value)
            .trim();
        // An empty value resets the theme.
        found = if value.is_empty() {
            None
        } else {
            parse_value(value)
        };
    }
    found
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

/// A theme given as a path is looked up by its file name.
fn theme_name(value: &str) -> String {
    if value.contains('/') {
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
    for path in paths {
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        let mut text = String::new();
        if file
            .take(MAX_CONFIG_BYTES)
            .read_to_string(&mut text)
            .is_err()
        {
            tracing::info!(path = %path.display(), "Ghostty's config is not readable text");
            continue;
        }
        if let Some(theme) = parse_config(&text) {
            found = Some(theme);
        }
    }
    found
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
        assert_eq!(parse_config("theme = /home/u/themes/Mine"), single("Mine"));
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
    fn config_paths_follow_xdg() {
        let paths = config_paths(Some("/home/u".into()), Some("/xdg".into()));
        assert_eq!(paths[0], PathBuf::from("/xdg/ghostty/config"));
    }
}
