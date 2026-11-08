use std::os::fd::{AsRawFd, OwnedFd};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

use super::*;

/// Ghostty 1.4's exact replies to [`QUERY`] (measured in the M9 spike).
const GHOSTTY: &[u8] = b"\x1b_Gi=31;OK\x1b\\\x1b[6;34;16t\x1b[?62;22;52c";
/// Terminal.app's: no Kitty reply, no cell size, DA1 only.
const TERMINAL_APP: &[u8] = b"\x1b[?1;2c";

fn parse(chunks: &[&[u8]]) -> ReplyParser {
    let mut parser = ReplyParser::default();
    for chunk in chunks {
        parser.feed(chunk);
    }
    parser
}

#[test]
fn ghostty_replies_mean_kitty_with_its_cell_size() {
    let parser = parse(&[GHOSTTY]);
    assert!(parser.done());
    assert!(parser.kitty);
    assert_eq!(parser.cell, Some((16, 34)));
    let reply = QueryReply {
        kitty: parser.kitty,
        cell: parser.cell,
        timed_out: false,
        inside: Inside::Nothing,
    };
    assert_eq!(
        reply.detection(None),
        Detection {
            graphics: Graphics::Kitty { cell: (16, 34) },
            queried: true,
            late: None,
        }
    );
}

#[test]
fn replies_split_anywhere_parse_the_same() {
    // One byte at a time.
    let bytes: Vec<&[u8]> = GHOSTTY.chunks(1).collect();
    let parser = parse(&bytes);
    assert!(parser.done() && parser.kitty);
    assert_eq!(parser.cell, Some((16, 34)));
    // Every split point into two reads.
    for split in 0..GHOSTTY.len() {
        let (a, b) = GHOSTTY.split_at(split);
        let parser = parse(&[a, b]);
        assert!(parser.done() && parser.kitty, "split at {split}");
        assert!(!parse(&[a]).done() || split == GHOSTTY.len());
    }
}

#[test]
fn no_kitty_reply_means_half_blocks() {
    let parser = parse(&[TERMINAL_APP]);
    assert!(parser.done());
    assert!(!parser.kitty);
    assert_eq!(parser.cell, None);
    // An error from a terminal that knows the protocol: no Kitty either.
    let parser = parse(&[
        b"\x1b_Gi=31;EINVAL:unsupported\x1b\\",
        b"\x1b[6;34;16t\x1b[?62c",
    ]);
    assert!(parser.done() && !parser.kitty);
    // A reply for another image id is not ours.
    assert!(!parse(&[b"\x1b_Gi=7;OK\x1b\\\x1b[?62c"]).kitty);
}

#[test]
fn garbage_and_typed_keys_around_the_replies_are_skipped() {
    let parser = parse(&[
        b"jk\x1bOA\x1b[A",
        b"\x1b_Gi=31;OK\x1b\\",
        b"q\x1b[6;x;yt\x1b[6;34;16;9t\x1b[200~",
        b"\x1b[?62;22c",
    ]);
    assert!(parser.done() && parser.kitty);
    assert_eq!(parser.cell, None, "malformed size replies are ignored");
    // Bytes after DA1 are not looked at.
    let mut parser = parse(&[b"\x1b[?62c"]);
    parser.feed(b"\x1b_Gi=31;OK\x1b\\");
    assert!(!parser.kitty);
    // An endless unterminated sequence does not grow without bound.
    let mut parser = ReplyParser::default();
    parser.feed(b"\x1b_G");
    for _ in 0..100 {
        parser.feed(&[b'x'; 1000]);
    }
    assert!(!parser.done());
    parser.feed(TERMINAL_APP);
    assert!(parser.done());
}

#[test]
fn where_a_cut_off_reply_stopped_is_noticed() {
    assert_eq!(parse(&[b"\x1b_Gi=3"]).inside(), Inside::Kitty);
    assert_eq!(parse(&[b"\x1b_Gi=31;OK\x1b\\"]).inside(), Inside::Nothing);
    assert_eq!(
        parse(&[b"\x1b_Gi=31;OK\x1b\\\x1b"]).inside(),
        Inside::Escape
    );
    assert_eq!(
        parse(&[b"\x1b_Gi=31;OK\x1b\\\x1b[?62"]).inside(),
        Inside::Csi
    );
    assert_eq!(parse(&[b"\x1b[6;3"]).inside(), Inside::Csi);
    assert_eq!(parse(&[b""]).inside(), Inside::Nothing);
}

