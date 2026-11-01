//! Serde types for Subsonic JSON responses (`subsonic-response`) and their
//! conversion into domain types.
//!
//! Parsing is lenient below the envelope: unknown fields are ignored,
//! missing fields are `None`, a field of the wrong type (or an out-of-range
//! number) is treated as missing, and a list item that is not an object
//! is skipped rather than failing the whole response.
//!
//! Nothing is ever built as a `serde_json::Value` (which costs many times
//! the body for small objects, and lets one hostile field of nested
//! arrays or objects take gigabytes): sections and list items are read
//! straight into typed structs, every field through a visitor that takes
//! only the scalar it expects and drains anything else (`IgnoredAny`),
//! unknown keys are skipped unparsed, strings are copied only up to the
//! length kept, and lists keep at most a cap of items (the rest drained
//! but counted). A malformed object (e.g. a duplicate key) fails the
//! response.
//!
//! Each endpoint parses its own reply type ([`Reply`]: `status`, `error`
//! and its one section; other keys are drained), and all objects built
//! from one response share one budget ([`MAX_RESPONSE_OBJECTS`]), so
//! nested lists cannot multiply the per-list caps.

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::time::Duration;

use serde::de::value::MapAccessDeserializer;
use serde::de::{DeserializeOwned, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::{Album, Artist, Playlist, UNKNOWN_ALBUM, UNKNOWN_ARTIST};
use crate::model::{MAX_METADATA_CHARS, Track, TrackMetadata, clean_text};
use crate::queue::MAX_QUEUE_LEN;

/// Ids longer than this are treated as invalid and their item is skipped
/// (truncating an id would silently point at a different item).
pub const MAX_ID_CHARS: usize = 256;

/// Subsonic error codes meaning the credentials were rejected: 40 wrong
/// username or password, 41 token authentication not supported.
pub const AUTH_ERROR_CODES: [i64; 2] = [40, 41];

/// Upper bound on the `size` of a `getAlbumList2` request (the Subsonic
/// maximum), and on the albums kept from one page.
pub const MAX_ALBUM_LIST_SIZE: usize = 500;
/// Upper bound on the playlists kept from `getPlaylists`.
pub const MAX_PLAYLISTS: usize = 10_000;
/// Upper bound on the songs kept from one `getPlaylist`: more could never
/// be queued anyway.
pub const MAX_PLAYLIST_ENTRIES: usize = MAX_QUEUE_LEN;
/// Upper bound on the items kept from any other list (artists of an index
/// entry, index entries, albums of an artist, songs of an album, search
/// results).
pub const MAX_LIST_ITEMS: usize = 50_000;
/// Name shown for a playlist whose name is missing or blank.
pub const UNKNOWN_PLAYLIST: &str = "Unnamed playlist";

/// Upper bound on the objects built from one response, shared by every
/// list and nesting level (`artists.index[].artist[].album[].song[]`
/// nests four deep, so per-list caps alone do not bound a response).
/// Objects past it are drained, not built. Fits the largest legitimate
/// responses: 10,000 playlist entries, a 50,000-artist index, an artist's
/// albums.
pub const MAX_RESPONSE_OBJECTS: usize = 100_000;

thread_local! {
    /// Objects the response being parsed on this thread may still build.
    static BUDGET: Cell<usize> = const { Cell::new(MAX_RESPONSE_OBJECTS) };
}

/// Gives the parse a full budget and restores it when dropped, also on
/// error or panic, so no parse inherits another's leftovers.
struct BudgetGuard;

impl BudgetGuard {
    fn start() -> Self {
        BUDGET.set(MAX_RESPONSE_OBJECTS);
        Self
    }
}

impl Drop for BudgetGuard {
    fn drop(&mut self) {
        BUDGET.set(MAX_RESPONSE_OBJECTS);
    }
}

/// Takes one object from the budget; `false` once it is spent.
fn take_budget() -> bool {
    BUDGET.with(|budget| match budget.get() {
        0 => false,
        n => {
            budget.set(n - 1);
            true
        }
    })
}

#[derive(Debug, Deserialize)]
struct Envelope<R> {
    #[serde(rename = "subsonic-response")]
    response: R,
}

/// One endpoint's response: `status`, `error` and its own section. Every
/// other key is drained unparsed.
pub trait Reply: DeserializeOwned {
    fn status(&self) -> Option<&str>;
    fn take_error(&mut self) -> Option<ApiError>;
}

/// Parses a `subsonic-response` body as `R`, within
/// [`MAX_RESPONSE_OBJECTS`]. Synchronous, so the thread-local budget
/// belongs to this parse alone.
pub fn parse<R: Reply>(body: &[u8]) -> serde_json::Result<R> {
    let _budget = BudgetGuard::start();
    serde_json::from_slice::<Envelope<R>>(body).map(|envelope| envelope.response)
}

macro_rules! reply {
    ($(#[$attr:meta])* $name:ident { $($key:literal => $field:ident: $ty:ty),* $(,)? }) => {
        $(#[$attr])*
        #[derive(Debug, Default, Deserialize)]
        #[serde(default)]
        pub struct $name {
            #[serde(deserialize_with = "text")]
            pub status: Option<String>,
            #[serde(deserialize_with = "object")]
            pub error: Option<ApiError>,
            $(
                #[serde(rename = $key, deserialize_with = "object")]
                pub $field: Option<$ty>,
            )*
        }

        $(#[$attr])*
        impl Reply for $name {
            fn status(&self) -> Option<&str> {
                self.status.as_deref()
            }

            fn take_error(&mut self) -> Option<ApiError> {
                self.error.take()
            }
        }
    };
}

reply!(
    /// `ping`, `scrobble`: no section.
    EmptyReply {}
);
reply!(ArtistsReply { "artists" => artists: ArtistsIndex });
reply!(ArtistReply { "artist" => artist: RawArtist });
reply!(AlbumReply { "album" => album: RawAlbum });
reply!(SearchReply { "searchResult3" => search_result3: SearchResult3 });
reply!(AlbumListReply { "albumList2" => album_list2: AlbumList2 });
reply!(PlaylistsReply { "playlists" => playlists: Playlists });
reply!(PlaylistReply { "playlist" => playlist: RawPlaylist });
reply!(
    /// Every section, for tests of the field parsers.
    #[cfg(test)]
    Response {
        "artists" => artists: ArtistsIndex,
        "artist" => artist: RawArtist,
        "album" => album: RawAlbum,
        "searchResult3" => search_result3: SearchResult3,
        "albumList2" => album_list2: AlbumList2,
        "playlists" => playlists: Playlists,
        "playlist" => playlist: RawPlaylist,
    }
);

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ApiError {
    #[serde(deserialize_with = "code")]
    pub code: Option<i64>,
    #[serde(deserialize_with = "text")]
    pub message: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ArtistsIndex {
    #[serde(deserialize_with = "list")]
    pub index: Vec<Index>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Index {
    #[serde(deserialize_with = "list")]
    pub artist: Vec<RawArtist>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RawArtist {
    #[serde(deserialize_with = "id")]
    pub id: Option<String>,
    #[serde(deserialize_with = "text")]
    pub name: Option<String>,
    #[serde(deserialize_with = "uint")]
    pub album_count: Option<u32>,
    #[serde(deserialize_with = "list")]
    pub album: Vec<RawAlbum>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RawAlbum {
    #[serde(deserialize_with = "id")]
    pub id: Option<String>,
    #[serde(deserialize_with = "text")]
    pub name: Option<String>,
    /// Older (non-ID3) responses use `title` instead of `name`.
    #[serde(deserialize_with = "text")]
    pub title: Option<String>,
    #[serde(deserialize_with = "text")]
    pub artist: Option<String>,
    #[serde(deserialize_with = "uint")]
    pub year: Option<u32>,
    #[serde(deserialize_with = "saturating_count")]
    pub song_count: Option<u32>,
    #[serde(deserialize_with = "saturating_secs")]
    pub duration: Option<Duration>,
    #[serde(deserialize_with = "list")]
    pub song: Vec<RawSong>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RawSong {
    #[serde(deserialize_with = "id")]
    pub id: Option<String>,
    #[serde(deserialize_with = "text")]
    pub title: Option<String>,
    #[serde(deserialize_with = "text")]
    pub artist: Option<String>,
    #[serde(deserialize_with = "text")]
    pub album: Option<String>,
    #[serde(deserialize_with = "uint")]
    pub track: Option<u32>,
    #[serde(deserialize_with = "uint")]
    pub disc_number: Option<u32>,
    #[serde(deserialize_with = "secs")]
    pub duration: Option<Duration>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SearchResult3 {
    #[serde(deserialize_with = "list")]
    pub artist: Vec<RawArtist>,
    #[serde(deserialize_with = "list")]
    pub album: Vec<RawAlbum>,
    #[serde(deserialize_with = "list")]
    pub song: Vec<RawSong>,
}

/// `getAlbumList2`. Subsonic omits `album` for an empty list.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AlbumList2 {
    #[serde(deserialize_with = "album_items")]
    pub album: Counted<RawAlbum>,
}

/// `getPlaylists`. Subsonic omits `playlist` when there are none.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Playlists {
    #[serde(deserialize_with = "playlist_items")]
    pub playlist: Counted<RawPlaylist>,
}

/// A playlist from `getPlaylists` (without entries) or `getPlaylist`
/// (with its songs as `entry`).
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RawPlaylist {
    #[serde(deserialize_with = "id")]
    pub id: Option<String>,
    #[serde(deserialize_with = "text")]
    pub name: Option<String>,
    #[serde(deserialize_with = "saturating_count")]
    pub song_count: Option<u32>,
    #[serde(deserialize_with = "saturating_secs")]
    pub duration: Option<Duration>,
    #[serde(deserialize_with = "playlist_entries")]
    pub entry: Counted<RawSong>,
}

/// The usable items of a capped list, and how many items it had in all.
#[derive(Debug)]
pub struct Counted<T> {
    /// Items that parsed, from the first `cap` items, in order.
    pub items: Vec<T>,
    /// Every item seen, including skipped and malformed ones.
    pub served: usize,
}

impl<T> Default for Counted<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            served: 0,
        }
    }
}

impl RawPlaylist {
    /// `None` if the id is missing, empty or oversized.
    pub fn into_playlist(self) -> Option<Playlist> {
        Some(Playlist {
            id: valid_id(self.id)?,
            name: clean_text(self.name).unwrap_or_else(|| UNKNOWN_PLAYLIST.to_owned()),
            song_count: self.song_count,
            duration: self.duration,
        })
    }
}

impl RawArtist {
    /// `None` if the id is missing, empty or oversized.
    pub fn into_artist(self) -> Option<Artist> {
        Some(Artist {
            id: valid_id(self.id)?,
            name: clean_text(self.name).unwrap_or_else(|| UNKNOWN_ARTIST.to_owned()),
            album_count: self.album_count,
        })
    }
}

impl RawAlbum {
    /// `None` if the id is missing, empty or oversized.
    pub fn into_album(self) -> Option<Album> {
        Some(Album {
            id: valid_id(self.id)?,
            name: clean_text(self.name)
                .or_else(|| clean_text(self.title))
                .unwrap_or_else(|| UNKNOWN_ALBUM.to_owned()),
            artist: clean_text(self.artist),
            year: self.year,
            song_count: self.song_count,
            duration: self.duration,
        })
    }
}

impl RawSong {
    /// `None` if the id is missing, empty or oversized.
    pub fn into_track(self) -> Option<Track> {
        let id = valid_id(self.id)?;
        Some(Track::navidrome(
            id,
            TrackMetadata {
                title: self.title,
                artist: self.artist,
                album: self.album,
                track_no: self.track,
                disc_no: self.disc_number,
                duration: self.duration,
            },
        ))
    }
}

/// Caps an untrusted server error message.
pub fn clean_message(message: Option<String>) -> String {
    clean_text(message).unwrap_or_else(|| "unknown error".to_owned())
}

fn valid_id(id: Option<String>) -> Option<String> {
    let id = id?;
    (!id.is_empty() && id.chars().count() <= MAX_ID_CHARS).then_some(id)
}

/// What a lenient field saw: a scalar it may accept, or anything else
/// (already consumed without building it).
#[derive(Debug, Clone, Copy)]
enum Scalar<'a> {
    Str(&'a str),
    U64(u64),
    I64(i64),
    F64(f64),
}

/// Reads one JSON value of any type without building it: a string or
/// number is handed to `accept`; `null`, booleans, arrays and objects are
/// skipped (their contents drained unparsed) and give `None`.
fn scalar<'de, D, T>(
    deserializer: D,
    accept: impl for<'a> FnOnce(Scalar<'a>) -> Option<T>,
) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
{
    struct ScalarVisitor<F>(F);

    impl<'de, T, F> Visitor<'de> for ScalarVisitor<F>
    where
        F: for<'a> FnOnce(Scalar<'a>) -> Option<T>,
    {
        type Value = Option<T>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("any JSON value")
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok((self.0)(Scalar::Str(v)))
        }

        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok((self.0)(Scalar::U64(v)))
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok((self.0)(Scalar::I64(v)))
        }

        fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
            Ok((self.0)(Scalar::F64(v)))
        }

        fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_bytes<E: serde::de::Error>(self, _: &[u8]) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
            d.deserialize_any(self)
        }

        fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
            drain_seq(seq).map(|()| None)
        }

        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
            drain_map(map).map(|()| None)
        }
    }

    deserializer.deserialize_any(ScalarVisitor(accept))
}

