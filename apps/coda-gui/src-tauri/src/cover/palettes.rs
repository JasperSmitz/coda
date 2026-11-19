//! The covers' palettes for the webview (DESIGN.md "Colour"): the
//! `cover_palettes` command answers a batch of cover URLs (the ones the
//! webview already has) with each cover's colours, computed by
//! `coda_core::cover::palette` from the small cover (loaded like the
//! `coda://` handler does, with the same loaders and disk cache, but
//! within its own load limit, [`PALETTE_LOADS`], so palettes never hold up
//! the covers on screen) on a blocking thread, and kept in a bounded
//! least-recently-used cache by cover key. Never touches the engine actor.
//!
//! The answer distinguishes a definitive "no palette" (`null`: an unknown
//! URL, a cover without art) from a failure worth retrying (the URL is
//! left out of the answer). The small cover is decoded again for the
//! palette even when the webview has just loaded it (no shared memory
//! cache yet).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use coda_core::cover::palette::{Palette, palette};
use coda_core::cover::{CoverKey, CoverSize, Outcome};
use serde::Serialize;
use tauri::State;
use tokio::task::JoinSet;

use super::CoverService;

/// Palettes kept (each about 100 bytes plus its key).
pub const MAX_PALETTES: usize = 4_000;
/// Cover URLs one `cover_palettes` call may ask for.
pub const MAX_BATCH: usize = 64;
/// Covers loaded at once for palettes (apart from the protocol's loads).
pub const PALETTE_LOADS: usize = 2;
/// The glow's opacity: the accent, translucent.
const GLOW_ALPHA: f64 = 0.45;

/// A cover's colours for the webview, hex strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteDto {
    /// A deep dark of the dominant hue (`#rrggbb`).
    pub tint: String,
    /// The most vivid colour, readable on the base and the tint (`#rrggbb`).
    pub accent: String,
    /// Black or white on the accent (`#rrggbb`).
    pub accent_text: String,
    /// The accent at 45% (`#rrggbbaa`).
    pub glow: String,
    /// `false`: a greyscale cover; show the house accent.
    pub vivid: bool,
}

impl From<Palette> for PaletteDto {
    fn from(p: Palette) -> Self {
        Self {
            tint: p.tint.to_string(),
            accent: p.accent.to_string(),
            accent_text: p.accent_text.to_string(),
            glow: p.accent.with_alpha(GLOW_ALPHA).to_string(),
            vivid: p.vivid,
        }
    }
}

/// Palettes by cover key, the least recently used dropped first.
pub(crate) struct PaletteCache {
    entries: HashMap<CoverKey, (PaletteDto, u64)>,
    /// Keys by last use.
    order: BTreeMap<u64, CoverKey>,
    next: u64,
    max: usize,
    /// Bumped when the Navidrome client changes: a palette loaded with the
    /// old one is not kept.
    pub(crate) generation: u64,
}

impl Default for PaletteCache {
    fn default() -> Self {
        Self::with_capacity(MAX_PALETTES)
    }
}

