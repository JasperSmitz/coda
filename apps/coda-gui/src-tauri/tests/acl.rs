//! Every command in `generate_handler!` (`src/lib.rs`) must also be
//! declared in `build.rs`'s app manifest and granted in
//! `capabilities/default.json`, and nothing else may be: a command
//! missing from either is rejected at runtime ("not allowed"), which no
//! test that calls the command's function directly can see.

use std::collections::BTreeSet;

const LIB: &str = include_str!("../src/lib.rs");
const BUILD: &str = include_str!("../build.rs");
const CAPABILITIES: &str = include_str!("../capabilities/default.json");

/// The command names in `generate_handler![…]` (last path segment).
fn handler_commands() -> BTreeSet<String> {
    let start = LIB
        .find("generate_handler![")
        .expect("generate_handler! in lib.rs");
    let body = &LIB[start + "generate_handler![".len()..];
    // Up to the macro's `])` (an item may carry an attribute, `#[cfg(…)]`).
    let body = &body[..body.find("])").expect("closing ])")];
    body.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            // Without its attributes.
            let path = item.rsplit(']').next().unwrap();
            path.rsplit("::").next().unwrap().trim().to_owned()
        })
        .collect()
}

/// The quoted names in `AppManifest::new().commands(&[…])`.
fn manifest_commands() -> BTreeSet<String> {
    let start = BUILD
        .find(".commands(&[")
        .expect("commands list in build.rs");
    let body = &BUILD[start..];
    let body = &body[..body.find("]);").expect("end of the commands list")];
    body.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//"))
        .filter_map(|line| line.strip_prefix('"')?.split('"').next())
        .map(str::to_owned)
        .collect()
}

/// The commands granted by `allow-<command>` (dashes to underscores).
fn granted_commands() -> BTreeSet<String> {
    let json: serde_json::Value = serde_json::from_str(CAPABILITIES).unwrap();
    json["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p.as_str())
        .filter(|p| !p.contains(':'))
        .map(|p| {
            p.strip_prefix("allow-")
                .unwrap_or_else(|| panic!("unexpected app permission {p}"))
                .replace('-', "_")
        })
        .collect()
}

#[test]
fn every_command_is_declared_and_granted() {
    let handler = handler_commands();
    assert!(handler.len() > 40, "parsed {handler:?}");
    assert!(handler.contains("local_search"));
    let manifest = manifest_commands();
    assert_eq!(
        handler.difference(&manifest).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "in generate_handler! but not in build.rs"
    );
    assert_eq!(
        manifest.difference(&handler).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "in build.rs but not in generate_handler!"
    );
    let granted = granted_commands();
    assert_eq!(
        handler.difference(&granted).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "in generate_handler! but not allowed in capabilities/default.json"
    );
    assert_eq!(
        granted.difference(&handler).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "allowed in capabilities/default.json but not a command"
    );
}

/// A capability file and the `core:` permissions (Tauri's own APIs) it
/// grants, exactly.
struct Capability {
    file: &'static str,
    platforms: Option<&'static [&'static str]>,
    core: &'static [&'static str],
}

/// Every capability file grants exactly these `core:` permissions: a
/// wider grant (or a new file) must change this test too.
#[test]
fn core_permissions_are_exactly_these() {
    let expected = [
        Capability {
            file: "default.json",
            platforms: None,
            core: &[
                "core:event:allow-listen",
                "core:event:allow-unlisten",
                "core:window:allow-set-fullscreen",
            ],
        },
        Capability {
            file: "macos.json",
            platforms: Some(&["macOS"]),
            core: &[
                "core:window:allow-internal-toggle-maximize",
                "core:window:allow-start-dragging",
            ],
        },
    ];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    let names: Vec<&str> = expected.iter().map(|c| c.file).collect();
    assert_eq!(files, names, "capability files");
    for Capability {
        file: name,
        platforms,
        core,
    } in expected
    {
        let text = std::fs::read_to_string(dir.join(name)).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            json["windows"],
            serde_json::json!(["main"]),
            "{name}: windows"
        );
        let found_platforms = json.get("platforms").map(|p| {
            p.as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            found_platforms,
            platforms.map(|p| p.iter().map(|s| s.to_string()).collect()),
            "{name}: platforms"
        );
        let granted: Vec<&str> = json["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().expect("permissions are plain names"))
            .collect();
        let found: BTreeSet<&str> = granted
            .iter()
            .copied()
            .filter(|p| p.contains(':'))
            .collect();
        let core: BTreeSet<&str> = core.iter().copied().collect();
        assert_eq!(found, core, "{name}: core permissions");
        if name != "default.json" {
            assert!(
                granted.iter().all(|p| p.contains(':')),
                "{name}: app commands belong in default.json"
            );
        }
    }
}
