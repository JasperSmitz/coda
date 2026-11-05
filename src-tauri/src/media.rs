//! OS media controls through `souvlaki`: MPRIS on Linux,
//! `MPNowPlayingInfoCenter` and remote commands on macOS. Media keys and
//! the desktop's media widgets send intents to the engine like the UI
//! does; the engine's snapshots and progress are published as metadata
//! (title, artist, album, duration, cover as a `file://` JPEG we write)
//! and playback status/position.
//!
//! [`Publisher`] decides what to publish (pure, tested); [`Media`] feeds
//! it from the engine's output and runs every `souvlaki` call on the main
//! thread (macOS needs the main run loop; the controls live in a
//! main-thread `thread_local`). If the controls cannot be created the app
//! logs it and runs without them.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use coda_core::cover::CoverSize;
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};
use tauri::{AppHandle, Manager};

use crate::cover::CoverService;
use crate::dto::{Intent, ProgressDto, Snapshot, StatusDto, TrackDto};
use crate::engine::EngineHandle;

/// A position change bigger than this (from where it should be) is a
/// seek, published at once.
const JUMP: Duration = Duration::from_millis(1500);
/// While playing, the position is republished this often (MPRIS clients
/// read the stored value).
const REFRESH: Duration = Duration::from_secs(5);
/// Media key seek step.
const SEEK_STEP: f64 = 10.0;
/// D-Bus name suffix (`org.mpris.MediaPlayer2.<this>`).
const DBUS_NAME: &str = "coda_app";

/// The metadata of the track heard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<Duration>,
    /// The track's `coda://` cover URL, resolved to a file separately.
    pub cover_url: Option<String>,
}

impl Metadata {
    fn from_track(track: &TrackDto) -> Self {
        Self {
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            duration: track.duration_ms.map(Duration::from_millis),
            cover_url: track.cover_url.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Stopped,
    Playing,
    Paused,
}

/// One change for the OS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// New track (or none: cleared).
    Metadata(Option<Metadata>),
    Playback {
        state: State,
        position: Option<Duration>,
    },
}

/// Decides what to publish. No I/O.
#[derive(Debug, Default)]
pub struct Publisher {
    /// The track as the snapshot gave it (what a track change compares).
    track: Option<Metadata>,
    /// What was published (the duration may come from mpv).
    metadata: Option<Metadata>,
    state: Option<State>,
    /// The last position published, and when.
    published: Option<(Duration, Instant)>,
    position: Option<Duration>,
    duration: Option<Duration>,
}

impl Publisher {
    pub fn state(&self) -> State {
        self.state.unwrap_or(State::Stopped)
    }

    /// The engine's snapshot: the track heard and the status.
    pub fn snapshot(&mut self, snapshot: &Snapshot, now: Instant) -> Vec<Update> {
        let mut updates = Vec::new();
        let track = snapshot.playing.as_ref().map(Metadata::from_track);
        let track_changed = track != self.track;
        if track_changed {
            // The old track's position and length do not carry over.
            self.track = track.clone();
            self.metadata = track.clone();
            self.position = None;
            self.duration = None;
            self.published = None;
            updates.push(Update::Metadata(track));
        }
        let state = match snapshot.status {
            StatusDto::Playing | StatusDto::Loading => State::Playing,
            StatusDto::Paused => State::Paused,
            StatusDto::Stopped | StatusDto::Unavailable => State::Stopped,
        };
        let state = if snapshot.playing.is_none() {
            State::Stopped
        } else {
            state
        };
        if Some(state) != self.state || track_changed {
            self.state = Some(state);
            updates.push(self.playback(now));
        }
        updates
    }

