use std::fs;
use std::path::Path;
use std::time::Duration;

use super::*;

fn ms(n: u64) -> Option<Duration> {
    Some(Duration::from_millis(n))
}

fn line(start: Option<Duration>, text: &str) -> Line {
    Line {
        start,
        text: text.to_owned(),
    }
}

fn texts(lyrics: &Lyrics) -> Vec<&str> {
    lyrics.lines.iter().map(|l| l.text.as_str()).collect()
}

fn starts(lyrics: &Lyrics) -> Vec<Option<Duration>> {
    lyrics.lines.iter().map(|l| l.start).collect()
}

// ----- the parser -----------------------------------------------------

#[test]
fn every_timestamp_form_is_read() {
    let lyrics = parse(
        "[00:01]a\n[00:02.5]b\n[00:03.25]c\n[00:04.125]d\n[00:05:50]e\n[1:06.00]f\n[123:00]g",
    );
    assert!(lyrics.timed);
    assert_eq!(
        lyrics.lines,
        [
            line(ms(1_000), "a"),
            line(ms(2_500), "b"),
            line(ms(3_250), "c"),
            line(ms(4_125), "d"),
            line(ms(5_500), "e"),
            line(ms(66_000), "f"),
            line(ms(7_380_000), "g"),
        ]
    );
}

#[test]
fn several_timestamps_give_one_line_each_sorted_by_start() {
    let lyrics = parse("[00:12.00][01:30.00]chorus\n[00:20.00]verse\n[00:05.00]intro");
    assert_eq!(
        lyrics.lines,
        [
            line(ms(5_000), "intro"),
            line(ms(12_000), "chorus"),
            line(ms(20_000), "verse"),
            line(ms(90_000), "chorus"),
        ]
    );
}

#[test]
fn equal_starts_keep_their_order() {
    let lyrics = parse("[00:01]first\n[00:01]second");
    assert_eq!(texts(&lyrics), ["first", "second"]);
}

#[test]
fn offset_shifts_every_start_and_id_tags_are_dropped() {
    let text = "[ar:Artist]\n[ti:Title]\n[al:Album]\n[by:me]\n[length: 03:20]\n\
                [00:01.00]one\n[offset:+500]\n[00:00.20]zero\n[00:10.00]ten";
    let lyrics = parse(text);
    assert_eq!(
        lyrics.lines,
        [
            line(ms(0), "zero"),
            line(ms(500), "one"),
            line(ms(9_500), "ten")
        ]
    );
    // A negative offset shows lines later.
    let later = parse("[offset:-250]\n[00:01.00]x");
    assert_eq!(later.lines, [line(ms(1_250), "x")]);
    // A malformed offset is ignored.
    let ignored = parse("[offset:soon]\n[00:01.00]x");
    assert_eq!(ignored.lines, [line(ms(1_000), "x")]);
    // A huge offset saturates instead of overflowing.
    let huge = parse(&format!("[offset:{}]\n[00:01.00]x", i64::MIN));
    assert_eq!(huge.lines.len(), 1);
    let huge = parse(&format!("[offset:{}]\n[00:01.00]x", i64::MAX));
    assert_eq!(huge.lines, [line(ms(0), "x")]);
}

#[test]
fn word_stamps_next_to_stray_angle_brackets() {
    let lyrics = parse("[00:01]a <3 <00:01.50>b < c <00:02.00>> d<00:03.00>");
    assert_eq!(texts(&lyrics), ["a <3 b < c > d"]);
    assert_eq!(texts(&parse("[00:01]x <y> z")), ["x <y> z"]);
}

#[test]
fn many_unmatched_angle_brackets_parse_quickly_and_are_kept() {
    // This took seconds with a scan that restarted after every `<`.
    for text in [
        "<".repeat(250_000),
        format!("{}>", "<".repeat(250_000)),
        "<a".repeat(120_000),
    ] {
        let started = std::time::Instant::now();
        let lyrics = parse(&format!("[00:01]{text}"));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(lyrics.lines.len(), 1);
        assert!(lyrics.lines[0].text.starts_with("<"));
    }
}