impl PaletteCache {
    pub(crate) fn with_capacity(max: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: BTreeMap::new(),
            next: 0,
            max,
            generation: 0,
        }
    }

    fn stamp(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// The palette of `key`, if cached (and now the most recently used).
    pub(crate) fn get(&mut self, key: &CoverKey) -> Option<PaletteDto> {
        let stamp = self.stamp();
        let (palette, used) = self.entries.get_mut(key)?;
        self.order.remove(used);
        *used = stamp;
        self.order.insert(stamp, key.clone());
        Some(palette.clone())
    }

    /// Keeps `palette` for `key` unless the client changed since
    /// `generation` (for a Navidrome key).
    pub(crate) fn insert(&mut self, key: CoverKey, palette: PaletteDto, generation: u64) {
        if key.is_remote() && generation != self.generation {
            return;
        }
        let stamp = self.stamp();
        if let Some((_, used)) = self.entries.insert(key.clone(), (palette, stamp)) {
            self.order.remove(&used);
        }
        self.order.insert(stamp, key);
        while self.entries.len() > self.max {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    /// The Navidrome client changed: its covers' palettes are forgotten.
    pub(crate) fn client_changed(&mut self) {
        self.generation += 1;
        self.entries.retain(|key, _| !key.is_remote());
        let entries = &self.entries;
        self.order.retain(|_, key| entries.contains_key(key));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// What `palette_for_url` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteAnswer {
    Palette(PaletteDto),
    /// Definitive: an unknown URL or a cover without art.
    None,
    /// The cover failed to load (a network error, a failed task): ask
    /// again later. Not cached.
    Retry,
}

impl CoverService {
    /// The palette of the cover behind `cover_url` (a URL this service
    /// handed out).
    pub async fn palette_for_url(&self, cover_url: &str) -> PaletteAnswer {
        let Some(key) = self.key_for_url(cover_url) else {
            return PaletteAnswer::None;
        };
        let generation = {
            let mut cache = self.palettes.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(hit) = cache.get(&key) {
                return PaletteAnswer::Palette(hit);
            }
            cache.generation
        };
        let image = match self
            .load_within(&self.palette_loads, &key, CoverSize::Small)
            .await
        {
            Outcome::Art(image) => image,
            Outcome::NoArt => return PaletteAnswer::None,
            Outcome::Failed => return PaletteAnswer::Retry,
        };
        let Ok(palette) =
            tokio::task::spawn_blocking(move || PaletteDto::from(palette(&image))).await
        else {
            return PaletteAnswer::Retry;
        };
        self.palettes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, palette.clone(), generation);
        PaletteAnswer::Palette(palette)
    }

    /// The palettes of up to [`MAX_BATCH`] cover URLs (duplicates once),
    /// loaded concurrently within [`PALETTE_LOADS`]. A URL whose cover
    /// failed to load is left out (ask again later).
    pub async fn palettes_for_urls(
        self: Arc<Self>,
        urls: Vec<String>,
    ) -> Result<HashMap<String, Option<PaletteDto>>, String> {
        if urls.len() > MAX_BATCH {
            return Err(format!("At most {MAX_BATCH} covers per call"));
        }
        let urls: BTreeSet<String> = urls.into_iter().collect();
        let mut tasks = JoinSet::new();
        for url in urls {
            let service = self.clone();
            tasks.spawn(async move {
                let palette = service.palette_for_url(&url).await;
                (url, palette)
            });
        }
        let mut answers = HashMap::new();
        while let Some(done) = tasks.join_next().await {
            match done {
                Ok((url, PaletteAnswer::Palette(palette))) => {
                    answers.insert(url, Some(palette));
                }
                Ok((url, PaletteAnswer::None)) => {
                    answers.insert(url, None);
                }
                Ok((_, PaletteAnswer::Retry)) | Err(_) => {}
            }
        }
        Ok(answers)
    }
}

/// The palettes of cover URLs the webview has (`coda://…/cover/<token>`):
/// `{url: palette | null}`; `null` for an unknown URL or a cover without
/// art; a URL whose cover failed to load is missing (ask again later). At
/// most [`MAX_BATCH`] URLs.
#[tauri::command]
pub async fn cover_palettes(
    covers: State<'_, Arc<CoverService>>,
    urls: Vec<String>,
) -> Result<HashMap<String, Option<PaletteDto>>, String> {
    covers.inner().clone().palettes_for_urls(urls).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use coda_core::model::{Track, TrackMetadata};

    fn dto(accent: &str) -> PaletteDto {
        PaletteDto {
            tint: "#000000".into(),
            accent: accent.into(),
            accent_text: "#000000".into(),
            glow: "#00000073".into(),
            vivid: true,
        }
    }

    fn png(dir: &std::path::Path, color: [u8; 3]) {
        let mut bytes = Vec::new();
        image::RgbImage::from_pixel(8, 8, image::Rgb(color))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        std::fs::write(dir.join("cover.png"), bytes).unwrap();
    }

    fn local_url(service: &CoverService, dir: &std::path::Path) -> String {
        let track = Track::local(dir.join("a.flac"), TrackMetadata::default());
        service.url_for(&track).unwrap()
    }

    #[test]
    fn the_dto_is_hex_with_a_translucent_glow() {
        let p = palette(&coda_core::cover::CoverImage::new(1, 1, vec![200, 30, 30]).unwrap());
        let dto = PaletteDto::from(p);
        assert_eq!(dto.accent, p.accent.to_string());
        assert_eq!(dto.glow, format!("{}73", dto.accent));
        assert!(dto.vivid);
        let json = serde_json::to_value(&dto).unwrap();
        for key in ["tint", "accent", "accentText", "glow", "vivid"] {
            assert!(json.get(key).is_some(), "{key}");
        }
    }

    #[tokio::test]
    async fn unknown_urls_answer_null() {
        let service = Arc::new(CoverService::new(None));
        let answers = service
            .palettes_for_urls(vec![
                "coda://localhost/cover/00000000000000ff".into(),
                "not a url".into(),
            ])
            .await
            .unwrap();
        assert_eq!(answers.len(), 2);
        assert!(answers.values().all(Option::is_none));
    }

    #[tokio::test]
    async fn a_cached_palette_is_not_loaded_again() {
        let dir = tempfile::tempdir().unwrap();
        png(dir.path(), [200, 30, 30]);
        let service = Arc::new(CoverService::new(None));
        let url = local_url(&service, dir.path());
        let PaletteAnswer::Palette(first) = service.palette_for_url(&url).await else {
            panic!("a palette");
        };
        assert!(first.vivid);
        // The file is gone: only the cache can answer.
        std::fs::remove_file(dir.path().join("cover.png")).unwrap();
        let answers = service
            .clone()
            .palettes_for_urls(vec![url.clone()])
            .await
            .unwrap();
        assert_eq!(answers[&url], Some(first));
        // A cover without art answers null.
        let bare = tempfile::tempdir().unwrap();
        let none = local_url(&service, bare.path());
        assert_eq!(service.palette_for_url(&none).await, PaletteAnswer::None);
        let answers = service.palettes_for_urls(vec![none.clone()]).await.unwrap();
        assert_eq!(answers.get(&none), Some(&None));
    }

    #[tokio::test]
    async fn batches_are_limited() {
        let service = Arc::new(CoverService::new(None));
        let urls = |n: usize| {
            (0..n)
                .map(|i| format!("coda://localhost/cover/{i:016x}"))
                .collect()
        };
        assert!(
            service
                .clone()
                .palettes_for_urls(urls(MAX_BATCH))
                .await
                .is_ok()
        );
        assert!(
            service
                .palettes_for_urls(urls(MAX_BATCH + 1))
                .await
                .is_err()
        );
    }

    /// A failed load (the server errs) is left out of the answer and
    /// retried; a cover the server has no art for is a definitive `null`.
    #[tokio::test]
    async fn a_failed_load_is_retried_and_no_art_is_null() {
        use coda_core::navidrome::client::parse_base_url;
        use coda_core::navidrome::{Client, Password};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = Client::new(
            parse_base_url(&server.uri()).unwrap(),
            "listener".into(),
            Password::new("pass".into()),
        )
        .unwrap();
        let service = Arc::new(CoverService::new(None));
        service.set_client(Some(client));
        let track = Track::navidrome(
            "s1".into(),
            TrackMetadata {
                cover_art: Some("al-1".into()),
                ..Default::default()
            },
        );
        let url = service.url_for(&track).unwrap();
        let failing = Mock::given(method("GET"))
            .and(path("/rest/getCoverArt"))
            .respond_with(ResponseTemplate::new(503))
            .mount_as_scoped(&server)
            .await;
        assert_eq!(service.palette_for_url(&url).await, PaletteAnswer::Retry);
        let answers = service
            .clone()
            .palettes_for_urls(vec![url.clone()])
            .await
            .unwrap();
        assert!(!answers.contains_key(&url), "missing: ask again later");
        drop(failing);
        Mock::given(method("GET"))
            .and(path("/rest/getCoverArt"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let answers = service.palettes_for_urls(vec![url.clone()]).await.unwrap();
        assert_eq!(answers.get(&url), Some(&None));
    }

    #[test]
    fn the_cache_is_bounded_and_drops_the_least_recently_used() {
        let mut cache = PaletteCache::with_capacity(3);
        let key = |i: u32| CoverKey::Local(format!("/{i}").into());
        for i in 0..3 {
            cache.insert(key(i), dto(&format!("#00000{i}")), 0);
        }
        // Used again: kept over the others.
        assert!(cache.get(&key(0)).is_some());
        cache.insert(key(3), dto("#000003"), 0);
        assert_eq!(cache.len(), 3);
        assert!(cache.get(&key(1)).is_none(), "the least recently used went");
        assert!(cache.get(&key(0)).is_some());
        // Replacing a key does not grow it.
        cache.insert(key(0), dto("#000009"), 0);
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.order.len(), 3);
        for i in 10..1000 {
            cache.insert(key(i), dto("#000000"), 0);
        }
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.order.len(), 3);
    }

    #[test]
    fn a_new_client_forgets_remote_palettes_and_late_ones() {
        let mut cache = PaletteCache::default();
        let remote = CoverKey::Navidrome("al-1".into());
        let local = CoverKey::Local("/a".into());
        cache.insert(remote.clone(), dto("#000001"), 0);
        cache.insert(local.clone(), dto("#000002"), 0);
        let before = cache.generation;
        cache.client_changed();
        assert!(cache.get(&remote).is_none());
        assert!(cache.get(&local).is_some());
        // Loaded with the old client, arriving after the change.
        cache.insert(remote.clone(), dto("#000003"), before);
        assert!(cache.get(&remote).is_none());
        cache.insert(remote.clone(), dto("#000004"), cache.generation);
        assert!(cache.get(&remote).is_some());
    }
}
