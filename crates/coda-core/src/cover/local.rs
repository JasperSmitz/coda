//! A local track's cover: the front cover embedded in its tags, else an
//! image file in its folder. Blocking; files are untrusted input (sizes
//! are checked before reading, and lofty's own allocation limit bounds
//! embedded pictures).

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use lofty::config::ParseOptions;
use lofty::picture::PictureType;
use lofty::prelude::TaggedFileExt;
use lofty::probe::Probe;

use super::{COVER_SIZE, CoverImage, MAX_IMAGE_BYTES, decode_at};

/// Folder image names, best first (compared case-insensitively).
pub const FOLDER_NAMES: [&str; 4] = ["cover", "folder", "front", "album"];
/// Folder image extensions, best first (compared case-insensitively).
pub const FOLDER_EXTENSIONS: [&str; 3] = ["jpg", "jpeg", "png"];
/// Embedded art is not read from larger files (the folder image still
/// is). lofty's allocation limit applies to each allocation, not to a
/// file, so a file packed with pictures could otherwise take about its
/// own size in memory; this bounds that. Generous enough for long
/// high-resolution tracks.
pub const MAX_EMBEDDED_SCAN_BYTES: u64 = 256 * 1024 * 1024;
/// A folder with more entries is searched only this far.
const MAX_FOLDER_ENTRIES: usize = 10_000;

/// The cover of the audio file at `path`, if it has one that decodes, at
/// most [`COVER_SIZE`] pixels a side.
pub fn load(path: &Path) -> Option<CoverImage> {
    load_at(path, COVER_SIZE)
}

/// Like [`load`], at most `size` pixels a side (within the same decode
/// limits).
pub fn load_at(path: &Path, size: u32) -> Option<CoverImage> {
    embedded(path, size).or_else(|| folder_image(path, size))
}

/// The embedded front cover (else the first picture), decoded. A panic in
/// the tag parser (untrusted file) counts as no picture.
fn embedded(path: &Path, size: u32) -> Option<CoverImage> {
    if fs::metadata(path).ok()?.len() > MAX_EMBEDDED_SCAN_BYTES {
        tracing::debug!("file too large to read embedded art; trying its folder");
        return None;
    }
    let bytes = std::panic::catch_unwind(|| embedded_bytes(path)).ok()??;
    match decode_at(&bytes, size) {
        Ok(image) => Some(image),
        Err(error) => {
            tracing::debug!(%error, "embedded cover did not decode");
            None
        }
    }
}

fn embedded_bytes(path: &Path) -> Option<Vec<u8>> {
    let tagged = Probe::open(path)
        .ok()?
        .options(
            ParseOptions::new()
                .read_properties(false)
                .read_cover_art(true),
        )
        .guess_file_type()
        .ok()?
        .read()
        .ok()?;
    let pictures = || tagged.tags().iter().flat_map(|tag| tag.pictures());
    let picture = pictures()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures().next())?;
    Some(picture.data().to_vec())
}

/// The best-named image file next to `path` that decodes.
fn folder_image(path: &Path, size: u32) -> Option<CoverImage> {
    let dir = path.parent()?;
    for candidate in folder_candidates(dir) {
        let Some(bytes) = read_small_file(&candidate) else {
            continue;
        };
        match decode_at(&bytes, size) {
            Ok(image) => return Some(image),
            Err(error) => tracing::debug!(%error, "folder cover did not decode"),
        }
    }
    None
}

/// Files in `dir` named like a cover, best first.
pub fn folder_candidates(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut ranked: Vec<(usize, PathBuf)> = entries
        .take(MAX_FOLDER_ENTRIES)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?.to_lowercase();
            let (stem, extension) = name.rsplit_once('.')?;
            let stem = FOLDER_NAMES.iter().position(|n| *n == stem)?;
            let extension = FOLDER_EXTENSIONS.iter().position(|e| *e == extension)?;
            Some((stem * FOLDER_EXTENSIONS.len() + extension, entry.path()))
        })
        .collect();
    ranked.sort();
    ranked.into_iter().map(|(_, path)| path).collect()
}

/// The contents of `path` if it is a regular file (symlinks followed) of
/// at most [`MAX_IMAGE_BYTES`].
fn read_small_file(path: &Path) -> Option<Vec<u8>> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_IMAGE_BYTES as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= MAX_IMAGE_BYTES).then_some(bytes)
}
