//! Remote browse state for the Navidrome source: a navigation stack of
//! views (Home → Playlists / Artists / Albums → …, plus search results), a
//! session-only cache of fetched lists, paged album lists, the search
//! input, and ordered background enqueue jobs. No Ratatui, no I/O.
//!
//! [`Browser`] never fetches anything itself. Like the player emits
//! playback commands, it queues [`FetchRequest`]s (taken with
//! [`Browser::take_requests`]); the event loop runs each with
//! [`spawn_fetch`] and feeds the [`FetchResponse`] back through
//! [`Browser::handle_response`].
//!
//! Rules:
//! - Every request has a [`RequestId`]. At most one request per
//!   [`Resource`] is in flight; opening a view whose list is already being
//!   fetched awaits that request instead of sending another.
//! - A successful response always replaces the cache entry. A view only
//!   changes if it is still awaiting that request id, so a response for a
//!   view the user has left is cached but changes nothing on screen.
//! - Album lists ([`Resource::Albums`]) are paged, one sort order each;
//!   see [`Paging`]. At most one page request per list is in flight, and
//!   a page response is applied only if it is that request (same id and
//!   offset) for the list as it is now; anything else is ignored. `r`
//!   while a later page is in flight waits for it, drops it and then
//!   starts over at the first page. Albums already in a list are skipped,
//!   and a full page that adds none ends it. The random order is a single
//!   page and never cached: it is dropped when no view shows it, and
//!   opening it or `r` reshuffles.
//! - Enqueue jobs complete strictly in the order they were created (key
//!   presses), whatever order their responses arrive in. A job whose data
//!   failed to load is dropped with an error, in its turn.
//! - Jobs fetch lazily: at most [`MAX_JOB_FETCHES`] of their requests are
//!   in flight at once, below [`MAX_CONCURRENT_FETCHES`], so `a` on an
//!   artist with hundreds of albums never queues view fetches behind it.
//! - Every spawned fetch answers exactly once: if its task is dropped or
//!   panics first, an [`Error::Aborted`] response is sent instead.
//! - A browser belongs to one client [`Generation`] (the app starts a new
//!   one when the Navidrome account changes). Its requests carry it, and
//!   a response from another generation is ignored, so nothing fetched
//!   from an old server lands in the new browser.
//!
//! Playlist editing (the user's own playlists only, by the `owner` the
//! server reports):
//! - Changes are [`EditRequest`]s, queued and handed out one at a time
//!   ([`Browser::take_edit_requests`]); the loop runs each as a small
//!   sequential job ([`super::edit`]) and feeds the [`EditResponse`] back
//!   ([`Browser::handle_edit_response`]). A response from another
//!   generation is ignored, so an edit in flight across an account swap
//!   is dropped.
//! - "Add to playlist" reuses the enqueue jobs: a job's destination is the
//!   queue or a playlist, and a playlist job turns into an edit in its
//!   turn. The picker lists "New playlist…" and the user's own playlists
//!   A–Z.
//! - Moving songs in a playlist view changes a local draft (one at a
//!   time), shown instead of the cached list until it is saved (one
//!   replace) or discarded.
//! - After every edit, the Playlists list and the playlist are reloaded: a
//!   shown list is refetched with a new request that supersedes any one
//!   in flight (whose answer is then neither cached nor shown); a list no
//!   view shows is only dropped from the cache.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use tokio::sync::{Semaphore, mpsc};

use super::api::{MAX_ALBUM_LIST_SIZE, MAX_PLAYLIST_ENTRIES};
use super::edit::{EditId, EditOp, EditRequest, EditResponse, EditResult};
use super::{
    Access, Album, AlbumPage, AlbumSort, Artist, Client, Error, Hidden, Playlist, PlaylistSongs,
    SearchResults,
};
use crate::line_input::LineInput;
use crate::model::{Track, thousands};
use crate::queue::Placement;

pub use crate::line_input::MAX_QUERY_CHARS;
/// Upper bound on concurrently running fetches.
pub const MAX_CONCURRENT_FETCHES: usize = 4;
/// Upper bound on fetches in flight for enqueue jobs; the remaining
/// capacity is kept for views.
pub const MAX_JOB_FETCHES: usize = 2;
const _: () = assert!(MAX_JOB_FETCHES < MAX_CONCURRENT_FETCHES);
/// Albums requested per page.
pub const ALBUM_PAGE_SIZE: usize = MAX_ALBUM_LIST_SIZE;
/// Upper bound on the albums held for one sort order; paging stops there.
pub const MAX_ALBUMS: usize = 50_000;
/// Rows assumed visible until the UI reports the real number.
pub const DEFAULT_VIEWPORT_ROWS: usize = 40;
/// Title of the root view.
pub const HOME_TITLE: &str = "Home";
pub const ARTISTS_TITLE: &str = "Artists";
pub const PLAYLISTS_TITLE: &str = "Playlists";
/// Upper bound on the songs one edit adds (the playlist entry cap).
pub const MAX_EDIT_SONGS: usize = MAX_PLAYLIST_ENTRIES;
/// After an edit refused because the playlist changed on the server.
pub const STALE_PLAYLIST: &str = "Playlist changed on the server; reloaded";
/// Refusals of edits to a playlist the user may not change.
pub const READ_ONLY_OTHER: &str = "Read-only: this playlist belongs to another user";
pub const READ_ONLY_FLAGGED: &str = "Read-only: the server does not allow changing this playlist";
pub const READ_ONLY_UNKNOWN: &str = "Read-only: the server does not say who owns this playlist";
/// Refusal of index-based edits when not every entry could be read.
pub const INCOMPLETE_PLAYLIST: &str =
    "This playlist has entries Coda cannot show; edit it in Navidrome";
/// Refusal while the shown playlist has an unsaved order.
pub const UNSAVED_ORDER: &str = "Unsaved order: w saves, Esc discards";
/// Refusal while an earlier edit of the same playlist has not answered.
pub const WAITING_FOR_EDIT: &str = "Waiting for the previous change…";
/// Refusal while the new order is being saved.
pub const SAVING_ORDER: &str = "Saving the new order…";
/// After Esc on an unsaved order.
pub const ORDER_DISCARDED: &str = "New order discarded";
/// Refusal of removing a playlist's only song (Navidrome ignores an empty
/// replace).
pub const LAST_SONG: &str = "Cannot remove the last song; delete the playlist instead";
/// Title suffix of a playlist with an unsaved order.
pub const MODIFIED: &str = " (modified)";
/// The first row of the playlist picker.
pub const NEW_PLAYLIST_ROW: &str = "New playlist…";

/// Why `username` may not edit a playlist with `access`. Another owner
/// comes first: Navidrome also flags other users' playlists `readonly`.
pub fn read_only_reason(access: &Access, username: &str) -> &'static str {
    match &access.owner {
        Some(owner) if owner.to_lowercase() != username.to_lowercase() => READ_ONLY_OTHER,
        _ if access.readonly => READ_ONLY_FLAGGED,
        None => READ_ONLY_UNKNOWN,
        Some(_) => READ_ONLY_FLAGGED,
    }
}

/// `1 song`, `1,234 songs`.
fn songs_text(n: usize) -> String {
    format!("{} song{}", thousands(n), if n == 1 { "" } else { "s" })
}

/// An entry of the Home view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeEntry {
    Playlists,
    Artists,
    Albums,
}

impl HomeEntry {
    pub fn label(self) -> &'static str {
        match self {
            Self::Playlists => PLAYLISTS_TITLE,
            Self::Artists => ARTISTS_TITLE,
            Self::Albums => "Albums",
        }
    }
}

/// The Home view's rows, in order.
pub const HOME_ENTRIES: [HomeEntry; 3] =
    [HomeEntry::Playlists, HomeEntry::Artists, HomeEntry::Albums];

/// Title of an album list view, e.g. `Albums (newest)`.
pub fn albums_title(sort: AlbumSort) -> String {
    format!("Albums ({})", sort.label())
}

/// A list the server can be asked for; also the cache key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Resource {
    /// The static Home menu; never fetched.
    Home,
    /// All artists (`getArtists`).
    Artists,
    /// The albums of an artist (`getArtist`).
    Artist(String),
    /// The songs of an album (`getAlbum`).
    Album(String),
    /// `search3` results for a (trimmed, non-empty) query.
    Search(String),
    /// The user's playlists (`getPlaylists`).
    Playlists,
    /// The songs of a playlist (`getPlaylist`).
    Playlist(String),
    /// Every album in one order, as loaded so far: a view and cache key,
    /// filled page by page. Never fetched as such.
    Albums(AlbumSort),
    /// One page of [`Resource::Albums`] (`getAlbumList2`); requested, never
    /// cached or shown.
    AlbumPage { sort: AlbumSort, offset: usize },
}

impl Resource {
    /// The Subsonic endpoint, for logs.
    pub fn endpoint(&self) -> &'static str {
        match self {
            Self::Home => "none",
            Self::Artists => "getArtists",
            Self::Artist(_) => "getArtist",
            Self::Album(_) => "getAlbum",
            Self::Search(_) => "search3",
            Self::Playlists => "getPlaylists",
            Self::Playlist(_) => "getPlaylist",
            Self::Albums(_) | Self::AlbumPage { .. } => "getAlbumList2",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(pub u64);

/// Which Navidrome client a request was made for. Request ids restart in
/// every new browser, so the id alone cannot tell responses apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Generation(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRequest {
    pub generation: Generation,
    pub id: RequestId,
    pub resource: Resource,
}

#[derive(Debug)]
pub struct FetchResponse {
    pub generation: Generation,
    pub id: RequestId,
    pub resource: Resource,
    pub result: Result<Content, Error>,
}

/// A fetched list. The variant follows from the [`Resource`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// The Home menu ([`HOME_ENTRIES`]).
    Home,
    Artists(Vec<Artist>),
    Albums(Vec<Album>),
    Songs(Vec<Track>),
    Search(SearchResults),
    Playlists(Vec<Playlist>),
    /// The songs of a playlist, which may have been cut at the entry cap.
    PlaylistSongs(PlaylistSongs),
    /// A page of an album list, as fetched; the cache holds the pages
    /// joined as [`Content::Albums`].
    AlbumPage(AlbumPage),
}

static HOME: Content = Content::Home;

/// Section header in search results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Artists,
    Albums,
    Songs,
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Self::Artists => "Artists",
            Self::Albums => "Albums",
            Self::Songs => "Songs",
        }
    }
}

/// One row of a remote view. Headers only appear in search results and
/// are never selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row<'a> {
    Header(Section),
    Home(HomeEntry),
    Artist(&'a Artist),
    Album(&'a Album),
    Song(&'a Track),
    Playlist(&'a Playlist),
}

impl Row<'_> {
    pub fn is_selectable(&self) -> bool {
        !matches!(self, Row::Header(_))
    }
}

impl Content {
    pub fn len(&self) -> usize {
        match self {
            Self::Home => HOME_ENTRIES.len(),
            Self::Artists(v) => v.len(),
            Self::Albums(v) => v.len(),
            Self::Songs(v) => v.len(),
            Self::Search(r) => search_rows(r).count(),
            Self::Playlists(v) => v.len(),
            Self::PlaylistSongs(p) => p.tracks.len(),
            Self::AlbumPage(page) => page.albums.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn row(&self, i: usize) -> Option<Row<'_>> {
        self.rows(i, 1).pop()
    }

    /// At most `len` rows starting at `start`, without building the rest.
    pub fn rows(&self, start: usize, len: usize) -> Vec<Row<'_>> {
        match self {
            Self::Home => window(&HOME_ENTRIES, start, len)
                .iter()
                .map(|e| Row::Home(*e))
                .collect(),
            Self::Artists(v) => window(v, start, len).iter().map(Row::Artist).collect(),
            Self::Albums(v) => window(v, start, len).iter().map(Row::Album).collect(),
            Self::AlbumPage(page) => window(&page.albums, start, len)
                .iter()
                .map(Row::Album)
                .collect(),
            Self::Songs(v) => window(v, start, len).iter().map(Row::Song).collect(),
            Self::PlaylistSongs(p) => window(&p.tracks, start, len)
                .iter()
                .map(Row::Song)
                .collect(),
            Self::Playlists(v) => window(v, start, len).iter().map(Row::Playlist).collect(),
            Self::Search(r) => search_rows(r).skip(start).take(len).collect(),
        }
    }
}

fn window<T>(items: &[T], start: usize, len: usize) -> &[T] {
    let start = start.min(items.len());
    let end = start.saturating_add(len).min(items.len());
    &items[start..end]
}

/// Search results as rows: each non-empty section with its header.
fn search_rows(results: &SearchResults) -> impl Iterator<Item = Row<'_>> {
    fn section<'a, T>(
        header: Section,
        items: &'a [T],
        row: fn(&'a T) -> Row<'a>,
    ) -> impl Iterator<Item = Row<'a>> {
        let header = (!items.is_empty()).then_some(Row::Header(header));
        header.into_iter().chain(items.iter().map(row))
    }
    section(Section::Artists, &results.artists, Row::Artist)
        .chain(section(Section::Albums, &results.albums, Row::Album))
        .chain(section(Section::Songs, &results.tracks, Row::Song))
}

