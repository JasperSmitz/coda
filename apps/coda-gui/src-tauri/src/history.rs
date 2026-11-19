//! Play history for the Home view: which contexts (albums, artists,
//! playlists, folders, saved mixes) were played, where each stopped, how
//! often each was listened to, and the day's moods ([`moods`]).
//!
//! The engine owns the [`History`] (it records context plays from the play
//! intents and listens from playback, see `engine/history.rs`) and saves
//! it to `<state>/coda/coda-app/history.json`, debounced like the session
//! and once more at quit. The Home commands ([`commands`]) ask the engine
//! for a copy and resolve it on a blocking thread.
//!
//! Local contexts are stored by *stable* keys (tag names, playlist names,
//! folder paths), not by library ids, which change with every scan; they
//! are resolved against the current library when shown, and skipped when
//! they are gone. Navidrome contexts keep the server's ids and `coverArt`
//! id, and the server they came from ([`server_id`]: a hash of the
//! server's address and user name), so another server never shows them;
//! the day's moods are cached per source and server too. The file never
//! holds stream URLs, credentials, server addresses or cover tokens.
//!
//! **Where it stopped** ([`Stop`]): per context, the entry that played
//! last (its track key, as for track listens, and its place in the
//! context's tracks as played) and the position, recorded by the engine
//! on track changes, pause, a new play and quit while that context fills
//! the queue (`engine/history.rs`). Kept only for contexts among the
//! plays, so it is bounded with them.
//!
//! Listens are counted per context and per track ([`TrackCount`]: Library's
//! local Most played order and an artist's "Your most played"). A local
//! track is keyed by its album's stable key and its disc, number and
//! title ([`local_track_key`]), never by its path; a Navidrome track by
//! its song id and server, with what its row shows (title, album, ids,
//! `coverArt` id).
//!
//! Bounded: the last [`MAX_PLAYS`] context plays, listen counts for at
//! most [`MAX_COUNTS`] contexts and [`MAX_TRACK_COUNTS`] tracks (the
//! least recently heard dropped first).
//!
//! File versions: 1 (no track counts) and 2 are read and migrated as they
//! are (their plays and counts kept; their Daily Picks, which the moods
//! replaced, dropped); 2 adds `tracks`; 3 adds `stops`, `moods`, a
//! Navidrome album count's `genres` and the `mix` context kind. A newer
//! version is left untouched and not saved over.

pub mod commands;
pub mod moods;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use coda_core::library::tags::{AlbumId, AlbumOrder, ArtistId, TagIndex};
use coda_core::library::tree::TrackId;
use coda_core::library::{Library, PlaylistId};
use coda_core::mix::{Ingredient, Mix};
use coda_core::model::Track;
use serde::{Deserialize, Serialize};

use crate::browse::{LocalView, local, local_id};
use crate::cover::CoverService;
use crate::fsio;

/// Context plays kept (newest last).
pub const MAX_PLAYS: usize = 500;
/// Contexts with listen counts kept.
pub const MAX_COUNTS: usize = 5_000;
/// Tracks with listen counts kept.
pub const MAX_TRACK_COUNTS: usize = 20_000;
/// An artist's most played songs shown.
pub const TOP_TRACKS: usize = 6;
/// Distinct recently played contexts shown (Where you left off).
pub const RECENT_SHOWN: usize = 8;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const FILE_VERSION: u32 = 3;
/// The oldest version read (and migrated on the next save).
const OLDEST_VERSION: u32 = 1;
/// Names and subtitles are cut to this many characters.
const MAX_TEXT: usize = 256;
/// Ids (a folder path can be long) are refused beyond this.
const MAX_ID: usize = 4096;
/// A stop's context has at most this many tracks (a whole local folder
/// can hold more than a queue).
const MAX_STOP_TOTAL: usize = 1_000_000;
/// A stop's position is at most this (a day): beyond any real track.
const MAX_POSITION_MS: u64 = 24 * 60 * 60 * 1000;
/// Scopes (the local library, servers) whose moods are cached at once.
const MAX_MOOD_SCOPES: usize = 5;
/// Genres a Navidrome album count keeps.
const MAX_COUNT_GENRES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    Local,
    Navidrome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextKind {
    Album,
    Artist,
    Playlist,
    Folder,
    /// A saved mix (its id; a quick mix is no context).
    Mix,
}

/// A Navidrome server's identity in the history (moved to core, which
/// also keys mixes by it).
pub use coda_core::navidrome::{server_id, server_id_of};

fn valid_server(id: &str) -> bool {
    coda_core::navidrome::is_server_id(id)
}

/// Which part of the history a view reads: the local library's, or one
/// Navidrome server's.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    Local,
    /// A [`server_id`].
    Navidrome(String),
}

impl Scope {
    pub fn source(&self) -> SourceKind {
        match self {
            Self::Local => SourceKind::Local,
            Self::Navidrome(_) => SourceKind::Navidrome,
        }
    }

    pub fn holds(&self, context: &Context) -> bool {
        match self {
            Self::Local => context.source == SourceKind::Local,
            Self::Navidrome(server) => {
                context.source == SourceKind::Navidrome
                    && context.server.as_deref() == Some(server.as_str())
            }
        }
    }
}

/// What identifies a context across runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContextKey {
    pub source: SourceKind,
    /// Navidrome: the [`server_id`]; local: `None`.
    pub server: Option<String>,
    pub kind: ContextKind,
    /// Navidrome: the server's id. Local: a stable key ([`local_album_key`],
    /// [`local_artist_key`], the playlist name, the folder path joined by
    /// `/`).
    pub id: String,
}

/// A context and how to show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub source: SourceKind,
    /// Navidrome: the [`server_id`] it came from (required).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    pub kind: ContextKind,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    /// Navidrome: the `getCoverArt` id (local covers come from the library).
    #[serde(default)]
    pub cover_art: Option<String>,
}

impl Context {
    pub fn key(&self) -> ContextKey {
        ContextKey {
            source: self.source,
            server: self.server.clone(),
            kind: self.kind,
            id: self.id.clone(),
        }
    }

