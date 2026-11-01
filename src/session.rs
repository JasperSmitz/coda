//! Session persistence: the queue, its current entry, the volume and the
//! repeat mode, saved as versioned JSON in `$XDG_STATE_HOME/coda/state.json`
//! (see [`crate::config::session_file_path`]).
//!
//! The file holds track sources (local paths and opaque Navidrome song
//! ids) and display metadata only: never credentials, stream URLs or the
//! server address. It is written atomically (a private temp file in the
//! same directory, `fsync`, rename) and read as untrusted input: text is
//! capped like tags, the queue is capped at [`MAX_QUEUE_ENTRIES`] while it
//! is parsed (excess entries are skipped unparsed, so memory stays
//! proportional to the file), and a file that cannot be used is kept as
//! `state.json.bad.<unix-seconds>` rather than being overwritten by the
//! next save. If it cannot be set aside, saving is off for the session.
//!
//! Blocking I/O: call [`save`] off the UI loop (`spawn_blocking`).

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::de::{Deserializer, IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

use crate::model::{MAX_METADATA_CHARS, Track, TrackMetadata, TrackSource};
use crate::player::{DEFAULT_VOLUME, MAX_VOLUME, Player, RepeatMode};

/// The schema version this build reads and writes.
pub const VERSION: u64 = 1;
/// Restored queues are cut to this many entries (the live queue's cap).
pub const MAX_QUEUE_ENTRIES: usize = crate::queue::MAX_QUEUE_LEN;
/// Larger files are rejected unread.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Longest accepted local path, in bytes.
const MAX_PATH_BYTES: usize = 4096;

/// What is saved and restored.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub volume: u8,
    pub repeat: RepeatMode,
    /// Index into `tracks`.
    pub current: Option<usize>,
    pub tracks: Vec<Track>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            volume: DEFAULT_VOLUME,
            repeat: RepeatMode::Off,
            current: None,
            tracks: Vec::new(),
        }
    }
}

impl Session {
    /// A snapshot of what `player` would restore to.
    pub fn from_player(player: &Player) -> Self {
        let queue = player.queue();
        Self {
            volume: player.preferred_volume(),
            repeat: player.repeat(),
            current: queue.current().and_then(|id| queue.position(id)),
            tracks: queue.entries().iter().map(|e| e.track.clone()).collect(),
        }
    }
}

/// How [`load`] went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome {
    /// No saved session.
    Missing,
    /// Restored; `skipped` entries were invalid or beyond the cap.
    Restored { skipped: usize },
    /// The file could not be used. It was renamed to `kept_as` if that
    /// succeeded. `reason` never contains file content.
    Rejected {
        reason: String,
        kept_as: Option<PathBuf>,
    },
}

/// The startup result: the session to restore, where to save (`None`
/// disables saving), and a one-time message for the user.
#[derive(Debug, Clone, PartialEq)]
pub struct Restored {
    pub session: Session,
    pub save_path: Option<PathBuf>,
    pub notice: Option<String>,
}

/// Loads the session at `path` (if a state directory is known). When the
/// file was rejected, the notice says so, naming only the kept file; if it
/// could not be set aside, saving is disabled so it is never overwritten.
pub fn restore(path: Option<PathBuf>) -> Restored {
    let Some(path) = path else {
        tracing::warn!("no state directory; the session is not saved");
        return Restored {
            session: Session::default(),
            save_path: None,
            notice: None,
        };
    };
    let (session, outcome) = load(&path);
    let (save_path, notice) = match outcome {
        LoadOutcome::Missing | LoadOutcome::Restored { .. } => (Some(path), None),
        LoadOutcome::Rejected {
            kept_as: Some(kept),
            ..
        } => {
            let name = kept
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let notice = format!("Saved queue could not be read; kept as {name}");
            (Some(path), Some(notice))
        }
        LoadOutcome::Rejected { kept_as: None, .. } => {
            tracing::warn!("saving disabled for this session so the saved state is kept");
            let notice = "Saved queue could not be read; saving is off for this session";
            (None, Some(notice.to_owned()))
        }
    };
    Restored {
        session,
        save_path,
        notice,
    }
}

// ----- file format -----------------------------------------------------

/// Read first, to check the version without building the whole file.
/// Other fields are skipped without being stored.
#[derive(Debug, Deserialize)]
struct Probe {
    version: Option<u64>,
}

#[derive(Debug, Serialize)]
struct FileV1Out {
    version: u64,
    volume: u64,
    repeat: RepeatFile,
    current: Option<usize>,
    queue: Vec<EntryFile>,
}