/// The load state of a view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Load {
    /// Awaiting this request.
    Loading(RequestId),
    /// The list is in the cache.
    Ready,
    /// Short, URL-free error text.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub resource: Resource,
    /// Untrusted when it comes from the server (artist/album names).
    pub title: String,
    pub selected: usize,
    pub load: Load,
}

/// Loading state of the rest of a paged list, below its loaded rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum More {
    /// Nothing to show: complete, or not needed yet.
    Idle,
    /// The next page is being fetched.
    Loading,
    /// The next page failed (short, URL-free text); `r` retries it.
    Failed(String),
}

/// Paging state of one album list (one sort order).
///
/// The list holds the pages `0..loaded` (by server offset). At most one
/// page request of the list is in flight, `pending`, and a response is
/// applied only if it matches it exactly. When the list starts over (`r`,
/// or random reopened or left) while a page is in flight, that request
/// stays the one in flight but is marked `discard`: its answer is dropped
/// and, if a view still awaits the list, the first page is requested
/// then. The list is complete after a page shorter than
/// [`ALBUM_PAGE_SIZE`] or once [`MAX_ALBUMS`] offsets are loaded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Paging {
    /// Offset of the next page: the items the server sent so far.
    loaded: usize,
    /// The page request in flight: request id and offset.
    pending: Option<(RequestId, usize)>,
    /// The pending request belongs to a list that has since started over.
    discard: bool,
    complete: bool,
    /// A page after the first failed; nothing more loads until `r`.
    error: Option<String>,
    /// Ids of the albums in the list; a page's albums already in it are
    /// skipped.
    seen: HashSet<String>,
}

impl Paging {
    /// Starts the list over, keeping the request in flight (if any) but
    /// marking it for discarding unless it is a first page still wanted.
    fn start_over(&mut self, keep_first_page: bool) {
        let pending = self.pending;
        let wanted = keep_first_page && !self.discard && matches!(pending, Some((_, 0)));
        *self = Self {
            pending,
            discard: pending.is_some() && !wanted,
            ..Self::default()
        };
    }

    /// Whether the page after `loaded` should be requested now, with the
    /// selection at `selected` of `len` rows and `lookahead` rows visible.
    fn wants_more(&self, selected: usize, len: usize, lookahead: usize) -> bool {
        !self.complete
            && self.pending.is_none()
            && self.error.is_none()
            && selected.saturating_add(lookahead) >= len
    }

    /// Records `page`, the awaited page at `offset`, and returns its
    /// albums not already in the list. The list is complete after a short
    /// page, a full page that adds nothing new (a server ignoring `offset`,
    /// or a page of unusable items), at [`MAX_ALBUMS`] offsets, or after
    /// the first page when `single_page`.
    fn add_page(&mut self, offset: usize, page: AlbumPage, single_page: bool) -> Vec<Album> {
        let served = page.served.min(ALBUM_PAGE_SIZE);
        let fresh: Vec<Album> = page
            .albums
            .into_iter()
            .filter(|album| self.seen.insert(album.id.clone()))
            .collect();
        self.loaded = offset.saturating_add(served);
        self.complete = single_page
            || served < ALBUM_PAGE_SIZE
            || fresh.is_empty()
            || self.loaded >= MAX_ALBUMS;
        self.error = None;
        fresh
    }
}

/// What an enqueue job adds once its data is in the cache.
#[derive(Debug, Clone)]
enum JobTarget {
    Song(Box<Track>),
    /// The songs of an album or playlist.
    Songs(Resource),
    Artist(String),
}

/// Where an enqueue job's songs go.
#[derive(Debug, Clone)]
enum Dest {
    Queue(Placement),
    /// Added to one of the user's playlists, by an edit.
    Playlist(PlaylistDest),
}

#[derive(Debug, Clone)]
enum PlaylistDest {
    Existing { id: String, name: String },
    New { name: String },
}

#[derive(Debug, Clone)]
struct Job {
    label: String,
    target: JobTarget,
    dest: Dest,
    error: Option<String>,
}

/// The playlist picker of "Add to playlist" (`P`): "New playlist…", then
/// the user's own playlists A–Z.
#[derive(Debug, Clone)]
pub struct Picker {
    /// What is added (untrusted).
    label: String,
    target: JobTarget,
    /// Of the Playlists list.
    load: Load,
    /// The user's own playlists, A–Z, once the list is loaded.
    choices: Vec<Playlist>,
    /// 0 is "New playlist…"; `i` is `choices[i - 1]`.
    selected: usize,
    /// The id of the selected playlist (`None`: "New playlist…"), so a
    /// reload keeps the selection on it.
    selected_id: Option<String>,
}

impl Picker {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn load(&self) -> &Load {
        &self.load
    }

    pub fn choices(&self) -> &[Playlist] {
        &self.choices
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Rows, "New playlist…" included.
    pub fn len(&self) -> usize {
        self.choices.len() + 1
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}

/// What a name prompt is for.
#[derive(Debug, Clone)]
enum PromptKind {
    /// A new playlist, empty or with a job's songs.
    NewPlaylist {
        from: Option<(String, JobTarget)>,
    },
    /// A new playlist with the queue's Navidrome songs.
    SaveQueue {
        songs: Vec<String>,
        local: usize,
    },
    Rename {
        playlist_id: String,
        old: String,
    },
}

#[derive(Debug, Clone)]
struct Prompt {
    kind: PromptKind,
    input: LineInput,
    /// The text it was opened with (as the input holds it: possibly cut).
    prefilled: String,
}

impl Prompt {
    fn new(kind: PromptKind, text: &str) -> Self {
        let input = LineInput::with_text(text);
        Self {
            kind,
            prefilled: input.as_str().to_owned(),
            input,
        }
    }
}

/// A changed order of a playlist, not yet saved.
#[derive(Debug, Clone)]
struct Draft {
    playlist_id: String,
    /// The entries (song ids, in order) the change started from.
    base: Vec<String>,
    /// The reordered songs, shown instead of the cached list.
    songs: Content,
    /// The edit saving it, while it runs. Only that edit's answer may end
    /// the draft.
    saving: Option<EditId>,
}

/// An entry to remove from the shown playlist; see
/// [`Browser::removal`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub playlist_id: String,
    pub index: usize,
    pub song_id: String,
    /// The song's display name (untrusted).
    pub song: String,
    /// The playlist's title (untrusted).
    pub playlist: String,
    /// Entries the server hides, which the removal (a replace) drops.
    pub hidden: Hidden,
}

/// What `w` needs: see [`Browser::save_order`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOrder {
    /// The save started, or was refused; the status message says which.
    Message(String),
    /// The playlist has entries the server hides, which saving would
    /// drop: the user must confirm first.
    Confirm(Hidden),
}

/// The warning before a change that rewrites a playlist with `hidden`
/// entries; `again` names the key and what it does, e.g. `w again to
/// save`. The instruction comes first, so a narrow status line cuts only
/// the explanation.
pub fn hidden_warning(hidden: Hidden, again: &str) -> String {
    let what = "(missing files or other libraries)";
    match hidden {
        Hidden::Some(1) => {
            format!("Press {again}: this also removes 1 entry Navidrome hides {what}")
        }
        Hidden::Some(n) => format!(
            "Press {again}: this also removes {} entries Navidrome hides {what}",
            thousands(n)
        ),
        Hidden::None | Hidden::Suspect => {
            format!("Press {again}: this may also remove entries Navidrome hides {what}")
        }
    }
}

/// An edit waiting or running, with what its status message needs.
#[derive(Debug, Clone)]
struct PendingEdit {
    request: EditRequest,
    /// The playlist's name (the new name, for a create or rename).
    playlist: String,
    /// The song removed, or the old name of a renamed playlist.
    subject: String,
}

/// Progress of enqueue jobs, in job order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobOutcome {
    /// Add these tracks to the queue, in order, at `placement`.
    Added {
        label: String,
        tracks: Vec<Track>,
        placement: Placement,
    },
    /// The job is waiting for its own data from the server.
    Fetching { label: String },
    /// The job's data is ready, but it completes after an earlier job.
    Queued { label: String, after: String },
    /// The job's data could not be fetched; nothing is added.
    Failed { label: String, error: String },
    /// A playlist edit was queued, or there was nothing to add to the
    /// playlist; `message` says which.
    Editing { message: String },
}

/// What Enter on the selected row asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activated {
    Nothing,
    /// A view was opened.
    Opened,
    /// Play this song now.
    Play(Track),
}

#[derive(Debug, Default)]
pub struct Browser {
    /// The client this browser fetches through.
    generation: Generation,
    /// Navigation stack; the last view is shown. Empty until started.
    stack: Vec<View>,
    cache: HashMap<Resource, Content>,
    in_flight: HashMap<Resource, RequestId>,
    /// In-flight requests started for enqueue jobs (at most
    /// [`MAX_JOB_FETCHES`]).
    job_fetches: HashSet<RequestId>,
    /// Paging state of each album list that has been opened.
    pages: HashMap<AlbumSort, Paging>,
    /// The album order last chosen; Home → Albums opens it.
    album_sort: AlbumSort,
    /// List rows visible in the UI (0: not reported yet).
    viewport_rows: usize,
    next_id: u64,
    outbox: Vec<FetchRequest>,
    jobs: VecDeque<Job>,
    input: Option<LineInput>,
    /// The Navidrome user, to tell their own playlists from others'.
    username: String,
    picker: Option<Picker>,
    prompt: Option<Prompt>,
    draft: Option<Draft>,
    /// Edits waiting for the one running.
    edits: VecDeque<PendingEdit>,
    edit_running: Option<PendingEdit>,
    next_edit_id: u64,
}

impl Browser {
    /// A browser for the first client ([`Generation`] 0).
    pub fn new() -> Self {
        Self::default()
    }

    /// A browser for the client of `generation`.
    pub fn with_generation(generation: Generation) -> Self {
        Self {
            generation,
            ..Self::default()
        }
    }

    /// A browser for the client of `generation`, logged in as `username`
    /// (whose playlists are editable).
    pub fn for_user(generation: Generation, username: &str) -> Self {
        Self {
            generation,
            username: username.to_owned(),
            ..Self::default()
        }
    }

    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Opens the Home view the first time; later calls do nothing. Home
    /// fetches nothing.
    pub fn ensure_started(&mut self) {
        if self.stack.is_empty() {
            self.stack.push(View {
                resource: Resource::Home,
                title: HOME_TITLE.to_owned(),
                selected: 0,
                load: Load::Ready,
            });
        }
    }

    // ----- read access -------------------------------------------------

    /// From the root to the shown view.
    pub fn stack(&self) -> &[View] {
        &self.stack
    }

    pub fn current(&self) -> Option<&View> {
        self.stack.last()
    }

    /// The cached list of a ready view (a playlist with an unsaved order
    /// shows that order).
    pub fn content(&self, view: &View) -> Option<&Content> {
        match (&view.load, &view.resource) {
            (Load::Ready, Resource::Home) => Some(&HOME),
            (Load::Ready, Resource::Playlist(id)) if self.draft_for(id).is_some() => {
                self.draft_for(id).map(|d| &d.songs)
            }
            (Load::Ready, resource) => self.cache.get(resource),
            _ => None,
        }
    }

    fn draft_for(&self, playlist_id: &str) -> Option<&Draft> {
        self.draft
            .as_ref()
            .filter(|draft| draft.playlist_id == playlist_id)
    }

    pub fn current_content(&self) -> Option<&Content> {
        self.current().and_then(|v| self.content(v))
    }

    /// The search input while it is open.
    pub fn input(&self) -> Option<&str> {
        self.input.as_ref().map(LineInput::as_str)
    }

    pub fn cached(&self, resource: &Resource) -> Option<&Content> {
        self.cache.get(resource)
    }

    pub fn is_in_flight(&self, resource: &Resource) -> bool {
        self.in_flight.contains_key(resource)
    }

    /// Requests in flight on behalf of enqueue jobs.
    pub fn job_fetches_in_flight(&self) -> usize {
        self.job_fetches.len()
    }

    /// Number of enqueue jobs not yet completed.
    pub fn pending_jobs(&self) -> usize {
        self.jobs.len()
    }

    /// The album order Home → Albums opens (the last one chosen).
    pub fn album_sort(&self) -> AlbumSort {
        self.album_sort
    }

    /// The view's title as shown: a playlist cut at the entry cap says
    /// so, e.g. `Road Trip (first 10,000 of 12,345)`, and one with an
    /// unsaved order says `(modified)`.
    pub fn title(&self, view: &View) -> String {
        let modified = match &view.resource {
            Resource::Playlist(id) if self.draft_for(id).is_some() => MODIFIED,
            _ => "",
        };
        format!(
            "{}{}{modified}",
            view.title,
            self.truncation_note(&view.resource)
        )
    }

