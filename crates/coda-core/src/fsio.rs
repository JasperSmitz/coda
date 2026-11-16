//! Small file helpers shared by core's own data files (the session and
//! the mixes file): a bounded read of an untrusted file, setting a bad
//! file aside, and an atomic write.
//!
//! Blocking I/O: call these off a frontend's loop (`spawn_blocking`).

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The bytes of the regular file at `path`, at most `max` of them;
/// `Ok(None)` if there is no file. Opened non-blocking (so a FIFO cannot
/// hang startup) and checked on the open handle, so the type cannot change
/// between the check and the read. Errors are short and never quote file
/// content.
pub(crate) fn read_bounded(path: &Path, max: u64) -> Result<Option<Vec<u8>>, String> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot open: {}", error.kind())),
    };
    let metadata = file
        .metadata()
        .map_err(|e| format!("cannot inspect: {}", e.kind()))?;
    if !metadata.is_file() {
        return Err("not a regular file".into());
    }
    if metadata.len() > max {
        return Err("too large".into());
    }
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read: {}", e.kind()))?;
    if bytes.len() as u64 > max {
        return Err("too large".into());
    }
    Ok(Some(bytes))
}

/// `<path>.bad.<secs>`, or with a `.<n>` suffix for the `n`th retry.
fn bad_path(path: &Path, secs: u64, n: u32) -> PathBuf {
    let mut bad = OsString::from(path.as_os_str());
    bad.push(format!(".bad.{secs}"));
    if n > 0 {
        bad.push(format!(".{n}"));
    }
    PathBuf::from(bad)
}

/// Renames `path` to the first unused `<path>.bad.<unix-seconds>[.n]`.
/// Never replaces an existing file.
pub(crate) fn quarantine(path: &Path) -> io::Result<PathBuf> {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    for n in 0..1000 {
        let candidate = bad_path(path, secs, n);
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::rename(path, &candidate)?;
                return Ok(candidate);
            }
            Err(error) => return Err(error),
            Ok(_) => continue,
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free name to keep the file",
    ))
}

/// Writes `bytes` to `path` atomically: a temp file (mode 0600) named
/// `.<file name>.*.tmp` in the same directory (created if missing) is
/// written, `fsync`ed and renamed over `path`, so a crash leaves either
/// the old or the new file. Blocking.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent directory"))?;
    fs::create_dir_all(dir)?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut temp = tempfile::Builder::new()
        .prefix(&format!(".{file_name}."))
        .suffix(".tmp")
        .permissions(Permissions::from_mode(0o600))
        .tempfile_in(dir)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    // Make the rename itself durable; failure only weakens durability.
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}
