//! Cover art on the terminal: which protocol to use, the Kitty graphics
//! encoder with Unicode placeholders, and half-block art.
//!
//! - **Detection** ([`detect`]): only when the environment names a
//!   Kitty-capable terminal (Kitty or Ghostty) outside tmux or screen
//!   ([`TermEnv::may_query`]), the terminal is asked once at startup, after
//!   it is set up and before crossterm's event reader starts: a Kitty
//!   graphics query, the cell size (`CSI 16 t`) and DA1 (`CSI c`), which
//!   every terminal answers, so its reply ends the wait. The replies are
//!   read on the calling thread with `poll(2)`, up to [`QUERY_TIMEOUT`]
//!   for the first byte and [`REPLY_CAP`] in all once the terminal has
//!   started answering; anything else gets half blocks. A Kitty reply
//!   arriving after that would reach crossterm as keys (`Alt-_`, `G`, …,
//!   `Alt-\`): [`LateReplyFilter`] drops exactly that bracket for a while.
//! - **Kitty** ([`ArtPainter`] with [`Graphics::Kitty`]): each cover is
//!   transmitted once (`a=T,U=1,q=2`), before the frame and apart from it,
//!   as a virtual placement of the art's cells. The cells then hold
//!   placeholder characters (U+10EEEE with row and column diacritics, the
//!   image id in the foreground colour), so ratatui's diffing, overlays
//!   and clears treat the image like text: an overlay replaces the cells
//!   it covers, and a visualizer-only redraw never touches them. The old
//!   image is deleted (`a=d,d=I`) when the cover changes or the art hides,
//!   at exit, and in the panic hook ([`take_cleanup`]). Every command
//!   carries `q=2`, so the terminal never answers (an answer would reach
//!   the key reader).
//! - **Half blocks** everywhere else: `▀` with the top pixel as the
//!   foreground and the bottom one as the background, two pixels a cell.

use std::fmt::Write as _;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::cover::CoverImage;

/// How long [`detect`] waits for the terminal's replies (Ghostty answers in
/// well under a millisecond).
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(1);
/// Once any reply byte has arrived, [`query`] waits for the rest up to
/// this long after the query in all.
pub const REPLY_CAP: Duration = Duration::from_secs(3);
/// The query: a Kitty graphics query for a 1×1 RGB image (id 31), the cell
/// size in pixels, then DA1.
pub const QUERY: &[u8] = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[c";
/// The Kitty query's image id.
const QUERY_ID: &str = "31";
/// Cell sizes outside this range (pixels) are ignored.
const CELL_PIXELS: std::ops::RangeInclusive<u16> = 1..=512;
/// The cell size assumed when the terminal reports none (width, height).
const DEFAULT_CELL: (u16, u16) = (8, 16);
/// Base64 characters per Kitty chunk (the protocol's maximum).
pub const KITTY_CHUNK: usize = 4096;
/// The Unicode placeholder for Kitty images.
pub const PLACEHOLDER: char = '\u{10EEEE}';
/// Row and column diacritics, from Kitty's `rowcolumn-diacritics.txt`
/// (the first 32: art is at most a few rows and a few dozen columns).
pub const DIACRITICS: [char; 32] = [
    '\u{305}', '\u{30D}', '\u{30E}', '\u{310}', '\u{312}', '\u{33D}', '\u{33E}', '\u{33F}',
    '\u{346}', '\u{34A}', '\u{34B}', '\u{34C}', '\u{350}', '\u{351}', '\u{352}', '\u{357}',
    '\u{35B}', '\u{363}', '\u{364}', '\u{365}', '\u{366}', '\u{367}', '\u{368}', '\u{369}',
    '\u{36A}', '\u{36B}', '\u{36C}', '\u{36D}', '\u{36E}', '\u{36F}', '\u{483}', '\u{484}',
];
/// The largest side of a transmitted image, in pixels.
const MAX_TRANSMIT_PIXELS: u32 = 512;
/// Upper bound on buffered reply bytes while parsing.
const MAX_PENDING: usize = 4096;
/// How long after a timed-out query a late Kitty reply is filtered.
pub const LATE_REPLY_WINDOW: Duration = Duration::from_secs(30);
/// A late reply is at most this many keys long.
const MAX_LATE_REPLY_KEYS: usize = 64;

/// The Kitty image the terminal holds (0: none), for [`take_cleanup`].
static HELD_IMAGE: AtomicU32 = AtomicU32::new(0);

/// How art is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Graphics {
    /// The Kitty graphics protocol; `cell` is a cell's size in pixels
    /// (width, height).
    Kitty { cell: (u16, u16) },
    /// Half-block characters with colours.
    HalfBlocks,
}