    fn truncation_note(&self, resource: &Resource) -> String {
        match self.cache.get(resource) {
            Some(Content::PlaylistSongs(playlist)) => playlist.truncation_note(),
            _ => String::new(),
        }
    }

    /// Loading state of the rest of a ready, paged view's list.
    pub fn more(&self, view: &View) -> More {
        let Resource::Albums(sort) = view.resource else {
            return More::Idle;
        };
        match self.pages.get(&sort) {
            Some(paging) if view.load == Load::Ready => match (&paging.error, paging.pending) {
                (Some(error), _) => More::Failed(error.clone()),
                (None, Some((_, offset))) if offset > 0 => More::Loading,
                _ => More::Idle,
            },
            _ => More::Idle,
        }
    }

    /// Fetches to start, in order.
    pub fn take_requests(&mut self) -> Vec<FetchRequest> {
        std::mem::take(&mut self.outbox)
    }

    // ----- navigation --------------------------------------------------

    /// The number of list rows the UI shows; paged lists load their next
    /// page once the selection is within this many rows of the end.
    pub fn set_viewport_rows(&mut self, rows: usize) {
        if rows != self.viewport_rows {
            self.viewport_rows = rows;
            self.load_more_if_needed();
        }
    }

    /// Moves the selection one selectable row up (`-1`) or down (`1`).
    pub fn move_selection(&mut self, delta: isize) {
        let Some(view) = self.current() else { return };
        let Some(content) = self.content(view) else {
            return;
        };
        let len = content.len() as isize;
        let step = delta.signum();
        let mut i = view.selected as isize;
        let target = loop {
            i += step;
            if step == 0 || i < 0 || i >= len {
                break None;
            }
            if content.row(i as usize).is_some_and(|r| r.is_selectable()) {
                break Some(i as usize);
            }
        };
        if let (Some(target), Some(view)) = (target, self.stack.last_mut()) {
            view.selected = target;
        }
        self.load_more_if_needed();
    }

    /// Enter: opens the selected Home entry, playlist, artist or album, or
    /// asks to play the selected song.
    pub fn activate(&mut self) -> Activated {
        let Some(row) = self.selected_row() else {
            return Activated::Nothing;
        };
        let (resource, title) = match row {
            Row::Header(_) => return Activated::Nothing,
            Row::Song(track) => return Activated::Play(track.clone()),
            Row::Home(HomeEntry::Albums) => {
                self.open_albums(self.album_sort);
                return Activated::Opened;
            }
            Row::Home(entry @ HomeEntry::Playlists) => {
                (Resource::Playlists, entry.label().to_owned())
            }
            Row::Home(entry @ HomeEntry::Artists) => (Resource::Artists, entry.label().to_owned()),
            Row::Playlist(p) => (Resource::Playlist(p.id.clone()), p.name.clone()),
            Row::Artist(a) => (Resource::Artist(a.id.clone()), a.name.clone()),
            Row::Album(a) => (Resource::Album(a.id.clone()), a.name.clone()),
        };
        self.open_view(resource, title);
        Activated::Opened
    }

