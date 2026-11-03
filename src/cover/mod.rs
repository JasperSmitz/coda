//! Cover art for the track heard, without any terminal code.
//!
//! - **Images** ([`CoverImage`]) are plain RGB pixels, square, at most
//!   [`COVER_SIZE`] pixels a side for the now-playing bar, or
//!   [`LARGE_COVER_SIZE`] for the Now playing screen ([`CoverSize`]). [`decode`] makes them from untrusted
//!   bytes (JPEG, PNG or WebP, recognised by their first bytes) within
//!   byte and pixel limits, so a decompression bomb is refused before it
//!   is allocated. Decoding runs on blocking threads, never on the loop.
//! - **Sources:** a local track's art is read from its tags or its folder
//!   ([`local`]); a Navidrome track's is fetched with `getCoverArt` and
//!   kept in a size-capped disk cache ([`cache`]).
//! - **State** ([`Covers`]) follows the audible track (the player's
//!   `playing_entry`). It asks for one cover at a time, remembers a few
//!   recent results in memory, and drops results for a track no longer
//!   heard (they are only remembered) and Navidrome results from before
//!   a client swap (by [`Generation`]). While the Now playing screen is
//!   open ([`Covers::set_large`]) it also loads the large variant of the
//!   cover shown, after the small one, and keeps the last
//!   [`LARGE_MEMORY_ENTRIES`] of those.
//!
//! Nothing here logs a path, URL, title or cover id.

pub mod cache;
pub mod local;

use std::collections::VecDeque;
use std::fmt;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::{DynamicImage, GenericImageView, ImageFormat, ImageReader, Limits};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::model::{Track, TrackSource};
use crate::navidrome::{self, browse::Generation};
use crate::setup::Reply;

/// The largest side of a decoded cover for the now-playing bar, in
/// pixels. Also the `size` asked of `getCoverArt`, and the size of cached
/// files.
pub const COVER_SIZE: u32 = 256;
/// The largest side of the Now playing screen's cover (the large
/// variant), in pixels; cached apart from the small ones.
pub const LARGE_COVER_SIZE: u32 = 1024;
/// Images wider or taller than this are refused before decoding.
pub const MAX_DIMENSION: u32 = 4096;
/// Upper bound on what one decode may allocate (the output image counts).
pub const MAX_DECODE_ALLOC: u64 = 64 * 1024 * 1024;
/// Upper bound on the bytes handed to [`decode`] (embedded pictures and
/// folder images; `getCoverArt` bodies are capped lower).
pub const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
/// How long the previous track's cover may stay while the next loads.
pub const LINGER: Duration = Duration::from_secs(1);
/// Recent results kept in memory (found or not): about 3 MB of pixels.
pub const MEMORY_ENTRIES: usize = 16;
/// Large covers kept in memory: about 3 MB of pixels each.
pub const LARGE_MEMORY_ENTRIES: usize = 2;

/// Which variant of a cover to load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CoverSize {
    /// For the now-playing bar: [`COVER_SIZE`].
    #[default]
    Small,
    /// For the Now playing screen: [`LARGE_COVER_SIZE`].
    Large,
}

impl CoverSize {
    /// The largest side, in pixels.
    pub fn pixels(self) -> u32 {
        match self {
            Self::Small => COVER_SIZE,
            Self::Large => LARGE_COVER_SIZE,
        }
    }
}

/// A decoded cover: `width` × `height` RGB pixels, row by row.
#[derive(Clone, PartialEq, Eq)]
pub struct CoverImage {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

impl fmt::Debug for CoverImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CoverImage({}x{})", self.width, self.height)
    }
}

impl CoverImage {
    /// `None` unless `rgb` holds exactly `width` × `height` pixels and
    /// neither side is zero.
    pub fn new(width: u32, height: u32, rgb: Vec<u8>) -> Option<Self> {
        let len = u64::from(width) * u64::from(height) * 3;
        (width > 0 && height > 0 && rgb.len() as u64 == len).then_some(Self { width, height, rgb })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixels, three bytes each, row by row.
    pub fn rgb(&self) -> &[u8] {
        &self.rgb
    }

    /// A hash of the size and pixels, to notice an identical picture that
    /// came as another image (say, every track of an album).
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        (self.width, self.height, &self.rgb).hash(&mut hasher);
        hasher.finish()
    }

