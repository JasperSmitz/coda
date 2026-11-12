//! Navidrome lyrics search: one server's lyrics kept on this computer, in
//! a cache file, and searched with a [`LyricsIndex`] (the same type and
//! rules as the local library's). Navidrome's `search3` does not search
//! lyrics, so Coda builds its own index from the server's lyrics (ADR
//! 0007).
//!
//! - **File:** `$XDG_CACHE_HOME/coda/lyrics/<key>.json` (default
//!   `~/.cache/coda/lyrics`; see [`cache_dir`]). The key is an md5 digest
//!   of the server's base URL and the username ([`server_key`]), so the
//!   name holds no URL; the file holds no URL, credential or token, only
//!   ids, song metadata and lyrics text. Compact JSON (short keys, absent
//!   fields left out), written atomically (a temp file renamed into
//!   place) as mode 0600 in a 0700 folder, like the cover cache. A
//!   corrupt, unreadable, wrong-version or oversized
//!   ([`MAX_FILE_BYTES`]) file counts as empty (a debug log, no error)
//!   and is rebuilt. It is parsed as it is read, one song at a time, so
//!   loading needs little more memory than the loaded cache; temp files a
//!   crash left behind are deleted when a cache opens ([`STALE_TEMP`]). A
//!   save writes only when something changed.
//! - **Contents:** the format version, when the last full refresh
//!   started (and the one in progress, so it resumes); per album its id,
//!   song count and when it was listed; per song its id, album id, the
//!   metadata a search row and a play need, its lyrics text (trimmed
//!   lines; timestamps are never needed for search), when it was checked
//!   and how many syncs in a row its request failed; the albums whose
//!   request failed recently, with their count. A song checked with no
//!   lyrics has a check time and no text.
//! - **Untrusted on load:** ids are bounded like the server's, metadata
//!   goes through [`Track::navidrome`]'s cleaning and caps, lyrics through
//!   [`IndexedLyrics::from_text`]'s.
//!
//! [`sync`] brings a [`Store`] up to date with the server; see its docs
//! for the steps, pace and errors. The store is shared (`Arc`): searches
//! read it while a sync runs, and see the songs indexed so far. No lock is
//! held across a request.
//!
//! Memory: the store holds the lyrics text once (see [`super::index`])
//! and each song's details compactly (ids shared, a [`Track`] built only
//! for the results shown; each song knows its index slot, so changing or
//! removing its lyrics is constant time). 20,000 songs with about 1.5 KB
//! of lyrics each (30.5 MB) held 41.6 MB once loaded and at most 42.1 MB
//! while loading, counting every allocation; loading took 0.17–0.21 s,
//! and a search at most 26 ms in Latin and CJK script and 43–45 ms in
//! Cyrillic (a common first letter), in a release build on an x86-64
//! Linux desktop (the `#[ignore]`d test `tests/lyrics_cache_memory.rs`).
//! A save of that cache took 60–85 ms.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, DirBuilder, File, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::de::{
    DeserializeSeed, Deserializer, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor,
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::task::JoinSet;

use super::index::{IndexedLyrics, LyricsIndex, LyricsMatch, LyricsResults};
use crate::model::{Track, TrackMetadata};
use crate::navidrome::api::{MAX_ALBUM_LIST_SIZE, MAX_ID_CHARS};
use crate::navidrome::{AlbumSort, Client, Error};

/// The file format; another version counts as empty.
pub const FORMAT_VERSION: u32 = 1;
/// A larger file is treated as corrupt (and is not written).
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// Requests a sync has in flight at most. Its own limit, apart from
/// browsing's, so browse and playback requests never queue behind it.
pub const MAX_IN_FLIGHT: usize = 3;
/// Every album and song is checked again when the last full refresh is
/// this old.
pub const REFRESH_EVERY: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// Albums per `getAlbumList2` page (the Subsonic maximum).
pub const ALBUM_PAGE: usize = MAX_ALBUM_LIST_SIZE;
/// Albums listed at most (a guard against a server that pages forever).
pub const MAX_ALBUMS: usize = 200_000;
/// Progress is saved after this many albums and songs...
pub const SAVE_EVERY: usize = 250;
/// ...once at least this long has passed since the last save (a large
/// cache takes a moment to write).
pub const SAVE_INTERVAL: Duration = Duration::from_secs(15);
/// An album or song whose request fails in this many completed syncs in a
/// row is skipped (counted as done) until the next full refresh.
pub const MAX_FAILURES: u8 = 3;
/// This many failed requests in a row stop a sync, recording nothing for
/// them: the server, not the items, is the problem.
pub const BREAKER: usize = 10;
/// The album list is refused (nothing removed) when the cache knows at
/// least this many albums...
pub const SHRINK_GUARD_MIN: usize = 20;
/// ...and the list has fewer than this share of them (an empty list is
/// always refused while the cache has albums). Rebuild accepts it.
pub const SHRINK_GUARD_SHARE: f64 = 0.5;
/// Leftover temp files older than this are deleted when a cache opens.
pub const STALE_TEMP: Duration = Duration::from_secs(10 * 60);

const TEMP_PREFIX: &str = ".lyrics.";

/// `$XDG_CACHE_HOME/coda/lyrics`, else `~/.cache/coda/lyrics`. Relative or
/// empty values are ignored; `None` without either.
pub fn cache_dir(xdg_cache_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let absolute = |v: Option<OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    let base = absolute(xdg_cache_home).or_else(|| absolute(home).map(|h| h.join(".cache")))?;
    Some(base.join("coda").join("lyrics"))
}

/// The cache key of the server at `base_url` seen by `username`: an md5
/// hex digest, so file names hold no URL. (Not a security boundary; a
/// stable key without secrets.)
pub fn server_key(base_url: &str, username: &str) -> String {
    let mut context = md5::Context::new();
    for part in ["coda-lyrics-v1", base_url, username] {
        context.consume(part.len().to_le_bytes());
        context.consume(part);
    }
    format!("{:x}", context.finalize())
}

/// The cache file of `key` in `dir`.
pub fn file_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.json"))
}

