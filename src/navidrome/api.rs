//! Serde types for Subsonic JSON responses (`subsonic-response`) and their
//! conversion into domain types.
//!
//! Parsing is lenient below the envelope: unknown fields are ignored,
//! missing fields are `None`, a field of the wrong type (or an out-of-range
//! number) is treated as missing, and a malformed list item is skipped
//! rather than failing the whole response.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use super::{Album, Artist, UNKNOWN_ALBUM, UNKNOWN_ARTIST};
use crate::model::{Track, TrackMetadata, clean_text};

/// Ids longer than this are treated as invalid and their item is skipped
/// (truncating an id would silently point at a different item).
pub const MAX_ID_CHARS: usize = 256;

/// Subsonic error codes meaning the credentials were rejected: 40 wrong
/// username or password, 41 token authentication not supported.
pub const AUTH_ERROR_CODES: [i64; 2] = [40, 41];

#[derive(Debug, Deserialize)]
pub struct Envelope {
    #[serde(rename = "subsonic-response")]
    pub response: Response,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Response {
    #[serde(deserialize_with = "lenient")]
    pub status: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub error: Option<ApiError>,
    #[serde(deserialize_with = "lenient")]
    pub artists: Option<ArtistsIndex>,
    #[serde(deserialize_with = "lenient")]
    pub artist: Option<RawArtist>,
    #[serde(deserialize_with = "lenient")]
    pub album: Option<RawAlbum>,
    #[serde(deserialize_with = "lenient")]
    pub search_result3: Option<SearchResult3>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ApiError {
    #[serde(deserialize_with = "lenient")]
    pub code: Option<i64>,
    #[serde(deserialize_with = "lenient")]
    pub message: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ArtistsIndex {
    #[serde(deserialize_with = "lenient_list")]
    pub index: Vec<Index>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Index {
    #[serde(deserialize_with = "lenient_list")]
    pub artist: Vec<RawArtist>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RawArtist {
    #[serde(deserialize_with = "lenient_id")]
    pub id: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub name: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub album_count: Option<u32>,
    #[serde(deserialize_with = "lenient_list")]
    pub album: Vec<RawAlbum>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RawAlbum {
    #[serde(deserialize_with = "lenient_id")]
    pub id: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub name: Option<String>,
    /// Older (non-ID3) responses use `title` instead of `name`.
    #[serde(deserialize_with = "lenient")]
    pub title: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub artist: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub year: Option<u32>,
    #[serde(deserialize_with = "lenient")]
    pub song_count: Option<u32>,
    #[serde(deserialize_with = "lenient_secs")]
    pub duration: Option<Duration>,
    #[serde(deserialize_with = "lenient_list")]
    pub song: Vec<RawSong>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RawSong {
    #[serde(deserialize_with = "lenient_id")]
    pub id: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub title: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub artist: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub album: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub track: Option<u32>,
    #[serde(deserialize_with = "lenient")]
    pub disc_number: Option<u32>,
    #[serde(deserialize_with = "lenient_secs")]
    pub duration: Option<Duration>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SearchResult3 {
    #[serde(deserialize_with = "lenient_list")]
    pub artist: Vec<RawArtist>,
    #[serde(deserialize_with = "lenient_list")]
    pub album: Vec<RawAlbum>,
    #[serde(deserialize_with = "lenient_list")]
    pub song: Vec<RawSong>,
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

/// Any value; `None` if it does not fit `T`.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).ok())
}

/// An array whose items that do not fit `T` are skipped; anything else is
/// an empty list.
fn lenient_list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::Array(items) => items
            .into_iter()
            .filter_map(|item| T::deserialize(item).ok())
            .collect(),
        _ => Vec::new(),
    })
}

/// An id: a string, or an integer (some Subsonic servers use numeric
/// ids). Oversized ids are rejected later by [`valid_id`]; other values
/// are `None`.
fn lenient_id<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::String(s) => Some(s),
        Value::Number(n) if n.is_u64() || n.is_i64() => Some(n.to_string()),
        _ => None,
    })
}

/// Seconds as an integer or float. Negative, NaN or overflowing values
/// are `None`, never a panic.
fn lenient_secs<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::Number(n) => match n.as_u64() {
            Some(secs) => Some(Duration::from_secs(secs)),
            None => n
                .as_f64()
                .and_then(|secs| Duration::try_from_secs_f64(secs).ok()),
        },
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MAX_METADATA_CHARS, UNKNOWN_TITLE};
    use serde_json::json;

    fn response(body: Value) -> Response {
        serde_json::from_value::<Envelope>(json!({ "subsonic-response": body }))
            .unwrap()
            .response
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
}