    /// Cleaned and capped like server data; `None` if the id is unusable.
    pub fn sanitized(mut self) -> Option<Self> {
        if self.id.is_empty() || self.id.chars().count() > MAX_ID || self.id.contains('\0') {
            return None;
        }
        self.name = clean(&self.name);
        self.subtitle = self
            .subtitle
            .as_deref()
            .map(clean)
            .filter(|s| !s.is_empty());
        self.cover_art = self
            .cover_art
            .filter(|c| !c.is_empty() && c.chars().count() <= MAX_TEXT && !c.contains('\0'));
        match self.source {
            SourceKind::Local => {
                self.cover_art = None;
                self.server = None;
            }
            // From no known server (or an older file): never shown.
            SourceKind::Navidrome => {
                if !self.server.as_deref().is_some_and(valid_server) {
                    return None;
                }
            }
        }
        Some(self)
    }
}

/// One context play.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Played {
    #[serde(flatten)]
    pub context: Context,
    /// Unix seconds.
    pub at: u64,
}

/// Listens counted for a context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Count {
    #[serde(flatten)]
    pub context: Context,
    pub listens: u64,
    /// The last listen (unix seconds), for trimming.
    pub last: u64,
    /// A Navidrome album's genres, from its songs as they were heard
    /// (version 3; the moods rank genres by them). Local albums take
    /// theirs from the tag index.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
}

/// What identifies a counted track across runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrackKey {
    pub source: SourceKind,
    /// Navidrome: the [`server_id`]; local: `None`.
    pub server: Option<String>,
    /// Navidrome: the song id. Local: [`local_track_key`].
    pub id: String,
}

/// What a Navidrome song's row shows (a local track's comes from the
/// library). No stream URL, no cover URL: the `coverArt` id only.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongInfo {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_no: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disc_no: Option<u32>,
}

impl SongInfo {
    fn sanitized(mut self) -> Self {
        let id_ok = |id: &String| valid_text_id(id);
        let text = |t: Option<String>| t.as_deref().map(clean).filter(|t| !t.is_empty());
        self.title = clean(&self.title);
        self.artist = text(self.artist);
        self.album = text(self.album);
        self.album_artist = text(self.album_artist);
        self.album_id = self.album_id.filter(id_ok);
        self.artist_id = self.artist_id.filter(id_ok);
        self.cover_art = self.cover_art.filter(id_ok);
        self
    }
}

/// A track that was heard: its key and, for Navidrome, its row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeardTrack {
    pub source: SourceKind,
    /// Navidrome: the [`server_id`] it came from (required).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    pub id: String,
    /// Navidrome only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub song: Option<SongInfo>,
}

impl HeardTrack {
    pub fn key(&self) -> TrackKey {
        TrackKey {
            source: self.source,
            server: self.server.clone(),
            id: self.id.clone(),
        }
    }

    /// Cleaned and capped like server data; `None` if the id (or a
    /// Navidrome track's server) is unusable.
    pub fn sanitized(mut self) -> Option<Self> {
        if self.id.is_empty() || self.id.chars().count() > MAX_ID || self.id.contains('\0') {
            return None;
        }
        match self.source {
            SourceKind::Local => {
                self.server = None;
                self.song = None;
            }
            SourceKind::Navidrome => {
                if !self.server.as_deref().is_some_and(valid_server) {
                    return None;
                }
                self.song = self.song.map(SongInfo::sanitized);
            }
        }
        Some(self)
    }
}

/// Listens counted for a track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackCount {
    #[serde(flatten)]
    pub track: HeardTrack,
    pub listens: u64,
    /// The last listen (unix seconds), for trimming.
    pub last: u64,
}

/// Where a context stopped: the entry that played last in it and the
/// position (version 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stop {
    pub source: SourceKind,
    /// Navidrome: the [`server_id`] (required).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    pub kind: ContextKind,
    /// The context's id ([`Context::id`]).
    pub id: String,
    /// The entry's track: a local [`local_track_key`], a Navidrome song id.
    pub track: String,
    /// Its place among the context's tracks as they were queued.
    pub index: usize,
    /// How many tracks the context queued.
    pub total: usize,
    pub position_ms: u64,
    /// Unix seconds.
    pub at: u64,
}

impl Stop {
    pub fn key(&self) -> ContextKey {
        ContextKey {
            source: self.source,
            server: self.server.clone(),
            kind: self.kind,
            id: self.id.clone(),
        }
    }

    /// `None` if unusable (ids as for a [`Context`], the place within the
    /// count, at most [`MAX_STOP_TOTAL`]); the position capped at
    /// [`MAX_POSITION_MS`].
    fn sanitized(mut self) -> Option<Self> {
        let id_ok = |id: &str| !id.is_empty() && id.chars().count() <= MAX_ID && !id.contains('\0');
        if !id_ok(&self.id)
            || !id_ok(&self.track)
            || self.kind == ContextKind::Mix
            || self.total == 0
            || self.total > MAX_STOP_TOTAL
            || self.index >= self.total
        {
            return None;
        }
        self.position_ms = self.position_ms.min(MAX_POSITION_MS);
        match self.source {
            SourceKind::Local => self.server = None,
            SourceKind::Navidrome => {
                if !self.server.as_deref().is_some_and(valid_server) {
                    return None;
                }
            }
        }
        Some(self)
    }
}

/// One album of a cached mood. Local: `id` is the album's stable key
/// ([`local_album_key`]) and the rest comes from the library when shown;
/// Navidrome: the server's id and what the card shows (the `coverArt` id,
/// never a URL).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoodAlbum {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// One cached mood: a genre and its albums.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mood {
    /// The genre's name as shown (the tag index's, the server's cleaned).
    pub genre: String,
    /// Navidrome: the genre's albums on the server (local: counted when
    /// shown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_count: Option<u32>,
    pub albums: Vec<MoodAlbum>,
}

/// A day's moods of one scope: today's first, then the other moods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoodsDay {
    /// The local calendar day, `YYYY-MM-DD`.
    pub date: String,
    pub source: SourceKind,
    /// Navidrome: the [`server_id`] they were drawn from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    pub moods: Vec<Mood>,
}

impl MoodsDay {
    fn scope(&self) -> Scope {
        match &self.server {
            Some(server) => Scope::Navidrome(server.clone()),
            None => Scope::Local,
        }
    }
}