#[test]
fn a_stray_timestamp_does_not_make_plain_lyrics_lrc() {
    // Fewer timed lines than other text lines: plain text, stamp and all.
    let lyrics = parse("Verse one\nline two\n[00:30] spoken\nline four");
    assert!(!lyrics.timed);
    assert_eq!(
        texts(&lyrics),
        ["Verse one", "line two", "[00:30] spoken", "line four"]
    );
    // As many timed lines as other text lines (blank lines and ID tags not
    // counted): LRC, the untimed lines dropped.
    let lyrics = parse("[ar:x]\n[ti:y]\ncredit\n\n\n[00:01]one\nnote\n[00:02]two");
    assert!(lyrics.timed);
    assert_eq!(texts(&lyrics), ["one", "two"]);
    // One more text line tips it to plain.
    let lyrics = parse("credit\n[00:01]one\nnote\n[00:02]two\nmore");
    assert!(!lyrics.timed);
    assert_eq!(lyrics.lines.len(), 5);
}

#[test]
fn word_stamps_are_stripped_and_other_angle_brackets_kept() {
    let lyrics =
        parse("[00:01.00]<00:01.00> Hello <00:01.50> world <00:02.00>\n[00:03.00]I <3 you");
    assert_eq!(texts(&lyrics), ["Hello world", "I <3 you"]);
}

#[test]
fn timed_lines_with_no_text_are_pauses_and_untimed_lines_are_dropped() {
    let lyrics = parse("[00:01.00]one\n[00:02.00]\n\nstray text\n[00:03.00]three");
    assert_eq!(
        lyrics.lines,
        [
            line(ms(1_000), "one"),
            line(ms(2_000), ""),
            line(ms(3_000), "three")
        ]
    );
    assert!(!lyrics.is_blank());
    assert!(parse("[00:01.00]\n[00:02.00]").is_blank());
}

#[test]
fn crlf_and_a_bom_are_accepted() {
    let lyrics = parse("\u{feff}[ti:x]\r\n[00:01.00]one\r\n[00:02.00]two\r\n");
    assert_eq!(texts(&lyrics), ["one", "two"]);
    let plain = parse("\u{feff}one\r\ntwo\r\n");
    assert_eq!(texts(&plain), ["one", "two"]);
    assert!(!plain.timed);
}

#[test]
fn text_without_timestamps_is_untimed_with_outer_blank_lines_trimmed() {
    let lyrics = parse("\n\n[Chorus]\nLa la  la\n\n[Verse 1: Someone]\nmore\n\n\n");
    assert!(!lyrics.timed);
    assert_eq!(
        texts(&lyrics),
        ["[Chorus]", "La la  la", "", "[Verse 1: Someone]", "more"]
    );
    assert!(starts(&lyrics).iter().all(Option::is_none));
    assert!(parse("").is_blank());
    assert!(parse("\n \n").lines.is_empty());
    // ID tags alone are not lyrics.
    assert!(parse("[ar:Someone]\n[ti:Something]").lines.is_empty());
}

#[test]
fn malformed_tags_are_text_not_timestamps() {
    for bad in [
        "[00:60.00]x",
        "[aa:bb]x",
        "[00:01.0000]x",
        "[00:01.]x",
        "[-1:00]x",
        "[123456:00]x",
        "[00:1x]x",
        "[00:01",
        "[]x",
        "[:]x",
        "[00]x",
    ] {
        let lyrics = parse(bad);
        assert!(!lyrics.timed, "{bad}");
        assert_eq!(texts(&lyrics), [bad], "{bad}");
    }
}