    /// Goes back to the previous view; Home stays.
    pub fn go_up(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
            self.forget_unshown_random();
            self.load_more_if_needed();
        }
    }

    /// `o` in an album list: shows the next sort order instead (from its
    /// cache when it has one). Returns `false` (and does nothing) in any
    /// other view.
    pub fn cycle_album_sort(&mut self) -> bool {
        let Some(Resource::Albums(sort)) = self.current().map(|v| v.resource.clone()) else {
            return false;
        };
        self.stack.pop();
        self.forget_unshown_random();
        self.open_albums(sort.next());
        true
    }

    /// `r`: refetches the shown view's list and replaces its cache entry.
    /// While a fetch for it is already running, that one is awaited
    /// instead. In an album list whose next page failed, retries that
    /// page; otherwise the list starts over at its first page.
    pub fn refresh(&mut self) {
        let Some(resource) = self.current().map(|v| v.resource.clone()) else {
            return;
        };
        let id = match resource {
            Resource::Home => return,
            Resource::Albums(sort) => {
                let paging = self.pages.entry(sort).or_default();
                if paging.error.take().is_some() {
                    let offset = paging.loaded;
                    self.request_page(sort, offset);
                    return;
                }
                self.restart_albums(sort)
            }
            _ => match self.in_flight.get(&resource) {
                Some(id) => *id,
                None => {
                    self.cache.remove(&resource);
                    self.request(resource.clone())
                }
            },
        };
        // Other views of the same list (deeper in the stack) follow too.
        for view in self.stack.iter_mut().filter(|v| v.resource == resource) {
            view.load = Load::Loading(id);
        }
    }

    fn selected_row(&self) -> Option<Row<'_>> {
        let view = self.current()?;
        self.content(view)?.row(view.selected)
    }

    fn open_view(&mut self, resource: Resource, title: String) {
        let load = if self.cache.contains_key(&resource) {
            Load::Ready
        } else {
            let id = match self.in_flight.get(&resource) {
                Some(id) => *id,
                None => self.request(resource.clone()),
            };
            Load::Loading(id)
        };
        self.stack.push(View {
            resource,
            title,
            selected: 0,
            load,
        });
        self.fix_selection();
    }

    /// Opens the album list in `sort` order: from the cache if it has one
    /// (never for random), else from its first page.
    fn open_albums(&mut self, sort: AlbumSort) {
        self.album_sort = sort;
        let resource = Resource::Albums(sort);
        let load = if sort != AlbumSort::Random && self.cache.contains_key(&resource) {
            Load::Ready
        } else {
            Load::Loading(self.restart_albums(sort))
        };
        self.stack.push(View {
            resource,
            title: albums_title(sort),
            selected: 0,
            load,
        });
        self.fix_selection();
        self.load_more_if_needed();
    }

    /// Drops the album list in `sort` order and starts it over at its
    /// first page. Returns the request the list now awaits: the first page
    /// if it is already in flight; a later page in flight, whose answer is
    /// then discarded and followed by the first page (see [`Paging`]); or
    /// a new first-page request.
    fn restart_albums(&mut self, sort: AlbumSort) -> RequestId {
        self.cache.remove(&Resource::Albums(sort));
        let paging = self.pages.entry(sort).or_default();
        paging.start_over(true);
        match paging.pending {
            Some((id, _)) => id,
            None => self.request_page(sort, 0),
        }
    }

    /// The random order is never cached: once no view shows it, its list
    /// goes, and a page still in flight is discarded when it answers.
    fn forget_unshown_random(&mut self) {
        let random = Resource::Albums(AlbumSort::Random);
        if !self.stack.iter().any(|v| v.resource == random) {
            self.cache.remove(&random);
            if let Some(paging) = self.pages.get_mut(&AlbumSort::Random) {
                paging.start_over(false);
            }
        }
    }

    fn request_page(&mut self, sort: AlbumSort, offset: usize) -> RequestId {
        let id = self.request(Resource::AlbumPage { sort, offset });
        self.pages.entry(sort).or_default().pending = Some((id, offset));
        id
    }

    /// Requests the next page of the shown album list when the selection
    /// is within a screen of its end.
    fn load_more_if_needed(&mut self) {
        let Some(view) = self.current() else { return };
        let Resource::Albums(sort) = view.resource else {
            return;
        };
        let Some(len) = self.content(view).map(Content::len) else {
            return;
        };
        let lookahead = match self.viewport_rows {
            0 => DEFAULT_VIEWPORT_ROWS,
            rows => rows,
        };
        let Some(paging) = self.pages.get(&sort) else {
            return;
        };
        if paging.wants_more(view.selected, len, lookahead) {
            let offset = paging.loaded;
            self.request_page(sort, offset);
        }
    }

    /// Keeps the shown view's selection in range and off headers.
    fn fix_selection(&mut self) {
        for i in 0..self.stack.len() {
            let view = &self.stack[i];
            let Some(content) = self.content(view) else {
                continue;
            };
            let len = content.len();
            let start = view.selected.min(len.saturating_sub(1));
            let selectable = |j: usize| content.row(j).is_some_and(|r| r.is_selectable());
            let selected = (start..len)
                .find(|&j| selectable(j))
                .or_else(|| (0..start).rev().find(|&j| selectable(j)))
                .unwrap_or(0);
            self.stack[i].selected = selected;
        }
    }

    fn request(&mut self, resource: Resource) -> RequestId {
        self.next_id += 1;
        let id = RequestId(self.next_id);
        self.in_flight.insert(resource.clone(), id);
        self.outbox.push(FetchRequest {
            generation: self.generation,
            id,
            resource,
        });
        id
    }

    /// Requests `resource` for an enqueue job unless it is cached, already
    /// in flight, or the job window is full.
    fn request_for_job(&mut self, resource: Resource) {
        if self.job_fetches.len() < MAX_JOB_FETCHES
            && !self.cache.contains_key(&resource)
            && !self.in_flight.contains_key(&resource)
        {
            let id = self.request(resource);
            self.job_fetches.insert(id);
        }
    }

    // ----- search input --------------------------------------------------

    pub fn open_search(&mut self) {
        self.input = Some(LineInput::new());
    }

    /// Adds a typed char to the open input; see [`LineInput::push_str`].
    pub fn input_char(&mut self, c: char) {
        if let Some(input) = &mut self.input {
            input.push_char(c);
        }
    }

    /// Adds pasted text to the open input; see [`LineInput::push_str`].
    pub fn input_str(&mut self, text: &str) {
        if let Some(input) = &mut self.input {
            input.push_str(text);
        }
    }

    pub fn input_backspace(&mut self) {
        if let Some(input) = &mut self.input {
            input.backspace();
        }
    }

    pub fn cancel_search(&mut self) {
        self.input = None;
    }

    /// Closes the input and opens a results view for the trimmed query.
    /// A blank query only closes the input. A search submitted while
    /// search results are shown replaces that results view instead of
    /// pushing another, so going up always returns to the view before the
    /// first search.
    pub fn submit_search(&mut self) {
        let Some(input) = self.input.take() else {
            return;
        };
        let query = input.as_str().trim();
        if query.is_empty() {
            return;
        }
        if self
            .current()
            .is_some_and(|v| matches!(v.resource, Resource::Search(_)))
        {
            self.stack.pop();
        }
        self.open_view(
            Resource::Search(query.to_owned()),
            format!("Search \"{query}\""),
        );
    }

    // ----- enqueue -------------------------------------------------------

    /// `a` / `A`: adds the selected song, album or playlist (all songs in
    /// order) or artist (all albums in served order, each album's songs in
    /// order) at `placement`. Missing lists are fetched; the job completes
    /// after every earlier job, in its turn.
    pub fn enqueue_selected(&mut self, placement: Placement) -> Vec<JobOutcome> {
        match self.selected_target() {
            Some((label, target)) => self.push_job(label, target, Dest::Queue(placement)),
            None => Vec::new(),
        }
    }

    /// What `a` (or "Add to playlist") on the selected row would add.
    fn selected_target(&self) -> Option<(String, JobTarget)> {
        Some(match self.selected_row()? {
            Row::Header(_) | Row::Home(_) => return None,
            Row::Song(t) => (t.display_name(), JobTarget::Song(Box::new(t.clone()))),
            Row::Album(a) => (
                a.name.clone(),
                JobTarget::Songs(Resource::Album(a.id.clone())),
            ),
            Row::Playlist(p) => (
                p.name.clone(),
                JobTarget::Songs(Resource::Playlist(p.id.clone())),
            ),
            Row::Artist(a) => (a.name.clone(), JobTarget::Artist(a.id.clone())),
        })
    }

    /// Queues a job behind the others and starts what it needs.
    fn push_job(&mut self, label: String, target: JobTarget, dest: Dest) -> Vec<JobOutcome> {
        self.jobs.push_back(Job {
            label: label.clone(),
            target,
            dest,
            error: None,
        });
        let mut outcomes = self.pump();
        if let Some(job) = self.jobs.back() {
            let waiting = if self.missing(&job.target).is_empty() {
                // Its own data is ready: it only waits for the front job.
                let after = self.jobs.front().map(|j| j.label.clone());
                JobOutcome::Queued {
                    label,
                    after: after.unwrap_or_default(),
                }
            } else {
                JobOutcome::Fetching { label }
            };
            outcomes.push(waiting);
        }
        outcomes
    }

    /// Lists a job still needs, in order.
    fn missing(&self, target: &JobTarget) -> Vec<Resource> {
        match target {
            JobTarget::Song(_) => Vec::new(),
            JobTarget::Songs(resource) => {
                if self.cache.contains_key(resource) {
                    Vec::new()
                } else {
                    vec![resource.clone()]
                }
            }
            JobTarget::Artist(id) => match self.cache.get(&Resource::Artist(id.clone())) {
                Some(Content::Albums(albums)) => albums
                    .iter()
                    .map(|a| Resource::Album(a.id.clone()))
                    .filter(|r| !self.cache.contains_key(r))
                    .collect(),
                _ => vec![Resource::Artist(id.clone())],
            },
        }
    }

    /// The job's tracks, once everything it needs is cached.
    fn tracks(&self, target: &JobTarget) -> Option<Vec<Track>> {
        let songs = |resource: &Resource| match self.cache.get(resource) {
            Some(Content::Songs(songs)) => Some(songs.clone()),
            Some(Content::PlaylistSongs(playlist)) => Some(playlist.tracks.clone()),
            _ => None,
        };
        match target {
            JobTarget::Song(track) => Some(vec![(**track).clone()]),
            JobTarget::Songs(resource) => songs(resource),
            JobTarget::Artist(id) => match self.cache.get(&Resource::Artist(id.clone()))? {
                Content::Albums(albums) => {
                    let mut out = Vec::new();
                    for album in albums {
                        out.extend(songs(&Resource::Album(album.id.clone()))?);
                    }
                    Some(out)
                }
                _ => None,
            },
        }
    }

    /// Completes jobs from the front while their data is ready (or they
    /// failed), then requests whatever the remaining jobs still need.
    fn pump(&mut self) -> Vec<JobOutcome> {
        let mut outcomes = Vec::new();
        while let Some(job) = self.jobs.front() {
            let ready = if let Some(error) = &job.error {
                Err(error.clone())
            } else if let Some(tracks) = self.tracks(&job.target) {
                Ok(tracks)
            } else {
                break;
            };
            let Some(job) = self.jobs.pop_front() else {
                break;
            };
            match ready {
                Err(error) => outcomes.push(JobOutcome::Failed {
                    label: job.label,
                    error,
                }),
                Ok(tracks) => {
                    let note = match &job.target {
                        JobTarget::Songs(resource) => self.truncation_note(resource),
                        _ => String::new(),
                    };
                    let label = format!("{}{note}", job.label);
                    outcomes.push(match job.dest {
                        Dest::Queue(placement) => JobOutcome::Added {
                            label,
                            tracks,
                            placement,
                        },
                        Dest::Playlist(dest) => self.add_to_playlist(&label, &tracks, dest),
                    });
                }
            }
        }
        let missing: Vec<Resource> = self
            .jobs
            .iter()
            .filter(|j| j.error.is_none())
            .flat_map(|j| self.missing(&j.target))
            .collect();
        for resource in missing {
            self.request_for_job(resource);
        }
        outcomes
    }

    // ----- playlist editing ----------------------------------------------

    /// The Navidrome user this browser edits as.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// The picker of "Add to playlist", while open.
    pub fn picker(&self) -> Option<&Picker> {
        self.picker.as_ref()
    }

    /// The name prompt, while open: its title and text.
    pub fn prompt(&self) -> Option<(&'static str, &str)> {
        self.prompt.as_ref().map(|prompt| {
            let title = match prompt.kind {
                PromptKind::NewPlaylist { .. } => "New playlist",
                PromptKind::SaveQueue { .. } => "Save queue as playlist",
                PromptKind::Rename { .. } => "Rename playlist",
            };
            (title, prompt.input.as_str())
        })
    }

    /// Edits waiting or running.
    pub fn pending_edits(&self) -> usize {
        self.edits.len() + usize::from(self.edit_running.is_some())
    }

    /// `true` while the shown view is a playlist with an unsaved order:
    /// leaving or reloading it is refused ([`UNSAVED_ORDER`]).
    pub fn has_unsaved_order(&self) -> bool {
        matches!(
            self.current().map(|v| &v.resource),
            Some(Resource::Playlist(id)) if self.draft_for(id).is_some()
        )
    }

    /// The next edit to run, once the previous one has answered.
    pub fn take_edit_requests(&mut self) -> Vec<EditRequest> {
        if self.edit_running.is_some() {
            return Vec::new();
        }
        match self.edits.pop_front() {
            Some(pending) => {
                let request = pending.request.clone();
                self.edit_running = Some(pending);
                vec![request]
            }
            None => Vec::new(),
        }
    }

    fn queue_edit(&mut self, op: EditOp, playlist: String, subject: String) -> EditId {
        self.next_edit_id += 1;
        let id = EditId(self.next_edit_id);
        self.edits.push_back(PendingEdit {
            request: EditRequest {
                generation: self.generation,
                id,
                op,
            },
            playlist,
            subject,
        });
        id
    }

    /// Edits queued but not started.
    pub fn queued_edits(&self) -> usize {
        self.edits.len()
    }

    /// `true` while an edit of `playlist_id` is queued or running.
    fn edit_pending_for(&self, playlist_id: &str) -> bool {
        self.edits
            .iter()
            .chain(&self.edit_running)
            .any(|pending| match &pending.request.op {
                EditOp::Create { .. } => false,
                EditOp::Add {
                    playlist_id: id, ..
                }
                | EditOp::Rename {
                    playlist_id: id, ..
                }
                | EditOp::Delete { playlist_id: id }
                | EditOp::Remove {
                    playlist_id: id, ..
                }
                | EditOp::Replace {
                    playlist_id: id, ..
                } => id == playlist_id,
            })
    }

    /// A finished job's songs, as an edit of the destination playlist.
    fn add_to_playlist(&mut self, label: &str, tracks: &[Track], dest: PlaylistDest) -> JobOutcome {
        let mut songs: Vec<String> = tracks
            .iter()
            .filter_map(|t| t.navidrome_id().map(str::to_owned))
            .collect();
        let cut = if songs.len() > MAX_EDIT_SONGS {
            let note = format!(
                " (first {} of {})",
                thousands(MAX_EDIT_SONGS),
                thousands(songs.len())
            );
            songs.truncate(MAX_EDIT_SONGS);
            note
        } else {
            String::new()
        };
        let n = songs_text(songs.len());
        match dest {
            PlaylistDest::Existing { id, name } => {
                if songs.is_empty() {
                    return JobOutcome::Editing {
                        message: format!("Nothing to add from {label}"),
                    };
                }
                let message = format!("Adding {n} from {label}{cut} to {name}…");
                self.queue_edit(
                    EditOp::Add {
                        playlist_id: id,
                        songs,
                    },
                    name,
                    String::new(),
                );
                JobOutcome::Editing { message }
            }
            PlaylistDest::New { name } => {
                let message = format!("Creating {name} with {n} from {label}{cut}…");
                self.queue_edit(
                    EditOp::Create {
                        name: name.clone(),
                        songs,
                    },
                    name,
                    String::new(),
                );
                JobOutcome::Editing { message }
            }
        }
    }

    /// `P`: opens the playlist picker for the selected song, album,
    /// playlist or artist. The Playlists list is fetched if needed.
    /// Returns `false` (and does nothing) on a row that adds nothing.
    pub fn open_picker(&mut self) -> bool {
        let Some((label, target)) = self.selected_target() else {
            return false;
        };
        let load = if self.cache.contains_key(&Resource::Playlists) {
            Load::Ready
        } else {
            Load::Loading(match self.in_flight.get(&Resource::Playlists) {
                Some(id) => *id,
                None => self.request(Resource::Playlists),
            })
        };
        self.picker = Some(Picker {
            label,
            target,
            load,
            choices: Vec::new(),
            selected: 0,
            selected_id: None,
        });
        self.fill_picker();
        true
    }

    /// Lists the user's own playlists in the picker once they are loaded,
    /// keeping the selection on the same playlist (or "New playlist…" if
    /// it is gone).
    fn fill_picker(&mut self) {
        let Some(picker) = self.picker.as_mut() else {
            return;
        };
        if picker.load != Load::Ready {
            return;
        }
        if let Some(Content::Playlists(list)) = self.cache.get(&Resource::Playlists) {
            let mut own: Vec<Playlist> = list
                .iter()
                .filter(|p| p.access.editable_by(&self.username))
                .cloned()
                .collect();
            own.sort_by_cached_key(|p| p.name.to_lowercase());
            picker.choices = own;
        }
        let found = picker
            .selected_id
            .as_ref()
            .and_then(|id| picker.choices.iter().position(|p| &p.id == id));
        match found {
            Some(i) => picker.selected = i + 1,
            None => {
                picker.selected = 0;
                picker.selected_id = None;
            }
        }
    }

    pub fn picker_move(&mut self, delta: isize) {
        if let Some(picker) = self.picker.as_mut() {
            picker.selected = picker
                .selected
                .saturating_add_signed(delta)
                .min(picker.choices.len());
            picker.selected_id = picker
                .selected
                .checked_sub(1)
                .and_then(|i| picker.choices.get(i))
                .map(|p| p.id.clone());
        }
    }

    pub fn close_picker(&mut self) {
        self.picker = None;
    }

    /// Enter in the picker: "New playlist…" asks for a name; a playlist
    /// gets the songs (fetched first if needed, in press order with the
    /// queue jobs).
    pub fn picker_choose(&mut self) -> Vec<JobOutcome> {
        let Some(picker) = self.picker.take() else {
            return Vec::new();
        };
        if picker.selected == 0 {
            self.prompt = Some(Prompt::new(
                PromptKind::NewPlaylist {
                    from: Some((picker.label, picker.target)),
                },
                "",
            ));
            return Vec::new();
        }
        // The choices stay listed while the list reloads, so the selected
        // row is always one the user sees; if not, the picker stays open.
        let Some(playlist) = picker.choices.get(picker.selected - 1) else {
            self.picker = Some(picker);
            return Vec::new();
        };
        let dest = PlaylistDest::Existing {
            id: playlist.id.clone(),
            name: playlist.name.clone(),
        };
        self.push_job(picker.label, picker.target, Dest::Playlist(dest))
    }

    /// `N` in the Playlists list: asks for the name of a new, empty
    /// playlist.
    pub fn open_new_playlist(&mut self) {
        self.prompt = Some(Prompt::new(PromptKind::NewPlaylist { from: None }, ""));
    }

    /// `S` in the queue: asks for the name of a new playlist holding
    /// `songs` (the queue's Navidrome songs; `local` tracks were left
    /// out), prefilled with `default_name`.
    pub fn open_save_queue(&mut self, songs: Vec<String>, local: usize, default_name: &str) {
        self.prompt = Some(Prompt::new(
            PromptKind::SaveQueue { songs, local },
            default_name,
        ));
    }

    /// The playlist selected in the Playlists list, if the user may edit
    /// it.
    fn selected_own_playlist(&self) -> Result<Playlist, &'static str> {
        match self.current() {
            Some(view) if view.resource == Resource::Playlists => {}
            _ => return Err(""),
        }
        match self.selected_row() {
            Some(Row::Playlist(p)) if p.access.editable_by(&self.username) => Ok(p.clone()),
            Some(Row::Playlist(p)) => Err(read_only_reason(&p.access, &self.username)),
            _ => Err(""),
        }
    }

    /// `e` in the Playlists list: asks for a new name, prefilled. `Err`
    /// with the reason (empty when not in the list) if it cannot be
    /// renamed.
    pub fn open_rename(&mut self) -> Result<(), &'static str> {
        let playlist = self.selected_own_playlist()?;
        self.prompt = Some(Prompt::new(
            PromptKind::Rename {
                playlist_id: playlist.id,
                old: playlist.name.clone(),
            },
            &playlist.name,
        ));
        Ok(())
    }

    /// The playlist `D` would delete: `(id, name)`. `Err` with the reason
    /// (empty when not in the Playlists list).
    pub fn deletion(&self) -> Result<(String, String), &'static str> {
        self.selected_own_playlist().map(|p| (p.id, p.name))
    }

    /// Deletes the playlist `id` (named `name`). Returns `true` once the
    /// edit is queued.
    pub fn delete_playlist(&mut self, id: String, name: String) -> bool {
        if self.draft_for(&id).is_some() {
            self.draft = None;
        }
        self.queue_edit(EditOp::Delete { playlist_id: id }, name, String::new());
        true
    }

    pub fn prompt_char(&mut self, c: char) {
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.input.push_char(c);
        }
    }

    pub fn prompt_str(&mut self, text: &str) {
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.input.push_str(text);
        }
    }

    pub fn prompt_backspace(&mut self) {
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.input.backspace();
        }
    }

    pub fn cancel_prompt(&mut self) {
        self.prompt = None;
    }

    /// Enter in the name prompt. A blank name cancels.
    pub fn submit_prompt(&mut self) -> Vec<JobOutcome> {
        let Some(prompt) = self.prompt.take() else {
            return Vec::new();
        };
        let name = prompt.input.as_str().trim().to_owned();
        if name.is_empty() {
            return Vec::new();
        }
        let untouched = prompt.input.as_str() == prompt.prefilled;
        let message = match prompt.kind {
            PromptKind::NewPlaylist {
                from: Some((label, target)),
            } => {
                return self.push_job(label, target, Dest::Playlist(PlaylistDest::New { name }));
            }
            PromptKind::NewPlaylist { from: None } => {
                let message = format!("Creating {name}…");
                self.queue_edit(
                    EditOp::Create {
                        name: name.clone(),
                        songs: Vec::new(),
                    },
                    name,
                    String::new(),
                );
                message
            }
            PromptKind::SaveQueue { songs, local } => {
                let left_out = match local {
                    0 => String::new(),
                    1 => " (1 local track left out)".to_owned(),
                    n => format!(" ({} local tracks left out)", thousands(n)),
                };
                let message = format!("Saving {} as {name}{left_out}…", songs_text(songs.len()));
                self.queue_edit(
                    EditOp::Create {
                        name: name.clone(),
                        songs,
                    },
                    name,
                    String::new(),
                );
                message
            }
            PromptKind::Rename { playlist_id, old } => {
                // An untouched prefill is unchanged, even if it had to be
                // cut to fit the input.
                if untouched || name == old {
                    "Name unchanged".to_owned()
                } else {
                    let message = format!("Renaming {old} to {name}…");
                    self.queue_edit(
                        EditOp::Rename {
                            playlist_id,
                            name: name.clone(),
                        },
                        name,
                        old,
                    );
                    message
                }
            }
        };
        vec![JobOutcome::Editing { message }]
    }

    /// The shown playlist's songs and id, if a ready playlist view is
    /// shown and the user may change its entries.
    fn editable_songs(&self) -> Result<(String, &PlaylistSongs), &'static str> {
        let view = self.current().ok_or("")?;
        let Resource::Playlist(id) = &view.resource else {
            return Err("");
        };
        let Some(Content::PlaylistSongs(songs)) = self.content(view) else {
            return Err("");
        };
        if !songs.access.editable_by(&self.username) {
            return Err(read_only_reason(&songs.access, &self.username));
        }
        if !songs.is_complete() {
            return Err(INCOMPLETE_PLAYLIST);
        }
        Ok((id.clone(), songs))
    }

    /// The entry `d` would remove: the selected song of the shown
    /// playlist. `Err` with the reason (empty when not in a playlist).
    pub fn removal(&self) -> Result<Removal, &'static str> {
        let (playlist_id, songs) = self.editable_songs()?;
        if self.draft.is_some() {
            return Err(UNSAVED_ORDER);
        }
        if self.edit_pending_for(&playlist_id) {
            return Err(WAITING_FOR_EDIT);
        }
        if songs.tracks.len() == 1 {
            return Err(LAST_SONG);
        }
        let view = self.current().ok_or("")?;
        let track = songs.tracks.get(view.selected).ok_or("")?;
        Ok(Removal {
            playlist_id,
            index: view.selected,
            song_id: track.navidrome_id().unwrap_or_default().to_owned(),
            song: track.display_name(),
            playlist: view.title.clone(),
            hidden: songs.hidden_entries(),
        })
    }

    /// Removes `removal`'s entry, if the playlist on the server still
    /// holds what is shown. Returns `true` once the edit is queued.
    pub fn remove_entry(&mut self, removal: Removal) -> bool {
        let expected = match self
            .cache
            .get(&Resource::Playlist(removal.playlist_id.clone()))
        {
            Some(Content::PlaylistSongs(songs)) => songs.song_ids(),
            _ => return false,
        };
        // The list behind the confirmation; never remove another entry.
        if expected.get(removal.index) != Some(&removal.song_id) || expected.len() < 2 {
            return false;
        }
        self.queue_edit(
            EditOp::Remove {
                playlist_id: removal.playlist_id,
                index: removal.index,
                expected,
                hidden: removal.hidden.bound(),
            },
            removal.playlist,
            removal.song,
        );
        true
    }

    /// `J`/`K` in a playlist: moves the selected song down (`1`) or up
    /// (`-1`) in a draft order; the selection follows it. `Err` with the
    /// reason (empty when not in a playlist).
    pub fn move_entry(&mut self, delta: isize) -> Result<(), &'static str> {
        let (playlist_id, songs) = self.editable_songs()?;
        let Some(view) = self.current() else {
            return Err("");
        };
        let from = view.selected;
        let to = match from.checked_add_signed(delta) {
            Some(to) if to < songs.tracks.len() => to,
            _ => return Ok(()),
        };
        match &self.draft {
            Some(draft) if draft.playlist_id != playlist_id => return Err(UNSAVED_ORDER),
            Some(draft) if draft.saving.is_some() => return Err(SAVING_ORDER),
            Some(_) => {}
            None if self.edit_pending_for(&playlist_id) => return Err(WAITING_FOR_EDIT),
            None => {
                self.draft = Some(Draft {
                    base: songs.song_ids(),
                    songs: Content::PlaylistSongs(songs.clone()),
                    playlist_id: playlist_id.clone(),
                    saving: None,
                });
            }
        }
        if let Some(Draft {
            songs: Content::PlaylistSongs(songs),
            base,
            ..
        }) = self.draft.as_mut()
        {
            songs.tracks.swap(from, to);
            // Moved back to where it started: nothing to save.
            if songs.song_ids() == *base {
                self.draft = None;
            }
        }
        if let Some(view) = self.stack.last_mut() {
            view.selected = to;
        }
        Ok(())
    }

    /// `w` in a playlist with an unsaved order: saves it (one replace,
    /// after checking the playlist did not change on the server).
    /// With hidden entries (see [`Hidden`]), the save needs `confirmed`:
    /// without it, [`SaveOrder::Confirm`] asks for it and nothing is sent.
    pub fn save_order(&mut self, confirmed: bool) -> Option<SaveOrder> {
        if !self.has_unsaved_order() {
            return None;
        }
        let title = self.current().map(|v| v.title.clone()).unwrap_or_default();
        let draft = self.draft.as_ref()?;
        if draft.saving.is_some() {
            return Some(SaveOrder::Message(SAVING_ORDER.to_owned()));
        }
        if self.edit_pending_for(&draft.playlist_id) {
            return Some(SaveOrder::Message(WAITING_FOR_EDIT.to_owned()));
        }
        let Content::PlaylistSongs(songs) = &draft.songs else {
            return None;
        };
        let hidden = songs.hidden_entries();
        if hidden != Hidden::None && !confirmed {
            return Some(SaveOrder::Confirm(hidden));
        }
        let op = EditOp::Replace {
            playlist_id: draft.playlist_id.clone(),
            expected: draft.base.clone(),
            songs: songs.song_ids(),
            hidden: hidden.bound(),
        };
        let id = self.queue_edit(op, title.clone(), String::new());
        if let Some(draft) = self.draft.as_mut() {
            draft.saving = Some(id);
        }
        Some(SaveOrder::Message(format!(
            "Saving the new order of {title}…"
        )))
    }

    /// The id of the playlist whose unsaved order is shown.
    pub fn unsaved_order_playlist(&self) -> Option<&str> {
        match self.current().map(|v| &v.resource) {
            Some(Resource::Playlist(id)) if self.draft_for(id).is_some() => Some(id),
            _ => None,
        }
    }

    /// Esc in a playlist with an unsaved order: drops it, unless it is
    /// being saved (the save would land anyway). Returns the status
    /// message, or `None` without an unsaved order.
    pub fn discard_order(&mut self) -> Option<&'static str> {
        if !self.has_unsaved_order() {
            return None;
        }
        if self.draft.as_ref().is_some_and(|d| d.saving.is_some()) {
            return Some(SAVING_ORDER);
        }
        self.draft = None;
        self.fix_selection();
        Some(ORDER_DISCARDED)
    }

    /// Applies a finished edit: reloads the lists it changed and returns
    /// the status message. A response for another generation, or for an
    /// edit that is not running, is ignored.
    pub fn handle_edit_response(&mut self, response: EditResponse) -> Option<String> {
        if response.generation != self.generation {
            return None;
        }
        let pending = self
            .edit_running
            .take_if(|pending| pending.request.id == response.id)?;
        let message = edit_message(&pending, &response.result);
        let result = &response.result;
        match &pending.request.op {
            EditOp::Create { .. } => {}
            EditOp::Delete { playlist_id } => {
                if matches!(result, EditResult::Done { .. }) {
                    self.cache.remove(&Resource::Playlist(playlist_id.clone()));
                }
            }
            EditOp::Replace { playlist_id, .. } => {
                let saved_or_stale = matches!(result, EditResult::Done { .. } | EditResult::Stale);
                // Only the answer of the edit saving the draft ends it.
                if let Some(draft) = self
                    .draft
                    .as_mut()
                    .filter(|d| d.saving == Some(response.id))
                {
                    draft.saving = None;
                    if saved_or_stale {
                        self.draft = None;
                    }
                }
                if !saved_or_stale {
                    // Nothing changed: the draft stays on screen as it was.
                    self.fix_selection();
                    return Some(message);
                }
                self.reload(Resource::Playlist(playlist_id.clone()));
            }
            EditOp::Add { playlist_id, .. }
            | EditOp::Rename { playlist_id, .. }
            | EditOp::Remove { playlist_id, .. } => {
                self.reload(Resource::Playlist(playlist_id.clone()));
            }
        }
        self.reload(Resource::Playlists);
        if let EditOp::Delete { playlist_id } = &pending.request.op {
            // A view still showing the deleted playlist shows the server's
            // answer for it.
            self.reload(Resource::Playlist(playlist_id.clone()));
        }
        self.fix_selection();
        Some(message)
    }

    /// Drops `resource` from the cache; if a view (or the picker) shows
    /// it, or a request for it is in flight (whose answer may predate the
    /// edit), fetches it again with a new request that supersedes that one.
    fn reload(&mut self, resource: Resource) {
        self.cache.remove(&resource);
        let picker_waits = resource == Resource::Playlists && self.picker.is_some();
        if !picker_waits
            && !self.stack.iter().any(|v| v.resource == resource)
            && !self.in_flight.contains_key(&resource)
        {
            return;
        }
        let id = self.request(resource.clone());
        for view in self.stack.iter_mut().filter(|v| v.resource == resource) {
            view.load = Load::Loading(id);
        }
        // The picker keeps listing the old choices until the new list
        // arrives.
        if let Some(picker) = self.picker.as_mut().filter(|_| picker_waits) {
            picker.load = Load::Loading(id);
        }
    }

    // ----- responses -------------------------------------------------------

    /// Applies a fetch result: the cache always, views only if they still
    /// await `response.id`; album pages only if awaited (see [`Paging`]).
    /// Returns the enqueue jobs that completed. A response for another
    /// client generation is ignored.
    pub fn handle_response(&mut self, response: FetchResponse) -> Vec<JobOutcome> {
        let FetchResponse {
            generation,
            id,
            resource,
            result,
        } = response;
        if generation != self.generation {
            return Vec::new();
        }
        // A newer request for the same list was sent after an edit: this
        // answer may predate the edit, so it is neither cached nor shown.
        let superseded = matches!(self.in_flight.get(&resource), Some(newer) if *newer != id);
        if self.in_flight.get(&resource) == Some(&id) {
            self.in_flight.remove(&resource);
        }
        self.job_fetches.remove(&id);
        if superseded {
            tracing::debug!(id = id.0, "superseded response ignored");
            return self.pump();
        }
        if let Resource::AlbumPage { sort, offset } = resource {
            self.apply_page(id, sort, offset, result);
            self.fix_selection();
            self.load_more_if_needed();
            return self.pump();
        }
        let load = match result {
            Ok(content) => {
                self.cache.insert(resource.clone(), content);
                Load::Ready
            }
            Err(error) => {
                let text = error.to_string();
                if !self.in_flight.contains_key(&resource) {
                    let jobs = std::mem::take(&mut self.jobs);
                    self.jobs = jobs
                        .into_iter()
                        .map(|mut job| {
                            if job.error.is_none() && self.missing(&job.target).contains(&resource)
                            {
                                job.error = Some(text.clone());
                            }
                            job
                        })
                        .collect();
                }
                Load::Failed(text)
            }
        };
        for view in &mut self.stack {
            let failed_same_list = load == Load::Ready
                && view.resource == resource
                && matches!(view.load, Load::Failed(_));
            if view.load == Load::Loading(id) || failed_same_list {
                view.load = load.clone();
            }
        }
        if let Some(picker) = self.picker.as_mut()
            && picker.load == Load::Loading(id)
        {
            picker.load = load;
            self.fill_picker();
        }
        self.fix_selection();
        self.pump()
    }

    /// Applies an album page response if it is the page `sort`'s list
    /// awaits; ignores it otherwise.
    fn apply_page(
        &mut self,
        id: RequestId,
        sort: AlbumSort,
        offset: usize,
        result: Result<Content, Error>,
    ) {
        let Some(paging) = self
            .pages
            .get_mut(&sort)
            .filter(|p| p.pending == Some((id, offset)))
        else {
            tracing::debug!(id = id.0, offset, "stale album page ignored");
            return;
        };
        paging.pending = None;
        let resource = Resource::Albums(sort);
        if std::mem::take(&mut paging.discard) {
            // The list started over meanwhile: ask for its first page now,
            // for whichever views await it.
            tracing::debug!(id = id.0, offset, "album page of an old list discarded");
            if self.stack.iter().any(|v| v.load == Load::Loading(id)) {
                let first = self.request_page(sort, 0);
                for view in &mut self.stack {
                    if view.load == Load::Loading(id) {
                        view.load = Load::Loading(first);
                    }
                }
            }
            return;
        }
        let result = match result {
            Ok(Content::AlbumPage(page)) => Ok(page),
            Ok(_) => Err("unexpected response from server".to_owned()),
            Err(error) => Err(error.to_string()),
        };
        let load = match result {
            Ok(page) => {
                let albums = paging.add_page(offset, page, sort == AlbumSort::Random);
                // The first page starts the list (a restart dropped it).
                if offset == 0 || !matches!(self.cache.get(&resource), Some(Content::Albums(_))) {
                    self.cache
                        .insert(resource.clone(), Content::Albums(Vec::new()));
                }
                if let Some(Content::Albums(list)) = self.cache.get_mut(&resource) {
                    let room = MAX_ALBUMS.saturating_sub(list.len());
                    list.extend(albums.into_iter().take(room));
                }
                Load::Ready
            }
            // A later page: the loaded rows stay, with the error below.
            Err(text) if offset > 0 => {
                paging.error = Some(text);
                return;
            }
            Err(text) => Load::Failed(text),
        };
        for view in &mut self.stack {
            if view.resource == resource && view.load == Load::Loading(id) {
                view.load = load.clone();
            }
        }
    }
}

