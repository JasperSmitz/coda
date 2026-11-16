//! The saved mixes: versioned JSON in `$XDG_DATA_HOME/coda/mixes.json`
//! (else `~/.local/share/coda/mixes.json`, see
//! [`crate::config::mixes_file_path`]), user data that both frontends read
//! and write (ADR 0008).
//!
//! It is the first file both apps write, so a change never writes a list
//! held in memory: [`add`], [`update`] and [`delete`] each take an
//! exclusive lock (`flock` on `.mixes.json.lock` beside it), read the file
//! again, apply that one change by mix id and write the result atomically
//! (a temp file, `fsync`, rename). Two apps editing at once both keep their
//! change. [`load`] reads under a shared lock, and sets a bad file aside
//! only under the exclusive lock after reading it again, so a reader never
//! sets aside a good file another app has just written.
//!
//! The file is untrusted input: size-capped, each mix checked like a new
//! one ([`Mix::validated`]), unusable mixes and ingredients skipped, at
//! most [`MAX_MIXES`] kept. A file that cannot be read at all is kept as
//! `mixes.json.bad.<unix-seconds>` and an empty list is used. A file of a
//! newer version (a newer Coda wrote it) is left alone: it reads as empty
//! and changes are refused, so an older build never overwrites it.
//!
//! Blocking I/O: call these through `spawn_blocking`.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{MAX_MIXES, Mix, MixError, MixFileOut, MixId, mix_from_value};
use crate::fsio::{quarantine, read_bounded, write_atomic};

/// The schema version this build reads and writes. Any new ingredient
/// kind or new field bumps it: an older build drops what it does not know
/// when it rewrites the file, so it must see a newer version instead (and
/// leave the file alone). Unknown ingredient kinds are dropped only for
/// hand edits at this version.
pub const VERSION: u64 = 1;
/// Larger files are rejected unread.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// How [`load`] went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome {
    /// No file yet.
    Missing,
    /// Read; `skipped` mixes were unusable or over the cap.
    Loaded { skipped: usize },
    /// The file could not be used. It was renamed to `kept_as` if that
    /// succeeded. `reason` never contains file content.
    Rejected {
        reason: String,
        kept_as: Option<PathBuf>,
    },
    /// Written by a newer Coda: left as it is, read as empty, and not
    /// changed by this build.
    Newer { version: u64 },
}

/// Why a change was not saved.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Invalid(#[from] MixError),
    #[error("that mix no longer exists")]
    NotFound,
    #[error("a mix with that id already exists")]
    Exists,
    #[error("there are already {MAX_MIXES} mixes")]
    Full,
    #[error("the mixes file was written by a newer version of Coda")]
    Newer,
    #[error("the unreadable mixes file could not be set aside")]
    NotKept,
    #[error("the mixes would not fit in the file")]
    TooLarge,
    #[error("cannot save mixes: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Deserialize)]
struct Probe {
    version: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct FileIn {
    mixes: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct FileOut<'a> {
    version: u64,
    mixes: Vec<MixFileOut<'a>>,
}

/// The saved mixes at `path`, in file order. A missing file is an empty
/// list; an unusable one is set aside (see the module docs). Never panics
/// on hostile content.
pub fn load(path: &Path) -> (Vec<Mix>, LoadOutcome) {
    // Without a lock file (no data directory yet, or one this user cannot
    // write) the file is read unlocked; it is then never set aside here.
    let shared = Lock::take(path, Mode::Shared).ok();
    let reason = match read(path) {
        Read::Bad(reason) => reason,
        Read::Done(mixes, outcome) => return (mixes, outcome),
    };
    drop(shared);
    #[cfg(test)]
    tests::before_recheck();
    match Lock::take(path, Mode::Exclusive) {
        // Another app may have set it aside or replaced it meanwhile.
        Ok(_lock) => load_locked(path),
        Err(error) => {
            tracing::warn!(error = %error.kind(), %reason, "mixes file ignored; cannot lock it");
            (
                Vec::new(),
                LoadOutcome::Rejected {
                    reason,
                    kept_as: None,
                },
            )
        }
    }
}

/// [`load`] under the exclusive lock: a bad file is set aside.
fn load_locked(path: &Path) -> (Vec<Mix>, LoadOutcome) {
    match read(path) {
        Read::Done(mixes, outcome) => (mixes, outcome),
        Read::Bad(reason) => (Vec::new(), reject(path, reason)),
    }
}

enum Read {
    Done(Vec<Mix>, LoadOutcome),
    /// Unusable: why (never file content).
    Bad(String),
}

/// Reads and decodes the file; never changes it.
fn read(path: &Path) -> Read {
    let bytes = match read_bounded(path, MAX_FILE_BYTES) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Read::Done(Vec::new(), LoadOutcome::Missing),
        Err(reason) => return Read::Bad(reason),
    };
    match decode(&bytes) {
        Ok((mixes, skipped)) => {
            if skipped > 0 {
                tracing::warn!(skipped, "saved mixes skipped (unusable or over the cap)");
            }
            Read::Done(mixes, LoadOutcome::Loaded { skipped })
        }
        Err(Decode::Newer(version)) => {
            tracing::warn!(version, "mixes file from a newer Coda; left as it is");
            Read::Done(Vec::new(), LoadOutcome::Newer { version })
        }
        Err(Decode::Bad(reason)) => Read::Bad(reason),
    }
}

/// Saves a new mix (checked with [`Mix::validated`]) after the others.
/// Returns every mix as saved.
pub fn add(path: &Path, mix: Mix) -> Result<Vec<Mix>, StoreError> {
    let mix = mix.validated()?;
    change(path, |mixes| {
        if mixes.iter().any(|m| m.id == mix.id) {
            return Err(StoreError::Exists);
        }
        if mixes.len() >= MAX_MIXES {
            return Err(StoreError::Full);
        }
        mixes.push(mix);
        Ok(true)
    })
}

