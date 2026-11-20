//! Tauri commands for Preferences and the browsed source. One change at
//! a time (the preferences lock), each saved on a blocking thread before
//! it answers. Errors are messages for the UI.

use tauri::State;

use super::{Accent, LibraryView, Prefs, PrefsDto, PrefsStore, StartScreen};
use crate::history::SourceKind;

type Result<T> = std::result::Result<T, String>;

/// The preferences and what Preferences shows with them.
#[tauri::command]
pub async fn prefs_get(store: State<'_, PrefsStore>) -> Result<PrefsDto> {
    Ok(PrefsDto::new(&store.state.lock().await.0))
}

/// Saves the accent: `"music"`, `"house"` or `#rrggbb`. Answers with the
/// new state.
#[tauri::command]
pub async fn prefs_set_accent(store: State<'_, PrefsStore>, accent: String) -> Result<PrefsDto> {
    let accent =
        Accent::parse(&accent).ok_or_else(|| format!("“{accent}” is not a colour; use #rrggbb"))?;
    save_prefs(&store, |prefs| prefs.accent = accent).await?;
    Ok(PrefsDto::new(&store.state.lock().await.0))
}

/// Saves Library's default view: `"covers"` or `"list"`. Answers with
/// the new state.
#[tauri::command]
pub async fn prefs_set_library_view(
    store: State<'_, PrefsStore>,
    view: String,
) -> Result<PrefsDto> {
    let view = LibraryView::parse(&view).ok_or_else(|| format!("“{view}” is not a view"))?;
    save_prefs(&store, |prefs| prefs.library_view = view).await?;
    Ok(PrefsDto::new(&store.state.lock().await.0))
}

/// Saves the screen the app starts on: `"home"`, `"search"` or
/// `"library"`. Answers with the new state.
#[tauri::command]
pub async fn prefs_set_start_screen(
    store: State<'_, PrefsStore>,
    screen: String,
) -> Result<PrefsDto> {
    let screen =
        StartScreen::parse(&screen).ok_or_else(|| format!("“{screen}” is not a screen"))?;
    save_prefs(&store, |prefs| prefs.start_screen = screen).await?;
    Ok(PrefsDto::new(&store.state.lock().await.0))
}

/// The source the app browses, as saved (`null`: never chosen).
#[tauri::command]
pub async fn ui_source_get(store: State<'_, PrefsStore>) -> Result<Option<SourceKind>> {
    Ok(store.state.lock().await.0.source)
}

/// Saves the source the app browses (the file's other settings stay).
#[tauri::command]
pub async fn ui_source_set(store: State<'_, PrefsStore>, source: SourceKind) -> Result<()> {
    save_prefs(&store, |prefs| prefs.source = Some(source)).await
}

/// Changes the preferences with `edit` and saves the file (its other
/// settings stay). Nothing is written when nothing changed.
pub(crate) async fn save_prefs(store: &PrefsStore, edit: impl FnOnce(&mut Prefs)) -> Result<()> {
    let mut state = store.state.lock().await;
    let (mut next, bad) = state.clone();
    edit(&mut next);
    if next == state.0 {
        return Ok(());
    }
    let Some(path) = store.path.clone() else {
        return Err("There is no config folder; set HOME or XDG_CONFIG_HOME".into());
    };
    let saved = next.clone();
    tauri::async_runtime::spawn_blocking(move || super::save(&path, &saved, bad))
        .await
        .map_err(|e| format!("Could not save the settings: {e}"))?
        .map_err(|e| format!("Could not save the settings: {e}"))?;
    *state = (next, false);
    Ok(())
}