/// Deletes the cache file of `key` in `dir`, if there is one. Blocking.
pub fn remove_file(dir: &Path, key: &str) -> io::Result<()> {
    match fs::remove_file(file_path(dir, key)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Seconds since the Unix epoch, now.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ----- status ---------------------------------------------------------------

/// What a store is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No sync has run since the store was opened.
    Idle,
    /// Listing albums, then fetching the songs of new and changed ones:
    /// `done` of `todo` albums fetched (both 0 while listing).
    Albums { done: usize, todo: usize },
    /// Fetching lyrics: `done` of `todo` songs this sync.
    Songs { done: usize, todo: usize },
    /// The last sync checked everything it had to.
    UpToDate,
    /// The last sync stopped or left songs unchecked; the next one goes
    /// on from there.
    Paused(Pause),
}

impl State {
    /// A sync is running.
    pub fn is_syncing(self) -> bool {
        matches!(self, Self::Albums { .. } | Self::Songs { .. })
    }
}

/// Why a sync stopped or left songs unchecked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pause {
    /// A timeout, connection failure or server error (5xx).
    Unreachable,
    /// The server rejected the credentials.
    Rejected,
    /// Anything else that stops a sync (a redirect, unusable answers to
    /// the album list).
    Failed,
}

/// A store's state and counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// Songs known.
    pub songs: usize,
    /// Songs whose lyrics were checked at least once.
    pub checked: usize,
    /// Songs with lyrics (searchable).
    pub with_lyrics: usize,
}

// ----- the in-memory cache --------------------------------------------------

#[derive(Debug)]
struct AlbumEntry {
    /// As `getAlbumList2` gave it when the album's songs were fetched.
    song_count: Option<u32>,
    /// When its songs were fetched (Unix seconds).
    listed: u64,
    /// Its songs' ids, in served order (shared with the song table).
    songs: Vec<Arc<str>>,
}

/// One song, compactly: what a search row and a play need. The id is the
/// song table's key (shared with the index and the album); a [`Track`] is
/// built only for the results shown ([`Song::track`]).
#[derive(Debug)]
struct Song {
    /// The album it was fetched with (shared with the album table).
    album_id: Arc<str>,
    title: Box<str>,
    artist: Option<Box<str>>,
    album: Option<Box<str>>,
    album_artist: Option<Box<str>>,
    artist_id: Option<Box<str>>,
    cover_art: Option<Box<str>>,
    disc_no: Option<u32>,
    track_no: Option<u32>,
    /// Milliseconds, saturated.
    duration_ms: Option<u32>,
    /// When its lyrics were checked (Unix seconds); `None`: never.
    checked: Option<u64>,
    /// Its lyrics' slot in the index; `None`: no lyrics.
    slot: Option<u32>,
    /// Completed syncs in a row whose lyrics request failed (see
    /// [`MAX_FAILURES`]).
    failures: u8,
}

fn boxed(s: Option<String>) -> Option<Box<str>> {
    s.map(String::into_boxed_str)
}

impl Song {
    /// From a track already cleaned and capped by [`Track::navidrome`].
    fn from_track(track: Track, album_id: Arc<str>) -> Self {
        Self {
            album_id,
            title: track.title.into_boxed_str(),
            artist: boxed(track.artist),
            album: boxed(track.album),
            album_artist: boxed(track.album_artist),
            artist_id: boxed(track.artist_id),
            cover_art: boxed(track.cover_art),
            disc_no: track.disc_no,
            track_no: track.track_no,
            duration_ms: track
                .duration
                .map(|d| d.as_millis().min(u32::MAX as u128) as u32),
            checked: None,
            slot: None,
            failures: 0,
        }
    }

    /// The song as a track that plays like any Navidrome track.
    fn track(&self, id: &str) -> Track {
        let owned = |s: &Option<Box<str>>| s.as_deref().map(str::to_owned);
        let mut track = Track::navidrome(
            id.to_owned(),
            TrackMetadata {
                title: Some(self.title.to_string()),
                artist: owned(&self.artist),
                album: owned(&self.album),
                album_artist: owned(&self.album_artist),
                track_no: self.track_no,
                disc_no: self.disc_no,
                duration: self.duration_ms.map(|ms| Duration::from_millis(ms.into())),
                cover_art: owned(&self.cover_art),
            },
        );
        track.album_id = Some(self.album_id.to_string());
        track.artist_id = owned(&self.artist_id);
        track
    }
}

/// One server's lyrics cache in memory.
#[derive(Debug, Default)]
struct Cache {
    /// When the last completed full refresh started (0: never).
    refreshed: u64,
    /// A full refresh in progress: everything listed or checked before
    /// this time is checked again.
    refresh_from: Option<u64>,
    albums: HashMap<Arc<str>, AlbumEntry>,
    songs: HashMap<Arc<str>, Song>,
    /// Lyrics of the songs with a `slot` (and only those).
    index: LyricsIndex<Arc<str>>,
    /// Songs with `checked` set.
    checked: usize,
    /// Completed syncs in a row whose `getAlbum` for this album failed.
    album_failures: HashMap<Arc<str>, u8>,
}

impl Cache {
    fn song_with_lyrics(&self, id: &str) -> Option<&Song> {
        self.songs.get(id).filter(|s| s.slot.is_some())
    }

    fn counts(&self, state: State) -> Status {
        Status {
            state,
            songs: self.songs.len(),
            checked: self.checked,
            with_lyrics: self.index.len(),
        }
    }

    /// Drops the index entry in `slot`, updating the song moved into it.
    fn drop_slot(&mut self, slot: u32) {
        if let Some(moved) = self.index.swap_remove(slot as usize).cloned()
            && let Some(song) = self.songs.get_mut(&moved)
        {
            song.slot = Some(slot);
        }
    }