/// `history.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryFile {
    pub version: u32,
    #[serde(default)]
    pub plays: Vec<Played>,
    #[serde(default)]
    pub counts: Vec<Count>,
    /// Listens per track (version 2; most recently heard first).
    #[serde(default)]
    pub tracks: Vec<TrackCount>,
    /// Where contexts stopped (version 3; most recent first).
    #[serde(default)]
    pub stops: Vec<Stop>,
    /// The latest day's moods per scope (version 3; at most
    /// [`MAX_MOOD_SCOPES`]). Version 1 and 2 files' `dailies` (the Daily
    /// Picks the moods replaced) are ignored.
    #[serde(default)]
    pub moods: Vec<MoodsDay>,
}

/// The history in memory. Every change bumps [`History::revision`].
#[derive(Debug, Clone, Default)]
pub struct History {
    /// Oldest first.
    plays: VecDeque<Played>,
    counts: HashMap<ContextKey, Count>,
    tracks: HashMap<TrackKey, TrackCount>,
    /// Only for contexts among `plays`.
    stops: HashMap<ContextKey, Stop>,
    /// Oldest first, one per scope.
    moods: Vec<MoodsDay>,
    revision: u64,
}

fn clean(text: &str) -> String {
    let text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    text.trim().chars().take(MAX_TEXT).collect()
}

/// `YYYY-MM-DD`.
pub fn valid_date(date: &str) -> bool {
    let b = date.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// Unix seconds now.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn sanitize_moods_day(day: MoodsDay) -> Option<MoodsDay> {
    if !valid_date(&day.date) {
        return None;
    }
    let server = match day.source {
        SourceKind::Local => None,
        SourceKind::Navidrome => Some(day.server.filter(|s| valid_server(s))?),
    };
    let moods = day
        .moods
        .into_iter()
        .filter_map(|mood| {
            let genre = mood.genre;
            if genre.trim().is_empty() || genre.chars().count() > MAX_TEXT || genre.contains('\0') {
                return None;
            }
            let albums: Vec<MoodAlbum> = mood
                .albums
                .into_iter()
                .filter_map(sanitize_mood_album)
                .take(moods::MOOD_ALBUMS)
                .collect();
            (!albums.is_empty()).then_some(Mood {
                genre,
                album_count: mood.album_count,
                albums,
            })
        })
        .take(moods::MOODS_KEPT)
        .collect();
    Some(MoodsDay {
        date: day.date,
        source: day.source,
        server,
        moods,
    })
}

fn sanitize_mood_album(mut album: MoodAlbum) -> Option<MoodAlbum> {
    if album.id.is_empty() || album.id.chars().count() > MAX_ID || album.id.contains('\0') {
        return None;
    }
    album.name = clean(&album.name);
    album.artist = album.artist.as_deref().map(clean).filter(|a| !a.is_empty());
    album.artist_id = album.artist_id.filter(|id| valid_text_id(id));
    album.cover_art = album.cover_art.filter(|id| valid_text_id(id));
    Some(album)
}

fn clean_genres(genres: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    genres
        .iter()
        .map(|g| clean(g))
        .filter(|g| !g.is_empty() && seen.insert(lower(g)))
        .take(MAX_COUNT_GENRES)
        .collect()
}

/// A short id (a server id, a `coverArt` id): non-empty, capped, no NUL.
fn valid_text_id(id: &str) -> bool {
    !id.is_empty() && id.chars().count() <= MAX_TEXT && !id.contains('\0')
}

impl History {
    /// From a file's content, cleaned and trimmed to the bounds.
    pub fn from_file(file: HistoryFile) -> Self {
        let mut plays: VecDeque<Played> = file
            .plays
            .into_iter()
            .filter_map(|p| {
                Some(Played {
                    context: p.context.sanitized()?,
                    at: p.at,
                })
            })
            .collect();
        while plays.len() > MAX_PLAYS {
            plays.pop_front();
        }
        let mut counts = HashMap::new();
        for count in file.counts {
            let Some(context) = count.context.sanitized() else {
                continue;
            };
            if count.listens == 0 {
                continue;
            }
            let genres = match (context.source, context.kind) {
                (SourceKind::Navidrome, ContextKind::Album) => clean_genres(count.genres),
                _ => Vec::new(),
            };
            counts.insert(
                context.key(),
                Count {
                    context,
                    listens: count.listens,
                    last: count.last,
                    genres,
                },
            );
        }
        let mut moods: Vec<MoodsDay> = Vec::new();
        for day in file.moods.into_iter().filter_map(sanitize_moods_day) {
            moods.retain(|d| d.scope() != day.scope());
            moods.push(day);
        }
        let excess = moods.len().saturating_sub(MAX_MOOD_SCOPES);
        moods.drain(..excess);
        let mut tracks = HashMap::new();
        for count in file.tracks {
            let Some(track) = count.track.sanitized() else {
                continue;
            };
            if count.listens == 0 {
                continue;
            }
            tracks.insert(
                track.key(),
                TrackCount {
                    track,
                    listens: count.listens,
                    last: count.last,
                },
            );
        }
        let played: HashSet<ContextKey> = plays.iter().map(|p| p.context.key()).collect();
        let mut stops = HashMap::new();
        for stop in file.stops.into_iter().filter_map(Stop::sanitized) {
            let key = stop.key();
            // The first (most recent) of a context stays.
            if played.contains(&key) {
                stops.entry(key).or_insert(stop);
            }
        }
        let mut history = Self {
            plays,
            counts,
            tracks,
            stops,
            moods,
            revision: 0,
        };
        history.trim_counts();
        history.trim_tracks();
        history
    }

