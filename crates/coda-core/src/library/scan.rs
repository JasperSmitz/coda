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
//! - With [`ScanOptions::lyrics`], each track's lyrics are read from the
//!   tags already parsed for its metadata (no second open); only for a
//!   track whose tags have none, a sidecar `<stem>.lrc` is read, and only
//!   when the walk listed one in that directory (no extra `stat` per
//!   file), with [`crate::lyrics::read_local`]'s rules and FIFO safety.
//!   Lyrics go out as [`ScanEvent::Lyrics`], always after the batch
//!   holding their track. Off by default (the terminal player does not
//!   search lyrics).

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
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
use crate::lyrics::{self, Lyrics, index::IndexedLyrics};
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
    /// Lyrics of tracks already sent in a batch, by path (only with
    /// [`ScanOptions::lyrics`]).
    Lyrics(Vec<(PathBuf, IndexedLyrics)>),
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

/// How a scan runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanOptions {
    /// Tracks per batch at most.
    pub batch_size: usize,
    /// A batch is sent at least this often while tracks are found.
    pub batch_interval: Duration,
    /// Read lyrics and send them as [`ScanEvent::Lyrics`].
    pub lyrics: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            batch_size: BATCH_SIZE,
            batch_interval: BATCH_INTERVAL,
            lyrics: false,
        }
    }
}

/// Starts a scan of `root` (absolute, canonicalized) on a blocking thread.
/// Dropping the receiver stops the scan early. Must be called within a
/// Tokio runtime.
pub fn spawn_scan(root: PathBuf, options: ScanOptions) -> mpsc::Receiver<ScanEvent> {
    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    tokio::task::spawn_blocking(move || scan(&root, &tx, options));
    rx
}

/// Blocking scan. Returns early if the receiver is dropped. Must not be
/// called from async context.
pub fn scan(root: &Path, tx: &mpsc::Sender<ScanEvent>, options: ScanOptions) {
    scan_inner(root, tx, options, &mut Instant::now, &read_file);
}

/// Blocking scan with explicit batching and no lyrics, for tests.
pub fn scan_with(
    root: &Path,
    tx: &mpsc::Sender<ScanEvent>,
    batch_size: usize,
    batch_interval: Duration,
) {
    let options = ScanOptions {
        batch_size,
        batch_interval,
        lyrics: false,
    };
    scan(root, tx, options);
}

/// Reads one audio file's metadata and, when asked, its tag lyrics.
type FileReader<'a> = dyn Fn(&Path, bool) -> (TrackMetadata, Option<Lyrics>) + 'a;

/// Tracks and lyrics found but not sent yet. Lyrics always go out after
/// the tracks, so a track is known before its lyrics arrive.
struct Pending {
    tracks: Vec<Track>,
    lyrics: Vec<(PathBuf, IndexedLyrics)>,
    last_flush: Instant,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.tracks.is_empty() && self.lyrics.is_empty()
    }

    /// Sends what is pending if a batch is full or the interval passed.
    /// `false` once the receiver is gone.
    fn flush_if_due(
        &mut self,
        tx: &mpsc::Sender<ScanEvent>,
        options: &ScanOptions,
        now: Instant,
    ) -> bool {
        let due = self.tracks.len() >= options.batch_size
            || self.lyrics.len() >= options.batch_size
            || now.saturating_duration_since(self.last_flush) >= options.batch_interval;
        if !due {
            return true;
        }
        self.last_flush = now;
        self.flush(tx)
    }

    /// Sends everything pending. `false` once the receiver is gone.
    fn flush(&mut self, tx: &mpsc::Sender<ScanEvent>) -> bool {
        if !self.tracks.is_empty()
            && tx
                .blocking_send(ScanEvent::Batch(std::mem::take(&mut self.tracks)))
                .is_err()
        {
            return false;
        }
        if !self.lyrics.is_empty()
            && tx
                .blocking_send(ScanEvent::Lyrics(std::mem::take(&mut self.lyrics)))
                .is_err()
        {
            return false;
        }
        true
    }
}