    /// Sets song `key`'s lyrics in the index (adding, replacing or
    /// removing its entry).
    fn set_lyrics(&mut self, key: &Arc<str>, lyrics: Option<IndexedLyrics>) {
        let Some(song) = self.songs.get_mut(key) else {
            return;
        };
        match (song.slot, lyrics) {
            (Some(slot), Some(lyrics)) => self.index.set(slot as usize, lyrics),
            (Some(slot), None) => {
                song.slot = None;
                self.drop_slot(slot);
            }
            (None, Some(lyrics)) => {
                song.slot = Some(self.index.push(key.clone(), lyrics) as u32);
            }
            (None, None) => {}
        }
    }

    fn remove_song(&mut self, id: &str) {
        if let Some(song) = self.songs.remove(id) {
            if song.checked.is_some() {
                self.checked -= 1;
            }
            if let Some(slot) = song.slot {
                self.drop_slot(slot);
            }
        }
    }

    fn remove_album(&mut self, id: &str) {
        self.album_failures.remove(id);
        if let Some(album) = self.albums.remove(id) {
            for song in album.songs {
                if self.album_of(&song) == Some(id) {
                    self.remove_song(&song);
                }
            }
        }
    }

    fn album_of(&self, song: &str) -> Option<&str> {
        Some(&self.songs.get(song)?.album_id)
    }

    /// The shared id: the table's key when the song is known.
    fn song_key(&self, id: &str) -> Arc<str> {
        self.songs
            .get_key_value(id)
            .map_or_else(|| Arc::from(id), |(k, _)| k.clone())
    }

    /// Stores album `id`'s songs (fetched now): songs no longer on it are
    /// removed, new ones added unchecked, known ones keep their lyrics
    /// and get the new metadata.
    fn put_album(&mut self, id: &str, song_count: Option<u32>, tracks: Vec<Track>, now: u64) {
        self.album_failures.remove(id);
        let album_id: Arc<str> = Arc::from(id);
        let new_ids: HashSet<&str> = tracks.iter().filter_map(Track::navidrome_id).collect();
        let old = self.albums.remove(id).map(|a| a.songs).unwrap_or_default();
        for song in old {
            if !new_ids.contains(&*song) && self.album_of(&song) == Some(id) {
                self.remove_song(&song);
            }
        }
        let mut ids: Vec<Arc<str>> = Vec::with_capacity(tracks.len());
        let mut seen = HashSet::new();
        for track in tracks {
            let Some(song_id) = track.navidrome_id() else {
                continue;
            };
            if !seen.insert(song_id.to_owned()) {
                continue;
            }
            let key = self.song_key(song_id);
            let mut song = Song::from_track(track, album_id.clone());
            match self.songs.get_mut(&key) {
                Some(entry) => {
                    // A song that moved here from another album leaves it.
                    if *entry.album_id != *id
                        && let Some(album) = self.albums.get_mut(&entry.album_id)
                    {
                        album.songs.retain(|s| *s != key);
                    }
                    song.checked = entry.checked;
                    song.slot = entry.slot;
                    song.failures = entry.failures;
                    *entry = song;
                }
                None => {
                    self.songs.insert(key.clone(), song);
                }
            }
            ids.push(key);
        }
        ids.shrink_to_fit();
        self.albums.insert(
            album_id,
            AlbumEntry {
                song_count,
                listed: now,
                songs: ids,
            },
        );
    }

    /// Records song `id`'s lyrics (checked now). Nothing if the song is
    /// gone.
    fn put_lyrics(&mut self, id: &str, lyrics: Option<IndexedLyrics>, now: u64) {
        let key = self.song_key(id);
        let Some(song) = self.songs.get_mut(&key) else {
            return;
        };
        if song.checked.is_none() {
            self.checked += 1;
        }
        song.checked = Some(now);
        song.failures = 0;
        self.set_lyrics(&key, lyrics);
    }

    /// Song `id`'s lyrics request failed in a completed sync. After
    /// [`MAX_FAILURES`] in a row it counts as checked (lyrics it had stay)
    /// and is asked again at the next full refresh. `true` when skipped.
    fn fail_song(&mut self, id: &str, now: u64) -> bool {
        let Some(song) = self.songs.get_mut(id) else {
            return false;
        };
        song.failures = song.failures.saturating_add(1);
        if song.failures < MAX_FAILURES {
            return false;
        }
        tracing::debug!("lyrics cache: a song failed {MAX_FAILURES} syncs in a row; skipped");
        if song.checked.is_none() {
            self.checked += 1;
        }
        song.checked = Some(now);
        song.failures = 0;
        true
    }

    /// Album `id`'s `getAlbum` failed in a completed sync. After
    /// [`MAX_FAILURES`] in a row it counts as listed (with the songs it
    /// had) until its song count changes or the next full refresh. `true`
    /// when skipped.
    fn fail_album(&mut self, id: &str, song_count: Option<u32>, now: u64) -> bool {
        let key: Arc<str> = self
            .albums
            .get_key_value(id)
            .map_or_else(|| Arc::from(id), |(k, _)| k.clone());
        let failures = self.album_failures.entry(key.clone()).or_default();
        *failures = failures.saturating_add(1);
        if *failures < MAX_FAILURES {
            return false;
        }
        tracing::debug!("lyrics cache: an album failed {MAX_FAILURES} syncs in a row; skipped");
        self.album_failures.remove(&key);
        let album = self.albums.entry(key).or_insert_with(|| AlbumEntry {
            song_count,
            listed: now,
            songs: Vec::new(),
        });
        album.song_count = song_count;
        album.listed = now;
        true
    }

    // ----- the file ---------------------------------------------------------