/// The status message for a finished edit.
fn edit_message(pending: &PendingEdit, result: &EditResult) -> String {
    let playlist = &pending.playlist;
    let subject = &pending.subject;
    let op = &pending.request.op;
    let error = match result {
        EditResult::Stale => return STALE_PLAYLIST.to_owned(),
        EditResult::Done { .. } => {
            return match op {
                EditOp::Create { songs, .. } if songs.is_empty() => format!("Created {playlist}"),
                EditOp::Create { songs, .. } => {
                    format!("Created {playlist} with {}", songs_text(songs.len()))
                }
                EditOp::Add { songs, .. } => {
                    format!("Added {} to {playlist}", songs_text(songs.len()))
                }
                EditOp::Rename { .. } => format!("Renamed {subject} to {playlist}"),
                EditOp::Delete { .. } => format!("Deleted {playlist}"),
                EditOp::Remove { .. } => format!("Removed {subject} from {playlist}"),
                EditOp::Replace { .. } => format!("Saved the new order of {playlist}"),
            };
        }
        EditResult::Failed(error) | EditResult::Partial { error, .. } => error,
    };
    match (op, result) {
        (EditOp::Create { songs, .. }, EditResult::Partial { added, .. }) => format!(
            "Created {playlist} with {} of {} songs; could not add the rest: {error}",
            thousands(*added),
            thousands(songs.len())
        ),
        (EditOp::Add { songs, .. }, EditResult::Partial { added, .. }) => format!(
            "Added {} of {} songs to {playlist}; could not add the rest: {error}",
            thousands(*added),
            thousands(songs.len())
        ),
        (EditOp::Create { .. }, _) => format!("Could not create {playlist}: {error}"),
        (EditOp::Add { .. }, _) => format!("Could not add to {playlist}: {error}"),
        (EditOp::Rename { .. }, _) => format!("Could not rename {subject}: {error}"),
        (EditOp::Delete { .. }, _) => format!("Could not delete {playlist}: {error}"),
        (EditOp::Remove { .. }, EditResult::Failed(Error::RequestTooLarge)) => {
            format!("Could not remove {subject}: too many songs to change on this server")
        }
        (EditOp::Replace { .. }, EditResult::Failed(Error::RequestTooLarge)) => format!(
            "Could not save the order of {playlist}: too many songs to reorder on this server; Esc discards"
        ),
        (EditOp::Remove { .. }, _) => format!("Could not remove {subject}: {error}"),
        (EditOp::Replace { .. }, _) => {
            format!("Could not save the order of {playlist}: {error}; w retries, Esc discards")
        }
    }
}

