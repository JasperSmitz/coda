//! A Navidrome mix's ingredients resolved to songs from its server
//! (decision 3), and the fetches a playing mix needs.
//!
//! - **Genre:** `getRandomSongs` with the server's exact genre value
//!   ([`RANDOM_SONGS_SIZE`] random songs). A sample is fetched again once
//!   the picker has [played it through](super::Picker::played_through);
//!   [`Picker::set_pool`](super::Picker::set_pool) keeps the recent picks.
//!   A sample of which the server sent fewer songs than a full request
//!   (counting songs it sent that are unusable) is the whole genre and is
//!   never fetched again. A fresh sample that comes back empty keeps the
//!   pool that plays and is asked for again after [`RETRY_AFTER`].
//! - **Artist:** `getArtist` (its albums, at most [`MAX_ARTIST_ALBUMS`]),
//!   then each album (`getAlbum`), in served order, until the pool holds
//!   [`MAX_POOL_TRACKS`] songs (the albums still in flight are then
//!   aborted), at most [`ALBUM_FETCHES`] albums ahead of the first not yet
//!   added (so albums that arrive early never pile up behind a slow one).
//!   Fetched once and kept (in the picker) for as long as the mix plays.
//!   A network failure, an aborted request, a refused account or a
//!   redirect fails the whole artist; an album the server no longer has, or that fails otherwise
//!   (an HTTP or Subsonic error, a response that does not decode), is
//!   skipped and the artist plays its other albums, unless no album came
//!   back at all.
//! - **Album:** `getAlbum`, kept the same way.
//! - **Id fallback:** when an artist or album has no id, or the server
//!   says it does not exist (Subsonic error 70), it is found by name: the
//!   first artist whose name matches ignoring case and surrounding space,
//!   searched with `search3`, else in the artist index (`getArtists`,
//!   since `search3` lists only the first 20 artists of a common name);
//!   the first album whose name and artist both match (searched by the
//!   album name, then by artist and album name; `search3` gives the album
//!   artist, so a mix stores an album with its album artist). An
//!   ingredient found nowhere is an empty pool: [`Finished::NoSongs`], for
//!   [`super::no_songs_message`].
//! - **One server:** a mix resolves only against the server it was made on
//!   ([`navidrome::server_id`](crate::navidrome::server_id)); against any
//!   other, [`MixFetches::new`] and [`Fetch::spawn`] refuse with
//!   [`ResolveError::OtherServer`] before any request, and the coordinator
//!   stops ([`Finished::OtherServer`]: the frontend ends the mix).
//! - **Limits:** a whole mix has at most [`ALBUM_FETCHES`] requests in
//!   flight, across all its ingredients (the coordinator's own semaphore,
//!   taken first), and every request also takes a permit of the caller's
//!   semaphore (the frontend's shared fetch limit,
//!   [`MAX_CONCURRENT_FETCHES`](crate::navidrome::browse::MAX_CONCURRENT_FETCHES)),
//!   so a mix never queues more than [`ALBUM_FETCHES`] requests there and
//!   browsing never waits behind a big mix. A pool holds at most
//!   [`MAX_POOL_TRACKS`] songs. Dropping the coordinator closes the mix's
//!   semaphore: its fetches still running make no further request and
//!   answer [`Error::Aborted`] (stale), so they soon release the shared
//!   permits.
//! - **Privacy:** genre values, names and ids go only into requests
//!   (encoded by the client); logs name the ingredient's kind and the
//!   error's [`detail`](crate::navidrome::Error::detail).
//!
//! # The coordinator
//!
//! [`MixFetches`] tracks, per ingredient, whether a fetch is in flight,
//! has failed (retried after [`RETRY_AFTER`], at most [`MAX_ATTEMPTS`]
//! times for a pool still to come) or is done. A frontend keeps it beside
//! its [`ActiveMix`] and drops both together:
//!
//! 1. [`MixFetches::new`] (checks the server), with
//!    `ActiveMix::new(mix, saved, Picker::new(n), &player)`.
//! 2. [`MixFetches::due`] gives the fetches to start; the frontend spawns
//!    each with [`Fetch::spawn`] (never on its loop), passing its shared
//!    fetch limit; the callback brings the [`Fetched`] back to
//!    [`MixFetches::finish`], which sets the pool. `spawn` is the one way
//!    to run a fetch, and it answers exactly once: a task that is aborted
//!    (or whose future is dropped) or panics answers [`Error::Aborted`], so
//!    an ingredient is never left in flight forever. A frontend never
//!    spawns or awaits a fetch while holding a permit of the shared limit
//!    (as a job wrapper would): the fetch takes those permits itself, per
//!    request, and holding one around it can deadlock the limit.
//! 3. Once [`MixFetches::settled`] (some pool has songs, or every pool
//!    still to come has found nothing or used its [`MAX_ATTEMPTS`]), the
//!    frontend starts the mix ([`ActiveMix::start`]); pools that come
//!    later join the next top-up.
//! 4. `due` again after every top-up (genre samples played through are
//!    fetched anew) **and** when [`MixFetches::next_retry`] comes (a timer
//!    the frontend arms after each `finish`), so a failed fetch is retried
//!    then, before and after the mix starts.
//! 5. On [`Finished::OtherServer`] (the account changed), the frontend
//!    ends the mix.
//!
//! Every coordinator has its own generation, and a result carries its
//! ticket: a result for another coordinator (a mix replaced by another),
//! for a mix whose queue was replaced ([`ActiveMix::has_ended`], also
//! before it started) or that is no longer the ingredient's fetch in
//! flight is [`Finished::Stale`] and changes nothing.