/// Replaces the saved mix with `mix`'s id, in place. Returns every mix as
/// saved.
pub fn update(path: &Path, mix: Mix) -> Result<Vec<Mix>, StoreError> {
    let mix = mix.validated()?;
    change(path, |mixes| {
        let slot = mixes
            .iter_mut()
            .find(|m| m.id == mix.id)
            .ok_or(StoreError::NotFound)?;
        *slot = mix;
        Ok(true)
    })
}

/// Deletes the saved mix `id`; one that is already gone is no error.
/// Returns every mix as saved.
pub fn delete(path: &Path, id: &MixId) -> Result<Vec<Mix>, StoreError> {
    change(path, |mixes| {
        let before = mixes.len();
        mixes.retain(|m| &m.id != id);
        Ok(mixes.len() != before)
    })
}

/// Locks, re-reads, applies `apply` and, if it changed anything (it
/// returns `true`), writes the result atomically: pretty-printed, or
/// compact when that alone fits under [`MAX_FILE_BYTES`] (a file written
/// by hand or another tool can be compact and near the cap), so deleting
/// from a file that loaded always fits.
fn change(
    path: &Path,
    apply: impl FnOnce(&mut Vec<Mix>) -> Result<bool, StoreError>,
) -> Result<Vec<Mix>, StoreError> {
    let _lock = Lock::take(path, Mode::Exclusive)?;
    let (mut mixes, outcome) = load_locked(path);
    match outcome {
        LoadOutcome::Newer { .. } => return Err(StoreError::Newer),
        LoadOutcome::Rejected { kept_as: None, .. } => return Err(StoreError::NotKept),
        _ => {}
    }
    if !apply(&mut mixes)? {
        return Ok(mixes);
    }
    let mut bytes = encode(&mixes).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        bytes = encode_compact(&mixes).map_err(io::Error::other)?;
    }
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(StoreError::TooLarge);
    }
    write_atomic(path, &bytes)?;
    Ok(mixes)
}

fn file_out(mixes: &[Mix]) -> FileOut<'_> {
    FileOut {
        version: VERSION,
        mixes: mixes.iter().map(MixFileOut::of).collect(),
    }
}

fn encode(mixes: &[Mix]) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec_pretty(&file_out(mixes))
}

fn encode_compact(mixes: &[Mix]) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&file_out(mixes))
}

enum Decode {
    Newer(u64),
    Bad(String),
}

/// The usable mixes (first [`MAX_MIXES`], ids unique, the first kept) and
/// how many were skipped.
fn decode(bytes: &[u8]) -> Result<(Vec<Mix>, usize), Decode> {
    let probe: Probe = serde_json::from_slice(bytes).map_err(|e| Decode::Bad(describe(e)))?;
    match probe.version {
        Some(VERSION) => {}
        Some(newer) if newer > VERSION => return Err(Decode::Newer(newer)),
        Some(other) => return Err(Decode::Bad(format!("unknown version {other}"))),
        None => return Err(Decode::Bad("no version".into())),
    }
    let file: FileIn = serde_json::from_slice(bytes).map_err(|e| Decode::Bad(describe(e)))?;
    let total = file.mixes.len();
    let mut mixes: Vec<Mix> = Vec::new();
    for value in file.mixes {
        if mixes.len() == MAX_MIXES {
            break;
        }
        if let Some(mix) = mix_from_value(value)
            && !mixes.iter().any(|m| m.id == mix.id)
        {
            mixes.push(mix);
        }
    }
    let skipped = total - mixes.len();
    Ok((mixes, skipped))
}

/// serde_json's `Display` can quote values from the file; report only the
/// kind of error and where it is.
fn describe(error: serde_json::Error) -> String {
    let kind = match error.classify() {
        serde_json::error::Category::Io => "read error",
        serde_json::error::Category::Syntax => "invalid JSON",
        serde_json::error::Category::Data => "unexpected content",
        serde_json::error::Category::Eof => "truncated JSON",
    };
    format!("{kind} at line {} column {}", error.line(), error.column())
}

fn reject(path: &Path, reason: String) -> LoadOutcome {
    let kept_as = match quarantine(path) {
        Ok(kept) => Some(kept),
        Err(error) => {
            tracing::warn!(error = %error.kind(), "could not set the mixes file aside");
            None
        }
    };
    tracing::warn!(
        %reason,
        kept = kept_as.is_some(),
        "mixes file ignored (kept = renamed to mixes.json.bad.*)"
    );
    LoadOutcome::Rejected { reason, kept_as }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// For reading; does not create the data directory.
    Shared,
    /// For changing or setting the file aside; creates the directory.
    Exclusive,
}

/// A `flock` on `.<file name>.lock` beside the file: exclusive while a
/// change re-reads and writes it or a bad file is set aside, shared while
/// [`load`] reads it. Released when dropped (or when the process dies).
/// Never taken twice by one call.
struct Lock {
    _file: File,
}

impl Lock {
    fn take(path: &Path, mode: Mode) -> io::Result<Self> {
        let dir = path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent directory"))?;
        if mode == Mode::Exclusive {
            std::fs::create_dir_all(dir)?;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(dir.join(format!(".{name}.lock")))?;
        // SAFETY: `flock` on a file descriptor this `File` owns.
        loop {
            let operation = match mode {
                Mode::Shared => libc::LOCK_SH,
                Mode::Exclusive => libc::LOCK_EX,
            };
            if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
                return Ok(Self { _file: file });
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

#[cfg(test)]
mod tests;