#[test]
fn the_cell_size_falls_back_to_the_window_size_then_a_default() {
    let reply = |cell| QueryReply {
        kitty: true,
        cell,
        timed_out: false,
        inside: Inside::Nothing,
    };
    let kitty = |cell| Graphics::Kitty { cell };
    assert_eq!(
        reply(None).detection(Some((9, 20))).graphics,
        kitty((9, 20))
    );
    assert_eq!(reply(None).detection(None).graphics, kitty((8, 16)));
    assert_eq!(
        reply(Some((0, 34))).detection(Some((0, 0))).graphics,
        kitty((8, 16))
    );
    assert_eq!(
        reply(Some((16, 9999))).detection(None).graphics,
        kitty((8, 16))
    );
    let timed_out = QueryReply {
        kitty: false,
        cell: None,
        timed_out: true,
        inside: Inside::Kitty,
    };
    let detection = timed_out.detection(None);
    assert_eq!(detection.graphics, Graphics::HalfBlocks);
    assert_eq!(
        detection.late,
        Some(LateReply {
            inside: Inside::Kitty
        })
    );
}

fn env(term: &str, program: Option<&str>, tmux: bool, kitty: bool) -> TermEnv {
    TermEnv {
        term: (!term.is_empty()).then(|| term.to_owned()),
        term_program: program.map(str::to_owned),
        tmux,
        kitty_window_id: kitty,
        colorterm: None,
    }
}

#[test]
fn only_kitty_capable_terminals_outside_multiplexers_are_queried() {
    // Kitty and Ghostty name themselves in TERM.
    assert!(env("xterm-kitty", None, false, false).may_query());
    assert!(env("xterm-ghostty", Some("ghostty"), false, false).may_query());
    // Ghostty with TERM overridden to a generic xterm.
    assert!(env("xterm-256color", Some("ghostty"), false, false).may_query());
    assert!(env("xterm-256color", None, false, true).may_query());
    // Inside tmux or screen: never, whatever is inherited.
    assert!(!env("xterm-ghostty", Some("ghostty"), true, false).may_query());
    assert!(!env("tmux-256color", Some("ghostty"), false, false).may_query());
    assert!(!env("screen-256color", None, false, true).may_query());
    // Alacritty started from Ghostty inherits TERM_PROGRAM=ghostty, but
    // names itself in TERM.
    assert!(!env("alacritty", Some("ghostty"), false, false).may_query());
    assert!(!env("alacritty", None, false, true).may_query());
    // Other terminals name themselves in TERM_PROGRAM.
    assert!(!env("xterm-256color", Some("Apple_Terminal"), false, false).may_query());
    assert!(!env("xterm-256color", Some("WezTerm"), false, true).may_query());
    assert!(!env("xterm-256color", None, false, false).may_query());
    assert!(!env("", None, false, false).may_query());
}

/// A pipe standing in for the terminal's input.
fn pipe() -> (OwnedFd, std::fs::File) {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: both were just opened and are owned here alone.
    unsafe {
        use std::os::fd::FromRawFd;
        (
            OwnedFd::from_raw_fd(fds[0]),
            std::fs::File::from_raw_fd(fds[1]),
        )
    }
}

#[test]
fn the_query_reads_until_da1() {
    let (input, mut terminal) = pipe();
    terminal.write_all(GHOSTTY).unwrap();
    let mut out = Vec::new();
    let reply = query(&mut out, input.as_raw_fd(), Duration::from_secs(5)).unwrap();
    assert_eq!(out, QUERY);
    assert!(reply.kitty && !reply.timed_out);
    assert_eq!(reply.cell, Some((16, 34)));
}

#[test]
fn a_slow_reply_in_pieces_is_waited_for() {
    let (input, mut terminal) = pipe();
    let writer = std::thread::spawn(move || {
        for chunk in GHOSTTY.chunks(5) {
            std::thread::sleep(Duration::from_millis(15));
            terminal.write_all(chunk).unwrap();
        }
    });
    let reply = query(&mut Vec::new(), input.as_raw_fd(), Duration::from_secs(5)).unwrap();
    writer.join().unwrap();
    assert!(reply.kitty && !reply.timed_out);
}

