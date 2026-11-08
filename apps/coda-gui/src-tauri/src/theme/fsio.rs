//! File access for untrusted theme folders and our own small files.
//!
//! Reading: only regular files. The type is checked from the path first
//! (following symlinks), then the file is opened non-blocking and checked
//! again on the handle, so a FIFO or device can neither hang the open nor
//! be swapped in between. Writing: a temporary file in the same folder,
//! synced, renamed over the target, then the folder synced.

use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    #[error("not a regular file")]
    NotRegular,
    #[error("larger than {0} bytes")]
    TooLarge(u64),
    #[error("not UTF-8 text")]
    NotText,
    #[error("cannot read: {0}")]
    Io(io::ErrorKind),
}

/// The text of the regular file at `path`, at most `max` bytes;
/// `Ok(None)` if nothing is there.
pub fn read_regular(path: &Path, max: u64) -> Result<Option<String>, ReadError> {
    match std::fs::metadata(path) {
        Ok(meta) if !meta.is_file() => return Err(ReadError::NotRegular),
        Ok(meta) if meta.len() > max => return Err(ReadError::TooLarge(max)),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ReadError::Io(e.kind())),
    }
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ReadError::Io(e.kind())),
    };
    let meta = file.metadata().map_err(|e| ReadError::Io(e.kind()))?;
    if !meta.is_file() {
        return Err(ReadError::NotRegular);
    }
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| ReadError::Io(e.kind()))?;
    if bytes.len() as u64 > max {
        return Err(ReadError::TooLarge(max));
    }
    String::from_utf8(bytes)
        .map_err(|_| ReadError::NotText)
        .map(Some)
}

/// Where a write to `path` should land: the target of a symlink (so a
/// dotfile manager's link stays a link), else `path`.
pub fn write_target(path: &Path) -> PathBuf {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
        }
        _ => path.to_path_buf(),
    }
}

/// Writes `bytes` to `path` atomically and syncs its folder.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("the file has no folder"))?;
    std::fs::create_dir_all(dir)?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    // The rename itself survives a crash only once the folder is synced.
    if let Ok(folder) = std::fs::File::open(dir) {
        let _ = folder.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_regular_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a");
        std::fs::write(&file, "hello").unwrap();
        assert_eq!(read_regular(&file, 10), Ok(Some("hello".into())));
        assert_eq!(read_regular(&file, 4), Err(ReadError::TooLarge(4)));
        assert_eq!(read_regular(&dir.path().join("missing"), 10), Ok(None));
        assert_eq!(read_regular(dir.path(), 10), Err(ReadError::NotRegular));
        std::fs::write(&file, [0xff, 0xfe]).unwrap();
        assert_eq!(read_regular(&file, 10), Err(ReadError::NotText));
    }

    #[test]
    fn a_fifo_is_refused_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        // Would block forever on a plain open (no writer).
        assert_eq!(read_regular(&fifo, 10), Err(ReadError::NotRegular));
    }

    #[test]
    fn writes_through_a_symlink_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.toml");
        let link = dir.path().join("link.toml");
        std::fs::write(&target, "old").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        write_atomic(&write_target(&link), b"new").unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        // A plain path is written in place.
        let plain = dir.path().join("plain");
        assert_eq!(write_target(&plain), plain);
        write_atomic(&plain, b"x").unwrap();
        assert_eq!(std::fs::read_to_string(&plain).unwrap(), "x");
    }
}