/// The environment variables that decide whether to query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TermEnv {
    pub term: Option<String>,
    pub term_program: Option<String>,
    pub tmux: bool,
    pub kitty_window_id: bool,
    pub colorterm: Option<String>,
}

impl TermEnv {
    pub fn from_process() -> Self {
        let var = |name| std::env::var(name).ok().filter(|v: &String| !v.is_empty());
        Self {
            term: var("TERM"),
            term_program: var("TERM_PROGRAM"),
            tmux: var("TMUX").is_some(),
            kitty_window_id: var("KITTY_WINDOW_ID").is_some(),
            colorterm: var("COLORTERM"),
        }
    }

    /// `COLORTERM` says the terminal takes 24-bit colour.
    pub fn truecolor(&self) -> bool {
        matches!(self.colorterm.as_deref(), Some("truecolor" | "24bit"))
    }

    /// `true` if the environment names a Kitty-capable terminal, outside
    /// tmux or screen. `TERM` is set by each terminal (`xterm-kitty`,
    /// `xterm-ghostty`); `TERM_PROGRAM` and `KITTY_WINDOW_ID` are
    /// inherited by terminals started from another, so they count only
    /// while `TERM` is generic (`xterm…`) and, for the Kitty id, no other
    /// terminal names itself in `TERM_PROGRAM`.
    pub fn may_query(&self) -> bool {
        let term = self.term.as_deref().unwrap_or("");
        if self.tmux || term.starts_with("screen") || term.starts_with("tmux") {
            return false;
        }
        match term {
            "xterm-kitty" | "xterm-ghostty" => true,
            term if !term.is_empty() && !term.starts_with("xterm") => false,
            _ => match self.term_program.as_deref() {
                Some(program) => program.eq_ignore_ascii_case("ghostty"),
                None => self.kitty_window_id,
            },
        }
    }
}

/// What [`detect`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detection {
    pub graphics: Graphics,
    /// The terminal was queried (it may have printed the query; clear the
    /// screen).
    pub queried: bool,
    /// The query timed out: arm a [`LateReplyFilter`] with this.
    pub late: Option<LateReply>,
}

/// A query that timed out, and where in the replies it stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LateReply {
    pub inside: Inside,
}

/// Where the bytes read so far end.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Inside {
    /// Between replies.
    #[default]
    Nothing,
    /// Right after an `ESC`: the next byte decides.
    Escape,
    /// Inside the Kitty reply (`ESC _ …`).
    Kitty,
    /// Inside a `CSI` reply (the cell size or DA1).
    Csi,
}

