//! End-to-end checks of the `coda` binary's startup validation.

use std::process::{Command, Output};

/// Runs `coda` with a private state dir and config dir (so the user's
/// own config file and `CODA_*` variables never leak in). Write a config
/// file to `<state>/config/coda/config.toml` before calling, via
/// [`write_config`].
fn coda(args: &[&str], env: &[(&str, &str)], path: Option<&str>) -> (Output, tempfile::TempDir) {
    let state = tempfile::tempdir().unwrap();
    coda_in(state, args, env, path)
}

fn coda_in(
    state: tempfile::TempDir,
    args: &[&str],
    env: &[(&str, &str)],
    path: Option<&str>,
) -> (Output, tempfile::TempDir) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coda"));
    command
        .args(args)
        .env_remove("CODA_MUSIC_DIR")
        .env_remove("CODA_CONFIG")
        .env_remove("CODA_NAVIDROME_URL")
        .env_remove("CODA_NAVIDROME_USER")
        .env_remove("CODA_NAVIDROME_PASSWORD")
        .env("XDG_STATE_HOME", state.path())
        .env("XDG_CONFIG_HOME", state.path().join("config"));
    if let Some(path) = path {
        command.env("PATH", path);
    }
    for (key, value) in env {
        command.env(key, value);
    }
    (command.output().unwrap(), state)
}

fn write_config(state: &tempfile::TempDir, text: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let dir = state.path().join("config").join("coda");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn empty_env_var_is_a_usage_error() {
    let (output, _state) = coda(&[], &[("CODA_MUSIC_DIR", "")], None);
    assert_eq!(output.status.code(), Some(2));
    let err = stderr(&output);
    assert!(err.contains("no music source configured"), "{err}");
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

#[test]
fn partial_navidrome_config_is_a_usage_error() {
    let state = tempfile::tempdir().unwrap();
    write_config(&state, "[navidrome]\nurl = \"https://h.example\"\n", 0o600);
    let (output, _state) = coda_in(state, &[], &[], None);
    assert_eq!(output.status.code(), Some(2));
    let err = stderr(&output);
    assert!(err.contains("username is missing"), "{err}");
    assert!(!err.contains("h.example"), "{err}");
}

#[test]
fn world_readable_password_file_exits_1() {
    let state = tempfile::tempdir().unwrap();
    write_config(
        &state,
        "[navidrome]\nurl = \"https://h\"\nusername = \"u\"\npassword = \"hunter2\"\n",
        0o644,
    );
    let (output, _state) = coda_in(state, &[], &[], None);
    assert_eq!(output.status.code(), Some(1));
    let err = stderr(&output);
    assert!(err.contains("chmod 600"), "{err}");
    assert!(!err.contains("hunter2"), "{err}");
}

#[test]
fn failing_password_command_exits_1() {
    let state = tempfile::tempdir().unwrap();
    write_config(
        &state,
        "[navidrome]\nurl = \"https://h\"\nusername = \"u\"\npassword_command = [\"false\"]\n",
        0o644,
    );
    let (output, _state) = coda_in(state, &[], &[], None);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("password_command failed"));
}

#[test]
fn navidrome_from_env_alone_is_a_valid_source() {
    // Config passes; startup then stops at the missing mpv (no TUI, no
    // network: the ping only happens inside the TUI).
    let empty_path = tempfile::tempdir().unwrap();
    let (output, _state) = coda(
        &[],
        &[
            ("CODA_NAVIDROME_URL", "http://127.0.0.1:9"),
            ("CODA_NAVIDROME_USER", "u"),
            ("CODA_NAVIDROME_PASSWORD", "p"),
        ],
        Some(empty_path.path().to_str().unwrap()),
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stderr(&output).contains("mpv was not found"));
}

#[test]
fn without_a_terminal_setup_does_not_run_and_nothing_is_written() {
    // `Command::output` gives the child no terminal on stdin or stdout.
    let (output, state) = coda(&[], &[], None);
    assert_eq!(output.status.code(), Some(2));
    let err = stderr(&output);
    assert!(err.contains("no music source configured"), "{err}");
    assert!(output.stdout.is_empty());
    assert!(!state.path().join("config").exists());
}

#[test]
fn setup_flag_without_a_terminal_is_a_usage_error() {
    // With a working config, and with a password_command that must never
    // run: `--setup` stops before reading anything.
    let state = tempfile::tempdir().unwrap();
    let marker = state.path().join("ran");
    let text = format!(
        "[navidrome]\nurl = \"https://h.example\"\nusername = \"u\"\n\
         password_command = [\"touch\", {:?}]\n",
        marker.to_str().unwrap()
    );
    write_config(&state, &text, 0o600);
    let (output, state) = coda_in(state, &["--setup"], &[], None);
    assert_eq!(output.status.code(), Some(2));
    let err = stderr(&output);
    assert!(err.contains("--setup needs a terminal"), "{err}");
    assert!(output.stdout.is_empty());
    assert!(!marker.exists(), "password_command ran");
    let written = std::fs::read_to_string(state.path().join("config/coda/config.toml")).unwrap();
    assert_eq!(written, text, "the config is unchanged");
}

#[test]
fn setup_flag_is_listed_in_help() {
    let (output, _state) = coda(&["--help"], &[], None);
    assert_eq!(output.status.code(), Some(0));
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--setup"), "{help}");
}

#[test]
fn setup_flag_checks_the_given_folder_first() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope");
    let missing = missing.to_str().unwrap();
    for (args, env) in [
        (vec!["--setup", missing], vec![]),
        (vec!["--setup"], vec![("CODA_MUSIC_DIR", missing)]),
    ] {
        let (output, _state) = coda(&args, &env, None);
        assert_eq!(output.status.code(), Some(1), "{args:?} {env:?}");
        let err = stderr(&output);
        assert!(err.contains("music directory does not exist"), "{err}");
    }
}
