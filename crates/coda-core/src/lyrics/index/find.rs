//! Finding a folded needle in text that is not folded: the result of
//! `fold(text).find(needle)` (see [`super::fold`]) without building the
//! folded text, so an index keeps only one copy of the lyrics.
//!
//! - **Candidates:** only the chars that fold to the needle's first char
//!   (`ж` and `Ж`; `i`, `I` and `İ`; `'` and the three curly forms; a
//!   reverse table of every char, built once) can start a match. A byte
//!   scan in 32-byte chunks the compiler vectorizes finds their ASCII
//!   bytes and the last bytes of their UTF-8 forms; a last byte is then
//!   confirmed against the whole char. Most of the text, in any script,
//!   is skipped this way.
//! - **Compare:** from a candidate, the text is folded char by char
//!   (ASCII without decoding) and compared to the needle until it differs.
//!   A char whose lowercase is several chars (`İ`) is tried at each of
//!   them.
//! - **Position:** the folded byte offset is counted only for the matches
//!   a caller shows ([`Found::folded_offset`]).
//! - **Worst case:** candidates and compares are budgeted per text; text
//!   that uses the budget (long runs of a repeated pattern) is folded and
//!   searched the simple way, so a search stays linear.

/// A needle, folded, ready to find.
#[derive(Debug, Clone)]
pub(crate) struct Needle {
    bytes: Box<[u8]>,
    /// Bytes a candidate's scan looks for (padded with 0xFF, never in
    /// UTF-8): the ASCII sources of the first char and the last bytes of
    /// its non-ASCII sources.
    probes: [u8; PROBES],
    /// How many probes there are.
    count: usize,
    /// When there are more than [`PROBES`] of those: all of them.
    table: Option<Box<[bool; 256]>>,
    /// The non-ASCII sources' UTF-8 forms.
    wide: Vec<([u8; 4], usize)>,
}

/// Probe bytes compared per text byte (vectorized).
const PROBES: usize = 8;

/// The bytes of `chunk` equal to one of the first `N` probes, as a bit
/// mask (bit `n` for byte `n`). A full chunk vectorizes.
#[inline]
fn probe_mask<const N: usize>(chunk: &[u8], probes: &[u8; PROBES]) -> u32 {
    let mut mask = 0;
    for (n, &b) in chunk.iter().enumerate() {
        let hit = probes[..N].iter().fold(false, |hit, &p| hit | (b == p));
        mask |= (hit as u32) << n;
    }
    mask
}

/// Tables built once from every char (a few ms, at the first search).
struct Tables {
    /// For each folded char, the chars whose folded output holds it at a
    /// char start, other than itself folding to itself: the reverse of
    /// folding.
    sources: std::collections::HashMap<char, Vec<char>>,
    /// One bit per char of the Basic Multilingual Plane: it folds to
    /// itself (not whitespace, not changed).
    stable: Box<[u64; 1024]>,
}

fn tables() -> &'static Tables {
    static TABLES: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    TABLES.get_or_init(|| {
        let mut sources: std::collections::HashMap<char, Vec<char>> =
            std::collections::HashMap::new();
        let mut stable = Box::new([0u64; 1024]);
        let mut buf = [0; 4];
        for source in '\0'..=char::MAX {
            let text = source.encode_utf8(&mut buf);
            let (Unit::Out(out, len), _) = unit(text, 0) else {
                continue;
            };
            let out = std::str::from_utf8(&out[..len as usize]).unwrap_or_default();
            if out == text {
                let cp = source as usize;
                if cp < 0x10000 {
                    stable[cp / 64] |= 1 << (cp % 64);
                }
                continue;
            }
            for folded in out.chars() {
                let list = sources.entry(folded).or_default();
                if !list.contains(&source) {
                    list.push(source);
                }
            }
        }
        Tables { sources, stable }
    })
}