    /// What is saved (counts most recently heard first).
    pub fn to_file(&self) -> HistoryFile {
        let mut counts: Vec<Count> = self.counts.values().cloned().collect();
        counts.sort_by(|a, b| {
            b.last
                .cmp(&a.last)
                .then_with(|| b.listens.cmp(&a.listens))
                .then_with(|| a.context.id.cmp(&b.context.id))
        });
        let mut tracks: Vec<TrackCount> = self.tracks.values().cloned().collect();
        tracks.sort_by(|a, b| {
            b.last
                .cmp(&a.last)
                .then_with(|| b.listens.cmp(&a.listens))
                .then_with(|| a.track.id.cmp(&b.track.id))
        });
        let mut stops: Vec<Stop> = self.stops.values().cloned().collect();
        stops.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.id.cmp(&b.id)));
        HistoryFile {
            version: FILE_VERSION,
            plays: self.plays.iter().cloned().collect(),
            counts,
            tracks,
            stops,
            moods: self.moods.clone(),
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn plays(&self) -> impl Iterator<Item = &Played> {
        self.plays.iter()
    }

    pub fn count(&self, key: &ContextKey) -> Option<&Count> {
        self.counts.get(key)
    }

    pub fn counts_len(&self) -> usize {
        self.counts.len()
    }

    /// A context was played (as a whole or from one of its tracks).
    pub fn record_play(&mut self, context: Context, at: u64) {
        let Some(context) = context.sanitized() else {
            return;
        };
        self.plays.push_back(Played { context, at });
        let mut trimmed = false;
        while self.plays.len() > MAX_PLAYS {
            self.plays.pop_front();
            trimmed = true;
        }
        if trimmed && !self.stops.is_empty() {
            let played: HashSet<ContextKey> = self.plays.iter().map(|p| p.context.key()).collect();
            self.stops.retain(|key, _| played.contains(key));
        }
        self.revision += 1;
    }

    /// Where a context stopped (`stop.key()` must be among the plays,
    /// else nothing is kept). Replaces the context's earlier stop.
    pub fn record_stop(&mut self, stop: Stop) {
        let Some(stop) = stop.sanitized() else {
            return;
        };
        let played = self.plays.iter().any(|p| {
            let c = &p.context;
            c.source == stop.source
                && c.kind == stop.kind
                && c.id == stop.id
                && c.server == stop.server
        });
        if !played {
            return;
        }
        let key = stop.key();
        if self.stops.get(&key) == Some(&stop) {
            return;
        }
        self.stops.insert(key, stop);
        self.revision += 1;
    }

    /// The context played to its end: no stop to resume from.
    pub fn clear_stop(&mut self, key: &ContextKey) {
        if self.stops.remove(key).is_some() {
            self.revision += 1;
        }
    }

    pub fn stop(&self, key: &ContextKey) -> Option<&Stop> {
        self.stops.get(key)
    }

    /// A Navidrome album's genres as its songs had them (the latest
    /// non-empty set wins); for a counted album only.
    pub fn set_album_genres(&mut self, key: &ContextKey, genres: Vec<String>) {
        if key.source != SourceKind::Navidrome || key.kind != ContextKind::Album {
            return;
        }
        let genres = clean_genres(genres);
        if genres.is_empty() {
            return;
        }
        if let Some(count) = self.counts.get_mut(key)
            && count.genres != genres
        {
            count.genres = genres;
            self.revision += 1;
        }
    }

    /// A track was heard long enough: one listen for each context (the
    /// track's album, the context it was played from), once per key.
    pub fn record_listen(&mut self, contexts: Vec<Context>, at: u64) {
        let mut seen = HashSet::new();
        for context in contexts {
            let Some(context) = context.sanitized() else {
                continue;
            };
            let key = context.key();
            if !seen.insert(key.clone()) {
                continue;
            }
            let count = self.counts.entry(key).or_insert_with(|| Count {
                context: context.clone(),
                listens: 0,
                last: at,
                genres: Vec::new(),
            });
            // The latest name and cover win (a renamed playlist).
            count.context = context;
            count.listens = count.listens.saturating_add(1);
            count.last = count.last.max(at);
        }
        if !seen.is_empty() {
            self.trim_counts();
            self.revision += 1;
        }
    }

    /// A track was heard long enough: one listen.
    pub fn record_track_listen(&mut self, track: HeardTrack, at: u64) {
        let Some(track) = track.sanitized() else {
            return;
        };
        let count = self
            .tracks
            .entry(track.key())
            .or_insert_with(|| TrackCount {
                track: track.clone(),
                listens: 0,
                last: at,
            });
        // The latest row wins (a retitled song).
        count.track = track;
        count.listens = count.listens.saturating_add(1);
        count.last = count.last.max(at);
        self.trim_tracks();
        self.revision += 1;
    }

    pub fn track_count(&self, key: &TrackKey) -> Option<&TrackCount> {
        self.tracks.get(key)
    }

    pub fn tracks_len(&self) -> usize {
        self.tracks.len()
    }

    /// Least recently heard first out (ties: the fewer listens, then the
    /// id, so the result does not depend on hash order).
    fn trim_tracks(&mut self) {
        let excess = self.tracks.len().saturating_sub(MAX_TRACK_COUNTS);
        if excess == 0 {
            return;
        }
        let mut order: Vec<(u64, u64, &TrackKey)> = self
            .tracks
            .iter()
            .map(|(key, count)| (count.last, count.listens, key))
            .collect();
        let order = if excess < order.len() {
            order.select_nth_unstable_by(excess - 1, |a, b| {
                (a.0, a.1, &a.2.id).cmp(&(b.0, b.1, &b.2.id))
            });
            &order[..excess]
        } else {
            &order[..]
        };
        let gone: Vec<TrackKey> = order.iter().map(|(_, _, key)| (*key).clone()).collect();
        for key in gone {
            self.tracks.remove(&key);
        }
    }

    /// Listens per local track key, with the last listen.
    pub fn local_track_listens(&self) -> HashMap<String, (u64, u64)> {
        self.tracks
            .values()
            .filter(|c| c.track.source == SourceKind::Local)
            .map(|c| (c.track.id.clone(), (c.listens, c.last)))
            .collect()
    }

    /// `server`'s most listened songs by the artist with `artist_id`: a
    /// song with an artist id matches by it alone (two artists of the
    /// same name with different ids stay apart); only a song without one
    /// matches by `name` (its album artist or artist, ignoring case).
    /// Most first (ties: the more recent), at most `limit`.
    pub fn navidrome_top_tracks(
        &self,
        server: &str,
        artist_id: &str,
        name: &str,
        limit: usize,
    ) -> Vec<TrackCount> {
        let name = lower(name);
        let by = |song: &SongInfo| match song.artist_id.as_deref() {
            Some(id) => id == artist_id,
            None => {
                !name.is_empty()
                    && [&song.album_artist, &song.artist]
                        .into_iter()
                        .flatten()
                        .any(|n| lower(n) == name)
            }
        };
        let mut found: Vec<&TrackCount> = self
            .tracks
            .values()
            .filter(|c| {
                c.track.source == SourceKind::Navidrome
                    && c.track.server.as_deref() == Some(server)
                    && c.track.song.as_ref().is_some_and(by)
            })
            .collect();
        found.sort_by(|a, b| {
            b.listens
                .cmp(&a.listens)
                .then_with(|| b.last.cmp(&a.last))
                .then_with(|| a.track.id.cmp(&b.track.id))
        });
        found.into_iter().take(limit).cloned().collect()
    }

    /// Least recently heard first out.
    fn trim_counts(&mut self) {
        if self.counts.len() <= MAX_COUNTS {
            return;
        }
        let mut order: Vec<(u64, ContextKey)> = self
            .counts
            .iter()
            .map(|(key, count)| (count.last, key.clone()))
            .collect();
        order.sort_by_key(|(last, _)| *last);
        let excess = self.counts.len() - MAX_COUNTS;
        for (_, key) in order.into_iter().take(excess) {
            self.counts.remove(&key);
        }
    }

    /// The latest distinct contexts in `scope`, newest first, at most
    /// `limit`.
    pub fn recent(&self, scope: &Scope, limit: usize) -> Vec<Played> {
        let mut seen = HashSet::new();
        self.plays
            .iter()
            .rev()
            .filter(|p| scope.holds(&p.context))
            .filter(|p| seen.insert(p.context.key()))
            .take(limit)
            .cloned()
            .collect()
    }

    /// The album listens in `scope`, by the album's context id: listens
    /// and the last one (for the moods).
    pub fn album_listens(&self, scope: &Scope) -> HashMap<String, moods::Listens> {
        self.counts
            .values()
            .filter(|c| c.context.kind == ContextKind::Album && scope.holds(&c.context))
            .map(|c| {
                (
                    c.context.id.clone(),
                    moods::Listens {
                        listens: c.listens,
                        last: c.last,
                    },
                )
            })
            .collect()
    }

    /// `scope`'s Navidrome albums' genres with their listens (the moods'
    /// ranking; local albums' genres come from the tag index).
    pub fn heard_album_genres(&self, scope: &Scope) -> Vec<(Vec<String>, moods::Listens)> {
        let mut heard: Vec<&Count> = self
            .counts
            .values()
            .filter(|c| {
                c.context.kind == ContextKind::Album
                    && !c.genres.is_empty()
                    && scope.holds(&c.context)
            })
            .collect();
        heard.sort_by(|a, b| a.context.id.cmp(&b.context.id));
        heard
            .into_iter()
            .map(|c| {
                (
                    c.genres.clone(),
                    moods::Listens {
                        listens: c.listens,
                        last: c.last,
                    },
                )
            })
            .collect()
    }

    /// The cached moods of `scope` for `date`.
    pub fn moods(&self, scope: &Scope, date: &str) -> Option<&[Mood]> {
        self.moods
            .iter()
            .find(|d| d.date == date && d.scope() == *scope)
            .map(|d| d.moods.as_slice())
    }

    /// Caches `scope`'s moods of `date`, unless that day already has some
    /// (the first answer of the day stays). Returns the day's moods.
    pub fn set_moods(&mut self, scope: &Scope, date: String, moods: Vec<Mood>) -> Vec<Mood> {
        if let Some(moods) = self.moods(scope, &date) {
            return moods.to_vec();
        }
        let (source, server) = match scope {
            Scope::Local => (SourceKind::Local, None),
            Scope::Navidrome(server) => (SourceKind::Navidrome, Some(server.clone())),
        };
        let Some(day) = sanitize_moods_day(MoodsDay {
            date,
            source,
            server,
            moods,
        }) else {
            return Vec::new();
        };
        let moods = day.moods.clone();
        self.moods.retain(|d| d.scope() != *scope);
        self.moods.push(day);
        let excess = self.moods.len().saturating_sub(MAX_MOOD_SCOPES);
        self.moods.drain(..excess);
        self.revision += 1;
        moods
    }
}