use std::iter::{Enumerate, Peekable};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::vec::IntoIter;

use tokio::sync::Semaphore;
use tokio::task::{JoinHandle, JoinSet};

use super::{ActiveMix, Ingredient, Mix, MixId, MixSource};
use crate::model::Track;
use crate::navidrome::api::RANDOM_SONGS_SIZE;
use crate::navidrome::browse::MAX_JOB_FETCHES;
use crate::navidrome::{Client, Error, server_id};
use crate::player::Player;

/// Requests one mix has in flight at most, across all its ingredients.
pub const ALBUM_FETCHES: usize = MAX_JOB_FETCHES;
/// An artist's albums fetched at most (the first, in served order).
pub const MAX_ARTIST_ALBUMS: usize = 200;
/// Songs in one ingredient's pool at most.
pub const MAX_POOL_TRACKS: usize = 10_000;
/// A failed fetch is tried again after this long ([`MixFetches::next_retry`]).
pub const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Attempts at an ingredient whose songs never arrived; after that it is
/// left out of the mix ([`MixFetches::failed`]).
pub const MAX_ATTEMPTS: u32 = 3;

/// Subsonic's "data not found".
const NOT_FOUND: i64 = 70;

/// Why a mix's songs could not be fetched.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// The mix was made on another server (or user) than the current
    /// client's.
    #[error("This mix was made on another Navidrome server")]
    OtherServer,
    /// A local mix.
    #[error("This mix is not a Navidrome mix")]
    NotNavidrome,
    #[error(transparent)]
    Navidrome(#[from] Error),
}

/// Which fetch a result answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket {
    generation: u64,
    index: usize,
    serial: u64,
}

/// One ingredient to fetch, from [`MixFetches::due`].
#[derive(Debug)]
pub struct Fetch {
    ticket: Ticket,
    /// The mix's server, checked again against the client that runs it.
    server: String,
    ingredient: Ingredient,
    /// The mix's own request limit ([`ALBUM_FETCHES`]).
    mix_limit: Arc<Semaphore>,
}

impl Fetch {
    /// The ingredient's index in the mix.
    pub fn index(&self) -> usize {
        self.ticket.index
    }

    pub fn ingredient(&self) -> &Ingredient {
        &self.ingredient
    }

