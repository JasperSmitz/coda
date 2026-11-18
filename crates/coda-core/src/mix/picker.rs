//! Which songs a mix plays next. Pure: no I/O, randomness from the
//! caller's [`Rng`] (seeded in tests).
//!
//! - **Balance (decision 4):** ingredients take turns. Each round visits
//!   every ingredient that has songs once, in a new random order, and each
//!   turn picks a random song of that ingredient, so three artists are a
//!   third each whatever their album counts.
//! - **The window wins over balance (decisions 4 and 5):** an ingredient
//!   whose every song is recent sits out its turn while any other
//!   ingredient still has a song that is not. So a 3-track album beside a
//!   2,000-song genre plays a track only when one has left the window,
//!   instead of every other song. The window is at most half the songs, so
//!   some ingredient always has a song outside it; when none has (a
//!   safety net), the turn plays its song picked longest ago.
//! - **Overlap is not favoured:** a song that fits several ingredients
//!   belongs to only one of them, the one with the fewest songs (the
//!   earliest on a tie), so it has one chance per round like any other
//!   song, and a small ingredient inside a large one (an album inside its
//!   genre) keeps its songs.
//! - **Variety (decision 5):** no song repeats within a window of recent
//!   picks (half the mix's distinct songs, at most
//!   [`MAX_RECENT_WINDOW`]); the next song avoids the previous song's
//!   album, and then its artist, when its ingredient has an alternative.
//!   Among the songs outside the window, one not yet picked since its
//!   pool was set comes first (before spacing), so each song of a pool
//!   plays about once before the pool is played through.
//!
//! Pools arrive per ingredient ([`Picker::set_pool`]): at once for a local
//! mix, or as each fetch returns for a Navidrome mix. A pool that is a
//! sample (a Navidrome genre's random songs) is worth fetching again once
//! it is [played through](Picker::played_through): every song it owns was
//! picked at least once since it was set, whatever the window. A batch
//! ([`Picker::next`]) that plays a pool through goes on picking from it as
//! usual; the resolver replaces it afterwards with [`Picker::set_pool`],
//! which keeps the recent picks, so the new sample's songs that were just
//! played still wait their turn.

use std::collections::{HashMap, HashSet};

use crate::model::{Track, TrackSource};
use crate::queue::Rng;

/// The longest no-repeat window: a song plays again only after this many
/// other picks at most.
pub const MAX_RECENT_WINDOW: usize = 200;

/// One candidate song and the keys spacing compares.
#[derive(Debug, Clone)]
struct Song {
    track: Track,
    /// The grouping artist (album artist, else artist), lowercased.
    artist: Option<String>,
    /// The album: Navidrome's album id, else the artist and the album tag
    /// lowercased.
    album: Option<String>,
}

impl Song {
    fn new(track: Track) -> Self {
        let lower = |s: Option<&str>| {
            s.map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_lowercase)
        };
        let artist = lower(
            track
                .album_artist
                .as_deref()
                .filter(|a| !a.trim().is_empty())
                .or(track.artist.as_deref()),
        );
        let album = match (&track.album_id, lower(track.album.as_deref())) {
            (Some(id), _) => Some(format!("id:{id}")),
            (None, Some(album)) => Some(format!(
                "{}\u{1f}{album}",
                artist.as_deref().unwrap_or_default()
            )),
            (None, None) => None,
        };
        Self {
            track,
            artist,
            album,
        }
    }
}

#[derive(Debug, Clone)]
enum Pool {
    /// Not resolved yet (a Navidrome fetch in flight): skipped.
    Pending,
    Ready {
        songs: Vec<Song>,
        /// Indices into `songs` this ingredient owns (see the module docs).
        owned: Vec<usize>,
        /// The songs picked from this pool since it was set, for
        /// [`Picker::played_through`]. Never pruned; at most the pool.
        played: HashSet<TrackSource>,
    },
}

/// What spacing avoids: the previous pick's album and artist.
#[derive(Debug, Clone)]
struct Previous {
    artist: Option<String>,
    album: Option<String>,
}

/// The songs a mix picks from, and what it picked recently.
#[derive(Debug, Clone)]
pub struct Picker {
    pools: Vec<Pool>,
    /// The ingredients still to visit this round; taken from the back.
    round: Vec<usize>,
    /// When each song was last picked (its pick number).
    last_pick: HashMap<TrackSource, u64>,
    /// Picks made so far.
    picks: u64,
    window: usize,
    previous: Option<Previous>,
}

