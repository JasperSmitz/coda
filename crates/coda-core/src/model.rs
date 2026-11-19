//! Music-domain types shared by the library, queue and player.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where a track's audio comes from. Local and remote tracks share one
/// queue; the source is resolved to a playback target only when loaded.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TrackSource {
    /// An absolute path below the canonicalized music directory.
    Local { path: PathBuf },
    /// A song on the configured Navidrome server. The id is opaque and
    /// carries no credentials.
    Navidrome { song_id: String },
}

/// Optional metadata as read from tags. `Track` applies the fallbacks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// The album artist (lofty's `AlbumArtist` item; Navidrome's
    /// `displayAlbumArtist`). Groups compilations under one artist.
    pub album_artist: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub duration: Option<Duration>,
    /// Navidrome's `coverArt` id for the song (or its album). Opaque, like
    /// song ids; ignored for local tracks.
    pub cover_art: Option<String>,
    /// The genres, in tag order. Sources fill it with [`split_genres`];
    /// the `Track` constructors clean, dedupe and cap it again (not
    /// split), so a list that was split once (a saved queue) is kept as
    /// it is.
    pub genres: Vec<String>,
    /// The release year (lofty's recording date, else its year item).
    /// Read from local tags only.
    pub year: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub source: TrackSource,
    /// Tag title, or the file stem when the tag has none.
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub duration: Option<Duration>,
    /// The id to ask Navidrome's `getCoverArt` for; always `None` for
    /// local tracks (their art is read from the file or its folder).
    pub cover_art: Option<String>,
    /// Navidrome's album id (`getAlbum`), when the server sent one; always
    /// `None` for local tracks. For navigation only: not saved in the
    /// session.
    pub album_id: Option<String>,
    /// Navidrome's artist id (`getArtist`), like [`Track::album_id`].
    pub artist_id: Option<String>,
    /// The genres as tagged, in tag order: each cleaned like other text,
    /// no two equal ignoring case (the first spelling kept), at most
    /// [`MAX_GENRES`].
    pub genres: Vec<String>,
    /// The release year from local tags (1–9999); always `None` for
    /// Navidrome tracks and for tracks restored from a saved session.
    pub year: Option<u32>,
}

/// A tagged year if it is one (1–9999).
fn valid_year(year: Option<u32>) -> Option<u32> {
    year.filter(|y| (1..=9999).contains(y))
}

impl Track {
    /// Builds a local track, falling back to the file stem for the title.
    /// Blank tag values are treated as absent. Text is untrusted, so it is
    /// capped at [`MAX_METADATA_CHARS`] here, once, which bounds every
    /// later use (sorting, rendering).
    pub fn local(path: PathBuf, metadata: TrackMetadata) -> Self {
        let title = clean_text(metadata.title).unwrap_or_else(|| cap_chars(file_stem(&path)));
        Self {
            title,
            artist: clean_text(metadata.artist),
            album: clean_text(metadata.album),
            album_artist: clean_text(metadata.album_artist),
            track_no: metadata.track_no,
            disc_no: metadata.disc_no,
            duration: metadata.duration,
            cover_art: None,
            album_id: None,
            artist_id: None,
            genres: tidy_genres(metadata.genres),
            year: valid_year(metadata.year),
            source: TrackSource::Local { path },
        }
    }

    /// Builds a Navidrome track. There is no file name to fall back on, so
    /// a missing title becomes [`UNKNOWN_TITLE`]. Text is capped like tags.
    pub fn navidrome(song_id: String, metadata: TrackMetadata) -> Self {
        Self {
            title: clean_text(metadata.title).unwrap_or_else(|| UNKNOWN_TITLE.to_owned()),
            artist: clean_text(metadata.artist),
            album: clean_text(metadata.album),
            album_artist: clean_text(metadata.album_artist),
            track_no: metadata.track_no,
            disc_no: metadata.disc_no,
            duration: metadata.duration,
            cover_art: metadata.cover_art.filter(|id| !id.is_empty()),
            album_id: None,
            artist_id: None,
            genres: tidy_genres(metadata.genres),
            year: None,
            source: TrackSource::Navidrome { song_id },
        }
    }

    /// The local path, if this track is a local file.
    pub fn local_path(&self) -> Option<&Path> {
        match &self.source {
            TrackSource::Local { path } => Some(path),
            TrackSource::Navidrome { .. } => None,
        }
    }

    /// The Navidrome song id, if this track is streamed from Navidrome.
    pub fn navidrome_id(&self) -> Option<&str> {
        match &self.source {
            TrackSource::Navidrome { song_id } => Some(song_id),
            TrackSource::Local { .. } => None,
        }
    }

