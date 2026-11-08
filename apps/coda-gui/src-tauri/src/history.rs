//! Play history for the Home view: which contexts (albums, artists,
//! playlists, folders) were played, how often each was listened to, and
//! the day's cached Navidrome picks.
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
//! the Daily Picks are cached per server too. The file never holds stream
//! URLs, credentials, server addresses or cover tokens.
//!
//! Bounded: the last [`MAX_PLAYS`] context plays, listen counts for at
//! most [`MAX_COUNTS`] contexts (the least recently heard dropped first).

pub mod commands;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use coda_core::library::tags::{AlbumId, AlbumOrder, ArtistId, TagIndex};
use coda_core::library::{Library, PlaylistId};
use serde::{Deserialize, Serialize};

use crate::browse::dto::AlbumDto;
use crate::browse::{LocalView, local, local_id};
use crate::cover::CoverService;
use crate::theme::fsio;

/// Context plays kept (newest last).
pub const MAX_PLAYS: usize = 500;
/// Contexts with listen counts kept.
pub const MAX_COUNTS: usize = 5_000;
/// Distinct recently played contexts shown.
pub const RECENT_SHOWN: usize = 20;
/// Most played contexts shown.
pub const MOST_SHOWN: usize = 20;
/// Albums in the Daily Picks.
pub const DAILY_PICKS: usize = 12;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const FILE_VERSION: u32 = 1;
/// Names and subtitles are cut to this many characters.
const MAX_TEXT: usize = 256;
/// Ids (a folder path can be long) are refused beyond this.
const MAX_ID: usize = 4096;
/// Servers whose Daily Picks are cached at once.
const MAX_DAILY_SERVERS: usize = 4;

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
}

/// A Navidrome server's identity in the history: FNV-1a (64 bit, hex) of
/// its base URL (normalised by the client) and user name. Stable across
/// runs; neither is stored in plain text, though a known server's id can
/// be confirmed by hashing (it is not a secret).
pub fn server_id(client: &coda_core::navidrome::Client) -> String {
    server_id_of(client.base_url().as_str(), client.username())
}

pub fn server_id_of(base_url: &str, username: &str) -> String {
    format!("{:016x}", seed(&format!("{base_url}\n{username}")))
}

fn valid_server(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit())
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
}

/// One album of the cached Navidrome Daily Picks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyAlbum {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub artist_id: Option<String>,
    #[serde(default)]
    pub year: Option<u32>,
    #[serde(default)]
    pub cover_art: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Daily {
    /// The local calendar day, `YYYY-MM-DD`.
    pub date: String,
    /// The [`server_id`] they were drawn from.
    pub server: String,
    pub albums: Vec<DailyAlbum>,
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
    /// The latest Daily Picks per server (at most [`MAX_DAILY_SERVERS`]).
    #[serde(default)]
    pub dailies: Vec<Daily>,
}

