//! Where themes come from, merged by name (later wins): the built-in
//! themes, Ghostty's installed themes, the user's Ghostty themes, then
//! the user's Coda themes (`~/.config/coda-app/themes/`, where the colour
//! editor saves). A theme's name is its file name, as in Ghostty.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::parse::{self, MAX_THEME_BYTES, ThemeFile};

/// The themes compiled into the app (see `themes/THEMES-CREDITS.md`).
pub const BUILT_IN: [(&str, &str); 7] = [
    ("Coda", include_str!("../../themes/Coda")),
    ("Coda Light", include_str!("../../themes/Coda Light")),
    ("Kanagawabones", include_str!("../../themes/Kanagawabones")),
    ("Kanso Zen", include_str!("../../themes/Kanso Zen")),
    ("Kanso Pearl", include_str!("../../themes/Kanso Pearl")),
    (
        "Catppuccin Mocha",
        include_str!("../../themes/Catppuccin Mocha"),
    ),
    (
        "Catppuccin Latte",
        include_str!("../../themes/Catppuccin Latte"),
    ),
];

pub const DEFAULT_DARK: &str = "Coda";
pub const DEFAULT_LIGHT: &str = "Coda Light";

/// At most this many files are read from one folder.
pub const MAX_THEMES_PER_DIR: usize = 2000;
/// Longer names are skipped (and refused for custom themes).
pub const MAX_NAME_CHARS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    BuiltIn,
    Ghostty,
    /// The user's own (`~/.config/coda-app/themes/`): editable.
    Custom,
}

/// The theme folders, lowest priority first.
#[derive(Debug, Clone, Default)]
pub struct ThemeDirs {
    /// Ghostty's installed themes, then the user's Ghostty themes.
    pub ghostty: Vec<PathBuf>,
    /// `~/.config/coda-app/themes/`.
    pub custom: Option<PathBuf>,
}

impl ThemeDirs {
    pub fn from_process() -> Self {
        Self::new(
            std::env::var_os("HOME"),
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("GHOSTTY_RESOURCES_DIR"),
        )
    }

    pub fn new(
        home: Option<OsString>,
        xdg_config_home: Option<OsString>,
        ghostty_resources: Option<OsString>,
    ) -> Self {
        let config = config_dir(home, xdg_config_home);
        let mut ghostty = Vec::new();
        if cfg!(target_os = "macos") {
            ghostty.push(PathBuf::from(
                "/Applications/Ghostty.app/Contents/Resources/ghostty/themes",
            ));
        } else {
            ghostty.push(PathBuf::from("/usr/share/ghostty/themes"));
        }
        if let Some(dir) = ghostty_resources.filter(|d| !d.is_empty()) {
            let dir = PathBuf::from(dir).join("themes");
            if !ghostty.contains(&dir) {
                ghostty.push(dir);
            }
        }
        if let Some(config) = &config {
            ghostty.push(config.join("ghostty").join("themes"));
        }
        Self {
            ghostty,
            custom: config.map(|c| c.join("coda-app").join("themes")),
        }
    }
}

/// `$XDG_CONFIG_HOME`, else `~/.config` (on macOS too, like Ghostty).
pub fn config_dir(home: Option<OsString>, xdg_config_home: Option<OsString>) -> Option<PathBuf> {
    xdg_config_home
        .filter(|d| !d.is_empty() && Path::new(d).is_absolute())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|h| !h.is_empty())
                .map(|h| PathBuf::from(h).join(".config"))
        })
}

/// One theme and where it was found.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub source: Source,
    pub theme: ThemeFile,
}

/// Whether `name` can be a theme file's name: no path separators, not
/// hidden, no control characters, not too long.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.trim() == name
        && name.chars().count() <= MAX_NAME_CHARS
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
}

fn built_in(name: &str) -> Option<Entry> {
    let (name, text) = BUILT_IN.iter().find(|(n, _)| *n == name)?;
    Some(Entry {
        name: (*name).to_owned(),
        source: Source::BuiltIn,
        theme: parse::parse(text).expect("built-in themes parse"),
    })
}

/// Reads one theme file (size capped). Errors are logged by the caller.
fn read_file(path: &Path) -> Result<ThemeFile, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > MAX_THEME_BYTES {
        return Err(parse::ParseError::TooLarge.to_string());
    }
    let mut text = String::new();
    file.take(MAX_THEME_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    parse::parse(&text).map_err(|e| e.to_string())
}

fn read_dir(dir: &Path, source: Source, into: &mut BTreeMap<String, Entry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // Not installed: nothing to log.
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| valid_name(n))
        .collect();
    names.sort();
    if names.len() > MAX_THEMES_PER_DIR {
        tracing::warn!(dir = %dir.display(), count = names.len(), "too many theme files; reading the first {MAX_THEMES_PER_DIR}");
        names.truncate(MAX_THEMES_PER_DIR);
    }
    for name in names {
        match read_file(&dir.join(&name)) {
            Ok(theme) => {
                into.insert(
                    name.clone(),
                    Entry {
                        name,
                        source,
                        theme,
                    },
                );
            }
            Err(error) => {
                tracing::info!(dir = %dir.display(), theme = %name, %error, "skipping a theme file");
            }
        }
    }
}

