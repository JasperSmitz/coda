//! mpv child process: argv, private socket directory, spawn, connect
//! retry and shutdown. mpv is spawned directly (never through a shell)
//! with stdio closed.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::process::{Child, Command};

pub const DEFAULT_PROGRAM: &str = "mpv";
pub const SOCKET_NAME: &str = "mpv.sock";
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
pub const CONNECT_RETRY: Duration = Duration::from_millis(20);
pub const QUIT_GRACE: Duration = Duration::from_millis(500);
/// Unix socket paths are limited to about 104 (macOS) or 108 (Linux)
/// bytes; stay safely below.
pub const MAX_SOCKET_PATH_LEN: usize = 100;
const FALLBACK_BASE_DIR: &str = "/tmp";
/// Exit status meaning "command not found".
const NOT_FOUND_EXIT_CODE: i32 = 127;

#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error(
        "mpv was not found (looked for `{program}` on PATH). Coda needs mpv 0.33 or newer. \
         Install it with `brew install mpv` (macOS) or `sudo pacman -S mpv` (Arch Linux)."
    )]
    NotFound { program: String },
    #[error("could not start mpv: {0}")]
    Spawn(#[source] io::Error),
    #[error("could not create a private directory for the mpv socket: {0}")]
    SocketDir(#[source] io::Error),
    #[error("mpv exited during startup ({0})")]
    ExitedEarly(ExitStatus),
    #[error("could not connect to mpv within {timeout:?}: {source}")]
    Connect {
        timeout: Duration,
        #[source]
        source: io::Error,
    },
}

impl SpawnError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }
}

/// Arguments passed to mpv (excluding the program name).
pub fn mpv_args(socket: &Path) -> Vec<OsString> {
    let mut ipc = OsString::from("--input-ipc-server=");
    ipc.push(socket);
    vec![
        "--no-config".into(),
        "--idle=yes".into(),
        "--no-video".into(),
        "--no-terminal".into(),
        "--ytdl=no".into(),
        // Give up on an unresponsive stream server instead of hanging.
        "--network-timeout=15".into(),
        // Do not follow references inside files: a file whose content is a
        // playlist (e.g. `#EXTM3U` in a `.mp3`) is rejected instead of
        // expanding into entries Coda never requested.
        "--access-references=no".into(),
        ipc,
    ]
}

/// Environment variables that may hold Navidrome credentials or the
/// server URL. mpv does not inherit them.
pub const STRIPPED_ENV: [&str; 3] = [
    "CODA_NAVIDROME_URL",
    "CODA_NAVIDROME_USER",
    "CODA_NAVIDROME_PASSWORD",
];

/// The mpv command: direct exec (no shell), stdio closed, killed on drop,
/// and without the `CODA_NAVIDROME_*` environment.
pub fn mpv_command(program: &OsStr, socket: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .args(mpv_args(socket))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for name in STRIPPED_ENV {
        command.env_remove(name);
    }
    command
}

