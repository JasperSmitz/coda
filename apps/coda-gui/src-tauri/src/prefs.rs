//! The GUI's own preferences (`~/.config/coda-app/settings.toml`), apart
//! from Coda's shared `config.toml`: the accent colour (DESIGN.md
//! "Preferences"; `"music"` by default, also when the key is missing),
//! the source the app browses, the Navidrome lyrics index switch,
//! Library's default view (`libraryView`: Covers or List) and the screen
//! the app starts on (`startScreen`: Home, Search or Library; 2.0 R4). Written atomically; an unreadable or invalid file means
//! the defaults (and is kept as `settings.toml.bad` when it is next
//! replaced). Keys this version does not know, such as 1.x's theme
//! settings (`mode`, `theme`, `lightTheme`, `darkTheme`, `accents`,
//! `visualizer`), are ignored on read and dropped by the next write
//! (ADR 0009).
//!
//! The accent mode, and a chosen accent, reach the webview before its
//! first paint as an initialization script ([`PrefsStore::init_script`]);
//! Preferences changes them through [`commands`].

pub mod commands;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsio;
use crate::history::SourceKind;
use crate::palette::color::{Rgba, contrast, text_on};
use crate::palette::{BASE, HOUSE_ACCENT, SWATCHES, Swatch};

const MAX_PREFS_BYTES: u64 = 256 * 1024;

/// The accent colour: from the music (the default: each cover's own
/// accent where a cover is the subject, the house accent elsewhere), the
/// house accent, or one fixed colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Accent {
    #[default]
    Music,
    House,
    Color(Rgba),
}

impl Accent {
    /// `"music"`, `"house"` or `#rrggbb` (any case; the `#` is optional).
    /// The house accent's own colour typed as hex is the house accent.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        match text {
            "music" => return Some(Self::Music),
            "house" => return Some(Self::House),
            _ => {}
        }
        Rgba::parse_rgb(text).map(|color| {
            if color == HOUSE_ACCENT {
                Self::House
            } else {
                Self::Color(color)
            }
        })
    }

    /// The colour it stands for (from the music: the house accent, shown
    /// where no cover is the subject).
    pub fn color(self) -> Rgba {
        match self {
            Self::Music | Self::House => HOUSE_ACCENT,
            Self::Color(color) => color,
        }
    }
}

/// An unknown value in the file falls back to the default (from the
/// music).
impl From<String> for Accent {
    fn from(text: String) -> Self {
        Self::parse(&text).unwrap_or_else(|| {
            tracing::info!("settings.toml: the accent is not a colour; using the music's");
            Self::default()
        })
    }
}

impl From<Accent> for String {
    fn from(accent: Accent) -> Self {
        match accent {
            Accent::Music => "music".to_owned(),
            Accent::House => "house".to_owned(),
            Accent::Color(color) => color.to_string(),
        }
    }
}

/// How Library shows albums and artists by default (Preferences ›
/// Library); a section the listener switched keeps its own choice (in
/// the webview).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum LibraryView {
    #[default]
    Covers,
    List,
}

impl LibraryView {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "covers" => Some(Self::Covers),
            "list" => Some(Self::List),
            _ => None,
        }
    }
}

/// An unknown value in the file falls back to the default (Covers),
/// keeping the file's other settings.
impl From<String> for LibraryView {
    fn from(text: String) -> Self {
        Self::parse(&text).unwrap_or_else(|| {
            tracing::info!("settings.toml: libraryView is not covers or list; using covers");
            Self::default()
        })
    }
}

impl From<LibraryView> for String {
    fn from(view: LibraryView) -> Self {
        match view {
            LibraryView::Covers => "covers".to_owned(),
            LibraryView::List => "list".to_owned(),
        }
    }
}

/// The screen the app starts on (Preferences › Start on; 2.0 R4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum StartScreen {
    #[default]
    Home,
    Search,
    Library,
}

impl StartScreen {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "home" => Some(Self::Home),
            "search" => Some(Self::Search),
            "library" => Some(Self::Library),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Search => "search",
            Self::Library => "library",
        }
    }
}

/// An unknown value in the file reads as Home, keeping the file's other
/// settings.
impl From<String> for StartScreen {
    fn from(text: String) -> Self {
        Self::parse(&text).unwrap_or_else(|| {
            tracing::info!("settings.toml: startScreen is not home, search or library; using home");
            Self::default()
        })
    }
}

impl From<StartScreen> for String {
    fn from(screen: StartScreen) -> Self {
        screen.as_str().to_owned()
    }
}

/// The preferences as saved (camelCase keys).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    pub accent: Accent,
    /// The source the app browses (`None`: Navidrome if configured, else
    /// local). Changed only by `ui_source_set`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceKind>,
    /// "Index lyrics for search" (Preferences › Navidrome; see
    /// `crate::lyrics_index`). Changed only by `lyrics_index_set_enabled`.
    pub lyrics_index: bool,
    /// Library's default view of albums and artists.
    pub library_view: LibraryView,
    /// The screen the app starts on.
    pub start_screen: StartScreen,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            accent: Accent::Music,
            source: None,
            lyrics_index: true,
            library_view: LibraryView::Covers,
            start_screen: StartScreen::Home,
        }
    }
}