#[test]
fn no_reply_times_out_into_half_blocks() {
    let (input, _terminal) = pipe();
    let started = Instant::now();
    let reply = query(
        &mut Vec::new(),
        input.as_raw_fd(),
        Duration::from_millis(80),
    )
    .unwrap();
    let took = started.elapsed();
    assert!(reply.timed_out && !reply.kitty && reply.inside == Inside::Nothing);
    assert!(
        took >= Duration::from_millis(80) && took < Duration::from_secs(1),
        "{took:?}"
    );
    assert_eq!(reply.detection(None).graphics, Graphics::HalfBlocks);
}

#[test]
fn a_reply_that_has_started_is_read_past_the_first_deadline() {
    // Part of the reply in time, the rest after the 50 ms deadline.
    let (input, mut terminal) = pipe();
    let cut = GHOSTTY.len() - 4; // inside DA1
    terminal.write_all(&GHOSTTY[..cut]).unwrap();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        terminal.write_all(&GHOSTTY[cut..]).unwrap();
        terminal
    });
    let reply = query(
        &mut Vec::new(),
        input.as_raw_fd(),
        Duration::from_millis(50),
    )
    .unwrap();
    let _terminal = writer.join().unwrap();
    assert!(reply.kitty && !reply.timed_out, "{reply:?}");
    // A reply that never finishes still ends, at the cap, inside it.
    let (input, mut terminal) = pipe();
    terminal.write_all(b"\x1b_Gi=31;O").unwrap();
    let started = Instant::now();
    let reply = query(
        &mut Vec::new(),
        input.as_raw_fd(),
        Duration::from_millis(50),
    )
    .unwrap();
    assert!(reply.timed_out && reply.inside == Inside::Kitty);
    let took = started.elapsed();
    assert!(
        took >= REPLY_CAP && took < REPLY_CAP + Duration::from_secs(1),
        "{took:?}"
    );
}