/// The history in memory. Every change bumps [`History::revision`].
#[derive(Debug, Clone, Default)]
pub struct History {
    /// Oldest first.
    plays: VecDeque<Played>,
    counts: HashMap<ContextKey, Count>,
    /// Oldest first, one per server.
    dailies: Vec<Daily>,
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

fn sanitize_daily(daily: Daily) -> Option<Daily> {
    if !valid_date(&daily.date) || !valid_server(&daily.server) {
        return None;
    }
    let albums = daily
        .albums
        .into_iter()
        .filter_map(sanitize_daily_album)
        .take(DAILY_PICKS)
        .collect();
    Some(Daily {
        date: daily.date,
        server: daily.server,
        albums,
    })
}

fn sanitize_daily_album(mut album: DailyAlbum) -> Option<DailyAlbum> {
    let ok = |id: &str| !id.is_empty() && id.chars().count() <= MAX_TEXT && !id.contains('\0');
    if !ok(&album.id) {
        return None;
    }
    album.name = clean(&album.name);
    album.artist = album.artist.as_deref().map(clean).filter(|a| !a.is_empty());
    album.artist_id = album.artist_id.filter(|id| ok(id));
    album.cover_art = album.cover_art.filter(|id| ok(id));
    Some(album)
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
            counts.insert(
                context.key(),
                Count {
                    context,
                    listens: count.listens,
                    last: count.last,
                },
            );
        }
        let mut dailies: Vec<Daily> = Vec::new();
        for daily in file.dailies.into_iter().filter_map(sanitize_daily) {
            dailies.retain(|d| d.server != daily.server);
            dailies.push(daily);
        }
        let excess = dailies.len().saturating_sub(MAX_DAILY_SERVERS);
        dailies.drain(..excess);
        let mut history = Self {
            plays,
            counts,
            dailies,
            revision: 0,
        };
        history.trim_counts();
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
        HistoryFile {
            version: FILE_VERSION,
            plays: self.plays.iter().cloned().collect(),
            counts,
            dailies: self.dailies.clone(),
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
        while self.plays.len() > MAX_PLAYS {
            self.plays.pop_front();
        }
        self.revision += 1;
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

    /// The most listened contexts in `scope` among `kinds`, most first
    /// (ties: the more recent), at most `limit`.
    pub fn most_played(&self, scope: &Scope, kinds: &[ContextKind], limit: usize) -> Vec<Count> {
        let mut counts: Vec<&Count> = self
            .counts
            .values()
            .filter(|c| scope.holds(&c.context) && kinds.contains(&c.context.kind))
            .collect();
        counts.sort_by(|a, b| {
            b.listens
                .cmp(&a.listens)
                .then_with(|| b.last.cmp(&a.last))
                .then_with(|| a.context.id.cmp(&b.context.id))
        });
        counts.into_iter().take(limit).cloned().collect()
    }

    /// The cached picks of `server` for `date`.
    pub fn daily(&self, date: &str, server: &str) -> Option<&[DailyAlbum]> {
        self.dailies
            .iter()
            .find(|d| d.date == date && d.server == server)
            .map(|d| d.albums.as_slice())
    }

    /// Caches `server`'s picks of `date`, unless that day already has some
    /// (the first answer of the day stays). Returns the day's picks.
    pub fn set_daily(
        &mut self,
        date: String,
        server: String,
        albums: Vec<DailyAlbum>,
    ) -> Vec<DailyAlbum> {
        if let Some(albums) = self.daily(&date, &server) {
            return albums.to_vec();
        }
        let Some(daily) = sanitize_daily(Daily {
            date,
            server,
            albums,
        }) else {
            return Vec::new();
        };
        let albums = daily.albums.clone();
        self.dailies.retain(|d| d.server != daily.server);
        self.dailies.push(daily);
        let excess = self.dailies.len().saturating_sub(MAX_DAILY_SERVERS);
        self.dailies.drain(..excess);
        self.revision += 1;
        albums
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
        if file.version != FILE_VERSION {
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
        if trimmed.counts.is_empty() && trimmed.plays.is_empty() {
            trimmed.dailies.clear();
        }
        // Counts are most recent first, plays oldest first.
        trimmed.counts.truncate(trimmed.counts.len() / 2);
        trimmed.plays.drain(..trimmed.plays.len().div_ceil(2));
        bytes = serde_json::to_vec(&trimmed).map_err(std::io::Error::other)?;
        if trimmed.counts.is_empty() && trimmed.plays.is_empty() && trimmed.dailies.is_empty() {
            break;
        }
    }
    tracing::warn!(
        counts = trimmed.counts.len(),
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

/// One Home card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeItemDto {
    pub source: SourceKind,
    pub kind: ContextKind,
    /// The id to open or play it by now (a local id of this library
    /// generation, or Navidrome's id); for a folder, its path joined.
    pub id: String,
    /// Folders only: folder names from the music root.
    pub path: Option<Vec<String>>,
    pub name: String,
    pub subtitle: Option<String>,
    /// Albums: the artist to go to, when known.
    pub artist_id: Option<String>,
    pub cover_url: Option<String>,
    /// Most played: the listens counted.
    pub listens: Option<u64>,
}

/// The history rows of the Home view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeDto {
    pub recent: Vec<HomeItemDto>,
    pub most_played: Vec<HomeItemDto>,
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

    /// The card for `context`, if it is still in the library.
    pub fn resolve(&self, context: &Context, covers: &CoverService) -> Option<HomeItemDto> {
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
            listens: None,
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
        }
    }
}

/// A Navidrome context's card (from what was stored).
pub fn navidrome_item(context: &Context, covers: &CoverService) -> HomeItemDto {
    HomeItemDto {
        source: SourceKind::Navidrome,
        kind: context.kind,
        id: context.id.clone(),
        path: None,
        name: context.name.clone(),
        subtitle: context.subtitle.clone(),
        artist_id: None,
        cover_url: context
            .cover_art
            .clone()
            .and_then(|c| covers.url_for_key(coda_core::cover::CoverKey::Navidrome(c))),
        listens: None,
    }
}

/// What the engine hands the Home command: the source's rows, unresolved.
#[derive(Debug, Clone, Default)]
pub struct HomeRows {
    pub recent: Vec<Played>,
    pub most_played: Vec<Count>,
}

impl History {
    /// More than shown: some local ones may no longer resolve.
    pub fn home_rows(&self, scope: &Scope) -> HomeRows {
        let kinds: &[ContextKind] = match scope.source() {
            // Navidrome's most played albums come from the server
            // (`frequent`, built from scrobbles, the TUI's too).
            SourceKind::Navidrome => &[ContextKind::Playlist],
            SourceKind::Local => &[ContextKind::Album, ContextKind::Playlist],
        };
        HomeRows {
            recent: self.recent(scope, RECENT_SHOWN * 2),
            most_played: self.most_played(scope, kinds, MOST_SHOWN * 2),
        }
    }
}

/// Resolves `rows` to cards (local ones against the library; `view` is
/// `None` for Navidrome).
pub fn home_dto(rows: HomeRows, view: Option<&LocalView<'_>>, covers: &CoverService) -> HomeDto {
    let index = view.map(LocalIndex::new);
    let resolve = |context: &Context| match (&index, context.source) {
        (Some(index), SourceKind::Local) => index.resolve(context, covers),
        (_, SourceKind::Navidrome) => Some(navidrome_item(context, covers)),
        (None, SourceKind::Local) => None,
    };
    let recent = rows
        .recent
        .iter()
        .filter_map(|p| resolve(&p.context))
        .take(RECENT_SHOWN)
        .collect();
    let most_played = rows
        .most_played
        .iter()
        .filter_map(|c| {
            resolve(&c.context).map(|item| HomeItemDto {
                listens: Some(c.listens),
                ..item
            })
        })
        .take(MOST_SHOWN)
        .collect();
    HomeDto {
        recent,
        most_played,
    }
}

// ----- Daily Picks ----------------------------------------------------------

/// FNV-1a: the same seed for the same day on every run.
fn seed(date: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in date.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The day's local picks: the [`DAILY_PICKS`] albums whose keys rank
/// lowest under a hash seeded by the date. Each album's rank depends only
/// on the date and its own key, never on scan order or on the other
/// albums, so the picks stay the same all day and across restarts, and a
/// library edit changes them only where an added album outranks one or a
/// picked album goes away.
pub fn local_daily_picks(view: &LocalView<'_>, date: &str, covers: &CoverService) -> Vec<AlbumDto> {
    let tags = view.library.tags();
    let mut albums: Vec<(u64, String, AlbumId)> = tags
        .albums(AlbumOrder::Name)
        .iter()
        .map(|id| {
            let key = local_album_key(tags, *id);
            (seed(&format!("{date}\n{key}")), key, *id)
        })
        .collect();
    let picks = DAILY_PICKS.min(albums.len());
    if picks < albums.len() {
        albums.select_nth_unstable(picks);
        albums.truncate(picks);
    }
    albums.sort();
    albums
        .into_iter()
        .map(|(_, _, id)| local::album_dto(view, id, covers))
        .collect()
}

pub fn daily_album(album: coda_core::navidrome::Album) -> DailyAlbum {
    DailyAlbum {
        id: album.id,
        name: album.name,
        artist: album.artist,
        artist_id: album.artist_id,
        year: album.year,
        cover_art: album.cover_art,
    }
}

pub fn daily_album_dto(album: &DailyAlbum, covers: &CoverService) -> AlbumDto {
    AlbumDto {
        id: album.id.clone(),
        name: album.name.clone(),
        artist: album.artist.clone(),
        artist_id: album.artist_id.clone(),
        year: album.year,
        track_count: None,
        duration_ms: None,
        cover_url: album
            .cover_art
            .clone()
            .and_then(|c| covers.url_for_key(coda_core::cover::CoverKey::Navidrome(c))),
    }
}

#[cfg(test)]
mod tests;
