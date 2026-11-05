//! `.m3u` / `.m3u8` playlists in the music folder: a pure parser
//! ([`parse`]), a pure entry resolver ([`resolve`]) and a bounded reader
//! ([`read_playlist`]) that the scan runs on its blocking thread.
//!
//! Playlists are untrusted input:
//! - At most [`MAX_PLAYLIST_BYTES`] are read and [`MAX_PLAYLIST_ENTRIES`]
//!   entries kept; a longer file is cut (and says so).
//! - Text is decoded as UTF-8, lossily if it is not valid (a Latin-1
//!   `.m3u` then keeps its ASCII entries; the others cannot match).
//! - Entries are only ever *compared* with scanned tracks by normalised
//!   path. Nothing in a playlist is opened, executed or followed, so an
//!   entry can only ever select a track the scanner found inside the music
//!   folder.
//! - Entry paths are never logged.

use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use crate::model::clean_text;

/// Bytes read from one playlist file; the rest is ignored.
pub const MAX_PLAYLIST_BYTES: usize = 1024 * 1024;
/// Entries kept from one playlist file; the rest are ignored.
pub const MAX_PLAYLIST_ENTRIES: usize = 10_000;
/// Name of a playlist whose file stem is blank.
pub const UNTITLED_PLAYLIST: &str = "Untitled playlist";

/// The entries of a playlist file, in file order, before resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// Entry lines, trimmed, in file order; duplicates kept.
    pub entries: Vec<String>,
    /// The file was longer than [`MAX_PLAYLIST_BYTES`] or had more than
    /// [`MAX_PLAYLIST_ENTRIES`] entries; only the start was kept.
    pub cut: bool,
}

/// Parses playlist text. Blank lines and `#` lines (`#EXTM3U`, `#EXTINF`
/// titles, comments) are skipped, a leading BOM is ignored, and LF, CRLF
/// and CR line endings are accepted. If `bytes` is longer than
/// [`MAX_PLAYLIST_BYTES`], only that much is used, minus the line it cuts
/// through.
pub fn parse(bytes: &[u8]) -> Parsed {
    let mut cut = false;
    let mut bytes = bytes;
    if bytes.len() > MAX_PLAYLIST_BYTES {
        cut = true;
        bytes = &bytes[..MAX_PLAYLIST_BYTES];
        // Drop the partial last line (it may also end mid-character).
        let end = bytes
            .iter()
            .rposition(|b| matches!(b, b'\n' | b'\r'))
            .unwrap_or(0);
        bytes = &bytes[..end];
    }
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut entries = Vec::new();
    for line in text.split(['\n', '\r']) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if entries.len() == MAX_PLAYLIST_ENTRIES {
            cut = true;
            break;
        }
        entries.push(line.to_owned());
    }
    Parsed { entries, cut }
}

/// What a playlist entry refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A normalised absolute path inside the music folder. It still has to
    /// match a scanned track.
    Path(PathBuf),
    /// A URL (`scheme://…` or `file:…`); skipped.
    Url,
    /// Resolves outside the music folder, to the folder itself, or is a
    /// Windows drive path (`C:\…`).
    Outside,
    /// A `..` after a folder name (`a/../b`), or an absolute path with
    /// `..`: lexical normalisation could disagree with the file system
    /// when a folder is a symlink, so these are not resolved.
    Unsupported,
}

/// Resolves `entry` from a playlist in `dir` against the music folder
/// `root`, without touching the file system.
///
/// `root` must be canonical and `dir` a folder the scanner walked below it
/// (so neither contains a symlink, `.` or `..`). A relative entry is joined
/// to `dir`; it may start with `..` components, which then step out of
/// real folders and are resolved exactly. An absolute entry must lie below
/// `root` as spelled (the canonical music folder path).
pub fn resolve(root: &Path, dir: &Path, entry: &str) -> Entry {
    if has_scheme(entry) {
        return Entry::Url;
    }
    // Windows playlists (foobar2000, Winamp) separate with `\`; mapped
    // before resolving, so the `..` rules below apply to them too.
    let entry = entry.replace('\\', "/");
    if is_drive_path(&entry) {
        return Entry::Outside;
    }
    let entry = Path::new(&entry);
    let mut path = if entry.is_absolute() {
        PathBuf::from("/")
    } else {
        dir.to_path_buf()
    };
    // Set once the entry has named a folder or file of its own.
    let mut named = false;
    for component in entry.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Prefix(_) => return Entry::Unsupported,
            Component::ParentDir => {
                if named || entry.is_absolute() {
                    return Entry::Unsupported;
                }
                if !path.pop() {
                    return Entry::Outside;
                }
            }
            Component::Normal(part) => {
                named = true;
                path.push(part);
            }
        }
    }
    if path != root && path.starts_with(root) {
        Entry::Path(path)
    } else {
        Entry::Outside
    }
}