fn drain_seq<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<(), A::Error> {
    while seq.next_element::<IgnoredAny>()?.is_some() {}
    Ok(())
}

fn drain_map<'de, A: MapAccess<'de>>(mut map: A) -> Result<(), A::Error> {
    while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
    Ok(())
}

/// A string, copied only up to what [`clean_text`] keeps: leading
/// whitespace dropped, at most [`MAX_METADATA_CHARS`] chars.
fn text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    scalar(deserializer, |value| match value {
        Scalar::Str(s) => Some(s.trim_start().chars().take(MAX_METADATA_CHARS).collect()),
        _ => None,
    })
}

/// An id: a string, or an integer (some Subsonic servers use numeric
/// ids). Ids over [`MAX_ID_CHARS`] are `None` (never copied); other
/// values are `None`.
fn id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    scalar(deserializer, |value| match value {
        Scalar::Str(s) => {
            (s.chars().take(MAX_ID_CHARS + 1).count() <= MAX_ID_CHARS).then(|| s.to_owned())
        }
        Scalar::U64(n) => Some(n.to_string()),
        Scalar::I64(n) => Some(n.to_string()),
        Scalar::F64(_) => None,
    })
}

/// An integer that fits a `u32`; anything else is `None`.
fn uint<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u32>, D::Error> {
    scalar(deserializer, |value| match value {
        Scalar::U64(n) => u32::try_from(n).ok(),
        Scalar::I64(n) => u32::try_from(n).ok(),
        _ => None,
    })
}