/// Decides how to draw art: queries the terminal on stdout and the tty
/// when [`TermEnv::may_query`], else half blocks. Call after the terminal
/// is in raw mode and before any crossterm event reader exists. Never
/// fails: an error means half blocks.
pub fn detect(env: &TermEnv) -> Detection {
    let half_blocks = Detection {
        graphics: Graphics::HalfBlocks,
        queried: false,
        late: None,
    };
    if !env.may_query() {
        return half_blocks;
    }
    let tty = match Tty::open() {
        Ok(tty) => tty,
        Err(error) => {
            tracing::debug!(%error, "no tty to query; half-block art");
            return half_blocks;
        }
    };
    let mut stdout = io::stdout();
    let reply = match query(&mut stdout, tty.fd(), QUERY_TIMEOUT) {
        Ok(reply) => reply,
        Err(error) => {
            tracing::debug!(%error, "terminal query failed; half-block art");
            return Detection {
                queried: true,
                ..half_blocks
            };
        }
    };
    let detection = reply.detection(crossterm::terminal::window_size().ok().and_then(|size| {
        let width = size.width.checked_div(size.columns)?;
        let height = size.height.checked_div(size.rows)?;
        Some((width, height))
    }));
    match detection.graphics {
        Graphics::Kitty { cell } => {
            tracing::info!(
                cell_width = cell.0,
                cell_height = cell.1,
                "cover art: kitty graphics"
            );
        }
        Graphics::HalfBlocks if reply.timed_out => {
            tracing::debug!("terminal query timed out; half-block art");
        }
        Graphics::HalfBlocks => tracing::debug!("no kitty graphics; half-block art"),
    }
    detection
}

/// The terminal's input: stdin if it is a tty, else `/dev/tty`.
struct Tty {
    file: Option<std::fs::File>,
}

impl Tty {
    fn open() -> io::Result<Self> {
        // SAFETY: `isatty` only inspects the descriptor.
        if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
            return Ok(Self { file: None });
        }
        Ok(Self {
            file: Some(std::fs::File::open("/dev/tty")?),
        })
    }

    fn fd(&self) -> RawFd {
        self.file
            .as_ref()
            .map_or(libc::STDIN_FILENO, |f| f.as_raw_fd())
    }
}

/// The replies read by [`query`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryReply {
    pub kitty: bool,
    pub cell: Option<(u16, u16)>,
    pub timed_out: bool,
    /// Where the replies were cut off, if the deadline passed.
    pub inside: Inside,
}

impl QueryReply {
    /// Kitty only if the terminal said `OK`; the cell size from its reply,
    /// else `fallback` (from `TIOCGWINSZ`), else [`DEFAULT_CELL`].
    pub fn detection(&self, fallback: Option<(u16, u16)>) -> Detection {
        let valid =
            |cell: &(u16, u16)| CELL_PIXELS.contains(&cell.0) && CELL_PIXELS.contains(&cell.1);
        let cell = self
            .cell
            .filter(valid)
            .or(fallback.filter(valid))
            .unwrap_or(DEFAULT_CELL);
        Detection {
            graphics: if self.kitty {
                Graphics::Kitty { cell }
            } else {
                Graphics::HalfBlocks
            },
            queried: true,
            late: self.timed_out.then_some(LateReply {
                inside: self.inside,
            }),
        }
    }
}

/// Writes [`QUERY`] to `out` and reads replies from `input` on this
/// thread until DA1, or `timeout` if nothing arrives, or [`REPLY_CAP`]
/// after the query once something has.
pub fn query(out: &mut impl Write, input: RawFd, timeout: Duration) -> io::Result<QueryReply> {
    out.write_all(QUERY)?;
    out.flush()?;
    let started = Instant::now();
    let mut deadline = started + timeout;
    let mut parser = ReplyParser::default();
    let mut buf = [0u8; 1024];
    let mut timed_out = false;
    while !parser.done() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            timed_out = true;
            break;
        }
        let mut poll = libc::pollfd {
            fd: input,
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = libc::c_int::try_from(left.as_millis().max(1)).unwrap_or(libc::c_int::MAX);
        // SAFETY: one valid `pollfd` for the duration of the call.
        let ready = unsafe { libc::poll(&mut poll, 1, millis) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready == 0 {
            continue;
        }
        if poll.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0
            && poll.revents & libc::POLLIN == 0
        {
            return Err(io::Error::other("the terminal closed"));
        }
        // SAFETY: `buf` is valid for `buf.len()` bytes.
        let read = unsafe { libc::read(input, buf.as_mut_ptr().cast(), buf.len()) };
        if read < 0 {
            let error = io::Error::last_os_error();
            if matches!(
                error.kind(),
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
            ) {
                continue;
            }
            return Err(error);
        }
        if read == 0 {
            return Err(io::Error::other("the terminal closed"));
        }
        parser.feed(&buf[..read as usize]);
        // A terminal that has started answering finishes: reading the rest
        // here beats filtering it out of the keys later.
        deadline = deadline.max(started + REPLY_CAP);
    }
    Ok(QueryReply {
        kitty: parser.kitty,
        cell: parser.cell,
        timed_out,
        inside: if timed_out {
            parser.inside()
        } else {
            Inside::Nothing
        },
    })
}