    /// A `progress` event: a jump (seek) or the periodic refresh while
    /// playing is published; the steady advance is left to the OS.
    pub fn progress(&mut self, progress: &ProgressDto, now: Instant) -> Vec<Update> {
        self.position = progress.position_ms.map(Duration::from_millis);
        let duration = progress.duration_ms.map(Duration::from_millis);
        let mut updates = Vec::new();
        if duration != self.duration {
            self.duration = duration;
            // A track whose length only mpv knows gets it now.
            if let Some(m) = self.metadata.as_mut().filter(|m| m.duration.is_none())
                && duration.is_some()
            {
                m.duration = duration;
                updates.push(Update::Metadata(Some(m.clone())));
            }
        }
        let Some(position) = self.position else {
            return updates;
        };
        let due = match self.published {
            None => true,
            Some((at, when)) => {
                let elapsed = if self.state() == State::Playing {
                    now.saturating_duration_since(when)
                } else {
                    Duration::ZERO
                };
                let expected = at + elapsed;
                let off = position.abs_diff(expected);
                off > JUMP || (self.state() == State::Playing && elapsed >= REFRESH)
            }
        };
        if due && self.state() != State::Stopped {
            updates.push(self.playback(now));
        }
        updates
    }

    fn playback(&mut self, now: Instant) -> Update {
        let state = self.state();
        let position = (state != State::Stopped).then_some(self.position).flatten();
        self.published = position.map(|p| (p, now));
        Update::Playback { state, position }
    }
}

/// What a media key or widget asks for, as an engine intent (`None`:
/// nothing to do, e.g. Play while already playing).
pub fn intent_for(event: &MediaControlEvent, state: State) -> Option<Intent> {
    let playing = state == State::Playing;
    Some(match event {
        MediaControlEvent::Toggle => Intent::TogglePause,
        MediaControlEvent::Play if !playing => Intent::TogglePause,
        MediaControlEvent::Pause | MediaControlEvent::Stop if playing => Intent::TogglePause,
        MediaControlEvent::Next => Intent::Next,
        MediaControlEvent::Previous => Intent::Previous,
        MediaControlEvent::Seek(direction) => Intent::SeekBy {
            seconds: signed(*direction, SEEK_STEP),
        },
        MediaControlEvent::SeekBy(direction, by) => Intent::SeekBy {
            seconds: signed(*direction, by.as_secs_f64()),
        },
        MediaControlEvent::SetPosition(MediaPosition(at)) => Intent::SeekTo {
            seconds: at.as_secs_f64(),
        },
        MediaControlEvent::SetVolume(volume) if volume.is_finite() => Intent::SetVolume {
            volume: (volume.clamp(0.0, 1.0) * 100.0).round() as u8,
        },
        _ => return None,
    })
}

fn signed(direction: SeekDirection, seconds: f64) -> f64 {
    match direction {
        SeekDirection::Forward => seconds,
        SeekDirection::Backward => -seconds,
    }
}

thread_local! {
    /// The controls, on the main thread only.
    static CONTROLS: RefCell<Option<MediaControls>> = const { RefCell::new(None) };
}

/// The media controls service, fed by the engine's sink.
pub struct Media {
    app: AppHandle,
    covers: Arc<CoverService>,
    /// Where the published cover JPEGs are written (the GUI's own folder).
    art_dir: Option<PathBuf>,
    publisher: Arc<Mutex<Publisher>>,
    /// Bumped per track, so a slow cover load for an old track is dropped.
    art_generation: Arc<AtomicU64>,
    /// The published art: the track's cover URL and its `file://` URL.
    art: Arc<Mutex<Option<(String, String)>>>,
}

impl Media {
    /// Creates the controls on the main thread and attaches the handler.
    /// Failures are logged; the app runs without media controls.
    pub fn start(
        app: &AppHandle,
        engine: EngineHandle,
        covers: Arc<CoverService>,
        art_dir: Option<PathBuf>,
    ) -> Self {
        let publisher = Arc::new(Mutex::new(Publisher::default()));
        let handler_state = publisher.clone();
        let handler_app = app.clone();
        let result = app.run_on_main_thread(move || {
            let config = PlatformConfig {
                display_name: "Coda",
                dbus_name: DBUS_NAME,
                hwnd: None,
            };
            let mut controls = match MediaControls::new(config) {
                Ok(controls) => controls,
                Err(error) => {
                    tracing::warn!(?error, "media controls unavailable");
                    return;
                }
            };
            let attached = controls.attach(move |event| {
                handle_event(&handler_app, &engine, &handler_state, event);
            });
            if let Err(error) = attached {
                tracing::warn!(?error, "media controls could not be attached");
                return;
            }
            tracing::info!("media controls ready");
            CONTROLS.with(|c| *c.borrow_mut() = Some(controls));
        });
        if let Err(error) = result {
            tracing::warn!(%error, "media controls not started");
        }
        Self {
            app: app.clone(),
            covers,
            art_dir,
            publisher,
            art_generation: Arc::new(AtomicU64::new(0)),
            art: Arc::new(Mutex::new(None)),
        }
    }