/// `true` for a URL: a scheme (`[A-Za-z][A-Za-z0-9+.-]*`) followed by
/// `://`, or a `file:` URL. A bare `name:` is a file name (`AC:DC/x.mp3`,
/// `Live:2001/01.mp3`), not a URL.
fn has_scheme(entry: &str) -> bool {
    let Some((scheme, rest)) = entry.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    let is_scheme = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
    is_scheme && (rest.starts_with("//") || scheme.eq_ignore_ascii_case("file"))
}

/// `true` for a Windows drive path (`C:/…`, after `\` was mapped to `/`,
/// or a bare `C:`): it can never be inside the music folder.
fn is_drive_path(entry: &str) -> bool {
    let bytes = entry.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'/')
}

/// A playlist file read and resolved against the music folder; its
/// entries still have to be matched with scanned tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistFile {
    /// The playlist file (inside the music folder). Never logged.
    pub path: PathBuf,
    /// The file stem, capped like tags.
    pub name: String,
    /// Normalised paths inside the music folder, in file order.
    pub entries: Vec<PathBuf>,
    /// Entries that cannot match a track: URLs, paths outside the music
    /// folder and unsupported paths.
    pub unresolved: usize,
    /// Only the start of the file was used.
    pub cut: bool,
}

impl PlaylistFile {
    /// Parses `bytes`, the content of the playlist at `path` (inside
    /// `root`), and resolves its entries. Pure.
    pub fn from_bytes(root: &Path, path: PathBuf, bytes: &[u8]) -> Self {
        let parsed = parse(bytes);
        let dir = path.parent().unwrap_or(root);
        let mut entries = Vec::new();
        let mut unresolved = 0;
        for entry in &parsed.entries {
            match resolve(root, dir, entry) {
                Entry::Path(path) => entries.push(path),
                Entry::Url | Entry::Outside | Entry::Unsupported => unresolved += 1,
            }
        }
        let name = clean_text(
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned()),
        )
        .unwrap_or_else(|| UNTITLED_PLAYLIST.to_owned());
        Self {
            path,
            name,
            entries,
            unresolved,
            cut: parsed.cut,
        }
    }
}

/// Reads at most [`MAX_PLAYLIST_BYTES`] (plus one, to notice a longer
/// file) of the playlist at `path` and resolves it. Opened non-blocking
/// and checked on the open handle, so a FIFO swapped in after the scan
/// classified the file cannot hang the scan. Blocking.
pub fn read_playlist(root: &Path, path: PathBuf) -> io::Result<PlaylistFile> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(&path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_PLAYLIST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(PlaylistFile::from_bytes(root, path, &bytes))
}

