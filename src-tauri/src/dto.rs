//! What the frontend sees: serializable DTOs (camelCase JSON), mirrored by
//! hand in `src/lib/types.ts`. Library types never cross the IPC boundary.
//!
//! Nothing here may carry a password, token or signed stream URL: tracks
//! name their source by local path or Navidrome song id only.

use serde::{Deserialize, Serialize};

/// Where a track comes from, as the UI names it. Sent back unchanged in
/// [`TrackRef`]s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SourceDto {
    /// A file in the local library, by its absolute path (lossy UTF-8).
    Local { path: String },
    /// A Navidrome song. `cover_art` is the server's opaque cover id.
    Navidrome {
        song_id: String,
        #[serde(default)]
        cover_art: Option<String>,
    },
}

/// A track for display. Serializes to the shape a [`TrackRef`] accepts,
/// so the UI can hand a `TrackDto` it got back in an intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDto {
    pub source: SourceDto,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub duration_ms: Option<u64>,
    /// Base URL of the cover (`<base>/small` or `<base>/large`), served by
    /// our `coda` URI scheme; `None` without art or with covers disabled.
    pub cover_url: Option<String>,
}

/// A track the UI asks to play or enqueue: a [`TrackDto`] it received
/// (unknown fields such as `coverUrl` are ignored). A local track is
/// looked up in the scanned library by path, so its metadata comes from
/// the scan, not from the UI; a Navidrome track is built from the
/// metadata given (the backend keeps no browse cache), which is cleaned
/// and capped like server data.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackRef {
    pub source: SourceDto,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub album_artist: Option<String>,
    #[serde(default)]
    pub track_no: Option<u32>,
    #[serde(default)]
    pub disc_no: Option<u32>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// One queue entry. `key` is stable for the entry's lifetime in this run
