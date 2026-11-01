//! Local library scan: walks the music directory on a blocking thread and
//! streams discovered tracks in batches.
//!
//! - Directory symlinks are not followed; file symlinks to regular files
//!   are included.
//! - Hidden files and directories (leading `.`) are skipped.
//! - Only allowlisted audio extensions are considered (case-insensitive).
//! - Files whose tags cannot be read are still listed with a filename
//!   fallback; mpv decides playability.
//! - Unreadable directories are counted as errors and the scan continues.
//! - Non-UTF-8 paths are skipped and counted (mpv IPC is JSON/UTF-8).
//! - `.m3u`/`.m3u8` playlists follow the same rules (not counted as
//!   skipped). After the walk, each is read (bounded, see [`m3u`]) and
//!   sent as [`ScanEvent::Playlist`], after every track and before
//!   [`ScanEvent::Finished`], so it can be matched with the whole library.
//!   At most [`MAX_PLAYLISTS`] are read.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lofty::config::ParseOptions;
use lofty::prelude::{Accessor, AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::ItemKey;
use tokio::sync::mpsc;
use walkdir::{DirEntry, WalkDir};

use super::MAX_PLAYLISTS;
use super::m3u::{self, PlaylistFile};
use crate::model::{Track, TrackMetadata};

pub const AUDIO_EXTENSIONS: [&str; 13] = [
    "mp3", "flac", "ogg", "oga", "opus", "m4a", "aac", "wav", "aif", "aiff", "wv", "ape", "mpc",
];
pub const BATCH_SIZE: usize = 200;
pub const BATCH_INTERVAL: Duration = Duration::from_millis(100);
const CHANNEL_CAPACITY: usize = 16;

#[derive(Debug, Clone, PartialEq)]
pub enum ScanEvent {
    Batch(Vec<Track>),
    /// A playlist file, read and resolved; sent after every batch.
    Playlist(PlaylistFile),
    Finished {
        /// Tracks delivered in batches.
        tracks: usize,
        /// Audio files that could not be listed (non-UTF-8 path, dangling
        /// or non-file symlink).
        skipped: usize,
        /// Walk errors, such as unreadable directories.
        errors: usize,
    },
}

/// Starts a scan of `root` (absolute, canonicalized) on a blocking thread.
/// Dropping the receiver stops the scan early. Must be called within a
/// Tokio runtime.
pub fn spawn_scan(root: PathBuf) -> mpsc::Receiver<ScanEvent> {
    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    tokio::task::spawn_blocking(move || scan(&root, &tx));
    rx
}

/// Blocking scan with the default batching. Returns early if the receiver
/// is dropped. Must not be called from async context.
pub fn scan(root: &Path, tx: &mpsc::Sender<ScanEvent>) {
    scan_with(root, tx, BATCH_SIZE, BATCH_INTERVAL);
}

/// Blocking scan with explicit batching, for tests.
pub fn scan_with(
    root: &Path,
    tx: &mpsc::Sender<ScanEvent>,
    batch_size: usize,
    batch_interval: Duration,
) {
    scan_inner(
        root,
        tx,
        batch_size,
        batch_interval,
        &mut Instant::now,
        &read_metadata,
    );
}

/// The scan loop with an injectable clock and metadata reader (tests).
fn scan_inner(
    root: &Path,
    tx: &mpsc::Sender<ScanEvent>,
    batch_size: usize,
    batch_interval: Duration,
    clock: &mut dyn FnMut() -> Instant,
    reader: &dyn Fn(&Path) -> TrackMetadata,
) {
    let started = Instant::now();
    let mut batch = Vec::new();
    let mut last_flush = clock();
    let (mut tracks, mut skipped, mut errors) = (0usize, 0usize, 0usize);
    let mut playlists = Vec::new();
    let mut playlists_over_cap = 0usize;

    let walker = WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden(e.file_name()));

    for entry in walker {
        if tx.is_closed() {
            tracing::debug!("scan receiver dropped; stopping early");
            return;
        }
        match entry {
            Ok(entry) => match classify(&entry) {
                Candidate::Ignore => {}
                Candidate::Skip(why) => {
                    skipped += 1;
                    tracing::debug!(why, "skipping audio file");
                }
                Candidate::Track => {
                    let path = entry.into_path();
                    let metadata = read_metadata_guarded(&path, reader);
                    batch.push(Track::local(path, metadata));
                    tracks += 1;
                }
                Candidate::Playlist if playlists.len() < MAX_PLAYLISTS => {
                    playlists.push(entry.into_path());
                }
                Candidate::Playlist => playlists_over_cap += 1,
            },
            Err(error) => {
                errors += 1;
                tracing::warn!(%error, "scan error");
            }
        }
        // Checked for every walked entry, so a slow stretch of non-audio
        // files cannot hold back tracks already found.
        if !batch.is_empty() {
            let now = clock();
            let due = batch.len() >= batch_size
                || now.saturating_duration_since(last_flush) >= batch_interval;
            if due {
                if tx
                    .blocking_send(ScanEvent::Batch(std::mem::take(&mut batch)))
                    .is_err()
                {
                    return;
                }
                last_flush = now;
            }
        }
    }
    if !batch.is_empty() && tx.blocking_send(ScanEvent::Batch(batch)).is_err() {
        return;
    }
    if playlists_over_cap > 0 {
        tracing::warn!(
            ignored = playlists_over_cap,
            max = MAX_PLAYLISTS,
            "too many playlists; the rest are ignored"
        );
    }
    let found = playlists.len();
    for path in playlists {
        if tx.is_closed() {
            return;
        }
        // Never log the path or its entries: only kinds and counts.
        match m3u::read_playlist(root, path) {
            Ok(file) => {
                if file.cut {
                    tracing::info!(
                        kept = file.entries.len() + file.unresolved,
                        "playlist cut at the size or entry cap"
                    );
                }
                if tx.blocking_send(ScanEvent::Playlist(file)).is_err() {
                    return;
                }
            }
            Err(error) => tracing::warn!(error = %error.kind(), "could not read a playlist"),
        }
    }
    tracing::info!(tracks, skipped, errors, playlists = found, elapsed = ?started.elapsed(), "scan finished");
    let _ = tx.blocking_send(ScanEvent::Finished {
        tracks,
        skipped,
        errors,
    });
}