#[test]
fn garbage_never_panics() {
    let mut seed: u32 = 0x1234_5678;
    let alphabet: Vec<char> = "[]<>:.+-0123456789 \r\n\u{feff}éoffsetar#漢🎵"
        .chars()
        .collect();
    for _ in 0..2_000 {
        let len = (seed % 60) as usize;
        let text: String = (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                alphabet[(seed >> 16) as usize % alphabet.len()]
            })
            .collect();
        let lyrics = parse(&text);
        if lyrics.timed {
            assert!(lyrics.lines.iter().all(|l| l.start.is_some()));
            assert!(lyrics.lines.is_sorted_by_key(|l| l.start));
        } else {
            assert!(lyrics.lines.iter().all(|l| l.start.is_none()));
        }
    }
}

#[test]
fn huge_input_is_capped() {
    // Lines past the cap are dropped (the earliest are kept).
    let many: String = (0..MAX_LINES as u64 + 100)
        .rev()
        .map(|i| format!("[{:02}:{:02}.00]l{i}\n", i / 60, i % 60))
        .collect();
    let lyrics = parse(&many);
    assert_eq!(lyrics.lines.len(), MAX_LINES);
    assert_eq!(lyrics.lines[0].text, "l0");
    // Long lines are cut.
    let long = parse(&format!("[00:01]{}", "é".repeat(MAX_LINE_CHARS * 2)));
    assert_eq!(long.lines[0].text.chars().count(), MAX_LINE_CHARS);
    // Text past the byte cap is ignored, cut on a character boundary.
    let mut big = "é".repeat(MAX_LYRICS_BYTES / 2 - 1);
    big.push_str("x\nafter the cap");
    let lyrics = parse(&big);
    assert!(texts(&lyrics).iter().all(|t| !t.contains("after")));
    // One line with a great many timestamps gives at most 100 lines (each
    // a copy of its text).
    let stamps = "[00:01]".repeat(MAX_LYRICS_BYTES / 8);
    let lyrics = parse(&format!("{stamps}{}", "€".repeat(MAX_LINE_CHARS)));
    assert_eq!(lyrics.lines.len(), MAX_STAMPS_PER_LINE);
    // Many such lines still stop at the line cap.
    let line = format!("{}x\n", "[00:01]".repeat(MAX_STAMPS_PER_LINE + 5));
    let lyrics = parse(&line.repeat(MAX_LINES / MAX_STAMPS_PER_LINE + 5));
    assert_eq!(lyrics.lines.len(), MAX_LINES);
    // Many plain lines.
    let lyrics = parse(&"a\n".repeat(MAX_LINES * 3));
    assert_eq!(lyrics.lines.len(), MAX_LINES);
}

// ----- local files ----------------------------------------------------

/// A minimal FLAC file: the marker and a STREAMINFO block (44.1 kHz,
/// stereo, 16 bit, no frames).
fn write_flac(path: &Path) {
    let mut bytes = b"fLaC".to_vec();
    bytes.push(0x80); // last metadata block, STREAMINFO
    bytes.extend(&34u32.to_be_bytes()[1..]);
    bytes.extend(4096u16.to_be_bytes()); // min block size
    bytes.extend(4096u16.to_be_bytes()); // max block size
    bytes.extend([0; 6]); // min and max frame size: unknown
    let packed: u64 = (44_100 << 44) | (1 << 41) | (15 << 36);
    bytes.extend(packed.to_be_bytes());
    bytes.extend([0; 16]); // MD5
    fs::write(path, bytes).unwrap();
}

/// A minimal MP3 file: a few silent MPEG-1 Layer III frames (128 kbit/s,
/// 44.1 kHz, 417 bytes each).
fn write_mp3(path: &Path) {
    let mut frame = vec![0xFF, 0xFB, 0x90, 0x00];
    frame.resize(417, 0);
    fs::write(path, frame.repeat(8)).unwrap();
}

fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = (8 + body.len() as u32).to_be_bytes().to_vec();
    out.extend(kind);
    out.extend(body);
    out
}