    pub fn snapshot(&self, snapshot: &Snapshot) {
        let updates = lock(&self.publisher).snapshot(snapshot, Instant::now());
        self.publish(updates);
    }

    pub fn progress(&self, progress: &ProgressDto) {
        let updates = lock(&self.publisher).progress(progress, Instant::now());
        self.publish(updates);
    }

    fn publish(&self, updates: Vec<Update>) {
        for update in updates {
            let mut art = None;
            if let Update::Metadata(metadata) = &update {
                let cover_url = metadata.as_ref().and_then(|m| m.cover_url.clone());
                let mut current = self.art.lock().unwrap_or_else(|e| e.into_inner());
                match current.as_ref() {
                    // Same cover (e.g. only the length changed): keep it.
                    Some((url, file)) if Some(url) == cover_url.as_ref() => {
                        art = Some(file.clone());
                    }
                    _ => {
                        *current = None;
                        let generation = self.art_generation.fetch_add(1, Ordering::SeqCst) + 1;
                        if let Some(metadata) = metadata.clone().filter(|m| m.cover_url.is_some()) {
                            self.load_art(metadata, generation);
                        }
                    }
                }
            }
            self.apply(update, art);
        }
    }

    /// Loads the cover, writes it as a JPEG and publishes the metadata
    /// again with its `file://` URL.
    fn load_art(&self, metadata: Metadata, generation: u64) {
        let (Some(dir), Some(cover_url)) = (self.art_dir.clone(), metadata.cover_url.clone())
        else {
            return;
        };
        let covers = self.covers.clone();
        let current = self.art_generation.clone();
        let art = self.art.clone();
        let publisher = self.publisher.clone();
        let media = MediaRef {
            app: self.app.clone(),
        };
        tauri::async_runtime::spawn(async move {
            let Some(image) = covers.image_for_url(&cover_url, CoverSize::Small).await else {
                return;
            };
            let written = tokio::task::spawn_blocking(move || write_art(&dir, generation, &image))
                .await
                .ok()
                .flatten();
            if let Some(path) = written
                && let Ok(url) = tauri::Url::from_file_path(&path)
            {
                let mut published = art.lock().unwrap_or_else(|e| e.into_inner());
                if current.load(Ordering::SeqCst) != generation {
                    return;
                }
                *published = Some((cover_url, url.to_string()));
                // Still holding `art`: a track change (which takes it to
                // bump the generation) cannot post its metadata before this
                // post, so this old track's metadata can never land last.
                // `run_on_main_thread` only posts, so no deadlock.
                // The newest metadata of this track (its length may have
                // come in meanwhile).
                let latest = lock(&publisher)
                    .metadata
                    .clone()
                    .filter(|m| m.cover_url == metadata.cover_url)
                    .unwrap_or(metadata);
                media.apply(Update::Metadata(Some(latest)), Some(url.to_string()));
                drop(published);
            }
        });
    }

    fn apply(&self, update: Update, art: Option<String>) {
        MediaRef {
            app: self.app.clone(),
        }
        .apply(update, art);
    }

