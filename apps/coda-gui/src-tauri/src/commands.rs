//! Tauri commands: thin forwards to the engine actor.

use tauri::State;

use crate::dto::{Intent, Snapshot};
use crate::engine::EngineHandle;

/// Sends one intent to the engine. Errors only while quitting.
#[tauri::command]
pub async fn intent(engine: State<'_, EngineHandle>, intent: Intent) -> Result<(), String> {
    engine.intent(intent).await.map_err(|e| e.to_string())
}

/// The full current state, for the UI's first render (events follow).
#[tauri::command]
pub async fn snapshot(engine: State<'_, EngineHandle>) -> Result<Snapshot, String> {
    engine.snapshot().await.map_err(|e| e.to_string())
}
