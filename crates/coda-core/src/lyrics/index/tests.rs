// Snippet highlights are lists of ranges; one-range lists are the point.
#![allow(clippy::single_range_in_vec_init)]

use std::time::{Duration, Instant};

use super::*;
use crate::lyrics::parse;

fn indexed(text: &str) -> IndexedLyrics {
    IndexedLyrics::new(&parse(text)).expect("lyrics with text")
}

fn index(texts: &[&str]) -> LyricsIndex<u32> {
    let mut index = LyricsIndex::new();
    for (i, text) in texts.iter().enumerate() {
        index.insert(i as u32, indexed(text));
    }
    index
}

fn by_key(a: &u32, b: &u32) -> Ordering {
    a.cmp(b)
}

fn search(index: &LyricsIndex<u32>, query: &str) -> LyricsResults<u32> {
    index.search(query, |_| false, DEFAULT_LIMIT, by_key)
}

fn keys(results: &LyricsResults<u32>) -> Vec<u32> {
    results.matches.iter().map(|m| m.key).collect()
}

/// The highlighted parts of a snippet.
fn marked(snippet: &Snippet) -> Vec<String> {
    let chars: Vec<char> = snippet.text.chars().collect();
    snippet
        .highlights
        .iter()
        .map(|r| chars[r.clone()].iter().collect())
        .collect()
}

#[test]
fn folding_lowercases_and_unifies_quotes_dashes_and_whitespace() {
    assert_eq!(
        fold("  Don\u{2019}t \u{201C}STOP\u{201D}\u{2014}now \t\n me\u{2018}  "),
        "don't \"stop\"-now me'"
    );
    assert_eq!(fold("it\u{02BC}s a \u{2013} b"), "it's a - b");
    assert_eq!(fold("ÉTÉ\u{00A0}Ÿes"), "été ÿes");
    assert_eq!(fold(" \n\t "), "");
}

#[test]
fn curly_and_straight_quotes_and_case_match_each_other() {
    let index = index(&["I Don\u{2019}t Know", "we don't care", "nothing here"]);
    assert_eq!(keys(&search(&index, "don't")), [0, 1]);
    assert_eq!(keys(&search(&index, "DON\u{2019}T")), [0, 1]);
    assert_eq!(keys(&search(&index, "  don\u{2018}t   KNOW ")), [0]);
}

#[test]
fn a_phrase_across_a_line_break_matches() {
    // Timed lyrics: the stamps never show, and blank pauses collapse too.
    let index =
        index(&["[00:01.00]  Hello  darkness\n[00:02.00]\n[00:03.00]my <00:03.50>old friend"]);
    let results = search(&index, "darkness my old");
    assert_eq!(results.total, 1);
    let snippet = &results.matches[0].snippet;
    assert_eq!(snippet.text, "Hello darkness / my old friend");
    assert_eq!(marked(snippet), ["darkness", "my old"]);
    // A match inside one line gives that line alone.
    let snippet = &search(&index, "old fri").matches[0].snippet;
    assert_eq!(snippet.text, "my old friend");
    assert_eq!(marked(snippet), ["old fri"]);
    // Internal runs of whitespace collapse in the match, not in the text
    // (plain lyrics keep them).
    let index = index_one("Hello \t darkness");
    let snippet = &search(&index, "hello darkness").matches[0].snippet;
    assert_eq!(snippet.text, "Hello \t darkness");
    assert_eq!(marked(snippet), ["Hello \t darkness"]);
    // A match over three lines keeps every line.
    let index = index_one("one\ntwo\nthree\nfour");
    let snippet = &search(&index, "e two thr").matches[0].snippet;
    assert_eq!(snippet.text, "one / two / three");
    assert_eq!(marked(snippet), ["e", "two", "thr"]);
}

fn index_one(text: &str) -> LyricsIndex<u32> {
    index(&[text])
}

#[test]
fn highlights_are_char_ranges_in_non_ascii_text() {
    let index = index_one("Ça plane pour moi\nÉTÉ \u{201C}indien\u{201D} 夏の歌");
    let snippet = &search(&index, "été \"indien\" 夏").matches[0].snippet;
    assert_eq!(snippet.text, "ÉTÉ \u{201C}indien\u{201D} 夏の歌");
    assert_eq!(snippet.highlights, [0..14]);
    assert_eq!(marked(snippet), ["ÉTÉ \u{201C}indien\u{201D} 夏"]);
    let snippet = &search(&index, "moi été").matches[0].snippet;
    assert_eq!(
        snippet.text,
        "Ça plane pour moi / ÉTÉ \u{201C}indien\u{201D} 夏の歌"
    );
    assert_eq!(snippet.highlights, [14..17, 20..23]);
}

#[test]
fn a_match_that_splits_a_case_mapping_has_no_highlight() {
    // `İ` lowercases to two chars (`i` and a combining dot): a match
    // that takes only the first cannot be mapped back.
    let index = index_one("xİy abc");
    let results = search(&index, "xi\u{307}y");
    assert_eq!(marked(&results.matches[0].snippet), ["xİy"]);
    let results = search(&index, "zzz");
    assert!(results.matches.is_empty());
    let index = index_one("abİ");
    let results = search(&index, "abi");
    assert_eq!(results.total, 1);
    assert_eq!(results.matches[0].snippet.text, "abİ");
    assert!(results.matches[0].snippet.highlights.is_empty());
}

