//! Lyrics search: an in-memory index of lyrics text, searched by
//! substring on a folded form (see [`fold`]).
//!
//! Each entry keeps its lyrics twice: the folded text that searches run
//! over, and the original lines (trimmed, joined by `\n`) that snippets
//! are cut from, by folding those lines again for the few results shown.
//! Memory is about twice the lyrics text; tracks without lyrics take no
//! space. Timestamps and word stamps are never kept (the
//! text comes from a parsed [`Lyrics`]).
//!
//! A search is linear over every entry's folded text. With 50,000
//! entries of about 1.5 KB each (76 MB of lyrics), a search took 30 ms
//! when nothing matched and at most 40 ms in all, in a release build on
//! an x86-64 Linux desktop; the index held 148 MB (1.95 times the text)
//! and took 0.76 s to build (the `#[ignore]`d test in `tests.rs`). So it
//! runs on a blocking thread per search, without a token index.

use std::cmp::Ordering;
use std::ops::Range;

use super::Lyrics;

/// Shortest query searched, in chars after folding: shorter ones give no
/// results and do no work.
pub const MIN_QUERY_CHARS: usize = 3;
/// Results a search returns by default; the total is still counted.
pub const DEFAULT_LIMIT: usize = 50;
/// Between two lines of a snippet when the match spans several.
pub const SNIPPET_SEPARATOR: &str = " / ";

/// One track's lyrics, ready to search. Build it with
/// [`IndexedLyrics::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedLyrics {
    /// Every non-blank line folded, joined by one space.
    folded: Box<str>,
    /// Every non-blank line trimmed, joined by `\n`.
    text: Box<str>,
}

impl IndexedLyrics {
    /// `None` for lyrics with no text (no lines, or only pauses and blank
    /// lines). Each line is trimmed (and split at any line break inside
    /// it); blank ones are left out.
    pub fn new(lyrics: &Lyrics) -> Option<Self> {
        let mut folded = String::new();
        let mut text = String::new();
        let lines = lyrics
            .lines
            .iter()
            .flat_map(|line| line.text.lines())
            .map(str::trim)
            .filter(|line| !line.is_empty());
        for line in lines {
            if !text.is_empty() {
                folded.push(' ');
                text.push('\n');
            }
            fold_into(line, &mut folded, |_, _| {});
            text.push_str(line);
        }
        if text.is_empty() {
            return None;
        }
        Some(Self {
            folded: folded.into_boxed_str(),
            text: text.into_boxed_str(),
        })
    }

    /// Bytes held on the heap (for the memory figure).
    pub fn heap_bytes(&self) -> usize {
        self.folded.len() + self.text.len()
    }

    /// The snippet for the match at `range` of `folded`. The lines are
    /// folded again, one by one, to find the ones the match spans (cheap:
    /// only for the results shown).
    fn snippet(&self, range: Range<usize>) -> Snippet {
        let mut text = String::new();
        let mut highlights = Vec::new();
        let mut exact = true;
        let mut chars_before = 0;
        let mut scratch = String::new();
        // Where the current line starts in `folded`.
        let mut start = 0;
        for line in self.text.split('\n') {
            scratch.clear();
            fold_into(line, &mut scratch, |_, _| {});
            let line_range = start..start + scratch.len();
            start = line_range.end + 1;
            if line_range.end <= range.start {
                continue;
            }
            if line_range.start >= range.end {
                break;
            }
            if !text.is_empty() {
                text.push_str(SNIPPET_SEPARATOR);
                chars_before += SNIPPET_SEPARATOR.chars().count();
            }
            // The part of the match inside this line, in the line's own
            // folded text.
            let from = range.start.max(line_range.start) - line_range.start;
            let to = range.end.min(line_range.end) - line_range.start;
            match map_back(line, from..to) {
                Some(chars) => {
                    highlights.push(chars_before + chars.start..chars_before + chars.end)
                }
                None => exact = false,
            }
            text.push_str(line);
            chars_before += line.chars().count();
        }
        if !exact {
            highlights.clear();
        }
        Snippet { text, highlights }
    }
}

/// The lyrics of one track, as shown in a result: the line (or lines)
/// holding the match, joined by [`SNIPPET_SEPARATOR`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    pub text: String,
    /// The matched part, as char ranges into `text` (one per line, in
    /// order). Empty when the match could not be mapped back exactly
    /// (a case mapping that changes length split by the match).
    pub highlights: Vec<Range<usize>>,
}

/// One lyric match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricsMatch<K> {
    pub key: K,
    pub snippet: Snippet,
}

