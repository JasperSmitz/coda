//! End-to-end checks of the `coda` binary's startup validation.

use std::process::{Command, Output};

fn coda(args: &[&str], env: &[(&str, &str)], path: Option<&str>) -> (Output, tempfile::TempDir) {
    let state = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_coda"));
    command
        .args(args)
        .env_remove("CODA_MUSIC_DIR")
        .env("XDG_STATE_HOME", state.path());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    for (key, value) in env {
        command.env(key, value);
    }
    (command.output().unwrap(), state)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn empty_env_var_is_a_usage_error() {
    let (output, _state) = coda(&[], &[("CODA_MUSIC_DIR", "")], None);
    assert_eq!(output.status.code(), Some(2));
    let err = stderr(&output);
    assert!(err.contains("no music directory given"), "{err}");
    assert!(err.contains("Usage: coda [MUSIC_DIR]"), "{err}");
}

#[test]
fn unset_is_a_usage_error() {
    let (output, _state) = coda(&[], &[], None);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("CODA_MUSIC_DIR"));
}

#[test]
fn missing_directory_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope");
    let (output, _state) = coda(&[missing.to_str().unwrap()], &[], None);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("does not exist"));
}

#[test]
fn argument_wins_over_env_var() {
    let dir = tempfile::tempdir().unwrap();
    let (output, _state) = coda(
        &[dir.path().join("nope").to_str().unwrap()],
        &[("CODA_MUSIC_DIR", dir.path().to_str().unwrap())],
        None,
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
}

#[test]
fn missing_mpv_gives_install_hint() {
    let music = tempfile::tempdir().unwrap();
    let empty_path = tempfile::tempdir().unwrap();
    let (output, _state) = coda(
        &[music.path().to_str().unwrap()],
        &[],
        Some(empty_path.path().to_str().unwrap()),
    );
    assert_eq!(output.status.code(), Some(1));
    let err = stderr(&output);
    assert!(err.contains("mpv was not found"), "{err}");
    assert!(err.contains("brew install mpv"), "{err}");
}