    /// Drops the controls (detaching them). Call on the main thread at
    /// exit.
    pub fn stop() {
        CONTROLS.with(|c| c.borrow_mut().take());
    }
}

/// What a background task needs to publish.
struct MediaRef {
    app: AppHandle,
}

impl MediaRef {
    fn apply(&self, update: Update, art: Option<String>) {
        let _ = self.app.run_on_main_thread(move || {
            CONTROLS.with(|c| {
                let mut c = c.borrow_mut();
                let Some(controls) = c.as_mut() else {
                    return;
                };
                let result = match &update {
                    Update::Metadata(None) => controls.set_metadata(MediaMetadata::default()),
                    Update::Metadata(Some(m)) => controls.set_metadata(MediaMetadata {
                        title: Some(&m.title),
                        artist: m.artist.as_deref(),
                        album: m.album.as_deref(),
                        cover_url: art.as_deref(),
                        duration: m.duration,
                    }),
                    Update::Playback { state, position } => {
                        let progress = position.map(MediaPosition);
                        controls.set_playback(match state {
                            State::Stopped => MediaPlayback::Stopped,
                            State::Playing => MediaPlayback::Playing { progress },
                            State::Paused => MediaPlayback::Paused { progress },
                        })
                    }
                };
                if let Err(error) = result {
                    tracing::debug!(?error, "media controls update failed");
                }
            });
        });
    }
}

fn handle_event(
    app: &AppHandle,
    engine: &EngineHandle,
    publisher: &Mutex<Publisher>,
    event: MediaControlEvent,
) {
    match event {
        MediaControlEvent::Raise => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        // The normal quit path (final save, then mpv).
        MediaControlEvent::Quit => app.exit(0),
        event => {
            let state = lock(publisher).state();
            if let Some(intent) = intent_for(&event, state) {
                let engine = engine.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = engine.intent(intent).await;
                });
            }
        }
    }
}

fn lock(publisher: &Mutex<Publisher>) -> std::sync::MutexGuard<'_, Publisher> {
    publisher.lock().unwrap_or_else(|e| e.into_inner())
}