/// Parses the replies to [`QUERY`], fed in pieces as they arrive. Bytes
/// that are not a complete escape sequence are skipped.
#[derive(Debug, Default)]
pub struct ReplyParser {
    pending: Vec<u8>,
    /// The Kitty query was answered `OK`.
    pub kitty: bool,
    /// The cell size reply (width, height).
    pub cell: Option<(u16, u16)>,
    da1: bool,
}

impl ReplyParser {
    /// DA1 has been answered: every earlier reply is in.
    pub fn done(&self) -> bool {
        self.da1
    }

    /// Where the bytes so far end.
    pub fn inside(&self) -> Inside {
        match self.pending.as_slice() {
            [] => Inside::Nothing,
            [0x1b] => Inside::Escape,
            [0x1b, b'_', ..] => Inside::Kitty,
            [0x1b, b'[', ..] => Inside::Csi,
            _ => Inside::Nothing,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        if self.da1 {
            return;
        }
        self.pending.extend_from_slice(bytes);
        loop {
            let Some(start) = self.pending.iter().position(|&b| b == 0x1b) else {
                self.pending.clear();
                return;
            };
            self.pending.drain(..start);
            match self.next_sequence() {
                Some(len) => {
                    self.pending.drain(..len);
                    if self.da1 {
                        self.pending.clear();
                        return;
                    }
                }
                None => {
                    if self.pending.len() > MAX_PENDING {
                        self.pending.clear();
                    }
                    return;
                }
            }
        }
    }

    /// Handles the escape sequence at the start of `pending`; its length,
    /// or `None` if it is incomplete.
    fn next_sequence(&mut self) -> Option<usize> {
        let bytes = &self.pending;
        match bytes.get(1)? {
            b'_' => {
                let end = bytes.windows(2).position(|w| w == b"\x1b\\")?;
                let body = bytes[2..end].to_vec();
                self.apc(&body);
                Some(end + 2)
            }
            b'[' => {
                let end = bytes[2..].iter().position(|b| (0x40..=0x7e).contains(b))? + 2;
                let params = bytes[2..end].to_vec();
                let final_byte = bytes[end];
                self.csi(&params, final_byte);
                Some(end + 1)
            }
            // Something else (`ESC x`): skip the escape.
            _ => Some(1),
        }
    }

    fn apc(&mut self, body: &[u8]) {
        let Some(body) = body.strip_prefix(b"G") else {
            return;
        };
        let Ok(body) = std::str::from_utf8(body) else {
            return;
        };
        let (params, message) = body.split_once(';').unwrap_or((body, ""));
        let ours = params.split(',').any(|p| p == format!("i={QUERY_ID}"));
        if ours {
            self.kitty = message == "OK";
        }
    }

    fn csi(&mut self, params: &[u8], final_byte: u8) {
        match final_byte {
            b'c' if params.starts_with(b"?") => self.da1 = true,
            b't' => {
                let Ok(params) = std::str::from_utf8(params) else {
                    return;
                };
                let mut parts = params.split(';');
                if parts.next() != Some("6") {
                    return;
                }
                let height = parts.next().and_then(|h| h.parse::<u16>().ok());
                let width = parts.next().and_then(|w| w.parse::<u16>().ok());
                if let (Some(width), Some(height), None) = (width, height, parts.next()) {
                    self.cell = Some((width, height));
                }
            }
            _ => {}
        }
    }
}

/// Drops a Kitty reply that arrives after [`detect`] gave up. crossterm
/// reports it as `Alt-_` (or, when the `ESC` ended a read, `Esc` then
/// `_`), the reply's characters, then `Alt-\` (or `Esc` then `\`).
///
/// Armed only after a timed-out query, for [`LATE_REPLY_WINDOW`], and for
/// one reply. A start must be followed by exactly `Gi=31;`: on any other
/// key the filter lets it through and keeps watching, so a user's own
/// `Alt-_` costs at most that key. A started reply is swallowed until its
/// terminator, [`MAX_LATE_REPLY_KEYS`] keys, or [`LATE_REPLY_GAP`] without
/// a key. The `Esc` of a split start reaches the app (the filter cannot
/// know yet); reading a started reply to its end in [`query`] makes that
/// rare. If the query stopped inside a `CSI` reply (the cell size or
/// DA1), its remaining parameters and final byte are dropped too.
#[derive(Debug, Clone, Default)]
pub struct LateReplyFilter {
    state: FilterState,
    /// Until when a reply may start.
    until: Option<Instant>,
    /// When the last key was swallowed.
    last: Option<Instant>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum FilterState {
    #[default]
    Off,
    /// Waiting for a reply to start; `esc`: the last key was `Esc`.
    Watching { esc: bool },
    /// Started; this many characters of [`LATE_REPLY_PREFIX`] matched.
    Prefix(usize),
    /// In the reply's message; this many keys swallowed.
    Body { keys: usize, esc: bool },
    /// The query stopped right after an `ESC`.
    Escape,
    /// The query stopped inside a `CSI` reply: its parameters and final
    /// byte are still to come.
    Csi,
}

/// What follows `ESC _` in a reply to [`QUERY`].
const LATE_REPLY_PREFIX: [char; 6] = ['G', 'i', '=', '3', '1', ';'];
/// A started reply ends when no key comes for this long.
pub const LATE_REPLY_GAP: Duration = Duration::from_secs(1);

impl LateReplyFilter {
    /// A filter for the query `late` describes (none: never filters).
    pub fn new(late: Option<LateReply>, now: Instant) -> Self {
        match late {
            None => Self::default(),
            Some(late) => Self {
                state: match late.inside {
                    Inside::Nothing => FilterState::Watching { esc: false },
                    Inside::Escape => FilterState::Escape,
                    Inside::Kitty => FilterState::Body {
                        keys: 0,
                        esc: false,
                    },
                    Inside::Csi => FilterState::Csi,
                },
                until: Some(now + LATE_REPLY_WINDOW),
                last: Some(now),
            },
        }
    }

