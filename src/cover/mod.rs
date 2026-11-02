//! Cover art for the track heard, without any terminal code.
//!
//! - **Images** ([`CoverImage`]) are plain RGB pixels, square, at most
//!   [`COVER_SIZE`] pixels a side. [`decode`] makes them from untrusted
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
//!   a client swap (by [`Generation`]).
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

/// The largest side of a decoded cover, in pixels. Also the `size` asked
/// of `getCoverArt`, and the size of cached files.
pub const COVER_SIZE: u32 = 256;
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
/// [`COVER_SIZE`] pixels (the centre of a non-square image). Refuses more
/// than [`MAX_IMAGE_BYTES`] and sides over [`MAX_DIMENSION`] before
/// decoding; [`MAX_DECODE_ALLOC`] bounds what the decoder allocates, the
/// output image included, which is checked before it is allocated. At
/// most one decode runs at a time ([`DECODE_GATE`]), so loads that were
/// superseded (their blocking work still finishes) cannot stack decodes.
/// Blocking.
pub fn decode(bytes: &[u8]) -> Result<CoverImage, DecodeError> {
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
    square(image, COVER_SIZE).ok_or(DecodeError::Malformed)
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
    pub outcome: Outcome,
}

/// The cover state; see the module docs.
#[derive(Debug)]
pub struct Covers {
    enabled: bool,
    generation: Generation,
    /// The audible track's cover.
    wanted: Option<CoverKey>,
    /// The wanted cover, once loaded.
    shown: Option<Arc<CoverImage>>,
    /// The previous track's cover, kept on screen while the wanted one
    /// loads (from when), for at most [`LINGER`].
    lingering: Option<(Arc<CoverImage>, Instant)>,
    /// The request for the wanted cover, while it runs.
    loading: Option<CoverRequestId>,
    /// The last settled cover was an image: while the next one loads,
    /// [`Covers::reserve`] keeps its place so the bar does not jump.
    had_art: bool,
    /// Recent results, newest last.
    memory: VecDeque<(CoverKey, Option<Arc<CoverImage>>)>,
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
        self.shown
            .as_ref()
            .or(self.lingering.as_ref().map(|(image, _)| image))
    }

    /// Whether to keep room for art: a cover is shown, or one is loading
    /// right after a track that had one.
    pub fn reserve(&self) -> bool {
        self.image().is_some() || (self.loading.is_some() && self.had_art)
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
            .map(|image| (image, Instant::now()))
            .or(self.lingering.take());
        self.wanted = key;
        self.loading = None;
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
        let id = CoverRequestId(self.next_id);
        self.next_id += 1;
        self.loading = Some(id);
        self.requests.push(CoverRequest {
            id,
            generation: self.generation,
            key,
        });
    }

    /// Requests for the loop to run. Each supersedes the one before.
    pub fn take_requests(&mut self) -> Vec<CoverRequest> {
        std::mem::take(&mut self.requests)
    }

    /// A load finished. Navidrome results of another client generation are
    /// dropped; others are remembered (unless they failed) and shown only
    /// if they are the wanted request's. Returns `true` if what is shown
    /// changed.
    pub fn handle(&mut self, response: CoverResponse) -> bool {
        if response.key.is_remote() && response.generation != self.generation {
            return false;
        }
        let result = match response.outcome {
            Outcome::Art(image) => Some(Some(image)),
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

    /// The Navidrome client changed (or went away): remembered Navidrome
    /// covers are forgotten, and a Navidrome cover being shown or loaded
    /// is asked again of the new client at the next [`Covers::follow`].
    pub fn client_changed(&mut self, generation: Generation) {
        self.generation = generation;
        self.memory.retain(|(key, _)| !key.is_remote());
        if self.wanted.as_ref().is_some_and(CoverKey::is_remote) {
            self.had_art = self.reserve();
            self.wanted = None;
            if let Some(image) = self.shown.take() {
                self.lingering = Some((image, Instant::now()));
            }
            self.loading = None;
        }
    }

    fn recall(&mut self, key: &CoverKey) -> Option<Option<Arc<CoverImage>>> {
        let index = self.memory.iter().position(|(k, _)| k == key)?;
        let entry = self.memory.remove(index)?;
        let image = entry.1.clone();
        self.memory.push_back(entry);
        Some(image)
    }

    fn remember(&mut self, key: CoverKey, image: Option<Arc<CoverImage>>) {
        self.memory.retain(|(k, _)| *k != key);
        self.memory.push_back((key, image));
        while self.memory.len() > MEMORY_ENTRIES {
            self.memory.pop_front();
        }
    }
}

/// Loads `request` on a Tokio task (decoding and file I/O on blocking
/// threads); the response arrives on `responses` exactly once, also if
/// the task is aborted or panics ([`Outcome::Failed`]). A Navidrome cover
/// needs `client` (without one there is no art) and is looked up in, and
/// saved to, `cache` when there is one.
pub fn spawn_load(
    request: CoverRequest,
    client: Option<navidrome::Client>,
    cache: Option<Arc<cache::DiskCache>>,
    responses: &mpsc::UnboundedSender<CoverResponse>,
) -> JoinHandle<()> {
    let answer = move |key: CoverKey, outcome| CoverResponse {
        id: request.id,
        generation: request.generation,
        key,
        outcome,
    };
    let reply = Reply::new(responses, answer(request.key.clone(), Outcome::Failed));
    tokio::spawn(async move {
        let outcome = match &request.key {
            CoverKey::Local(path) => {
                let path = path.clone();
                match tokio::task::spawn_blocking(move || local::load(&path)).await {
                    Ok(Some(image)) => Outcome::Art(Arc::new(image)),
                    Ok(None) => Outcome::NoArt,
                    Err(_) => Outcome::Failed,
                }
            }
            CoverKey::Navidrome(id) => match client {
                Some(client) => remote(&client, cache, id).await,
                None => Outcome::NoArt,
            },
        };
        reply.send(answer(request.key, outcome));
    })
}

/// A Navidrome cover: from the disk cache, else fetched (and cached
/// unless the server said `no-store`).
async fn remote(
    client: &navidrome::Client,
    cache: Option<Arc<cache::DiskCache>>,
    cover_id: &str,
) -> Outcome {
    let name = cache::DiskCache::name(client.base_url().as_str(), client.username(), cover_id);
    if let Some(cache) = cache.clone() {
        let name = name.clone();
        if let Ok(Some(image)) = tokio::task::spawn_blocking(move || cache.get(&name)).await {
            return Outcome::Art(Arc::new(image));
        }
    }
    let bytes = match client.cover_art(cover_id, COVER_SIZE).await {
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
        let image = decode(&bytes.bytes)?;
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