#[test]
fn short_queries_find_nothing() {
    let index = index(&["ab ab", "abc"]);
    assert_eq!(search(&index, "ab"), LyricsResults::default());
    // Three chars after folding is enough (`a b` is three).
    assert_eq!(keys(&search(&index, "  a  b ")), Vec::<u32>::new());
    assert_eq!(keys(&search(&index, "b a")), [0]);
    assert_eq!(keys(&search(&index, "abc")), [1]);
    // Counted in chars, not bytes.
    let index = index_one("été");
    assert_eq!(search(&index, "ét").total, 0);
    assert_eq!(search(&index, "été").total, 1);
}

#[test]
fn results_are_capped_ordered_and_counted() {
    let texts: Vec<String> = (0..120).map(|i| format!("line {i}\nla la la")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let index = index(&refs);
    // Reverse key order.
    let results = index.search("la la", |_| false, 50, |a, b| b.cmp(a));
    assert_eq!(results.total, 120);
    assert_eq!(results.matches.len(), 50);
    assert_eq!(results.matches[0].key, 119);
    assert_eq!(results.matches[49].key, 70);
    // One result per track, at its first match.
    assert_eq!(results.matches[0].snippet.text, "la la la");
    assert_eq!(results.matches[0].snippet.highlights, [0..5]);
}

#[test]
fn excluded_keys_are_skipped_and_not_counted() {
    let index = index(&["same words", "same words", "same words"]);
    let exclude = std::collections::HashSet::from([1]);
    let results = index.search("same", |k| exclude.contains(k), 50, by_key);
    assert_eq!(keys(&results), [0, 2]);
    assert_eq!(results.total, 2);
}

#[test]
fn blank_lyrics_take_no_entry() {
    assert_eq!(IndexedLyrics::new(&parse("")), None);
    assert_eq!(IndexedLyrics::new(&parse("[00:01]\n[00:02]")), None);
    let lyrics = indexed("[00:01]a\n[00:02]\n[00:03]b");
    assert_eq!(&*lyrics.folded, "a b");
    assert_eq!(&*lyrics.text, "a\nb");
    assert_eq!(lyrics.heap_bytes(), 3 + 3);
}

#[test]
fn untrimmed_and_blank_lines_are_trimmed_and_skipped() {
    // `Lyrics` has pub fields (slice 2 builds it from a cache): lines with
    // stray whitespace, whitespace-only lines and embedded line breaks
    // must not break matching across lines or the highlight.
    use crate::lyrics::Line;
    let line = |text: &str| Line {
        start: None,
        text: text.to_owned(),
    };
    let lyrics = Lyrics {
        lines: vec![
            line("hello "),
            line("  \t "),
            line(" world\r\n  again "),
            line(""),
        ],
        timed: false,
    };
    let indexed = IndexedLyrics::new(&lyrics).unwrap();
    assert_eq!(&*indexed.folded, "hello world again");
    assert_eq!(&*indexed.text, "hello\nworld\nagain");
    let mut index = LyricsIndex::new();
    index.insert(0u32, indexed);
    let results = search(&index, "hello world");
    assert_eq!(results.total, 1);
    let snippet = &results.matches[0].snippet;
    assert_eq!(snippet.text, "hello / world");
    assert_eq!(marked(snippet), ["hello", "world"]);
    let snippet = &search(&index, "world again").matches[0].snippet;
    assert_eq!(snippet.text, "world / again");
    assert_eq!(marked(snippet), ["world", "again"]);
    // Only whitespace is no lyrics.
    let blank = Lyrics {
        lines: vec![line(" "), line("\n\t")],
        timed: false,
    };
    assert_eq!(IndexedLyrics::new(&blank), None);
}

/// A verse of about 1.5 KB that varies per track, so the text is not one
/// repeated string.
fn synthetic_lyrics(i: usize) -> String {
    let words = [
        "night",
        "river",
        "golden",
        "Shadow",
        "dancing",
        "Heart’s",
        "fire",
        "“echo”",
        "slowly",
        "morning",
        "Stranger",
        "window",
        "silver",
        "train",
        "falling",
        "à",
        "été",
        "rain",
    ];
    let mut out = String::new();
    let mut n = i as u64;
    while out.len() < 1_500 {
        for _ in 0..7 {
            n = n
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            out.push_str(words[(n >> 33) as usize % words.len()]);
            out.push(' ');
        }
        out.push('\n');
    }
    out
}

/// 50,000 tracks with about 1.5 KB of lyrics each. Prints the build time,
/// the memory held and the time per search (run with `--release
/// --ignored --nocapture` for the real figures); the assertions only
/// catch something pathological.
#[test]
#[ignore = "timing: run in release with --nocapture"]
fn fifty_thousand_tracks_of_lyrics_search_quickly() {
    let texts: Vec<String> = (0..50_000).map(synthetic_lyrics).collect();
    let text_bytes: usize = texts.iter().map(String::len).sum();
    let start = Instant::now();
    let mut index = LyricsIndex::new();
    for (i, text) in texts.iter().enumerate() {
        index.insert(i as u32, indexed(text));
    }
    let built = start.elapsed();
    println!(
        "index 50,000 tracks ({:.1} MB of lyrics): {built:?}, {:.1} MB held",
        text_bytes as f64 / 1e6,
        index.heap_bytes() as f64 / 1e6
    );
    drop(texts);
    let mut worst = Duration::ZERO;
    for query in ["zzz no match", "golden river night", "heart's fire", "ÉTÉ"] {
        let start = Instant::now();
        let results = search(&index, query);
        let elapsed = start.elapsed();
        worst = worst.max(elapsed);
        println!("search {query:?}: {} matches in {elapsed:?}", results.total);
    }
    assert!(search(&index, "heart's fire").total > 0);
    assert!(worst < Duration::from_secs(5), "{worst:?}");
}