    fn to_bytes(&self) -> serde_json::Result<Vec<u8>> {
        let mut albums: Vec<AlbumOut<'_>> = self
            .albums
            .iter()
            .map(|(id, a)| AlbumOut {
                id,
                n: a.song_count,
                t: a.listed,
            })
            .collect();
        albums.sort_by(|a, b| a.id.cmp(b.id));
        let mut songs: Vec<SongOut<'_>> = self
            .songs
            .iter()
            .map(|(id, s)| SongOut {
                id,
                al: &s.album_id,
                ti: &s.title,
                ar: s.artist.as_deref(),
                alb: s.album.as_deref(),
                aa: s.album_artist.as_deref(),
                arid: s.artist_id.as_deref(),
                c: s.cover_art.as_deref(),
                d: s.disc_no,
                n: s.track_no,
                ms: s.duration_ms,
                t: s.checked,
                f: s.failures,
                ly: s
                    .slot
                    .and_then(|slot| self.index.get(slot as usize))
                    .map(IndexedLyrics::text),
            })
            .collect();
        songs.sort_by(|a, b| a.al.cmp(b.al).then(a.id.cmp(b.id)));
        let mut af: Vec<(&str, u8)> = self
            .album_failures
            .iter()
            .map(|(id, n)| (&**id, *n))
            .collect();
        af.sort();
        serde_json::to_vec(&FileOut {
            v: FORMAT_VERSION,
            refreshed: self.refreshed,
            refresh_from: self.refresh_from,
            albums,
            af,
            songs,
        })
    }

    /// Reads a cache file, one song at a time (the whole file is never in
    /// memory as text or as parsed JSON). The albums must come before the
    /// songs, as [`Self::to_bytes`] writes them (a song read before its
    /// album is dropped); another version fails the whole file.
    fn read(reader: impl Read) -> Result<Self, String> {
        let mut cache = Cache::default();
        let mut de = serde_json::Deserializer::from_reader(reader);
        FileSeed(&mut cache)
            .deserialize(&mut de)
            .and_then(|()| de.end())
            .map_err(|e| format!("not a lyrics cache: {e}"))?;
        for album in cache.albums.values_mut() {
            album.songs.shrink_to_fit();
        }
        cache.songs.shrink_to_fit();
        Ok(cache)
    }

    #[cfg(test)]
    fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        Self::read(bytes)
    }

    fn add_loaded_album(&mut self, album: AlbumIn) {
        if valid_id(&album.id) {
            self.albums.insert(
                Arc::from(album.id),
                AlbumEntry {
                    song_count: album.n,
                    listed: album.t,
                    songs: Vec::new(),
                },
            );
        }
    }

    fn add_loaded_song(&mut self, song: SongIn) {
        if !valid_id(&song.id) || self.songs.contains_key(song.id.as_str()) {
            return;
        }
        let Some(album_id) = self
            .albums
            .get_key_value(song.al.as_str())
            .map(|(k, _)| k.clone())
        else {
            return;
        };
        // Cleaned and capped like server data.
        let mut track = Track::navidrome(
            song.id.clone(),
            TrackMetadata {
                title: song.ti,
                artist: song.ar,
                album: song.alb,
                album_artist: song.aa,
                track_no: song.n,
                disc_no: song.d,
                duration: song.ms.map(|ms| Duration::from_millis(ms.into())),
                cover_art: song.c.filter(|c| valid_id(c)),
            },
        );
        track.artist_id = song.arid.filter(|a| valid_id(a));
        let mut entry = Song::from_track(track, album_id.clone());
        let key: Arc<str> = Arc::from(song.id);
        entry.checked = song.t;
        entry.failures = song.f.min(MAX_FAILURES);
        if entry.checked.is_some() {
            self.checked += 1;
        }
        let lyrics = song
            .t
            .and(song.ly.as_deref())
            .and_then(IndexedLyrics::from_text);
        if let Some(lyrics) = lyrics {
            entry.slot = Some(self.index.push(key.clone(), lyrics) as u32);
        }
        if let Some(album) = self.albums.get_mut(&album_id) {
            album.songs.push(key.clone());
        }
        self.songs.insert(key, entry);
    }
}

/// Reads the file's top-level object into the cache.
struct FileSeed<'a>(&'a mut Cache);

impl<'de> DeserializeSeed<'de> for FileSeed<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for FileSeed<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a lyrics cache")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let cache = self.0;
        let mut version = false;
        let mut refreshed = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "v" => {
                    let v: u32 = map.next_value()?;
                    if v != FORMAT_VERSION {
                        return Err(A::Error::custom(format!("format version {v}")));
                    }
                    version = true;
                }
                "refreshed" => refreshed = Some(map.next_value::<u64>()?),
                "refresh_from" => cache.refresh_from = map.next_value()?,
                "albums" => {
                    for album in map.next_value::<Vec<AlbumIn>>()? {
                        cache.add_loaded_album(album);
                    }
                }
                "af" => {
                    for (id, n) in map.next_value::<Vec<(String, u8)>>()? {
                        if valid_id(&id) {
                            cache
                                .album_failures
                                .insert(Arc::from(id), n.min(MAX_FAILURES));
                        }
                    }
                }
                "songs" => map.next_value_seed(SongsSeed(cache))?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        if !version {
            return Err(A::Error::custom("no version"));
        }
        cache.refreshed = refreshed.ok_or_else(|| A::Error::missing_field("refreshed"))?;
        Ok(())
    }
}

/// Reads the songs, adding each as it is read.
struct SongsSeed<'a>(&'a mut Cache);

impl<'de> DeserializeSeed<'de> for SongsSeed<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for SongsSeed<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a list of songs")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(song) = seq.next_element::<SongIn>()? {
            self.0.add_loaded_song(song);
        }
        Ok(())
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().count() <= MAX_ID_CHARS
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}

#[derive(Serialize)]
struct FileOut<'a> {
    v: u32,
    refreshed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_from: Option<u64>,
    albums: Vec<AlbumOut<'a>>,
    /// Albums whose `getAlbum` failed in recent syncs, and how often.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    af: Vec<(&'a str, u8)>,
    songs: Vec<SongOut<'a>>,
}

#[derive(Serialize)]
struct AlbumOut<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
    t: u64,
}

/// One song. Short keys: `al` album id, `ti` title, `ar` artist, `alb`
/// album, `aa` album artist, `arid` artist id, `c` cover id, `d` disc,
/// `n` track, `ms` duration, `t` checked, `f` failed syncs, `ly` lyrics.
#[derive(Serialize)]
struct SongOut<'a> {
    id: &'a str,
    al: &'a str,
    ti: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    ar: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    alb: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    aa: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arid: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    c: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    d: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    t: Option<u64>,
    #[serde(skip_serializing_if = "is_zero")]
    f: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    ly: Option<&'a str>,
}