impl Picker {
    /// A picker for `ingredients` ingredients whose pools are all still
    /// to come ([`Picker::set_pool`]).
    pub fn new(ingredients: usize) -> Self {
        Self {
            pools: vec![Pool::Pending; ingredients],
            round: Vec::new(),
            last_pick: HashMap::new(),
            picks: 0,
            window: 0,
            previous: None,
        }
    }

    /// A picker with every pool given, one per ingredient in order.
    pub fn from_pools(pools: Vec<Vec<Track>>) -> Self {
        let mut picker = Self::new(pools.len());
        for (i, pool) in pools.into_iter().enumerate() {
            picker.set_ready(i, pool);
        }
        picker.reassign();
        picker
    }

    /// Sets (or replaces) ingredient `i`'s songs. Out of range is ignored.
    pub fn set_pool(&mut self, i: usize, tracks: Vec<Track>) {
        if i < self.pools.len() {
            self.set_ready(i, tracks);
            self.reassign();
        }
    }

    fn set_ready(&mut self, i: usize, tracks: Vec<Track>) {
        self.pools[i] = Pool::Ready {
            songs: tracks.into_iter().map(Song::new).collect(),
            owned: Vec::new(),
            played: HashSet::new(),
        };
    }

    /// The ingredients whose songs are still to come.
    pub fn pending(&self) -> Vec<usize> {
        self.indices(|pool| matches!(pool, Pool::Pending))
    }

    /// Every pool is known.
    pub fn is_ready(&self) -> bool {
        self.pending().is_empty()
    }

    /// The ingredients that found no songs at all.
    pub fn empty(&self) -> Vec<usize> {
        self.indices(|pool| matches!(pool, Pool::Ready { songs, .. } if songs.is_empty()))
    }

    /// The ingredients with songs to pick whose every song has been
    /// picked at least once since its pool was set: a pool that is a
    /// sample (a Navidrome genre's random songs) is worth fetching again.
    /// Independent of the recent window.
    pub fn played_through(&self) -> Vec<usize> {
        self.indices(|pool| match pool {
            Pool::Ready {
                songs,
                owned,
                played,
            } => {
                !owned.is_empty()
                    && owned
                        .iter()
                        .all(|&s| played.contains(&songs[s].track.source))
            }
            Pool::Pending => false,
        })
    }

    /// Ingredient `i` has songs to pick, and all of them are recent.
    fn all_recent(&self, i: usize) -> bool {
        match &self.pools[i] {
            Pool::Ready { songs, owned, .. } => {
                !owned.is_empty() && owned.iter().all(|&s| self.is_recent(&songs[s].track))
            }
            Pool::Pending => false,
        }
    }

    /// Some ingredient other than `i` has a song that is not recent.
    fn fresh_elsewhere(&self, i: usize) -> bool {
        self.pools.iter().enumerate().any(|(j, pool)| match pool {
            Pool::Ready { songs, owned, .. } if j != i => {
                owned.iter().any(|&s| !self.is_recent(&songs[s].track))
            }
            _ => false,
        })
    }

    fn indices(&self, f: impl Fn(&Pool) -> bool) -> Vec<usize> {
        (0..self.pools.len())
            .filter(|&i| f(&self.pools[i]))
            .collect()
    }

    /// The window: half the distinct songs, at most [`MAX_RECENT_WINDOW`].
    pub fn window(&self) -> usize {
        self.window
    }

    /// Gives each distinct song to one ingredient (the smallest that has
    /// it, the earliest on a tie) and sizes the window.
    fn reassign(&mut self) {
        let mut owner: HashMap<&TrackSource, (usize, usize)> = HashMap::new();
        for (i, pool) in self.pools.iter().enumerate() {
            if let Pool::Ready { songs, .. } = pool {
                for song in songs {
                    let size = songs.len();
                    owner
                        .entry(&song.track.source)
                        .and_modify(|o| {
                            if size < o.1 {
                                *o = (i, size);
                            }
                        })
                        .or_insert((i, size));
                }
            }
        }
        let owner: HashMap<TrackSource, usize> = owner
            .into_iter()
            .map(|(source, (i, _))| (source.clone(), i))
            .collect();
        self.window = (owner.len() / 2).min(MAX_RECENT_WINDOW);
        for (i, pool) in self.pools.iter_mut().enumerate() {
            if let Pool::Ready { songs, owned, .. } = pool {
                let mut seen = std::collections::HashSet::new();
                *owned = (0..songs.len())
                    .filter(|&s| {
                        let source = &songs[s].track.source;
                        owner.get(source) == Some(&i) && seen.insert(source.clone())
                    })
                    .collect();
            }
        }
    }