/// Runs one fetch.
pub async fn fetch(client: &Client, resource: &Resource) -> Result<Content, Error> {
    match resource {
        Resource::Artists => client.artists().await.map(Content::Artists),
        Resource::Artist(id) => client.artist(id).await.map(Content::Albums),
        Resource::Album(id) => client.album(id).await.map(Content::Songs),
        Resource::Search(query) => client.search(query).await.map(Content::Search),
        Resource::Playlists => client.playlists().await.map(Content::Playlists),
        Resource::Playlist(id) => client.playlist(id).await.map(Content::PlaylistSongs),
        Resource::AlbumPage { sort, offset } => client
            .album_list(*sort, *offset, ALBUM_PAGE_SIZE)
            .await
            .map(Content::AlbumPage),
        // Never requested: Home is static, album lists load by page.
        Resource::Home | Resource::Albums(_) => {
            Err(Error::Decode("not a fetchable resource".to_owned()))
        }
    }
}

/// Runs `request` on a Tokio task, at most `limit` at a time, and sends
/// the response to `responses`. Never blocks the caller.
pub fn spawn_fetch(
    client: Client,
    limit: Arc<Semaphore>,
    request: FetchRequest,
    responses: mpsc::UnboundedSender<FetchResponse>,
) -> tokio::task::JoinHandle<()> {
    // Built before spawning, so even a task dropped before its first poll
    // answers.
    let reply = Reply {
        responses,
        generation: request.generation,
        id: request.id,
        resource: Some(request.resource),
    };
    tokio::spawn(async move {
        // The semaphore is never closed; if it were, run unbounded.
        let _permit = limit.acquire().await.ok();
        let Some(resource) = reply.resource.as_ref() else {
            return;
        };
        let result = fetch(&client, resource).await;
        if let Err(error) = &result {
            // Only the detail: `Display` can carry untrusted server text,
            // and `Debug` of network errors includes the server address.
            tracing::warn!(
                endpoint = resource.endpoint(),
                detail = %error.detail(),
                "navidrome fetch failed"
            );
        }
        reply.send(result);
    })
}

/// Sends a fetch task's response exactly once. Dropped without sending
/// (the task panicked or was cancelled), it sends [`Error::Aborted`], so
/// the browser never awaits a request forever.
struct Reply {
    responses: mpsc::UnboundedSender<FetchResponse>,
    generation: Generation,
    id: RequestId,
    /// `None` once sent.
    resource: Option<Resource>,
}

impl Reply {
    fn send(mut self, result: Result<Content, Error>) {
        self.send_once(result);
    }