    /// Fetches the ingredient's songs with `client` on a Tokio task, every
    /// request under a permit of the mix's own limit, then of `limit` (the
    /// frontend's shared one; never call this while holding a permit of
    /// it), and hands the result to `send` exactly once: if the task is
    /// aborted before it answers (or panics), `send` gets
    /// [`Error::Aborted`] instead. Refused without a request when `client`
    /// is for another server than the mix's. Logs failures without names.
    pub fn spawn<F>(self, client: Client, limit: Arc<Semaphore>, send: F) -> JoinHandle<()>
    where
        F: FnOnce(Fetched) + Send + 'static,
    {
        // Built before spawning, so even a task dropped before its first
        // poll answers.
        let reply = Reply {
            ticket: self.ticket,
            send: Some(send),
        };
        tokio::spawn(async move {
            let fetched = self.run(client, limit).await;
            reply.send(fetched);
        })
    }

    /// The fetch itself, for [`Fetch::spawn`].
    async fn run(self, client: Client, limit: Arc<Semaphore>) -> Fetched {
        let result = if server_id(&client) != self.server {
            Err(ResolveError::OtherServer)
        } else {
            let limits = Limits {
                mix: self.mix_limit,
                shared: limit,
            };
            resolve(&client, &limits, &self.ingredient)
                .await
                .map_err(ResolveError::from)
        };
        if let Err(ResolveError::Navidrome(Error::Aborted)) = &result {
            tracing::debug!(
                kind = kind(&self.ingredient),
                "mix ingredient fetch stopped"
            );
        } else if let Err(ResolveError::Navidrome(error)) = &result {
            tracing::warn!(
                kind = kind(&self.ingredient),
                detail = %error.detail(),
                "mix ingredient fetch failed"
            );
        }
        Fetched {
            ticket: self.ticket,
            result,
        }
    }
}

/// Hands a spawned fetch's result over exactly once; dropped without
/// sending (the task was aborted or panicked), it sends
/// [`Error::Aborted`], like the browser's fetches.
struct Reply<F: FnOnce(Fetched)> {
    ticket: Ticket,
    /// `None` once sent.
    send: Option<F>,
}

impl<F: FnOnce(Fetched)> Reply<F> {
    fn send(mut self, fetched: Fetched) {
        if let Some(send) = self.send.take() {
            send(fetched);
        }
    }
}

impl<F: FnOnce(Fetched)> Drop for Reply<F> {
    fn drop(&mut self) {
        if let Some(send) = self.send.take() {
            tracing::debug!("mix ingredient fetch ended without a result");
            send(Fetched::aborted(self.ticket));
        }
    }
}

/// An ingredient's songs, and whether they are all of it (a genre sample
/// that was not full).
#[derive(Debug)]
struct Songs {
    tracks: Vec<Track>,
    whole: bool,
}

/// A fetch's answer, for [`MixFetches::finish`].
#[derive(Debug)]
pub struct Fetched {
    ticket: Ticket,
    result: Result<Songs, ResolveError>,
}

impl Fetched {
    fn aborted(ticket: Ticket) -> Self {
        Self {
            ticket,
            result: Err(Error::Aborted.into()),
        }
    }

    /// The ingredient's index in the mix.
    pub fn index(&self) -> usize {
        self.ticket.index
    }
}

/// What [`MixFetches::finish`] did with a result.
#[derive(Debug, PartialEq, Eq)]
pub enum Finished {
    /// Not for this mix any more (another mix, the queue replaced, or not
    /// the fetch in flight): dropped, nothing changed.
    Stale,
    /// The ingredient's pool was set (or replaced) with `songs` songs.
    Pool { index: usize, songs: usize },
    /// The ingredient found no songs; its pool is set empty (decision 3).
    NoSongs { index: usize },
    /// A fresh sample came back empty: the pool it had is kept, and it is
    /// asked for again after [`RETRY_AFTER`].
    Unchanged { index: usize },
    /// The fetch failed; the pool is as it was. Retried after
    /// [`RETRY_AFTER`] ([`MixFetches::next_retry`]; a pool still to come
    /// at most [`MAX_ATTEMPTS`] times in all).
    Failed { index: usize },
    /// The client is for another server (or user) than the mix's: nothing
    /// more is fetched, and the frontend ends the mix.
    OtherServer { index: usize },
}