/// Every theme from every source, by name (later sources win).
pub fn load_all(dirs: &ThemeDirs) -> BTreeMap<String, Entry> {
    let mut themes = BTreeMap::new();
    for (name, _) in BUILT_IN {
        if let Some(entry) = built_in(name) {
            themes.insert(name.to_owned(), entry);
        }
    }
    for dir in &dirs.ghostty {
        read_dir(dir, Source::Ghostty, &mut themes);
    }
    if let Some(dir) = &dirs.custom {
        read_dir(dir, Source::Custom, &mut themes);
    }
    themes
}

/// One theme by name, without reading every folder: the highest-priority
/// source that has a valid file of that name.
pub fn find(dirs: &ThemeDirs, name: &str) -> Option<Entry> {
    if !valid_name(name) {
        return None;
    }
    let folders = dirs
        .custom
        .iter()
        .map(|d| (d, Source::Custom))
        .chain(dirs.ghostty.iter().rev().map(|d| (d, Source::Ghostty)));
    for (dir, source) in folders {
        let path = dir.join(name);
        if !path.exists() {
            continue;
        }
        match read_file(&path) {
            Ok(theme) => {
                return Some(Entry {
                    name: name.to_owned(),
                    source,
                    theme,
                });
            }
            Err(error) => {
                tracing::info!(path = %path.display(), %error, "skipping a theme file");
            }
        }
    }
    built_in(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARK: &str = "background = #000000\nforeground = #ffffff\n";
    const LIGHT: &str = "background = #ffffff\nforeground = #000000\n";

    fn dirs(root: &Path) -> ThemeDirs {
        let system = root.join("system");
        let user_ghostty = root.join("ghostty");
        let custom = root.join("custom");
        for d in [&system, &user_ghostty, &custom] {
            std::fs::create_dir_all(d).unwrap();
        }
        ThemeDirs {
            ghostty: vec![system, user_ghostty],
            custom: Some(custom),
        }
    }

    #[test]
    fn built_ins_parse() {
        for (name, text) in BUILT_IN {
            parse::parse(text).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    #[test]
    fn later_sources_win_and_record_their_source() {
        let root = tempfile::tempdir().unwrap();
        let d = dirs(root.path());
        std::fs::write(d.ghostty[0].join("Shared"), DARK).unwrap();
        std::fs::write(d.ghostty[1].join("Shared"), LIGHT).unwrap();
        std::fs::write(d.ghostty[0].join("Only System"), DARK).unwrap();
        // A user theme may replace a built-in one.
        std::fs::write(d.custom.as_ref().unwrap().join("Coda"), LIGHT).unwrap();
        let all = load_all(&d);
        assert_eq!(all["Shared"].theme.background.r, 255);
        assert_eq!(all["Shared"].source, Source::Ghostty);
        assert_eq!(all["Only System"].source, Source::Ghostty);
        assert_eq!(all["Coda"].source, Source::Custom);
        assert_eq!(all["Kanso Zen"].source, Source::BuiltIn);
        // `find` agrees with the merge.
        assert_eq!(find(&d, "Shared").unwrap().theme.background.r, 255);
        assert_eq!(find(&d, "Coda").unwrap().source, Source::Custom);
        assert_eq!(find(&d, "Kanso Zen").unwrap().source, Source::BuiltIn);
        assert!(find(&d, "Nope").is_none());
    }

    #[test]
    fn invalid_oversized_and_hidden_files_are_skipped() {
        let root = tempfile::tempdir().unwrap();
        let d = dirs(root.path());
        let dir = &d.ghostty[1];
        std::fs::write(dir.join("Broken"), "background = nope\n").unwrap();
        std::fs::write(dir.join(".hidden"), DARK).unwrap();
        std::fs::write(dir.join("Huge"), format!("{DARK}{}", "#".repeat(70 * 1024))).unwrap();
        std::fs::create_dir(dir.join("A folder")).unwrap();
        std::fs::write(dir.join("Good"), DARK).unwrap();
        let all = load_all(&d);
        assert!(all.contains_key("Good"));
        for skipped in ["Broken", ".hidden", "Huge", "A folder"] {
            assert!(!all.contains_key(skipped), "{skipped}");
        }
        // A broken file falls through to a lower source in `find`.
        std::fs::write(d.ghostty[0].join("Broken"), DARK).unwrap();
        assert!(find(&d, "Broken").is_some());
    }

    #[test]
    fn names_that_are_paths_are_refused() {
        for bad in ["", "../x", "a/b", ".x", " x", "x\n", &"x".repeat(101)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        assert!(valid_name("Catppuccin Mocha (custom)"));
        let root = tempfile::tempdir().unwrap();
        assert!(find(&dirs(root.path()), "../custom/x").is_none());
    }

    #[test]
    fn folders_follow_xdg_and_ghostty_resources() {
        let d = ThemeDirs::new(
            Some("/home/u".into()),
            Some("/xdg".into()),
            Some("/opt/ghostty".into()),
        );
        assert_eq!(d.custom, Some(PathBuf::from("/xdg/coda-app/themes")));
        assert!(d.ghostty.contains(&PathBuf::from("/opt/ghostty/themes")));
        assert_eq!(
            d.ghostty.last(),
            Some(&PathBuf::from("/xdg/ghostty/themes"))
        );
        let d = ThemeDirs::new(Some("/home/u".into()), Some("relative".into()), None);
        assert_eq!(
            d.custom,
            Some(PathBuf::from("/home/u/.config/coda-app/themes"))
        );
    }
}
