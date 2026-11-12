//! Song lyrics: the [`Lyrics`] model, an LRC parser ([`parse`]) and the
//! reader for a local file's lyrics ([`read_local`]). Navidrome lyrics are
//! fetched by [`crate::navidrome::Client::lyrics`]. [`index`] searches
//! lyrics.
//!
//! Lyrics come from tags, sidecar files and servers, so they are
//! untrusted: the text is capped at [`MAX_LYRICS_BYTES`], the lines at
//! [`MAX_LINES`] and each line at [`MAX_LINE_CHARS`], and no input makes
//! the parser panic.

pub mod index;
mod local;

use std::time::Duration;

pub use local::{Error, read_local};
pub(crate) use local::{from_tagged, read_sidecar};

/// Upper bound on the lyrics text read or parsed: longer text is cut (on
/// a character boundary) before parsing.
pub const MAX_LYRICS_BYTES: usize = 256 * 1024;
/// Upper bound on the lines kept; later lines are dropped.
pub const MAX_LINES: usize = 5_000;
/// Upper bound on one line's text, in characters; longer lines are cut.
pub const MAX_LINE_CHARS: usize = 500;

/// A song's lyrics.
///
/// When `timed` is true, every line has a `start` and the lines are
/// sorted by it; otherwise no line has one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lyrics {
    pub lines: Vec<Line>,
    pub timed: bool,
}

/// One line of lyrics. A timed line with empty text is a pause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// When the line starts, for timed lyrics.
    pub start: Option<Duration>,
    pub text: String,
}

impl Lyrics {
    /// `true` when no line has any text (no lines, or only pauses):
    /// nothing worth showing.
    pub fn is_blank(&self) -> bool {
        self.lines.iter().all(|line| line.text.is_empty())
    }

    /// Timed lyrics from `(start, text)` pairs in any order: sorted by
    /// start (stable, so lines with the same start keep their order), at
    /// most [`MAX_LINES`], each text trimmed and capped.
    pub(crate) fn timed(mut lines: Vec<(Duration, String)>) -> Self {
        lines.sort_by_key(|(start, _)| *start);
        lines.truncate(MAX_LINES);
        Self {
            lines: lines
                .into_iter()
                .map(|(start, text)| Line {
                    start: Some(start),
                    text: clean_line(&text),
                })
                .collect(),
            timed: true,
        }
    }

    /// Untimed lyrics from lines in order: each trimmed and capped, blank
    /// lines at the start and end dropped, at most [`MAX_LINES`].
    pub(crate) fn untimed<S: AsRef<str>>(lines: impl IntoIterator<Item = S>) -> Self {
        let mut lines: Vec<Line> = lines
            .into_iter()
            .map(|text| Line {
                start: None,
                text: clean_line(text.as_ref()),
            })
            .skip_while(|line| line.text.is_empty())
            .take(MAX_LINES)
            .collect();
        while lines.last().is_some_and(|line| line.text.is_empty()) {
            lines.pop();
        }
        Self {
            lines,
            timed: false,
        }
    }
}

/// Trims a line and caps it at [`MAX_LINE_CHARS`].
fn clean_line(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(MAX_LINE_CHARS) {
        Some((end, _)) => text[..end].trim_end().to_owned(),
        None => text.to_owned(),
    }
}