#[derive(Debug, Deserialize)]
struct FileV1In {
    volume: u64,
    repeat: RepeatFile,
    current: Option<usize>,
    queue: CappedQueue,
}

/// The first [`MAX_QUEUE_ENTRIES`] entries; the rest are skipped unparsed
/// (`IgnoredAny`) and only counted.
#[derive(Debug)]
struct CappedQueue {
    entries: Vec<EntryFile>,
    total: usize,
}

impl<'de> Deserialize<'de> for CappedQueue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CappedVisitor;

        impl<'de> Visitor<'de> for CappedVisitor {
            type Value = CappedQueue;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a list of queue entries")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<CappedQueue, A::Error> {
                // No `size_hint` preallocation: the input is untrusted.
                let mut entries = Vec::new();
                let mut total = 0;
                loop {
                    let more = if entries.len() < MAX_QUEUE_ENTRIES {
                        match seq.next_element::<EntryFile>()? {
                            Some(entry) => {
                                entries.push(entry);
                                true
                            }
                            None => false,
                        }
                    } else {
                        seq.next_element::<IgnoredAny>()?.is_some()
                    };
                    if !more {
                        return Ok(CappedQueue { entries, total });
                    }
                    total += 1;
                }
            }
        }

        deserializer.deserialize_seq(CappedVisitor)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RepeatFile {
    Off,
    All,
    One,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SourceFile {
    Local(String),
    Navidrome(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct EntryFile {
    source: SourceFile,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    album: Option<String>,
    /// Added after version 1 shipped: optional, and left out when absent,
    /// so files with and without it are both version 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    album_artist: Option<String>,
    #[serde(default)]
    track_no: Option<u32>,
    #[serde(default)]
    disc_no: Option<u32>,
    #[serde(default)]
    duration_ms: Option<u64>,
}

impl From<RepeatMode> for RepeatFile {
    fn from(mode: RepeatMode) -> Self {
        match mode {
            RepeatMode::Off => Self::Off,
            RepeatMode::All => Self::All,
            RepeatMode::One => Self::One,
        }
    }
}

impl From<RepeatFile> for RepeatMode {
    fn from(mode: RepeatFile) -> Self {
        match mode {
            RepeatFile::Off => Self::Off,
            RepeatFile::All => Self::All,
            RepeatFile::One => Self::One,
        }
    }
}

impl EntryFile {
    /// `None` for a track that cannot be represented (a non-UTF-8 path,
    /// which mpv could not open either).
    fn from_track(track: &Track) -> Option<Self> {
        let source = match &track.source {
            TrackSource::Local { path } => SourceFile::Local(path.to_str()?.to_owned()),
            TrackSource::Navidrome { song_id } => SourceFile::Navidrome(song_id.clone()),
        };
        Some(Self {
            source,
            title: Some(track.title.clone()),
            artist: track.artist.clone(),
            album: track.album.clone(),
            album_artist: track.album_artist.clone(),
            track_no: track.track_no,
            disc_no: track.disc_no,
            duration_ms: track
                .duration
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
        })
    }

    /// Validates the source and rebuilds the track through the same
    /// constructors (and caps) as tags and server responses.
    fn into_track(self) -> Option<Track> {
        let metadata = TrackMetadata {
            title: self.title,
            artist: self.artist,
            album: self.album,
            album_artist: self.album_artist,
            track_no: self.track_no,
            disc_no: self.disc_no,
            duration: self.duration_ms.map(Duration::from_millis),
        };
        match self.source {
            SourceFile::Local(path) => {
                let valid = path.len() <= MAX_PATH_BYTES
                    && !path.contains('\0')
                    && Path::new(&path).is_absolute();
                valid.then(|| Track::local(PathBuf::from(path), metadata))
            }
            SourceFile::Navidrome(id) => {
                let valid = !id.is_empty()
                    && id.chars().count() <= MAX_METADATA_CHARS
                    && !id.chars().any(char::is_control);
                valid.then(|| Track::navidrome(id, metadata))
            }
        }
    }
}

fn encode(session: &Session) -> serde_json::Result<Vec<u8>> {
    let mut queue = Vec::with_capacity(session.tracks.len());
    let mut current = None;
    for (i, track) in session.tracks.iter().enumerate() {
        if let Some(entry) = EntryFile::from_track(track) {
            if session.current == Some(i) {
                current = Some(queue.len());
            }
            queue.push(entry);
        }
    }
    serde_json::to_vec(&FileV1Out {
        version: VERSION,
        volume: u64::from(session.volume),
        repeat: session.repeat.into(),
        current,
        queue,
    })
}

/// Parses a saved session. Errors are short and never quote file content.
fn decode(bytes: &[u8]) -> Result<(Session, usize), String> {
    let probe: Probe = serde_json::from_slice(bytes).map_err(describe)?;
    match probe.version {
        Some(VERSION) => {}
        Some(other) => return Err(format!("unknown version {other}")),
        None => return Err("no version".into()),
    }
    let file: FileV1In = serde_json::from_slice(bytes).map_err(describe)?;
    let total = file.queue.total;
    let mut tracks = Vec::new();
    let mut current = None;
    for (i, entry) in file.queue.entries.into_iter().enumerate() {
        if let Some(track) = entry.into_track() {
            if file.current == Some(i) {
                current = Some(tracks.len());
            }
            tracks.push(track);
        }
    }
    let session = Session {
        volume: u8::try_from(file.volume.min(u64::from(MAX_VOLUME))).unwrap_or(MAX_VOLUME),
        repeat: file.repeat.into(),
        current,
        tracks,
    };
    let skipped = total - session.tracks.len();
    Ok((session, skipped))
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

// ----- I/O -------------------------------------------------------------

/// Reads the saved session at `path`. A missing file is an empty session.
/// A file that cannot be used (not a regular file, unreadable, too large,
/// invalid, or of an unknown version) is renamed to a new
/// `<path>.bad.<unix-seconds>[.n]`, logged, and an empty session is
/// returned. Never panics on hostile content.
pub fn load(path: &Path) -> (Session, LoadOutcome) {
    let bytes = match read_bounded(path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return (Session::default(), LoadOutcome::Missing),
        Err(reason) => return (Session::default(), reject(path, reason)),
    };
    match decode(&bytes) {
        Ok((session, skipped)) => {
            if skipped > 0 {
                tracing::warn!(
                    skipped,
                    "saved queue entries skipped (invalid or over the cap)"
                );
            }
            tracing::info!(entries = session.tracks.len(), "session restored");
            (session, LoadOutcome::Restored { skipped })
        }
        Err(reason) => (Session::default(), reject(path, reason)),
    }
}

/// `Ok(None)` if there is no file. Opened non-blocking (so a FIFO cannot
/// hang startup) and checked on the open handle, so the type cannot change
/// between the check and the read.
fn read_bounded(path: &Path) -> Result<Option<Vec<u8>>, String> {
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
    if metadata.len() > MAX_FILE_BYTES {
        return Err("too large".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read: {}", e.kind()))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
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

/// Renames `path` to the first unused `bad_path`. Never replaces an
/// existing file.
fn quarantine(path: &Path) -> io::Result<PathBuf> {
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

fn reject(path: &Path, reason: String) -> LoadOutcome {
    let kept_as = match quarantine(path) {
        Ok(kept) => Some(kept),
        Err(error) => {
            tracing::warn!(error = %error.kind(), "could not set the saved session aside");
            None
        }
    };
    tracing::warn!(
        %reason,
        kept = kept_as.is_some(),
        "saved session ignored; starting with an empty queue (kept = renamed to state.json.bad.*)"
    );
    LoadOutcome::Rejected { reason, kept_as }
}

/// Writes `session` to `path` atomically: a temp file (mode 0600) in the
/// same directory is written, `fsync`ed and renamed over `path`, so a
/// crash leaves either the old or the new file. Blocking.
pub fn save(path: &Path, session: &Session) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent directory"))?;
    fs::create_dir_all(dir)?;
    let json = encode(session).map_err(io::Error::other)?;
    let mut temp = tempfile::Builder::new()
        .prefix(".state.json.")
        .suffix(".tmp")
        .permissions(Permissions::from_mode(0o600))
        .tempfile_in(dir)?;
    temp.write_all(&json)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    // Make the rename itself durable; failure only weakens durability.
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn local(path: &str, title: &str) -> Track {
        Track::local(
            path.into(),
            TrackMetadata {
                title: Some(title.into()),
                artist: Some("Artist".into()),
                album: Some("Album".into()),
                album_artist: Some("Album Artist".into()),
                track_no: Some(3),
                disc_no: Some(1),
                duration: Some(Duration::from_millis(183_250)),
            },
        )
    }

    fn sample() -> Session {
        Session {
            volume: 35,
            repeat: RepeatMode::One,
            current: Some(1),
            tracks: vec![
                local("/music/a b/01 Ünïcode.flac", "Song A"),
                Track::navidrome(
                    "song-42".into(),
                    TrackMetadata {
                        title: Some("Remote".into()),
                        ..TrackMetadata::default()
                    },
                ),
                local("/music/c.mp3", "C"),
            ],
        }
    }

    fn state_path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("coda").join("state.json")
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        let session = sample();
        save(&path, &session).unwrap();
        let (loaded, outcome) = load(&path);
        assert_eq!(outcome, LoadOutcome::Restored { skipped: 0 });
        assert_eq!(loaded, session);
        // Saving again replaces the file.
        let empty = Session::default();
        save(&path, &empty).unwrap();
        assert_eq!(load(&path).0, empty);
    }

    #[test]
    fn album_artist_is_optional_within_version_1() {
        // A version 1 file written before `album_artist` existed.
        let old = br#"{"version":1,"volume":50,"repeat":"off","current":0,
            "queue":[{"source":{"local":"/music/a.flac"},"title":"A",
            "artist":"X","album":"Y","track_no":1,"disc_no":null,"duration_ms":1000}]}"#;
        let (session, skipped) = decode(old).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(session.tracks[0].album_artist, None);
        assert_eq!(session.tracks[0].artist.as_deref(), Some("X"));
        // Absent: left out of the file, which stays version 1.
        let json = String::from_utf8(encode(&session).unwrap()).unwrap();
        assert!(!json.contains("album_artist"), "{json}");
        assert!(json.contains(r#""version":1"#), "{json}");
        // Present: kept through a round trip, capped like other tags.
        let mut with = sample();
        with.tracks[0].album_artist = Some("Album Artist".into());
        let json = encode(&with).unwrap();
        assert!(String::from_utf8_lossy(&json).contains(r#""album_artist":"Album Artist""#));
        assert_eq!(decode(&json).unwrap().0, with);
        let long = format!(
            r#"{{"version":1,"volume":50,"repeat":"off","current":null,
            "queue":[{{"source":{{"navidrome":"s"}},"album_artist":"{}"}}]}}"#,
            "x".repeat(10_000)
        );
        let (session, _) = decode(long.as_bytes()).unwrap();
        assert_eq!(
            session.tracks[0].album_artist.as_ref().unwrap().len(),
            MAX_METADATA_CHARS
        );
    }

    #[test]
    fn missing_file_is_an_empty_session() {
        let dir = tempfile::tempdir().unwrap();
        let (session, outcome) = load(&state_path(&dir));
        assert_eq!(outcome, LoadOutcome::Missing);
        assert_eq!(session, Session::default());
    }

    #[test]
    fn atomic_write_is_private_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        save(&path, &sample()).unwrap();
        save(&path, &sample()).unwrap();
        assert_eq!(entries(path.parent().unwrap()), ["state.json"]);
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn failed_save_leaves_the_old_file_and_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        save(&path, &sample()).unwrap();
        // The target is now a non-empty directory: rename fails.
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(path.join("x"), "keep").unwrap();
        assert!(save(&path, &sample()).is_err());
        assert_eq!(entries(path.parent().unwrap()), ["state.json"]);
        assert_eq!(fs::read_to_string(path.join("x")).unwrap(), "keep");
    }

    #[test]
    fn file_never_contains_urls_or_credentials() {
        let json = String::from_utf8(encode(&sample()).unwrap()).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 5, "{keys:?}");
        for needle in ["http", "token", "password", "salt", "server", "url"] {
            assert!(!json.to_lowercase().contains(needle), "{needle}: {json}");
        }
        assert!(json.contains(r#"{"navidrome":"song-42"}"#), "{json}");
        assert!(json.contains(r#"{"local":"/music/c.mp3"}"#), "{json}");
        assert!(json.contains(r#""repeat":"one""#), "{json}");
        assert!(json.contains(r#""duration_ms":183250"#), "{json}");
    }

    /// Loads `content`; asserts it was moved aside unchanged and returns
    /// the kept file's name.
    fn rejected(content: &[u8]) -> (Session, LoadOutcome, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        let (session, outcome) = load(&path);
        assert!(!path.exists(), "moved aside");
        let LoadOutcome::Rejected {
            kept_as: Some(kept),
            ..
        } = &outcome
        else {
            panic!("{outcome:?}");
        };
        assert_eq!(fs::read(kept).unwrap(), content, "kept unchanged");
        let name = kept.file_name().unwrap().to_string_lossy().into_owned();
        (session, outcome, name)
    }

    #[test]
    fn invalid_files_are_kept_as_bad_and_start_empty() {
        let cases: [&[u8]; 7] = [
            b"",
            b"{not json",
            b"[]",
            br#"{"version": 1}"#,
            br#"{"version": 1, "volume": 50, "repeat": "sideways", "current": null, "queue": []}"#,
            br#"{"version": 1, "volume": 50, "repeat": "off", "current": null, "queue": [{"source": {"ftp": "x"}}]}"#,
            b"\xff\xfe",
        ];
        for content in cases {
            let (session, outcome, name) = rejected(content);
            assert_eq!(session, Session::default());
            assert!(name.starts_with("state.json.bad."), "{name}");
            let LoadOutcome::Rejected { reason, .. } = outcome else {
                unreachable!()
            };
            assert!(
                !reason.contains("sideways") && !reason.contains("ftp"),
                "{reason}"
            );
        }
    }

    #[test]
    fn unknown_version_is_rejected() {
        let (_, outcome, _) = rejected(
            br#"{"version": 2, "volume": 50, "repeat": "off", "current": null, "queue": []}"#,
        );
        assert!(
            matches!(&outcome, LoadOutcome::Rejected { reason, .. } if reason == "unknown version 2"),
            "{outcome:?}"
        );
    }

    #[test]
    fn non_regular_files_are_rejected_without_reading() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        fs::create_dir_all(&path).unwrap();
        let (session, outcome) = load(&path);
        assert_eq!(session, Session::default());
        let LoadOutcome::Rejected {
            kept_as: Some(kept),
            reason,
        } = outcome
        else {
            panic!("{outcome:?}");
        };
        assert_eq!(reason, "not a regular file");
        assert!(kept.is_dir());

        // A FIFO with no writer must not block startup.
        let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path; mkfifo has no other effects.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let (session, outcome) = load(&path);
        assert_eq!(session, Session::default());
        assert!(
            matches!(&outcome, LoadOutcome::Rejected { reason, kept_as: Some(_) } if reason == "not a regular file"),
            "{outcome:?}"
        );
    }

    #[test]
    fn earlier_bad_files_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut kept = Vec::new();
        for content in ["first", "second", "third"] {
            fs::write(&path, content).unwrap();
            let (_, outcome) = load(&path);
            let LoadOutcome::Rejected {
                kept_as: Some(bad), ..
            } = outcome
            else {
                panic!("{outcome:?}");
            };
            assert_eq!(fs::read_to_string(&bad).unwrap(), content);
            kept.push(bad);
        }
        kept.sort();
        kept.dedup();
        assert_eq!(kept.len(), 3, "{kept:?}");
        for bad in &kept {
            assert!(bad.exists());
        }
    }

    #[test]
    fn restore_reports_a_rejected_file_by_name_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "nope").unwrap();
        let restored = restore(Some(path.clone()));
        assert_eq!(restored.session, Session::default());
        assert_eq!(restored.save_path, Some(path.clone()));
        let notice = restored.notice.unwrap();
        assert!(
            notice.starts_with("Saved queue could not be read; kept as state.json.bad."),
            "{notice}"
        );
        assert!(!notice.contains('/'), "no directories: {notice}");

        // Fine or missing files: no notice.
        assert_eq!(restore(Some(path.clone())).notice, None);
        save(&path, &sample()).unwrap();
        let restored = restore(Some(path.clone()));
        assert_eq!(restored.notice, None);
        assert_eq!(restored.session, sample());
        assert_eq!(restore(None).save_path, None);
    }

    #[test]
    fn saving_is_disabled_when_the_bad_file_cannot_be_kept() {
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return; // root ignores the permissions this test relies on
        }
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        let parent = path.parent().unwrap().to_path_buf();
        fs::create_dir_all(&parent).unwrap();
        fs::write(&path, "nope").unwrap();
        fs::set_permissions(&parent, Permissions::from_mode(0o500)).unwrap();
        let restored = restore(Some(path.clone()));
        fs::set_permissions(&parent, Permissions::from_mode(0o700)).unwrap();
        assert_eq!(restored.save_path, None);
        assert_eq!(
            restored.notice.as_deref(),
            Some("Saved queue could not be read; saving is off for this session")
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "nope", "left in place");
    }

    #[test]
    fn hostile_entries_are_capped_or_skipped() {
        let huge = "é".repeat(100_000);
        let mut queue = vec![
            serde_json::json!({"source": {"local": "relative/x.flac"}, "title": "rel"}),
            serde_json::json!({"source": {"local": "/m/nul\u{0}.flac"}}),
            serde_json::json!({"source": {"local": format!("/{}", "a".repeat(5000))}}),
            serde_json::json!({"source": {"navidrome": ""}}),
            serde_json::json!({"source": {"navidrome": "a\nb"}}),
            serde_json::json!({"source": {"navidrome": "x".repeat(600)}}),
            serde_json::json!({"source": {"local": "/m/ok.flac"}, "title": huge, "artist": huge,
                               "duration_ms": u64::MAX}),
        ];
        queue.push(serde_json::json!({"source": {"navidrome": "s1"}}));
        let file = serde_json::json!({
            "version": 1, "volume": 999, "repeat": "all", "current": 7, "queue": queue,
            "extra": "ignored"
        });
        let (session, skipped) = decode(file.to_string().as_bytes()).unwrap();
        assert_eq!(skipped, 6);
        assert_eq!(session.volume, MAX_VOLUME);
        assert_eq!(session.repeat, RepeatMode::All);
        assert_eq!(session.tracks.len(), 2);
        assert_eq!(session.current, Some(1), "index follows skipped entries");
        assert_eq!(session.tracks[0].title.chars().count(), MAX_METADATA_CHARS);
        assert_eq!(
            session.tracks[0].artist.as_ref().unwrap().chars().count(),
            MAX_METADATA_CHARS
        );
        assert_eq!(session.tracks[1].title, crate::model::UNKNOWN_TITLE);
    }

    #[test]
    fn queue_is_capped_and_current_beyond_the_cap_is_dropped() {
        let n = MAX_QUEUE_ENTRIES + 5;
        let queue: Vec<Value> = (0..n)
            .map(|i| serde_json::json!({"source": {"local": format!("/m/{i}.flac")}}))
            .collect();
        let file = serde_json::json!({
            "version": 1, "volume": 50, "repeat": "off", "current": n - 1, "queue": queue
        });
        let (session, skipped) = decode(file.to_string().as_bytes()).unwrap();
        assert_eq!(session.tracks.len(), MAX_QUEUE_ENTRIES);
        assert_eq!(skipped, 5);
        assert_eq!(session.current, None);
        assert_eq!(session.tracks.last().unwrap().title, "9999");
        // Entries beyond the cap are skipped unparsed, whatever they hold.
        let mut queue: Vec<Value> = (0..MAX_QUEUE_ENTRIES)
            .map(|i| serde_json::json!({"source": {"local": format!("/m/{i}.flac")}}))
            .collect();
        queue.push(serde_json::json!({"not": ["an", {"entry": 1}]}));
        queue.push(serde_json::json!(42));
        let file = serde_json::json!({
            "version": 1, "volume": 50, "repeat": "off", "current": 0, "queue": queue
        });
        let (session, skipped) = decode(file.to_string().as_bytes()).unwrap();
        assert_eq!(session.tracks.len(), MAX_QUEUE_ENTRIES);
        assert_eq!(skipped, 2);
        assert_eq!(session.current, Some(0));
        // An out-of-range index is just no current entry.
        let file = r#"{"version": 1, "volume": 50, "repeat": "off", "current": 3, "queue": []}"#;
        assert_eq!(decode(file.as_bytes()).unwrap().0.current, None);
    }

    #[test]
    fn unrepresentable_paths_are_left_out_of_the_file() {
        use std::os::unix::ffi::OsStrExt;
        let odd = std::ffi::OsStr::from_bytes(b"/m/\xff.flac");
        let session = Session {
            current: Some(2),
            tracks: vec![
                local("/m/a.flac", "A"),
                Track::local(odd.into(), TrackMetadata::default()),
                local("/m/c.flac", "C"),
            ],
            ..Session::default()
        };
        let (decoded, _) = decode(&encode(&session).unwrap()).unwrap();
        assert_eq!(decoded.tracks.len(), 2);
        assert_eq!(decoded.current, Some(1));
        assert_eq!(decoded.tracks[1].title, "C");
    }

    #[test]
    fn snapshot_of_a_player() {
        let mut player = Player::new();
        let cmds = player.restore(sample().tracks, Some(2), RepeatMode::All, 70);
        assert_eq!(cmds.len(), 1);
        let session = Session::from_player(&player);
        assert_eq!(session.current, Some(2));
        assert_eq!(session.volume, 70);
        assert_eq!(session.repeat, RepeatMode::All);
        assert_eq!(session.tracks, sample().tracks);
    }
}