/// Per ingredient: what the coordinator knows of its fetches.
#[derive(Debug, Clone, Default)]
struct Slot {
    /// The serial of the fetch in flight.
    in_flight: Option<u64>,
    /// A fetch has answered at least once.
    answered: bool,
    /// Failed attempts in a row.
    failures: u32,
    /// Not to be fetched again before then.
    retry_at: Option<Instant>,
    /// A genre's sample held the whole genre: never fetched again.
    complete: bool,
}

/// The fetches of one playing Navidrome mix; see the module docs. Not
/// `Clone`: each coordinator is the only one with its generation.
#[derive(Debug)]
pub struct MixFetches {
    generation: u64,
    mix: MixId,
    server: String,
    slots: Vec<Slot>,
    serial: u64,
    /// The mix's own request limit, shared by its fetches.
    limit: Arc<Semaphore>,
    /// A fetch found the client on another server: nothing more to do.
    other_server: bool,
}

impl Drop for MixFetches {
    /// Stops the fetches still running: their next request is refused
    /// ([`Error::Aborted`]), so they no longer hold the shared permits.
    fn drop(&mut self) {
        self.limit.close();
    }
}

/// Generations handed out so far, so no two coordinators share one.
static GENERATIONS: AtomicU64 = AtomicU64::new(0);

impl MixFetches {
    /// For `mix` played with `client`: refused for a local mix
    /// ([`ResolveError::NotNavidrome`]) or one made on another server
    /// ([`ResolveError::OtherServer`]), so it never starts there.
    pub fn new(mix: &Mix, client: &Client) -> Result<Self, ResolveError> {
        let MixSource::Navidrome { server } = &mix.source else {
            return Err(ResolveError::NotNavidrome);
        };
        if *server != server_id(client) {
            return Err(ResolveError::OtherServer);
        }
        Ok(Self {
            generation: GENERATIONS.fetch_add(1, Ordering::Relaxed) + 1,
            mix: mix.id.clone(),
            server: server.clone(),
            slots: vec![Slot::default(); mix.ingredients.len()],
            serial: 0,
            limit: Arc::new(Semaphore::new(ALBUM_FETCHES)),
            other_server: false,
        })
    }

    /// Fetches in flight.
    pub fn in_flight(&self) -> usize {
        self.slots.iter().filter(|s| s.in_flight.is_some()).count()
    }

    /// A fetch found the client on another server ([`Finished::OtherServer`]):
    /// the mix cannot resolve and the frontend ends it.
    pub fn other_server(&self) -> bool {
        self.other_server
    }

    /// Time to start the mix with what is there: every ingredient still
    /// to come has answered at least once and has no fetch in flight, and
    /// either some pool has songs, or every pool still to come has used
    /// its [`MAX_ATTEMPTS`] (pools that found nothing are not to come).
    /// Never after [`Finished::OtherServer`].
    pub fn settled(&self, active: &ActiveMix) -> bool {
        if self.other_server {
            return false;
        }
        let picker = active.picker();
        let pending = picker.pending();
        let slot = |i: usize| self.slots.get(i);
        let answered = pending
            .iter()
            .all(|&i| slot(i).is_none_or(|s| s.answered && s.in_flight.is_none()));
        if !answered {
            return false;
        }
        let empty = picker.empty();
        let has_songs = (0..active.mix().ingredients.len())
            .any(|i| !pending.contains(&i) && !empty.contains(&i));
        has_songs
            || pending
                .iter()
                .all(|&i| slot(i).is_none_or(|s| s.failures >= MAX_ATTEMPTS))
    }

    /// The ingredients whose songs never arrived because their fetches
    /// failed (left out of the mix until a retry succeeds).
    pub fn failed(&self, active: &ActiveMix) -> Vec<usize> {
        active
            .picker()
            .pending()
            .into_iter()
            .filter(|&i| self.slots.get(i).is_some_and(|s| s.failures > 0))
            .collect()
    }