    /// The pixel at (`x`, `y`), clamped to the image.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let x = x.min(self.width - 1) as usize;
        let y = y.min(self.height - 1) as usize;
        let i = (y * self.width as usize + x) * 3;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }

    /// The image at `width` × `height` (each at least 1): every output
    /// pixel is the average of the source pixels it covers (the nearest
    /// one when enlarging). Cheap at cover sizes, so it may run on the
    /// loop.
    pub fn resized(&self, width: u32, height: u32) -> CoverImage {
        let (width, height) = (width.max(1), height.max(1));
        let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
        for y in 0..height {
            let (y0, y1) = span(y, height, self.height);
            for x in 0..width {
                let (x0, x1) = span(x, width, self.width);
                let mut sum = [0u64; 3];
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let p = self.pixel(sx, sy);
                        for c in 0..3 {
                            sum[c] += u64::from(p[c]);
                        }
                    }
                }
                let n = u64::from((x1 - x0) * (y1 - y0));
                for total in sum {
                    rgb.push((total / n) as u8);
                }
            }
        }
        CoverImage { width, height, rgb }
    }
}

/// The source pixels `[start, end)` that output pixel `i` of `out` covers
/// in a source of `src` pixels; never empty.
fn span(i: u32, out: u32, src: u32) -> (u32, u32) {
    let start = (u64::from(i) * u64::from(src) / u64::from(out)) as u32;
    let end = (u64::from(i + 1) * u64::from(src)).div_ceil(u64::from(out)) as u32;
    (start.min(src - 1), end.clamp(start.min(src - 1) + 1, src))
}

/// The image formats Coda decodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    WebP,
}

impl From<Format> for ImageFormat {
    fn from(format: Format) -> Self {
        match format {
            Format::Jpeg => ImageFormat::Jpeg,
            Format::Png => ImageFormat::Png,
            Format::WebP => ImageFormat::WebP,
        }
    }
}

/// The format `bytes` start with, if Coda decodes it. Never trusts a
/// `Content-Type`, MIME type or file extension.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    match bytes {
        [0xFF, 0xD8, 0xFF, ..] => Some(Format::Jpeg),
        [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, ..] => Some(Format::Png),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some(Format::WebP),
        _ => None,
    }
}

/// Why bytes did not become a cover. Carries no image data, so it is safe
/// to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("not a JPEG, PNG or WebP image")]
    NotAnImage,
    #[error("image over the size limits")]
    TooLarge,
    #[error("malformed image")]
    Malformed,
}

/// Decodes untrusted image bytes into a square cover of at most
/// [`COVER_SIZE`] pixels; see [`decode_at`].
pub fn decode(bytes: &[u8]) -> Result<CoverImage, DecodeError> {
    decode_at(bytes, COVER_SIZE)
}

/// Decodes untrusted image bytes into a square cover of at most `size`
/// pixels (the centre of a non-square image). Refuses more
/// than [`MAX_IMAGE_BYTES`] and sides over [`MAX_DIMENSION`] before
/// decoding; [`MAX_DECODE_ALLOC`] bounds what the decoder allocates, the
/// output image included, which is checked before it is allocated. At
/// most one decode runs at a time ([`DECODE_GATE`]), so loads that were
/// superseded (their blocking work still finishes) cannot stack decodes.
/// Blocking.
pub fn decode_at(bytes: &[u8], size: u32) -> Result<CoverImage, DecodeError> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(DecodeError::TooLarge);
    }
    let format = sniff(bytes).ok_or(DecodeError::NotAnImage)?;
    let _gate = DECODE_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format.into());
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let image = reader.decode().map_err(|error| match error {
        image::ImageError::Limits(_) => DecodeError::TooLarge,
        _ => DecodeError::Malformed,
    })?;
    square(image, size.max(1)).ok_or(DecodeError::Malformed)
}

/// Serialises [`decode`].
static DECODE_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The centred square of `image`, scaled down to at most `size` pixels,
/// read through a view (no copy of the crop). RGB images (JPEG, most
/// PNGs) are not copied for the conversion either.
fn square(image: DynamicImage, size: u32) -> Option<CoverImage> {
    let rgb = image.into_rgb8();
    let side = rgb.width().min(rgb.height());
    if side == 0 {
        return None;
    }
    let x = (rgb.width() - side) / 2;
    let y = (rgb.height() - side) / 2;
    let view = rgb.view(x, y, side, side);
    let small = if side > size {
        image::imageops::thumbnail(&*view, size, size)
    } else {
        view.to_image()
    };
    CoverImage::new(small.width(), small.height(), small.into_raw())
}

