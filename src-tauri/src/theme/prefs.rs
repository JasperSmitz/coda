//! The GUI's own preferences (`~/.config/coda-app/settings.toml`), apart
//! from Coda's shared `config.toml`: the theme mode and names, accent
//! overrides per theme and the visualizer's colours. Written atomically;
//! an unreadable or invalid file means the defaults (and is kept as
//! `settings.toml.bad` when it is next replaced).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::color::Rgba;
use super::sources::{DEFAULT_DARK, DEFAULT_LIGHT, valid_name};

const MAX_PREFS_BYTES: u64 = 256 * 1024;
const MAX_ACCENTS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// One theme, always.
    Single,
    /// A light and a dark theme, following the system's appearance.
    #[default]
    System,
    /// Whatever Ghostty's config sets.
    Ghostty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VisualizerColors {
    #[default]
    Accent,
    /// Palette colours 1→6 across the bars.
    Palette,
}

/// The appearance preferences; also their DTO (camelCase).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    pub mode: Mode,
    /// The theme in `single` mode.
    pub theme: String,
    /// The themes in `system` mode.
    pub light_theme: String,
    pub dark_theme: String,
    /// Accent override by theme name (`#rrggbb`).
    pub accents: BTreeMap<String, String>,
    pub visualizer: VisualizerColors,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            mode: Mode::System,
            theme: DEFAULT_DARK.into(),
            light_theme: DEFAULT_LIGHT.into(),
            dark_theme: DEFAULT_DARK.into(),
            accents: BTreeMap::new(),
            visualizer: VisualizerColors::Accent,
        }
    }
}

impl Prefs {
    /// Drops what cannot be valid (bad names, non-hex accents); names
    /// that are invalid fall back to the defaults.
    pub fn sanitized(mut self) -> Self {
        let defaults = Prefs::default();
        for (name, default) in [
            (&mut self.theme, &defaults.theme),
            (&mut self.light_theme, &defaults.light_theme),
            (&mut self.dark_theme, &defaults.dark_theme),
        ] {
            if !valid_name(name) {
                *name = default.clone();
            }
        }
        self.accents = self
            .accents
            .into_iter()
            .filter(|(name, _)| valid_name(name))
            .filter_map(|(name, hex)| Some((name, Rgba::parse_rgb(&hex)?.to_string())))
            .take(MAX_ACCENTS)
            .collect();
        self
    }

    /// The accent override for `theme`, if any.
    pub fn accent_for(&self, theme: &str) -> Option<Rgba> {
        self.accents.get(theme).and_then(|hex| Rgba::parse_rgb(hex))
    }

    /// Every theme name these preferences point at, renamed.
    pub fn rename_theme(&mut self, from: &str, to: &str) {
        for name in [&mut self.theme, &mut self.light_theme, &mut self.dark_theme] {
            if name == from {
                *name = to.to_owned();
            }
        }
        if let Some(accent) = self.accents.remove(from) {
            self.accents.insert(to.to_owned(), accent);
        }
    }
}

/// `<config>/coda-app/settings.toml`.
pub fn path(config_dir: Option<&Path>) -> Option<PathBuf> {
    config_dir.map(|c| c.join("coda-app").join("settings.toml"))
}

/// The saved preferences, or the defaults (logged) if the file is missing,
/// unreadable or invalid. The flag says the file exists but was not valid.
pub fn load(path: Option<&Path>) -> (Prefs, bool) {
    let Some(path) = path else {
        return (Prefs::default(), false);
    };
    let text = match std::fs::metadata(path) {
        Err(_) => return (Prefs::default(), false),
        Ok(meta) if meta.len() > MAX_PREFS_BYTES => {
            tracing::warn!(path = %path.display(), "settings.toml is too large; using the defaults");
            return (Prefs::default(), true);
        }
        Ok(_) => std::fs::read_to_string(path),
    };
    match text
        .map_err(|e| e.to_string())
        .and_then(|t| toml::from_str::<Prefs>(&t).map_err(|e| e.to_string()))
    {
        Ok(prefs) => (prefs.sanitized(), false),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "settings.toml is not valid; using the defaults");
            (Prefs::default(), true)
        }
    }
}

/// Writes `prefs` atomically (a temporary file renamed over the old one).
/// `keep_bad` first renames an invalid existing file to `.bad`.
pub fn save(path: &Path, prefs: &Prefs, keep_bad: bool) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("settings.toml has no folder"))?;
    std::fs::create_dir_all(dir)?;
    if keep_bad && path.exists() {
        let _ = std::fs::rename(path, path.with_extension("toml.bad"));
    }
    let text = toml::to_string(prefs).map_err(std::io::Error::other)?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(b"# Coda (desktop app) preferences. Written by the app.\n")?;
    temp.write_all(text.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coda-app").join("settings.toml");
        let mut prefs = Prefs {
            mode: Mode::Ghostty,
            theme: "Kanso Zen".into(),
            visualizer: VisualizerColors::Palette,
            ..Prefs::default()
        };
        prefs.accents.insert("Kanso Zen".into(), "#112233".into());
        save(&path, &prefs, false).unwrap();
        assert_eq!(load(Some(&path)), (prefs, false));
    }

    #[test]
    fn missing_or_invalid_files_mean_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        assert_eq!(load(Some(&path)), (Prefs::default(), false));
        assert_eq!(load(None), (Prefs::default(), false));
        std::fs::write(&path, "mode = 3\n").unwrap();
        assert_eq!(load(Some(&path)), (Prefs::default(), true));
        // The bad file is kept aside on the next save.
        save(&path, &Prefs::default(), true).unwrap();
        assert!(dir.path().join("settings.toml.bad").exists());
        assert_eq!(load(Some(&path)), (Prefs::default(), false));
    }

    #[test]
    fn partial_files_fill_in_defaults_and_drop_bad_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(
            &path,
            "mode = \"single\"\ntheme = \"../evil\"\n[accents]\n\"Kanso Zen\" = \"#8BA4B0\"\n\"X\" = \"red\"\n",
        )
        .unwrap();
        let (prefs, bad) = load(Some(&path));
        assert!(!bad);
        assert_eq!(prefs.mode, Mode::Single);
        assert_eq!(prefs.theme, DEFAULT_DARK);
        assert_eq!(prefs.light_theme, DEFAULT_LIGHT);
        assert_eq!(prefs.accents.len(), 1);
        assert_eq!(prefs.accents["Kanso Zen"], "#8ba4b0");
    }

    #[test]
    fn renaming_follows_every_reference() {
        let mut prefs = Prefs {
            theme: "A".into(),
            dark_theme: "A".into(),
            ..Prefs::default()
        };
        prefs.accents.insert("A".into(), "#000000".into());
        prefs.rename_theme("A", "B");
        assert_eq!(
            (prefs.theme.as_str(), prefs.dark_theme.as_str()),
            ("B", "B")
        );
        assert_eq!(prefs.light_theme, DEFAULT_LIGHT);
        assert!(prefs.accents.contains_key("B") && !prefs.accents.contains_key("A"));
    }
}