enum Candidate {
    Track,
    Playlist,
    Ignore,
    Skip(&'static str),
}

fn classify(entry: &DirEntry) -> Candidate {
    let file_type = entry.file_type();
    if file_type.is_dir() {
        return Candidate::Ignore;
    }
    let found = if is_audio_file(entry.path()) {
        Candidate::Track
    } else if m3u::is_playlist_file(entry.path()) {
        Candidate::Playlist
    } else {
        return Candidate::Ignore;
    };
    if file_type.is_symlink() {
        // Follow file symlinks only; directory symlinks are never walked.
        match std::fs::metadata(entry.path()) {
            Ok(target) if target.is_file() => {}
            _ => return skip(found, "symlink does not point at a regular file"),
        }
    } else if !file_type.is_file() {
        return Candidate::Ignore;
    }
    if entry.path().to_str().is_none() {
        return skip(found, "path is not valid UTF-8");
    }
    found
}

/// Audio files that cannot be listed are counted as skipped; playlists
/// are only noted in the debug log.
fn skip(found: Candidate, why: &'static str) -> Candidate {
    match found {
        Candidate::Playlist => {
            tracing::debug!(why, "skipping playlist");
            Candidate::Ignore
        }
        _ => Candidate::Skip(why),
    }
}

fn is_hidden(name: &OsStr) -> bool {
    name.as_encoded_bytes().first() == Some(&b'.')
}

/// `true` if the extension is on the audio allowlist (case-insensitive).
pub fn is_audio_file(path: &Path) -> bool {
    path.extension().and_then(OsStr::to_str).is_some_and(|ext| {
        AUDIO_EXTENSIONS
            .iter()
            .any(|known| known.eq_ignore_ascii_case(ext))
    })
}

/// [`read_metadata`], but a panic inside the tag parser (untrusted file
/// contents) falls back to file-name-only metadata instead of ending the
/// scan.
fn read_metadata_guarded(path: &Path, reader: &dyn Fn(&Path) -> TrackMetadata) -> TrackMetadata {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reader(path))).unwrap_or_else(|_| {
        tracing::warn!("tag parser panicked; listing file by name only");
        TrackMetadata::default()
    })
}