/// `true` if the 2- or 3-byte char `bytes` folds to itself.
#[inline]
fn is_stable(tables: &Tables, bytes: &[u8]) -> bool {
    let cp = match *bytes {
        [a, b] => ((a as usize & 0x1F) << 6) | (b as usize & 0x3F),
        [a, b, c] => ((a as usize & 0x0F) << 12) | ((b as usize & 0x3F) << 6) | (c as usize & 0x3F),
        _ => return false,
    };
    tables.stable[cp / 64] & (1 << (cp % 64)) != 0
}

/// Every char whose folded output holds `c` at a char start, other than
/// `c` folding to itself.
fn sources_of(c: char) -> &'static [char] {
    tables().sources.get(&c).map_or(&[], Vec::as_slice)
}

/// Every char whose folded output holds `c` at a char start.
fn sources(c: char) -> Vec<char> {
    let mut all = sources_of(c).to_vec();
    let mut buf = [0; 4];
    if let (Unit::Out(out, len), _) = unit(c.encode_utf8(&mut buf), 0)
        && std::str::from_utf8(&out[..len as usize])
            .unwrap_or_default()
            .contains(c)
        && !all.contains(&c)
    {
        all.push(c);
    }
    all
}

/// Where a match starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Found {
    /// In the folded output of the char at byte `at` of the text, `skip`
    /// bytes into it.
    Raw { at: usize, skip: usize },
    /// At this byte offset of the folded text (the simple way found it).
    Folded(usize),
}

impl Found {
    /// The match's byte offset in `fold(text)`.
    pub(crate) fn folded_offset(self, text: &str) -> usize {
        match self {
            Self::Folded(at) => at,
            Self::Raw { at, skip } => folded_len_before(text, at) + skip,
        }
    }
}

/// Compares per text byte allowed before the simple way is used instead.
const BUDGET_PER_BYTE: usize = 4;
const BUDGET_BASE: usize = 4096;
/// Bytes scanned at once for candidates.
const CHUNK: usize = 32;

impl Needle {
    /// `needle` must be folded: non-empty, with no space at either end.
    pub(crate) fn new(needle: &str) -> Self {
        debug_assert!(!needle.is_empty() && !needle.starts_with(' ') && !needle.ends_with(' '));
        let bytes: Box<[u8]> = needle.as_bytes().into();
        let first = needle.chars().next().unwrap_or(' ');
        let mut probe_list = Vec::new();
        let mut wide = Vec::new();
        for source in sources(first) {
            let mut encoded = [0; 4];
            let len = source.encode_utf8(&mut encoded).len();
            let probe = encoded[len - 1];
            if len > 1 {
                wide.push((encoded, len));
            }
            if !probe_list.contains(&probe) {
                probe_list.push(probe);
            }
        }
        let mut probes = [0xFF; PROBES];
        let table = if probe_list.len() <= PROBES {
            probes[..probe_list.len()].copy_from_slice(&probe_list);
            None
        } else {
            let mut table = Box::new([false; 256]);
            for &b in &probe_list {
                table[b as usize] = true;
            }
            Some(table)
        };
        Self {
            bytes,
            probes,
            count: probe_list.len(),
            table,
            wide,
        }
    }

    /// The bytes of `chunk` (at most 32) that are probes, as a bit mask
    /// (bit `n` for byte `n`). A full chunk vectorizes.
    #[inline]
    fn candidates(&self, chunk: &[u8]) -> u32 {
        let mut mask = 0;
        if let Some(table) = &self.table {
            for (n, &b) in chunk.iter().enumerate() {
                mask |= (table[b as usize] as u32) << n;
            }
            return mask;
        }
        match self.count {
            0..=2 => probe_mask::<2>(chunk, &self.probes),
            3..=4 => probe_mask::<4>(chunk, &self.probes),
            _ => probe_mask::<PROBES>(chunk, &self.probes),
        }
    }

    /// The start of the source char whose last byte is at `end`, if the
    /// probe there is one (an ASCII probe is a char of its own).
    #[inline]
    fn source_at(&self, bytes: &[u8], end: usize) -> Option<usize> {
        if bytes[end].is_ascii() {
            return Some(end);
        }
        self.wide.iter().find_map(|(encoded, len)| {
            let start = (end + 1).checked_sub(*len)?;
            (bytes[start..=end] == encoded[..*len]).then_some(start)
        })
    }