/// Which cover a track has. Its `Debug` hides the path and id.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum CoverKey {
    /// A local file: its tags, else an image in its folder.
    Local(PathBuf),
    /// A Navidrome `coverArt` id, for the current client.
    Navidrome(String),
}

impl fmt::Debug for CoverKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local(_) => f.write_str("CoverKey::Local(<path>)"),
            Self::Navidrome(_) => f.write_str("CoverKey::Navidrome(<id>)"),
        }
    }
}

impl CoverKey {
    /// The cover to show for `track`: its file for a local track, its
    /// `coverArt` id for a Navidrome track (none without one).
    pub fn for_track(track: &Track) -> Option<Self> {
        match &track.source {
            TrackSource::Local { path } => Some(Self::Local(path.clone())),
            TrackSource::Navidrome { .. } => track.cover_art.clone().map(Self::Navidrome),
        }
    }

    /// `true` if this is [`CoverKey::for_track`] of `track`.
    pub fn is_for(&self, track: &Track) -> bool {
        match (self, &track.source) {
            (Self::Local(path), TrackSource::Local { path: other }) => path == other,
            (Self::Navidrome(id), TrackSource::Navidrome { .. }) => {
                track.cover_art.as_deref() == Some(id.as_str())
            }
            _ => false,
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Navidrome(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoverRequestId(pub u64);

/// One cover to load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverRequest {
    pub id: CoverRequestId,
    /// The client generation a Navidrome cover is fetched with.
    pub generation: Generation,
    pub key: CoverKey,
    pub size: CoverSize,
}

/// How loading a cover went.
#[derive(Debug, Clone)]
pub enum Outcome {
    Art(Arc<CoverImage>),
    /// There is none, for certain (no picture, no such art, Navidrome's
    /// placeholder, bytes that are not an image): remembered.
    NoArt,
    /// It could not be loaded this time (network, cancelled): not
    /// remembered, so the next time the track is heard asks again.
    Failed,
}

#[derive(Debug, Clone)]
pub struct CoverResponse {
    pub id: CoverRequestId,
    pub generation: Generation,
    pub key: CoverKey,
    pub size: CoverSize,
    pub outcome: Outcome,
}

/// A loaded cover and its [`CoverImage::fingerprint`].
#[derive(Debug, Clone)]
struct Art {
    image: Arc<CoverImage>,
    fingerprint: u64,
}

impl Art {
    fn new(image: Arc<CoverImage>) -> Self {
        let fingerprint = image.fingerprint();
        Self { image, fingerprint }
    }
}

/// The cover state; see the module docs.
#[derive(Debug)]
pub struct Covers {
    enabled: bool,
    generation: Generation,
    /// The audible track's cover.
    wanted: Option<CoverKey>,
    /// The wanted cover, once loaded.
    shown: Option<Art>,
    /// The previous track's cover, kept on screen while the wanted one
    /// loads (from when), for at most [`LINGER`].
    lingering: Option<(Art, Instant)>,
    /// The request for the wanted cover, while it runs.
    loading: Option<CoverRequestId>,
    /// The last settled cover was an image: while the next one loads,
    /// [`Covers::reserve`] keeps its place so the bar does not jump.
    had_art: bool,
    /// Recent results, newest last.
    memory: VecDeque<(CoverKey, Option<Art>)>,
    /// Large covers are wanted (the Now playing screen is open).
    large_wanted: bool,
    /// Recent large covers by the fingerprint of the small cover they
    /// enlarge, newest last. Keyed by the picture rather than the track,
    /// so the tracks of an album share one.
    large: VecDeque<(u64, Arc<CoverImage>)>,
    /// The large load running, and the fingerprint it is for.
    large_loading: Option<(CoverRequestId, u64)>,
    /// The fingerprint whose large load found nothing or failed: not
    /// asked again until the track changes or the screen opens again.
    large_settled: Option<u64>,
    next_id: u64,
    requests: Vec<CoverRequest>,
}

impl Covers {
    /// With `enabled` false, nothing is ever requested or shown.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            generation: Generation::default(),
            wanted: None,
            shown: None,
            lingering: None,
            loading: None,
            had_art: false,
            memory: VecDeque::new(),
            large_wanted: false,
            large: VecDeque::new(),
            large_loading: None,
            large_settled: None,
            next_id: 0,
            requests: Vec::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The cover to show: the track heard's, once loaded; meanwhile, for
    /// at most [`LINGER`] after a track change, the previous track's (so
    /// the tracks of an album do not blank at each change; the new cover
    /// replaces it as soon as its result arrives, and "no art" removes it).
    pub fn image(&self) -> Option<&Arc<CoverImage>> {
        self.art().map(|art| &art.image)
    }

    fn art(&self) -> Option<&Art> {
        self.shown
            .as_ref()
            .or(self.lingering.as_ref().map(|(art, _)| art))
    }

    /// The cover for the Now playing screen: the large variant of
    /// [`Covers::image`] once it has loaded, else the small one (shown
    /// enlarged meanwhile, never a blank).
    pub fn large_image(&self) -> Option<&Arc<CoverImage>> {
        self.large_picture().map(|(image, _)| image)
    }

    /// [`Covers::image`] and a name for its picture: the fingerprint of the
    /// small cover, and `false`. Equal names mean equal pixels, without
    /// hashing on the loop.
    pub fn picture(&self) -> Option<(&Arc<CoverImage>, (u64, bool))> {
        self.art().map(|art| (&art.image, (art.fingerprint, false)))
    }

    /// [`Covers::large_image`] and a name for its picture: the small
    /// cover's fingerprint, and whether it is the large variant.
    pub fn large_picture(&self) -> Option<(&Arc<CoverImage>, (u64, bool))> {
        let art = self.art()?;
        Some(
            self.large
                .iter()
                .find(|(fingerprint, _)| *fingerprint == art.fingerprint)
                .map_or((&art.image, (art.fingerprint, false)), |(_, image)| {
                    (image, (art.fingerprint, true))
                }),
        )
    }

    /// The large load the state is waiting for, if any; any other large
    /// load running is unwanted and may be aborted.
    pub fn large_request(&self) -> Option<CoverRequestId> {
        self.large_loading.map(|(id, _)| id)
    }

    /// Whether to keep room for art: a cover is shown, or one is loading
    /// right after a track that had one.
    pub fn reserve(&self) -> bool {
        self.image().is_some() || (self.loading.is_some() && self.had_art)
    }

    /// The Now playing screen opened (`true`) or closed. Large covers are
    /// loaded only while it is open; closing it forgets the large load
    /// running (the loop aborts it), opening it again asks again for one
    /// that failed.
    pub fn set_large(&mut self, wanted: bool) {
        if wanted && !self.large_wanted {
            self.large_settled = None;
        }
        if !wanted {
            self.large_loading = None;
        }
        self.large_wanted = wanted;
    }

    /// Removes a lingering cover once [`LINGER`] has passed (the loop's
    /// tick calls this). Returns `true` if it was removed.
    pub fn expire(&mut self, now: Instant) -> bool {
        if self
            .lingering
            .as_ref()
            .is_some_and(|(_, since)| now.saturating_duration_since(*since) >= LINGER)
        {
            self.lingering = None;
            return true;
        }
        false
    }

    /// Follows the track heard (`None`: nothing). A new cover is shown at
    /// once from memory, or requested; the same one changes nothing.
    pub fn follow(&mut self, track: Option<&Track>) {
        if !self.enabled {
            return;
        }
        // Runs before every frame: compare without allocating first.
        let same = match (&self.wanted, track) {
            (None, None) => true,
            (Some(wanted), Some(track)) => wanted.is_for(track),
            (None, Some(track)) => CoverKey::for_track(track).is_none(),
            (Some(_), None) => false,
        };
        if same {
            return;
        }
        let key = track.and_then(CoverKey::for_track);
        // Nothing wanted after a client swap still remembers whether the
        // swapped-out cover had art.
        self.had_art = self.reserve() || (self.wanted.is_none() && self.had_art);
        // What was on screen stays while the new cover loads (not past
        // its original deadline when changes follow each other).
        let previous = self
            .shown
            .take()
            .map(|art| (art, Instant::now()))
            .or(self.lingering.take());
        self.wanted = key;
        self.loading = None;
        self.large_loading = None;
        self.large_settled = None;
        let Some(key) = self.wanted.clone() else {
            self.had_art = false;
            return;
        };
        if let Some(remembered) = self.recall(&key) {
            self.shown = remembered;
            self.had_art = self.shown.is_some();
            return;
        }
        self.lingering = previous;
        let id = self.next_request_id();
        self.loading = Some(id);
        self.requests.push(CoverRequest {
            id,
            generation: self.generation,
            key,
            size: CoverSize::Small,
        });
    }

    fn next_request_id(&mut self) -> CoverRequestId {
        let id = CoverRequestId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Requests for the loop to run. Each supersedes the one before. While
    /// large covers are wanted, the large variant of the cover shown is
    /// asked for once its small one has loaded (not while that loads, so
    /// it never displaces it), unless it is in memory, the small one is
    /// already as large as its source, or it found nothing before.
    pub fn take_requests(&mut self) -> Vec<CoverRequest> {
        self.ask_large();
        std::mem::take(&mut self.requests)
    }

    fn ask_large(&mut self) {
        if !self.enabled
            || !self.large_wanted
            || self.loading.is_some()
            || self.large_loading.is_some()
        {
            return;
        }
        let (Some(key), Some(art)) = (&self.wanted, &self.shown) else {
            return;
        };
        let fingerprint = art.fingerprint;
        // A small cover under `COVER_SIZE` is its whole source already.
        let enlargeable = art.image.width() >= COVER_SIZE && art.image.height() >= COVER_SIZE;
        if !enlargeable
            || self.large_settled == Some(fingerprint)
            || self.large.iter().any(|(f, _)| *f == fingerprint)
        {
            return;
        }
        let key = key.clone();
        let id = self.next_request_id();
        self.large_loading = Some((id, fingerprint));
        self.requests.push(CoverRequest {
            id,
            generation: self.generation,
            key,
            size: CoverSize::Large,
        });
    }

    /// A load finished. Navidrome results of another client generation are
    /// dropped; others are remembered (unless they failed) and shown only
    /// if they are the wanted request's. Returns `true` if what is shown
    /// changed.
    pub fn handle(&mut self, response: CoverResponse) -> bool {
        if response.key.is_remote() && response.generation != self.generation {
            return false;
        }
        if response.size == CoverSize::Large {
            return self.handle_large(response);
        }
        let result = match response.outcome {
            Outcome::Art(image) => Some(Some(Art::new(image))),
            Outcome::NoArt => Some(None),
            Outcome::Failed => None,
        };
        if let Some(result) = &result {
            self.remember(response.key.clone(), result.clone());
        }
        if self.loading != Some(response.id) {
            return false;
        }
        self.loading = None;
        self.shown = result.flatten();
        self.lingering = None;
        self.had_art = self.shown.is_some();
        true
    }

    /// A large load finished: kept (and shown, if its picture is) only if
    /// it answers the large request running.
    fn handle_large(&mut self, response: CoverResponse) -> bool {
        let Some((id, fingerprint)) = self.large_loading else {
            return false;
        };
        if id != response.id {
            return false;
        }
        self.large_loading = None;
        match response.outcome {
            Outcome::Art(image) => {
                self.large.retain(|(f, _)| *f != fingerprint);
                self.large.push_back((fingerprint, image));
                while self.large.len() > LARGE_MEMORY_ENTRIES {
                    self.large.pop_front();
                }
                self.art().is_some_and(|art| art.fingerprint == fingerprint)
            }
            Outcome::NoArt | Outcome::Failed => {
                self.large_settled = Some(fingerprint);
                false
            }
        }
    }

    /// The Navidrome client changed (or went away): remembered Navidrome
    /// covers are forgotten, and a Navidrome cover being shown or loaded
    /// is asked again of the new client at the next [`Covers::follow`].
    /// Large covers stay remembered: they are kept by their picture.
    pub fn client_changed(&mut self, generation: Generation) {
        self.generation = generation;
        self.memory.retain(|(key, _)| !key.is_remote());
        if self.wanted.as_ref().is_some_and(CoverKey::is_remote) {
            self.had_art = self.reserve();
            self.wanted = None;
            if let Some(art) = self.shown.take() {
                self.lingering = Some((art, Instant::now()));
            }
            self.loading = None;
            self.large_loading = None;
        }
    }

    fn recall(&mut self, key: &CoverKey) -> Option<Option<Art>> {
        let index = self.memory.iter().position(|(k, _)| k == key)?;
        let entry = self.memory.remove(index)?;
        let art = entry.1.clone();
        self.memory.push_back(entry);
        Some(art)
    }

    fn remember(&mut self, key: CoverKey, art: Option<Art>) {
        self.memory.retain(|(k, _)| *k != key);
        self.memory.push_back((key, art));
        while self.memory.len() > MEMORY_ENTRIES {
            self.memory.pop_front();
        }
    }
}

/// Loads `request` on a Tokio task (decoding and file I/O on blocking
/// threads); the response arrives on `responses` exactly once, also if
/// the task is aborted or panics ([`Outcome::Failed`]). The cover is
/// decoded at the request's [`CoverSize`]. A Navidrome cover needs
/// `client` (without one there is no art) and is looked up in, and saved
/// to, `cache` when there is one (the cache for that size).
pub fn spawn_load(
    request: CoverRequest,
    client: Option<navidrome::Client>,
    cache: Option<Arc<cache::DiskCache>>,
    responses: &mpsc::UnboundedSender<CoverResponse>,
) -> JoinHandle<()> {
    let size = request.size;
    let answer = move |key: CoverKey, outcome| CoverResponse {
        id: request.id,
        generation: request.generation,
        key,
        size,
        outcome,
    };
    let reply = Reply::new(responses, answer(request.key.clone(), Outcome::Failed));
    tokio::spawn(async move {
        let outcome = match &request.key {
            CoverKey::Local(path) => {
                let path = path.clone();
                let pixels = size.pixels();
                match tokio::task::spawn_blocking(move || local::load_at(&path, pixels)).await {
                    Ok(Some(image)) => Outcome::Art(Arc::new(image)),
                    Ok(None) => Outcome::NoArt,
                    Err(_) => Outcome::Failed,
                }
            }
            CoverKey::Navidrome(id) => match client {
                Some(client) => remote(&client, cache, id, size).await,
                None => Outcome::NoArt,
            },
        };
        reply.send(answer(request.key, outcome));
    })
}

/// A Navidrome cover at `size`: from the disk cache, else fetched (and
/// cached unless the server said `no-store`).
async fn remote(
    client: &navidrome::Client,
    cache: Option<Arc<cache::DiskCache>>,
    cover_id: &str,
    size: CoverSize,
) -> Outcome {
    // A cache of the other size would hand back the wrong size.
    let cache = cache.filter(|cache| cache.size() == size);
    let name = cache::DiskCache::name(client.base_url().as_str(), client.username(), cover_id);
    if let Some(cache) = cache.clone() {
        let name = name.clone();
        if let Ok(Some(image)) = tokio::task::spawn_blocking(move || cache.get(&name)).await {
            return Outcome::Art(Arc::new(image));
        }
    }
    let bytes = match client.cover_art(cover_id, size.pixels()).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::debug!(detail = %error.detail(), "cover art fetch failed");
            return match error {
                navidrome::Error::Server { .. }
                | navidrome::Error::Http(404)
                | navidrome::Error::Decode(_)
                | navidrome::Error::TooLarge => Outcome::NoArt,
                _ => Outcome::Failed,
            };
        }
    };
    if bytes.no_store {
        tracing::debug!("cover art is a placeholder; not shown");
        return Outcome::NoArt;
    }
    let decoded = tokio::task::spawn_blocking(move || {
        let image = decode_at(&bytes.bytes, size.pixels())?;
        if let Some(cache) = cache
            && let Err(error) = cache.put(&name, &image)
        {
            tracing::debug!(kind = ?error.kind(), "could not cache a cover");
        }
        Ok::<_, DecodeError>(image)
    })
    .await;
    match decoded {
        Ok(Ok(image)) => Outcome::Art(Arc::new(image)),
        Ok(Err(error)) => {
            tracing::debug!(%error, "cover art did not decode");
            Outcome::NoArt
        }
        Err(_) => Outcome::Failed,
    }
}

#[cfg(test)]
mod tests;