    /// When a failed (or empty) fetch is next due again: the frontend arms
    /// a timer and calls [`MixFetches::due`] then (as well as after every
    /// top-up). `None` when no retry is waiting.
    pub fn next_retry(&self, active: &ActiveMix) -> Option<Instant> {
        if self.other_server || active.mix().id != self.mix {
            return None;
        }
        let picker = active.picker();
        let (pending, played_through) = (picker.pending(), picker.played_through());
        active
            .mix()
            .ingredients
            .iter()
            .zip(&self.slots)
            .enumerate()
            .filter(|(i, (ingredient, slot))| {
                slot.in_flight.is_none() && wanted(*i, ingredient, slot, &pending, &played_through)
            })
            .filter_map(|(_, (_, slot))| slot.retry_at)
            .min()
    }

    /// The fetches to start now: each ingredient whose pool is still to
    /// come, and each genre whose sample is played through, unless a fetch
    /// of it is in flight, it failed less than [`RETRY_AFTER`] ago, or (a
    /// pool still to come) it failed [`MAX_ATTEMPTS`] times. None once the
    /// mix has ended, for another mix or after [`Finished::OtherServer`].
    pub fn due(&mut self, active: &ActiveMix, player: &Player, now: Instant) -> Vec<Fetch> {
        if self.other_server || active.mix().id != self.mix || active.has_ended(player) {
            return Vec::new();
        }
        let picker = active.picker();
        let pending = picker.pending();
        let played_through = picker.played_through();
        let mut fetches = Vec::new();
        for (i, ingredient) in active.mix().ingredients.iter().enumerate() {
            let Some(slot) = self.slots.get_mut(i) else {
                break;
            };
            let waiting = slot.in_flight.is_some() || slot.retry_at.is_some_and(|at| now < at);
            if waiting || !wanted(i, ingredient, slot, &pending, &played_through) {
                continue;
            }
            self.serial += 1;
            slot.in_flight = Some(self.serial);
            fetches.push(Fetch {
                ticket: Ticket {
                    generation: self.generation,
                    index: i,
                    serial: self.serial,
                },
                server: self.server.clone(),
                ingredient: ingredient.clone(),
                mix_limit: Arc::clone(&self.limit),
            });
        }
        fetches
    }

    /// Applies a fetch's result to `active`'s picker: a new pool, an empty
    /// one (found nothing), or nothing (a failure, an empty fresh sample, a
    /// stale result).
    pub fn finish(
        &mut self,
        fetched: Fetched,
        active: &mut ActiveMix,
        player: &Player,
        now: Instant,
    ) -> Finished {
        let Ticket {
            generation,
            index,
            serial,
        } = fetched.ticket;
        if generation != self.generation || active.mix().id != self.mix {
            return Finished::Stale;
        }
        let Some(slot) = self.slots.get_mut(index) else {
            return Finished::Stale;
        };
        if slot.in_flight != Some(serial) {
            return Finished::Stale;
        }
        slot.in_flight = None;
        if active.has_ended(player) {
            return Finished::Stale;
        }
        slot.answered = true;
        let pending = active.picker().pending().contains(&index);
        match fetched.result {
            Ok(Songs { tracks, whole }) => {
                slot.failures = 0;
                if tracks.is_empty() && !pending {
                    // The sample came back empty; keep what plays (and
                    // what was known of the genre), ask again later.
                    slot.retry_at = Some(now + RETRY_AFTER);
                    return Finished::Unchanged { index };
                }
                slot.retry_at = None;
                if matches!(
                    active.mix().ingredients.get(index),
                    Some(Ingredient::Genre { .. })
                ) {
                    slot.complete = whole;
                }
                let songs = tracks.len();
                active.picker_mut().set_pool(index, tracks);
                if songs == 0 {
                    Finished::NoSongs { index }
                } else {
                    Finished::Pool { index, songs }
                }
            }
            Err(ResolveError::OtherServer | ResolveError::NotNavidrome) => {
                self.other_server = true;
                Finished::OtherServer { index }
            }
            Err(ResolveError::Navidrome(_)) => {
                slot.failures = slot.failures.saturating_add(1);
                slot.retry_at = Some(now + RETRY_AFTER);
                Finished::Failed { index }
            }
        }
    }
}

