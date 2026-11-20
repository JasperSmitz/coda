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
        "local_genres",
        "local_genre",
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
        "navidrome_genres",
        "navidrome_genre_albums",
        "navidrome_genre_random_songs",
        "navidrome_playlists",
        "navidrome_playlist",
        "navidrome_random_songs",
        "navidrome_search",
        "navidrome_lyrics_search",
        // Navidrome playlist edits.
        "playlist_create",
        "playlist_save_queue",
        "playlist_add",
        "playlist_rename",
        "playlist_delete",
        "playlist_remove",
        "playlist_reorder",
        // Preferences (settings.toml: the accent, Library's default view).
        "prefs_get",
        "prefs_set_accent",
        "prefs_set_library_view",
        "prefs_set_start_screen",
        // The browsed source (settings.toml) and the Home view.
        "ui_source_get",
        "ui_source_set",
        "home_history",
        "home_moods",
        // An artist's most played songs (the play history).
        "artist_most_played",
        "now_playing_lyrics",
        // The Navidrome lyrics index (Settings › Navidrome).
        "lyrics_index_status",
        "lyrics_index_rebuild",
        "lyrics_index_set_enabled",
        // Mixes: the shared mixes file and the editor's searches.
        "list_mixes",
        "add_mix",
        "update_mix",
        "add_to_mix",
        "delete_mix",
        "mix_search_genres",
        "mix_search_artists",
        "mix_search_albums",
        // The window's real full-screen state (`frame`).
        "window_is_fullscreen",
        // The covers' palettes (music colours).
        "cover_palettes",
        // Development builds only (the command is compiled only with
        // `debug_assertions`): trace lines for the log (2.0 R3).
        "debug_trace",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to run tauri-build");
}
