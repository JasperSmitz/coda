//! Remote browse state for the Navidrome source: a navigation stack of
//! views (Artists → Albums → Songs, plus search results), a session-only
//! cache of fetched lists, the search input, and ordered background
//! enqueue jobs. No Ratatui, no I/O.
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
//! - Enqueue jobs complete strictly in the order they were created (key
//!   presses), whatever order their responses arrive in. A job whose data
//!   failed to load is dropped with an error, in its turn.
//! - Jobs fetch lazily: at most [`MAX_JOB_FETCHES`] of their requests are
//!   in flight at once, below [`MAX_CONCURRENT_FETCHES`], so `a` on an
//!   artist with hundreds of albums never queues view fetches behind it.
//! - Every spawned fetch answers exactly once: if its task is dropped or
//!   panics first, an [`Error::Aborted`] response is sent instead.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use tokio::sync::{Semaphore, mpsc};

use super::{Album, Artist, Client, Error, SearchResults};
use crate::model::Track;

/// Upper bound on the search input, in chars.
pub const MAX_QUERY_CHARS: usize = 256;
/// Upper bound on concurrently running fetches.
pub const MAX_CONCURRENT_FETCHES: usize = 4;
/// Upper bound on fetches in flight for enqueue jobs; the remaining
/// capacity is kept for views.
pub const MAX_JOB_FETCHES: usize = 2;
const _: () = assert!(MAX_JOB_FETCHES < MAX_CONCURRENT_FETCHES);
/// Title of the root view.
pub const ARTISTS_TITLE: &str = "Artists";

/// A list the server can be asked for; also the cache key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Resource {
    /// All artists (`getArtists`).
    Artists,
    /// The albums of an artist (`getArtist`).
    Artist(String),
    /// The songs of an album (`getAlbum`).
    Album(String),
    /// `search3` results for a (trimmed, non-empty) query.
    Search(String),
}

