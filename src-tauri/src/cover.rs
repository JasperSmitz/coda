//! Cover art for the webview, served by our own `coda` URI scheme.
//!
//! Track DTOs carry an opaque cover URL (`coda://localhost/cover/<token>`,
//! `http://coda.localhost/cover/<token>` where the platform needs that
//! form); the webview appends `/small` (256 px) or `/large` (1024 px). The
//! token maps to a [`CoverKey`] in a bounded [`CoverRegistry`], filled
//! when the engine builds DTOs, so the webview can never name a file
//! path or reach anything but covers of tracks it was shown. Images come
//! from `coda_core::cover::load` (tags / folder image for local tracks,
//! `getCoverArt` + Coda's disk cache for Navidrome) and are sent as JPEG.

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, VecDeque};
use std::hash::BuildHasher;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use coda_core::cover::{self, CoverKey, CoverSize, Outcome, cache::DiskCache};
use coda_core::model::Track;
use coda_core::navidrome::Client;
use tauri::http::{Request, Response, StatusCode, header};
use tokio::sync::Semaphore;

/// The URI scheme name.
pub const SCHEME: &str = "coda";
/// Tokens remembered (the queue's cap plus browse lists); the oldest are
/// forgotten first, and their URLs then answer 404.
const MAX_TOKENS: usize = 25_000;
/// Covers loaded at once.
const MAX_LOADS: usize = 4;
/// Webview cache lifetime for a served cover.
const CACHE_CONTROL: &str = "private, max-age=86400";

/// The base URL for cover `token` on this platform.
fn url(token: u64) -> String {
    // Tauri serves custom schemes as `http://<scheme>.localhost` on
    // Windows and Android, `<scheme>://localhost` elsewhere.
    if cfg!(any(windows, target_os = "android")) {
        format!("http://{SCHEME}.localhost/cover/{token:016x}")
    } else {
        format!("{SCHEME}://localhost/cover/{token:016x}")
    }
}

#[derive(Default)]
struct Tokens {
    keys: HashMap<u64, CoverKey>,
    order: VecDeque<u64>,
}

/// Opaque tokens for cover keys. Tokens are keyed hashes (a per-run
/// random key), so they reveal nothing about the path or id.
pub struct CoverRegistry {
    hasher: RandomState,
    tokens: Mutex<Tokens>,
    max: usize,
}

impl Default for CoverRegistry {
    fn default() -> Self {
        Self::with_capacity(MAX_TOKENS)
    }
}

impl CoverRegistry {
    fn with_capacity(max: usize) -> Self {
        Self {
            hasher: RandomState::new(),
            tokens: Mutex::new(Tokens::default()),
            max,
        }
    }

    /// The cover URL for `track` (registering its key), if it has a cover
    /// key at all.
    pub fn url_for(&self, track: &Track) -> Option<String> {
        CoverKey::for_track(track).map(|key| url(self.register(key)))
    }

    fn register(&self, key: CoverKey) -> u64 {
        let token = self.hasher.hash_one(&key);
        let mut tokens = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        if !tokens.keys.contains_key(&token) {
            while tokens.order.len() >= self.max {
                if let Some(old) = tokens.order.pop_front() {
                    tokens.keys.remove(&old);
                }
            }
            tokens.order.push_back(token);
            tokens.keys.insert(token, key);
        }
        token
    }

    fn get(&self, token: u64) -> Option<CoverKey> {
        let tokens = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        tokens.keys.get(&token).cloned()
    }
}

/// What the protocol handler needs, shared with the engine (which sets the
/// client once it is built). Not engine state: only the cover lookups.
pub struct CoverService {
    pub registry: CoverRegistry,
    client: RwLock<Option<Client>>,
    small: Option<Arc<DiskCache>>,
    large: Option<Arc<DiskCache>>,
    loads: Semaphore,
    enabled: RwLock<bool>,
}

