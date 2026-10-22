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
        // Do not follow references inside files: a file whose content is a
        // playlist (e.g. `#EXTM3U` in a `.mp3`) is rejected instead of
        // expanding into entries Coda never requested.
        "--access-references=no".into(),
        ipc,
    ]
}

/// Base directory for the socket directory: `$XDG_RUNTIME_DIR`, else
/// `$TMPDIR`, else `/tmp`. Empty or relative values are ignored.
pub fn socket_base_dir(xdg_runtime_dir: Option<OsString>, tmpdir: Option<OsString>) -> PathBuf {
    [xdg_runtime_dir, tmpdir]
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .find(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(FALLBACK_BASE_DIR))
}

/// A private `0700` directory holding the socket. Removed on drop or
/// [`SocketDir::close`].
#[derive(Debug)]
pub struct SocketDir {
    dir: TempDir,
}

impl SocketDir {
    /// Creates the directory under `base`, falling back to `/tmp` when the
    /// resulting socket path would be too long.
    pub fn create_in(base: &Path) -> io::Result<Self> {
        let dir = Self::create_exact(base)?;
        if dir.socket_path().as_os_str().len() < MAX_SOCKET_PATH_LEN {
            return Ok(dir);
        }
        drop(dir);
        let dir = Self::create_exact(Path::new(FALLBACK_BASE_DIR))?;
        if dir.socket_path().as_os_str().len() >= MAX_SOCKET_PATH_LEN {
            return Err(io::Error::other("socket path is too long"));
        }
        Ok(dir)
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
        let base = socket_base_dir(
            std::env::var_os("XDG_RUNTIME_DIR"),
            std::env::var_os("TMPDIR"),
        );
        let socket_dir = SocketDir::create_in(&base).map_err(SpawnError::SocketDir)?;
        let socket = socket_dir.socket_path();

        let child = Command::new(program)
            .args(mpv_args(&socket))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
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
                "--access-references=no",
                "--input-ipc-server=/run/user/1000/coda-x/mpv.sock",
            ]
        );
    }

    #[test]
    fn base_dir_preference() {
        assert_eq!(
            socket_base_dir(Some("/run/user/1".into()), Some("/var/tmp".into())),
            PathBuf::from("/run/user/1")
        );
        assert_eq!(
            socket_base_dir(None, Some("/var/tmp".into())),
            PathBuf::from("/var/tmp")
        );
        assert_eq!(
            socket_base_dir(Some("".into()), Some("relative".into())),
            PathBuf::from("/tmp")
        );
        assert_eq!(socket_base_dir(None, None), PathBuf::from("/tmp"));
    }

    #[test]
    fn socket_dir_is_private_and_removed() {
        let base = tempfile::tempdir().unwrap();
        let dir = SocketDir::create_in(base.path()).unwrap();
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        assert!(dir.socket_path().as_os_str().len() < MAX_SOCKET_PATH_LEN);
        assert!(dir.socket_path().ends_with(SOCKET_NAME));
        let path = dir.path().to_path_buf();
        dir.close().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn long_base_dir_falls_back_to_tmp() {
        let base = tempfile::tempdir().unwrap();
        let long = base.path().join("x".repeat(120));
        std::fs::create_dir(&long).unwrap();
        let dir = SocketDir::create_in(&long).unwrap();
        assert!(dir.path().starts_with(FALLBACK_BASE_DIR));
        assert!(dir.socket_path().as_os_str().len() < MAX_SOCKET_PATH_LEN);
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
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
    async fn program_that_exits_immediately_is_reported() {
        // `false` exists everywhere and exits at once without a socket.
        let err = MpvProcess::launch(OsStr::new("false")).await.unwrap_err();
        assert!(matches!(err, SpawnError::ExitedEarly(_)), "{err:?}");
    }
}
