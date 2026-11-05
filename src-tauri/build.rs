fn main() {
    // Our commands, declared so each needs an explicit `allow-<command>`
    // permission in `capabilities/` (nothing is allowed by default).
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
        "local_artist_tracks",
        "local_album",
        "local_folder",
        "local_folder_tracks",
        "local_playlists",
        "local_playlist",
        "local_search",
        "navidrome_artists",
        "navidrome_artist",
        "navidrome_artist_tracks",
        "navidrome_album",
        "navidrome_albums",
        "navidrome_playlists",
        "navidrome_playlist",
        "navidrome_random_songs",
        "navidrome_search",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to run tauri-build");
}