/// One directory being walked, for sidecar lyrics: its tracks without
/// tag lyrics, and the `.lrc` names listed in it. Resolved once the walk
/// has left the directory (its listing is complete).
#[derive(Default)]
struct OpenDir {
    no_tag_lyrics: Vec<PathBuf>,
    /// As [`name_key`] gives them.
    lrc_names: HashSet<OsString>,
}

/// The directories on the walk's current path, by depth.
#[derive(Default)]
struct SidecarState {
    open: Vec<OpenDir>,
}

impl SidecarState {
    /// An entry at `depth` was reached: directories deeper than its
    /// parent are complete, and their sidecars are read into `out`.
    fn reach(&mut self, depth: usize, out: &mut Vec<(PathBuf, IndexedLyrics)>) {
        while self.open.len() > depth {
            if let Some(dir) = self.open.pop() {
                dir.resolve(out);
            }
        }
    }

    /// Resolves every directory still open (the walk ended).
    fn finish(&mut self, out: &mut Vec<(PathBuf, IndexedLyrics)>) {
        self.reach(0, out);
    }

    /// The directory holding an entry at `depth` (> 0), opened if needed.
    fn parent(&mut self, depth: usize) -> &mut OpenDir {
        while self.open.len() < depth {
            self.open.push(OpenDir::default());
        }
        &mut self.open[depth - 1]
    }
}

impl OpenDir {
    fn resolve(self, out: &mut Vec<(PathBuf, IndexedLyrics)>) {
        if self.lrc_names.is_empty() {
            return;
        }
        for path in self.no_tag_lyrics {
            if !sidecar_listed(&self.lrc_names, &path, CASE_INSENSITIVE_NAMES) {
                continue;
            }
            let read = std::panic::catch_unwind(|| lyrics::read_sidecar(&path));
            if let Some(found) = read.ok().flatten().as_ref().and_then(IndexedLyrics::new) {
                out.push((path, found));
            }
        }
    }
}

/// Whether file names are compared ignoring case, as the file system
/// does: on macOS (APFS and HFS+ are case-insensitive by default),
/// [`crate::lyrics::read_local`]'s open of `<stem>.lrc` also finds
/// `Song.LRC` or `song.lrc` for `Song.flac`, so the scan must too.
/// Linux file systems are case-sensitive.
const CASE_INSENSITIVE_NAMES: bool = cfg!(target_os = "macos");

/// A file name as it is looked up in a listing: lowercased when names
/// compare ignoring case.
fn name_key(name: &OsStr, case_insensitive: bool) -> OsString {
    if case_insensitive {
        OsString::from(name.to_string_lossy().to_lowercase())
    } else {
        name.to_owned()
    }
}

/// `true` for a `.lrc` file name, the extension as
/// [`crate::lyrics::read_local`] would open it.
fn is_lrc(path: &Path, case_insensitive: bool) -> bool {
    path.extension().is_some_and(|ext| {
        if case_insensitive {
            ext.eq_ignore_ascii_case("lrc")
        } else {
            ext == "lrc"
        }
    })
}

/// Whether `lrc_names` (keys from [`name_key`]) holds the sidecar
/// `<stem>.lrc` of the audio file at `audio`.
fn sidecar_listed(lrc_names: &HashSet<OsString>, audio: &Path, case_insensitive: bool) -> bool {
    audio
        .with_extension("lrc")
        .file_name()
        .is_some_and(|name| lrc_names.contains(&name_key(name, case_insensitive)))
}