/// Writes the cover as `<dir>/cover-<generation>.jpg` (0600, in a 0700
/// folder), removing older ones. A new name per track, so the OS never
/// reads a file while it is replaced.
fn write_art(
    dir: &std::path::Path,
    generation: u64,
    image: &coda_core::cover::CoverImage,
) -> Option<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let bytes = coda_core::cover::encode_jpeg(image).ok()?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .ok()?;
    let name = format!("cover-{generation}.jpg");
    let path = dir.join(&name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .ok()?;
    file.write_all(&bytes).ok()?;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let other = entry.file_name();
            let other = other.to_string_lossy();
            if other.starts_with("cover-") && other.ends_with(".jpg") && other != name {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::*;

    fn track(title: &str) -> TrackDto {
        TrackDto {
            source: SourceDto::Local {
                path: format!("/m/{title}.flac"),
            },
            title: title.into(),
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            album_artist: None,
            track_no: None,
            disc_no: None,
            duration_ms: Some(200_000),
            cover_url: Some("coda://localhost/cover/00000000000000ff".into()),
        }
    }

    fn snapshot(playing: Option<TrackDto>, status: StatusDto) -> Snapshot {
        Snapshot {
            phase: Phase::Ready,
            status,
            current_key: None,
            playing_key: None,
            up_next_key: None,
            playing,
            queue: None,
            volume: 100,
            repeat: RepeatDto::Off,
            scan: ScanDto {
                state: ScanStateDto::NoLibrary,
                tracks: 0,
                playlists: 0,
                skipped: 0,
                errors: 0,
            },
            navidrome: NavidromeDto {
                state: ConnectionDto::NotConfigured,
                message: None,
            },
            problems: Vec::new(),
            notices: Vec::new(),
            visualizer_available: true,
        }
    }

    fn progress(ms: u64) -> ProgressDto {
        ProgressDto {
            position_ms: Some(ms),
            duration_ms: Some(200_000),
        }
    }

    #[test]
    fn track_and_status_changes_are_published_once() {
        let mut p = Publisher::default();
        let t0 = Instant::now();
        let updates = p.snapshot(&snapshot(Some(track("A")), StatusDto::Playing), t0);
        assert!(matches!(&updates[0], Update::Metadata(Some(m)) if m.title == "A"));
        assert_eq!(
            updates[1],
            Update::Playback {
                state: State::Playing,
                position: None
            }
        );
        // The same snapshot again: nothing.
        assert!(
            p.snapshot(&snapshot(Some(track("A")), StatusDto::Playing), t0)
                .is_empty()
        );
        // Paused: status only.
        assert_eq!(
            p.snapshot(&snapshot(Some(track("A")), StatusDto::Paused), t0),
            [Update::Playback {
                state: State::Paused,
                position: None
            }]
        );
        // Nothing playing: cleared and stopped.
        let updates = p.snapshot(&snapshot(None, StatusDto::Stopped), t0);
        assert_eq!(
            updates,
            [
                Update::Metadata(None),
                Update::Playback {
                    state: State::Stopped,
                    position: None
                }
            ]
        );
    }

    #[test]
    fn a_new_track_starts_without_the_old_position_or_length() {
        let mut p = Publisher::default();
        let t0 = Instant::now();
        let mut a = track("A");
        a.duration_ms = None;
        p.snapshot(&snapshot(Some(a.clone()), StatusDto::Playing), t0);
        // mpv's length fills in a track without one.
        let updates = p.progress(&progress(90_000), t0);
        assert!(matches!(
            &updates[0],
            Update::Metadata(Some(m)) if m.duration == Some(Duration::from_secs(200))
        ));
        // The same track again is no change, although the length was filled.
        assert!(
            p.snapshot(&snapshot(Some(a), StatusDto::Playing), t0)
                .is_empty()
        );
        let mut b = track("B");
        b.duration_ms = None;
        let updates = p.snapshot(&snapshot(Some(b), StatusDto::Playing), t0);
        assert!(matches!(&updates[0], Update::Metadata(Some(m)) if m.duration.is_none()));
        assert_eq!(
            updates[1],
            Update::Playback {
                state: State::Playing,
                position: None
            }
        );
    }

    #[test]
    fn positions_are_published_on_jumps_and_periodically() {
        let mut p = Publisher::default();
        let t0 = Instant::now();
        p.snapshot(&snapshot(Some(track("A")), StatusDto::Playing), t0);
        // The first position is published.
        assert_eq!(p.progress(&progress(1_000), t0).len(), 1);
        // The steady advance is not.
        let t1 = t0 + Duration::from_secs(1);
        assert!(p.progress(&progress(2_000), t1).is_empty());
        // A seek is.
        assert_eq!(
            p.progress(&progress(60_000), t1),
            [Update::Playback {
                state: State::Playing,
                position: Some(Duration::from_secs(60))
            }]
        );
        // And the periodic refresh.
        let t2 = t1 + REFRESH;
        assert_eq!(p.progress(&progress(65_000), t2).len(), 1);
        // Paused, the position does not advance; only a seek publishes.
        p.snapshot(&snapshot(Some(track("A")), StatusDto::Paused), t2);
        let t3 = t2 + REFRESH * 2;
        assert!(p.progress(&progress(65_000), t3).is_empty());
        assert_eq!(p.progress(&progress(10_000), t3).len(), 1);
    }

    #[test]
    fn media_keys_become_intents() {
        use MediaControlEvent as E;
        assert_eq!(
            intent_for(&E::Play, State::Paused),
            Some(Intent::TogglePause)
        );
        assert_eq!(intent_for(&E::Play, State::Playing), None);
        assert_eq!(intent_for(&E::Pause, State::Paused), None);
        assert_eq!(
            intent_for(&E::Pause, State::Playing),
            Some(Intent::TogglePause)
        );
        assert_eq!(
            intent_for(&E::Toggle, State::Stopped),
            Some(Intent::TogglePause)
        );
        assert_eq!(intent_for(&E::Next, State::Stopped), Some(Intent::Next));
        assert_eq!(
            intent_for(&E::Seek(SeekDirection::Backward), State::Playing),
            Some(Intent::SeekBy { seconds: -10.0 })
        );
        assert_eq!(
            intent_for(
                &E::SetPosition(MediaPosition(Duration::from_secs(42))),
                State::Playing
            ),
            Some(Intent::SeekTo { seconds: 42.0 })
        );
        assert_eq!(
            intent_for(&E::SetVolume(0.5), State::Playing),
            Some(Intent::SetVolume { volume: 50 })
        );
        assert_eq!(intent_for(&E::SetVolume(f64::NAN), State::Playing), None);
    }

    #[test]
    fn art_files_are_private_and_replaced() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let art = dir.path().join("media");
        let image = coda_core::cover::CoverImage::new(2, 2, vec![10; 12]).unwrap();
        let first = write_art(&art, 1, &image).unwrap();
        let second = write_art(&art, 2, &image).unwrap();
        assert!(!first.exists(), "the old cover is removed");
        let mode = std::fs::metadata(&second).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let mode = std::fs::metadata(&art).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
