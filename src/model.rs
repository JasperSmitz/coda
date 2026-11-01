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
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub duration: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub source: TrackSource,
    /// Tag title, or the file stem when the tag has none.
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub duration: Option<Duration>,
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
            track_no: metadata.track_no,
            disc_no: metadata.disc_no,
            duration: metadata.duration,
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
            track_no: metadata.track_no,
            disc_no: metadata.disc_no,
            duration: metadata.duration,
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