    /// The first match in `text`, as `fold(text).find(needle)` finds it.
    pub(crate) fn find(&self, text: &str) -> Option<Found> {
        let bytes = text.as_bytes();
        let mut budget = BUDGET_BASE + BUDGET_PER_BYTE * bytes.len();
        // A whole source char ends at each confirmed probe, so its start
        // is a char start; starts come in order (chars never overlap).
        for (c, chunk) in bytes.chunks(CHUNK).enumerate() {
            let mut mask = self.candidates(chunk);
            while mask != 0 {
                let end = c * CHUNK + mask.trailing_zeros() as usize;
                mask &= mask - 1;
                budget = budget.saturating_sub(1);
                if let Some(at) = self.source_at(bytes, end)
                    && self.matches_at(text, at, &mut budget)
                {
                    return Some(Found::Raw {
                        at,
                        skip: self.skip_at(text, at),
                    });
                }
                if budget == 0 {
                    return self.find_simply(text);
                }
            }
        }
        None
    }

    /// Where in the folded output of the char at `at` a match starts
    /// (the first place it does), when [`Self::matches_at`] is true.
    fn skip_at(&self, text: &str, at: usize) -> usize {
        let (unit, next) = unit(text, at);
        let Unit::Out(out, len) = unit else { return 0 };
        let out = &out[..len as usize];
        let mut unlimited = usize::MAX;
        (0..out.len())
            .find(|&skip| {
                out[skip] & 0xC0 != 0x80
                    && out[skip] == self.bytes[0]
                    && matches_from(text, next, &out[skip..], &self.bytes, &mut unlimited)
            })
            .unwrap_or(0)
    }

    /// `true` if a match starts in the folded output of the char at `at`.
    #[inline]
    fn matches_at(&self, text: &str, at: usize, budget: &mut usize) -> bool {
        let needle = &self.bytes[..];
        let bytes = text.as_bytes();
        // An ASCII candidate folds to the needle's first byte.
        if bytes[at].is_ascii() {
            return matches_from(text, at + 1, &needle[..1], needle, budget);
        }
        // The needle's first char itself (already folded): it folds to
        // itself, and to nothing else.
        let len = utf8_len(bytes[at]);
        if bytes.get(at..at + len) == needle.get(..len) {
            return matches_from(text, at + len, &needle[..len], needle, budget);
        }
        let (unit, next) = unit(text, at);
        let Unit::Out(out, len) = unit else {
            return false;
        };
        let out = &out[..len as usize];
        (0..out.len()).any(|skip| {
            // Only char starts in the output.
            out[skip] & 0xC0 != 0x80
                && out[skip] == needle[0]
                && matches_from(text, next, &out[skip..], needle, budget)
        })
    }

    /// Fold, then find: linear whatever the text.
    #[cold]
    fn find_simply(&self, text: &str) -> Option<Found> {
        let needle = std::str::from_utf8(&self.bytes).unwrap_or_default();
        super::fold(text).find(needle).map(Found::Folded)
    }
}

/// What one char of text folds to.
#[derive(Debug, Clone, Copy)]
enum Unit {
    /// Whitespace (collapses into one space before the next char).
    Space,
    /// Its folded bytes (a char lowercases to at most three chars).
    Out([u8; 12], u8),
}

/// The char at byte `i` (a char start) of `text`: what it folds to, and
/// where the next char starts.
#[inline]
fn unit(text: &str, i: usize) -> (Unit, usize) {
    let b = text.as_bytes()[i];
    if b.is_ascii() {
        if is_ascii_space(b) {
            return (Unit::Space, i + 1);
        }
        let mut out = [0; 12];
        out[0] = b.to_ascii_lowercase();
        return (Unit::Out(out, 1), i + 1);
    }
    let c = text[i..].chars().next().unwrap_or_default();
    let next = i + c.len_utf8();
    if c.is_whitespace() {
        return (Unit::Space, next);
    }
    let mut out = [0; 12];
    let mut len = 0;
    let mut push = |c: char| len += c.encode_utf8(&mut out[len..]).len();
    match c {
        '\u{2019}' | '\u{2018}' | '\u{02BC}' => push('\''),
        '\u{201C}' | '\u{201D}' => push('"'),
        '\u{2013}' | '\u{2014}' => push('-'),
        _ => c.to_lowercase().for_each(push),
    }
    (Unit::Out(out, len as u8), next)
}

