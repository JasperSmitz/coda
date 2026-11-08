//! The disk caches of Navidrome covers: `$XDG_CACHE_HOME/coda/covers`
//! (default `~/.cache/coda/covers`) for the now-playing bar's covers, and
//! `covers/large` for the Now playing screen's ([`large_dir`]), each mode
//! 0700, created when its first cover is stored (so a local-only setup
//! never has one), and each with its own size cap.
//!
//! - **Names** are an md5 hex digest of the server's base URL, the
//!   username and the `coverArt` id, plus `.jpg`: never a token, salt or
//!   URL. (Not a security boundary; just a stable key without secrets.)
//! - **Contents:** the decoded cover (at most [`super::COVER_SIZE`] or
//!   [`super::LARGE_COVER_SIZE`] pixels, by the cache's [`CoverSize`]) as
//!   a JPEG, written to a temp file in the folder and renamed into place,
//!   so a reader never sees half a file.
//! - **Least recently used** by modification time, with no index: a hit
//!   touches the file; when a write takes the folder over the cap, the
//!   oldest files go until it is under the low mark. The folder scan is
//!   bounded.
//! - **A broken entry** (unreadable, oversized, not a file, or not
//!   decoding) is deleted, so the cover is fetched again.
//!
//! Blocking: call from blocking threads.

use std::ffi::OsString;
use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;

use super::{CoverImage, CoverSize, decode_at};

/// The cache is trimmed when a write takes it over this many bytes...
pub const CAP_BYTES: u64 = 50 * 1024 * 1024;
/// ...down to this many.
pub const LOW_BYTES: u64 = 45 * 1024 * 1024;
/// A cached file larger than this is broken (a 256 px JPEG is 10–40 KB).
pub const MAX_ENTRY_BYTES: u64 = 1024 * 1024;
/// A large cached file larger than this is broken: a 1024 px JPEG at
/// quality 85 measured 65–216 KB for real covers and photos, and about
/// 1.5 MB for pure noise, the worst case.
pub const MAX_LARGE_ENTRY_BYTES: u64 = 4 * 1024 * 1024;
/// At most this many folder entries are looked at when trimming.
const MAX_SCAN: usize = 20_000;
const JPEG_QUALITY: u8 = 85;
const TEMP_PREFIX: &str = ".cover.";

/// The large covers' folder inside the cache folder `dir`. The small
/// cache's trimming skips it (it only counts files).
pub fn large_dir(dir: &Path) -> PathBuf {
    dir.join("large")
}

/// `$XDG_CACHE_HOME/coda/covers`, else `~/.cache/coda/covers`. Relative or
/// empty values are ignored; `None` without either.
pub fn cache_dir(xdg_cache_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let absolute = |v: Option<OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    let base = absolute(xdg_cache_home).or_else(|| absolute(home).map(|h| h.join(".cache")))?;
    Some(base.join("coda").join("covers"))
}

#[derive(Debug)]
pub struct DiskCache {
    dir: PathBuf,
    /// The covers it holds.
    size: CoverSize,
    cap: u64,
    low: u64,
    /// Larger files are broken.
    max_entry: u64,
    /// The folder has been created (or checked) with mode 0700.
    ready: std::sync::atomic::AtomicBool,
}

impl DiskCache {
    /// A cache of small covers in `dir`, which is not touched until a
    /// cover is stored.
    pub fn new(dir: PathBuf) -> Self {
        Self::with_limits(dir, CAP_BYTES, LOW_BYTES)
    }

    /// A cache of large covers in `dir` (see [`large_dir`]), with the same
    /// cap as the small one.
    pub fn large(dir: PathBuf) -> Self {
        Self {
            size: CoverSize::Large,
            max_entry: MAX_LARGE_ENTRY_BYTES,
            ..Self::new(dir)
        }
    }

    /// Like [`DiskCache::new`], trimming from over `cap` bytes to `low`.
    pub fn with_limits(dir: PathBuf, cap: u64, low: u64) -> Self {
        Self {
            dir,
            size: CoverSize::Small,
            cap,
            low,
            max_entry: MAX_ENTRY_BYTES,
            ready: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The covers it holds.
    pub fn size(&self) -> CoverSize {
        self.size
    }

    /// Creates the folder (and missing parents) with mode 0700, and sets
    /// the folder itself to 0700 if it already existed wider. Once per
    /// cache; a failure is tried again next time.
    fn ensure_dir(&self) -> io::Result<()> {
        use std::sync::atomic::Ordering;
        if self.ready.load(Ordering::Acquire) {
            return Ok(());
        }
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)?;
        let metadata = fs::symlink_metadata(&self.dir)?;
        if !metadata.is_dir() {
            return Err(io::Error::other("the cover cache is not a folder"));
        }
        if metadata.permissions().mode() & 0o777 != 0o700 {
            fs::set_permissions(&self.dir, Permissions::from_mode(0o700))?;
        }
        self.ready.store(true, Ordering::Release);
        Ok(())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file name for a cover of the server at `base_url`, seen by
    /// `username`.
    pub fn name(base_url: &str, username: &str, cover_id: &str) -> String {
        let mut context = md5::Context::new();
        for part in ["coda-cover-v1", base_url, username, cover_id] {
            context.consume(part.len().to_le_bytes());
            context.consume(part);
        }
        format!("{:x}.jpg", context.finalize())
    }

    /// The cached cover `name`, if there is a good one; it then counts as
    /// just used. A broken entry is deleted.
    pub fn get(&self, name: &str) -> Option<CoverImage> {
        let path = self.dir.join(name);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => return None,
        };
        if !metadata.is_file() || metadata.len() > self.max_entry {
            self.discard(&path);
            return None;
        }
        let image = fs::read(&path)
            .ok()
            .and_then(|bytes| decode_at(&bytes, self.size.pixels()).ok());
        match image {
            Some(image) => {
                if let Ok(file) = OpenOptions::new().write(true).open(&path) {
                    let _ = file.set_modified(SystemTime::now());
                }
                Some(image)
            }
            None => {
                self.discard(&path);
                None
            }
        }
    }

    /// Saves `image` as `name` (replacing any entry), then trims the cache.
    pub fn put(&self, name: &str, image: &CoverImage) -> io::Result<()> {
        self.ensure_dir()?;
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
            .encode(
                image.rgb(),
                image.width(),
                image.height(),
                ExtendedColorType::Rgb8,
            )
            .map_err(io::Error::other)?;
        let mut temp = tempfile::Builder::new()
            .prefix(TEMP_PREFIX)
            .suffix(".tmp")
            .permissions(Permissions::from_mode(0o600))
            .tempfile_in(&self.dir)?;
        temp.write_all(&bytes)?;
        temp.persist(self.dir.join(name)).map_err(|e| e.error)?;
        self.trim()
    }

    /// Deletes the oldest files while the folder is over the cap, down to
    /// the low mark. Temp files left by a crash count, and age out too.
    fn trim(&self) -> io::Result<()> {
        let mut files = Vec::new();
        let mut total = 0u64;
        for entry in fs::read_dir(&self.dir)?.take(MAX_SCAN) {
            let Ok(entry) = entry else { continue };
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            total += metadata.len();
            files.push((modified, metadata.len(), entry.path()));
        }
        if total <= self.cap {
            return Ok(());
        }
        files.sort();
        for (_, len, path) in files {
            if total <= self.low {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
            }
        }
        tracing::debug!("cover cache trimmed");
        Ok(())
    }

    fn discard(&self, path: &Path) {
        if fs::remove_file(path).is_ok() {
            tracing::debug!("broken cover cache entry removed");
        }
    }
}
