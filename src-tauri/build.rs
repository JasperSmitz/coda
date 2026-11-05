fn main() {
    // Our commands, declared so each needs an explicit `allow-<command>`
    // permission in `capabilities/` (nothing is allowed by default).
    let manifest = tauri_build::AppManifest::new().commands(&["intent", "snapshot"]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to run tauri-build");
}
