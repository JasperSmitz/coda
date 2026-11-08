//! Colour themes (DESIGN.md "Theming"): Ghostty theme files from several
//! folders ([`sources`]), parsed defensively ([`parse`]), turned into UI
//! tokens with a contrast guard ([`derive`]), chosen by the GUI's own
//! preferences ([`prefs`]: one theme, a light/dark pair following the
//! system, or Ghostty's own `theme` setting via [`ghostty`]).
//!
//! The chosen theme reaches the webview before its first paint as an
//! initialization script ([`Appearance::init_script`]); the Settings view
//! changes it through [`commands`]. Custom themes (the colour editor's)
//! are Ghostty files in `~/.config/coda-app/themes/`; built-in and
//! Ghostty-installed themes are never written.
//!
//! None of this touches the engine: it is the webview's look only.

pub mod color;
pub mod commands;
pub mod derive;
pub mod fsio;
pub mod ghostty;
pub mod parse;
pub mod prefs;
pub mod sources;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::Serialize;

use self::color::Rgba;
use self::derive::{Scheme, Token, derive, editable_keys};
use self::ghostty::GhosttyTheme;
use self::parse::{AccentSpec, ThemeFile};
use self::prefs::{Mode, Prefs};
use self::sources::{DEFAULT_DARK, DEFAULT_LIGHT, Entry, Source, ThemeDirs, valid_name};

/// Colour edits from the editor, by key ([`editable_keys`]): `Some(hex)`
/// sets a colour; `None` drops the override so the colour is computed
/// again (for `accent`: the theme's own, without the saved override).
pub type Edits = BTreeMap<String, Option<String>>;

/// A theme ready to apply: every token.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedTheme {
    pub name: String,
    pub source: Source,
    pub scheme: Scheme,
    pub tokens: Vec<Token>,
}

/// One line of the theme picker.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeSummary {
    pub name: String,
    pub source: Source,
    pub scheme: Scheme,
    pub bg: String,
    pub fg: String,
    pub accent: String,
    /// Palette 1–6.
    pub swatches: Vec<String>,
}

/// The light and dark themes in use (equal in single mode).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub light: ResolvedTheme,
    pub dark: ResolvedTheme,
    /// What Ghostty's config names, in Ghostty mode.
    pub ghostty: Option<GhosttyNames>,
    /// Why something else than asked is shown (a missing theme, …).
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GhosttyNames {
    pub light: String,
    pub dark: String,
}

/// Everything the Appearance settings show.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceDto {
    pub prefs: Prefs,
    #[serde(flatten)]
    pub applied: Applied,
    /// Where custom themes are saved.
    pub themes_folder: Option<String>,
}

/// The theme state the commands share: folders and the preferences.
pub struct Appearance {
    pub dirs: ThemeDirs,
    pub prefs_path: Option<PathBuf>,
    pub ghostty_configs: Vec<PathBuf>,
    /// The preferences, and whether the file on disk was invalid. One
    /// change (preferences or custom theme files) at a time.
    pub state: tokio::sync::Mutex<(Prefs, bool)>,
}

impl Appearance {
    pub fn from_process() -> Self {
        let home = std::env::var_os("HOME");
        let xdg = std::env::var_os("XDG_CONFIG_HOME");
        let config = sources::config_dir(home.clone(), xdg.clone());
        Self::new(
            ThemeDirs::from_process(),
            prefs::path(config.as_deref()),
            ghostty::config_paths(home, xdg),
        )
    }

    pub fn new(
        dirs: ThemeDirs,
        prefs_path: Option<PathBuf>,
        ghostty_configs: Vec<PathBuf>,
    ) -> Self {
        let loaded = prefs::load(prefs_path.as_deref());
        Self {
            dirs,
            prefs_path,
            ghostty_configs,
            state: tokio::sync::Mutex::new(loaded),
        }
    }