/// A minimal M4A file: `ftyp` and a `moov` with an `mvhd`.
fn write_m4a(path: &Path) {
    let mut ftyp = b"M4A ".to_vec();
    ftyp.extend(0u32.to_be_bytes());
    ftyp.extend(b"M4A isom");
    let mut mvhd = vec![0; 4]; // version 0, no flags
    mvhd.extend(0u32.to_be_bytes()); // created
    mvhd.extend(0u32.to_be_bytes()); // modified
    mvhd.extend(1000u32.to_be_bytes()); // timescale
    mvhd.extend(0u32.to_be_bytes()); // duration
    mvhd.extend(0x0001_0000u32.to_be_bytes()); // rate
    mvhd.extend(0x0100u16.to_be_bytes()); // volume
    mvhd.extend([0; 10]);
    for value in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        mvhd.extend(value.to_be_bytes());
    }
    mvhd.extend([0; 24]);
    mvhd.extend(2u32.to_be_bytes()); // next track id
    let mut bytes = mp4_box(b"ftyp", &ftyp);
    bytes.extend(mp4_box(b"moov", &mp4_box(b"mvhd", &mvhd)));
    fs::write(path, bytes).unwrap();
}

/// A fresh minimal FLAC at `path` with `items` as its Vorbis comments
/// (rewriting the comments of a frameless FLAC in place can leave a file
/// lofty no longer reads, so every case starts from a new file).
fn flac_with(path: &Path, items: &[(lofty::tag::ItemKey, &str)]) {
    write_flac(path);
    tag(path, lofty::tag::TagType::VorbisComments, items);
}

/// Writes `items` as a tag of `tag_type` into the file at `path`.
fn tag(path: &Path, tag_type: lofty::tag::TagType, items: &[(lofty::tag::ItemKey, &str)]) {
    use lofty::config::WriteOptions;
    use lofty::prelude::TagExt;
    use lofty::tag::{ItemValue, Tag, TagItem};
    let mut tag = Tag::new(tag_type);
    for (key, value) in items {
        tag.push(TagItem::new(*key, ItemValue::Text((*value).to_owned())));
    }
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

const LRC: &str = "[ar:Someone]\n[00:01.00]first\n[00:02.50]second\n";

#[test]
fn flac_lyrics_prefer_the_first_timed_value() {
    use lofty::tag::ItemKey;
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song.flac");
    write_flac(&song);
    assert_eq!(read_local(&song).unwrap(), None, "no tags at all");

    flac_with(&song, &[(ItemKey::Lyrics, LRC)]);
    let lyrics = read_local(&song).unwrap().unwrap();
    assert!(lyrics.timed);
    assert_eq!(
        lyrics.lines,
        [line(ms(1_000), "first"), line(ms(2_500), "second")]
    );

    // A plain value first, a timed one later: the timed one wins.
    flac_with(
        &song,
        &[
            (ItemKey::UnsyncLyrics, "just words"),
            (ItemKey::Lyrics, "   "),
            (ItemKey::Lyrics, "plain one\nplain two"),
            (ItemKey::Lyrics, LRC),
        ],
    );
    assert!(read_local(&song).unwrap().unwrap().timed);

    // Only plain values: the first one with text.
    flac_with(
        &song,
        &[
            (ItemKey::Lyrics, "\n"),
            (ItemKey::Lyrics, "plain one\nplain two"),
        ],
    );
    let lyrics = read_local(&song).unwrap().unwrap();
    assert!(!lyrics.timed);
    assert_eq!(texts(&lyrics), ["plain one", "plain two"]);

    // UNSYNCEDLYRICS alone is read too.
    flac_with(&song, &[(ItemKey::UnsyncLyrics, "unsynced words")]);
    let lyrics = read_local(&song).unwrap().unwrap();
    assert_eq!(texts(&lyrics), ["unsynced words"]);
}

#[test]
fn m4a_lyrics_are_read() {
    use lofty::tag::{ItemKey, TagType};
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song.m4a");
    write_m4a(&song);
    assert_eq!(read_local(&song).unwrap(), None);
    tag(&song, TagType::Mp4Ilst, &[(ItemKey::Lyrics, LRC)]);
    let lyrics = read_local(&song).unwrap().unwrap();
    assert!(lyrics.timed);
    assert_eq!(texts(&lyrics), ["first", "second"]);
}

#[test]
fn mp3_uslt_lyrics_are_read() {
    use lofty::tag::{ItemKey, TagType};
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song.mp3");
    write_mp3(&song);
    assert_eq!(read_local(&song).unwrap(), None);
    // ID3v2 has no `Lyrics` item: lofty writes and reads `USLT` as
    // `UnsyncLyrics`, timed text included.
    tag(&song, TagType::Id3v2, &[(ItemKey::UnsyncLyrics, LRC)]);
    let lyrics = read_local(&song).unwrap().unwrap();
    assert!(lyrics.timed);
    assert_eq!(texts(&lyrics), ["first", "second"]);
}

#[cfg(unix)]
#[test]
fn a_fifo_sidecar_is_ignored_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song.flac");
    write_flac(&song);
    let fifo = dir.path().join("song.lrc");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());
    // Run on another thread so a regression fails the test instead of
    // hanging it.
    let (tx, rx) = std::sync::mpsc::channel();
    let path = song.clone();
    std::thread::spawn(move || tx.send(read_local(&path).map_err(|e| e.to_string())));
    let result = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("blocked on the FIFO");
    assert_eq!(result.unwrap(), None);
    // A symlink to the FIFO too.
    let other = dir.path().join("other.flac");
    write_flac(&other);
    std::os::unix::fs::symlink(&fifo, dir.path().join("other.lrc")).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(read_local(&other).map_err(|e| e.to_string())));
    let result = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("blocked on the FIFO");
    assert_eq!(result.unwrap(), None);
    // The non-blocking open itself does not wait for a writer either.
    assert!(super::local::open_nonblocking(&fifo).is_ok());
}