    /// `true` for tracks streamed from a server.
    pub fn is_remote(&self) -> bool {
        matches!(self.source, TrackSource::Navidrome { .. })
    }

    /// Human-readable name for status lines and logs: `Artist - Title` or
    /// just the title. Never contains the playback target.
    pub fn display_name(&self) -> String {
        match &self.artist {
            Some(artist) => format!("{artist} - {}", self.title),
            None => self.title.clone(),
        }
    }
}

/// Formats with `,` thousands separators.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `1 track`, `1,234 tracks`: `n` and `noun`, plural unless `n` is 1.
pub fn count(n: usize, noun: &str) -> String {
    let plural = if n == 1 { "" } else { "s" };
    format!("{} {noun}{plural}", thousands(n))
}

/// Upper bound on the length of metadata strings, in chars.
pub const MAX_METADATA_CHARS: usize = 512;

/// Title of a remote track whose metadata has none.
pub const UNKNOWN_TITLE: &str = "Unknown title";

/// Trims untrusted text and caps it at [`MAX_METADATA_CHARS`]; blank
/// becomes `None`. Used for every string from tags or a server.
pub fn clean_text(value: Option<String>) -> Option<String> {
    value
        .map(|s| cap_chars(s.trim().to_owned()))
        .filter(|s| !s.is_empty())
}

/// Upper bound on the genres kept per track; later ones are dropped.
pub const MAX_GENRES: usize = 16;

/// What one genre tag value is split on: the separators Navidrome splits
/// the `genre` tag on by default (`resources/mappings.yaml`), so a song
/// has the same genres from both sources.
pub const GENRE_SEPARATORS: [char; 3] = [';', '/', ','];

/// Splits genre tag values on [`GENRE_SEPARATORS`] into a track's genre
/// list: each part cleaned with [`clean_text`] (blank parts dropped), no
/// two equal ignoring case (the first spelling kept), in order, at most
/// [`MAX_GENRES`]. Used for local tags and Navidrome's `genre` string.
pub fn split_genres<I, S>(values: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut genres = Vec::new();
    for value in values {
        for part in value.as_ref().split(GENRE_SEPARATORS) {
            if genres.len() == MAX_GENRES {
                return genres;
            }
            add_genre(&mut genres, part);
        }
    }
    genres
}

/// Cleans, dedupes and caps a genre list that is already split.
fn tidy_genres(values: Vec<String>) -> Vec<String> {
    let mut genres = Vec::new();
    for value in &values {
        if genres.len() == MAX_GENRES {
            break;
        }
        add_genre(&mut genres, value);
    }
    genres
}

/// Appends `value`, cleaned, unless it is blank or already listed
/// (ignoring case).
fn add_genre(genres: &mut Vec<String>, value: &str) {
    // Copies no more than is kept.
    let Some(genre) = clean_text(Some(
        value.trim().chars().take(MAX_METADATA_CHARS).collect(),
    )) else {
        return;
    };
    let key = genre.to_lowercase();
    if !genres.iter().any(|g| g.to_lowercase() == key) {
        genres.push(genre);
    }
}