/// An integer that fits an `i64` (Subsonic error codes).
fn code<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
    scalar(deserializer, |value| match value {
        Scalar::U64(n) => i64::try_from(n).ok(),
        Scalar::I64(n) => Some(n),
        _ => None,
    })
}

/// A count: integers beyond `u32::MAX` (and huge floats) saturate;
/// negative or non-numeric values are `None`.
fn saturating_count<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u32>, D::Error> {
    scalar(deserializer, |value| match value {
        Scalar::U64(n) => Some(u32::try_from(n).unwrap_or(u32::MAX)),
        // A float `as` cast saturates (and the sign is checked first).
        Scalar::F64(n) if n >= 0.0 => Some(n as u32),
        _ => None,
    })
}

/// Seconds as an integer or float. Negative, NaN or overflowing values
/// are `None`, never a panic.
fn secs<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Duration>, D::Error> {
    scalar(deserializer, |value| match value {
        Scalar::U64(n) => Some(Duration::from_secs(n)),
        Scalar::F64(n) => Duration::try_from_secs_f64(n).ok(),
        _ => None,
    })
}

/// Seconds as an integer or float; values too large for a [`Duration`]
/// saturate to [`Duration::MAX`]. Negative or non-numeric values are
/// `None`, never a panic.
fn saturating_secs<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Duration>, D::Error> {
    scalar(deserializer, |value| match value {
        Scalar::U64(n) => Some(Duration::from_secs(n)),
        Scalar::F64(n) if n >= 0.0 => Some(Duration::try_from_secs_f64(n).unwrap_or(Duration::MAX)),
        _ => None,
    })
}