/// The scan loop with an injectable clock and file reader (tests).
fn scan_inner(
    root: &Path,
    tx: &mpsc::Sender<ScanEvent>,
    options: ScanOptions,
    clock: &mut dyn FnMut() -> Instant,
    reader: &FileReader<'_>,
) {
    let started = Instant::now();
    let mut pending = Pending {
        tracks: Vec::new(),
        lyrics: Vec::new(),
        last_flush: clock(),
    };
    let mut sidecars = SidecarState::default();
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
            Ok(entry) => {
                let depth = entry.depth();
                if options.lyrics && depth > 0 {
                    sidecars.reach(depth, &mut pending.lyrics);
                    if is_lrc(entry.path(), CASE_INSENSITIVE_NAMES) && !entry.file_type().is_dir() {
                        let key = name_key(entry.file_name(), CASE_INSENSITIVE_NAMES);
                        sidecars.parent(depth).lrc_names.insert(key);
                    }
                }
                match classify(&entry) {
                    Candidate::Ignore => {}
                    Candidate::Skip(why) => {
                        skipped += 1;
                        tracing::debug!(why, "skipping audio file");
                    }
                    Candidate::Track => {
                        let path = entry.into_path();
                        let (metadata, found) = read_file_guarded(&path, options.lyrics, reader);
                        match found.as_ref().and_then(IndexedLyrics::new) {
                            Some(found) => pending.lyrics.push((path.clone(), found)),
                            None if options.lyrics && depth > 0 => {
                                sidecars.parent(depth).no_tag_lyrics.push(path.clone());
                            }
                            None => {}
                        }
                        pending.tracks.push(Track::local(path, metadata));
                        tracks += 1;
                    }
                    Candidate::Playlist if playlists.len() < MAX_PLAYLISTS => {
                        playlists.push(entry.into_path());
                    }
                    Candidate::Playlist => playlists_over_cap += 1,
                }
            }
            Err(error) => {
                errors += 1;
                // walkdir's message names the path; log only the kind.
                let kind = error.io_error().map(std::io::Error::kind);
                tracing::warn!(?kind, depth = error.depth(), "scan error");
            }
        }
        // Checked for every walked entry, so a slow stretch of non-audio
        // files cannot hold back tracks already found.
        if !pending.is_empty() && !pending.flush_if_due(tx, &options, clock()) {
            return;
        }
    }
    sidecars.finish(&mut pending.lyrics);
    if !pending.flush(tx) {
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

/// [`read_file`], but a panic inside the tag parser (untrusted file
/// contents) falls back to file-name-only metadata and no lyrics instead
/// of ending the scan.
fn read_file_guarded(
    path: &Path,
    lyrics: bool,
    reader: &FileReader<'_>,
) -> (TrackMetadata, Option<Lyrics>) {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reader(path, lyrics))).unwrap_or_else(
        |_| {
            tracing::warn!("tag parser panicked; listing file by name only");
            (TrackMetadata::default(), None)
        },
    )
}

/// Reads tags and duration with lofty. Any failure yields empty metadata,
/// so the file is still listed under its file name.
pub fn read_metadata(path: &Path) -> TrackMetadata {
    read_file(path, false).0
}