/// Whether ingredient `i` wants a fetch (timing and fetches in flight
/// aside): its pool is still to come and it has attempts left, or it is a
/// genre sample played through that was not the whole genre.
fn wanted(
    i: usize,
    ingredient: &Ingredient,
    slot: &Slot,
    pending: &[usize],
    played_through: &[usize],
) -> bool {
    if pending.contains(&i) {
        slot.failures < MAX_ATTEMPTS
    } else {
        matches!(ingredient, Ingredient::Genre { .. })
            && !slot.complete
            && played_through.contains(&i)
    }
}

/// The ingredient's kind, for logs (never its name).
fn kind(ingredient: &Ingredient) -> &'static str {
    match ingredient {
        Ingredient::Genre { .. } => "genre",
        Ingredient::Artist { .. } => "artist",
        Ingredient::Album { .. } => "album",
    }
}

/// The semaphores a mix's request waits for: the mix's own, then the
/// frontend's shared one.
#[derive(Clone)]
struct Limits {
    mix: Arc<Semaphore>,
    shared: Arc<Semaphore>,
}

/// Runs `request` under a permit of the mix's limit, then of the shared
/// one. The mix's limit is closed when its coordinator is dropped: the
/// request is then not made ([`Error::Aborted`]). A closed shared limit
/// is unbounded (nobody closes it).
async fn limited<T>(
    limits: &Limits,
    request: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    let _mix = limits.mix.acquire().await.map_err(|_| Error::Aborted)?;
    let _shared = limits.shared.acquire().await.ok();
    // The coordinator may have been dropped while this request waited for a
    // shared permit: then it is not sent.
    if limits.mix.is_closed() {
        return Err(Error::Aborted);
    }
    request.await
}

/// The songs `ingredient` stands for on `client`'s server; empty when
/// nothing matches. Every request under `limits`.
async fn resolve(
    client: &Client,
    limits: &Limits,
    ingredient: &Ingredient,
) -> Result<Songs, Error> {
    let (mut tracks, whole) = match ingredient {
        Ingredient::Genre { name } => {
            let sample = limited(limits, client.random_songs_in_genre_counted(name)).await?;
            (sample.items, sample.served < RANDOM_SONGS_SIZE)
        }
        Ingredient::Artist { name, id } => (
            artist_tracks(client, limits, name, id.as_deref()).await?,
            false,
        ),
        Ingredient::Album { artist, name, id } => (
            album_tracks(client, limits, artist, name, id.as_deref()).await?,
            false,
        ),
    };
    tracks.truncate(MAX_POOL_TRACKS);
    Ok(Songs { tracks, whole })
}

/// `Ok(None)` for Subsonic's "not found".
fn found<T>(result: Result<T, Error>) -> Result<Option<T>, Error> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(Error::Server {
            code: NOT_FOUND, ..
        }) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Equal ignoring surrounding space and case.
fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

async fn artist_tracks(
    client: &Client,
    limits: &Limits,
    name: &str,
    id: Option<&str>,
) -> Result<Vec<Track>, Error> {
    let mut albums = match id {
        Some(id) => found(limited(limits, client.artist(id)).await)?,
        None => None,
    };
    if albums.is_none() {
        let search = limited(limits, client.search(name)).await?;
        let mut by_name = search
            .artists
            .into_iter()
            .find(|a| same_name(&a.name, name))
            .map(|a| a.id);
        if by_name.is_none() {
            // `search3` lists only the first few artists of a common name.
            by_name = limited(limits, client.artists())
                .await?
                .into_iter()
                .find(|a| same_name(&a.name, name))
                .map(|a| a.id);
        }
        if let Some(found_id) = by_name {
            albums = found(limited(limits, client.artist(&found_id)).await)?;
        }
    }
    let Some(mut albums) = albums else {
        return Ok(Vec::new());
    };
    albums.truncate(MAX_ARTIST_ALBUMS);
    let ids = albums.into_iter().map(|a| a.id).collect();
    albums_tracks(client, limits, ids).await
}

