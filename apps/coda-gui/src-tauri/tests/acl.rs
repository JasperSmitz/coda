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
    let body = &body[..body.find(']').expect("closing ]")];
    body.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.rsplit("::").next().unwrap().trim().to_owned())
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