/// Candidate base directories for the socket directory, in preference
/// order: `$XDG_RUNTIME_DIR`, `$TMPDIR`, then `/tmp`. Empty or relative
/// values are ignored and duplicates removed. Each comes with a label for
/// logs and errors.
pub fn socket_base_dirs(
    xdg_runtime_dir: Option<OsString>,
    tmpdir: Option<OsString>,
) -> Vec<(&'static str, PathBuf)> {
    let mut candidates: Vec<(&'static str, PathBuf)> = Vec::new();
    let env = [("XDG_RUNTIME_DIR", xdg_runtime_dir), ("TMPDIR", tmpdir)];
    let found = env
        .into_iter()
        .filter_map(|(label, value)| Some((label, PathBuf::from(value?))))
        .filter(|(_, path)| path.is_absolute());
    for (label, path) in found.chain([("/tmp", PathBuf::from(FALLBACK_BASE_DIR))]) {
        if !candidates.iter().any(|(_, p)| *p == path) {
            candidates.push((label, path));
        }
    }
    candidates
}

/// A private `0700` directory holding the socket. Removed on drop or
/// [`SocketDir::close`].
#[derive(Debug)]
pub struct SocketDir {
    dir: TempDir,
}

impl SocketDir {
    /// Creates the directory under the first candidate that works. A
    /// candidate is skipped when creating the directory fails (a stale
    /// `XDG_RUNTIME_DIR` after `su`/`sudo -E` is common) or when the socket
    /// path would be too long. Fails only if every candidate fails, naming
    /// the ones tried.
    pub fn create_in_first(candidates: &[(&str, PathBuf)]) -> io::Result<Self> {
        let mut tried = Vec::new();
        for (label, base) in candidates {
            match Self::create_exact(base) {
                Ok(dir) if dir.socket_path().as_os_str().len() < MAX_SOCKET_PATH_LEN => {
                    return Ok(dir);
                }
                Ok(_) => {
                    tracing::debug!(candidate = label, "socket path too long; trying next");
                    tried.push(format!(
                        "{label} ({}): socket path too long",
                        base.display()
                    ));
                }
                Err(error) => {
                    tracing::debug!(candidate = label, %error, "cannot create socket dir; trying next");
                    tried.push(format!("{label} ({}): {error}", base.display()));
                }
            }
        }
        Err(io::Error::other(format!(
            "no usable directory; tried {}",
            tried.join("; ")
        )))
    }

    fn create_exact(base: &Path) -> io::Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("coda-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(base)?;
        // The umask can only narrow the mode, but set it explicitly anyway.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        Ok(Self { dir })
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn socket_path(&self) -> PathBuf {
        self.dir.path().join(SOCKET_NAME)
    }

    pub fn close(self) -> io::Result<()> {
        self.dir.close()
    }
}

/// A running mpv child with its socket directory.
#[derive(Debug)]
pub struct MpvProcess {
    child: Child,
    socket_dir: Option<SocketDir>,
}

impl MpvProcess {
    /// Spawns `program` and connects to its IPC socket, retrying every
    /// [`CONNECT_RETRY`] for up to [`CONNECT_TIMEOUT`], and giving up early
    /// if the child exits.
    pub async fn launch(program: &OsStr) -> Result<(Self, UnixStream), SpawnError> {
        let candidates = socket_base_dirs(
            std::env::var_os("XDG_RUNTIME_DIR"),
            std::env::var_os("TMPDIR"),
        );
        let socket_dir = SocketDir::create_in_first(&candidates).map_err(SpawnError::SocketDir)?;
        let socket = socket_dir.socket_path();

        let child = mpv_command(program, &socket)
            .spawn()
            .map_err(|e| match e.kind() {
                io::ErrorKind::NotFound => SpawnError::NotFound {
                    program: program.to_string_lossy().into_owned(),
                },
                _ => SpawnError::Spawn(e),
            })?;
        let mut process = Self {
            child,
            socket_dir: Some(socket_dir),
        };
        tracing::debug!(pid = ?process.id(), "spawned mpv");

        let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
        loop {
            match UnixStream::connect(&socket).await {
                Ok(stream) => return Ok((process, stream)),
                Err(error) => {
                    if let Ok(Some(status)) = process.child.try_wait() {
                        process.cleanup_dir();
                        // Some environments (seen under Rosetta) report a
                        // missing program as a child exiting with 127, the
                        // shell's "command not found", instead of a spawn
                        // error.
                        if status.code() == Some(NOT_FOUND_EXIT_CODE) {
                            return Err(SpawnError::NotFound {
                                program: program.to_string_lossy().into_owned(),
                            });
                        }
                        return Err(SpawnError::ExitedEarly(status));
                    }
                    if tokio::time::Instant::now() >= deadline {
                        process.kill().await;
                        return Err(SpawnError::Connect {
                            timeout: CONNECT_TIMEOUT,
                            source: error,
                        });
                    }
                    tokio::time::sleep(CONNECT_RETRY).await;
                }
            }
        }
    }

    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    pub fn socket_dir(&self) -> Option<&Path> {
        self.socket_dir.as_ref().map(SocketDir::path)
    }

    /// Waits for the child to exit. Cancel safe.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().await
    }

    /// Kills (if still running) and reaps the child, then removes the
    /// socket dir.
    pub async fn kill(&mut self) {
        if let Err(error) = self.child.start_kill() {
            // Already exited (and possibly reaped) is the common case.
            tracing::debug!(%error, "kill mpv");
        }
        let _ = self.child.wait().await;
        self.cleanup_dir();
    }

    fn cleanup_dir(&mut self) {
        if let Some(dir) = self.socket_dir.take()
            && let Err(error) = dir.close()
        {
            tracing::warn!(%error, "could not remove mpv socket directory");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_contents() {
        let args = mpv_args(Path::new("/run/user/1000/coda-x/mpv.sock"));
        let args: Vec<&str> = args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "--no-config",
                "--idle=yes",
                "--no-video",
                "--no-terminal",
                "--ytdl=no",
                "--network-timeout=15",
                "--access-references=no",
                "--input-ipc-server=/run/user/1000/coda-x/mpv.sock",
            ]
        );
    }

    #[test]
    fn navidrome_env_is_not_inherited() {
        let command = mpv_command(OsStr::new("mpv"), Path::new("/tmp/x/mpv.sock"));
        let envs: Vec<_> = command.as_std().get_envs().collect();
        for name in STRIPPED_ENV {
            assert!(
                envs.contains(&(OsStr::new(name), None)),
                "{name} not removed: {envs:?}"
            );
        }
        assert!(STRIPPED_ENV.contains(&crate::config::NAVIDROME_PASSWORD_ENV));
    }

    fn paths(candidates: &[(&str, PathBuf)]) -> Vec<PathBuf> {
        candidates.iter().map(|(_, p)| p.clone()).collect()
    }

    #[test]
    fn base_dir_preference() {
        assert_eq!(
            paths(&socket_base_dirs(
                Some("/run/user/1".into()),
                Some("/var/tmp".into())
            )),
            [
                PathBuf::from("/run/user/1"),
                PathBuf::from("/var/tmp"),
                PathBuf::from("/tmp")
            ]
        );
        assert_eq!(
            paths(&socket_base_dirs(None, Some("/var/tmp".into()))),
            [PathBuf::from("/var/tmp"), PathBuf::from("/tmp")]
        );
        assert_eq!(
            paths(&socket_base_dirs(Some("".into()), Some("relative".into()))),
            [PathBuf::from("/tmp")]
        );
        assert_eq!(
            socket_base_dirs(None, Some("/tmp".into())),
            [("TMPDIR", PathBuf::from("/tmp"))],
            "duplicates removed"
        );
        assert_eq!(
            paths(&socket_base_dirs(None, None)),
            [PathBuf::from("/tmp")]
        );
    }

    #[test]
    fn socket_dir_is_private_and_removed() {
        let base = tempfile::tempdir().unwrap();
        let dir = SocketDir::create_in_first(&[("base", base.path().into())]).unwrap();
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        assert!(dir.socket_path().as_os_str().len() < MAX_SOCKET_PATH_LEN);
        assert!(dir.socket_path().ends_with(SOCKET_NAME));
        let path = dir.path().to_path_buf();
        dir.close().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn long_base_dir_falls_back_to_the_next_candidate() {
        let base = tempfile::tempdir().unwrap();
        let long = base.path().join("x".repeat(120));
        std::fs::create_dir(&long).unwrap();
        let candidates = socket_base_dirs(Some(long.into()), None);
        let dir = SocketDir::create_in_first(&candidates).unwrap();
        assert!(dir.path().starts_with(FALLBACK_BASE_DIR));
        assert!(dir.socket_path().as_os_str().len() < MAX_SOCKET_PATH_LEN);
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn missing_xdg_runtime_dir_falls_back() {
        let base = tempfile::tempdir().unwrap();
        let stale = base.path().join("run-user-1000-gone");
        let tmpdir = base.path().join("t");
        std::fs::create_dir(&tmpdir).unwrap();
        let candidates = socket_base_dirs(Some(stale.into()), Some(tmpdir.clone().into()));
        let dir = SocketDir::create_in_first(&candidates).unwrap();
        assert!(dir.path().starts_with(&tmpdir), "{:?}", dir.path());
    }

    #[test]
    fn every_candidate_failing_names_the_ones_tried() {
        let base = tempfile::tempdir().unwrap();
        let candidates = [
            ("XDG_RUNTIME_DIR", base.path().join("gone")),
            ("TMPDIR", base.path().join("also-gone")),
        ];
        let err = SocketDir::create_in_first(&candidates).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("XDG_RUNTIME_DIR"), "{message}");
        assert!(message.contains("TMPDIR"), "{message}");
        assert!(message.contains("also-gone"), "{message}");
    }

    #[tokio::test]
    async fn nonexistent_program_is_not_found() {
        let err = MpvProcess::launch(OsStr::new("coda-test-no-such-mpv-binary"))
            .await
            .unwrap_err();
        assert!(err.is_not_found(), "{err:?}");
        let message = err.to_string();
        assert!(message.contains("brew install mpv"), "{message}");
        assert!(message.contains("pacman -S mpv"), "{message}");
    }

    #[tokio::test]
    async fn exit_status_127_is_not_found() {
        // Stands in for environments where a missing program "spawns" and
        // the child exits with 127.
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("fake-mpv");
        std::fs::write(&program, "#!/bin/sh\nexit 127\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = MpvProcess::launch(program.as_os_str()).await.unwrap_err();
        assert!(err.is_not_found(), "{err:?}");
        assert!(err.to_string().contains("brew install mpv"), "{err}");
    }

    #[tokio::test]
    async fn program_that_exits_immediately_is_reported() {
        // `false` exists everywhere and exits at once without a socket.
        let err = MpvProcess::launch(OsStr::new("false")).await.unwrap_err();
        assert!(matches!(err, SpawnError::ExitedEarly(_)), "{err:?}");
    }
}