#[derive(Deserialize)]
struct AlbumIn {
    id: String,
    #[serde(default)]
    n: Option<u32>,
    t: u64,
}

#[derive(Deserialize)]
struct SongIn {
    id: String,
    al: String,
    #[serde(default)]
    ti: Option<String>,
    #[serde(default)]
    ar: Option<String>,
    #[serde(default)]
    alb: Option<String>,
    #[serde(default)]
    aa: Option<String>,
    #[serde(default)]
    arid: Option<String>,
    #[serde(default)]
    c: Option<String>,
    #[serde(default)]
    d: Option<u32>,
    #[serde(default)]
    n: Option<u32>,
    #[serde(default)]
    ms: Option<u32>,
    #[serde(default)]
    t: Option<u64>,
    #[serde(default)]
    f: u8,
    #[serde(default)]
    ly: Option<String>,
}

/// The order of lyric results: artist, album, disc, track (then title and
/// id), case-insensitively.
fn order(a: (&str, &Song), b: (&str, &Song)) -> Ordering {
    fn text(a: Option<&str>, b: Option<&str>) -> Ordering {
        fn fold(s: Option<&str>) -> impl Iterator<Item = char> + '_ {
            s.unwrap_or_default().chars().flat_map(char::to_lowercase)
        }
        fold(a).cmp(fold(b))
    }
    let ((a_id, a), (b_id, b)) = (a, b);
    text(a.artist.as_deref(), b.artist.as_deref())
        .then_with(|| text(a.album.as_deref(), b.album.as_deref()))
        .then(a.disc_no.cmp(&b.disc_no))
        .then(a.track_no.cmp(&b.track_no))
        .then_with(|| text(Some(&a.title), Some(&b.title)))
        .then_with(|| a_id.cmp(b_id))
}

// ----- the store ------------------------------------------------------------

/// One server's lyrics cache: in memory, searchable, saved to its file.
/// Share it in an `Arc`.
pub struct Store {
    /// The cache file; `None` keeps it in memory only.
    path: Option<PathBuf>,
    cache: RwLock<Cache>,
    /// Held while the file is written or deleted; `true` once deleted
    /// (later saves then write nothing, so a save racing a delete never
    /// brings the file back).
    file: Mutex<bool>,
    /// Changed since it was loaded or last saved.
    dirty: AtomicBool,
    status: watch::Sender<Status>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("status", &*self.status.borrow())
            .finish_non_exhaustive()
    }
}

impl Store {
    /// The store of `key` in `dir`, loaded from its file (empty when there
    /// is none or it is unusable). Blocking: reads and parses the file.
    ///
    /// Temp files a crash left in `dir` (older than [`STALE_TEMP`]) are
    /// deleted first. The file is parsed as it is read, one song at a time,
    /// so loading needs little more memory than the loaded cache.
    pub fn open(dir: &Path, key: &str) -> Self {
        remove_stale_temps(dir);
        let path = file_path(dir, key);
        let cache = match open_file(&path) {
            Ok(Some(file)) => Cache::read(io::BufReader::with_capacity(
                64 * 1024,
                file.take(MAX_FILE_BYTES),
            ))
            .unwrap_or_else(|reason| {
                tracing::debug!(%reason, "lyrics cache unusable; starting over");
                Cache::default()
            }),
            Ok(None) => Cache::default(),
            Err(error) => {
                tracing::debug!(%error, "lyrics cache unreadable; starting over");
                Cache::default()
            }
        };
        let store = Self::with_cache(Some(path), cache);
        store.dirty.store(false, AtomicOrdering::Release);
        store
    }

    /// An empty store that is never saved.
    pub fn in_memory() -> Self {
        Self::with_cache(None, Cache::default())
    }

    fn with_cache(path: Option<PathBuf>, cache: Cache) -> Self {
        let status = cache.counts(State::Idle);
        Self {
            path,
            cache: RwLock::new(cache),
            file: Mutex::new(false),
            dirty: AtomicBool::new(true),
            status: watch::Sender::new(status),
        }
    }

    fn read(&self) -> RwLockReadGuard<'_, Cache> {
        self.cache.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, Cache> {
        self.cache.write().unwrap_or_else(|e| e.into_inner())
    }

    /// Changes the cache, then publishes the counts (the state stays).
    fn update<R>(&self, f: impl FnOnce(&mut Cache) -> R) -> R {
        let (result, status) = {
            let mut cache = self.write();
            self.dirty.store(true, AtomicOrdering::Release);
            let result = f(&mut cache);
            (result, cache.counts(self.status.borrow().state))
        };
        self.status.send_replace(status);
        result
    }

    fn set_state(&self, state: State) {
        self.status.send_modify(|s| s.state = state);
    }

    /// The current status.
    pub fn status(&self) -> Status {
        *self.status.borrow()
    }

    /// Status changes, as they happen.
    pub fn subscribe(&self) -> watch::Receiver<Status> {
        self.status.subscribe()
    }