    /// `true` if `key` belongs to a late reply and must be ignored.
    pub fn swallow(&mut self, key: &KeyEvent, now: Instant) -> bool {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let char_is = |c| key.code == KeyCode::Char(c);
        let plain = |c| char_is(c) && !alt;
        let in_reply = !matches!(self.state, FilterState::Off | FilterState::Watching { .. });
        if in_reply && self.last.is_some_and(|last| now > last + LATE_REPLY_GAP) {
            // A pause: whatever this was, it is over.
            self.state = FilterState::Watching { esc: false };
        }
        if matches!(self.state, FilterState::Watching { .. })
            && self.until.is_some_and(|until| now > until)
        {
            self.state = FilterState::Off;
        }
        let (state, swallowed) = match self.state {
            FilterState::Off => (FilterState::Off, false),
            FilterState::Watching { esc } => {
                if (char_is('_') && alt) || (esc && plain('_')) {
                    (FilterState::Prefix(0), true)
                } else {
                    let esc = key.code == KeyCode::Esc && !alt;
                    (FilterState::Watching { esc }, false)
                }
            }
            FilterState::Prefix(n) => {
                // Uppercase letters come with Shift.
                if char_is(LATE_REPLY_PREFIX[n]) && !alt {
                    if n + 1 == LATE_REPLY_PREFIX.len() {
                        (
                            FilterState::Body {
                                keys: 0,
                                esc: false,
                            },
                            true,
                        )
                    } else {
                        (FilterState::Prefix(n + 1), true)
                    }
                } else {
                    // Not a reply (a user's own Alt-_): let it through.
                    (FilterState::Watching { esc: false }, false)
                }
            }
            FilterState::Escape => {
                if plain('_') {
                    (FilterState::Prefix(0), true)
                } else if plain('[') {
                    (FilterState::Csi, true)
                } else {
                    let esc = key.code == KeyCode::Esc && !alt;
                    (FilterState::Watching { esc }, false)
                }
            }
            FilterState::Csi => match key.code {
                KeyCode::Char('0'..='9' | ';' | ':' | '?') if !alt => (FilterState::Csi, true),
                // The final byte ends it; every reply before it is in.
                KeyCode::Char('@'..='~') if !alt => (FilterState::Off, true),
                _ => (FilterState::Watching { esc: false }, false),
            },
            FilterState::Body { keys, esc } => {
                let end = (char_is('\\') && alt) || (esc && plain('\\'));
                if end || keys + 1 >= MAX_LATE_REPLY_KEYS {
                    (FilterState::Off, true)
                } else {
                    let esc = key.code == KeyCode::Esc && !alt;
                    (
                        FilterState::Body {
                            keys: keys + 1,
                            esc,
                        },
                        true,
                    )
                }
            }
        };
        self.state = state;
        if swallowed {
            self.last = Some(now);
        }
        swallowed
    }
}

/// Base64 (standard alphabet, padded).
pub fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        out.push(char::from(TABLE[(n >> 18) as usize & 63]));
        out.push(char::from(TABLE[(n >> 12) as usize & 63]));
        out.push(if chunk.len() > 1 {
            char::from(TABLE[(n >> 6) as usize & 63])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(TABLE[n as usize & 63])
        } else {
            '='
        });
    }
    out
}