#[test]
fn a_closed_terminal_is_an_error() {
    let (input, terminal) = pipe();
    drop(terminal);
    assert!(query(&mut Vec::new(), input.as_raw_fd(), Duration::from_secs(5)).is_err());
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

/// The keys crossterm makes of a late Kitty reply.
fn late_reply_keys() -> Vec<KeyEvent> {
    let mut keys = vec![key(KeyCode::Char('_'), KeyModifiers::ALT)];
    for c in "Gi=31;OK".chars() {
        let shift = if c.is_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        keys.push(key(KeyCode::Char(c), shift));
    }
    keys.push(key(KeyCode::Char('\\'), KeyModifiers::ALT));
    keys
}

#[test]
fn a_late_kitty_reply_is_dropped_once_and_other_keys_pass() {
    let now = Instant::now();
    let mut filter = LateReplyFilter::new(
        Some(LateReply {
            inside: Inside::Nothing,
        }),
        now,
    );
    let j = key(KeyCode::Char('j'), KeyModifiers::NONE);
    assert!(!filter.swallow(&j, now));
    for key in late_reply_keys() {
        assert!(filter.swallow(&key, now), "{key:?}");
    }
    // One reply only: afterwards everything passes, even Alt-_.
    assert!(!filter.swallow(&j, now));
    assert!(!filter.swallow(&late_reply_keys()[0], now));
    // Cut off inside the reply: the rest is dropped from the first key.
    let mut filter = LateReplyFilter::new(
        Some(LateReply {
            inside: Inside::Kitty,
        }),
        now,
    );
    for key in &late_reply_keys()[5..] {
        assert!(filter.swallow(key, now));
    }
    assert!(!filter.swallow(&j, now));
    // Not armed, or past its window: nothing is dropped.
    let mut filter = LateReplyFilter::new(None, now);
    assert!(!filter.swallow(&late_reply_keys()[0], now));
    let mut filter = LateReplyFilter::new(
        Some(LateReply {
            inside: Inside::Nothing,
        }),
        now,
    );
    let later = now + LATE_REPLY_WINDOW + Duration::from_secs(1);
    assert!(!filter.swallow(&late_reply_keys()[0], later));
    // A started reply ends at a pause.
    let mut filter = LateReplyFilter::new(
        Some(LateReply {
            inside: Inside::Kitty,
        }),
        now,
    );
    assert!(filter.swallow(&j, now));
    assert!(!filter.swallow(&j, now + LATE_REPLY_GAP + Duration::from_millis(1)));
    // A runaway bracket ends after a bounded number of keys.
    let mut filter = LateReplyFilter::new(
        Some(LateReply {
            inside: Inside::Kitty,
        }),
        now,
    );
    let swallowed = (0..1000).take_while(|_| filter.swallow(&j, now)).count();
    assert_eq!(swallowed, 64);
}

#[test]
fn a_reply_whose_escape_ended_a_read_is_still_dropped() {
    let now = Instant::now();
    let mut filter = LateReplyFilter::new(
        Some(LateReply {
            inside: Inside::Nothing,
        }),
        now,
    );
    let esc = key(KeyCode::Esc, KeyModifiers::NONE);
    // `ESC` alone, then `_Gi=31;OK`, then `ESC` alone again, then `\`.
    assert!(
        !filter.swallow(&esc, now),
        "the Esc itself cannot be known yet"
    );
    assert!(filter.swallow(&key(KeyCode::Char('_'), KeyModifiers::NONE), now));
    let keys = late_reply_keys();
    for key in &keys[1..keys.len() - 1] {
        assert!(filter.swallow(key, now), "{key:?}");
    }
    assert!(filter.swallow(&esc, now));
    assert!(filter.swallow(&key(KeyCode::Char('\\'), KeyModifiers::NONE), now));
    let j = key(KeyCode::Char('j'), KeyModifiers::NONE);
    assert!(!filter.swallow(&j, now));
    // A plain `_` without an `Esc` before it is a key.
    let mut filter = LateReplyFilter::new(
        Some(LateReply {
            inside: Inside::Nothing,
        }),
        now,
    );
    assert!(!filter.swallow(&key(KeyCode::Char('_'), KeyModifiers::NONE), now));
    assert!(!filter.swallow(&key(KeyCode::Char('G'), KeyModifiers::SHIFT), now));
}

#[test]
fn the_tail_of_a_cut_off_csi_reply_is_dropped() {
    let now = Instant::now();
    let plain = |c| key(KeyCode::Char(c), KeyModifiers::NONE);
    let j = plain('j');
    // DA1 cut after `ESC [ ? 62`: `;22;52c` follows.
    let mut filter = LateReplyFilter::new(
        Some(LateReply {
            inside: Inside::Csi,
        }),
        now,
    );
    for c in ";22;52c".chars() {
        assert!(filter.swallow(&plain(c), now), "{c}");
    }
    assert!(!filter.swallow(&j, now));
    assert!(!filter.swallow(&plain('2'), now), "digits are keys again");
    // Cut right after an `ESC`: `[?62c` follows, or `_Gi=31;…`.
    let mut filter = LateReplyFilter::new(
        Some(LateReply {
            inside: Inside::Escape,
        }),
        now,
    );
    for c in "[?62c".chars() {
        assert!(filter.swallow(&plain(c), now), "{c}");
    }
    assert!(!filter.swallow(&j, now));
    let mut filter = LateReplyFilter::new(
        Some(LateReply {
            inside: Inside::Escape,
        }),
        now,
    );
    assert!(filter.swallow(&plain('_'), now));
    assert!(filter.swallow(&key(KeyCode::Char('G'), KeyModifiers::SHIFT), now));
    // Anything else is a key.
    let mut filter = LateReplyFilter::new(
        Some(LateReply {
            inside: Inside::Csi,
        }),
        now,
    );
    assert!(!filter.swallow(&key(KeyCode::Up, KeyModifiers::NONE), now));
}

#[test]
fn a_users_own_alt_underscore_costs_no_other_key() {
    let now = Instant::now();
    let mut filter = LateReplyFilter::new(
        Some(LateReply {
            inside: Inside::Nothing,
        }),
        now,
    );
    let alt_underscore = key(KeyCode::Char('_'), KeyModifiers::ALT);
    assert!(filter.swallow(&alt_underscore, now));
    for c in "jk G".chars() {
        assert!(
            !filter.swallow(&key(KeyCode::Char(c), KeyModifiers::NONE), now),
            "{c}"
        );
    }
    // A partial prefix that breaks off lets the breaking key through.
    assert!(filter.swallow(&alt_underscore, now));
    assert!(filter.swallow(&key(KeyCode::Char('G'), KeyModifiers::SHIFT), now));
    assert!(!filter.swallow(&key(KeyCode::Char('x'), KeyModifiers::NONE), now));
    // The real reply still gets dropped afterwards.
    for key in late_reply_keys() {
        assert!(filter.swallow(&key, now), "{key:?}");
    }
}

#[test]
fn base64_matches_the_rfc_vectors() {
    for (input, output) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64(input.as_bytes()), output);
    }
    assert_eq!(base64(&[0xFF, 0xEF, 0x00]), "/+8A");
}