/// An object read straight into `T`; any other JSON type is drained and
/// gives `None`. `T`'s fields must all be lenient (see the module docs);
/// a malformed object (e.g. a duplicate key) fails the response.
struct ObjectSeed<T>(PhantomData<T>);

impl<T> ObjectSeed<T> {
    fn new() -> Self {
        Self(PhantomData)
    }
}

impl<'de, T: Deserialize<'de>> DeserializeSeed<'de> for ObjectSeed<T> {
    type Value = Option<T>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectSeed<T> {
    type Value = Option<T>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        if !take_budget() {
            return drain_map(map).map(|()| None);
        }
        T::deserialize(MapAccessDeserializer::new(map)).map(Some)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        drain_seq(seq).map(|()| None)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self)
    }

    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_bytes<E: serde::de::Error>(self, _: &[u8]) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
}

/// A section object; see [`ObjectSeed`].
fn object<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    ObjectSeed::new().deserialize(deserializer)
}

/// An array read one item at a time: each of the first `cap` items is
/// read as an object ([`ObjectSeed`]; other items are skipped), later
/// items are drained unparsed but counted. Any other JSON type is an
/// empty list.
fn capped_list<'de, D, T>(deserializer: D, cap: usize) -> Result<Counted<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct ListVisitor<T> {
        cap: usize,
        item: PhantomData<T>,
    }

    impl<'de, T: Deserialize<'de>> Visitor<'de> for ListVisitor<T> {
        type Value = Counted<T>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("any JSON value")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut list = Counted::default();
            loop {
                if list.served < self.cap {
                    match seq.next_element_seed(ObjectSeed::new())? {
                        None => break,
                        Some(Some(item)) => list.items.push(item),
                        Some(None) => {}
                    }
                } else if seq.next_element::<IgnoredAny>()?.is_none() {
                    break;
                }
                list.served = list.served.saturating_add(1);
            }
            Ok(list)
        }

        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
            drain_map(map).map(|()| Counted::default())
        }

        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
            d.deserialize_any(self)
        }

        fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok(Counted::default())
        }

        fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok(Counted::default())
        }

        fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok(Counted::default())
        }

        fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok(Counted::default())
        }

        fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
            Ok(Counted::default())
        }

        fn visit_bytes<E: serde::de::Error>(self, _: &[u8]) -> Result<Self::Value, E> {
            Ok(Counted::default())
        }

        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(Counted::default())
        }

        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(Counted::default())
        }
    }

    deserializer.deserialize_any(ListVisitor {
        cap,
        item: PhantomData,
    })
}