/// The Kitty commands that transmit `image` as image `id` (RGB, `f=24`)
/// and create a virtual placement of `cols` × `rows` cells (`U=1`), in
/// chunks of at most [`KITTY_CHUNK`] base64 characters, all quiet (`q=2`).
pub fn kitty_transmit(image: &CoverImage, id: u32, cols: u16, rows: u16) -> Vec<u8> {
    let payload = base64(image.rgb());
    let chunks: Vec<&str> = payload
        .as_bytes()
        .chunks(KITTY_CHUNK)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect();
    let mut out = String::with_capacity(payload.len() + chunks.len() * 16 + 64);
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        if i == 0 {
            let _ = write!(
                out,
                "\x1b_Ga=T,U=1,q=2,f=24,i={id},s={},v={},c={cols},r={rows},m={more};",
                image.width(),
                image.height()
            );
        } else {
            let _ = write!(out, "\x1b_Gq=2,m={more};");
        }
        out.push_str(chunk);
        out.push_str("\x1b\\");
    }
    out.into_bytes()
}

/// The Kitty command that deletes image `id` and frees its data.
pub fn kitty_delete(id: u32) -> Vec<u8> {
    format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\").into_bytes()
}

/// The placeholder cell at `row`, `col` of an image: U+10EEEE with the row
/// and column diacritics. `None` past the diacritics table.
pub fn placeholder(row: u16, col: u16) -> Option<String> {
    let row = DIACRITICS.get(usize::from(row))?;
    let col = DIACRITICS.get(usize::from(col))?;
    Some([PLACEHOLDER, *row, *col].iter().collect())
}

/// The foreground colour that names image `id` (24 bits).
pub fn id_colour(id: u32) -> Color {
    Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8)
}

/// The delete for the Kitty image the terminal holds, if any, which is
/// then forgotten. For the panic hook, which cannot reach the painter.
pub fn take_cleanup() -> Option<Vec<u8>> {
    match HELD_IMAGE.swap(0, Ordering::SeqCst) {
        0 => None,
        id => Some(kitty_delete(id)),
    }
}

