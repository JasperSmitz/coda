//! Artists and albums grouped by tags, updated incrementally as scan
//! batches arrive. Refers to tracks by [`TrackId`]; the tracks themselves
//! live in the [`LibraryTree`](super::tree::LibraryTree).
//!
//! - **Artist:** the album artist, else the artist, else "Unknown artist".
//! - **Album:** (artist, album tag); a track without an album tag is in
//!   "Unknown album" under its artist. The same album name under two
//!   album artists is two albums.
//! - Names compare case-insensitively with surrounding whitespace trimmed
//!   (tags are trimmed when read); the most common spelling is shown.
//! - Artists sort A–Z, an artist's albums A–Z, the unknown ones last.
//!   Album tracks sort by disc, track number, title, then path.
//!
//! Inserting a track costs a few hash lookups and, for a new artist or
//! album, one binary-search insert into each sorted list.

use std::cmp::Ordering;
use std::collections::HashMap;

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

    /// Adds track `id` (`tracks[id]`) to its artist and album.
    pub fn insert(&mut self, id: TrackId, tracks: &[Track]) {
        let track = &tracks[id.index()];
        let artist_tag = track
            .album_artist
            .as_deref()
            .filter(|a| !a.trim().is_empty())
            .or(track.artist.as_deref());
        let artist = self.artist_for(key_of(artist_tag));
        let album = self.album_for(artist, key_of(track.album.as_deref()));
        self.artists[artist.0 as usize].tracks += 1;
        let list = &mut self.albums[album.0 as usize].tracks;
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
}