    /// Searches the songs with lyrics for `query` (see
    /// [`LyricsIndex::search`]), skipping the song ids in `exclude`: the
    /// first `limit` in [`order`] (artist, album, disc, track) as tracks
    /// that play like any Navidrome track, and the total.
    pub fn search(
        &self,
        query: &str,
        exclude: &HashSet<String>,
        limit: usize,
    ) -> LyricsResults<Track> {
        let cache = self.read();
        fn song<'a>(cache: &'a Cache, id: &'a Arc<str>) -> Option<(&'a str, &'a Song)> {
            cache.song_with_lyrics(id).map(|s| (&**id, s))
        }
        let found = cache.index.search(
            query,
            |id| exclude.contains(&**id) || cache.song_with_lyrics(id).is_none(),
            limit,
            |a, b| match (song(&cache, a), song(&cache, b)) {
                (Some(a), Some(b)) => order(a, b),
                _ => Ordering::Equal,
            },
        );
        LyricsResults {
            matches: found
                .matches
                .into_iter()
                .filter_map(|m| {
                    Some(LyricsMatch {
                        key: cache.song_with_lyrics(&m.key)?.track(&m.key),
                        snippet: m.snippet,
                    })
                })
                .collect(),
            total: found.total,
        }
    }

    /// Bytes the cache holds on the heap, roughly (lyrics, metadata
    /// strings, ids and table slots; not allocator overhead).
    pub fn heap_bytes(&self) -> usize {
        let cache = self.read();
        let opt = |s: &Option<Box<str>>| s.as_ref().map_or(0, |s| s.len());
        let songs: usize = cache
            .songs
            .iter()
            .map(|(id, s)| {
                // The id's Arc: two counters and the text.
                16 + id.len()
                    + size_of::<(Arc<str>, Song)>()
                    + s.title.len()
                    + opt(&s.artist)
                    + opt(&s.album)
                    + opt(&s.album_artist)
                    + opt(&s.artist_id)
                    + opt(&s.cover_art)
            })
            .sum();
        let albums: usize = cache
            .albums
            .iter()
            .map(|(id, a)| {
                16 + id.len()
                    + size_of::<(Arc<str>, AlbumEntry)>()
                    + a.songs.capacity() * size_of::<Arc<str>>()
            })
            .sum();
        cache.index.heap_bytes() + songs + albums
    }

    /// Writes the cache to its file now (atomically, mode 0600, the folder
    /// 0700), if it changed since it was loaded or last saved. Nothing for
    /// an in-memory store or once the file was deleted. Blocking.
    pub fn save_now(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let deleted = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if *deleted {
            return Ok(());
        }
        let bytes = {
            let cache = self.read();
            // Under the read lock, so no change slips between the check
            // and the bytes.
            if !self.dirty.swap(false, AtomicOrdering::AcqRel) {
                return Ok(());
            }
            cache.to_bytes()
        };
        let result = bytes.map_err(io::Error::other).and_then(|bytes| {
            if bytes.len() as u64 > MAX_FILE_BYTES {
                return Err(io::Error::other("the lyrics cache is too large to save"));
            }
            write_file(path, &bytes)
        });
        if result.is_err() {
            self.dirty.store(true, AtomicOrdering::Release);
        }
        result
    }

    /// Whether the cache changed since it was loaded or last saved.
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(AtomicOrdering::Acquire)
    }
}

/// Writes `bytes` to `path` atomically: a 0600 temp file in its folder
/// (created 0700), synced, renamed into place.
fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("no cache folder"))?;
    ensure_dir(dir)?;
    let mut temp = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .suffix(".tmp")
        .permissions(Permissions::from_mode(0o600))
        .tempfile_in(dir)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

impl Store {
    /// [`Self::save_now`] on a blocking thread.
    pub async fn save(self: &Arc<Self>) -> io::Result<()> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.save_now())
            .await
            .map_err(io::Error::other)?
    }

    /// Deletes the cache file; later saves write nothing. Blocking.
    pub fn delete_file(&self) -> io::Result<()> {
        let mut deleted = self.file.lock().unwrap_or_else(|e| e.into_inner());
        *deleted = true;
        match &self.path {
            Some(path) => match fs::remove_file(path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            },
            None => Ok(()),
        }
    }
}

/// The file, opened; `None` when there is none. An oversized file or one
/// that is not a regular file is an error.
fn open_file(path: &Path) -> io::Result<Option<File>> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() {
        return Err(io::Error::other("not a file"));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(io::Error::other("oversized"));
    }
    File::open(path).map(Some)
}

/// Deletes temp files older than [`STALE_TEMP`] that a crash or a kill
/// during a save left in `dir` (at most 1,000 entries are looked at).
fn remove_stale_temps(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.take(1_000).flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(TEMP_PREFIX) || !name.ends_with(".tmp") {
            continue;
        }
        let old = entry
            .metadata()
            .ok()
            .filter(|m| m.is_file())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= STALE_TEMP);
        if old && fs::remove_file(entry.path()).is_ok() {
            tracing::debug!("lyrics cache: removed a stale temp file");
        }
    }
}

/// Creates `dir` (and missing parents) with mode 0700, and sets `dir` to
/// 0700 if it existed wider.
fn ensure_dir(dir: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let metadata = fs::symlink_metadata(dir)?;
    if !metadata.is_dir() {
        return Err(io::Error::other("the lyrics cache is not a folder"));
    }
    if metadata.permissions().mode() & 0o777 != 0o700 {
        fs::set_permissions(dir, Permissions::from_mode(0o700))?;
    }
    Ok(())
}

// ----- sync -----------------------------------------------------------------

/// How a sync runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncOptions {
    /// Check every album and song again ("Rebuild").
    pub rebuild: bool,
    /// The time now, in Unix seconds.
    pub now: u64,
    /// Progress is saved after this many albums and songs...
    pub save_every: usize,
    /// ...once this long has passed since the last save.
    pub save_interval: Duration,
}

impl SyncOptions {
    /// The defaults, now.
    pub fn new(rebuild: bool) -> Self {
        Self {
            rebuild,
            now: unix_now(),
            save_every: SAVE_EVERY,
            save_interval: SAVE_INTERVAL,
        }
    }
}

/// What a completed sync did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Albums whose songs were fetched.
    pub albums: usize,
    /// Songs whose lyrics were checked.
    pub songs: usize,
    /// Albums and songs whose request failed, left for the next sync.
    pub left: usize,
    /// Albums and songs skipped after failing [`MAX_FAILURES`] syncs in a
    /// row (until the next full refresh).
    pub skipped: usize,
    /// Requests this sync made for albums and songs.
    pub attempted: usize,
}

impl SyncReport {
    /// Many of the requests failed (at least one in 20): the state ends
    /// paused rather than up to date.
    pub fn many_failed(&self) -> bool {
        self.left > 0 && self.left * 20 >= self.attempted.max(1)
    }
}

/// Why a sync stopped. Details are logged (without URLs), not carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SyncError {
    #[error("could not reach the server")]
    Unreachable,
    #[error("the server rejected the login")]
    Rejected,
    #[error("the server's answers could not be used")]
    Failed,
}

