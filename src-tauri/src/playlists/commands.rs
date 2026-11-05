//! Playlist edit commands. Each validates its input, waits for earlier
//! edits ([`PlaylistEditor`]), runs, shows its outcome as a notice and
//! returns it, so the UI can reload the playlist and the list.

use coda_core::navidrome::edit::EditOp;
use tauri::State;

use super::dto::EditOutcomeDto;
use super::*;
use crate::browse::BrowseHandle;
use crate::engine::EngineHandle;

type Result<T> = std::result::Result<T, String>;

/// Runs `edit` (or reports why it was refused) with the current client.
pub async fn submit(
    editor: &PlaylistEditor,
    browse: &BrowseHandle,
    edit: std::result::Result<Edit, String>,
) -> EditOutcomeDto {
    let edit = match edit {
        Ok(edit) => edit,
        Err(message) => return refused(message),
    };
    let Ok((client, generation)) = browse.client() else {
        return refused(NO_SERVER);
    };
    editor
        .run(&client, edit, || browse.client_generation() == generation)
        .await
}

async fn finish(
    editor: &PlaylistEditor,
    browse: &BrowseHandle,
    engine: &EngineHandle,
    edit: std::result::Result<Edit, String>,
) -> Result<EditOutcomeDto> {
    let outcome = submit(editor, browse, edit).await;
    engine.notify(outcome.message.clone()).await;
    Ok(outcome)
}

fn edit(op: EditOp, playlist: String) -> Edit {
    Edit {
        op,
        playlist,
        old_name: String::new(),
        left_out: 0,
    }
}

/// A new playlist named `name` holding `songs` (may be empty).
pub fn create_edit(name: &str, songs: Vec<String>) -> std::result::Result<Edit, String> {
    let name = clean_name(name)?;
    check_ids(&songs, "song")?;
    Ok(edit(
        EditOp::Create {
            name: name.clone(),
            songs,
        },
        name,
    ))
}

#[tauri::command]
pub async fn playlist_create(
    editor: State<'_, PlaylistEditor>,
    browse: State<'_, BrowseHandle>,
    engine: State<'_, EngineHandle>,
    name: String,
    songs: Vec<String>,
) -> Result<EditOutcomeDto> {
    finish(&editor, &browse, &engine, create_edit(&name, songs)).await
}

/// "Save queue as playlist": the queue's Navidrome songs, in queue order;
/// local tracks are left out (as in Coda).
#[tauri::command]
pub async fn playlist_save_queue(
    editor: State<'_, PlaylistEditor>,
    browse: State<'_, BrowseHandle>,
    engine: State<'_, EngineHandle>,
    name: String,
) -> Result<EditOutcomeDto> {
    let edit = match engine.queue_songs().await {
        Ok((songs, _)) if songs.is_empty() => Err(NO_REMOTE_IN_QUEUE.to_owned()),
        Ok((songs, local)) => create_edit(&name, songs).map(|mut edit| {
            edit.left_out = local;
            edit
        }),
        Err(gone) => Err(gone.to_string()),
    };
    finish(&editor, &browse, &engine, edit).await
}

#[tauri::command]
pub async fn playlist_add(
    editor: State<'_, PlaylistEditor>,
    browse: State<'_, BrowseHandle>,
    engine: State<'_, EngineHandle>,
    playlist_id: String,
    name: String,
    songs: Vec<String>,
) -> Result<EditOutcomeDto> {
    let edit = (|| {
        check_playlist_id(&playlist_id)?;
        check_ids(&songs, "song")?;
        if songs.is_empty() {
            return Err("Nothing to add".to_owned());
        }
        Ok(edit(EditOp::Add { playlist_id, songs }, shown_name(&name)))
    })();
    finish(&editor, &browse, &engine, edit).await
}

#[tauri::command]
pub async fn playlist_rename(
    editor: State<'_, PlaylistEditor>,
    browse: State<'_, BrowseHandle>,
    engine: State<'_, EngineHandle>,
    playlist_id: String,
    old_name: String,
    name: String,
) -> Result<EditOutcomeDto> {
    let edit = (|| {
        check_playlist_id(&playlist_id)?;
        let name = clean_name(&name)?;
        Ok(Edit {
            op: EditOp::Rename {
                playlist_id,
                name: name.clone(),
            },
            playlist: name,
            old_name: shown_name(&old_name),
            left_out: 0,
        })
    })();
    finish(&editor, &browse, &engine, edit).await
}

#[tauri::command]
pub async fn playlist_delete(
    editor: State<'_, PlaylistEditor>,
    browse: State<'_, BrowseHandle>,
    engine: State<'_, EngineHandle>,
    playlist_id: String,
    name: String,
) -> Result<EditOutcomeDto> {
    let edit = check_playlist_id(&playlist_id)
        .map(|()| edit(EditOp::Delete { playlist_id }, shown_name(&name)));
    finish(&editor, &browse, &engine, edit).await
}

/// Removes the entries at `indices` of `expected` (the song ids as shown),
/// if the server still holds exactly that list; `hidden` is what the user
/// confirmed losing.
pub fn remove_edit(
    playlist_id: String,
    name: &str,
    expected: Vec<String>,
    indices: &[usize],
    hidden: HiddenDto,
) -> std::result::Result<Edit, String> {
    check_playlist_id(&playlist_id)?;
    check_ids(&expected, "song")?;
    let op = removal(playlist_id, expected, indices, hidden_bound(hidden))?;
    Ok(edit(op, shown_name(name)))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn playlist_remove(
    editor: State<'_, PlaylistEditor>,
    browse: State<'_, BrowseHandle>,
    engine: State<'_, EngineHandle>,
    playlist_id: String,
    name: String,
    expected: Vec<String>,
    indices: Vec<usize>,
    hidden: HiddenDto,
) -> Result<EditOutcomeDto> {
    let edit = remove_edit(playlist_id, &name, expected, &indices, hidden);
    finish(&editor, &browse, &engine, edit).await
}

/// Saves `order` (a permutation of `expected`'s indices).
pub fn reorder_edit(
    playlist_id: String,
    name: &str,
    expected: Vec<String>,
    order: &[usize],
    hidden: HiddenDto,
) -> std::result::Result<Option<Edit>, String> {
    check_playlist_id(&playlist_id)?;
    check_ids(&expected, "song")?;
    let op = reorder(playlist_id, expected, order, hidden_bound(hidden))?;
    Ok(op.map(|op| edit(op, shown_name(name))))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn playlist_reorder(
    editor: State<'_, PlaylistEditor>,
    browse: State<'_, BrowseHandle>,
    engine: State<'_, EngineHandle>,
    playlist_id: String,
    name: String,
    expected: Vec<String>,
    order: Vec<usize>,
    hidden: HiddenDto,
) -> Result<EditOutcomeDto> {
    match reorder_edit(playlist_id, &name, expected, &order, hidden) {
        // The shown order: nothing to save, nothing to say.
        Ok(None) => Ok(EditOutcomeDto {
            status: super::dto::EditStatus::Done,
            message: String::new(),
            created: None,
        }),
        Ok(Some(edit)) => finish(&editor, &browse, &engine, Ok(edit)).await,
        Err(message) => finish(&editor, &browse, &engine, Err(message)).await,
    }
}