impl ArtPainter {
    fn colour(&self, [r, g, b]: [u8; 3]) -> Color {
        if self.truecolor {
            Color::Rgb(r, g, b)
        } else {
            Color::Indexed(palette_index([r, g, b]))
        }
    }
}

/// The nearest colour of the xterm 256-colour palette's colour cube
/// (16–231) or grey ramp (232–255); the first 16 vary by terminal.
pub fn palette_index([r, g, b]: [u8; 3]) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let nearest_level = |v: u8| {
        (0..6)
            .min_by_key(|&i| (i32::from(LEVELS[i]) - i32::from(v)).abs())
            .unwrap_or(0)
    };
    let (ri, gi, bi) = (nearest_level(r), nearest_level(g), nearest_level(b));
    let cube = [LEVELS[ri], LEVELS[gi], LEVELS[bi]];
    let average = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let grey_step = (average.saturating_sub(8) / 10).min(23) as u8;
    let grey_level = 8 + 10 * grey_step;
    let distance = |c: [u8; 3]| {
        [r, g, b]
            .iter()
            .zip(c)
            .map(|(&a, b)| (i32::from(a) - i32::from(b)).pow(2))
            .sum::<i32>()
    };
    if distance([grey_level; 3]) < distance(cube) {
        232 + grey_step
    } else {
        16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8
    }
}

/// A hash of an image's size and pixels, to notice an identical cover
/// that came as another image.
fn pixels_hash(image: &CoverImage) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    (image.width(), image.height(), image.rgb()).hash(&mut hasher);
    hasher.finish()
}

/// What the art area shows in the next frame.
#[derive(Debug, Clone)]
pub struct ArtFrame {
    pub image: Arc<CoverImage>,
    pub cols: u16,
    pub rows: u16,
}

/// Turns the cover into terminal output; lives across frames. Call
/// [`ArtPainter::prepare`] and write [`ArtPainter::take_output`] before
/// each full draw, then [`ArtPainter::draw`] from it.
#[derive(Debug)]
pub struct ArtPainter {
    graphics: Graphics,
    /// The next Kitty image id (24 bits, never 0).
    next_id: u32,
    /// The Kitty image the terminal holds for `frame`.
    sent: Option<u32>,
    /// What the next frame draws.
    frame: Option<ArtFrame>,
    /// The half-block pixels for `frame` (two per cell, top and bottom).
    blocks: Option<CoverImage>,
    /// Half blocks in 24-bit colour; else the 256-colour palette.
    truecolor: bool,
    output: Vec<u8>,
}

impl ArtPainter {
    /// `first_id` seeds Kitty image ids (any value; reduced to 24 bits).
    pub fn new(graphics: Graphics, first_id: u32) -> Self {
        Self {
            graphics,
            next_id: (first_id & 0xFF_FFFF).max(1),
            sent: None,
            frame: None,
            blocks: None,
            truecolor: true,
            output: Vec::new(),
        }
    }

    /// Whether half blocks may use 24-bit colour ([`TermEnv::truecolor`]);
    /// without it they use the nearest of the 256-colour palette.
    pub fn with_truecolor(mut self, truecolor: bool) -> Self {
        self.truecolor = truecolor;
        self
    }

    pub fn graphics(&self) -> Graphics {
        self.graphics
    }

    /// Columns of a square-looking art area `rows` cells high.
    pub fn columns(&self, rows: u16) -> u16 {
        match self.graphics {
            Graphics::HalfBlocks => rows.saturating_mul(2),
            Graphics::Kitty {
                cell: (width, height),
            } => {
                let width = u32::from(width.max(1));
                let cols = (u32::from(rows) * u32::from(height) * 2 + width) / (2 * width);
                let max = u32::from(rows)
                    .saturating_mul(4)
                    .min(DIACRITICS.len() as u32);
                cols.clamp(1, max.max(1)) as u16
            }
        }
    }