    fn is_recent(&self, track: &Track) -> bool {
        self.last_pick
            .get(&track.source)
            .is_some_and(|&at| self.picks - at < self.window as u64)
    }

    /// The next `n` songs (fewer only when no ingredient has songs). A
    /// pool played through during the batch is picked from as usual.
    pub fn next(&mut self, n: usize, rng: &mut Rng) -> Vec<Track> {
        let mut picked = Vec::with_capacity(n);
        while picked.len() < n {
            let Some(i) = self.next_ingredient(rng) else {
                break;
            };
            if let Some(track) = self.pick_from(i, rng) {
                picked.push(track);
            }
        }
        self.forget_old();
        picked
    }

    /// The next ingredient to pick from: the next turn, skipping an
    /// ingredient whose songs are all recent while another has a song that
    /// is not (it sits out that turn). `None` when no ingredient has songs.
    fn next_ingredient(&mut self, rng: &mut Rng) -> Option<usize> {
        // Ends: a skipped ingredient's state does not change, and one with
        // a fresh song comes up within two rounds.
        loop {
            let i = self.next_turn(rng)?;
            if !self.all_recent(i) || !self.fresh_elsewhere(i) {
                return Some(i);
            }
        }
    }

    /// The next turn, starting a new round (in a new random order) when
    /// this one is done. `None` when no ingredient has songs.
    fn next_turn(&mut self, rng: &mut Rng) -> Option<usize> {
        let has_songs =
            |pool: &Pool| matches!(pool, Pool::Ready { owned, .. } if !owned.is_empty());
        while let Some(i) = self.round.pop() {
            // A pool may have changed since the round began.
            if has_songs(&self.pools[i]) {
                return Some(i);
            }
        }
        self.round = self.indices(has_songs);
        crate::queue::shuffle_with_first(&mut self.round, None, rng);
        self.round.pop()
    }

    /// A song of ingredient `i`: among those not picked recently, one not
    /// yet picked since its pool was set if there is one (so a pool is
    /// [played through](Picker::played_through) in about as many of its
    /// own picks as it has songs); among those, one of another album and
    /// artist than the previous pick if there is one, else of another
    /// album, else any. When all are recent, the one picked longest ago.
    fn pick_from(&mut self, i: usize, rng: &mut Rng) -> Option<Track> {
        let Pool::Ready {
            songs,
            owned,
            played,
        } = &self.pools[i]
        else {
            return None;
        };
        const RECENT: u8 = 6;
        let same = |a: &Option<String>, b: &Option<String>| a.is_some() && a == b;
        let score = |song: &Song| -> u8 {
            if self.is_recent(&song.track) {
                return RECENT;
            }
            let replay = if played.contains(&song.track.source) {
                3
            } else {
                0
            };
            replay
                + match &self.previous {
                    Some(p) if same(&song.album, &p.album) => 2,
                    Some(p) if same(&song.artist, &p.artist) => 1,
                    _ => 0,
                }
        };
        let mut best = u8::MAX;
        let mut candidates: Vec<usize> = Vec::new();
        for &s in owned {
            let score = score(&songs[s]);
            if score < best {
                best = score;
                candidates.clear();
            }
            if score == best {
                candidates.push(s);
            }
        }
        let chosen = if best == RECENT {
            // Every song is recent: the one picked longest ago.
            *candidates
                .iter()
                .min_by_key(|&&s| self.last_pick.get(&songs[s].track.source))?
        } else {
            *candidates.get(rng.below(candidates.len()))?
        };
        let song = songs[chosen].clone();
        if let Pool::Ready { played, .. } = &mut self.pools[i] {
            played.insert(song.track.source.clone());
        }
        self.picks += 1;
        self.last_pick.insert(song.track.source.clone(), self.picks);
        self.previous = Some(Previous {
            artist: song.artist,
            album: song.album,
        });
        Some(song.track)
    }

    /// Drops pick records older than the window, so an endless mix keeps
    /// memory bounded.
    fn forget_old(&mut self) {
        if self.last_pick.len() > 2 * (self.window + MAX_RECENT_WINDOW) {
            let (picks, window) = (self.picks, self.window as u64);
            self.last_pick.retain(|_, at| picks - *at < window);
        }
    }
}

#[cfg(test)]
mod tests;