/// [`read_metadata`], plus the lyrics in the same tags when `lyrics` is
/// set (`None` when they have none).
pub fn read_file(path: &Path, lyrics: bool) -> (TrackMetadata, Option<Lyrics>) {
    let tagged = Probe::open(path)
        .map(|p| p.options(ParseOptions::new().read_cover_art(false)))
        .and_then(|p| Ok(p.guess_file_type()?))
        .and_then(|p| p.read());
    let tagged = match tagged {
        Ok(tagged) => tagged,
        Err(error) => {
            tracing::debug!(%error, "could not read tags");
            return (TrackMetadata::default(), None);
        }
    };
    let found = if lyrics {
        lyrics::from_tagged(&tagged)
    } else {
        None
    };
    let duration = Some(tagged.properties().duration()).filter(|d| !d.is_zero());
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        let metadata = TrackMetadata {
            duration,
            ..TrackMetadata::default()
        };
        return (metadata, found);
    };
    let metadata = TrackMetadata {
        title: tag.title().map(|s| s.into_owned()),
        artist: tag.artist().map(|s| s.into_owned()),
        album: tag.album().map(|s| s.into_owned()),
        album_artist: tag.get_string(ItemKey::AlbumArtist).map(str::to_owned),
        track_no: tag.track(),
        disc_no: tag.disk(),
        duration,
        cover_art: None,
    };
    (metadata, found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn options(batch_size: usize, batch_interval: Duration) -> ScanOptions {
        ScanOptions {
            batch_size,
            batch_interval,
            lyrics: false,
        }
    }

    fn collect(rx: &mut mpsc::Receiver<ScanEvent>) -> (Vec<Vec<String>>, Option<usize>) {
        let mut batches = Vec::new();
        let mut finished = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                ScanEvent::Batch(b) => batches.push(b.into_iter().map(|t| t.title).collect()),
                ScanEvent::Playlist(_) | ScanEvent::Lyrics(_) => {}
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
        let reader = |path: &Path, _: bool| {
            if path.ends_with("a.mp3") {
                a_read.set(true);
            }
            (TrackMetadata::default(), None)
        };
        let (tx, mut rx) = mpsc::channel(16);
        scan_inner(
            dir.path(),
            &tx,
            options(1000, Duration::from_secs(1)),
            &mut clock,
            &reader,
        );
        let (batches, finished) = collect(&mut rx);
        // `a` is flushed while walking non-audio entries, before `z` is seen.
        assert_eq!(batches, [vec!["a".to_owned()], vec!["z".to_owned()]]);
        assert_eq!(finished, Some(2));
    }

    #[test]
    fn sidecar_names_match_by_case_only_where_the_file_system_ignores_it() {
        let listing = |names: &[&str], ci: bool| -> HashSet<OsString> {
            names
                .iter()
                .map(Path::new)
                .filter(|p| is_lrc(p, ci))
                .map(|p| name_key(p.as_os_str(), ci))
                .collect()
        };
        // Case-sensitive (Linux): only the exact `<stem>.lrc`.
        let names = listing(&["Song.lrc", "other.LRC", "x.lrc.txt"], false);
        assert_eq!(names.len(), 1);
        assert!(sidecar_listed(&names, Path::new("/m/Song.flac"), false));
        assert!(!sidecar_listed(&names, Path::new("/m/song.flac"), false));
        assert!(!sidecar_listed(&names, Path::new("/m/other.flac"), false));
        // Case-insensitive (macOS): any case of stem and extension.
        let names = listing(&["song.lrc", "OTHER.LRC", "x.lrc.txt"], true);
        assert_eq!(names.len(), 2);
        assert!(sidecar_listed(&names, Path::new("/m/Song.flac"), true));
        assert!(sidecar_listed(&names, Path::new("/m/Other.mp3"), true));
        assert!(!sidecar_listed(&names, Path::new("/m/ÉTÉ.mp3"), false));
        let names = listing(&["été.lrc"], true);
        assert!(sidecar_listed(&names, Path::new("/m/ÉTÉ.mp3"), true));
        assert!(!sidecar_listed(&names, Path::new("/m/x.mp3"), true));
    }

    #[test]
    fn panicking_tag_parser_falls_back_and_scan_finishes() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["boom.flac", "fine.flac"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let reader = |path: &Path, _: bool| {
            if path.ends_with("boom.flac") {
                panic!("simulated lofty panic");
            }
            let metadata = TrackMetadata {
                title: Some("Fine Title".into()),
                ..TrackMetadata::default()
            };
            (metadata, None)
        };
        let (tx, mut rx) = mpsc::channel(16);
        scan_inner(
            dir.path(),
            &tx,
            options(1000, Duration::from_secs(3600)),
            &mut Instant::now,
            &reader,
        );
        let (batches, finished) = collect(&mut rx);
        assert_eq!(batches, [vec!["boom".to_owned(), "Fine Title".to_owned()]]);
        assert_eq!(finished, Some(2));
    }
}