/// (an opaque string such as `q12`), so the UI can use it as a React key
/// and in intents; it is not stable across restarts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItemDto {
    pub key: String,
    pub track: TrackDto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Config, password, mpv and the session are being loaded.
    Starting,
    Ready,
    /// Quitting: final save and mpv shutdown under way.
    ShuttingDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StatusDto {
    Stopped,
    Loading,
    Playing,
    Paused,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RepeatDto {
    Off,
    All,
    One,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanStateDto {
    /// No music folder configured.
    NoLibrary,
    Scanning,
    Finished,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanDto {
    pub state: ScanStateDto,
    pub tracks: usize,
    pub playlists: usize,
    pub skipped: usize,
    pub errors: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionDto {
    NotConfigured,
    Connecting,
    Connected,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NavidromeDto {
    pub state: ConnectionDto,
    /// Why the connection failed (never credentials).
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProblemKind {
    /// No music folder or Navidrome server configured (onboarding later).
    NoConfig,
    /// The config file could not be used.
    Config,
    /// mpv is not installed (or not on `PATH`).
    MpvMissing,
    /// mpv could not be started, or exited; playback is unavailable.
    Playback,
    /// The Navidrome password could not be obtained, or the client failed.
    Navidrome,
}

/// A persistent problem, shown as a banner until it is resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProblemDto {
    pub kind: ProblemKind,
    pub message: String,
}

/// A transient message (player messages, errors), shown as a toast. Ids
/// increase; the backend drops a notice after a few seconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoticeDto {
    pub id: u64,
    pub text: String,
}

/// The full UI state, sent as the `snapshot` event (coalesced, only when
/// something changed) and returned by the `snapshot` command.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub phase: Phase,
    pub status: StatusDto,
    /// The queue's current entry (what plays, or would play on Space).
    pub current_key: Option<String>,
    /// The entry heard; differs from `current_key` during a gapless change.
    pub playing_key: Option<String>,
    pub up_next_key: Option<String>,
    /// The track heard, else the current one.
    pub playing: Option<TrackDto>,
    /// The queue in order. In `snapshot` events it is `null` when the
    /// entries are unchanged since the previous event (keep the last one);
    /// the `snapshot` command always includes it.
    pub queue: Option<Vec<QueueItemDto>>,
    /// Effective volume, 0..=100.
    pub volume: u8,
    pub repeat: RepeatDto,
    pub scan: ScanDto,
    pub navidrome: NavidromeDto,
    pub problems: Vec<ProblemDto>,
    pub notices: Vec<NoticeDto>,
    /// `false` once mpv said it cannot provide visualizer data.
    pub visualizer_available: bool,
}

/// The `progress` event: the heard track's position (at most 10 per
/// second, only when it changed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressDto {
    pub position_ms: Option<u64>,
    pub duration_ms: Option<u64>,
}

/// The `visualizer` event: one raw reading for the audible track (up to
/// 30 per second, only while a view asked for it). Values are 0..=1;
/// `null` bands are above the track's Nyquist frequency.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualizerDto {
    pub levels: [f32; 2],
    pub bands: [Option<f32>; coda_core::playback::VISUALIZER_BANDS],
}

/// What the UI asks the engine to do (the `intent` command's argument),
/// tagged by `type`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Intent {
    TogglePause,
    Next,
    Previous,
    /// Seek to `seconds` from the start of the heard track.
    SeekTo {
        seconds: f64,
    },
    /// Seek by `seconds` (negative: back).
    SeekBy {
        seconds: f64,
    },
    SetVolume {
        volume: u8,
    },
    CycleRepeat,
    PlayEntry {
        key: String,
    },
    RemoveEntry {
        key: String,
    },
    ClearQueue,
    /// A view showing the visualizer appeared (`true`) or went away.
    SetVisualizerVisible {
        visible: bool,
    },
    /// Replace the queue with `tracks` and play from `start` (shuffled with
    /// `start` first, or all shuffled without a start, when `shuffle`).
    PlayTracks {
        tracks: Vec<TrackRef>,
        #[serde(default)]
        start: Option<usize>,
        #[serde(default)]
        shuffle: bool,
    },
    /// Add `tracks` at the end, or after the current entry when `next`.
    Enqueue {
        tracks: Vec<TrackRef>,
        #[serde(default)]
        next: bool,
    },
    DismissNotice {
        id: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn track() -> TrackDto {
        TrackDto {
            source: SourceDto::Navidrome {
                song_id: "s1".into(),
                cover_art: Some("al-1".into()),
            },
            title: "Song".into(),
            artist: Some("Artist".into()),
            album: None,
            album_artist: None,
            track_no: Some(3),
            disc_no: None,
            duration_ms: Some(61_500),
            cover_url: Some("coda://localhost/cover/00ff".into()),
        }
    }

    #[test]
    fn track_dto_serializes_camel_case() {
        let value = serde_json::to_value(track()).unwrap();
        assert_eq!(
            value,
            json!({
                "source": {"kind": "navidrome", "songId": "s1", "coverArt": "al-1"},
                "title": "Song",
                "artist": "Artist",
                "album": null,
                "albumArtist": null,
                "trackNo": 3,
                "discNo": null,
                "durationMs": 61500,
                "coverUrl": "coda://localhost/cover/00ff",
            })
        );
        let local = SourceDto::Local {
            path: "/m/a.flac".into(),
        };
        assert_eq!(
            serde_json::to_value(local).unwrap(),
            json!({"kind": "local", "path": "/m/a.flac"})
        );
    }

    #[test]
    fn a_track_dto_round_trips_as_a_track_ref() {
        let value = serde_json::to_value(track()).unwrap();
        let back: TrackRef = serde_json::from_value(value).unwrap();
        assert_eq!(back.title.as_deref(), Some("Song"));
        assert_eq!(back.duration_ms, Some(61_500));
        assert_eq!(
            back.source,
            SourceDto::Navidrome {
                song_id: "s1".into(),
                cover_art: Some("al-1".into())
            }
        );
        // A bare local ref is enough.
        let local: TrackRef =
            serde_json::from_value(json!({"source": {"kind": "local", "path": "/m/a.flac"}}))
                .unwrap();
        assert_eq!(local.title, None);
    }

    #[test]
    fn intents_deserialize_from_tagged_json() {
        let cases = [
            (json!({"type": "togglePause"}), Intent::TogglePause),
            (
                json!({"type": "seekTo", "seconds": 12.5}),
                Intent::SeekTo { seconds: 12.5 },
            ),
            (
                json!({"type": "setVolume", "volume": 40}),
                Intent::SetVolume { volume: 40 },
            ),
            (
                json!({"type": "playEntry", "key": "q3"}),
                Intent::PlayEntry { key: "q3".into() },
            ),
            (
                json!({"type": "setVisualizerVisible", "visible": true}),
                Intent::SetVisualizerVisible { visible: true },
            ),
            (
                json!({"type": "enqueue", "tracks": [], "next": true}),
                Intent::Enqueue {
                    tracks: vec![],
                    next: true,
                },
            ),
            (
                json!({"type": "playTracks", "tracks": []}),
                Intent::PlayTracks {
                    tracks: vec![],
                    start: None,
                    shuffle: false,
                },
            ),
            (
                json!({"type": "dismissNotice", "id": 7}),
                Intent::DismissNotice { id: 7 },
            ),
        ];
        for (value, intent) in cases {
            assert_eq!(serde_json::from_value::<Intent>(value).unwrap(), intent);
        }
        assert!(
            serde_json::from_value::<Intent>(json!({"type": "setVolume", "volume": 300})).is_err()
        );
        assert!(serde_json::from_value::<Intent>(json!({"type": "explode"})).is_err());
    }

    #[test]
    fn snapshot_serializes_camel_case() {
        let snapshot = Snapshot {
            phase: Phase::Ready,
            status: StatusDto::Paused,
            current_key: Some("q1".into()),
            playing_key: Some("q1".into()),
            up_next_key: None,
            playing: None,
            queue: Some(vec![QueueItemDto {
                key: "q1".into(),
                track: track(),
            }]),
            volume: 80,
            repeat: RepeatDto::All,
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
            problems: vec![ProblemDto {
                kind: ProblemKind::MpvMissing,
                message: "mpv was not found".into(),
            }],
            notices: vec![NoticeDto {
                id: 1,
                text: "Repeat all".into(),
            }],
            visualizer_available: true,
        };
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value["phase"], "ready");
        assert_eq!(value["status"], "paused");
        assert_eq!(value["currentKey"], "q1");
        assert_eq!(value["upNextKey"], json!(null));
        assert_eq!(value["queue"][0]["key"], "q1");
        assert_eq!(value["queue"][0]["track"]["title"], "Song");
        assert_eq!(value["repeat"], "all");
        assert_eq!(value["scan"]["state"], "noLibrary");
        assert_eq!(value["navidrome"]["state"], "notConfigured");
        assert_eq!(value["problems"][0]["kind"], "mpvMissing");
        assert_eq!(value["notices"][0], json!({"id": 1, "text": "Repeat all"}));
        assert_eq!(value["visualizerAvailable"], true);
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys.len(), 14, "{keys:?}");
    }

    #[test]
    fn progress_and_visualizer_serialize_camel_case() {
        let progress = ProgressDto {
            position_ms: Some(1500),
            duration_ms: None,
        };
        assert_eq!(
            serde_json::to_value(progress).unwrap(),
            json!({"positionMs": 1500, "durationMs": null})
        );
        let frame = VisualizerDto {
            levels: [0.5, 0.25],
            bands: [Some(0.5), None, None, None, None, None, None],
        };
        assert_eq!(
            serde_json::to_value(frame).unwrap(),
            json!({"levels": [0.5, 0.25], "bands": [0.5, null, null, null, null, null, null]})
        );
    }
}