/// `text` cut to at most [`MAX_LYRICS_BYTES`], on a character boundary.
pub(crate) fn cap_text(text: &str) -> &str {
    if text.len() <= MAX_LYRICS_BYTES {
        return text;
    }
    let mut end = MAX_LYRICS_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// LRC ID tags that are dropped (`offset` is applied instead).
const ID_TAGS: [&str; 12] = [
    "ar", "ti", "al", "au", "by", "length", "offset", "re", "tool", "ve", "la", "#",
];

/// At most this many timestamps are read from one LRC line (real files use
/// a handful); later ones on the line are ignored.
const MAX_STAMPS_PER_LINE: usize = 100;

/// Parses lyrics text: LRC when it has at least as many timestamped lines
/// as other non-blank text lines (ID tags not counted), otherwise plain
/// text with one line per text line, timestamps and all (so one stray
/// timestamp does not reduce plain lyrics to a single line).
///
/// LRC timestamps are `[mm:ss]`, `[mm:ss.x]`, `[mm:ss.xx]`,
/// `[mm:ss.xxx]` or `[mm:ss:xx]`; a line may carry several (one line per
/// timestamp, at most 100 per line). `[offset:±ms]` shifts every start (positive is earlier);
/// other ID tags (`[ar:]`, `[ti:]`, …) are dropped, and so are untimed
/// lines in LRC text. Enhanced-LRC word stamps (`<mm:ss.xx>`) are
/// stripped. Timed lines with empty text are kept as pauses. A leading BOM
/// and CRLF line ends are accepted. Text past [`MAX_LYRICS_BYTES`] is
/// ignored. Never panics.
pub fn parse(text: &str) -> Lyrics {
    let text = cap_text(text);
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut offset_ms: i64 = 0;
    // Starts point into `texts`, so each line's text is stored once while
    // parsing; the lines kept are copied at the end (a line repeats at most
    // `MAX_STAMPS_PER_LINE` times).
    let mut timed: Vec<(Duration, usize)> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    // Every line but ID tags, for plain text.
    let mut plain: Vec<&str> = Vec::new();
    let mut untimed_text_lines = 0usize;
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let (stamps, rest, id_tag) = leading_tags(line, &mut offset_ms);
        if stamps.is_empty() {
            // ID tag lines are metadata in LRC and in plain text alike.
            if !id_tag {
                plain.push(line);
                if !line.trim().is_empty() {
                    untimed_text_lines += 1;
                }
            }
            continue;
        }
        plain.push(line);
        let index = texts.len();
        texts.push(clean_line(&strip_word_stamps(rest)));
        timed.extend(stamps.into_iter().map(|start| (start, index)));
    }
    if texts.is_empty() || texts.len() < untimed_text_lines {
        return Lyrics::untimed(plain);
    }
    if offset_ms != 0 {
        let shift = Duration::from_millis(offset_ms.unsigned_abs());
        for (start, _) in &mut timed {
            *start = if offset_ms > 0 {
                start.saturating_sub(shift)
            } else {
                start.saturating_add(shift)
            };
        }
    }
    timed.sort_by_key(|(start, _)| *start);
    timed.truncate(MAX_LINES);
    Lyrics::timed(
        timed
            .into_iter()
            .map(|(start, index)| (start, texts[index].clone()))
            .collect(),
    )
}

/// Reads the `[…]` tags at the start of `line`: the timestamps, the text
/// after the tags, and whether a known ID tag was among them. An
/// `[offset:]` tag sets `offset_ms`. Stops at the first `[…]` that is
/// neither (so `[Chorus]` stays text).
fn leading_tags<'a>(line: &'a str, offset_ms: &mut i64) -> (Vec<Duration>, &'a str, bool) {
    let mut stamps = Vec::new();
    let mut id_tag = false;
    let mut rest = line.trim_start();
    while let Some(inner) = rest.strip_prefix('[') {
        let Some(close) = inner.find(']') else { break };
        let tag = &inner[..close];
        if let Some(start) = timestamp(tag) {
            if stamps.len() < MAX_STAMPS_PER_LINE {
                stamps.push(start);
            }
        } else if let Some((key, value)) = tag.split_once(':')
            && ID_TAGS.iter().any(|k| k.eq_ignore_ascii_case(key.trim()))
        {
            id_tag = true;
            if key.trim().eq_ignore_ascii_case("offset")
                && let Some(ms) = offset(value)
            {
                *offset_ms = ms;
            }
        } else {
            break;
        }
        rest = inner[close + 1..].trim_start();
    }
    (stamps, rest, id_tag)
}

/// An `[offset:]` value: optionally signed milliseconds.
fn offset(value: &str) -> Option<i64> {
    let value = value.trim();
    let value = value.strip_prefix('+').unwrap_or(value);
    value.parse().ok()
}

/// `mm:ss`, then optionally `.` or `:` and one to three fraction digits.
/// Minutes have one to five digits, seconds one or two (below 60).
fn timestamp(tag: &str) -> Option<Duration> {
    let (minutes, rest) = tag.trim().split_once(':')?;
    let (seconds, fraction) = match rest.find(['.', ':']) {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    let minutes = digits(minutes, 1..=5)?;
    let seconds = digits(seconds, 1..=2)?;
    if seconds >= 60 {
        return None;
    }
    let millis = match fraction {
        None => 0,
        Some(fraction) => {
            let value = digits(fraction, 1..=3)?;
            value * 10u64.pow(3 - fraction.len() as u32)
        }
    };
    Some(Duration::from_millis(
        (minutes * 60 + seconds) * 1000 + millis,
    ))
}

/// An ASCII decimal number with a digit count in `len`.
fn digits(text: &str, len: std::ops::RangeInclusive<usize>) -> Option<u64> {
    (len.contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// `text` without enhanced-LRC word stamps (`<mm:ss.xx>`); other `<…>`
/// stays. Linear: each `>` is matched with the last `<` before it, and a
/// `<` with no `>` after it ends the search.
fn strip_word_stamps(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let Some(close) = rest[open..].find('>').map(|i| open + i) else {
            break;
        };
        // The last `<` before this `>` starts the candidate stamp.
        let start = open + rest[open..close].rfind('<').unwrap_or(0);
        out.push_str(&rest[..start]);
        if timestamp(&rest[start + 1..close]).is_none() {
            out.push_str(&rest[start..=close]);
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    // Word stamps often sit between words with spaces on both sides.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests;