// ----- the file -------------------------------------------------------------

/// What startup loaded.
#[derive(Debug, Default)]
pub struct Loaded {
    pub history: History,
    /// Where to save; `None` disables saving (no state folder, or a file
    /// that exists but could not be read, which is never overwritten).
    pub path: Option<PathBuf>,
}

/// Reads `path` (regular files only, never blocking on a FIFO). A file
/// that is not valid history is renamed to `history.json.bad` and the
/// history starts empty.
pub fn load(path: Option<PathBuf>) -> Loaded {
    let Some(path) = path else {
        return Loaded::default();
    };
    let text = match fsio::read_regular(&path, MAX_FILE_BYTES) {
        Ok(None) => {
            return Loaded {
                history: History::default(),
                path: Some(path),
            };
        }
        Ok(Some(text)) => Ok(text),
        Err(fsio::ReadError::Io(kind)) => {
            tracing::warn!(error = %kind, "the play history could not be read; not saving it this run");
            return Loaded::default();
        }
        // This build never writes a file this large (see `save`): a newer
        // build's, kept as it is.
        Err(fsio::ReadError::TooLarge(_)) => {
            tracing::warn!(
                "the play history is too large to read; keeping it, not saving this run"
            );
            return Loaded::default();
        }
        Err(error) => Err(error.to_string()),
    };
    /// Just the version, read before the rest.
    #[derive(Deserialize)]
    struct Version {
        version: u32,
    }
    if let Ok(t) = &text
        && let Ok(Version { version }) = serde_json::from_str::<Version>(t)
        && version > FILE_VERSION
    {
        // Written by a newer build: not corrupt, so keep it untouched.
        tracing::warn!(
            version,
            "the play history is from a newer version; keeping it, not saving this run"
        );
        return Loaded::default();
    }
    let parsed = text.and_then(|t| {
        let file: HistoryFile = serde_json::from_str(&t).map_err(|e| e.to_string())?;
        // Versions 1 and 2 lack what later versions added (it defaults to
        // empty): read as they are and saved as the current version.
        if !(OLDEST_VERSION..=FILE_VERSION).contains(&file.version) {
            return Err(format!("unknown version {}", file.version));
        }
        Ok(file)
    });
    match parsed {
        Ok(file) => Loaded {
            history: History::from_file(file),
            path: Some(path),
        },
        Err(error) => {
            tracing::warn!(%error, "the play history is not valid; set aside as history.json.bad.<time>");
            let aside = path.with_extension(format!("json.bad.{}", unix_now()));
            if let Err(error) = std::fs::rename(&path, aside) {
                tracing::warn!(error = %error.kind(), "could not set the play history aside; not saving it this run");
                return Loaded::default();
            }
            Loaded {
                history: History::default(),
                path: Some(path),
            }
        }
    }
}