/// The length of the UTF-8 char whose lead byte is `b` (not ASCII).
#[inline]
fn utf8_len(b: u8) -> usize {
    match b {
        0xF0.. => 4,
        0xE0.. => 3,
        _ => 2,
    }
}

/// ASCII whitespace as `char::is_whitespace` has it.
#[inline]
fn is_ascii_space(b: u8) -> bool {
    matches!(b, b'\t'..=b'\r' | b' ')
}

#[inline(always)]
/// `true` if the folded text continues as `needle`: `first` (the rest of
/// a char's folded output) and then the text from byte `i`.
fn matches_from(text: &str, mut i: usize, first: &[u8], needle: &[u8], budget: &mut usize) -> bool {
    if first.len() >= needle.len() {
        return first.starts_with(needle);
    }
    if !needle.starts_with(first) {
        return false;
    }
    let bytes = text.as_bytes();
    let mut k = first.len();
    let mut space = false;
    while k < needle.len() {
        *budget = budget.saturating_sub(1);
        let Some(&b) = bytes.get(i) else {
            return false;
        };
        // ASCII, the common case, without decoding.
        if b.is_ascii() {
            i += 1;
            if is_ascii_space(b) {
                space = true;
                continue;
            }
            if space {
                space = false;
                if needle[k] != b' ' {
                    return false;
                }
                k += 1;
                if k == needle.len() {
                    return true;
                }
            }
            if needle[k] != b.to_ascii_lowercase() {
                return false;
            }
            k += 1;
            continue;
        }
        // A char equal to the needle's next one, byte for byte, matches
        // it: the needle is folded, and folded chars fold to themselves.
        // One that folds to itself and differs does not (the needle's
        // next char is whole: it cannot be a prefix of the char's output).
        if !space {
            let len = utf8_len(b);
            if let Some(chars) = bytes.get(i..i + len) {
                if needle.get(k..k + len) == Some(chars) {
                    i += len;
                    k += len;
                    continue;
                }
                if is_stable(tables(), chars) {
                    return false;
                }
            }
        }
        let (unit, next) = unit(text, i);
        i = next;
        let Unit::Out(out, len) = unit else {
            space = true;
            continue;
        };
        if space {
            space = false;
            if needle[k] != b' ' {
                return false;
            }
            k += 1;
        }
        for &o in &out[..len as usize] {
            if k == needle.len() {
                return true;
            }
            if needle[k] != o {
                return false;
            }
            k += 1;
        }
    }
    true
}