#[test]
fn a_sidecar_is_read_only_when_the_tags_have_no_lyrics() {
    use lofty::tag::ItemKey;
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("song.flac");
    write_flac(&song);
    fs::write(
        dir.path().join("song.lrc"),
        b"\xef\xbb\xbf[00:03.00]from the sidecar\r\n",
    )
    .unwrap();
    let lyrics = read_local(&song).unwrap().unwrap();
    assert_eq!(lyrics.lines, [line(ms(3_000), "from the sidecar")]);

    flac_with(&song, &[(ItemKey::Lyrics, "tagged")]);
    assert_eq!(texts(&read_local(&song).unwrap().unwrap()), ["tagged"]);

    // A blank sidecar is no lyrics.
    flac_with(&song, &[(ItemKey::TrackTitle, "x")]);
    fs::write(dir.path().join("song.lrc"), "\n\n").unwrap();
    assert_eq!(read_local(&song).unwrap(), None);

    // An oversized sidecar is read only up to the cap.
    let mut big = "[00:01.00]early\n".to_owned();
    big.push_str(&"x".repeat(MAX_LYRICS_BYTES));
    big.push_str("\n[00:00.50]past the cap\n");
    fs::write(dir.path().join("song.lrc"), big).unwrap();
    let lyrics = read_local(&song).unwrap().unwrap();
    assert_eq!(texts(&lyrics), ["early"]);
}

#[test]
fn unreadable_or_garbage_files() {
    let dir = tempfile::tempdir().unwrap();
    // A missing file is an error.
    assert!(read_local(&dir.path().join("missing.flac")).is_err());
    // A directory is an error.
    assert!(read_local(dir.path()).is_err());
    // A file lofty cannot parse has no tag lyrics, but its sidecar counts.
    let song = dir.path().join("garbage.flac");
    fs::write(&song, b"fLaC\xff\xff\xff\xff not really").unwrap();
    assert_eq!(read_local(&song).unwrap(), None);
    fs::write(dir.path().join("garbage.lrc"), "words").unwrap();
    assert_eq!(texts(&read_local(&song).unwrap().unwrap()), ["words"]);
    // A file named `.lrc` is not its own sidecar.
    let lrc = dir.path().join("only.lrc");
    fs::write(&lrc, "[00:01]x").unwrap();
    assert_eq!(read_local(&lrc).unwrap(), None);
}