impl Resource {
    /// The Subsonic endpoint, for logs.
    pub fn endpoint(&self) -> &'static str {
        match self {
            Self::Artists => "getArtists",
            Self::Artist(_) => "getArtist",
            Self::Album(_) => "getAlbum",
            Self::Search(_) => "search3",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRequest {
    pub id: RequestId,
    pub resource: Resource,
}

#[derive(Debug)]
pub struct FetchResponse {
    pub id: RequestId,
    pub resource: Resource,
    pub result: Result<Content, Error>,
}

/// A fetched list. The variant follows from the [`Resource`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Artists(Vec<Artist>),
    Albums(Vec<Album>),
    Songs(Vec<Track>),
    Search(SearchResults),
}

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
    Artist(&'a Artist),
    Album(&'a Album),
    Song(&'a Track),
}

impl Row<'_> {
    pub fn is_selectable(&self) -> bool {
        !matches!(self, Row::Header(_))
    }
}

impl Content {
    pub fn len(&self) -> usize {
        match self {
            Self::Artists(v) => v.len(),
            Self::Albums(v) => v.len(),
            Self::Songs(v) => v.len(),
            Self::Search(r) => search_rows(r).count(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn row(&self, i: usize) -> Option<Row<'_>> {
        match self {
            Self::Artists(v) => v.get(i).map(Row::Artist),
            Self::Albums(v) => v.get(i).map(Row::Album),
            Self::Songs(v) => v.get(i).map(Row::Song),
            Self::Search(r) => search_rows(r).nth(i),
        }
    }

    /// At most `len` rows starting at `start`, without building the rest.
    pub fn rows(&self, start: usize, len: usize) -> Vec<Row<'_>> {
        match self {
            Self::Artists(v) => window(v, start, len).iter().map(Row::Artist).collect(),
            Self::Albums(v) => window(v, start, len).iter().map(Row::Album).collect(),
            Self::Songs(v) => window(v, start, len).iter().map(Row::Song).collect(),
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

/// What an enqueue job adds once its data is in the cache.
#[derive(Debug, Clone)]
enum JobTarget {
    Song(Box<Track>),
    Album(String),
    Artist(String),
}

#[derive(Debug, Clone)]
struct Job {
    label: String,
    target: JobTarget,
    error: Option<String>,
}

/// Progress of enqueue jobs, in job order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobOutcome {
    /// Append these tracks to the queue, in order.
    Added { label: String, tracks: Vec<Track> },
    /// The job is waiting for its own data from the server.
    Fetching { label: String },
    /// The job's data is ready, but it completes after an earlier job.
    Queued { label: String, after: String },
    /// The job's data could not be fetched; nothing is added.
    Failed { label: String, error: String },
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
    /// Navigation stack; the last view is shown. Empty until started.
    stack: Vec<View>,
    cache: HashMap<Resource, Content>,
    in_flight: HashMap<Resource, RequestId>,
    /// In-flight requests started for enqueue jobs (at most
    /// [`MAX_JOB_FETCHES`]).
    job_fetches: HashSet<RequestId>,
    next_id: u64,
    outbox: Vec<FetchRequest>,
    jobs: VecDeque<Job>,
    input: Option<String>,
}

impl Browser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the Artists root view the first time; later calls do nothing.
    pub fn ensure_started(&mut self) {
        if self.stack.is_empty() {
            self.open_view(Resource::Artists, ARTISTS_TITLE.to_owned());
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

    /// The cached list of a ready view.
    pub fn content(&self, view: &View) -> Option<&Content> {
        match view.load {
            Load::Ready => self.cache.get(&view.resource),
            _ => None,
        }
    }

    pub fn current_content(&self) -> Option<&Content> {
        self.current().and_then(|v| self.content(v))
    }

    /// The search input while it is open.
    pub fn input(&self) -> Option<&str> {
        self.input.as_deref()
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

    /// Fetches to start, in order.
    pub fn take_requests(&mut self) -> Vec<FetchRequest> {
        std::mem::take(&mut self.outbox)
    }

    // ----- navigation --------------------------------------------------

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
    }

    /// Enter: opens the selected artist or album, or asks to play the
    /// selected song.
    pub fn activate(&mut self) -> Activated {
        let Some(row) = self.selected_row() else {
            return Activated::Nothing;
        };
        let (resource, title) = match row {
            Row::Header(_) => return Activated::Nothing,
            Row::Song(track) => return Activated::Play(track.clone()),
            Row::Artist(a) => (Resource::Artist(a.id.clone()), a.name.clone()),
            Row::Album(a) => (Resource::Album(a.id.clone()), a.name.clone()),
        };
        self.open_view(resource, title);
        Activated::Opened
    }

    /// Goes back to the previous view; the root stays.
    pub fn go_up(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
        }
    }

    /// Refetches the shown view's list and replaces its cache entry. While
    /// a fetch for it is already running, that one is awaited instead.
    pub fn refresh(&mut self) {
        let Some(resource) = self.current().map(|v| v.resource.clone()) else {
            return;
        };
        let id = match self.in_flight.get(&resource) {
            Some(id) => *id,
            None => {
                self.cache.remove(&resource);
                self.request(resource.clone())
            }
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
        self.outbox.push(FetchRequest { id, resource });
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
        self.input = Some(String::new());
    }

    /// Adds a typed char; control characters and chars past
    /// [`MAX_QUERY_CHARS`] are ignored.
    pub fn input_char(&mut self, c: char) {
        let mut buf = [0u8; 4];
        self.input_str(c.encode_utf8(&mut buf));
    }

    /// Adds pasted (or typed) text. Line breaks and tabs become a single
    /// space (`\r\n` gives one space, not two); other control characters
    /// are dropped, so a paste never submits. Input stops at
    /// [`MAX_QUERY_CHARS`]; work is bounded by the cap, not the paste size.
    pub fn input_str(&mut self, text: &str) {
        let Some(input) = &mut self.input else {
            return;
        };
        let mut len = input.chars().count();
        let mut previous = None;
        for c in text.chars() {
            if len >= MAX_QUERY_CHARS {
                break;
            }
            let crlf = previous == Some('\r') && c == '\n';
            previous = Some(c);
            let c = match c {
                '\n' | '\r' | '\t' => ' ',
                c if c.is_control() => continue,
                c => c,
            };
            if !crlf {
                input.push(c);
                len += 1;
            }
        }
    }

    pub fn input_backspace(&mut self) {
        if let Some(input) = &mut self.input {
            input.pop();
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
        let query = input.trim();
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

    /// `a`: enqueues the selected song, album (all songs in order) or
    /// artist (all albums in served order, each album's songs in order).
    /// Missing lists are fetched; the job completes after every earlier
    /// job, in its turn.
    pub fn enqueue_selected(&mut self) -> Vec<JobOutcome> {
        let Some(row) = self.selected_row() else {
            return Vec::new();
        };
        let (label, target) = match row {
            Row::Header(_) => return Vec::new(),
            Row::Song(t) => (t.display_name(), JobTarget::Song(Box::new(t.clone()))),
            Row::Album(a) => (a.name.clone(), JobTarget::Album(a.id.clone())),
            Row::Artist(a) => (a.name.clone(), JobTarget::Artist(a.id.clone())),
        };
        self.jobs.push_back(Job {
            label: label.clone(),
            target,
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
            JobTarget::Album(id) => {
                let resource = Resource::Album(id.clone());
                if self.cache.contains_key(&resource) {
                    Vec::new()
                } else {
                    vec![resource]
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
        let songs = |id: &str| match self.cache.get(&Resource::Album(id.to_owned())) {
            Some(Content::Songs(songs)) => Some(songs.clone()),
            _ => None,
        };
        match target {
            JobTarget::Song(track) => Some(vec![(**track).clone()]),
            JobTarget::Album(id) => songs(id),
            JobTarget::Artist(id) => match self.cache.get(&Resource::Artist(id.clone()))? {
                Content::Albums(albums) => {
                    let mut out = Vec::new();
                    for album in albums {
                        out.extend(songs(&album.id)?);
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
            if let Some(error) = &job.error {
                outcomes.push(JobOutcome::Failed {
                    label: job.label.clone(),
                    error: error.clone(),
                });
            } else if let Some(tracks) = self.tracks(&job.target) {
                outcomes.push(JobOutcome::Added {
                    label: job.label.clone(),
                    tracks,
                });
            } else {
                break;
            }
            self.jobs.pop_front();
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

    // ----- responses -------------------------------------------------------

    /// Applies a fetch result: the cache always, views only if they still
    /// await `response.id`. Returns the enqueue jobs that completed.
    pub fn handle_response(&mut self, response: FetchResponse) -> Vec<JobOutcome> {
        let FetchResponse {
            id,
            resource,
            result,
        } = response;
        if self.in_flight.get(&resource) == Some(&id) {
            self.in_flight.remove(&resource);
        }
        self.job_fetches.remove(&id);
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
        self.fix_selection();
        self.pump()
    }
}

/// Runs one fetch.
pub async fn fetch(client: &Client, resource: &Resource) -> Result<Content, Error> {
    match resource {
        Resource::Artists => client.artists().await.map(Content::Artists),
        Resource::Artist(id) => client.artist(id).await.map(Content::Albums),
        Resource::Album(id) => client.album(id).await.map(Content::Songs),
        Resource::Search(query) => client.search(query).await.map(Content::Search),
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

    fn ok(browser: &mut Browser, request: &FetchRequest, content: Content) -> Vec<JobOutcome> {
        browser.handle_response(FetchResponse {
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
        b.ensure_started();
        b.ensure_started();
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
        b.ensure_started();
        let root = b.take_requests().remove(0);
        ok(
            &mut b,
            &root,
            Content::Artists(vec![artist("1"), artist("2")]),
        );
        let waiting = b.enqueue_selected();
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
        b.enqueue_selected();
        let none = b.take_requests().remove(0);
        let outcomes = ok(&mut b, &none, Content::Albums(Vec::new()));
        assert_eq!(
            outcomes,
            [JobOutcome::Added {
                label: "Artist 2".into(),
                tracks: Vec::new()
            }]
        );
    }

    #[test]
    fn selection_ignores_headers_and_moves_only_within_ready_lists() {
        let mut b = Browser::new();
        b.ensure_started();
        // Loading: moving does nothing and nothing is selected.
        b.move_selection(1);
        assert_eq!(b.activate(), Activated::Nothing);
        assert!(b.enqueue_selected().is_empty());
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
        b.ensure_started();
        let root = b.take_requests().remove(0);
        ok(
            &mut b,
            &root,
            Content::Artists(vec![artist("big"), artist("other")]),
        );
        b.enqueue_selected();
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
        b.ensure_started();
        let root = b.take_requests().remove(0);
        b.handle_response(FetchResponse {
            id: root.id,
            resource: Resource::Artists,
            result: Err(Error::Http(500)),
        });
        assert!(matches!(b.current().unwrap().load, Load::Failed(_)));
        // Any successful answer for the list (e.g. one a job requested).
        b.handle_response(FetchResponse {
            id: RequestId(99),
            resource: Resource::Artists,
            result: Ok(Content::Artists(vec![artist("1")])),
        });
        assert_eq!(b.current().unwrap().load, Load::Ready);
        // A failure for an unrelated id leaves a ready view alone.
        b.handle_response(FetchResponse {
            id: RequestId(100),
            resource: Resource::Artists,
            result: Err(Error::Http(500)),
        });
        assert_eq!(b.current().unwrap().load, Load::Ready);
    }
}