/// Reads tags and duration with lofty. Any failure yields empty metadata,
/// so the file is still listed under its file name.
pub fn read_metadata(path: &Path) -> TrackMetadata {
    let tagged = Probe::open(path)
        .map(|p| p.options(ParseOptions::new().read_cover_art(false)))
        .and_then(|p| Ok(p.guess_file_type()?))
        .and_then(|p| p.read());
    let tagged = match tagged {
        Ok(tagged) => tagged,
        Err(error) => {
            tracing::debug!(%error, "could not read tags");
            return TrackMetadata::default();
        }
    };
    let duration = Some(tagged.properties().duration()).filter(|d| !d.is_zero());
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return TrackMetadata {
            duration,
            ..TrackMetadata::default()
        };
    };
    TrackMetadata {
        title: tag.title().map(|s| s.into_owned()),
        artist: tag.artist().map(|s| s.into_owned()),
        album: tag.album().map(|s| s.into_owned()),
        album_artist: tag.get_string(ItemKey::AlbumArtist).map(str::to_owned),
        track_no: tag.track(),
        disc_no: tag.disk(),
        duration,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn collect(rx: &mut mpsc::Receiver<ScanEvent>) -> (Vec<Vec<String>>, Option<usize>) {
        let mut batches = Vec::new();
        let mut finished = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                ScanEvent::Batch(b) => batches.push(b.into_iter().map(|t| t.title).collect()),
                ScanEvent::Playlist(_) => {}
                ScanEvent::Finished { tracks, .. } => finished = Some(tracks),
            }
        }
        (batches, finished)
    }

    #[test]
    fn flush_timer_is_checked_on_non_audio_entries() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.mp3", "b.txt", "c.txt", "z.mp3"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        // The clock jumps an hour on its second reading after `a.mp3` was
        // read, i.e. while the walk is on `b.txt`.
        let t0 = Instant::now();
        let a_read = Cell::new(false);
        let readings_since_a = Cell::new(0u32);
        let mut clock = || {
            if a_read.get() {
                readings_since_a.set(readings_since_a.get() + 1);
            }
            if readings_since_a.get() >= 2 {
                t0 + Duration::from_secs(3600)
            } else {
                t0
            }
        };
        let reader = |path: &Path| {
            if path.ends_with("a.mp3") {
                a_read.set(true);
            }
            TrackMetadata::default()
        };
        let (tx, mut rx) = mpsc::channel(16);
        scan_inner(
            dir.path(),
            &tx,
            1000,
            Duration::from_secs(1),
            &mut clock,
            &reader,
        );
        let (batches, finished) = collect(&mut rx);
        // `a` is flushed while walking non-audio entries, before `z` is seen.
        assert_eq!(batches, [vec!["a".to_owned()], vec!["z".to_owned()]]);
        assert_eq!(finished, Some(2));
    }

    #[test]
    fn panicking_tag_parser_falls_back_and_scan_finishes() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["boom.flac", "fine.flac"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let reader = |path: &Path| {
            if path.ends_with("boom.flac") {
                panic!("simulated lofty panic");
            }
            TrackMetadata {
                title: Some("Fine Title".into()),
                ..TrackMetadata::default()
            }
        };
        let (tx, mut rx) = mpsc::channel(16);
        scan_inner(
            dir.path(),
            &tx,
            1000,
            Duration::from_secs(3600),
            &mut Instant::now,
            &reader,
        );
        let (batches, finished) = collect(&mut rx);
        assert_eq!(batches, [vec!["boom".to_owned(), "Fine Title".to_owned()]]);
        assert_eq!(finished, Some(2));
    }
}