    /// The script the main window runs before its document: it sets the
    /// chosen theme's tokens on `<html>` (light or dark by the system's
    /// appearance), so the first paint already has the right colours.
    /// Also the light and dark backgrounds, for the native window behind
    /// the webview. Blocking (reads a few small files); call at window
    /// creation.
    pub fn startup(&self) -> Startup {
        // Nothing else holds the lock at window creation.
        let prefs = self
            .state
            .try_lock()
            .map(|s| s.0.clone())
            .unwrap_or_default();
        let applied = applied(&self.dirs, &self.ghostty_configs, &prefs);
        Startup {
            script: init_script(&applied, &prefs),
            light_bg: background(&applied.light),
            dark_bg: background(&applied.dark),
        }
    }
}

/// What the main window is created with.
pub struct Startup {
    pub script: String,
    pub light_bg: Rgba,
    pub dark_bg: Rgba,
}

/// A resolved theme's background (opaque).
pub fn background(theme: &ResolvedTheme) -> Rgba {
    theme
        .tokens
        .iter()
        .find(|t| t.key == "bg")
        .and_then(|t| Rgba::parse(&t.value))
        .unwrap_or(color::BLACK)
        .opaque()
}

/// The script for [`Appearance::init_script`]. Only token names and hex
/// values go in (no theme names), as JSON.
pub fn init_script(applied: &Applied, prefs: &Prefs) -> String {
    let vars = |t: &ResolvedTheme| {
        serde_json::json!({
            "scheme": t.scheme,
            "tokens": t.tokens.iter().map(|t| (t.key.clone(), t.value.clone())).collect::<BTreeMap<_, _>>(),
        })
    };
    let payload = serde_json::json!({
        "light": vars(&applied.light),
        "dark": vars(&applied.dark),
        "visualizer": prefs.visualizer,
    });
    format!(
        r#"(function () {{
  var theme = {payload};
  window.__CODA_THEME__ = theme;
  function apply() {{
    var root = document.documentElement;
    if (!root) return false;
    var light = window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches;
    var t = light ? theme.light : theme.dark;
    for (var key in t.tokens) root.style.setProperty("--" + key, t.tokens[key]);
    root.style.colorScheme = t.scheme;
    root.setAttribute("data-scheme", t.scheme);
    root.setAttribute("data-visualizer", theme.visualizer);
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

/// Applies `edits` to `theme`; returns the keys they set explicitly.
fn apply_edits(theme: &mut ThemeFile, edits: &Edits) -> Result<BTreeSet<String>, String> {
    let keys: BTreeSet<String> = editable_keys().collect();
    let mut explicit = BTreeSet::new();
    for (key, value) in edits {
        if !keys.contains(key) {
            return Err(format!("Unknown colour `{key}`"));
        }
        let Some(value) = value else {
            if key == "accent" {
                // Handled by the caller (drops the saved override only).
            } else {
                theme.overrides.remove(key);
            }
            continue;
        };
        let opaque_only =
            key == "bg" || key == "fg" || key == "accent" || key.starts_with("palette-");
        let color = if opaque_only {
            Rgba::parse_rgb(value)
        } else {
            Rgba::parse(value)
        }
        .ok_or_else(|| format!("`{value}` is not a hex colour"))?;
        match key.as_str() {
            "bg" => theme.background = color,
            "fg" => theme.foreground = color,
            "accent" => theme.accent = Some(AccentSpec::Color(color)),
            _ => {
                if let Some(n) = key.strip_prefix("palette-") {
                    let n: usize = n.parse().map_err(|_| format!("Unknown colour `{key}`"))?;
                    theme.palette[n] = Some(color);
                } else if key == "selection" && color.is_opaque() {
                    // Ghostty's own key, so Ghostty sees the edit too.
                    theme.selection_background = Some(color);
                    theme.overrides.remove(key);
                } else {
                    theme.overrides.insert(key.clone(), color);
                }
            }
        }
        explicit.insert(key.clone());
    }
    Ok(explicit)
}

/// `entry` with the saved accent override and `edits`, as a theme file
/// and the keys the user set: the file's `coda-explicit` keys, a saved
/// accent and every edit (a reset un-sets). The returned file carries
/// that set, so a saved or duplicated copy is guarded exactly like the
/// original.
fn edited(
    entry: &Entry,
    prefs: &Prefs,
    edits: &Edits,
) -> Result<(ThemeFile, BTreeSet<String>), String> {
    let mut theme = entry.theme.clone();
    let mut explicit = theme.explicit.clone();
    for (key, value) in edits {
        if value.is_none() {
            explicit.remove(key);
        }
    }
    match edits.get("accent") {
        // Reset: the theme's own accent (a custom theme's computed one).
        Some(None) => {
            if entry.source == Source::Custom {
                theme.accent = None;
            }
        }
        Some(Some(_)) => {}
        None => {
            if let Some(accent) = prefs.accent_for(&entry.name) {
                theme.accent = Some(AccentSpec::Color(accent));
                explicit.insert("accent".to_owned());
            }
        }
    }
    explicit.extend(apply_edits(&mut theme, edits)?);
    theme.explicit = explicit.clone();
    Ok((theme, explicit))
}

/// `entry` resolved with the saved accent override and `edits`.
pub fn resolve(entry: &Entry, prefs: &Prefs, edits: &Edits) -> Result<ResolvedTheme, String> {
    let (theme, explicit) = edited(entry, prefs, edits)?;
    let derived = derive(&theme, &explicit);
    Ok(ResolvedTheme {
        name: entry.name.clone(),
        source: entry.source,
        scheme: derived.scheme,
        tokens: derived.tokens,
    })
}

fn summary(entry: &Entry, prefs: &Prefs) -> ThemeSummary {
    let resolved = resolve(entry, prefs, &Edits::new()).expect("no edits cannot fail");
    let value = |key: &str| {
        resolved
            .tokens
            .iter()
            .find(|t| t.key == key)
            .map(|t| t.value.clone())
            .unwrap_or_default()
    };
    ThemeSummary {
        name: entry.name.clone(),
        source: entry.source,
        scheme: resolved.scheme,
        bg: value("bg"),
        fg: value("fg"),
        accent: value("accent"),
        swatches: (1..=6).map(|n| value(&format!("palette-{n}"))).collect(),
    }
}

/// Every theme for the picker, by name. Blocking.
pub fn list(dirs: &ThemeDirs, prefs: &Prefs) -> Vec<ThemeSummary> {
    sources::load_all(dirs)
        .values()
        .map(|entry| summary(entry, prefs))
        .collect()
}

/// The theme called `name`, else the default for `fallback_scheme`
/// (noted). Blocking.
fn find_or_default(dirs: &ThemeDirs, name: &str, dark: bool, notes: &mut Vec<String>) -> Entry {
    // Ghostty also takes an absolute path to a theme file.
    let found = if std::path::Path::new(name).is_absolute() {
        sources::find_path(std::path::Path::new(name))
    } else {
        sources::find(dirs, name)
    };
    if let Some(entry) = found {
        return entry;
    }
    let fallback = if dark { DEFAULT_DARK } else { DEFAULT_LIGHT };
    notes.push(format!(
        "The theme “{name}” was not found; {fallback} is used."
    ));
    sources::find(dirs, fallback)
        .filter(|e| e.source == Source::BuiltIn)
        .or_else(|| {
            // A custom file may shadow the default; the built-in is the fallback.
            let text = sources::BUILT_IN.iter().find(|(n, _)| *n == fallback)?.1;
            Some(Entry {
                name: fallback.to_owned(),
                source: Source::BuiltIn,
                theme: parse::parse(text).ok()?,
            })
        })
        .expect("the default themes are built in")
}

/// The themes the preferences select now (Ghostty's config is read again
/// in Ghostty mode). Blocking.
pub fn applied(dirs: &ThemeDirs, ghostty_configs: &[PathBuf], prefs: &Prefs) -> Applied {
    let mut notes = Vec::new();
    let mut ghostty_names = None;
    let (light, dark) = match prefs.mode {
        Mode::Single => (prefs.theme.clone(), prefs.theme.clone()),
        Mode::System => (prefs.light_theme.clone(), prefs.dark_theme.clone()),
        Mode::Ghostty => match ghostty::read(ghostty_configs) {
            Some(theme) => {
                let names = (theme.light().to_owned(), theme.dark().to_owned());
                ghostty_names = Some(GhosttyNames {
                    light: names.0.clone(),
                    dark: names.1.clone(),
                });
                if matches!(theme, GhosttyTheme::Single(_)) {
                    tracing::debug!("Ghostty uses one theme for light and dark");
                }
                names
            }
            None => {
                notes.push("Ghostty's config sets no theme; Coda's defaults are used.".to_owned());
                (DEFAULT_LIGHT.to_owned(), DEFAULT_DARK.to_owned())
            }
        },
    };
    let resolve_slot = |name: &str, dark: bool, notes: &mut Vec<String>| {
        let entry = find_or_default(dirs, name, dark, notes);
        resolve(&entry, prefs, &Edits::new()).expect("no edits cannot fail")
    };
    // One theme for both (single mode): a missing one falls back to Coda.
    let light_theme = resolve_slot(&light, dark == light, &mut notes);
    let dark_theme = if dark == light {
        light_theme.clone()
    } else {
        resolve_slot(&dark, true, &mut notes)
    };
    notes.dedup();
    Applied {
        light: light_theme,
        dark: dark_theme,
        ghostty: ghostty_names,
        notes,
    }
}

// ----- custom themes (the user's folder only) -----

fn custom_dir(dirs: &ThemeDirs) -> Result<&PathBuf, String> {
    dirs.custom.as_ref().ok_or_else(|| {
        "There is no config folder for themes; set HOME or XDG_CONFIG_HOME".to_owned()
    })
}

fn check_new_name(dirs: &ThemeDirs, name: &str, replacing: Option<&str>) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!(
            "A theme name needs 1–{} characters, without / or \\, not starting with a dot",
            sources::MAX_NAME_CHARS
        ));
    }
    if replacing == Some(name) {
        return Ok(());
    }
    let dir = custom_dir(dirs)?;
    // Only the letter case changes (case-insensitive file systems).
    if let Some(old) = replacing
        && same_file(&dir.join(old), &dir.join(name))
    {
        return Ok(());
    }
    if dir.join(name).exists() {
        return Err(format!("There is already a custom theme called “{name}”"));
    }
    if let Some(other) = sources::find(dirs, name) {
        let whose = match other.source {
            Source::BuiltIn => "a built-in theme",
            Source::Ghostty => "a Ghostty theme",
            Source::Custom => "a custom theme",
        };
        return Err(format!(
            "“{name}” is the name of {whose}; choose another name"
        ));
    }
    Ok(())
}

/// Whether two paths name the same existing file.
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

fn custom_entry(dirs: &ThemeDirs, name: &str) -> Result<Entry, String> {
    let dir = custom_dir(dirs)?;
    if !valid_name(name) || !dir.join(name).is_file() {
        return Err(format!("“{name}” is not a custom theme"));
    }
    sources::find(dirs, name)
        .filter(|e| e.source == Source::Custom)
        .ok_or_else(|| format!("The custom theme “{name}” could not be read"))
}

fn write_theme(
    dirs: &ThemeDirs,
    name: &str,
    theme: &ThemeFile,
    based_on: &str,
) -> Result<(), String> {
    let dir = custom_dir(dirs)?;
    let text = parse::write(theme, Some(based_on));
    // A symlink in the custom folder is replaced, never its target.
    fsio::write_atomic(&dir.join(name), text.as_bytes())
        .map_err(|e| format!("Could not save the theme: {e}"))
}

/// Points the preferences at `name` (the slot matching its scheme in
/// system mode; Ghostty mode switches to single).
fn select(prefs: &mut Prefs, name: &str, scheme: Scheme) {
    match prefs.mode {
        Mode::System => match scheme {
            Scheme::Light => prefs.light_theme = name.to_owned(),
            Scheme::Dark => prefs.dark_theme = name.to_owned(),
        },
        Mode::Single | Mode::Ghostty => {
            prefs.mode = Mode::Single;
            prefs.theme = name.to_owned();
        }
    }
}

/// Saves `base` with `edits` as the custom theme `name` and selects it.
/// `replacing` is the custom theme being edited (renamed if `name`
/// differs). Blocking.
pub fn save_custom(
    dirs: &ThemeDirs,
    prefs: &mut Prefs,
    base: &str,
    name: &str,
    edits: &Edits,
    replacing: Option<&str>,
) -> Result<(), String> {
    if let Some(old) = replacing {
        custom_entry(dirs, old)?;
    }
    check_new_name(dirs, name, replacing)?;
    let entry =
        sources::find(dirs, base).ok_or_else(|| format!("The theme “{base}” was not found"))?;
    let (theme, _) = edited(&entry, prefs, edits)?;
    let scheme = Scheme::of(theme.background);
    let dir = custom_dir(dirs)?;
    let renamed = replacing.filter(|old| *old != name);
    if let Some(old) = renamed
        && same_file(&dir.join(old), &dir.join(name))
    {
        // A case-only rename: rename first, or the write and the removal
        // of the old name would hit the same file.
        std::fs::rename(dir.join(old), dir.join(name))
            .map_err(|e| format!("Could not rename the theme: {e}"))?;
    }
    write_theme(dirs, name, &theme, base)?;
    if let Some(old) = renamed {
        if !same_file(&dir.join(old), &dir.join(name)) {
            let _ = std::fs::remove_file(dir.join(old));
        }
        prefs.rename_theme(old, name);
    }
    // The accent is in the file now.
    prefs.accents.remove(name);
    select(prefs, name, scheme);
    Ok(())
}

/// Renames a custom theme (and the preferences' references). Blocking.
pub fn rename_custom(
    dirs: &ThemeDirs,
    prefs: &mut Prefs,
    from: &str,
    to: &str,
) -> Result<(), String> {
    custom_entry(dirs, from)?;
    if from == to {
        return Ok(());
    }
    check_new_name(dirs, to, Some(from))?;
    let dir = custom_dir(dirs)?;
    std::fs::rename(dir.join(from), dir.join(to))
        .map_err(|e| format!("Could not rename the theme: {e}"))?;
    prefs.rename_theme(from, to);
    Ok(())
}

/// Copies any theme into a new custom theme `to`. Blocking.
pub fn duplicate(dirs: &ThemeDirs, prefs: &Prefs, name: &str, to: &str) -> Result<(), String> {
    check_new_name(dirs, to, None)?;
    let entry =
        sources::find(dirs, name).ok_or_else(|| format!("The theme “{name}” was not found"))?;
    let (theme, _) = edited(&entry, prefs, &Edits::new())?;
    write_theme(dirs, to, &theme, name)
}

/// Deletes a custom theme. References to it go back to the defaults,
/// unless another theme of that name shows again (one it shadowed).
/// Blocking.
pub fn delete_custom(dirs: &ThemeDirs, prefs: &mut Prefs, name: &str) -> Result<(), String> {
    custom_entry(dirs, name)?;
    std::fs::remove_file(custom_dir(dirs)?.join(name))
        .map_err(|e| format!("Could not delete the theme: {e}"))?;
    if sources::find(dirs, name).is_some() {
        return Ok(());
    }
    let defaults = Prefs::default();
    for (slot, default) in [
        (&mut prefs.theme, &defaults.theme),
        (&mut prefs.light_theme, &defaults.light_theme),
        (&mut prefs.dark_theme, &defaults.dark_theme),
    ] {
        if slot == name {
            *slot = default.clone();
        }
    }
    prefs.accents.remove(name);
    Ok(())
}

#[cfg(test)]
mod tests;
