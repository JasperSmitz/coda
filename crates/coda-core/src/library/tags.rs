//! Artists, albums and genres grouped by tags, updated incrementally as scan
//! batches arrive. Refers to tracks by [`TrackId`]; the tracks themselves
//! live in the [`LibraryTree`](super::tree::LibraryTree).
//!
//! - **Artist:** the album artist, else the artist, else "Unknown artist".
//! - **Album:** (artist, album tag); a track without an album tag is in
//!   "Unknown album" under its artist. The same album name under two
//!   album artists is two albums.
//! - Names compare case-insensitively with surrounding whitespace trimmed
//!   (tags are trimmed when read); the most common spelling is shown.
//! - **Genre:** each of a track's genres (a track has none, one or
//!   several; there is no "unknown" genre). A genre's albums are the
//!   albums any of its tracks is on.
//! - Artists sort A–Z, an artist's albums A–Z, the unknown ones last.
//!   Album tracks sort by disc, track number, title, then path. Genres
//!   sort A–Z; a genre's albums by artist, then album (as
//!   [`AlbumOrder::Artist`]).
//!
//! Inserting a track costs a few hash lookups and, for a new artist or
//! album, one binary-search insert into each sorted list.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::Duration;

use super::tree::{TrackId, compare_names};
use crate::model::Track;

/// Shown for tracks without an (album) artist tag.
pub const UNKNOWN_ARTIST: &str = "Unknown artist";
/// Shown for tracks without an album tag.
pub const UNKNOWN_ALBUM: &str = "Unknown album";
/// Distinct spellings counted per name; later ones are not counted (bounds
/// the work for hostile tags that vary only by case).
const MAX_SPELLINGS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArtistId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AlbumId(pub u32);

/// A genre's position in the index: dense, in the order genres were first
/// seen, and stable while the library grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GenreId(pub u32);

/// The spellings of one grouping key and how often each was seen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Spellings(Vec<(String, u32)>);

impl Spellings {
    fn add(&mut self, spelling: &str) {
        if let Some((_, n)) = self.0.iter_mut().find(|(s, _)| s == spelling) {
            *n = n.saturating_add(1);
        } else if self.0.len() < MAX_SPELLINGS {
            self.0.push((spelling.to_owned(), 1));
        }
    }