impl SyncError {
    pub fn pause(self) -> Pause {
        match self {
            Self::Unreachable => Pause::Unreachable,
            Self::Rejected => Pause::Rejected,
            Self::Failed => Pause::Failed,
        }
    }
}

/// What an error means for a sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorKind {
    /// That album or song failed: tried again next sync (and skipped
    /// after [`MAX_FAILURES`] completed syncs in a row).
    Failed,
    /// Stop the sync.
    Stop(SyncError),
}

fn classify(error: &Error) -> ErrorKind {
    match error {
        Error::Auth | Error::Http(401 | 403) => ErrorKind::Stop(SyncError::Rejected),
        Error::Redirect(_) | Error::InvalidUrl(_) | Error::Setup(_) | Error::Random => {
            ErrorKind::Stop(SyncError::Failed)
        }
        _ => ErrorKind::Failed,
    }
}

/// A connection problem or an overloaded server, as opposed to answers
/// that cannot be used.
fn is_network(error: &Error) -> bool {
    match error {
        Error::Network(_) | Error::Aborted => true,
        Error::Http(status) => *status >= 500 || matches!(status, 408 | 429),
        _ => false,
    }
}

/// Failed requests in a row; [`BREAKER`] of them stop the sync.
#[derive(Default)]
struct Breaker {
    failures: usize,
    all_network: bool,
}

impl Breaker {
    fn success(&mut self) {
        self.failures = 0;
    }

    /// Counts a failed request; `Err` when the sync must stop.
    fn failure(&mut self, error: &Error) -> Result<(), SyncError> {
        if self.failures == 0 {
            self.all_network = true;
        }
        self.failures += 1;
        self.all_network &= is_network(error);
        if self.failures < BREAKER {
            return Ok(());
        }
        tracing::info!(
            failures = self.failures,
            "lyrics cache: requests keep failing; sync stopped"
        );
        Err(if self.all_network {
            SyncError::Unreachable
        } else {
            SyncError::Failed
        })
    }
}

/// Saves every so often.
struct Saver {
    every: usize,
    interval: Duration,
    since: usize,
    last: Instant,
}

impl Saver {
    fn new(options: &SyncOptions) -> Self {
        Self {
            every: options.save_every,
            interval: options.save_interval,
            since: 0,
            last: Instant::now(),
        }
    }

    async fn tick(&mut self, store: &Arc<Store>) {
        self.since += 1;
        if self.since >= self.every && self.last.elapsed() >= self.interval {
            if let Err(error) = store.save().await {
                tracing::warn!(%error, "could not save the lyrics cache");
            }
            self.since = 0;
            self.last = Instant::now();
        }
    }
}

/// Brings `store` up to date with the server `client` talks to:
///
/// 1. pages every album with `getAlbumList2`, alphabetically, 500 a page;
///    a list that is empty, or under half of the albums known (from
///    [`SHRINK_GUARD_MIN`] up), is refused unless `rebuild`: the sync
///    stops with `Failed` and nothing is removed;
/// 2. fetches the songs (`getAlbum`) of albums that are new or whose song
///    count changed, and drops albums (and their songs) no longer listed;
/// 3. fetches the lyrics ([`Client::lyrics`]) of songs never checked;
/// 4. when the last full refresh is [`REFRESH_EVERY`] old, or with
///    `rebuild`, checks every album and song again (resuming one that was
///    interrupted; a refreshed song keeps its old lyrics until its new
///    answer arrives).
///
/// At most [`MAX_IN_FLIGHT`] requests run at once. Progress is saved
/// every [`SyncOptions::save_every`] items (at most once per
/// `save_interval`) and at the end, so an interrupted sync (its task
/// aborted) goes on where it stopped.
///
/// Errors: rejected credentials (or HTTP 401/403) stop the sync
/// (`Rejected`), and so does a redirect (`Failed`). Any other failed
/// request (a timeout, a 5xx, an answer that cannot be used) leaves that
/// album or song for the next sync; once the sync completes, each counts
/// one failure, and after [`MAX_FAILURES`] completed syncs in a row it is
/// skipped until the next full refresh. [`BREAKER`] failures in a row
/// stop the sync (`Unreachable` when all were connection problems, else
/// `Failed`) and record nothing for them. The state ends `Paused` when
/// the sync stopped or many requests failed ([`SyncReport::many_failed`]),
/// else `UpToDate`. The status follows along ([`Store::subscribe`]).
pub async fn sync(
    store: &Arc<Store>,
    client: &Client,
    options: SyncOptions,
) -> Result<SyncReport, SyncError> {
    store.set_state(State::Albums { done: 0, todo: 0 });
    let result = run(store, client, &options).await;
    if let Err(error) = store.save().await {
        tracing::warn!(%error, "could not save the lyrics cache");
    }
    let state = match &result {
        Ok(report) if report.many_failed() => State::Paused(Pause::Unreachable),
        Ok(_) => State::UpToDate,
        Err(error) => State::Paused(error.pause()),
    };
    store.set_state(state);
    match &result {
        Ok(report) => tracing::info!(
            albums = report.albums,
            songs = report.songs,
            left = report.left,
            skipped = report.skipped,
            "lyrics cache synced"
        ),
        Err(error) => tracing::info!(%error, "lyrics cache sync stopped"),
    }
    result
}