/// The length of `fold(text)` before the char at byte `at` (a char that
/// folds to something): the folded prefix, plus the space a whitespace
/// run right before it becomes.
fn folded_len_before(text: &str, at: usize) -> usize {
    let mut total = 0;
    let mut space = false;
    let mut i = 0;
    while i < at {
        let (unit, next) = unit(text, i);
        i = next;
        match unit {
            Unit::Space => space = true,
            Unit::Out(_, len) => {
                if space && total > 0 {
                    total += 1;
                }
                space = false;
                total += len as usize;
            }
        }
    }
    if space && total > 0 {
        total += 1;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::super::fold;
    use super::*;

    /// The reference: fold, then find.
    fn reference(text: &str, needle: &str) -> Option<usize> {
        fold(text).find(needle)
    }

    fn fast(text: &str, needle: &str) -> Option<usize> {
        Needle::new(needle)
            .find(text)
            .map(|found| found.folded_offset(text))
    }

    fn check(text: &str, query: &str) {
        let needle = fold(query);
        if needle.is_empty() {
            return;
        }
        assert_eq!(
            fast(text, &needle),
            reference(text, &needle),
            "text {text:?}, needle {needle:?}"
        );
    }

    #[test]
    fn hand_written_cases_match_the_reference() {
        let cases: &[(&str, &str)] = &[
            ("Golden river\nruns to the sea", "river runs"),
            ("Golden river\nruns to the sea", "RIVER  RUNS"),
            ("  leading\t\tand\r\n\r\ntrailing  ", "and trailing"),
            ("  leading", "leading"),
            ("Heart’s on fire", "heart's"),
            ("Heart‘s ʼon", "heart's 'on"),
            ("“quoted”", "\"quoted\""),
            ("rock – roll — on", "rock - roll - on"),
            ("İstanbul", "i̇stanbul"),
            ("İstanbul", "stanbul"),
            ("İstanbul", "\u{307}stan"),
            ("xİx", "i"),
            ("KELVIN \u{212A}elvin", "kelvin kelvin"),
            ("ÉTÉ été", "été été"),
            ("straße STRASSE", "strasse"),
            ("ΣΊΣΥΦΟΣ", "σίσυφοσ"),
            ("aaaab", "aab"),
            ("abababc", "ababc"),
            ("la la la la land", "la la land"),
            ("no\u{a0}break\u{3000}space", "no break space"),
            ("end", "end"),
            ("en", "end"),
            ("a\n\nb", "a b"),
            ("a\n \nb", "a b"),
            ("line one\nline two", "one line"),
            ("x", "x"),
            ("日本語の歌詞", "語の"),
            ("Ünïcödé ÜNÏCÖDÉ", "ünïcödé ünï"),
        ];
        for (text, query) in cases {
            check(text, query);
        }
        assert_eq!(fast("Golden river\nruns", "river runs"), Some(7));
        assert_eq!(fast("  a  b", "a b"), Some(0));
        assert_eq!(fast("x  İx", "i"), Some(2));
    }

    /// A small PRNG (no dependency).
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }

        fn below(&mut self, n: usize) -> usize {
            self.next() as usize % n
        }
    }

    const ALPHABET: &[char] = &[
        'a',
        'A',
        'b',
        'B',
        'i',
        'I',
        'k',
        'K',
        'l',
        's',
        'S',
        ' ',
        ' ',
        ' ',
        '\n',
        '\n',
        '\t',
        '\r',
        '\u{b}',
        '\u{c}',
        '\u{85}',
        '\u{a0}',
        '\u{1680}',
        '\u{2028}',
        '\u{2029}',
        '\u{3000}',
        '\u{200b}',
        '\'',
        '"',
        '-',
        '\u{2019}',
        '\u{2018}',
        '\u{02BC}',
        '\u{201C}',
        '\u{201D}',
        '\u{2013}',
        '\u{2014}',
        'é',
        'É',
        'ß',
        'ẞ',
        'İ',
        'ı',
        '\u{307}',
        '\u{212A}',
        '\u{2126}',
        'Ω',
        'ω',
        '\u{212B}',
        'Å',
        'å',
        'Σ',
        'σ',
        'ς',
        'ǅ',
        'ﬃ',
        'ж',
        'Ж',
        'д',
        'Д',
        '語',
        '日',
        'x',
        '\u{10400}',
        '\u{1D400}',
    ];

    fn random_text(rng: &mut Rng, len: usize) -> String {
        (0..len)
            .map(|_| ALPHABET[rng.below(ALPHABET.len())])
            .collect()
    }

    #[test]
    fn generated_cases_match_the_reference() {
        let mut rng = Rng(7);
        for round in 0..30_000 {
            let len = if round % 10 == 0 {
                rng.below(300)
            } else {
                rng.below(40)
            };
            let text = random_text(&mut rng, len);
            // A random query (usually no match)...
            let qlen = 1 + rng.below(4);
            let query = random_text(&mut rng, qlen);
            check(&text, &query);
            // ...and a piece of the folded text (always a match).
            let folded: Vec<char> = fold(&text).chars().collect();
            if !folded.is_empty() {
                let start = rng.below(folded.len());
                let end = start + 1 + rng.below(folded.len() - start);
                let piece: String = folded[start..end].iter().collect();
                check(&text, &piece);
                // The piece, refolded from different case and quotes.
                check(&text, &piece.to_uppercase());
            }
        }
    }

    /// Folding is idempotent: every char of a folded output folds to
    /// itself, so text that already equals the needle byte for byte
    /// matches it (the compare's fast path relies on this).
    #[test]
    fn folded_chars_fold_to_themselves() {
        let mut buf = [0; 4];
        for c in '\0'..=char::MAX {
            let text = c.encode_utf8(&mut buf);
            let (Unit::Out(out, len), _) = unit(text, 0) else {
                continue;
            };
            for folded in std::str::from_utf8(&out[..len as usize]).unwrap().chars() {
                let mut again = [0; 4];
                let text = folded.encode_utf8(&mut again);
                let (Unit::Out(out, len), _) = unit(text, 0) else {
                    panic!("{c:?} folds to whitespace {folded:?}");
                };
                assert_eq!(&out[..len as usize], text.as_bytes(), "{c:?} → {folded:?}");
            }
        }
    }

    #[test]
    fn stable_bits_are_the_chars_that_fold_to_themselves() {
        let tables = tables();
        let mut buf = [0; 4];
        for c in ('\u{80}'..='\u{FFFF}').filter(|c| !(0xD800..0xE000).contains(&(*c as u32))) {
            let text = c.encode_utf8(&mut buf);
            let folds_to_itself = matches!(unit(text, 0), (Unit::Out(out, len), _) if &out[..len as usize] == text.as_bytes());
            assert_eq!(is_stable(tables, text.as_bytes()), folds_to_itself, "{c:?}");
        }
    }

    #[test]
    fn sources_are_every_char_that_folds_to_the_char() {
        let all: Vec<(char, String)> = ('\0'..=char::MAX)
            .filter_map(|c| {
                let text = c.to_string();
                match unit(&text, 0) {
                    (Unit::Out(out, len), _) => Some((
                        c,
                        std::str::from_utf8(&out[..len as usize])
                            .unwrap()
                            .to_owned(),
                    )),
                    _ => None,
                }
            })
            .collect();
        for target in [
            'a', 'i', 'k', 's', '\'', '"', '-', '\u{307}', 'ж', 'σ', 'ς', 'ω', 'å', 'ß', 'é', '語',
            'ǆ',
        ] {
            let mut expected: Vec<char> = all
                .iter()
                .filter(|(_, out)| out.contains(target))
                .map(|(c, _)| *c)
                .collect();
            let mut got = sources(target);
            expected.sort();
            got.sort();
            assert_eq!(got, expected, "{target:?}");
        }
        assert_eq!(sources('ж').len(), 2);
        let mut quotes = sources('\'');
        quotes.sort();
        assert_eq!(quotes, ['\'', '\u{2BC}', '\u{2018}', '\u{2019}']);
    }

    #[test]
    fn repetitive_text_uses_the_simple_way_and_stays_exact() {
        let text = "a".repeat(20_000) + "b";
        let needle = "a".repeat(200) + "b";
        check(&text, &needle);
        let needle = "a".repeat(200) + "c";
        check(&text, &needle);
        let found = Needle::new(&("a".repeat(200) + "b")).find(&text);
        assert!(matches!(found, Some(Found::Folded(_))), "{found:?}");
        // Non-ASCII text through the same fallback, matches spanning
        // whitespace kinds and case.
        let text = "Жж\u{a0}".repeat(5_000) + "Жж\u{2028}д";
        let needle = fold(&"жж ".repeat(150)) + " д";
        check(&text, &needle);
        check(&text, &(fold(&"жж ".repeat(150)) + "x"));
        let found = Needle::new(&needle).find(&text);
        assert!(matches!(found, Some(Found::Folded(_))), "{found:?}");
        // Many rejected candidates also count.
        let text = "ж".repeat(40_000) + "д";
        check(&text, "жд");
    }
}