impl CoverService {
    /// `cache_dir`: Coda's cover cache folder (shared with the TUI), if one
    /// is known.
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        Self {
            registry: CoverRegistry::default(),
            client: RwLock::new(None),
            small: cache_dir.clone().map(|dir| Arc::new(DiskCache::new(dir))),
            large: cache_dir.map(|dir| Arc::new(DiskCache::large(cover::cache::large_dir(&dir)))),
            loads: Semaphore::new(MAX_LOADS),
            enabled: RwLock::new(true),
        }
    }

    pub fn set_client(&self, client: Option<Client>) {
        *self.client.write().unwrap_or_else(|e| e.into_inner()) = client;
    }

    /// `[ui] cover_art = false` turns cover URLs off.
    pub fn set_enabled(&self, enabled: bool) {
        *self.enabled.write().unwrap_or_else(|e| e.into_inner()) = enabled;
    }

    /// The cover URL for `track`, if covers are on and it has a key.
    pub fn url_for(&self, track: &Track) -> Option<String> {
        let enabled = *self.enabled.read().unwrap_or_else(|e| e.into_inner());
        enabled.then(|| self.registry.url_for(track)).flatten()
    }

    /// Answers one `coda://` request: `GET /cover/<token>/<small|large>`.
    pub async fn respond(&self, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
        if request.method() != tauri::http::Method::GET {
            return status(StatusCode::METHOD_NOT_ALLOWED);
        }
        let Some((token, size)) = parse_path(request.uri().path()) else {
            return status(StatusCode::NOT_FOUND);
        };
        let Some(key) = self.registry.get(token) else {
            return status(StatusCode::NOT_FOUND);
        };
        let Ok(_permit) = self.loads.acquire().await else {
            return status(StatusCode::SERVICE_UNAVAILABLE);
        };
        let client = self
            .client
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let cache = match size {
            CoverSize::Small => self.small.clone(),
            CoverSize::Large => self.large.clone(),
        };
        let image = match cover::load(&key, size, client, cache).await {
            Outcome::Art(image) => image,
            Outcome::NoArt => return status(StatusCode::NOT_FOUND),
            Outcome::Failed => return status(StatusCode::SERVICE_UNAVAILABLE),
        };
        let encoded = tokio::task::spawn_blocking(move || cover::encode_jpeg(&image)).await;
        match encoded {
            Ok(Ok(bytes)) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "image/jpeg")
                .header(header::CACHE_CONTROL, CACHE_CONTROL)
                .body(bytes)
                .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR)),
            _ => {
                tracing::debug!("cover did not encode");
                status(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    let mut response = Response::new(Vec::new());
    *response.status_mut() = code;
    response
}

/// `/cover/<16 hex digits>/<small|large>`.
fn parse_path(path: &str) -> Option<(u64, CoverSize)> {
    let rest = path.strip_prefix("/cover/")?;
    let (token, size) = rest.split_once('/')?;
    if token.len() != 16 {
        return None;
    }
    let token = u64::from_str_radix(token, 16).ok()?;
    let size = match size {
        "small" => CoverSize::Small,
        "large" => CoverSize::Large,
        _ => return None,
    };
    Some((token, size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use coda_core::model::TrackMetadata;

    fn local(path: &str) -> Track {
        Track::local(PathBuf::from(path), TrackMetadata::default())
    }

    #[test]
    fn urls_are_opaque_and_stable() {
        let registry = CoverRegistry::default();
        let a = registry
            .url_for(&local("/music/secret name/a.flac"))
            .unwrap();
        assert!(!a.contains("secret"), "{a}");
        assert_eq!(
            registry
                .url_for(&local("/music/secret name/a.flac"))
                .unwrap(),
            a
        );
        assert_ne!(registry.url_for(&local("/music/b.flac")).unwrap(), a);
        let token = a.rsplit('/').next().unwrap();
        assert_eq!(token.len(), 16);
        let path = format!("/cover/{token}/small");
        let (parsed, size) = parse_path(&path).unwrap();
        assert_eq!(size, CoverSize::Small);
        assert_eq!(
            registry.get(parsed),
            Some(CoverKey::Local("/music/secret name/a.flac".into()))
        );
    }

    #[test]
    fn navidrome_tracks_without_cover_art_have_no_url() {
        let registry = CoverRegistry::default();
        let track = Track::navidrome("s1".into(), TrackMetadata::default());
        assert_eq!(registry.url_for(&track), None);
        let track = Track::navidrome(
            "s1".into(),
            TrackMetadata {
                cover_art: Some("al-1".into()),
                ..Default::default()
            },
        );
        assert!(registry.url_for(&track).is_some());
    }

    #[test]
    fn the_registry_forgets_the_oldest_tokens() {
        let registry = CoverRegistry::with_capacity(2);
        let first = registry.register(CoverKey::Local("/a".into()));
        registry.register(CoverKey::Local("/b".into()));
        registry.register(CoverKey::Local("/c".into()));
        assert_eq!(registry.get(first), None);
        assert_eq!(registry.tokens.lock().unwrap().keys.len(), 2);
    }

    #[test]
    fn paths_are_parsed_strictly() {
        assert!(parse_path("/cover/00000000000000ff/large").is_some());
        for bad in [
            "/cover/ff/small",
            "/cover/00000000000000ff/huge",
            "/cover/00000000000000ff",
            "/other/00000000000000ff/small",
            "/cover/zzzzzzzzzzzzzzzz/small",
            "/cover/../../etc/passwd/small",
        ] {
            assert_eq!(parse_path(bad), None, "{bad}");
        }
    }

    #[tokio::test]
    async fn unknown_tokens_and_other_methods_are_refused() {
        let service = CoverService::new(None);
        let get = |uri: &str| Request::builder().uri(uri).body(Vec::new()).unwrap();
        let response = service
            .respond(get("coda://localhost/cover/00000000000000ff/small"))
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let post = Request::builder()
            .method("POST")
            .uri("coda://localhost/cover/00000000000000ff/small")
            .body(Vec::new())
            .unwrap();
        assert_eq!(
            service.respond(post).await.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }

    #[tokio::test]
    async fn a_local_folder_cover_is_served_as_jpeg() {
        let dir = tempfile::tempdir().unwrap();
        let mut png = Vec::new();
        image::RgbImage::from_pixel(8, 8, image::Rgb([200, 10, 10]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        std::fs::write(dir.path().join("cover.png"), png).unwrap();
        let track = local(dir.path().join("a.flac").to_str().unwrap());
        let service = CoverService::new(None);
        let url = service.url_for(&track).unwrap();
        let path = url.split("localhost").nth(1).unwrap();
        let request = Request::builder()
            .uri(format!("coda://localhost{path}/small"))
            .body(Vec::new())
            .unwrap();
        let response = service.respond(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
        assert_eq!(&response.body()[..2], &[0xFF, 0xD8]);
        service.set_enabled(false);
        assert_eq!(service.url_for(&track), None);
    }
}