async fn run(
    store: &Arc<Store>,
    client: &Client,
    options: &SyncOptions,
) -> Result<SyncReport, SyncError> {
    let now = options.now;
    let refresh_every = REFRESH_EVERY.as_secs();
    let cutoff = store.update(|c| {
        if options.rebuild
            || (c.refresh_from.is_none() && now.saturating_sub(c.refreshed) >= refresh_every)
        {
            c.refresh_from = Some(now);
        }
        c.refresh_from
    });
    let stale = |checked: u64| cutoff.is_some_and(|from| checked < from);

    // 1. Every album.
    let mut listed: Vec<(String, Option<u32>)> = Vec::new();
    let mut seen = HashSet::new();
    let mut offset = 0;
    loop {
        let page = client
            .album_list(AlbumSort::Name, offset, ALBUM_PAGE)
            .await
            .map_err(|error| {
                tracing::debug!(detail = %error.detail(), "lyrics cache: album list failed");
                match classify(&error) {
                    ErrorKind::Stop(stop) => stop,
                    ErrorKind::Failed if is_network(&error) => SyncError::Unreachable,
                    ErrorKind::Failed => SyncError::Failed,
                }
            })?;
        for album in page.albums {
            if seen.insert(album.id.clone()) {
                listed.push((album.id, album.song_count));
            }
        }
        if page.served < ALBUM_PAGE {
            break;
        }
        offset += page.served;
        if offset >= MAX_ALBUMS {
            tracing::warn!(albums = offset, "lyrics cache: album list cut off");
            break;
        }
    }

    // A list far shorter than what is known is more likely a server or
    // permission hiccup than a library emptied: keep everything.
    let known = store.read().albums.len();
    let shrunk = known > 0
        && (listed.is_empty()
            || (known >= SHRINK_GUARD_MIN
                && (listed.len() as f64) < known as f64 * SHRINK_GUARD_SHARE));
    if shrunk && !options.rebuild {
        tracing::warn!(
            listed = listed.len(),
            known,
            "lyrics cache: the album list is much shorter than the cache; nothing removed (Rebuild accepts it)"
        );
        return Err(SyncError::Failed);
    }

    // 2. Drop what is gone; fetch what is new or changed.
    let to_fetch: Vec<(String, Option<u32>)> = store.update(|c| {
        let gone: Vec<Arc<str>> = c
            .albums
            .keys()
            .filter(|id| !seen.contains(&***id))
            .cloned()
            .collect();
        for id in &gone {
            c.remove_album(id);
        }
        c.album_failures.retain(|id, _| seen.contains(&**id));
        listed
            .into_iter()
            .filter(|(id, count)| match c.albums.get(id.as_str()) {
                None => true,
                Some(album) => album.song_count != *count || stale(album.listed),
            })
            .collect()
    });
    let mut report = SyncReport::default();
    let mut breaker = Breaker::default();
    let mut saver = Saver::new(options);
    let mut failed_albums: Vec<(String, Option<u32>)> = Vec::new();
    let mut failed_songs: Vec<String> = Vec::new();
    let todo = to_fetch.len();
    store.set_state(State::Albums { done: 0, todo });
    let mut pending = to_fetch.into_iter();
    let mut running = JoinSet::new();
    let mut done = 0;
    loop {
        while running.len() < MAX_IN_FLIGHT
            && let Some((id, count)) = pending.next()
        {
            let client = client.clone();
            running.spawn(async move {
                let result = client.album_detail(&id).await;
                (id, count, result)
            });
        }
        let Some(joined) = running.join_next().await else {
            break;
        };
        let (id, count, result) = joined.map_err(|_| SyncError::Failed)?;
        report.attempted += 1;
        match result {
            Ok((_, tracks)) => {
                breaker.success();
                store.update(|c| c.put_album(&id, count, tracks, now));
                report.albums += 1;
            }
            Err(error) => {
                tracing::debug!(detail = %error.detail(), "lyrics cache: album fetch failed");
                match classify(&error) {
                    ErrorKind::Stop(stop) => return Err(stop),
                    ErrorKind::Failed => {
                        breaker.failure(&error)?;
                        failed_albums.push((id, count));
                    }
                }
            }
        }
        done += 1;
        store.set_state(State::Albums { done, todo });
        saver.tick(store).await;
    }

    // 3. Lyrics of songs not checked (in this refresh).
    let to_check: Vec<(String, Option<String>, String)> = {
        let cache = store.read();
        let mut songs: Vec<(&str, &Song)> = cache
            .songs
            .iter()
            .filter(|(_, s)| s.checked.is_none_or(stale))
            .map(|(id, s)| (&**id, s))
            .collect();
        songs.sort_by(|a, b| order(*a, *b));
        songs
            .into_iter()
            .map(|(id, s)| {
                (
                    id.to_owned(),
                    s.artist.as_deref().map(str::to_owned),
                    s.title.to_string(),
                )
            })
            .collect()
    };
    let todo = to_check.len();
    store.set_state(State::Songs { done: 0, todo });
    let mut pending = to_check.into_iter();
    let mut running = JoinSet::new();
    let mut done = 0;
    loop {
        while running.len() < MAX_IN_FLIGHT
            && let Some((id, artist, title)) = pending.next()
        {
            let client = client.clone();
            running.spawn(async move {
                let result = client.lyrics(&id, artist.as_deref(), &title).await;
                (id, result)
            });
        }
        let Some(joined) = running.join_next().await else {
            break;
        };
        let (id, result) = joined.map_err(|_| SyncError::Failed)?;
        report.attempted += 1;
        match result {
            Ok(lyrics) => {
                breaker.success();
                let lyrics = lyrics.as_ref().and_then(IndexedLyrics::new);
                store.update(|c| c.put_lyrics(&id, lyrics, now));
                report.songs += 1;
            }
            Err(error) => {
                tracing::debug!(detail = %error.detail(), "lyrics cache: lyrics fetch failed");
                match classify(&error) {
                    ErrorKind::Stop(stop) => return Err(stop),
                    ErrorKind::Failed => {
                        breaker.failure(&error)?;
                        failed_songs.push(id);
                    }
                }
            }
        }
        done += 1;
        store.set_state(State::Songs { done, todo });
        saver.tick(store).await;
    }

    // 4. The sync completed: count the failures (skipping items that
    //    failed too often); a refresh with nothing left over is complete.
    store.update(|c| {
        for (id, count) in &failed_albums {
            if c.fail_album(id, *count, now) {
                report.skipped += 1;
            } else {
                report.left += 1;
            }
        }
        for id in &failed_songs {
            if c.fail_song(id, now) {
                report.skipped += 1;
            } else {
                report.left += 1;
            }
        }
        if report.left == 0
            && let Some(from) = c.refresh_from.take()
        {
            c.refreshed = from;
        }
    });
    Ok(report)
}

#[cfg(test)]
mod tests;
