fn main() {
    // Our commands, declared so each needs an explicit `allow-<command>`
    // permission in `capabilities/` (nothing is allowed by default). Keep
    // in step with `generate_handler!` in `src/lib.rs` and
    // `capabilities/default.json`: `tests/acl.rs` checks all three.
    let manifest = tauri_build::AppManifest::new().commands(&[
        "intent",
        "snapshot",
        "settings_get",
        "settings_test_server",
        "settings_save_server",
        "settings_disconnect",
        "settings_pick_folder",
        "settings_set_music_dir",
        "settings_set_options",
        "onboarding_finish",
        // Browse (read-only).
        "local_artists",
        "local_albums",
        "local_artist",
        "local_album",
        "local_folder",
        "local_playlists",
        "local_playlist",
        "local_search",
        "local_lyrics_search",
        "navidrome_artists",
        "navidrome_artist",
        "navidrome_artist_tracks",
        "navidrome_album",
        "navidrome_albums",
        "navidrome_playlists",
        "navidrome_playlist",
        "navidrome_random_songs",
        "navidrome_search",
        // Navidrome playlist edits.
        "playlist_create",
        "playlist_save_queue",
        "playlist_add",
        "playlist_rename",
        "playlist_delete",
        "playlist_remove",
        "playlist_reorder",
        // Appearance (themes, the GUI's own preferences).
        "appearance_get",
        "appearance_set",
        "theme_list",
        "theme_resolve",
        "theme_save_custom",
        "theme_rename",
        "theme_duplicate",
        "theme_delete",
        "theme_reveal_folder",
        "theme_window_background",
        // The browsed source (settings.toml) and the Home view.
        "ui_source_get",
        "ui_source_set",
        "home_history",
        "home_daily_picks",
        "now_playing_lyrics",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to run tauri-build");
}