    /// The most common spelling; the first one seen on a tie.
    fn best(&self) -> Option<&str> {
        let mut best: Option<&(String, u32)> = None;
        for entry in &self.0 {
            if best.is_none_or(|b| entry.1 > b.1) {
                best = Some(entry);
            }
        }
        best.map(|(s, _)| s.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artist {
    id: ArtistId,
    /// Lowercased name; `None` for "Unknown artist".
    key: Option<String>,
    spellings: Spellings,
    /// Sorted A–Z, unknown last.
    albums: Vec<AlbumId>,
    tracks: usize,
}

impl Artist {
    pub fn id(&self) -> ArtistId {
        self.id
    }

    /// The most common spelling, or [`UNKNOWN_ARTIST`].
    pub fn name(&self) -> &str {
        self.spellings.best().unwrap_or(UNKNOWN_ARTIST)
    }

    pub fn albums(&self) -> &[AlbumId] {
        &self.albums
    }

    pub fn track_count(&self) -> usize {
        self.tracks
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    id: AlbumId,
    artist: ArtistId,
    /// Lowercased album tag; `None` for "Unknown album".
    key: Option<String>,
    spellings: Spellings,
    /// Sorted by disc, track number, title, path.
    tracks: Vec<TrackId>,
    /// The tracks' durations summed (saturating).
    duration: Duration,
    /// Some track's duration is unknown.
    duration_unknown: bool,
}

impl Album {
    pub fn id(&self) -> AlbumId {
        self.id
    }

    /// The most common spelling, or [`UNKNOWN_ALBUM`].
    pub fn name(&self) -> &str {
        self.spellings.best().unwrap_or(UNKNOWN_ALBUM)
    }

    pub fn artist(&self) -> ArtistId {
        self.artist
    }

    pub fn tracks(&self) -> &[TrackId] {
        &self.tracks
    }

    /// The total running time; `None` if any track's duration is unknown
    /// (a total that leaves tracks out would mislead) or there are no
    /// tracks.
    pub fn duration(&self) -> Option<Duration> {
        (!self.duration_unknown && !self.tracks.is_empty()).then_some(self.duration)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Genre {
    id: GenreId,
    /// Lowercased name.
    key: String,
    spellings: Spellings,
    /// Sorted by id (scan order), for membership tests.
    tracks: Vec<TrackId>,
    /// Sorted by artist, then album.
    albums: Vec<AlbumId>,
}

impl Genre {
    pub fn id(&self) -> GenreId {
        self.id
    }

    /// The most common spelling.
    pub fn name(&self) -> &str {
        self.spellings.best().unwrap_or_default()
    }

    /// The albums any of its tracks is on, by artist then album.
    pub fn albums(&self) -> &[AlbumId] {
        &self.albums
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// `true` if track `id` has this genre.
    pub fn contains(&self, id: TrackId) -> bool {
        self.tracks.binary_search(&id).is_ok()
    }
}

/// The order of the Albums list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlbumOrder {
    /// A–Z by album name, then artist.
    #[default]
    Name,
    /// By artist, then album name.
    Artist,
}

impl AlbumOrder {
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "A–Z",
            Self::Artist => "by artist",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Artist,
            Self::Artist => Self::Name,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TagIndex {
    artists: Vec<Artist>,
    albums: Vec<Album>,
    artist_ids: HashMap<Option<String>, ArtistId>,
    album_ids: HashMap<(ArtistId, Option<String>), AlbumId>,
    /// Every artist, A–Z.
    artists_sorted: Vec<ArtistId>,
    albums_by_name: Vec<AlbumId>,
    albums_by_artist: Vec<AlbumId>,
    genres: Vec<Genre>,
    genre_ids: HashMap<String, GenreId>,
    /// Every genre, A–Z.
    genres_sorted: Vec<GenreId>,
}

/// `None` keys (unknown) sort after every name.
fn compare_keys(a: &Option<String>, b: &Option<String>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// The grouping key and spelling of a tag value; `None` when absent.
fn key_of(value: Option<&str>) -> Option<(String, &str)> {
    let value = value.map(str::trim).filter(|v| !v.is_empty())?;
    Some((value.to_lowercase(), value))
}

/// The tag a track is grouped by: the album artist, else the artist.
fn artist_tag(track: &Track) -> Option<&str> {
    track
        .album_artist
        .as_deref()
        .filter(|a| !a.trim().is_empty())
        .or(track.artist.as_deref())
}

/// Album track order: disc, track number, title, then path.
pub fn compare_album_tracks(a: &Track, b: &Track) -> Ordering {
    a.disc_no
        .cmp(&b.disc_no)
        .then_with(|| a.track_no.cmp(&b.track_no))
        .then_with(|| compare_names(&a.title, &b.title))
        .then_with(|| a.local_path().cmp(&b.local_path()))
}

impl TagIndex {
    pub fn artist(&self, id: ArtistId) -> &Artist {
        &self.artists[id.0 as usize]
    }

    pub fn album(&self, id: AlbumId) -> &Album {
        &self.albums[id.0 as usize]
    }

    pub fn genre(&self, id: GenreId) -> &Genre {
        &self.genres[id.0 as usize]
    }

    /// Every genre, A–Z.
    pub fn genres(&self) -> &[GenreId] {
        &self.genres_sorted
    }

    /// The tracks of genre `id` in play order: its albums by artist then
    /// album, each album's tracks of that genre in album order.
    pub fn genre_tracks(&self, id: GenreId) -> Vec<TrackId> {
        let genre = self.genre(id);
        let mut ids = Vec::with_capacity(genre.tracks.len());
        for album in &genre.albums {
            ids.extend(
                self.album(*album)
                    .tracks()
                    .iter()
                    .copied()
                    .filter(|t| genre.contains(*t)),
            );
        }
        ids
    }

    /// Every artist, A–Z (unknown last).
    pub fn artists(&self) -> &[ArtistId] {
        &self.artists_sorted
    }

    /// Every album in `order`.
    pub fn albums(&self, order: AlbumOrder) -> &[AlbumId] {
        match order {
            AlbumOrder::Name => &self.albums_by_name,
            AlbumOrder::Artist => &self.albums_by_artist,
        }
    }

    /// The artist `track` is grouped under, if it was inserted.
    pub fn artist_of(&self, track: &Track) -> Option<ArtistId> {
        let key = key_of(artist_tag(track)).map(|(key, _)| key);
        self.artist_ids.get(&key).copied()
    }

    /// The artist shown as `name`, matched like tracks are grouped
    /// (trimmed, ignoring case), so it survives a rescan where
    /// [`ArtistId`]s do not. [`UNKNOWN_ARTIST`] finds the artist of
    /// untagged tracks unless an artist is really tagged that way (which
    /// then wins: the two share a shown name).
    pub fn artist_by_name(&self, name: &str) -> Option<ArtistId> {
        let key = key_of(Some(name)).map(|(key, _)| key);
        self.artist_ids.get(&key).copied().or_else(|| {
            let unknown = name.trim().eq_ignore_ascii_case(UNKNOWN_ARTIST);
            unknown
                .then(|| self.artist_ids.get(&None).copied())
                .flatten()
        })
    }

    /// The album shown as `name` under `artist`, matched like
    /// [`Self::artist_by_name`]; [`UNKNOWN_ALBUM`] finds the artist's
    /// untagged tracks unless an album is really tagged that way.
    pub fn album_by_name(&self, artist: ArtistId, name: &str) -> Option<AlbumId> {
        let key = key_of(Some(name)).map(|(key, _)| key);
        self.album_ids.get(&(artist, key)).copied().or_else(|| {
            let unknown = name.trim().eq_ignore_ascii_case(UNKNOWN_ALBUM);
            unknown
                .then(|| self.album_ids.get(&(artist, None)).copied())
                .flatten()
        })
    }

    /// The genre shown as `name`, matched like [`Self::artist_by_name`].
    pub fn genre_by_name(&self, name: &str) -> Option<GenreId> {
        let (key, _) = key_of(Some(name))?;
        self.genre_ids.get(&key).copied()
    }

    /// The album `track` is grouped under, if it was inserted.
    pub fn album_of(&self, track: &Track) -> Option<AlbumId> {
        let artist = self.artist_of(track)?;
        let key = key_of(track.album.as_deref()).map(|(key, _)| key);
        self.album_ids.get(&(artist, key)).copied()
    }

    /// Artists and albums whose shown name contains `query` (trimmed,
    /// ignoring case), each A–Z, at most `limit` of each. Empty for a
    /// blank query.
    pub fn search(&self, query: &str, limit: usize) -> (Vec<ArtistId>, Vec<AlbumId>) {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let has = |name: &str| name.to_lowercase().contains(&needle);
        let artists = self
            .artists_sorted
            .iter()
            .copied()
            .filter(|id| has(self.artist(*id).name()))
            .take(limit)
            .collect();
        let albums = self
            .albums_by_name
            .iter()
            .copied()
            .filter(|id| has(self.album(*id).name()))
            .take(limit)
            .collect();
        (artists, albums)
    }

    /// Adds track `id` (`tracks[id]`) to its artist, album and genres.
    pub fn insert(&mut self, id: TrackId, tracks: &[Track]) {
        let track = &tracks[id.index()];
        let artist = self.artist_for(key_of(artist_tag(track)));
        let album_id = self.album_for(artist, key_of(track.album.as_deref()));
        self.artists[artist.0 as usize].tracks += 1;
        // A track's genres are already distinct ignoring case.
        for name in &track.genres {
            if let Some(key) = key_of(Some(name)) {
                let genre = self.genre_for(key);
                self.add_to_genre(genre, id, album_id);
            }
        }
        let album = &mut self.albums[album_id.0 as usize];
        match track.duration {
            Some(duration) => album.duration = album.duration.saturating_add(duration),
            None => album.duration_unknown = true,
        }
        let list = &mut album.tracks;
        let at = list.partition_point(|t| {
            compare_album_tracks(&tracks[t.index()], track) != Ordering::Greater
        });
        list.insert(at, id);
    }

    fn artist_for(&mut self, key: Option<(String, &str)>) -> ArtistId {
        let (key, spelling) = match key {
            Some((key, spelling)) => (Some(key), Some(spelling)),
            None => (None, None),
        };
        let id = match self.artist_ids.get(&key) {
            Some(id) => *id,
            None => {
                let id = ArtistId(self.artists.len() as u32);
                self.artists.push(Artist {
                    id,
                    key: key.clone(),
                    spellings: Spellings::default(),
                    albums: Vec::new(),
                    tracks: 0,
                });
                self.artist_ids.insert(key, id);
                let artists = &self.artists;
                let at = self.artists_sorted.partition_point(|a| {
                    compare_keys(&artists[a.0 as usize].key, &artists[id.0 as usize].key)
                        == Ordering::Less
                });
                self.artists_sorted.insert(at, id);
                id
            }
        };
        if let Some(spelling) = spelling {
            self.artists[id.0 as usize].spellings.add(spelling);
        }
        id
    }

    fn album_for(&mut self, artist: ArtistId, key: Option<(String, &str)>) -> AlbumId {
        let (key, spelling) = match key {
            Some((key, spelling)) => (Some(key), Some(spelling)),
            None => (None, None),
        };
        let lookup = (artist, key);
        let id = match self.album_ids.get(&lookup) {
            Some(id) => *id,
            None => {
                let id = AlbumId(self.albums.len() as u32);
                self.albums.push(Album {
                    id,
                    artist,
                    key: lookup.1.clone(),
                    spellings: Spellings::default(),
                    tracks: Vec::new(),
                    duration: Duration::ZERO,
                    duration_unknown: false,
                });
                self.album_ids.insert(lookup, id);
                self.place_album(id);
                id
            }
        };
        if let Some(spelling) = spelling {
            self.albums[id.0 as usize].spellings.add(spelling);
        }
        id
    }

    fn genre_for(&mut self, (key, spelling): (String, &str)) -> GenreId {
        let id = match self.genre_ids.get(&key) {
            Some(id) => *id,
            None => {
                let id = GenreId(self.genres.len() as u32);
                let genres = &self.genres;
                let at = self
                    .genres_sorted
                    .partition_point(|g| genres[g.0 as usize].key < key);
                self.genres_sorted.insert(at, id);
                self.genre_ids.insert(key.clone(), id);
                self.genres.push(Genre {
                    id,
                    key,
                    spellings: Spellings::default(),
                    tracks: Vec::new(),
                    albums: Vec::new(),
                });
                id
            }
        };
        self.genres[id.0 as usize].spellings.add(spelling);
        id
    }

    /// Adds track `track` on album `album` to genre `id`.
    fn add_to_genre(&mut self, id: GenreId, track: TrackId, album: AlbumId) {
        let Self {
            artists,
            albums,
            genres,
            ..
        } = self;
        let genre = &mut genres[id.0 as usize];
        if let Err(at) = genre.tracks.binary_search(&track) {
            genre.tracks.insert(at, track);
        }
        let by_artist = |a: AlbumId, b: AlbumId| {
            let (a, b) = (&albums[a.0 as usize], &albums[b.0 as usize]);
            compare_keys(
                &artists[a.artist.0 as usize].key,
                &artists[b.artist.0 as usize].key,
            )
            .then_with(|| compare_keys(&a.key, &b.key))
            .then(a.id.cmp(&b.id))
        };
        if let Err(at) = genre.albums.binary_search_by(|a| by_artist(*a, album)) {
            genre.albums.insert(at, album);
        }
    }

    /// Inserts a new album into its artist's list and both album lists.
    fn place_album(&mut self, id: AlbumId) {
        let Self {
            artists,
            albums,
            albums_by_name,
            albums_by_artist,
            ..
        } = self;
        let album = |a: AlbumId| &albums[a.0 as usize];
        let artist_key = |a: AlbumId| &artists[album(a).artist.0 as usize].key;
        let by_album =
            |a: AlbumId, b: AlbumId| compare_keys(&album(a).key, &album(b).key).then(a.cmp(&b));
        let by_name = |a: AlbumId, b: AlbumId| {
            compare_keys(&album(a).key, &album(b).key)
                .then_with(|| compare_keys(artist_key(a), artist_key(b)))
                .then(a.cmp(&b))
        };
        let by_artist = |a: AlbumId, b: AlbumId| {
            compare_keys(artist_key(a), artist_key(b))
                .then_with(|| compare_keys(&album(a).key, &album(b).key))
                .then(a.cmp(&b))
        };
        let at = albums_by_name.partition_point(|a| by_name(*a, id) == Ordering::Less);
        albums_by_name.insert(at, id);
        let at = albums_by_artist.partition_point(|a| by_artist(*a, id) == Ordering::Less);
        albums_by_artist.insert(at, id);
        let own = &artists[album(id).artist.0 as usize].albums;
        let at = own.partition_point(|a| by_album(*a, id) == Ordering::Less);
        let artist = album(id).artist;
        artists[artist.0 as usize].albums.insert(at, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackMetadata;

    #[derive(Default)]
    struct Tags<'a> {
        title: Option<&'a str>,
        artist: Option<&'a str>,
        album_artist: Option<&'a str>,
        album: Option<&'a str>,
        disc: Option<u32>,
        no: Option<u32>,
        genres: &'a [&'a str],
    }

    fn track(path: &str, tags: Tags<'_>) -> Track {
        Track::local(
            format!("/m/{path}").into(),
            TrackMetadata {
                title: tags.title.map(Into::into),
                artist: tags.artist.map(Into::into),
                album_artist: tags.album_artist.map(Into::into),
                album: tags.album.map(Into::into),
                disc_no: tags.disc,
                track_no: tags.no,
                genres: tags.genres.iter().map(|g| (*g).to_owned()).collect(),
                ..TrackMetadata::default()
            },
        )
    }

    fn index(tracks: &[Track]) -> TagIndex {
        let mut index = TagIndex::default();
        for i in 0..tracks.len() {
            index.insert(TrackId(i as u32), tracks);
        }
        index
    }

    #[test]
    fn lookups_by_name_match_the_grouping_keys() {
        let tracks = [
            track(
                "a",
                Tags {
                    artist: Some("Yes"),
                    album: Some("Fragile"),
                    genres: &["Prog Rock"],
                    ..Tags::default()
                },
            ),
            track(
                "b",
                Tags {
                    artist: Some("YES "),
                    album: Some("fragile"),
                    ..Tags::default()
                },
            ),
            track(
                "c",
                Tags {
                    album_artist: Some("Genesis"),
                    artist: Some("Peter"),
                    ..Tags::default()
                },
            ),
            track("d", Tags::default()),
        ];
        let index = index(&tracks);
        let yes = index.artist_by_name("  yes").unwrap();
        assert_eq!(index.artist(yes).name(), "Yes");
        let fragile = index.album_by_name(yes, "FRAGILE").unwrap();
        assert_eq!(index.album(fragile).tracks().len(), 2);
        assert_eq!(index.album_by_name(yes, "Close to the Edge"), None);
        let genesis = index.artist_by_name("genesis").unwrap();
        assert_eq!(
            index.artist_by_name("Peter"),
            None,
            "grouped by album artist"
        );
        let untitled = index.album_by_name(genesis, UNKNOWN_ALBUM).unwrap();
        assert_eq!(index.album(untitled).tracks(), [TrackId(2)]);
        let unknown = index.artist_by_name("unknown ARTIST").unwrap();
        assert_eq!(index.artist(unknown).name(), UNKNOWN_ARTIST);
        assert_eq!(index.genre_by_name("prog rock"), Some(index.genres()[0]));
        assert_eq!(index.genre_by_name(" "), None);
        assert_eq!(index.genre_by_name("Rock"), None);

        // A real artist tagged "Unknown artist" wins over untagged tracks.
        let tracks = [
            track("a", Tags::default()),
            track(
                "b",
                Tags {
                    artist: Some("Unknown Artist"),
                    ..Tags::default()
                },
            ),
        ];
        let index = super::tests::index(&tracks);
        let found = index.artist_by_name(UNKNOWN_ARTIST).unwrap();
        assert_eq!(index.artist(found).albums().len(), 1);
        assert_eq!(
            index.album(index.artist(found).albums()[0]).tracks(),
            [TrackId(1)]
        );
    }

    fn artist_names(index: &TagIndex) -> Vec<&str> {
        index
            .artists()
            .iter()
            .map(|a| index.artist(*a).name())
            .collect()
    }

    fn album_rows(index: &TagIndex, order: AlbumOrder) -> Vec<String> {
        index
            .albums(order)
            .iter()
            .map(|a| {
                let album = index.album(*a);
                format!("{} / {}", album.name(), index.artist(album.artist()).name())
            })
            .collect()
    }

    fn titles(index: &TagIndex, tracks: &[Track], album: AlbumId) -> Vec<String> {
        index
            .album(album)
            .tracks()
            .iter()
            .map(|t| tracks[t.index()].title.clone())
            .collect()
    }

    #[test]
    fn an_album_length_is_known_only_if_every_track_length_is() {
        let timed = |path: &str, album: &'static str, secs: Option<u64>| {
            let mut track = track(
                path,
                Tags {
                    artist: Some("A"),
                    album: Some(album),
                    ..Tags::default()
                },
            );
            track.duration = secs.map(Duration::from_secs);
            track
        };
        let tracks = [
            timed("1.flac", "Whole", Some(200)),
            timed("2.flac", "Whole", Some(2572)),
            timed("3.flac", "Partial", Some(300)),
            timed("4.flac", "Partial", None),
            timed("5.flac", "Huge", Some(u64::MAX)),
            timed("6.flac", "Huge", Some(10)),
        ];
        let mut index = TagIndex::default();
        let length = |index: &TagIndex, name: &str| {
            index
                .albums(AlbumOrder::Name)
                .iter()
                .map(|a| index.album(*a))
                .find(|a| a.name() == name)
                .and_then(Album::duration)
        };
        // Grows as the scan finds tracks.
        index.insert(TrackId(0), &tracks);
        assert_eq!(length(&index, "Whole"), Some(Duration::from_secs(200)));
        index.insert(TrackId(1), &tracks);
        assert_eq!(length(&index, "Whole"), Some(Duration::from_secs(2772)));
        index.insert(TrackId(2), &tracks);
        assert_eq!(length(&index, "Partial"), Some(Duration::from_secs(300)));
        // One unknown track: no length rather than a short total, and it
        // stays unknown.
        index.insert(TrackId(3), &tracks);
        assert_eq!(length(&index, "Partial"), None);
        // Saturates rather than overflowing.
        index.insert(TrackId(4), &tracks);
        index.insert(TrackId(5), &tracks);
        assert_eq!(length(&index, "Huge"), Some(Duration::MAX));
    }

    #[test]
    fn album_artist_wins_over_artist() {
        let tracks = [
            track(
                "1.flac",
                Tags {
                    artist: Some("Guest"),
                    album_artist: Some("Various Artists"),
                    album: Some("Hits"),
                    ..Tags::default()
                },
            ),
            track(
                "2.flac",
                Tags {
                    artist: Some("Other"),
                    album_artist: Some("Various Artists"),
                    album: Some("Hits"),
                    ..Tags::default()
                },
            ),
            // A blank album artist falls back to the artist.
            track(
                "3.flac",
                Tags {
                    artist: Some("Solo"),
                    album_artist: Some("  "),
                    album: Some("Own"),
                    ..Tags::default()
                },
            ),
        ];
        let index = index(&tracks);
        assert_eq!(artist_names(&index), ["Solo", "Various Artists"]);
        let various = index.artists()[1];
        assert_eq!(index.artist(various).albums().len(), 1);
        assert_eq!(index.artist(various).track_count(), 2);
        assert_eq!(
            album_rows(&index, AlbumOrder::Name),
            ["Hits / Various Artists", "Own / Solo"]
        );
    }

    #[test]
    fn names_merge_ignoring_case_and_show_the_most_common_spelling() {
        let tracks: Vec<Track> = ["The Band", "the band", "THE BAND", "the band"]
            .iter()
            .enumerate()
            .map(|(i, artist)| {
                track(
                    &format!("{i}.flac"),
                    Tags {
                        artist: Some(artist),
                        album: Some(if i == 0 { "Album" } else { "ALBUM" }),
                        ..Tags::default()
                    },
                )
            })
            .collect();
        let index = index(&tracks);
        assert_eq!(artist_names(&index), ["the band"]);
        assert_eq!(album_rows(&index, AlbumOrder::Name), ["ALBUM / the band"]);
        // A tie keeps the first spelling seen.
        let tracks = [
            track(
                "a.flac",
                Tags {
                    artist: Some("Abc"),
                    ..Tags::default()
                },
            ),
            track(
                "b.flac",
                Tags {
                    artist: Some("ABC"),
                    ..Tags::default()
                },
            ),
        ];
        assert_eq!(artist_names(&self::index(&tracks)), ["Abc"]);
    }

    #[test]
    fn the_same_album_name_under_two_artists_is_two_albums() {
        let tracks = [
            track(
                "1.flac",
                Tags {
                    artist: Some("B"),
                    album: Some("Greatest Hits"),
                    ..Tags::default()
                },
            ),
            track(
                "2.flac",
                Tags {
                    artist: Some("A"),
                    album: Some("Greatest Hits"),
                    ..Tags::default()
                },
            ),
            track(
                "3.flac",
                Tags {
                    artist: Some("A"),
                    album: Some("Zebra"),
                    ..Tags::default()
                },
            ),
        ];
        let index = index(&tracks);
        assert_eq!(
            album_rows(&index, AlbumOrder::Name),
            ["Greatest Hits / A", "Greatest Hits / B", "Zebra / A"]
        );
        assert_eq!(
            album_rows(&index, AlbumOrder::Artist),
            ["Greatest Hits / A", "Zebra / A", "Greatest Hits / B"]
        );
    }

    #[test]
    fn unknown_artist_and_album_sort_last() {
        let tracks = [
            track("no-tags.flac", Tags::default()),
            track(
                "no-album.flac",
                Tags {
                    artist: Some("Zed"),
                    ..Tags::default()
                },
            ),
            track(
                "zed-album.flac",
                Tags {
                    artist: Some("Zed"),
                    album: Some("Zz"),
                    ..Tags::default()
                },
            ),
            track(
                "album-only.flac",
                Tags {
                    album: Some("Aa"),
                    ..Tags::default()
                },
            ),
        ];
        let index = index(&tracks);
        assert_eq!(artist_names(&index), ["Zed", UNKNOWN_ARTIST]);
        assert_eq!(
            album_rows(&index, AlbumOrder::Name),
            [
                "Aa / Unknown artist",
                "Zz / Zed",
                "Unknown album / Zed",
                "Unknown album / Unknown artist"
            ]
        );
        assert_eq!(
            album_rows(&index, AlbumOrder::Artist),
            [
                "Zz / Zed",
                "Unknown album / Zed",
                "Aa / Unknown artist",
                "Unknown album / Unknown artist"
            ]
        );
        let zed = index.artist(index.artists()[0]);
        let names: Vec<&str> = zed
            .albums()
            .iter()
            .map(|a| index.album(*a).name())
            .collect();
        assert_eq!(names, ["Zz", UNKNOWN_ALBUM]);
    }

    #[test]
    fn album_tracks_sort_by_disc_track_title_then_path() {
        let tag = |title, disc, no| Tags {
            title: Some(title),
            artist: Some("A"),
            album: Some("X"),
            disc,
            no,
            ..Tags::default()
        };
        let tracks = [
            track("z.flac", tag("d2t1", Some(2), Some(1))),
            track("y.flac", tag("d1t2", Some(1), Some(2))),
            track("b.flac", tag("same", Some(1), Some(1))),
            track("a.flac", tag("same", Some(1), Some(1))),
            track("c.flac", tag("Alpha", Some(1), Some(1))),
            track("n.flac", tag("none", None, None)),
        ];
        let index = index(&tracks);
        let album = index.albums(AlbumOrder::Name)[0];
        assert_eq!(
            titles(&index, &tracks, album),
            ["none", "Alpha", "same", "same", "d1t2", "d2t1"]
        );
        // The two "same" titles are in path order: a before b.
        let ids = index.album(album).tracks();
        assert_eq!(ids[2], TrackId(3));
        assert_eq!(ids[3], TrackId(2));
    }

    #[test]
    fn spellings_are_bounded() {
        let names: Vec<String> = (0..64)
            .map(|i| {
                (0..6)
                    .map(|b| if i >> b & 1 == 1 { 'A' } else { 'a' })
                    .collect()
            })
            .collect();
        let tracks: Vec<Track> = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                track(
                    &format!("{i}.flac"),
                    Tags {
                        artist: Some(n),
                        ..Tags::default()
                    },
                )
            })
            .collect();
        let index = index(&tracks);
        assert_eq!(index.artists().len(), 1);
        let artist = index.artist(index.artists()[0]);
        assert!(artist.spellings.0.len() <= MAX_SPELLINGS);
        assert_eq!(artist.name(), "aaaaaa");
        assert_eq!(artist.track_count(), 64);
    }

    #[test]
    fn a_track_finds_its_album_and_artist() {
        let tracks = [
            track(
                "a/1.flac",
                Tags {
                    artist: Some("Guest"),
                    album_artist: Some("Band"),
                    album: Some("Live"),
                    ..Tags::default()
                },
            ),
            track(
                "b/2.flac",
                Tags {
                    artist: Some("band "),
                    album: Some("live"),
                    ..Tags::default()
                },
            ),
            track("c/3.flac", Tags::default()),
        ];
        let index = index(&tracks);
        let album = index.album_of(&tracks[0]).unwrap();
        assert_eq!(index.album_of(&tracks[1]), Some(album));
        assert_eq!(index.album(album).tracks().len(), 2);
        let artist = index.artist_of(&tracks[1]).unwrap();
        assert_eq!(index.album(album).artist(), artist);
        let unknown = index.album_of(&tracks[2]).unwrap();
        assert_eq!(index.album(unknown).name(), UNKNOWN_ALBUM);
        // Not inserted: an artist the index has never seen.
        let stranger = track(
            "d/4.flac",
            Tags {
                artist: Some("Nobody"),
                ..Tags::default()
            },
        );
        assert_eq!(index.album_of(&stranger), None);
    }

    #[test]
    fn names_are_searched_ignoring_case() {
        let tracks = [
            track(
                "1.flac",
                Tags {
                    artist: Some("The Bandits"),
                    album: Some("Bandwagon"),
                    ..Tags::default()
                },
            ),
            track(
                "2.flac",
                Tags {
                    artist: Some("Other"),
                    album: Some("Quiet"),
                    ..Tags::default()
                },
            ),
        ];
        let index = index(&tracks);
        let (artists, albums) = index.search("  BAND ", 10);
        assert_eq!(artists.len(), 1);
        assert_eq!(index.artist(artists[0]).name(), "The Bandits");
        assert_eq!(albums.len(), 1);
        assert_eq!(index.album(albums[0]).name(), "Bandwagon");
        assert_eq!(index.search("   ", 10), (vec![], vec![]));
        assert_eq!(index.search("t", 1).0.len(), 1);
    }

    fn genre_rows(index: &TagIndex) -> Vec<String> {
        index
            .genres()
            .iter()
            .map(|g| {
                let genre = index.genre(*g);
                format!(
                    "{} ({} albums, {} tracks)",
                    genre.name(),
                    genre.albums().len(),
                    genre.track_count()
                )
            })
            .collect()
    }

    fn genre_named(index: &TagIndex, name: &str) -> GenreId {
        *index
            .genres()
            .iter()
            .find(|g| index.genre(**g).name() == name)
            .unwrap()
    }

    fn genre_albums(index: &TagIndex, genre: GenreId) -> Vec<String> {
        index
            .genre(genre)
            .albums()
            .iter()
            .map(|a| {
                let album = index.album(*a);
                format!("{} / {}", album.name(), index.artist(album.artist()).name())
            })
            .collect()
    }

    fn tagged<'a>(artist: &'a str, album: &'a str, no: u32, genres: &'a [&'a str]) -> Tags<'a> {
        Tags {
            title: None,
            artist: Some(artist),
            album: Some(album),
            no: Some(no),
            genres,
            ..Tags::default()
        }
    }

    #[test]
    fn genres_group_ignoring_case_and_show_the_most_common_spelling() {
        let tracks = [
            track("1.flac", tagged("A", "X", 1, &["Hip-Hop", "Jazz"])),
            track("2.flac", tagged("A", "X", 2, &["hip-hop"])),
            track("3.flac", tagged("B", "Y", 1, &["hip-hop"])),
            track("4.flac", tagged("B", "Y", 2, &[])),
            track("5.flac", tagged("C", "Z", 1, &["Ambient"])),
        ];
        let index = index(&tracks);
        // A–Z, with no entry for untagged tracks.
        assert_eq!(
            genre_rows(&index),
            [
                "Ambient (1 albums, 1 tracks)",
                "hip-hop (2 albums, 3 tracks)",
                "Jazz (1 albums, 1 tracks)",
            ]
        );
        // Ids are dense, in the order genres were first seen.
        assert_eq!(genre_named(&index, "hip-hop"), GenreId(0));
        assert_eq!(genre_named(&index, "Jazz"), GenreId(1));
        assert_eq!(genre_named(&index, "Ambient"), GenreId(2));
    }

    #[test]
    fn a_genre_has_the_albums_of_its_tracks_by_artist_then_album() {
        let tracks = [
            track("1.flac", tagged("Zed", "Alpha", 1, &["Rock"])),
            track(
                "2.flac",
                tagged("Abe", "Omega", 1, &["Rock", "Progressive Rock"]),
            ),
            track("3.flac", tagged("Abe", "Beta", 1, &["Pop"])),
            // One track of an album is enough.
            track("4.flac", tagged("Abe", "Beta", 2, &["Rock"])),
            track("5.flac", tagged("Abe", "Omega", 2, &["Rock"])),
            track("6.flac", tagged("Abe", "Omega", 3, &[])),
        ];
        let index = index(&tracks);
        let rock = genre_named(&index, "Rock");
        assert_eq!(
            genre_albums(&index, rock),
            ["Beta / Abe", "Omega / Abe", "Alpha / Zed"]
        );
        assert_eq!(index.genre(rock).track_count(), 4);
        // Playing the genre: its albums in that order, each album's tracks
        // of the genre in album order.
        let names: Vec<String> = index
            .genre_tracks(rock)
            .iter()
            .map(|t| tracks[t.index()].title.clone())
            .collect();
        assert_eq!(names, ["4", "2", "5", "1"]);
        let prog = genre_named(&index, "Progressive Rock");
        assert_eq!(genre_albums(&index, prog), ["Omega / Abe"]);
        assert_eq!(index.genre_tracks(prog), [TrackId(1)]);
    }

    #[test]
    fn genres_grow_with_incremental_batches() {
        let tracks = [
            track("1.flac", tagged("M", "Mid", 1, &["Folk"])),
            track("2.flac", tagged("Z", "Last", 1, &["folk", "Blues"])),
            track("3.flac", tagged("A", "First", 1, &["FOLK"])),
            track("4.flac", tagged("A", "First", 2, &["Folk", "Ambient"])),
        ];
        let mut index = TagIndex::default();
        index.insert(TrackId(0), &tracks);
        let folk = genre_named(&index, "Folk");
        assert_eq!(genre_albums(&index, folk), ["Mid / M"]);
        index.insert(TrackId(1), &tracks);
        index.insert(TrackId(2), &tracks);
        // The id stays; the list and albums grow in order around it.
        assert_eq!(genre_named(&index, "Folk"), folk);
        assert_eq!(
            genre_albums(&index, folk),
            ["First / A", "Mid / M", "Last / Z"]
        );
        assert_eq!(
            genre_rows(&index),
            ["Blues (1 albums, 1 tracks)", "Folk (3 albums, 3 tracks)"]
        );
        index.insert(TrackId(3), &tracks);
        assert_eq!(
            genre_rows(&index),
            [
                "Ambient (1 albums, 1 tracks)",
                "Blues (1 albums, 1 tracks)",
                "Folk (3 albums, 4 tracks)",
            ]
        );
        assert_eq!(index.genre_tracks(folk).len(), 4);
    }
}