/// The outcome of one lyrics search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricsResults<K> {
    /// The first matches in the caller's order, at most the limit.
    pub matches: Vec<LyricsMatch<K>>,
    /// Every matching track not excluded, including those past the limit.
    pub total: usize,
}

impl<K> Default for LyricsResults<K> {
    fn default() -> Self {
        Self {
            matches: Vec::new(),
            total: 0,
        }
    }
}

/// Lyrics by track key. Plain data: fill it, then search it.
#[derive(Debug, Clone)]
pub struct LyricsIndex<K> {
    entries: Vec<(K, IndexedLyrics)>,
}

impl<K> Default for LyricsIndex<K> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<K: Clone> LyricsIndex<K> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `key`'s lyrics. A key added twice is searched twice; callers
    /// add each track once.
    pub fn insert(&mut self, key: K, lyrics: IndexedLyrics) {
        self.entries.push((key, lyrics));
    }

    /// Tracks with lyrics.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bytes held on the heap by the entries (lyrics, line starts and the
    /// entry list; not what the keys own).
    pub fn heap_bytes(&self) -> usize {
        self.entries.capacity() * size_of::<(K, IndexedLyrics)>()
            + self
                .entries
                .iter()
                .map(|(_, l)| l.heap_bytes())
                .sum::<usize>()
    }

    /// Searches for `query` (folded; see [`fold`]), skipping the keys
    /// `skip` accepts: one result per track at its first match, the first
    /// `limit` in the order `order` gives (ties in insertion order), and
    /// the total (skipped keys not counted). No results and no work for a
    /// query shorter than [`MIN_QUERY_CHARS`].
    pub fn search(
        &self,
        query: &str,
        skip: impl Fn(&K) -> bool,
        limit: usize,
        order: impl Fn(&K, &K) -> Ordering,
    ) -> LyricsResults<K> {
        let needle = fold(query);
        if needle.chars().count() < MIN_QUERY_CHARS {
            return LyricsResults::default();
        }
        let mut found: Vec<(usize, usize)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, (key, lyrics))| {
                let at = lyrics.folded.find(&needle)?;
                (!skip(key)).then_some((i, at))
            })
            .collect();
        let total = found.len();
        found.sort_by(|a, b| order(&self.entries[a.0].0, &self.entries[b.0].0));
        let matches = found
            .into_iter()
            .take(limit)
            .map(|(i, at)| {
                let (key, lyrics) = &self.entries[i];
                LyricsMatch {
                    key: key.clone(),
                    snippet: lyrics.snippet(at..at + needle.len()),
                }
            })
            .collect();
        LyricsResults { matches, total }
    }
}

/// The folded form lyrics and queries are matched in: lowercase; `’ ‘ ʼ`
/// as `'`, `“ ”` as `"` and `– —` as `-`; every run of whitespace (line
/// breaks included) as one space; trimmed. No diacritic stripping.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    fold_into(text, &mut out, |_, _| {});
    out
}

/// Appends `text` folded to `out` (which does not end in a space).
/// `each` gets every char's index in `text` and the range its output took
/// in `out` (empty for collapsed whitespace). Trailing whitespace adds
/// nothing.
fn fold_into(text: &str, out: &mut String, mut each: impl FnMut(usize, Range<usize>)) {
    let mut space = false;
    for (n, c) in text.chars().enumerate() {
        let start = out.len();
        if c.is_whitespace() {
            space = !out.is_empty();
        } else {
            if space {
                // Attributed to no char: a trimmed query never starts or
                // ends on it.
                out.push(' ');
                space = false;
            }
            let start = out.len();
            match c {
                '\u{2019}' | '\u{2018}' | '\u{02BC}' => out.push('\''),
                '\u{201C}' | '\u{201D}' => out.push('"'),
                '\u{2013}' | '\u{2014}' => out.push('-'),
                _ => out.extend(c.to_lowercase()),
            }
            each(n, start..out.len());
            continue;
        }
        each(n, start..start);
    }
}

/// The char range in `line` whose folded form is `range` of the line's
/// own folded text, if `range` starts and ends on the output of whole
/// chars.
fn map_back(line: &str, range: Range<usize>) -> Option<Range<usize>> {
    let mut start = None;
    let mut end = None;
    let mut out = String::with_capacity(line.len());
    fold_into(line, &mut out, |n, produced| {
        if produced.is_empty() {
            return;
        }
        if produced.start == range.start && start.is_none() {
            start = Some(n);
        }
        if produced.end == range.end {
            end = Some(n + 1);
        }
    });
    let (start, end) = (start?, end?);
    (start < end).then_some(start..end)
}

#[cfg(test)]
mod tests;