/// Writes `file` atomically (a temporary file renamed over the old one),
/// never larger than `load` accepts.
pub fn save(path: &Path, file: &HistoryFile) -> std::io::Result<()> {
    fsio::write_atomic(path, &bounded_bytes(file, MAX_FILE_BYTES)?)
}

/// `file` as JSON within `max` bytes: while too large, the least recently
/// heard half of the counts and the oldest half of the plays are left out.
fn bounded_bytes(file: &HistoryFile, max: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(file).map_err(std::io::Error::other)?;
    if bytes.len() as u64 <= max {
        return Ok(bytes);
    }
    let mut trimmed = file.clone();
    while bytes.len() as u64 > max {
        if trimmed.counts.is_empty() && trimmed.plays.is_empty() && trimmed.tracks.is_empty() {
            trimmed.moods.clear();
        }
        // Counts and stops are most recent first, plays oldest first.
        trimmed.counts.truncate(trimmed.counts.len() / 2);
        trimmed.tracks.truncate(trimmed.tracks.len() / 2);
        trimmed.stops.truncate(trimmed.stops.len() / 2);
        trimmed.plays.drain(..trimmed.plays.len().div_ceil(2));
        bytes = serde_json::to_vec(&trimmed).map_err(std::io::Error::other)?;
        if trimmed.counts.is_empty()
            && trimmed.plays.is_empty()
            && trimmed.tracks.is_empty()
            && trimmed.stops.is_empty()
            && trimmed.moods.is_empty()
        {
            break;
        }
    }
    tracing::warn!(
        counts = trimmed.counts.len(),
        tracks = trimmed.tracks.len(),
        plays = trimmed.plays.len(),
        "the play history was too large to save whole; the oldest entries were left out"
    );
    Ok(bytes)
}

// ----- local keys and resolution ----------------------------------------

fn lower(text: &str) -> String {
    text.trim().to_lowercase()
}

/// A local album's stable key: its artist's and its own name, as grouped.
pub fn local_album_key(tags: &TagIndex, id: AlbumId) -> String {
    let album = tags.album(id);
    let artist = tags.artist(album.artist());
    format!("{}\u{1f}{}", lower(artist.name()), lower(album.name()))
}

/// A local track's stable key: its album's ([`local_album_key`], as
/// passed), then its disc and track numbers and its title, as tagged;
/// without a track number also its file name (never the path), so two
/// unnumbered tracks of one title on one album stay apart. Survives
/// rescans and moved files; a retagged (or, unnumbered, renamed) track
/// starts again. `None` for a track without an album tag: like its album
/// listens, it is not counted (all such tracks would share one album
/// key, "Unknown album").
pub fn local_track_key(album_key: &str, track: &Track) -> Option<String> {
    track.album.as_deref().filter(|a| !a.trim().is_empty())?;
    let number = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
    let mut key = format!(
        "{album_key}\u{1f}{}\u{1f}{}\u{1f}{}",
        number(track.disc_no),
        number(track.track_no),
        lower(&track.title)
    );
    if track.track_no.is_none() {
        let file = track
            .local_path()
            .and_then(|p| p.file_name())
            .map(|f| f.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        key.push('\u{1f}');
        key.push_str(&file);
    }
    Some(key)
}

/// Each album's listens (indexed by [`AlbumId`]): the sum of its
/// tracks' in `listens` ([`History::local_track_listens`]).
pub fn local_album_listens(library: &Library, listens: &HashMap<String, (u64, u64)>) -> Vec<u64> {
    let tags = library.tags();
    let albums = tags.albums(AlbumOrder::Name);
    let mut sums = vec![0u64; albums.len()];
    if listens.is_empty() {
        return sums;
    }
    for id in albums {
        let key = local_album_key(tags, *id);
        sums[id.0 as usize] = tags
            .album(*id)
            .tracks()
            .iter()
            .filter_map(|t| listens.get(&local_track_key(&key, library.track(*t))?))
            .fold(0u64, |sum, (n, _)| sum.saturating_add(*n));
    }
    sums
}

/// Up to `limit` of a local artist's tracks by their listens (most first;
/// ties: the more recent, then album order), each with its album and
/// listens. Tracks never heard are left out.
pub fn local_artist_top_tracks(
    library: &Library,
    artist: ArtistId,
    listens: &HashMap<String, (u64, u64)>,
    limit: usize,
) -> Vec<(TrackId, AlbumId, u64)> {
    let tags = library.tags();
    let mut found: Vec<(u64, u64, usize, TrackId, AlbumId)> = Vec::new();
    for album in tags.artist(artist).albums() {
        let key = local_album_key(tags, *album);
        for track in tags.album(*album).tracks() {
            if let Some((n, last)) =
                local_track_key(&key, library.track(*track)).and_then(|k| listens.get(&k))
            {
                found.push((*n, *last, found.len(), *track, *album));
            }
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    found
        .into_iter()
        .take(limit)
        .map(|(n, _, _, track, album)| (track, album, n))
        .collect()
}

pub fn local_artist_key(tags: &TagIndex, id: ArtistId) -> String {
    lower(tags.artist(id).name())
}

/// The local album context of `id`.
pub fn local_album_context(tags: &TagIndex, id: AlbumId) -> Context {
    let album = tags.album(id);
    Context {
        source: SourceKind::Local,
        server: None,
        kind: ContextKind::Album,
        id: local_album_key(tags, id),
        name: album.name().to_owned(),
        subtitle: Some(tags.artist(album.artist()).name().to_owned()),
        cover_art: None,
    }
}

pub fn local_artist_context(tags: &TagIndex, id: ArtistId) -> Context {
    Context {
        source: SourceKind::Local,
        server: None,
        kind: ContextKind::Artist,
        id: local_artist_key(tags, id),
        name: tags.artist(id).name().to_owned(),
        subtitle: None,
        cover_art: None,
    }
}

pub fn local_playlist_context(library: &Library, id: PlaylistId) -> Context {
    let name = library.playlist(id).name.clone();
    Context {
        source: SourceKind::Local,
        server: None,
        kind: ContextKind::Playlist,
        id: name.clone(),
        name,
        subtitle: None,
        cover_art: None,
    }
}

pub fn local_folder_context(path: &[String]) -> Context {
    Context {
        source: SourceKind::Local,
        server: None,
        kind: ContextKind::Folder,
        id: if path.is_empty() {
            "/".to_owned()
        } else {
            path.join("/")
        },
        name: path.last().cloned().unwrap_or_else(|| "Music".to_owned()),
        subtitle: None,
        cover_art: None,
    }
}

/// A folder context's path (folder names from the music root).
fn folder_path(id: &str) -> Vec<String> {
    if id == "/" {
        Vec::new()
    } else {
        id.split('/').map(str::to_owned).collect()
    }
}

/// Where a Home item stopped, resolved for playing it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StopDto {
    /// The entry's place among the context's tracks (local: in its tracks
    /// now; Navidrome: as they were queued).
    pub index: usize,
    /// The context's tracks (as above).
    pub total: usize,
    /// The entry's track as a `TrackDto` names it (a local id of this
    /// library generation, a song id); `None` when it is no longer there
    /// (then `index` is where it was).
    pub track_id: Option<String>,
    pub position_ms: u64,
}

/// One item of Where you left off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeItemDto {
    pub source: SourceKind,
    pub kind: ContextKind,
    /// The id to open or play it by now (a local id of this library
    /// generation, Navidrome's id, a mix id); for a folder, its path
    /// joined.
    pub id: String,
    /// Folders only: folder names from the music root.
    pub path: Option<Vec<String>>,
    pub name: String,
    pub subtitle: Option<String>,
    /// Albums: the artist to go to, when known.
    pub artist_id: Option<String>,
    pub cover_url: Option<String>,
    /// When it was last played or heard (unix seconds).
    pub played_at: u64,
    /// Where it stopped (none for a mix, or one played to its end).
    pub stop: Option<StopDto>,
}

/// The Home view's history: Where you left off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeDto {
    /// Newest first, at most [`RECENT_SHOWN`].
    pub recent: Vec<HomeItemDto>,
}