/// Truncates to at most [`MAX_METADATA_CHARS`] chars, on a char boundary.
fn cap_chars(mut s: String) -> String {
    if let Some((end, _)) = s.char_indices().nth(MAX_METADATA_CHARS) {
        s.truncate(end);
    }
    s
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_grouped_and_plural_unless_one() {
        assert_eq!(count(0, "track"), "0 tracks");
        assert_eq!(count(1, "track"), "1 track");
        assert_eq!(count(2, "album"), "2 albums");
        assert_eq!(count(1_234, "track"), "1,234 tracks");
    }

    #[test]
    fn title_falls_back_to_file_stem() {
        let track = Track::local("/music/a/01 Intro.flac".into(), TrackMetadata::default());
        assert_eq!(track.title, "01 Intro");
        assert_eq!(track.display_name(), "01 Intro");
        assert_eq!(
            track.local_path(),
            Some(Path::new("/music/a/01 Intro.flac"))
        );
    }

    #[test]
    fn blank_tags_are_treated_as_absent() {
        let meta = TrackMetadata {
            title: Some("  ".into()),
            artist: Some(String::new()),
            ..TrackMetadata::default()
        };
        let track = Track::local("/m/x.mp3".into(), meta);
        assert_eq!(track.title, "x");
        assert_eq!(track.artist, None);
    }

    #[test]
    fn oversized_metadata_is_capped_on_a_char_boundary() {
        let huge = format!("a{}", "\u{301}".repeat(100_000));
        let meta = TrackMetadata {
            title: Some(huge.clone()),
            artist: Some("雨".repeat(1000)),
            album: Some("x".repeat(MAX_METADATA_CHARS)),
            ..TrackMetadata::default()
        };
        let track = Track::local("/m/x.mp3".into(), meta);
        assert_eq!(track.title.chars().count(), MAX_METADATA_CHARS);
        assert!(huge.starts_with(&track.title));
        assert_eq!(track.artist.unwrap(), "雨".repeat(MAX_METADATA_CHARS));
        assert_eq!(
            track.album.unwrap().len(),
            MAX_METADATA_CHARS,
            "at the cap: untouched"
        );
        // The file-stem fallback is capped too.
        let stem = "é".repeat(600);
        let track = Track::local(format!("/m/{stem}.mp3").into(), TrackMetadata::default());
        assert_eq!(track.title, "é".repeat(MAX_METADATA_CHARS));
    }

    #[test]
    fn navidrome_track_falls_back_to_unknown_title() {
        let track = Track::navidrome("s1".into(), TrackMetadata::default());
        assert_eq!(track.title, UNKNOWN_TITLE);
        assert_eq!(track.local_path(), None);
        assert!(track.is_remote());
        assert_eq!(
            track.source,
            TrackSource::Navidrome {
                song_id: "s1".into()
            }
        );
        let meta = TrackMetadata {
            title: Some("x".repeat(10_000)),
            artist: Some("  ".into()),
            ..TrackMetadata::default()
        };
        let track = Track::navidrome("s2".into(), meta);
        assert_eq!(track.title.chars().count(), MAX_METADATA_CHARS);
        assert_eq!(track.artist, None);
        assert!(!Track::local("/m/x.mp3".into(), TrackMetadata::default()).is_remote());
    }

    #[test]
    fn genres_are_split_on_navidromes_separators() {
        assert_eq!(
            split_genres(["Rock;Progressive Rock", "Jazz/Fusion", "Pop, Synthpop"]),
            [
                "Rock",
                "Progressive Rock",
                "Jazz",
                "Fusion",
                "Pop",
                "Synthpop"
            ]
        );
        // Other punctuation is part of the name.
        assert_eq!(
            split_genres(["Drum & Bass", "R&B", "Hip-Hop"]),
            ["Drum & Bass", "R&B", "Hip-Hop"]
        );
        assert!(split_genres(Vec::<String>::new()).is_empty());
    }

    #[test]
    fn genre_duplicates_and_blanks_are_dropped_keeping_the_first_spelling() {
        assert_eq!(
            split_genres([
                "rock",
                "Rock; ROCK ",
                " ;; / ,",
                "",
                "Jazz",
                "jazz;Ärger",
                "ärger"
            ]),
            ["rock", "Jazz", "Ärger"]
        );
    }

    #[test]
    fn genres_are_capped_in_count_and_length() {
        let many: Vec<String> = (0..100).map(|i| format!("G{i}")).collect();
        let genres = split_genres([many.join(";")]);
        assert_eq!(genres.len(), MAX_GENRES);
        assert_eq!(genres[0], "G0");
        assert_eq!(genres[MAX_GENRES - 1], format!("G{}", MAX_GENRES - 1));
        // One huge value, and a hostile run of separators and duplicates.
        let huge = "é".repeat(100_000);
        assert_eq!(
            split_genres([huge.as_str()]),
            ["é".repeat(MAX_METADATA_CHARS)]
        );
        let noise = "a;".repeat(200_000) + &"/".repeat(100_000);
        assert_eq!(split_genres([noise]), ["a"]);
        // Control characters and odd whitespace are kept as text, trimmed.
        assert_eq!(split_genres(["\tBlues\n"]), ["Blues"]);
    }

    #[test]
    fn track_constructors_clean_genres_without_splitting() {
        let meta = |genres: &[&str]| TrackMetadata {
            genres: genres.iter().map(|g| (*g).to_owned()).collect(),
            ..TrackMetadata::default()
        };
        let local = Track::local(
            "/m/x.flac".into(),
            meta(&[" Rock ", "rock", "", "AC/DC Tribute"]),
        );
        assert_eq!(local.genres, ["Rock", "AC/DC Tribute"]);
        let many: Vec<String> = (0..40).map(|i| format!("g{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let remote = Track::navidrome("s".into(), meta(&refs));
        assert_eq!(remote.genres.len(), MAX_GENRES);
        assert!(
            Track::local("/m/y.flac".into(), TrackMetadata::default())
                .genres
                .is_empty()
        );
    }

    #[test]
    fn display_name_includes_artist() {
        let meta = TrackMetadata {
            title: Some("Song".into()),
            artist: Some("Band".into()),
            ..TrackMetadata::default()
        };
        assert_eq!(
            Track::local("/m/x.mp3".into(), meta).display_name(),
            "Band - Song"
        );
    }
}