/// An album failure that fails a whole artist: the server is likely
/// unreachable (or refuses the account, or redirects) for every album,
/// so the artist is retried whole rather than kept partial.
fn fails_whole(error: &Error) -> bool {
    matches!(
        error,
        Error::Network(_) | Error::Aborted | Error::Auth | Error::Redirect(_)
    )
}

/// The songs of `ids`' albums, in that order, at most [`MAX_POOL_TRACKS`]:
/// an album is requested only while it is within [`ALBUM_FETCHES`] of
/// the first not yet added (those in flight plus those arrived early
/// never exceed that window), none once the pool is full (dropping the
/// set aborts those in flight). An album the server no longer has, or
/// that fails other than by the network, the account or a redirect, is
/// skipped; those fail the whole, and so does a skip when no album came
/// back at all.
async fn albums_tracks(
    client: &Client,
    limits: &Limits,
    ids: Vec<String>,
) -> Result<Vec<Track>, Error> {
    // Albums that arrived before one served ahead of them; with the
    // window below at most `ALBUM_FETCHES - 1` wait here, each capped.
    let mut arrived: Vec<Option<Vec<Track>>> = ids.iter().map(|_| None).collect();
    let mut next_out = 0;
    let mut tracks = Vec::new();
    let mut any_album = false;
    let mut skipped = 0usize;
    let mut first_skip: Option<Error> = None;
    let mut set = JoinSet::new();
    let mut ids = ids.into_iter().enumerate().peekable();
    // Requests the albums within `ALBUM_FETCHES` of `next_out`.
    let top_up =
        |set: &mut JoinSet<_>, ids: &mut Peekable<Enumerate<IntoIter<String>>>, next_out| {
            while let Some((i, id)) = ids.next_if(|(i, _)| *i < next_out + ALBUM_FETCHES) {
                let (client, limits) = (client.clone(), limits.clone());
                set.spawn(async move { (i, limited(&limits, client.album(&id)).await) });
            }
        };
    top_up(&mut set, &mut ids, next_out);
    while let Some(joined) = set.join_next().await {
        let (i, result) = joined.map_err(|_| Error::Aborted)?;
        let album = match found(result) {
            Ok(Some(mut album)) => {
                any_album = true;
                album.truncate(MAX_POOL_TRACKS);
                album
            }
            Ok(None) => Vec::new(),
            Err(error) if fails_whole(&error) => return Err(error),
            Err(error) => {
                skipped += 1;
                first_skip.get_or_insert(error);
                Vec::new()
            }
        };
        if let Some(place) = arrived.get_mut(i) {
            *place = Some(album);
        }
        while let Some(album) = arrived.get_mut(next_out).and_then(Option::take) {
            let room = MAX_POOL_TRACKS - tracks.len();
            tracks.extend(album.into_iter().take(room));
            next_out += 1;
        }
        if tracks.len() >= MAX_POOL_TRACKS {
            break;
        }
        top_up(&mut set, &mut ids, next_out);
    }
    if let Some(error) = first_skip {
        if !any_album {
            return Err(error);
        }
        tracing::warn!(
            skipped,
            detail = %error.detail(),
            "mix artist: albums that failed were skipped"
        );
    }
    Ok(tracks)
}

async fn album_tracks(
    client: &Client,
    limits: &Limits,
    artist: &str,
    name: &str,
    id: Option<&str>,
) -> Result<Vec<Track>, Error> {
    if let Some(id) = id
        && let Some(tracks) = found(limited(limits, client.album(id)).await)?
    {
        return Ok(tracks);
    }
    // By the album name, then by artist and album name (a common album
    // name may fill the first search with other artists' albums).
    for query in [name.to_owned(), format!("{artist} {name}")] {
        let search = limited(limits, client.search(&query)).await?;
        let album = search.albums.into_iter().find(|a| {
            same_name(&a.name, name) && a.artist.as_deref().is_some_and(|a| same_name(a, artist))
        });
        if let Some(album) = album {
            let tracks = found(limited(limits, client.album(&album.id)).await)?;
            return Ok(tracks.unwrap_or_default());
        }
    }
    Ok(Vec::new())
}