/// Looks local contexts up in the current library.
pub struct LocalIndex<'a> {
    view: &'a LocalView<'a>,
    albums: HashMap<String, AlbumId>,
    artists: HashMap<String, ArtistId>,
    playlists: HashMap<&'a str, PlaylistId>,
}

impl<'a> LocalIndex<'a> {
    pub fn new(view: &'a LocalView<'a>) -> Self {
        let tags = view.library.tags();
        let albums = tags
            .albums(AlbumOrder::Name)
            .iter()
            .map(|id| (local_album_key(tags, *id), *id))
            .collect();
        let artists = tags
            .artists()
            .iter()
            .map(|id| (local_artist_key(tags, *id), *id))
            .collect();
        let mut playlists = HashMap::new();
        for id in view.library.playlists() {
            playlists
                .entry(view.library.playlist(*id).name.as_str())
                .or_insert(*id);
        }
        Self {
            view,
            albums,
            artists,
            playlists,
        }
    }

    /// The album with stable key `key`, if it is still in the library.
    pub fn album(&self, key: &str) -> Option<AlbumId> {
        self.albums.get(key).copied()
    }

    /// The item for `context`, if it is still in the library (a mix: in
    /// `mixes`). `played_at` and `stop` are left empty.
    pub fn resolve(
        &self,
        context: &Context,
        mixes: &HashMap<String, Mix>,
        covers: &CoverService,
    ) -> Option<HomeItemDto> {
        let view = self.view;
        let library = view.library;
        let tags = library.tags();
        let generation = view.generation;
        let item = |id: String, name: String, subtitle: Option<String>, cover_url| HomeItemDto {
            source: SourceKind::Local,
            kind: context.kind,
            id,
            path: None,
            name,
            subtitle,
            artist_id: None,
            cover_url,
            played_at: 0,
            stop: None,
        };
        match context.kind {
            ContextKind::Album => {
                let id = *self.albums.get(&context.id)?;
                let album = tags.album(id);
                let artist = tags.artist(album.artist());
                Some(HomeItemDto {
                    artist_id: Some(local_id(generation, artist.id().0)),
                    ..item(
                        local_id(generation, id.0),
                        album.name().to_owned(),
                        Some(artist.name().to_owned()),
                        local::first_track_cover(library, album.tracks(), covers),
                    )
                })
            }
            ContextKind::Artist => {
                let id = *self.artists.get(&context.id)?;
                let artist = tags.artist(id);
                let cover = artist.albums().first().and_then(|a| {
                    local::first_track_cover(library, tags.album(*a).tracks(), covers)
                });
                Some(item(
                    local_id(generation, id.0),
                    artist.name().to_owned(),
                    None,
                    cover,
                ))
            }
            ContextKind::Playlist => {
                let id = *self.playlists.get(context.id.as_str())?;
                let playlist = library.playlist(id);
                Some(item(
                    local_id(generation, id.0),
                    playlist.name.clone(),
                    None,
                    local::first_track_cover(library, &playlist.tracks, covers),
                ))
            }
            ContextKind::Folder => {
                let path = folder_path(&context.id);
                let dir = library.tree().dir(&path)?;
                let mut first = None;
                library.tree().for_each_id(dir, |id| {
                    first.get_or_insert(id);
                });
                let cover = first.and_then(|id| local::first_track_cover(library, &[id], covers));
                Some(HomeItemDto {
                    path: Some(path.clone()),
                    ..item(
                        context.id.clone(),
                        path.last().cloned().unwrap_or_else(|| "Music".to_owned()),
                        None,
                        cover,
                    )
                })
            }
            ContextKind::Mix => {
                let mix = mixes.get(&context.id)?;
                let cover = mix
                    .ingredients
                    .iter()
                    .find_map(|i| self.ingredient_cover(i, covers));
                Some(item(context.id.clone(), mix.name.clone(), None, cover))
            }
        }
    }