    fn send_once(&mut self, result: Result<Content, Error>) {
        if let Some(resource) = self.resource.take() {
            let _ = self.responses.send(FetchResponse {
                generation: self.generation,
                id: self.id,
                resource,
                result,
            });
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if self.resource.is_none() {
            return;
        }
        if self.responses.is_closed() {
            // The app is quitting; nobody awaits this request any more.
            tracing::debug!(id = self.id.0, "navidrome fetch dropped at exit");
            return;
        }
        tracing::warn!(id = self.id.0, "navidrome fetch ended without a result");
        self.send_once(Err(Error::Aborted));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackMetadata;

    fn artist(id: &str) -> Artist {
        Artist {
            id: id.into(),
            name: format!("Artist {id}"),
            album_count: None,
        }
    }

    fn album(id: &str) -> Album {
        Album {
            id: id.into(),
            name: format!("Album {id}"),
            artist: None,
            year: None,
            song_count: None,
            duration: None,
        }
    }

    fn song(id: &str) -> Track {
        Track::navidrome(
            id.into(),
            TrackMetadata {
                title: Some(id.into()),
                ..TrackMetadata::default()
            },
        )
    }

    /// Goes back to Home and opens `entry`.
    fn open_home(b: &mut Browser, entry: HomeEntry) {
        b.ensure_started();
        while b.stack().len() > 1 {
            b.go_up();
        }
        for _ in 0..HOME_ENTRIES.len() {
            b.move_selection(-1);
        }
        let pos = HOME_ENTRIES.iter().position(|e| *e == entry).unwrap();
        for _ in 0..pos {
            b.move_selection(1);
        }
        assert_eq!(b.activate(), Activated::Opened);
    }

    fn ok(browser: &mut Browser, request: &FetchRequest, content: Content) -> Vec<JobOutcome> {
        browser.handle_response(FetchResponse {
            generation: request.generation,
            id: request.id,
            resource: request.resource.clone(),
            result: Ok(content),
        })
    }

    fn titles(outcomes: &[JobOutcome]) -> Vec<String> {
        outcomes
            .iter()
            .flat_map(|o| match o {
                JobOutcome::Added { tracks, .. } => {
                    tracks.iter().map(|t| t.title.clone()).collect()
                }
                _ => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn responses_for_another_generation_are_ignored() {
        let mut b = Browser::with_generation(Generation(2));
        open_home(&mut b, HomeEntry::Artists);
        let request = b.take_requests().pop().unwrap();
        assert_eq!(request.generation, Generation(2));
        let stale = FetchResponse {
            generation: Generation(1),
            id: request.id,
            resource: Resource::Artists,
            result: Ok(Content::Artists(vec![artist("old")])),
        };
        assert!(b.handle_response(stale).is_empty());
        assert_eq!(b.current().unwrap().load, Load::Loading(request.id));
        assert!(b.cached(&Resource::Artists).is_none());
        assert!(b.is_in_flight(&Resource::Artists));
        ok(&mut b, &request, Content::Artists(vec![artist("new")]));
        assert_eq!(b.current().unwrap().load, Load::Ready);
    }

    #[test]
    fn search_rows_have_headers_for_non_empty_sections() {
        let content = Content::Search(SearchResults {
            artists: vec![artist("a")],
            albums: Vec::new(),
            tracks: vec![song("s1"), song("s2")],
        });
        assert_eq!(content.len(), 5);
        assert_eq!(content.row(0), Some(Row::Header(Section::Artists)));
        assert_eq!(content.row(2), Some(Row::Header(Section::Songs)));
        assert_eq!(content.rows(3, 10).len(), 2);
        assert_eq!(content.row(5), None);
        assert!(Content::Search(SearchResults::default()).is_empty());
        assert!(Content::Artists(vec![artist("a")]).rows(5, 3).is_empty());
    }

    #[test]
    fn requests_are_deduplicated_per_resource() {
        let mut b = Browser::new();
        open_home(&mut b, HomeEntry::Artists);
        open_home(&mut b, HomeEntry::Artists);
        let requests = b.take_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].resource, Resource::Artists);
        ok(&mut b, &requests[0], Content::Artists(vec![artist("1")]));
        // Open, go up, reopen while still loading: one request.
        b.activate();
        b.go_up();
        b.activate();
        let requests = b.take_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(b.current().unwrap().load, Load::Loading(requests[0].id));
    }

    #[test]
    fn artist_jobs_collect_albums_in_served_order_including_empty_ones() {
        let mut b = Browser::new();
        open_home(&mut b, HomeEntry::Artists);
        let root = b.take_requests().remove(0);
        ok(
            &mut b,
            &root,
            Content::Artists(vec![artist("1"), artist("2")]),
        );
        let waiting = b.enqueue_selected(Placement::End);
        assert_eq!(
            waiting,
            [JobOutcome::Fetching {
                label: "Artist 1".into()
            }]
        );
        let albums = b.take_requests().remove(0);
        let outcomes = ok(
            &mut b,
            &albums,
            Content::Albums(vec![album("x"), album("empty"), album("y")]),
        );
        assert!(outcomes.is_empty());
        // Within the job window: x and empty first, y once one completes.
        let mut requests = b.take_requests();
        assert_eq!(requests.len(), MAX_JOB_FETCHES);
        let x = requests.remove(0);
        let empty = requests.remove(0);
        assert!(ok(&mut b, &empty, Content::Songs(Vec::new())).is_empty());
        let y = b.take_requests().remove(0);
        assert_eq!(y.resource, Resource::Album("y".into()));
        // y answers before x; only x's answer completes the job.
        assert!(ok(&mut b, &y, Content::Songs(vec![song("y1")])).is_empty());
        let outcomes = ok(&mut b, &x, Content::Songs(vec![song("x1"), song("x2")]));
        assert_eq!(titles(&outcomes), ["x1", "x2", "y1"]);
        assert_eq!(b.pending_jobs(), 0);
        // An artist without albums completes with nothing.
        b.move_selection(1);
        b.enqueue_selected(Placement::End);
        let none = b.take_requests().remove(0);
        let outcomes = ok(&mut b, &none, Content::Albums(Vec::new()));
        assert_eq!(
            outcomes,
            [JobOutcome::Added {
                label: "Artist 2".into(),
                tracks: Vec::new(),
                placement: Placement::End,
            }]
        );
    }

    #[test]
    fn selection_ignores_headers_and_moves_only_within_ready_lists() {
        let mut b = Browser::new();
        open_home(&mut b, HomeEntry::Artists);
        // Loading: moving does nothing and nothing is selected.
        b.move_selection(1);
        assert_eq!(b.activate(), Activated::Nothing);
        assert!(b.enqueue_selected(Placement::End).is_empty());
        b.open_search();
        b.input_char('q');
        b.submit_search();
        let search = b.take_requests().pop().unwrap();
        ok(
            &mut b,
            &search,
            Content::Search(SearchResults {
                artists: Vec::new(),
                albums: vec![album("a")],
                tracks: vec![song("s")],
            }),
        );
        assert_eq!(b.current().unwrap().selected, 1);
        b.move_selection(1);
        assert_eq!(b.current().unwrap().selected, 3);
        b.move_selection(1);
        assert_eq!(b.current().unwrap().selected, 3);
        b.move_selection(-1);
        b.move_selection(-1);
        assert_eq!(b.current().unwrap().selected, 1);
    }

    #[test]
    fn job_fetches_leave_capacity_for_views() {
        let mut b = Browser::new();
        open_home(&mut b, HomeEntry::Artists);
        let root = b.take_requests().remove(0);
        ok(
            &mut b,
            &root,
            Content::Artists(vec![artist("big"), artist("other")]),
        );
        b.enqueue_selected(Placement::End);
        let albums = b.take_requests().remove(0);
        let many: Vec<Album> = (0..200).map(|i| album(&format!("al{i}"))).collect();
        ok(&mut b, &albums, Content::Albums(many));
        // Only a small window of the 200 album fetches is started.
        let first = b.take_requests();
        assert_eq!(first.len(), MAX_JOB_FETCHES);
        assert_eq!(b.job_fetches_in_flight(), MAX_JOB_FETCHES);
        // A view opened now gets its own request at once, with free permits.
        b.move_selection(1);
        b.activate();
        let view = b.take_requests();
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].resource, Resource::Artist("other".into()));
        assert!(b.job_fetches_in_flight() + view.len() <= MAX_CONCURRENT_FETCHES);
        // Each completed job fetch starts the next one, in album order.
        ok(&mut b, &first[0], Content::Songs(vec![song("s0")]));
        let next = b.take_requests();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].resource, Resource::Album("al2".into()));
        // The view's answer does not count against the job window.
        ok(&mut b, &view[0], Content::Albums(Vec::new()));
        assert!(b.take_requests().is_empty());
        assert_eq!(b.job_fetches_in_flight(), MAX_JOB_FETCHES);
    }

    #[test]
    fn a_later_success_heals_a_failed_view_of_the_same_list() {
        let mut b = Browser::new();
        open_home(&mut b, HomeEntry::Artists);
        let root = b.take_requests().remove(0);
        b.handle_response(FetchResponse {
            generation: Default::default(),
            id: root.id,
            resource: Resource::Artists,
            result: Err(Error::Http(500)),
        });
        assert!(matches!(b.current().unwrap().load, Load::Failed(_)));
        // Any successful answer for the list (e.g. one a job requested).
        b.handle_response(FetchResponse {
            generation: Default::default(),
            id: RequestId(99),
            resource: Resource::Artists,
            result: Ok(Content::Artists(vec![artist("1")])),
        });
        assert_eq!(b.current().unwrap().load, Load::Ready);
        // A failure for an unrelated id leaves a ready view alone.
        b.handle_response(FetchResponse {
            generation: Default::default(),
            id: RequestId(100),
            resource: Resource::Artists,
            result: Err(Error::Http(500)),
        });
        assert_eq!(b.current().unwrap().load, Load::Ready);
    }

    // ----- Home, playlists, album lists ---------------------------------

    fn playlist(id: &str) -> Playlist {
        Playlist {
            id: id.into(),
            name: format!("Playlist {id}"),
            song_count: None,
            duration: None,
            access: Default::default(),
        }
    }

    /// A page of `n` albums named from `start`, as the server sent it.
    fn page(start: usize, n: usize) -> Content {
        Content::AlbumPage(AlbumPage {
            albums: (start..start + n)
                .map(|i| album(&format!("al{i}")))
                .collect(),
            served: n,
        })
    }

    fn err(browser: &mut Browser, request: &FetchRequest, error: Error) -> Vec<JobOutcome> {
        browser.handle_response(FetchResponse {
            generation: request.generation,
            id: request.id,
            resource: request.resource.clone(),
            result: Err(error),
        })
    }

    fn one(b: &mut Browser) -> FetchRequest {
        let mut requests = b.take_requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        requests.remove(0)
    }

    fn albums_len(b: &Browser) -> usize {
        b.current_content().map_or(0, Content::len)
    }

    fn select(b: &mut Browser, index: usize) {
        while b.current().unwrap().selected < index {
            b.move_selection(1);
        }
    }

    fn page_request(sort: AlbumSort, offset: usize) -> Resource {
        Resource::AlbumPage { sort, offset }
    }

    #[test]
    fn home_is_static_and_h_returns_to_it_from_every_list() {
        let mut b = Browser::new();
        b.ensure_started();
        assert!(b.take_requests().is_empty(), "Home fetches nothing");
        let home = b.current().unwrap();
        assert_eq!(
            (home.resource.clone(), home.load.clone()),
            (Resource::Home, Load::Ready)
        );
        let rows: Vec<Row> = b.current_content().unwrap().rows(0, 10);
        assert_eq!(
            rows,
            HOME_ENTRIES.map(Row::Home).to_vec(),
            "Playlists, Artists, Albums"
        );
        // Nothing to add or refresh on Home, and it is the root.
        assert!(b.enqueue_selected(Placement::End).is_empty());
        b.refresh();
        b.go_up();
        assert!(b.take_requests().is_empty());
        assert_eq!(b.stack().len(), 1);
        for (entry, resource, title) in [
            (HomeEntry::Playlists, Resource::Playlists, "Playlists"),
            (HomeEntry::Artists, Resource::Artists, "Artists"),
            (
                HomeEntry::Albums,
                page_request(AlbumSort::Name, 0),
                "Albums (A–Z)",
            ),
        ] {
            open_home(&mut b, entry);
            assert_eq!(one(&mut b).resource, resource);
            assert_eq!(b.current().unwrap().title, title);
            // Search from the list; `h` goes back to it, then to Home.
            b.open_search();
            b.input_char('x');
            b.submit_search();
            b.take_requests();
            b.go_up();
            assert_eq!(b.current().unwrap().title, title);
            b.go_up();
            assert_eq!(b.current().unwrap().resource, Resource::Home);
        }
    }

    #[test]
    fn playlists_open_and_enqueue_in_order_with_duplicates() {
        let mut b = Browser::new();
        open_home(&mut b, HomeEntry::Playlists);
        let lists = one(&mut b);
        ok(
            &mut b,
            &lists,
            Content::Playlists(vec![playlist("p1"), playlist("p2")]),
        );
        // `A` on a playlist row: fetched, then added in playlist order.
        let waiting = b.enqueue_selected(Placement::Next);
        assert_eq!(
            waiting,
            [JobOutcome::Fetching {
                label: "Playlist p1".into()
            }]
        );
        let p1 = one(&mut b);
        assert_eq!(p1.resource, Resource::Playlist("p1".into()));
        let outcomes = ok(
            &mut b,
            &p1,
            Content::Songs(vec![song("s1"), song("s2"), song("s1")]),
        );
        assert_eq!(titles(&outcomes), ["s1", "s2", "s1"]);
        assert!(matches!(
            outcomes[0],
            JobOutcome::Added {
                placement: Placement::Next,
                ..
            }
        ));
        // Enter opens it from the cache; Enter on a song plays it.
        assert_eq!(b.activate(), Activated::Opened);
        assert!(b.take_requests().is_empty());
        assert_eq!(b.current().unwrap().title, "Playlist p1");
        b.move_selection(1);
        b.move_selection(1);
        assert_eq!(b.activate(), Activated::Play(song("s1")));
        // `a` on a song of the playlist appends it.
        let outcomes = b.enqueue_selected(Placement::End);
        assert_eq!(titles(&outcomes), ["s1"]);
        // The second playlist is fetched when opened.
        b.go_up();
        b.move_selection(1);
        b.activate();
        assert_eq!(one(&mut b).resource, Resource::Playlist("p2".into()));
    }

    #[test]
    fn album_sort_cycles_and_each_order_is_cached_except_random() {
        let mut b = Browser::new();
        open_home(&mut b, HomeEntry::Playlists);
        b.take_requests();
        assert!(!b.cycle_album_sort(), "only in an album list");
        b.go_up();
        open_home(&mut b, HomeEntry::Albums);
        let first = one(&mut b);
        ok(&mut b, &first, page(0, 3));
        let mut seen = Vec::new();
        for _ in 0..AlbumSort::ALL.len() {
            assert!(b.cycle_album_sort());
            let sort = b.album_sort();
            seen.push(sort);
            assert_eq!(b.current().unwrap().title, albums_title(sort));
            assert_eq!(b.stack().len(), 2, "the view is replaced");
            let request = b.take_requests();
            if sort == AlbumSort::Name {
                assert!(request.is_empty(), "cached");
                assert_eq!(b.current().unwrap().load, Load::Ready);
            } else {
                assert_eq!(request[0].resource, page_request(sort, 0));
                ok(&mut b, &request[0], page(0, 2));
            }
        }
        let mut expected = AlbumSort::ALL.to_vec();
        expected.rotate_left(1);
        assert_eq!(seen, expected);
        // Every order but random is cached; random comes back reshuffled.
        for sort in AlbumSort::ALL {
            let cached = b.cached(&Resource::Albums(sort)).is_some();
            assert_eq!(cached, sort != AlbumSort::Random, "{sort:?}");
        }
        // Home → Albums opens the last order chosen.
        b.go_up();
        open_home(&mut b, HomeEntry::Albums);
        assert_eq!(b.current().unwrap().title, "Albums (A–Z)");
        assert!(b.take_requests().is_empty());
    }

    #[test]
    fn the_next_page_loads_within_a_screen_of_the_end_until_a_short_page() {
        let mut b = Browser::new();
        b.set_viewport_rows(10);
        open_home(&mut b, HomeEntry::Albums);
        let first = one(&mut b);
        ok(&mut b, &first, page(0, ALBUM_PAGE_SIZE));
        assert_eq!(albums_len(&b), 500);
        assert!(b.take_requests().is_empty(), "far from the end");
        assert_eq!(b.more(b.current().unwrap()), More::Idle);
        select(&mut b, 489);
        assert!(b.take_requests().is_empty());
        b.move_selection(1);
        let second = one(&mut b);
        assert_eq!(second.resource, page_request(AlbumSort::Name, 500));
        assert_eq!(b.more(b.current().unwrap()), More::Loading);
        // One page at a time.
        b.move_selection(1);
        assert!(b.take_requests().is_empty());
        ok(&mut b, &second, page(500, ALBUM_PAGE_SIZE));
        assert_eq!(albums_len(&b), 1000);
        assert_eq!(b.current().unwrap().selected, 491, "selection kept");
        assert!(b.take_requests().is_empty());
        select(&mut b, 990);
        let third = one(&mut b);
        assert_eq!(third.resource, page_request(AlbumSort::Name, 1000));
        ok(&mut b, &third, page(1000, 7));
        assert_eq!(albums_len(&b), 1007);
        // A short page ends the list.
        select(&mut b, 1006);
        assert!(b.take_requests().is_empty());
        assert_eq!(b.more(b.current().unwrap()), More::Idle);
        // A bigger screen reaches further ahead at once.
        let mut b = Browser::new();
        open_home(&mut b, HomeEntry::Albums);
        let first = one(&mut b);
        ok(&mut b, &first, page(0, ALBUM_PAGE_SIZE));
        assert!(b.take_requests().is_empty());
        b.set_viewport_rows(500);
        assert_eq!(one(&mut b).resource, page_request(AlbumSort::Name, 500));
    }

    #[test]
    fn unusable_items_count_towards_the_offset_and_the_end() {
        let mut b = Browser::new();
        b.set_viewport_rows(600);
        open_home(&mut b, HomeEntry::Albums);
        let first = one(&mut b);
        // The server sent 500 items, two without an id.
        let Content::AlbumPage(mut full) = page(0, ALBUM_PAGE_SIZE) else {
            unreachable!()
        };
        full.albums.truncate(498);
        ok(&mut b, &first, Content::AlbumPage(full));
        assert_eq!(albums_len(&b), 498);
        let next = one(&mut b);
        assert_eq!(next.resource, page_request(AlbumSort::Name, 500));
        // A page claiming more than the size advances by the size only.
        let Content::AlbumPage(mut more) = page(500, 3) else {
            unreachable!()
        };
        more.served = 9_999;
        ok(&mut b, &next, Content::AlbumPage(more));
        let third = one(&mut b);
        assert_eq!(third.resource, page_request(AlbumSort::Name, 1000));
        // A full page of only unusable items ends the list.
        ok(
            &mut b,
            &third,
            Content::AlbumPage(AlbumPage {
                albums: Vec::new(),
                served: ALBUM_PAGE_SIZE,
            }),
        );
        assert!(b.take_requests().is_empty());
        assert_eq!(albums_len(&b), 501);
        assert_eq!(b.more(b.current().unwrap()), More::Idle);
    }

    #[test]
    fn albums_already_loaded_are_skipped_and_a_page_of_repeats_ends_the_list() {
        let mut b = Browser::new();
        b.set_viewport_rows(600);
        open_home(&mut b, HomeEntry::Albums);
        let first = one(&mut b);
        ok(&mut b, &first, page(0, ALBUM_PAGE_SIZE));
        // The pages overlap (e.g. the library changed meanwhile).
        let second = one(&mut b);
        ok(&mut b, &second, page(400, ALBUM_PAGE_SIZE));
        assert_eq!(albums_len(&b), 900);
        let ids: HashSet<String> = b
            .current_content()
            .unwrap()
            .rows(0, 1000)
            .iter()
            .map(|r| match r {
                Row::Album(a) => a.id.clone(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(ids.len(), 900, "no duplicates");
        // A server that ignores `offset` sends the first page again: the
        // list ends there instead of paging to the cap.
        select(&mut b, 899);
        let third = one(&mut b);
        assert_eq!(third.resource, page_request(AlbumSort::Name, 1000));
        ok(&mut b, &third, page(0, ALBUM_PAGE_SIZE));
        assert!(b.take_requests().is_empty());
        assert_eq!(albums_len(&b), 900);
        // `r` starts over with a fresh set.
        b.refresh();
        let again = one(&mut b);
        ok(&mut b, &again, page(0, 2));
        assert_eq!(albums_len(&b), 2);
    }

    #[test]
    fn stale_pages_are_ignored_and_r_restarts_the_list() {
        let mut b = Browser::new();
        b.set_viewport_rows(10);
        open_home(&mut b, HomeEntry::Albums);
        let first = one(&mut b);
        ok(&mut b, &first, page(0, ALBUM_PAGE_SIZE));
        select(&mut b, 495);
        let second = one(&mut b);
        // A response with another id or offset is not the awaited page.
        let wrong_id = FetchRequest {
            id: RequestId(999),
            ..second.clone()
        };
        ok(&mut b, &wrong_id, page(500, ALBUM_PAGE_SIZE));
        let wrong_offset = FetchRequest {
            resource: page_request(AlbumSort::Name, 1000),
            ..second.clone()
        };
        ok(&mut b, &wrong_offset, page(1000, ALBUM_PAGE_SIZE));
        let other_sort = FetchRequest {
            resource: page_request(AlbumSort::Newest, 500),
            ..second.clone()
        };
        ok(&mut b, &other_sort, page(500, ALBUM_PAGE_SIZE));
        assert_eq!(albums_len(&b), 500);
        assert!(b.cached(&Resource::Albums(AlbumSort::Newest)).is_none());
        assert_eq!(b.more(b.current().unwrap()), More::Loading);
        // A repeated first page (e.g. a duplicate answer) is stale too.
        ok(&mut b, &first, page(0, ALBUM_PAGE_SIZE));
        assert_eq!(albums_len(&b), 500);
        // `r` while page 2 is in flight: the list starts over, but only
        // once that request has answered (one request per list).
        b.refresh();
        assert!(b.take_requests().is_empty());
        assert_eq!(b.current().unwrap().load, Load::Loading(second.id));
        assert!(b.cached(&Resource::Albums(AlbumSort::Name)).is_none());
        b.refresh();
        assert!(b.take_requests().is_empty());
        // Its answer is dropped and the first page requested.
        ok(&mut b, &second, page(500, ALBUM_PAGE_SIZE));
        assert!(b.cached(&Resource::Albums(AlbumSort::Name)).is_none());
        assert!(!b.is_in_flight(&second.resource));
        let restart = one(&mut b);
        assert_eq!(restart.resource, page_request(AlbumSort::Name, 0));
        assert_eq!(b.current().unwrap().load, Load::Loading(restart.id));
        // More `r` presses await the same first page.
        b.refresh();
        assert!(b.take_requests().is_empty());
        ok(&mut b, &restart, page(0, 3));
        assert_eq!(b.current().unwrap().load, Load::Ready);
        assert_eq!(albums_len(&b), 3);
        assert_eq!(b.current().unwrap().selected, 2, "clamped to the list");
    }

    #[test]
    fn a_failed_page_shows_below_the_list_and_r_retries_it() {
        let mut b = Browser::new();
        b.set_viewport_rows(10);
        open_home(&mut b, HomeEntry::Albums);
        let first = one(&mut b);
        ok(&mut b, &first, page(0, ALBUM_PAGE_SIZE));
        select(&mut b, 495);
        let second = one(&mut b);
        err(&mut b, &second, Error::Http(502));
        let view = b.current().unwrap();
        assert_eq!(view.load, Load::Ready, "the loaded rows stay");
        assert_eq!(
            b.more(view),
            More::Failed("server returned HTTP 502".into())
        );
        // Moving does not hammer the server.
        b.move_selection(1);
        assert!(b.take_requests().is_empty());
        // `r` retries the failed page, not the whole list.
        b.refresh();
        let retry = one(&mut b);
        assert_eq!(retry.resource, page_request(AlbumSort::Name, 500));
        assert_eq!(b.more(b.current().unwrap()), More::Loading);
        ok(&mut b, &retry, page(500, 1));
        assert_eq!(albums_len(&b), 501);
        assert_eq!(b.more(b.current().unwrap()), More::Idle);
        // A failed first page fails the view; `r` starts over.
        let mut b = Browser::new();
        open_home(&mut b, HomeEntry::Albums);
        let first = one(&mut b);
        err(&mut b, &first, Error::Aborted);
        assert_eq!(
            b.current().unwrap().load,
            Load::Failed("request did not complete".into())
        );
        b.refresh();
        let again = one(&mut b);
        assert_eq!(again.resource, page_request(AlbumSort::Name, 0));
        ok(&mut b, &again, page(0, 1));
        assert_eq!(b.current().unwrap().load, Load::Ready);
    }

    #[test]
    fn random_is_never_cached_and_reshuffles_on_reopen_and_r() {
        let mut b = Browser::new();
        open_home(&mut b, HomeEntry::Albums);
        b.take_requests();
        while b.album_sort() != AlbumSort::Random {
            b.cycle_album_sort();
        }
        let random = b.take_requests().pop().unwrap();
        assert_eq!(random.resource, page_request(AlbumSort::Random, 0));
        ok(&mut b, &random, page(0, 2));
        // Into an album and back: still the same list.
        b.activate();
        b.take_requests();
        b.go_up();
        assert_eq!(albums_len(&b), 2);
        assert!(b.take_requests().is_empty());
        // `r` reshuffles.
        b.refresh();
        let reshuffle = one(&mut b);
        assert_eq!(reshuffle.resource, page_request(AlbumSort::Random, 0));
        ok(&mut b, &reshuffle, page(10, 2));
        // Leaving drops it; reopening asks again, and a late page of the
        // old shuffle is ignored.
        b.go_up();
        assert!(b.cached(&Resource::Albums(AlbumSort::Random)).is_none());
        open_home(&mut b, HomeEntry::Albums);
        let reopen = one(&mut b);
        assert_eq!(reopen.resource, page_request(AlbumSort::Random, 0));
        ok(&mut b, &reshuffle, page(20, 2));
        assert_eq!(b.current().unwrap().load, Load::Loading(reopen.id));
        ok(&mut b, &reopen, page(30, 2));
        let Some(Row::Album(first)) = b.current_content().unwrap().row(0) else {
            panic!()
        };
        assert_eq!(first.id, "al30");
        // Cycling away from random forgets it as well.
        b.cycle_album_sort();
        assert!(b.cached(&Resource::Albums(AlbumSort::Random)).is_none());
    }

    #[test]
    fn random_is_one_page_and_a_shuffle_in_flight_is_dropped_when_left() {
        let mut b = Browser::new();
        b.set_viewport_rows(10);
        open_home(&mut b, HomeEntry::Albums);
        b.take_requests();
        while b.album_sort() != AlbumSort::Random {
            b.cycle_album_sort();
        }
        let first = b.take_requests().pop().unwrap();
        // Leave before it answers: dropped, nothing requested or kept.
        b.go_up();
        ok(&mut b, &first, page(0, ALBUM_PAGE_SIZE));
        assert!(b.take_requests().is_empty());
        assert!(b.cached(&Resource::Albums(AlbumSort::Random)).is_none());
        // Reopen, leave and reopen while that shuffle is in flight: the
        // view waits for it, drops it, then asks for a fresh one.
        open_home(&mut b, HomeEntry::Albums);
        let second = one(&mut b);
        b.go_up();
        open_home(&mut b, HomeEntry::Albums);
        assert!(b.take_requests().is_empty(), "one request per list");
        assert_eq!(b.current().unwrap().load, Load::Loading(second.id));
        ok(&mut b, &second, page(0, ALBUM_PAGE_SIZE));
        let third = one(&mut b);
        assert_eq!(third.resource, page_request(AlbumSort::Random, 0));
        assert_eq!(b.current().unwrap().load, Load::Loading(third.id));
        ok(&mut b, &third, page(0, ALBUM_PAGE_SIZE));
        assert_eq!(albums_len(&b), ALBUM_PAGE_SIZE);
        // A single page: no paging at the end.
        select(&mut b, ALBUM_PAGE_SIZE - 1);
        assert!(b.take_requests().is_empty());
        assert_eq!(b.more(b.current().unwrap()), More::Idle);
    }

    #[test]
    fn album_lists_stop_at_the_cap() {
        let mut b = Browser::new();
        b.set_viewport_rows(usize::MAX);
        open_home(&mut b, HomeEntry::Albums);
        let mut pages = 0;
        while let Some(request) = b.take_requests().pop() {
            let Resource::AlbumPage { offset, .. } = request.resource else {
                panic!("{request:?}")
            };
            ok(&mut b, &request, page(offset, ALBUM_PAGE_SIZE));
            pages += 1;
            assert!(pages <= MAX_ALBUMS / ALBUM_PAGE_SIZE, "never stops");
        }
        assert_eq!(albums_len(&b), MAX_ALBUMS);
        assert_eq!(b.more(b.current().unwrap()), More::Idle);
    }

    #[test]
    fn new_resources_ignore_other_generations() {
        let mut b = Browser::with_generation(Generation(3));
        open_home(&mut b, HomeEntry::Albums);
        let request = one(&mut b);
        let stale = |resource: Resource, content: Content| FetchResponse {
            generation: Generation(2),
            id: request.id,
            resource,
            result: Ok(content),
        };
        b.handle_response(stale(request.resource.clone(), page(0, 5)));
        assert_eq!(b.current().unwrap().load, Load::Loading(request.id));
        assert!(b.is_in_flight(&request.resource));
        b.go_up();
        open_home(&mut b, HomeEntry::Playlists);
        let lists = one(&mut b);
        b.handle_response(stale(
            Resource::Playlists,
            Content::Playlists(vec![playlist("old")]),
        ));
        assert!(b.cached(&Resource::Playlists).is_none());
        assert_eq!(b.current().unwrap().load, Load::Loading(lists.id));
    }

    #[test]
    fn a_cut_playlist_says_so_in_its_title_and_when_added() {
        let mut b = Browser::new();
        open_home(&mut b, HomeEntry::Playlists);
        let lists = one(&mut b);
        ok(&mut b, &lists, Content::Playlists(vec![playlist("p1")]));
        b.enqueue_selected(Placement::End);
        let p1 = one(&mut b);
        let cut = Content::PlaylistSongs(PlaylistSongs {
            tracks: vec![song("s1"), song("s2")],
            served: 12_345,
            access: Default::default(),
            song_count: None,
        });
        let outcomes = ok(&mut b, &p1, cut);
        assert_eq!(
            outcomes,
            [JobOutcome::Added {
                label: "Playlist p1 (first 10,000 of 12,345)".into(),
                tracks: vec![song("s1"), song("s2")],
                placement: Placement::End,
            }]
        );
        b.activate();
        let view = b.current().unwrap();
        assert_eq!(b.title(view), "Playlist p1 (first 10,000 of 12,345)");
        assert_eq!(b.current_content().unwrap().len(), 2);
        // A playlist that fits has its plain name.
        let whole = Content::PlaylistSongs(PlaylistSongs {
            tracks: vec![song("s1")],
            served: 1,
            access: Default::default(),
            song_count: None,
        });
        b.refresh();
        let again = one(&mut b);
        ok(&mut b, &again, whole);
        assert_eq!(b.title(b.current().unwrap()), "Playlist p1");
    }

    #[test]
    fn a_failed_playlist_fails_its_job_in_turn() {
        let mut b = Browser::new();
        open_home(&mut b, HomeEntry::Playlists);
        let lists = one(&mut b);
        ok(&mut b, &lists, Content::Playlists(vec![playlist("p1")]));
        b.enqueue_selected(Placement::End);
        let p1 = one(&mut b);
        let outcomes = err(&mut b, &p1, Error::Http(404));
        assert_eq!(
            outcomes,
            [JobOutcome::Failed {
                label: "Playlist p1".into(),
                error: "server returned HTTP 404".into()
            }]
        );
    }
}
