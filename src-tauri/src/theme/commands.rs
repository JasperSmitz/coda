//! Tauri commands for Settings → Appearance. File work runs on blocking
//! threads; one change at a time (the preferences lock), each saved
//! before it answers. Errors are messages for the UI.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use super::prefs::{self, Prefs};
use super::sources::{self, ThemeDirs};
use super::{Appearance, AppearanceDto, Edits, ResolvedTheme, ThemeSummary};

type Result<T> = std::result::Result<T, String>;

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("The theme work failed: {e}"))
}

struct Shared {
    dirs: ThemeDirs,
    configs: Vec<PathBuf>,
    prefs_path: Option<PathBuf>,
}

fn shared(appearance: &Appearance) -> Arc<Shared> {
    Arc::new(Shared {
        dirs: appearance.dirs.clone(),
        configs: appearance.ghostty_configs.clone(),
        prefs_path: appearance.prefs_path.clone(),
    })
}

fn dto(shared: &Shared, prefs: &Prefs) -> AppearanceDto {
    AppearanceDto {
        prefs: prefs.clone(),
        applied: super::applied(&shared.dirs, &shared.configs, prefs),
        themes_folder: shared.dirs.custom.as_ref().map(|d| d.display().to_string()),
    }
}

/// Runs `change` on a copy of the preferences (blocking), saves them if it
/// succeeded, and answers with the new state.
async fn change(
    appearance: &Appearance,
    change: impl FnOnce(&ThemeDirs, &mut Prefs) -> Result<()> + Send + 'static,
) -> Result<AppearanceDto> {
    let mut state = appearance.state.lock().await;
    let shared = shared(appearance);
    let (mut next, bad) = state.clone();
    let (next, result) = blocking(move || {
        let result = change(&shared.dirs, &mut next).and_then(|()| {
            next = next.clone().sanitized();
            match &shared.prefs_path {
                Some(path) => prefs::save(path, &next, bad)
                    .map_err(|e| format!("Could not save the settings: {e}")),
                None => Err("There is no config folder; set HOME or XDG_CONFIG_HOME".into()),
            }
        });
        let answer = result.map(|()| dto(&shared, &next));
        (next, answer)
    })
    .await?;
    if result.is_ok() {
        *state = (next, false);
    }
    result
}

/// The preferences and the themes they select now.
#[tauri::command]
pub async fn appearance_get(appearance: State<'_, Appearance>) -> Result<AppearanceDto> {
    let prefs = appearance.state.lock().await.0.clone();
    let shared = shared(&appearance);
    blocking(move || dto(&shared, &prefs)).await
}

/// Saves new preferences (mode, theme names, accents, visualizer).
#[tauri::command]
pub async fn appearance_set(
    appearance: State<'_, Appearance>,
    prefs: Prefs,
) -> Result<AppearanceDto> {
    change(&appearance, move |_, current| {
        // The browsed source is not an appearance setting.
        let source = current.source;
        *current = prefs;
        current.source = source;
        Ok(())
    })
    .await
}

/// Every theme, for the picker.
#[tauri::command]
pub async fn theme_list(appearance: State<'_, Appearance>) -> Result<Vec<ThemeSummary>> {
    let prefs = appearance.state.lock().await.0.clone();
    let dirs = appearance.dirs.clone();
    blocking(move || super::list(&dirs, &prefs)).await
}

/// One theme with the saved accent and `edits`, for previews.
#[tauri::command]
pub async fn theme_resolve(
    appearance: State<'_, Appearance>,
    name: String,
    edits: Option<Edits>,
) -> Result<ResolvedTheme> {
    let prefs = appearance.state.lock().await.0.clone();
    let dirs = appearance.dirs.clone();
    blocking(move || {
        let entry = sources::find(&dirs, &name)
            .ok_or_else(|| format!("The theme “{name}” was not found"))?;
        super::resolve(&entry, &prefs, &edits.unwrap_or_default())
    })
    .await?
}

/// Saves the colour editor's work as a custom theme and selects it.
#[tauri::command]
pub async fn theme_save_custom(
    appearance: State<'_, Appearance>,
    base: String,
    name: String,
    edits: Edits,
    replacing: Option<String>,
) -> Result<AppearanceDto> {
    change(&appearance, move |dirs, prefs| {
        super::save_custom(dirs, prefs, &base, &name, &edits, replacing.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn theme_rename(
    appearance: State<'_, Appearance>,
    from: String,
    to: String,
) -> Result<AppearanceDto> {
    change(&appearance, move |dirs, prefs| {
        super::rename_custom(dirs, prefs, &from, &to)
    })
    .await
}

#[tauri::command]
pub async fn theme_duplicate(
    appearance: State<'_, Appearance>,
    name: String,
    to: String,
) -> Result<AppearanceDto> {
    change(&appearance, move |dirs, prefs| {
        super::duplicate(dirs, prefs, &name, &to)
    })
    .await
}

#[tauri::command]
pub async fn theme_delete(
    appearance: State<'_, Appearance>,
    name: String,
) -> Result<AppearanceDto> {
    change(&appearance, move |dirs, prefs| {
        super::delete_custom(dirs, prefs, &name)
    })
    .await
}

/// Sets the main window's native background (behind the webview) to the
/// applied theme's background, `#rrggbb`, so resizes and the next start
/// show no other colour.
#[tauri::command]
pub async fn theme_window_background(window: tauri::WebviewWindow, color: String) -> Result<()> {
    let c = super::color::Rgba::parse_rgb(&color)
        .ok_or_else(|| format!("`{color}` is not a hex colour"))?;
    window
        .set_background_color(Some(tauri::window::Color(c.r, c.g, c.b, 255)))
        .map_err(|e| e.to_string())
}

/// Opens the custom themes folder (created if needed) in the file
/// manager. Only that folder: no path comes from the webview.
#[tauri::command]
pub async fn theme_reveal_folder(appearance: State<'_, Appearance>) -> Result<()> {
    let dir = appearance
        .dirs
        .custom
        .clone()
        .ok_or_else(|| "There is no config folder for themes".to_owned())?;
    blocking(move || {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let mut child = std::process::Command::new(opener)
            .arg(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("Could not open the folder ({opener}): {e}"))?;
        // Reaped off-thread; the file manager may outlive it.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    })
    .await?
}

/// The source the sidebar browses, as saved (`null`: never chosen).
#[tauri::command]
pub async fn ui_source_get(
    appearance: State<'_, Appearance>,
) -> Result<Option<crate::history::SourceKind>> {
    Ok(appearance.state.lock().await.0.source)
}

/// Saves the source the sidebar browses (the file's other settings stay).
#[tauri::command]
pub async fn ui_source_set(
    appearance: State<'_, Appearance>,
    source: crate::history::SourceKind,
) -> Result<()> {
    let mut state = appearance.state.lock().await;
    if state.0.source == Some(source) {
        return Ok(());
    }
    let Some(path) = appearance.prefs_path.clone() else {
        return Err("There is no config folder; set HOME or XDG_CONFIG_HOME".into());
    };
    let (mut next, bad) = state.clone();
    next.source = Some(source);
    let saved = next.clone();
    blocking(move || prefs::save(&path, &saved, bad))
        .await?
        .map_err(|e| format!("Could not save the settings: {e}"))?;
    *state = (next, false);
    Ok(())
}