/// A list of up to [`MAX_LIST_ITEMS`] objects (artists, albums, songs,
/// index entries); see [`capped_list`].
fn list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    capped_list(deserializer, MAX_LIST_ITEMS).map(|list| list.items)
}

fn album_items<'de, D>(deserializer: D) -> Result<Counted<RawAlbum>, D::Error>
where
    D: Deserializer<'de>,
{
    capped_list(deserializer, MAX_ALBUM_LIST_SIZE)
}

fn playlist_items<'de, D>(deserializer: D) -> Result<Counted<RawPlaylist>, D::Error>
where
    D: Deserializer<'de>,
{
    capped_list(deserializer, MAX_PLAYLISTS)
}

fn playlist_entries<'de, D>(deserializer: D) -> Result<Counted<RawSong>, D::Error>
where
    D: Deserializer<'de>,
{
    capped_list(deserializer, MAX_PLAYLIST_ENTRIES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MAX_METADATA_CHARS, UNKNOWN_TITLE};
    use crate::navidrome::Playlist;
    use serde_json::{Value, json};

    fn response(body: Value) -> Response {
        let body = serde_json::to_vec(&json!({ "subsonic-response": body })).unwrap();
        parse::<Response>(&body).unwrap()
    }

    #[test]
    fn hostile_song_fields_are_tolerated() {
        let r = response(json!({
            "status": "ok",
            "album": { "id": "al", "song": [
                { "id": "1", "title": "x".repeat(100_000), "duration": -5, "track": -1 },
                { "id": "2", "duration": 1e300, "discNumber": 4_294_967_296u64 },
                { "id": "", "title": "empty id" },
                { "title": "no id" },
                { "id": "y".repeat(MAX_ID_CHARS + 1) },
                { "id": 42, "duration": 7.9, "title": ["wrong", "type"] },
                "not an object",
                { "id": "3", "duration": 18_446_744_073_709_551_615u64, "artist": 5 },
            ]}
        }));
        let tracks: Vec<Track> = r
            .album
            .unwrap()
            .song
            .into_iter()
            .filter_map(RawSong::into_track)
            .collect();
        assert_eq!(tracks.len(), 4, "{tracks:?}");
        assert_eq!(tracks[0].title.chars().count(), MAX_METADATA_CHARS);
        assert_eq!(tracks[0].duration, None);
        assert_eq!(tracks[0].track_no, None);
        assert_eq!(tracks[1].duration, None);
        assert_eq!(tracks[1].disc_no, None);
        assert_eq!(tracks[1].title, UNKNOWN_TITLE);
        assert_eq!(
            tracks[2].source,
            crate::model::TrackSource::Navidrome {
                song_id: "42".into()
            }
        );
        assert_eq!(tracks[2].duration, Some(Duration::from_secs_f64(7.9)));
        assert_eq!(tracks[2].title, UNKNOWN_TITLE);
        assert_eq!(tracks[3].duration, Some(Duration::from_secs(u64::MAX)));
        assert_eq!(tracks[3].artist, None);
    }

    #[test]
    fn missing_names_fall_back() {
        let artist = RawArtist {
            id: Some("a".into()),
            ..RawArtist::default()
        };
        assert_eq!(artist.into_artist().unwrap().name, UNKNOWN_ARTIST);
        let album = RawAlbum {
            id: Some("b".into()),
            title: Some("Old style".into()),
            ..RawAlbum::default()
        };
        assert_eq!(album.into_album().unwrap().name, "Old style");
        let album = RawAlbum {
            id: Some("b".into()),
            ..RawAlbum::default()
        };
        assert_eq!(album.into_album().unwrap().name, UNKNOWN_ALBUM);
    }

    #[test]
    fn wrong_typed_sections_are_treated_as_missing() {
        let r = response(json!({
            "status": "ok",
            "artists": "nope",
            "error": 5,
            "searchResult3": { "artist": {"id": "1"}, "album": null, "song": [] }
        }));
        assert!(r.artists.is_none());
        assert!(r.error.is_none());
        let search = r.search_result3.unwrap();
        assert!(search.artist.is_empty() && search.album.is_empty());
    }

    #[test]
    fn server_messages_are_capped() {
        let message = clean_message(Some("e".repeat(10_000)));
        assert_eq!(message.chars().count(), MAX_METADATA_CHARS);
        assert_eq!(clean_message(None), "unknown error");
    }

    fn envelope(body: &str) -> Response {
        parse::<Response>(format!(r#"{{"subsonic-response": {body}}}"#).as_bytes()).unwrap()
    }

    #[test]
    fn list_sections_of_the_wrong_type_are_missing_or_empty() {
        for value in ["null", "5", "true", "\"x\"", "[1, {}]", "1.5"] {
            let r = envelope(&format!(
                r#"{{"status": "ok", "albumList2": {value}, "playlists": {value}, "playlist": {value}}}"#
            ));
            assert!(r.album_list2.is_none(), "{value}");
            assert!(r.playlists.is_none(), "{value}");
            assert!(r.playlist.is_none(), "{value}");
            let r = envelope(&format!(
                r#"{{"status": "ok", "albumList2": {{"album": {value}}},
                    "playlist": {{"id": "p", "entry": {value}}}}}"#
            ));
            let list = r.album_list2.unwrap().album;
            let entries = r.playlist.unwrap().entry;
            let expected = usize::from(value.starts_with('['));
            assert_eq!(list.served, expected * 2, "{value}");
            assert_eq!(entries.served, expected * 2, "{value}");
            assert_eq!(list.items.len(), expected, "{value}: only the object");
        }
        // Subsonic omits the array of an empty list.
        let r = envelope(r#"{"status": "ok", "albumList2": {}, "playlists": {}}"#);
        assert_eq!(r.album_list2.unwrap().album.served, 0);
        assert_eq!(r.playlists.unwrap().playlist.served, 0);
    }

    #[test]
    fn capped_lists_keep_the_first_items_and_count_the_rest() {
        let items: Vec<String> = (0..MAX_ALBUM_LIST_SIZE + 25)
            .map(|i| format!(r#"{{"id": "a{i}", "song": [{{"id": "s"}}]}}"#))
            .collect();
        let r = envelope(&format!(
            r#"{{"status": "ok", "albumList2": {{"album": [{}]}}}}"#,
            items.join(",")
        ));
        let list = r.album_list2.unwrap().album;
        assert_eq!(list.items.len(), MAX_ALBUM_LIST_SIZE);
        assert_eq!(list.served, MAX_ALBUM_LIST_SIZE + 25);
        assert_eq!(list.items.last().unwrap().id.as_deref(), Some("a499"));
    }

    #[test]
    fn playlist_counts_and_durations_saturate() {
        let r = response(json!({
            "status": "ok",
            "playlists": { "playlist": [
                { "id": "1", "name": "x".repeat(10_000), "songCount": 1e12, "duration": 1e300 },
                { "id": "2", "songCount": 18_446_744_073_709_551_615u64, "duration": 4_294_967_296u64 },
                { "id": "3", "songCount": -4, "duration": -1, "name": "   " },
                { "id": "4", "songCount": "12", "duration": [1] },
                { "name": "no id" },
                { "id": "" },
                { "id": "y".repeat(MAX_ID_CHARS + 1) },
                7,
            ]}
        }));
        let list = r.playlists.unwrap().playlist;
        assert_eq!(list.served, 8);
        let playlists: Vec<Playlist> = list
            .items
            .into_iter()
            .filter_map(RawPlaylist::into_playlist)
            .collect();
        assert_eq!(playlists.len(), 4, "{playlists:?}");
        assert_eq!(playlists[0].name.chars().count(), MAX_METADATA_CHARS);
        assert_eq!(playlists[0].song_count, Some(u32::MAX));
        assert_eq!(playlists[0].duration, Some(Duration::MAX));
        assert_eq!(playlists[1].song_count, Some(u32::MAX));
        assert_eq!(
            playlists[1].duration,
            Some(Duration::from_secs(4_294_967_296))
        );
        assert_eq!(playlists[2].song_count, None);
        assert_eq!(playlists[2].duration, None);
        assert_eq!(playlists[2].name, UNKNOWN_PLAYLIST);
        assert_eq!(
            (playlists[3].song_count, playlists[3].duration),
            (None, None)
        );
    }

    #[test]
    fn duplicate_keys_fail_the_response() {
        for body in [
            r#"{"subsonic-response": {"status": "ok", "playlist": {"id": "p", "id": "q"}}}"#,
            r#"{"subsonic-response": {"status": "ok", "playlist": {"id": "p", "entry": [{"id": "1", "id": "2"}]}}}"#,
        ] {
            assert!(parse::<Response>(body.as_bytes()).is_err(), "{body}");
        }
    }

    /// `"<key>": [{"a":0},{"a":0},…]` of about `bytes` bytes.
    fn nested(key: &str, bytes: usize) -> String {
        let item = r#"{"a":0},"#;
        let mut out = format!(r#""{key}":["#);
        out.push_str(&item.repeat(bytes / item.len()));
        out.push_str(r#"{"a":0}]"#);
        out
    }

    #[test]
    fn hostile_nested_values_are_drained_not_built() {
        // One item whose known field, or an unknown key, holds a large
        // array of objects: skipped without building it, in every section.
        for key in ["title", "unknownKey", "id", "duration", "entry"] {
            let body = format!(
                r#"{{"subsonic-response": {{"status": "ok", "playlist": {{"id": "p", "entry": [{{"track": 1, {}}}]}}}}}}"#,
                nested(key, 1 << 20)
            );
            let r = parse::<Response>(body.as_bytes()).unwrap();
            let entries = r.playlist.unwrap().entry;
            assert_eq!(entries.served, 1, "{key}");
        }
        for section in [
            "artists",
            "artist",
            "album",
            "searchResult3",
            "albumList2",
            "error",
        ] {
            let body = format!(
                r#"{{"subsonic-response": {{"status": "ok", "{section}": {{{}, "x": 1}}}}}}"#,
                nested("zzz", 1 << 20)
            );
            assert!(parse::<Response>(body.as_bytes()).is_ok(), "{section}");
        }
        // Strings are copied only up to what is kept.
        let body = format!(
            r#"{{"subsonic-response": {{"status": "ok", "album": {{"id": "a", "name": "  {}"}}}}}}"#,
            "n".repeat(1 << 20)
        );
        let r = parse::<Response>(body.as_bytes()).unwrap();
        let name = r.album.unwrap().name.unwrap();
        assert_eq!(name.chars().count(), MAX_METADATA_CHARS);
        assert!(
            name.capacity() < 4 * MAX_METADATA_CHARS,
            "{}",
            name.capacity()
        );
        let body = format!(
            r#"{{"subsonic-response": {{"status": "ok", "album": {{"id": "{}"}}}}}}"#,
            "i".repeat(1 << 20)
        );
        let r = parse::<Response>(body.as_bytes()).unwrap();
        assert_eq!(r.album.unwrap().id, None);
    }

    /// `n` copies of a chunk that nests artists, albums and songs.
    fn nested_artists(n: usize) -> String {
        let chunk = r#"{"artist":[{"album":[{"song":[{},{},{}]},{}]},{}]},"#;
        format!(
            r#"{{"subsonic-response":{{"status":"ok","artists":{{"index":[{}{{}}]}}}}}}"#,
            chunk.repeat(n)
        )
    }

    #[test]
    fn one_budget_bounds_every_nesting_level_and_resets_between_parses() {
        // Nine objects per chunk: far more than the budget in all.
        let body = nested_artists(MAX_RESPONSE_OBJECTS);
        let r = parse::<ArtistsReply>(body.as_bytes()).unwrap();
        let index = r.artists.unwrap().index;
        let built: usize = index
            .iter()
            .map(|i| {
                1 + i
                    .artist
                    .iter()
                    .map(|a| 1 + a.album.iter().map(|al| 1 + al.song.len()).sum::<usize>())
                    .sum::<usize>()
            })
            .sum();
        // The section object itself takes one too.
        assert_eq!(built + 1, MAX_RESPONSE_OBJECTS);
        assert_eq!(BUDGET.get(), MAX_RESPONSE_OBJECTS, "reset after the parse");
        // The next parse starts with a full budget.
        let small = nested_artists(2);
        let r = parse::<ArtistsReply>(small.as_bytes()).unwrap();
        assert_eq!(r.artists.unwrap().index.len(), 3);
        // Also after a parse that fails half way.
        let broken = &body.as_bytes()[..body.len() / 2];
        assert!(parse::<ArtistsReply>(broken).is_err());
        assert_eq!(BUDGET.get(), MAX_RESPONSE_OBJECTS);
    }

    #[test]
    fn each_endpoint_reads_only_its_own_section() {
        let body = format!(
            r#"{{"subsonic-response":{{"status":"ok","playlist":{{"id":"p","entry":[{{"id":"s"}}]}},"artists":{{"index":[{}]}}}}}}"#,
            r#"{"artist":[{"id":"a"}]},"#.repeat(1000) + "{}"
        );
        let r = parse::<PlaylistReply>(body.as_bytes()).unwrap();
        assert_eq!(r.playlist.unwrap().entry.items.len(), 1);
        let r = parse::<ArtistsReply>(body.as_bytes()).unwrap();
        assert_eq!(r.artists.unwrap().index.len(), 1001);
    }
}