/// `true` if the extension is `m3u` or `m3u8` (case-insensitive).
pub fn is_playlist_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("m3u") || ext.eq_ignore_ascii_case("m3u8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/music";
    const DIR: &str = "/music/lists";

    fn resolve_in(entry: &str) -> Entry {
        resolve(Path::new(ROOT), Path::new(DIR), entry)
    }

    fn path(p: &str) -> Entry {
        Entry::Path(PathBuf::from(p))
    }

    #[test]
    fn comments_extinf_blank_lines_and_bom_are_skipped() {
        let text = "\u{feff}#EXTM3U\n\n#EXTINF:123,Artist - Title\n  a.mp3  \n# comment\nb.mp3\n";
        assert_eq!(
            parse(text.as_bytes()),
            Parsed {
                entries: vec!["a.mp3".into(), "b.mp3".into()],
                cut: false
            }
        );
        // A BOM directly before an entry.
        assert_eq!(parse("\u{feff}x.mp3".as_bytes()).entries, ["x.mp3"]);
    }

    #[test]
    fn crlf_and_cr_line_endings() {
        assert_eq!(
            parse(b"#EXTM3U\r\na.mp3\r\nb.mp3\r\n").entries,
            ["a.mp3", "b.mp3"]
        );
        assert_eq!(parse(b"a.mp3\rb.mp3").entries, ["a.mp3", "b.mp3"]);
    }

    #[test]
    fn invalid_utf8_is_decoded_lossily() {
        // Latin-1 "Café.mp3" and a valid ASCII entry.
        let parsed = parse(b"Caf\xe9.mp3\nplain.mp3\n");
        assert_eq!(parsed.entries, ["Caf\u{fffd}.mp3", "plain.mp3"]);
        assert!(!parsed.cut);
        // Valid UTF-8 (as in `.m3u8`) is kept exactly.
        assert_eq!(parse("Ünïcode.flac".as_bytes()).entries, ["Ünïcode.flac"]);
    }

    #[test]
    fn duplicates_are_kept_in_order() {
        assert_eq!(parse(b"a\nb\na\n").entries, ["a", "b", "a"]);
    }

    #[test]
    fn entries_are_capped() {
        let text: String = (0..MAX_PLAYLIST_ENTRIES + 5)
            .map(|i| format!("{i}.mp3\n"))
            .collect();
        let parsed = parse(text.as_bytes());
        assert_eq!(parsed.entries.len(), MAX_PLAYLIST_ENTRIES);
        assert_eq!(parsed.entries.last().unwrap(), "9999.mp3");
        assert!(parsed.cut);
        // Exactly at the cap: not cut; comments do not count.
        let text: String = (0..MAX_PLAYLIST_ENTRIES)
            .map(|i| format!("#EXTINF:1,x\n{i}.mp3\n"))
            .collect();
        let parsed = parse(text.as_bytes());
        assert_eq!(parsed.entries.len(), MAX_PLAYLIST_ENTRIES);
        assert!(!parsed.cut);
    }

    #[test]
    fn bytes_are_capped_on_a_line_boundary() {
        // Lines of 200 bytes (fewer than the entry cap fit); the cap
        // falls inside a line.
        let line = format!("{}\n", "x".repeat(199));
        let lines = MAX_PLAYLIST_BYTES / 200 + 3;
        let mut bytes = line.repeat(lines).into_bytes();
        // A multi-byte character straddling the cap must not leak through.
        bytes.splice(MAX_PLAYLIST_BYTES - 1..MAX_PLAYLIST_BYTES - 1, "é".bytes());
        let parsed = parse(&bytes);
        assert!(parsed.cut);
        assert_eq!(parsed.entries.len(), MAX_PLAYLIST_BYTES / 200);
        assert!(parsed.entries.iter().all(|e| e == &"x".repeat(199)));
        // Exactly at the cap: not cut.
        let exact = vec![b'a'; MAX_PLAYLIST_BYTES];
        let parsed = parse(&exact);
        assert!(!parsed.cut);
        assert_eq!(parsed.entries.len(), 1);
    }

    #[test]
    fn relative_entries_resolve_against_the_playlist_folder() {
        assert_eq!(resolve_in("a.mp3"), path("/music/lists/a.mp3"));
        assert_eq!(resolve_in("./sub/a.mp3"), path("/music/lists/sub/a.mp3"));
        assert_eq!(
            resolve_in("../Artist/Album/01 x.flac"),
            path("/music/Artist/Album/01 x.flac")
        );
        assert_eq!(resolve_in("sub//a.mp3"), path("/music/lists/sub/a.mp3"));
    }

    #[test]
    fn absolute_entries_must_be_inside_the_root() {
        assert_eq!(resolve_in("/music/a/b.mp3"), path("/music/a/b.mp3"));
        assert_eq!(resolve_in("/elsewhere/b.mp3"), Entry::Outside);
        assert_eq!(resolve_in("/musicx/b.mp3"), Entry::Outside);
        assert_eq!(resolve_in("/music"), Entry::Outside);
        assert_eq!(resolve_in("/"), Entry::Outside);
        // `..` in an absolute path is never resolved.
        assert_eq!(resolve_in("/music/a/../b.mp3"), Entry::Unsupported);
        assert_eq!(resolve_in("/music/../etc/passwd"), Entry::Unsupported);
    }

    #[test]
    fn entries_cannot_climb_out_of_the_root() {
        assert_eq!(resolve_in("../../etc/passwd"), Entry::Outside);
        assert_eq!(resolve_in("../.."), Entry::Outside);
        assert_eq!(resolve_in(".."), Entry::Outside);
        assert_eq!(
            resolve_in(&format!("{}etc/passwd", "../".repeat(50))),
            Entry::Outside
        );
        // Out and back in through the root's own name is still inside.
        assert_eq!(resolve_in("../../music/a.mp3"), path("/music/a.mp3"));
        // `..` after a folder name could pass through a symlinked folder.
        assert_eq!(resolve_in("linked/../a.mp3"), Entry::Unsupported);
        assert_eq!(resolve_in("../a/../../x.mp3"), Entry::Unsupported);
    }

    #[test]
    fn urls_are_skipped() {
        for url in [
            "http://example.com/stream.mp3",
            "https://example.com/a.mp3",
            "file:///music/a.mp3",
            "rtsp://host/x",
            "svn+ssh://x",
            "FILE:/music/a.mp3",
            "file:a.mp3",
        ] {
            assert_eq!(resolve_in(url), Entry::Url, "{url}");
        }
        // A name with a colon is a file name, not a scheme.
        assert_eq!(
            resolve_in("Live:2001/01.mp3"),
            path("/music/lists/Live:2001/01.mp3")
        );
        assert_eq!(resolve_in("AC:DC/x.mp3"), path("/music/lists/AC:DC/x.mp3"));
        assert_eq!(resolve_in("mailto:x"), path("/music/lists/mailto:x"));
        // Not a scheme: a colon after a space or a digit first.
        assert_eq!(
            resolve_in("Track 1: Intro.mp3"),
            path("/music/lists/Track 1: Intro.mp3")
        );
        assert_eq!(resolve_in("1:2.mp3"), path("/music/lists/1:2.mp3"));
    }

    #[test]
    fn windows_separators_and_drive_letters() {
        assert_eq!(
            resolve_in("..\\Artist\\Album\\01.flac"),
            path("/music/Artist/Album/01.flac")
        );
        assert_eq!(resolve_in("sub\\a.mp3"), path("/music/lists/sub/a.mp3"));
        // The `..` rules hold after mapping.
        assert_eq!(resolve_in("..\\..\\x.mp3"), Entry::Outside);
        assert_eq!(resolve_in("..\\..\\..\\etc\\passwd"), Entry::Outside);
        assert_eq!(resolve_in("a\\..\\b.mp3"), Entry::Unsupported);
        assert_eq!(resolve_in("\\music\\a\\..\\b.mp3"), Entry::Unsupported);
        assert_eq!(resolve_in("\\music\\a.mp3"), path("/music/a.mp3"));
        // Drive paths are never inside the music folder.
        for drive in ["C:\\Music\\a.mp3", "C:/Music/a.mp3", "c:", "D:\\"] {
            assert_eq!(resolve_in(drive), Entry::Outside, "{drive}");
        }
    }

    #[test]
    fn a_playlist_file_counts_unresolved_entries() {
        let text = "#EXTM3U\n\
                    a.mp3\n\
                    ../x/b.mp3\n\
                    /music/c.mp3\n\
                    /etc/passwd\n\
                    ../../outside.mp3\n\
                    http://radio.example/stream\n\
                    sub/../d.mp3\n\
                    a.mp3\n";
        let file = PlaylistFile::from_bytes(
            Path::new(ROOT),
            PathBuf::from("/music/lists/Road Trip.m3u8"),
            text.as_bytes(),
        );
        assert_eq!(file.name, "Road Trip");
        assert_eq!(
            file.entries,
            [
                "/music/lists/a.mp3",
                "/music/x/b.mp3",
                "/music/c.mp3",
                "/music/lists/a.mp3"
            ]
            .map(PathBuf::from)
        );
        assert_eq!(file.unresolved, 4);
        assert!(!file.cut);
    }

    #[test]
    fn playlist_extensions() {
        assert!(is_playlist_file(Path::new("/m/a.m3u")));
        assert!(is_playlist_file(Path::new("/m/a.M3U8")));
        assert!(!is_playlist_file(Path::new("/m/a.pls")));
        assert!(!is_playlist_file(Path::new("/m/m3u")));
    }

    #[test]
    fn reading_is_capped_and_skips_non_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let big = root.join("big.m3u");
        std::fs::write(&big, "a.mp3\n".repeat(MAX_PLAYLIST_BYTES)).unwrap();
        let file = read_playlist(&root, big).unwrap();
        assert!(file.cut);
        assert_eq!(file.entries.len(), MAX_PLAYLIST_ENTRIES);
        let fifo = root.join("fifo.m3u");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        assert!(read_playlist(&root, fifo).is_err());
    }
}
