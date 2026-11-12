//! A local file's lyrics: from its tags, else from a sidecar `.lrc` file
//! next to it. Blocking (call it through `spawn_blocking`); files are
//! untrusted input.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::TaggedFile;
use lofty::prelude::TaggedFileExt;
use lofty::probe::Probe;
use lofty::tag::ItemKey;

use super::{Lyrics, MAX_LYRICS_BYTES, cap_text, parse};

/// Upper bound on the lyrics values read from one file's tags.
const MAX_TAG_VALUES: usize = 16;

/// Why a local file's lyrics could not be read. Never contains the path.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The audio file itself could not be read (missing, unreadable).
    #[error("could not read the file: {0}")]
    Io(#[from] std::io::Error),
}

/// The lyrics of the audio file at `path`, or `None` when it has none.
///
/// The tags are read first (lofty's `Lyrics` and `UnsyncLyrics` items:
/// the Vorbis `LYRICS` and `UNSYNCEDLYRICS` comments, MP4 `©lyr`, ID3
/// `USLT`, APE `Lyrics`); of several values, the first timed one wins,
/// else the first with text. Only when the tags have none is a sidecar
/// `<stem>.lrc` next to the file read. Each value and the sidecar are
/// read up to [`MAX_LYRICS_BYTES`]. Tags that cannot be parsed (or a
/// parser panic) count as no lyrics; only an audio file that cannot be
/// read at all is an error. Blocking.
pub fn read_local(path: &Path) -> Result<Option<Lyrics>, Error> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a file",
        )));
    }
    let from_tags = std::panic::catch_unwind(|| tag_lyrics(path)).unwrap_or_else(|_| {
        tracing::warn!("tag parser panicked while reading lyrics");
        None
    });
    if from_tags.is_some() {
        return Ok(from_tags);
    }
    Ok(read_sidecar(path))
}

/// The best lyrics in the file's tags; see [`read_local`].
fn tag_lyrics(path: &Path) -> Option<Lyrics> {
    let tagged = Probe::open(path)
        .and_then(|probe| {
            probe
                .options(
                    ParseOptions::new()
                        .read_properties(false)
                        .read_cover_art(false),
                )
                .guess_file_type()
                .map_err(Into::into)
        })
        .and_then(|probe| probe.read());
    let tagged = match tagged {
        Ok(tagged) => tagged,
        Err(error) => {
            tracing::debug!(%error, "could not read tags for lyrics");
            return None;
        }
    };
    from_tagged(&tagged)
}

/// The best lyrics in an already read file's tags (see [`read_local`]):
/// the scan reads them from the tags it parses anyway.
pub(crate) fn from_tagged(tagged: &TaggedFile) -> Option<Lyrics> {
    let primary = tagged.primary_tag();
    let tags = primary.into_iter().chain(
        tagged
            .tags()
            .iter()
            .filter(|tag| primary.is_none_or(|p| p.tag_type() != tag.tag_type())),
    );
    let values = tags
        .flat_map(|tag| {
            tag.get_strings(ItemKey::Lyrics)
                .chain(tag.get_strings(ItemKey::UnsyncLyrics))
        })
        .take(MAX_TAG_VALUES);
    let mut first_with_text = None;
    for value in values {
        let lyrics = parse(value);
        if lyrics.is_blank() {
            continue;
        }
        if lyrics.timed {
            return Some(lyrics);
        }
        first_with_text.get_or_insert(lyrics);
    }
    first_with_text
}

/// The lyrics in `<stem>.lrc` next to `path`, if that is a readable
/// regular file (symlinks followed) with text. Read up to
/// [`MAX_LYRICS_BYTES`]; invalid UTF-8 is replaced. Anything else (a FIFO,
/// a device, a directory) is never opened for reading, so it cannot block;
/// the open is non-blocking as well, against a swap after the check.
pub(crate) fn read_sidecar(path: &Path) -> Option<Lyrics> {
    let sidecar = path.with_extension("lrc");
    if sidecar == path {
        return None;
    }
    match fs::metadata(&sidecar) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            tracing::debug!("lyrics sidecar is not a regular file; ignored");
            return None;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::debug!(%error, "could not inspect the lyrics sidecar");
            return None;
        }
    }
    let file = match open_nonblocking(&sidecar) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::debug!(%error, "could not open the lyrics sidecar");
            return None;
        }
    };
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        tracing::debug!("lyrics sidecar is not a regular file; ignored");
        return None;
    }
    let mut bytes = Vec::new();
    if let Err(error) = file.take(MAX_LYRICS_BYTES as u64).read_to_end(&mut bytes) {
        tracing::debug!(%error, "could not read the lyrics sidecar");
        return None;
    }
    let text = String::from_utf8_lossy(&bytes);
    Some(parse(cap_text(&text))).filter(|lyrics| !lyrics.is_blank())
}

/// Opens `path` for reading with `O_NONBLOCK`, so a FIFO swapped in after
/// the type check makes the open return instead of waiting for a writer.
/// Reads from a regular file are unaffected by the flag.
pub(super) fn open_nonblocking(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}