fn decode_base64(text: &str) -> Vec<u8> {
    const TABLE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = 0u32;
    let mut count = 0;
    let mut out = Vec::new();
    for c in text.chars().filter(|&c| c != '=') {
        bits = (bits << 6) | TABLE.find(c).unwrap() as u32;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    out
}

fn gradient(width: u32, height: u32) -> CoverImage {
    let rgb = (0..width * height * 3).map(|i| (i % 251) as u8).collect();
    CoverImage::new(width, height, rgb).unwrap()
}

#[test]
fn transmissions_are_chunked_quiet_virtual_placements() {
    let image = gradient(40, 40); // 4800 bytes, 6400 base64 characters
    let bytes = kitty_transmit(&image, 0x12_3456, 9, 4);
    let text = String::from_utf8(bytes).unwrap();
    let commands: Vec<&str> = text
        .split("\x1b_G")
        .skip(1)
        .map(|c| c.strip_suffix("\x1b\\").unwrap())
        .collect();
    assert_eq!(commands.len(), 2);
    assert_eq!(
        commands[0].split_once(';').unwrap().0,
        "a=T,U=1,q=2,f=24,i=1193046,s=40,v=40,c=9,r=4,m=1"
    );
    assert_eq!(commands[1].split_once(';').unwrap().0, "q=2,m=0");
    let mut payload = String::new();
    for command in &commands {
        let data = command.split_once(';').unwrap().1;
        assert!(data.len() <= KITTY_CHUNK);
        payload.push_str(data);
    }
    assert_eq!(commands[0].split_once(';').unwrap().1.len(), KITTY_CHUNK);
    assert_eq!(decode_base64(&payload), image.rgb());
    // A small image is one chunk.
    let one = String::from_utf8(kitty_transmit(&gradient(2, 2), 5, 1, 1)).unwrap();
    assert_eq!(one.matches("\x1b_G").count(), 1);
    assert!(one.contains(",m=0;"));
    assert_eq!(
        String::from_utf8(kitty_delete(77)).unwrap(),
        "\x1b_Ga=d,d=I,i=77,q=2\x1b\\"
    );
}

#[test]
fn placeholders_carry_row_and_column_and_are_one_cell_wide() {
    let cell = placeholder(2, 5).unwrap();
    let chars: Vec<char> = cell.chars().collect();
    assert_eq!(chars, [PLACEHOLDER, DIACRITICS[2], DIACRITICS[5]]);
    assert_eq!(cell.width(), 1);
    assert_eq!(placeholder(0, 0).unwrap().width(), 1);
    // Every row and column the table covers, and nothing past it.
    let last = DIACRITICS.len() as u16 - 1;
    let corner = placeholder(last, last).unwrap();
    assert_eq!(corner.width(), 1);
    assert!(corner.ends_with('\u{1D244}'));
    assert!(placeholder(last + 1, 0).is_none() && placeholder(0, last + 1).is_none());
    assert_eq!(id_colour(0x12_3456), Color::Rgb(0x12, 0x34, 0x56));
}

#[test]
fn the_diacritics_are_kittys_whole_table() {
    // `rowcolumn-diacritics.txt`: 297 entries from U+0305 to U+1D244, in
    // code point order, each a combining mark (zero width).
    assert_eq!(DIACRITICS.len(), 297);
    assert_eq!(DIACRITICS[0], '\u{305}');
    assert_eq!(DIACRITICS[1], '\u{30D}');
    assert_eq!(DIACRITICS[31], '\u{484}');
    assert_eq!(DIACRITICS[32], '\u{485}');
    assert_eq!(DIACRITICS[295], '\u{1D243}');
    assert_eq!(DIACRITICS[296], '\u{1D244}');
    assert!(DIACRITICS.windows(2).all(|pair| pair[0] < pair[1]));
    for c in DIACRITICS {
        assert_eq!(unicode_width::UnicodeWidthChar::width(c), Some(0), "{c:?}");
    }
}

#[test]
fn columns_make_a_square_on_screen() {
    let half = ArtPainter::new(Graphics::HalfBlocks, 1);
    assert_eq!(half.columns(4), 8);
    assert_eq!(half.columns(5), 10);
    let ghostty = ArtPainter::new(Graphics::Kitty { cell: (16, 34) }, 1);
    assert_eq!(ghostty.columns(4), 9);
    assert_eq!(ghostty.columns(5), 11);
    // Odd cell shapes stay within bounds.
    let tall = ArtPainter::new(Graphics::Kitty { cell: (1, 512) }, 1);
    assert_eq!(tall.columns(5), 20);
    let wide = ArtPainter::new(Graphics::Kitty { cell: (512, 1) }, 1);
    assert_eq!(wide.columns(5), 1);
}

fn frame(image: &Arc<CoverImage>, cols: u16, rows: u16) -> Option<ArtFrame> {
    Some(ArtFrame {
        image: image.clone(),
        picture: (image.fingerprint(), false),
        cols,
        rows,
    })
}

fn commands(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .split("\x1b_G")
        .skip(1)
        .map(|c| {
            let c = c.strip_suffix("\x1b\\").unwrap_or(c);
            c.split(';').next().unwrap().to_owned()
        })
        .collect()
}

#[test]
fn the_painter_sends_each_cover_once_and_deletes_the_old_one() {
    let mut painter = ArtPainter::new(Graphics::Kitty { cell: (16, 34) }, 0x0A_0000);
    let a = Arc::new(gradient(64, 64));
    let b = Arc::new(gradient(32, 32));
    painter.prepare(frame(&a, 9, 4));
    let first = commands(&painter.take_output());
    assert!(
        // 144 × 136 px of area, but a 64 px picture: sent at 64 × 60.
        first[0].starts_with("a=T,U=1,q=2,f=24,i=655360,s=64,v=60,c=9,r=4"),
        "{first:?}"
    );
    // The same frame again: nothing to send.
    painter.prepare(frame(&a, 9, 4));
    assert!(painter.take_output().is_empty());
    // Another cover: the old image is deleted first, the new one sent.
    painter.prepare(frame(&b, 9, 4));
    let change = commands(&painter.take_output());
    assert_eq!(change[0], "a=d,d=I,i=655360,q=2");
    assert!(
        change[1].starts_with("a=T,U=1,q=2,f=24,i=655361,"),
        "{change:?}"
    );
    // A new size (the visualizer turned on): sent again at that size.
    painter.prepare(frame(&b, 11, 5));
    let resized = commands(&painter.take_output());
    assert_eq!(resized[0], "a=d,d=I,i=655361,q=2");
    assert!(resized[1].contains("s=32,v=30,c=11,r=5"), "{resized:?}");
    // The art hides: just the delete.
    painter.prepare(None);
    assert_eq!(commands(&painter.take_output()), ["a=d,d=I,i=655362,q=2"]);
    painter.prepare(None);
    assert!(painter.take_output().is_empty());
    // At exit: the delete of what the terminal holds.
    painter.prepare(frame(&a, 9, 4));
    let _ = painter.take_output();
    assert_eq!(commands(&painter.finish()), ["a=d,d=I,i=655363,q=2"]);
    assert!(painter.finish().is_empty());
}

#[test]
fn an_identical_cover_is_not_sent_again() {
    let mut painter = ArtPainter::new(Graphics::Kitty { cell: (16, 34) }, 7);
    let a = Arc::new(gradient(16, 16));
    painter.prepare(frame(&a, 9, 4));
    assert!(!painter.take_output().is_empty());
    // Another image with the same pixels (the next track of the album).
    let same = Arc::new(gradient(16, 16));
    painter.prepare(frame(&same, 9, 4));
    assert!(painter.take_output().is_empty());
    let mut buf = Buffer::empty(Rect::new(0, 0, 9, 4));
    painter.draw(&mut buf, Rect::new(0, 0, 9, 4));
    assert_eq!(buf[(0, 0)].fg, id_colour(7), "still the image sent");
    // Different pixels are sent.
    let mut other = gradient(16, 16).rgb().to_vec();
    other[0] ^= 1;
    let other = Arc::new(CoverImage::new(16, 16, other).unwrap());
    painter.prepare(frame(&other, 9, 4));
    assert_eq!(commands(&painter.take_output())[0], "a=d,d=I,i=7,q=2");
}

#[test]
fn image_ids_stay_within_24_bits_and_skip_zero() {
    let mut painter = ArtPainter::new(Graphics::Kitty { cell: (8, 16) }, 0xFFFF_FFFF);
    let a = Arc::new(gradient(4, 4));
    let b = Arc::new(gradient(5, 5));
    painter.prepare(frame(&a, 2, 1));
    assert!(commands(&painter.take_output())[0].contains("i=16777215,"));
    painter.prepare(frame(&b, 2, 1));
    assert!(commands(&painter.take_output())[1].contains("i=1,"));
}

#[test]
fn kitty_cells_are_placeholders_coloured_with_the_id() {
    let mut painter = ArtPainter::new(Graphics::Kitty { cell: (16, 34) }, 0x01_0203);
    let area = Rect::new(2, 1, 3, 2);
    let mut buf = Buffer::empty(Rect::new(0, 0, 8, 4));
    // Nothing prepared: nothing drawn.
    painter.draw(&mut buf, area);
    assert_eq!(buf[(2, 1)].symbol(), " ");
    painter.prepare(frame(&Arc::new(gradient(8, 8)), 3, 2));
    painter.draw(&mut buf, area);
    for row in 0..2 {
        for col in 0..3 {
            let cell = &buf[(2 + col, 1 + row)];
            assert_eq!(cell.symbol(), placeholder(row, col).unwrap());
            assert_eq!(cell.fg, Color::Rgb(1, 2, 3));
        }
    }
    assert_eq!(buf[(5, 1)].symbol(), " ", "nothing outside the area");
    // An area of another size (a resize since `prepare`): not drawn.
    let mut other = Buffer::empty(Rect::new(0, 0, 8, 4));
    painter.draw(&mut other, Rect::new(0, 0, 4, 2));
    assert_eq!(other[(0, 0)].symbol(), " ");
}

#[test]
fn half_blocks_put_two_pixels_in_each_cell() {
    let mut painter = ArtPainter::new(Graphics::HalfBlocks, 1);
    // Top half red, bottom half blue.
    let mut rgb = [255, 0, 0].repeat(8);
    rgb.extend([0, 0, 255].repeat(8));
    let image = Arc::new(CoverImage::new(4, 4, rgb).unwrap());
    painter.prepare(frame(&image, 2, 1));
    assert!(painter.take_output().is_empty(), "no escape sequences");
    let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
    painter.draw(&mut buf, Rect::new(0, 0, 2, 1));
    for x in 0..2 {
        assert_eq!(buf[(x, 0)].symbol(), "▀");
        assert_eq!(buf[(x, 0)].fg, Color::Rgb(255, 0, 0));
        assert_eq!(buf[(x, 0)].bg, Color::Rgb(0, 0, 255));
    }
    assert!(painter.finish().is_empty(), "nothing to delete");
}

#[test]
fn half_blocks_use_the_palette_without_truecolor() {
    let truecolor = |v: Option<&str>| {
        TermEnv {
            colorterm: v.map(str::to_owned),
            ..TermEnv::default()
        }
        .truecolor()
    };
    assert!(truecolor(Some("truecolor")) && truecolor(Some("24bit")));
    assert!(!truecolor(None) && !truecolor(Some("yes")));
    assert_eq!(palette_index([0, 0, 0]), 16);
    assert_eq!(palette_index([255, 255, 255]), 231);
    assert_eq!(palette_index([255, 0, 0]), 196);
    assert_eq!(palette_index([95, 135, 175]), 16 + 36 + 12 + 3);
    assert_eq!(palette_index([128, 128, 128]), 244);
    let mut painter = ArtPainter::new(Graphics::HalfBlocks, 1).with_truecolor(false);
    let mut rgb = [255, 0, 0].repeat(8);
    rgb.extend([0, 0, 255].repeat(8));
    let image = Arc::new(CoverImage::new(4, 4, rgb).unwrap());
    painter.prepare(frame(&image, 2, 1));
    let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
    painter.draw(&mut buf, Rect::new(0, 0, 2, 1));
    assert_eq!(buf[(0, 0)].fg, Color::Indexed(196));
    assert_eq!(buf[(0, 0)].bg, Color::Indexed(21));
}

#[test]
fn transmissions_are_the_areas_pixel_size_up_to_the_cap() {
    // The bar's art in Ghostty at 2x.
    let big = LARGE_SOURCE;
    assert_eq!(transmit_size(9, 4, (16, 34), big), (144, 136));
    // A Now playing cover on a 50-row Ghostty window (9 × 19 cells).
    assert_eq!(transmit_size(63, 30, (9, 19), big), (567, 570));
    // A full-screen 5K window at 2x: scaled to the cap, same shape.
    assert_eq!(
        transmit_size(120, 60, (18, 38), big),
        (970, MAX_TRANSMIT_PIXELS)
    );
    assert_eq!(
        transmit_size(300, 1, (512, 512), big),
        (MAX_TRANSMIT_PIXELS, 3)
    );
    assert_eq!(transmit_size(0, 0, (9, 19), big), (1, 1));
}

const LARGE_SOURCE: u32 = 1024;

#[test]
fn transmissions_never_have_more_pixels_than_the_picture() {
    // The bar's 256 px cover enlarged on the screen: sent at 256 a side,
    // in the area's shape; the terminal scales it.
    assert_eq!(transmit_size(63, 30, (9, 19), 256), (254, 256));
    assert_eq!(
        transmit_size(9, 4, (16, 34), 256),
        (144, 136),
        "smaller area"
    );
    let mut painter = ArtPainter::new(Graphics::Kitty { cell: (9, 19) }, 1);
    painter.prepare(frame(&Arc::new(gradient(256, 256)), 63, 30));
    let bytes = painter.take_output();
    assert!(commands(&bytes)[0].contains("s=254,v=256,c=63,r=30"));
    assert!(bytes.len() < 254 * 256 * 4 + 2_000, "{}", bytes.len());
}

#[test]
fn one_screen_sized_transmission_costs_about_a_megabyte_and_a_quarter() {
    // What a Now playing cover sends, once per picture and size: RGB as
    // base64 (4 bytes per 3) in 4096-byte chunks of 14 bytes' framing.
    let mut painter = ArtPainter::new(Graphics::Kitty { cell: (9, 19) }, 1);
    let image = Arc::new(gradient(1024, 1024));
    painter.prepare(frame(&image, 63, 30));
    let bytes = painter.take_output();
    let payload = (567 * 570 * 3_usize).div_ceil(3) * 4;
    assert_eq!(payload, 1_292_760);
    let chunks = payload.div_ceil(KITTY_CHUNK);
    assert_eq!(chunks, 316);
    assert!(bytes.len() > payload && bytes.len() < payload + chunks * 16 + 64);
    assert!(commands(&bytes)[0].contains("s=567,v=570,c=63,r=30"));
    // At the cap: 1024 × 1024 is 4 MiB of base64.
    let mut painter = ArtPainter::new(Graphics::Kitty { cell: (18, 38) }, 1);
    painter.prepare(frame(&image, 57, 27));
    let bytes = painter.take_output();
    assert!(
        commands(&bytes)[0].contains("s=1024,v=1024,"),
        "{:?}",
        commands(&bytes)[0]
    );
    assert!(bytes.len() < 4 * 1024 * 1024 + 20_000);
}
