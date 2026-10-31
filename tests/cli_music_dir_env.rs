//! `Cli::try_parse_with_source` with `CODA_MUSIC_DIR` set. In a test binary
//! of its own, with a single test, because it changes the process
//! environment.

use coda_player::config::{Cli, MUSIC_DIR_ENV, MusicDirArg};

#[test]
fn music_dir_from_the_environment_is_reported_as_such() {
    // SAFETY: the only test in this binary, so no other thread reads the
    // environment while it changes.
    unsafe { std::env::set_var(MUSIC_DIR_ENV, "/from-env") };
    let (cli, source) = Cli::try_parse_with_source(["coda", "--setup"]).unwrap();
    assert_eq!(cli.music_dir.as_deref(), Some("/from-env".as_ref()));
    assert_eq!(source, Some(MusicDirArg::Env));
    // The argument still wins, and says so.
    let (cli, source) = Cli::try_parse_with_source(["coda", "/from-arg"]).unwrap();
    assert_eq!(cli.music_dir.as_deref(), Some("/from-arg".as_ref()));
    assert_eq!(source, Some(MusicDirArg::Argument));
    // An empty value is no folder.
    unsafe { std::env::set_var(MUSIC_DIR_ENV, "") };
    let (_, source) = Cli::try_parse_with_source(["coda"]).unwrap();
    assert_eq!(source, None);
}
