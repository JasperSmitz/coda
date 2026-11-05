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
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to run tauri-build");
}