    /// The cover of a local mix ingredient's first album.
    fn ingredient_cover(&self, ingredient: &Ingredient, covers: &CoverService) -> Option<String> {
        let library = self.view.library;
        let tags = library.tags();
        let album = match ingredient {
            Ingredient::Album { artist, name, .. } => {
                tags.album_by_name(tags.artist_by_name(artist)?, name)?
            }
            Ingredient::Artist { name, .. } => {
                *tags.artist(tags.artist_by_name(name)?).albums().first()?
            }
            Ingredient::Genre { name } => {
                *tags.genre(tags.genre_by_name(name)?).albums().first()?
            }
        };
        local::first_track_cover(library, tags.album(album).tracks(), covers)
    }

    /// The tracks a local context holds now, in its order (as it plays).
    fn context_tracks(&self, context: &Context) -> Option<Vec<TrackId>> {
        let library = self.view.library;
        let tags = library.tags();
        Some(match context.kind {
            ContextKind::Album => tags.album(*self.albums.get(&context.id)?).tracks().to_vec(),
            ContextKind::Playlist => library
                .playlist(*self.playlists.get(context.id.as_str())?)
                .tracks
                .clone(),
            ContextKind::Artist => tags
                .artist(*self.artists.get(&context.id)?)
                .albums()
                .iter()
                .flat_map(|a| tags.album(*a).tracks().iter().copied())
                .collect(),
            ContextKind::Folder => {
                let dir = library.tree().dir(&folder_path(&context.id))?;
                let mut ids = Vec::new();
                library.tree().for_each_id(dir, |id| ids.push(id));
                ids
            }
            ContextKind::Mix => return None,
        })
    }

    /// `stop` in the context's tracks now: the stopped track's place if it
    /// is still there, else the recorded place (within the tracks there
    /// are), without a track.
    pub fn resolve_stop(&self, context: &Context, stop: &Stop) -> Option<StopDto> {
        let tracks = self.context_tracks(context)?;
        if tracks.is_empty() {
            return None;
        }
        let library = self.view.library;
        let tags = library.tags();
        let found = tracks.iter().position(|id| {
            let track = library.track(*id);
            tags.album_of(track)
                .and_then(|album| local_track_key(&local_album_key(tags, album), track))
                .is_some_and(|key| key == stop.track)
        });
        Some(match found {
            Some(index) => StopDto {
                index,
                total: tracks.len(),
                track_id: Some(local_id(self.view.generation, tracks[index].0)),
                position_ms: stop.position_ms,
            },
            None => StopDto {
                index: stop.index.min(tracks.len() - 1),
                total: tracks.len(),
                track_id: None,
                position_ms: 0,
            },
        })
    }
}

/// A Navidrome context's item (from what was stored; a mix only while it
/// is in `mixes`, with its current name).
pub fn navidrome_item(
    context: &Context,
    mixes: &HashMap<String, Mix>,
    covers: &CoverService,
) -> Option<HomeItemDto> {
    let name = match context.kind {
        ContextKind::Mix => mixes.get(&context.id)?.name.clone(),
        _ => context.name.clone(),
    };
    Some(HomeItemDto {
        source: SourceKind::Navidrome,
        kind: context.kind,
        id: context.id.clone(),
        path: None,
        name,
        subtitle: context.subtitle.clone(),
        artist_id: None,
        cover_url: context
            .cover_art
            .clone()
            .and_then(|c| covers.url_for_key(coda_core::cover::CoverKey::Navidrome(c))),
        played_at: 0,
        stop: None,
    })
}

/// What the engine hands the Home command: the source's recent contexts
/// (more than shown: some may no longer resolve), and their stops.
#[derive(Debug, Clone, Default)]
pub struct HomeRows {
    pub recent: Vec<Played>,
    pub stops: Vec<Stop>,
}

impl History {
    pub fn home_rows(&self, scope: &Scope) -> HomeRows {
        let recent = self.recent(scope, RECENT_SHOWN * 4);
        let stops = recent
            .iter()
            .filter_map(|p| self.stops.get(&p.context.key()).cloned())
            .collect();
        HomeRows { recent, stops }
    }
}

/// Resolves `rows` to items (local ones against the library; `view` is
/// `None` for Navidrome; mixes against `mixes`, by id). Artists and
/// folders without a cover are left out.
pub fn home_dto(
    rows: HomeRows,
    view: Option<&LocalView<'_>>,
    mixes: &HashMap<String, Mix>,
    covers: &CoverService,
) -> HomeDto {
    let index = view.map(LocalIndex::new);
    let stops: HashMap<ContextKey, &Stop> = rows.stops.iter().map(|s| (s.key(), s)).collect();
    let recent = rows
        .recent
        .iter()
        .filter_map(|played| {
            let context = &played.context;
            let item = match (&index, context.source) {
                (Some(index), SourceKind::Local) => index.resolve(context, mixes, covers),
                (_, SourceKind::Navidrome) => navidrome_item(context, mixes, covers),
                (None, SourceKind::Local) => None,
            }?;
            if matches!(context.kind, ContextKind::Artist | ContextKind::Folder)
                && item.cover_url.is_none()
            {
                return None;
            }
            let stopped = stops.get(&context.key()).copied();
            let stop = stopped.and_then(|stop| match (&index, context.source) {
                (Some(index), SourceKind::Local) => index.resolve_stop(context, stop),
                (_, SourceKind::Navidrome) => Some(StopDto {
                    index: stop.index,
                    total: stop.total,
                    track_id: Some(stop.track.clone()),
                    position_ms: stop.position_ms,
                }),
                (None, SourceKind::Local) => None,
            });
            Some(HomeItemDto {
                played_at: played.at.max(stopped.map_or(0, |s| s.at)),
                stop,
                ..item
            })
        })
        .take(RECENT_SHOWN)
        .collect();
    HomeDto { recent }
}

/// FNV-1a: the same seed for the same text on every run.
pub(crate) fn seed(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests;