/// What Preferences shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefsDto {
    /// `"music"`, `"house"` or `#rrggbb`, as saved.
    pub accent: String,
    /// The accent's colour, `#rrggbb` (the house accent's for `"music"`).
    pub accent_color: String,
    /// Black or white, whichever reads better on the accent.
    pub accent_text: String,
    /// The accent's contrast with the base (WCAG ratio, two decimals).
    pub accent_contrast: f64,
    /// The curated accents.
    pub swatches: &'static [Swatch],
    pub source: Option<SourceKind>,
    pub lyrics_index: bool,
    /// `"covers"` or `"list"`.
    pub library_view: LibraryView,
    /// `"home"`, `"search"` or `"library"`.
    pub start_screen: StartScreen,
}

impl PrefsDto {
    pub fn new(prefs: &Prefs) -> Self {
        let color = prefs.accent.color();
        Self {
            accent: prefs.accent.into(),
            accent_color: color.to_string(),
            accent_text: text_on(color).to_string(),
            accent_contrast: (contrast(color, BASE) * 100.0).round() / 100.0,
            swatches: &SWATCHES,
            source: prefs.source,
            lyrics_index: prefs.lyrics_index,
            library_view: prefs.library_view,
            start_screen: prefs.start_screen,
        }
    }
}

/// The preferences the commands share (managed state).
pub struct PrefsStore {
    pub path: Option<PathBuf>,
    /// The preferences, and whether the file on disk was invalid. One
    /// change at a time.
    pub state: tokio::sync::Mutex<(Prefs, bool)>,
}

impl PrefsStore {
    pub fn from_process() -> Self {
        let config = config_dir(
            std::env::var_os("HOME"),
            std::env::var_os("XDG_CONFIG_HOME"),
        );
        Self::new(path(config.as_deref()))
    }

    /// Loads the file at `path` (blocking: call at startup).
    pub fn new(path: Option<PathBuf>) -> Self {
        let loaded = load(path.as_deref());
        Self {
            path,
            state: tokio::sync::Mutex::new(loaded),
        }
    }

    /// The preferences now (at startup, when nothing else holds the lock).
    pub fn current(&self) -> Prefs {
        self.state
            .try_lock()
            .map(|s| s.0.clone())
            .unwrap_or_default()
    }

    /// The main window's initialization script.
    pub fn init_script(&self) -> String {
        init_script(&self.current())
    }
}

/// A script that marks the accent mode on `<html>` before the document
/// paints (`data-accent-mode`: `music`, `house` or `custom`; the colour
/// scopes read it until the preferences have loaded, so a "From the
/// music" start never blends from amber) and sets a chosen accent
/// (`--accent`, `--accent-text`; `--accent-soft` and the visualizer's
/// `--viz` follow in `tokens.css`). It also marks the screen to start on
/// (`data-start-screen`: `home`, `search` or `library`), which the
/// navigation reads when it is created, so the first paint is already
/// that screen. Only the mode, the screen and two hex values go in, as
/// JSON.
pub fn init_script(prefs: &Prefs) -> String {
    let mut payload = match prefs.accent {
        Accent::Color(color) => serde_json::json!({
            "mode": "custom",
            "accent": color.to_string(),
            "accentText": text_on(color).to_string(),
        }),
        Accent::Music => serde_json::json!({ "mode": "music" }),
        Accent::House => serde_json::json!({ "mode": "house" }),
    };
    payload["start"] = serde_json::Value::from(prefs.start_screen.as_str());
    format!(
        r#"(function () {{
  var a = {payload};
  function apply() {{
    var root = document.documentElement;
    if (!root) return false;
    root.dataset.accentMode = a.mode;
    root.dataset.startScreen = a.start;
    if (a.accent) {{
      root.style.setProperty("--accent", a.accent);
      root.style.setProperty("--accent-text", a.accentText);
    }}
    return true;
  }}
  if (!apply()) {{
    new MutationObserver(function (_, observer) {{
      if (apply()) observer.disconnect();
    }}).observe(document, {{ childList: true }});
  }}
}})();"#
    )
}

/// `$XDG_CONFIG_HOME` if absolute, else `~/.config` (on macOS too).
pub fn config_dir(home: Option<OsString>, xdg_config_home: Option<OsString>) -> Option<PathBuf> {
    xdg_config_home
        .filter(|d| !d.is_empty() && Path::new(d).is_absolute())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|h| !h.is_empty())
                .map(|h| PathBuf::from(h).join(".config"))
        })
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
    let text = match fsio::read_regular(path, MAX_PREFS_BYTES) {
        Ok(None) => return (Prefs::default(), false),
        Ok(Some(text)) => Ok(text),
        Err(error) => Err(error.to_string()),
    };
    match text.and_then(|t| toml::from_str::<Prefs>(&t).map_err(|e| e.to_string())) {
        Ok(prefs) => (prefs, false),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "settings.toml is not valid; using the defaults");
            (Prefs::default(), true)
        }
    }
}

/// Writes `prefs` atomically (a temporary file renamed over the old one).
/// `keep_bad` first copies an invalid existing file to `.bad`.
/// A symlinked `settings.toml` (dotfile managers) stays a link: its
/// target is written.
pub fn save(path: &Path, prefs: &Prefs, keep_bad: bool) -> std::io::Result<()> {
    let target = fsio::write_target(path);
    if keep_bad && target.is_file() {
        let _ = std::fs::copy(&target, path.with_extension("toml.bad"));
    }
    let text = toml::to_string(prefs).map_err(std::io::Error::other)?;
    let mut bytes = b"# Coda (desktop app) preferences. Written by the app.\n".to_vec();
    bytes.extend_from_slice(text.as_bytes());
    fsio::write_atomic(&target, &bytes)
}

#[cfg(test)]
mod tests;