    /// Sets what the next frame shows (`None`: no art) and queues what the
    /// terminal must be sent first: the old Kitty image's delete when it
    /// changes or hides, and the new one's transmission.
    pub fn prepare(&mut self, frame: Option<ArtFrame>) {
        let unchanged = match (&self.frame, &frame) {
            (Some(old), Some(new)) => {
                (old.cols, old.rows) == (new.cols, new.rows)
                    && (Arc::ptr_eq(&old.image, &new.image)
                        || pixels_hash(&old.image) == pixels_hash(&new.image))
            }
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            // The same picture (say, the next track of an album shown
            // straight from memory): keep what the terminal has.
            self.frame = frame;
            return;
        }
        self.blocks = None;
        self.frame = frame;
        let Graphics::Kitty { cell } = self.graphics else {
            if let Some(frame) = &self.frame {
                self.blocks = Some(
                    frame
                        .image
                        .resized(u32::from(frame.cols), u32::from(frame.rows) * 2),
                );
            }
            return;
        };
        if let Some(old) = self.sent.take() {
            self.output.extend_from_slice(&kitty_delete(old));
            HELD_IMAGE.store(0, Ordering::SeqCst);
        }
        let Some(frame) = &self.frame else {
            return;
        };
        let id = self.next_id;
        self.next_id = match (self.next_id + 1) & 0xFF_FFFF {
            0 => 1,
            next => next,
        };
        // Exactly the area's pixel size (capped), so the image fills its
        // cells whether or not the terminal keeps the aspect ratio.
        let width = (u32::from(frame.cols) * u32::from(cell.0)).clamp(1, MAX_TRANSMIT_PIXELS);
        let height = (u32::from(frame.rows) * u32::from(cell.1)).clamp(1, MAX_TRANSMIT_PIXELS);
        let pixels = frame.image.resized(width, height);
        self.output
            .extend_from_slice(&kitty_transmit(&pixels, id, frame.cols, frame.rows));
        HELD_IMAGE.store(id, Ordering::SeqCst);
        self.sent = Some(id);
    }

    /// The bytes to write before the next frame (deletes, transmissions).
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }

    /// The bytes to write at exit: the delete of the image the terminal
    /// holds, if any.
    pub fn finish(&mut self) -> Vec<u8> {
        self.frame = None;
        self.blocks = None;
        let mut out = self.take_output();
        if let Some(sent) = self.sent.take() {
            out.extend_from_slice(&kitty_delete(sent));
            HELD_IMAGE.store(0, Ordering::SeqCst);
        }
        out
    }

    /// Draws the prepared art into `area` (its size must match what was
    /// prepared, else nothing is drawn, e.g. right after a resize).
    pub fn draw(&self, buf: &mut Buffer, area: Rect) {
        let Some(frame) = &self.frame else {
            return;
        };
        if (area.width, area.height) != (frame.cols, frame.rows) {
            return;
        }
        match (&self.graphics, &self.sent, &self.blocks) {
            (Graphics::Kitty { .. }, Some(sent), _) => {
                let colour = id_colour(*sent);
                for row in 0..area.height {
                    for col in 0..area.width {
                        let (Some(symbol), Some(cell)) = (
                            placeholder(row, col),
                            buf.cell_mut((area.x + col, area.y + row)),
                        ) else {
                            continue;
                        };
                        cell.reset();
                        cell.set_symbol(&symbol).set_fg(colour);
                    }
                }
            }
            (Graphics::HalfBlocks, _, Some(blocks)) => {
                for row in 0..area.height {
                    for col in 0..area.width {
                        let Some(cell) = buf.cell_mut((area.x + col, area.y + row)) else {
                            continue;
                        };
                        let top = blocks.pixel(u32::from(col), u32::from(row) * 2);
                        let bottom = blocks.pixel(u32::from(col), u32::from(row) * 2 + 1);
                        cell.reset();
                        cell.set_char('▀')
                            .set_fg(self.colour(top))
                            .set_bg(self.colour(bottom));
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
