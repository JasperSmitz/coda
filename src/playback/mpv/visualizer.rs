//! The visualizer's mpv side. Pure: no I/O and no Tokio; [`super::state`]
//! drives it and sends the requests it asks for.
//!
//! How it works (M6 spike findings, `docs/plans/m6-visualizer.md`):
//! - [`GRAPH`] is added with `af add` the first time the visualizer is
//!   wanted while a track plays. mpv checks the graph then and refuses a
//!   bad one, leaving playback alone. It is never passed at spawn or added
//!   while idle: mpv accepts a bad graph unchecked then, and every later
//!   file fails. mpv always outputs stereo (`--audio-channels=stereo`), and
//!   the graph's first filter converts to stereo the same way, so adding
//!   it never changes what is heard.
//! - The graph's `astats` reports per-channel levels as frame metadata.
//!   Channels 1–2 are left and right, 3–9 the bands (a mono downmix through
//!   one `bandpass` each), and 10 is a time code: `mod(t, 32) / 32`, read
//!   back from its peak level in dBFS.
//! - The driver polls `af-metadata/viz` at most every [`POLL_INTERVAL`],
//!   one request at a time, only while the visualizer is wanted, attached
//!   and a track is playing. (Observing the property instead delivers at
//!   most about 15 changes a second and keeps sending while hidden.)
//! - The analysed audio runs ahead of what is heard (about 0.26 s on
//!   CoreAudio, seconds on slow outputs). Each reading is held until the
//!   playback position, extrapolated from mpv's `time-pos`, reaches its
//!   time code.
//! - Removing the graph drops the audio inside it (23–55 ms), an audible
//!   skip, so it is removed only at a track change or stop while hidden.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::playback::{
    LoadId, PlaybackEvent, VISUALIZER_BAND_HZ, VISUALIZER_BANDS, VisualizerFrame,
};

/// The filter graph added for the visualizer: the spike's recommended
/// graph with 2048-sample chunks (about 21–23 readings a second at
/// 44.1/48 kHz). The `format` filter is labelled so that it is removed
/// together with the analysis.
pub const GRAPH: &str = "@vizfmt:format=channels=stereo,@viz:lavfi=[\
asetnsamples=n=2048:p=0,asplit[o][a];\
[a]aformat=channel_layouts=mono,asplit=8[b0][b1][b2][b3][b4][b5][b6][t];\
[b0]bandpass=f=63:width_type=o:w=1.3[f0];\
[b1]bandpass=f=160:width_type=o:w=1.3[f1];\
[b2]bandpass=f=400:width_type=o:w=1.3[f2];\
[b3]bandpass=f=1000:width_type=o:w=1.3[f3];\
[b4]bandpass=f=2500:width_type=o:w=1.3[f4];\
[b5]bandpass=f=6300:width_type=o:w=1.3[f5];\
[b6]bandpass=f=16000:width_type=o:w=1.3[f6];\
[t]aeval=exprs=mod(t\\,32)/32[tc];\
[o][f0][f1][f2][f3][f4][f5][f6][tc]amerge=inputs=9,\
astats=metadata=1:reset=1:measure_perchannel=RMS_level+Peak_level:measure_overall=none,\
pan=stereo|c0=c0|c1=c1]";

/// Labels of every filter in [`GRAPH`], for `af remove`.
pub const GRAPH_LABELS: &str = "@vizfmt,@viz";

/// The metadata the graph's `astats` publishes.
pub const METADATA_PROPERTY: &str = "af-metadata/viz";

/// Shortest spacing between polls (at most about 30 a second).
pub const POLL_INTERVAL: Duration = Duration::from_millis(33);

/// `astats` channel (1-based) of the first band and of the time code.
const FIRST_BAND_CHANNEL: usize = 3;
const TIME_CODE_CHANNEL: usize = FIRST_BAND_CHANNEL + VISUALIZER_BANDS;
/// The time code repeats every this many seconds.
const TIME_CODE_PERIOD: f64 = 32.0;

/// dBFS mapped to 0 and 1. Bands are quieter than the whole signal (each
/// holds part of it), so they reach full height sooner.
const LEVEL_FLOOR_DB: f64 = -60.0;
const LEVEL_CEILING_DB: f64 = 0.0;
const BAND_FLOOR_DB: f64 = -60.0;
const BAND_CEILING_DB: f64 = -10.0;
/// Longer metadata strings are not numbers mpv would write.
const MAX_VALUE_CHARS: usize = 32;

/// Readings further ahead of the playback position than this are not
/// believed (the slowest output seen buffers about 3 s).
const MAX_LEAD: f64 = 5.0;
/// A reading already this far behind the position is too late to show.
const MAX_LAG: f64 = 0.25;
/// Readings waiting to be shown; at 23 a second this covers the lead.
const MAX_PENDING: usize = 128;
/// The position is extrapolated from the last `time-pos` for at most this
/// long (mpv reports it many times a second while playing).
const MAX_EXTRAPOLATION: Duration = Duration::from_secs(1);

/// One parsed `af-metadata/viz` value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Seconds modulo [`TIME_CODE_PERIOD`] of the end of the analysed
    /// chunk.
    pub time_code: f64,
    pub frame: VisualizerFrame,
}

/// Parses an untrusted `af-metadata/viz` value. Values are dBFS strings
/// (`"-23.08"`, `"-inf"` for silence). A missing, malformed or non-finite
/// level counts as silence; a sample without a usable time code is `None`,
/// because it cannot be placed in time. Extra keys are ignored.
pub fn parse_metadata(value: &Value) -> Option<Sample> {
    let map = value.as_object()?;
    let time_code = time_code(peak(map, TIME_CODE_CHANNEL))?;
    let level = |channel| normalise(rms(map, channel), LEVEL_FLOOR_DB, LEVEL_CEILING_DB);
    let mut frame = VisualizerFrame {
        levels: [level(1), level(2)],
        bands: [None; VISUALIZER_BANDS],
    };
    for (i, band) in frame.bands.iter_mut().enumerate() {
        *band = Some(normalise(
            rms(map, FIRST_BAND_CHANNEL + i),
            BAND_FLOOR_DB,
            BAND_CEILING_DB,
        ));
    }
    Some(Sample { time_code, frame })
}

fn rms(map: &Map<String, Value>, channel: usize) -> Option<f64> {
    decibels(map.get(&format!("lavfi.astats.{channel}.RMS_level")))
}

fn peak(map: &Map<String, Value>, channel: usize) -> Option<f64> {
    decibels(map.get(&format!("lavfi.astats.{channel}.Peak_level")))
}

/// A dB value as mpv writes it (a string), or a plain JSON number. May be
/// infinite or NaN; callers decide what that means.
fn decibels(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::String(text) if text.len() <= MAX_VALUE_CHARS => text.trim().parse().ok(),
        Value::Number(number) => number.as_f64(),
        _ => None,
    }
}

/// Maps dBFS onto `0.0..=1.0` between `floor` and `ceiling`. Anything that
/// is not a finite number is silence.
fn normalise(db: Option<f64>, floor: f64, ceiling: f64) -> f32 {
    match db {
        Some(db) if db.is_finite() => {
            ((db.clamp(floor, ceiling) - floor) / (ceiling - floor)) as f32
        }
        _ => 0.0,
    }
}

/// Seconds modulo the period from the time-code channel's peak level:
/// `t = 32 · 10^(dB/20)`. `-inf` is 0; NaN, `+inf` and values beyond the
/// period are rejected.
fn time_code(db: Option<f64>) -> Option<f64> {
    let db = db?;
    if db == f64::NEG_INFINITY {
        return Some(0.0);
    }
    if !db.is_finite() {
        return None;
    }
    let seconds = TIME_CODE_PERIOD * 10f64.powf(db / 20.0);
    // `astats` rounds; allow a hair over the top before rejecting.
    (seconds <= TIME_CODE_PERIOD * 1.001).then(|| seconds.min(TIME_CODE_PERIOD))
}

/// The presentation time nearest `position` whose value modulo the period
/// is `time_code`.
fn unwrap_time_code(time_code: f64, position: f64) -> f64 {
    let periods = ((position - time_code) / TIME_CODE_PERIOD).round();
    time_code + periods * TIME_CODE_PERIOD
}

/// The playback position, extrapolated between mpv's `time-pos` reports.
#[derive(Debug, Default)]
struct PlayClock {
    anchor: Option<(f64, Instant)>,
    paused: bool,
}

impl PlayClock {
    fn observe(&mut self, position: f64, now: Instant) {
        self.anchor = Some((position, now));
    }

    fn position(&self, now: Instant) -> Option<f64> {
        let (position, at) = self.anchor?;
        if self.paused {
            return Some(position);
        }
        let elapsed = now.saturating_duration_since(at).min(MAX_EXTRAPOLATION);
        Some(position + elapsed.as_secs_f64())
    }

    fn set_paused(&mut self, paused: bool, now: Instant) {
        if paused != self.paused {
            self.anchor = self.position(now).map(|position| (position, now));
            self.paused = paused;
        }
    }

    fn clear(&mut self) {
        self.anchor = None;
    }
}

/// Readings waiting for the playback position to reach them.
#[derive(Debug, Default)]
struct Aligner {
    pending: VecDeque<(f64, VisualizerFrame)>,
    /// The last time code polled. mpv keeps the last value readable until
    /// the graph produces a new one (also across pauses and seeks), so the
    /// same time code again is not a new reading.
    last_time_code: Option<f64>,
    last_shown: Option<f64>,
}

impl Aligner {
    /// Queues `sample` if it is new and plausibly near `position`.
    /// Returns `true` if it was queued.
    fn offer(&mut self, sample: Sample, position: f64) -> bool {
        if self.last_time_code == Some(sample.time_code) {
            return false;
        }
        self.last_time_code = Some(sample.time_code);
        let at = unwrap_time_code(sample.time_code, position);
        if at < position - MAX_LAG || at > position + MAX_LEAD {
            tracing::trace!(at, position, "visualizer reading out of range dropped");
            return false;
        }
        let newest = self.pending.back().map(|&(t, _)| t).or(self.last_shown);
        if newest.is_some_and(|newest| at <= newest) {
            return false;
        }
        if self.pending.len() >= MAX_PENDING {
            self.pending.pop_front();
        }
        self.pending.push_back((at, sample.frame));
        true
    }

    /// The newest reading the position has reached, dropping older ones.
    /// Readings (and the last one shown) more than [`MAX_LEAD`] ahead of
    /// the position belong to a position playback left without a seek
    /// Coda saw; they are dropped so they cannot hold back new ones.
    fn take_due(&mut self, position: f64) -> Option<VisualizerFrame> {
        let limit = position + MAX_LEAD;
        if self.last_shown.is_some_and(|at| at > limit) {
            self.last_shown = None;
        }
        self.pending.retain(|&(at, _)| at <= limit);
        let mut due = None;
        while let Some(&(at, _)) = self.pending.front() {
            if at > position {
                break;
            }
            due = self.pending.pop_front();
        }
        let (at, frame) = due?;
        self.last_shown = Some(at);
        Some(frame)
    }

    /// Forgets queued readings (a seek, a new track, hidden). The last
    /// polled time code is kept, so a value left over from before is not
    /// taken for a new one.
    fn clear(&mut self) {
        self.pending.clear();
        self.last_shown = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attach {
    Detached,
    /// `af add` sent; waiting for the reply.
    Adding,
    Attached,
    /// mpv refused the graph; never retried.
    Unavailable,
}

/// What the driver should do on a visualizer tick.
#[derive(Debug, Default, PartialEq)]
pub struct Tick {
    pub event: Option<PlaybackEvent>,
    /// Send a poll tagged with this generation.
    pub poll: Option<u64>,
}

#[derive(Debug)]
pub struct Visualizer {
    graph: String,
    wanted: bool,
    attach: Attach,
    /// The attached graph has produced a reading. Until then, a track that
    /// fails while it is attached is blamed on the graph (see
    /// [`Self::ended`]).
    verified: bool,
    /// Bumped whenever earlier readings stop counting: replies to polls of
    /// another generation are dropped.
    generation: u64,
    poll_in_flight: bool,
    next_poll: Option<Instant>,
    /// After a load or seek, until mpv's `playback-restart`.
    restarting: bool,
    /// The load whose track mpv is playing (started, not ended).
    playing: Option<LoadId>,
    /// The load most recently sent to mpv; `started` for any other is
    /// ignored.
    expected: Option<LoadId>,
    paused: bool,
    sample_rate: Option<f64>,
    clock: PlayClock,
    aligner: Aligner,
}

impl Default for Visualizer {
    fn default() -> Self {
        Self::new(GRAPH.to_owned())
    }
}

impl Visualizer {
    /// A visualizer that adds `graph` (tests use a broken one).
    pub fn new(graph: String) -> Self {
        Self {
            graph,
            wanted: false,
            attach: Attach::Detached,
            verified: false,
            generation: 0,
            poll_in_flight: false,
            next_poll: None,
            restarting: true,
            playing: None,
            expected: None,
            paused: false,
            sample_rate: None,
            clock: PlayClock::default(),
            aligner: Aligner::default(),
        }
    }

    /// Readings are being polled and shown.
    pub fn active(&self) -> bool {
        self.wanted && self.attach == Attach::Attached && self.playing.is_some() && !self.paused
    }

    /// When the driver should call [`Self::tick`]; `None` while inactive.
    pub fn deadline(&self, now: Instant) -> Option<Instant> {
        self.active().then(|| self.next_poll.unwrap_or(now))
    }

    pub fn set_wanted(&mut self, wanted: bool) {
        self.wanted = wanted;
        if !wanted {
            self.forget_readings();
        }
    }

    /// The graph to add now, if the visualizer is wanted, not yet added
    /// and a track is playing. Marks it as being added.
    pub fn take_attach(&mut self) -> Option<String> {
        let due = self.wanted
            && self.attach == Attach::Detached
            && self.playing.is_some()
            && !self.paused;
        due.then(|| {
            self.attach = Attach::Adding;
            self.graph.clone()
        })
    }

    /// mpv answered `af add`. Returns `true` if the visualizer just became
    /// unavailable (report it once).
    pub fn attach_answered(&mut self, result: Result<(), String>) -> bool {
        if self.attach != Attach::Adding {
            return false;
        }
        match result {
            Ok(()) => {
                tracing::debug!("visualizer attached");
                self.attach = Attach::Attached;
                false
            }
            Err(error) => {
                tracing::warn!(%error, "mpv refused the visualizer filter; visualizer unavailable");
                self.attach = Attach::Unavailable;
                true
            }
        }
    }

    /// A new load (`Some`) or a stop (`None`) is being sent. Returns `true`
    /// if the graph should be removed now (it is attached but not wanted).
    pub fn track_change(&mut self, next: Option<LoadId>) -> bool {
        self.expected = next;
        self.playing = None;
        self.restarting = true;
        self.clock.clear();
        self.forget_readings();
        if !self.wanted && self.attach == Attach::Attached {
            tracing::debug!("detaching hidden visualizer at a track change");
            self.attach = Attach::Detached;
            return true;
        }
        false
    }

    /// A seek command is being sent: replies to polls already out no
    /// longer count. Readings carry on until mpv says it is seeking
    /// ([`Self::seeking`]); mpv answers `success` to a seek it does not
    /// perform (a non-seekable stream) and then sends neither `seek` nor
    /// `playback-restart`.
    pub fn seek_sent(&mut self) {
        self.generation += 1;
    }

    /// mpv's `seek` event: it is seeking, so until `playback-restart`
    /// neither positions nor readings count.
    pub fn seeking(&mut self) {
        self.restarting = true;
        self.clock.clear();
        self.forget_readings();
    }

    pub fn restarted(&mut self) {
        self.restarting = false;
    }

    pub fn started(&mut self, load: LoadId) {
        if self.expected == Some(load) {
            self.playing = Some(load);
        } else {
            tracing::debug!(%load, "visualizer ignores the start of a superseded load");
        }
    }

    #[cfg(test)]
    pub(super) fn restarting_for_tests(&self) -> bool {
        self.restarting
    }

    /// A track is playing (started, not ended).
    pub fn is_playing(&self) -> bool {
        self.playing.is_some()
    }

    /// The track of `load` ended. Returns `true` if the graph is to blame
    /// and must be removed: the track failed while the graph was attached
    /// and before it ever produced a reading. That happens when `af add`
    /// raced the end of a track and mpv took the graph unchecked while
    /// idle; if the graph is bad, every later track would fail too.
    pub fn ended(&mut self, load: LoadId, failed: bool) -> bool {
        if self.playing == Some(load) {
            self.playing = None;
        }
        if failed && self.attach == Attach::Attached && !self.verified {
            tracing::warn!("track failed with an unproven visualizer filter; removing it");
            self.attach = Attach::Unavailable;
            self.forget_readings();
            return true;
        }
        false
    }

    pub fn set_paused(&mut self, paused: bool, now: Instant) {
        self.paused = paused;
        self.clock.set_paused(paused, now);
    }

    pub fn position(&mut self, seconds: f64, now: Instant) {
        if !self.restarting {
            self.clock.observe(seconds, now);
        }
    }

    pub fn set_sample_rate(&mut self, rate: Option<f64>) {
        self.sample_rate = rate.filter(|r| r.is_finite() && *r > 0.0);
    }

    /// Shows the newest due reading and polls again if no poll is out.
    pub fn tick(&mut self, now: Instant) -> Tick {
        if !self.active() {
            self.next_poll = None;
            return Tick::default();
        }
        if self.next_poll.is_some_and(|due| now < due) {
            return Tick::default();
        }
        self.next_poll = Some(now + POLL_INTERVAL);
        let event = match (self.playing, self.restarting, self.clock.position(now)) {
            (Some(load), false, Some(position)) => {
                self.aligner
                    .take_due(position)
                    .map(|frame| PlaybackEvent::Visualizer {
                        load,
                        frame: self.hide_unreachable_bands(frame),
                    })
            }
            _ => None,
        };
        let poll = (!self.poll_in_flight).then(|| {
            self.poll_in_flight = true;
            self.generation
        });
        Tick { event, poll }
    }

    /// mpv answered a poll sent in `generation`.
    pub fn poll_answered(
        &mut self,
        generation: u64,
        result: Result<Option<Value>, String>,
        now: Instant,
    ) {
        self.poll_in_flight = false;
        let value = match result {
            Ok(Some(value)) => value,
            Ok(None) => return,
            Err(error) => {
                tracing::trace!(%error, "visualizer poll failed");
                return;
            }
        };
        let Some(sample) = parse_metadata(&value) else {
            return;
        };
        // mpv ran the graph: it works, whatever happens to this reading.
        self.verified = true;
        if generation != self.generation || self.restarting {
            // Not shown, but remembered: mpv keeps returning this value
            // until the graph produces the next one.
            self.aligner.last_time_code = Some(sample.time_code);
            return;
        }
        if let Some(position) = self.clock.position(now) {
            self.aligner.offer(sample, position);
        }
    }

    fn forget_readings(&mut self) {
        self.generation += 1;
        self.next_poll = None;
        self.aligner.clear();
    }

    /// Bands at or above half the sample rate only echo the whole signal.
    fn hide_unreachable_bands(&self, mut frame: VisualizerFrame) -> VisualizerFrame {
        if let Some(rate) = self.sample_rate {
            for (band, hz) in frame.bands.iter_mut().zip(VISUALIZER_BAND_HZ) {
                if f64::from(hz) >= rate / 2.0 {
                    *band = None;
                }
            }
        }
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// dBFS of a time code `t` seconds into the period.
    fn time_code_db(t: f64) -> String {
        let t = t % TIME_CODE_PERIOD;
        format!("{:.6}", 20.0 * (t / TIME_CODE_PERIOD).log10())
    }

    fn metadata(t: f64, levels: [&str; 2], bands: [&str; 7]) -> Value {
        let mut map = Map::new();
        let mut put = |channel: usize, stat: &str, value: &str| {
            map.insert(
                format!("lavfi.astats.{channel}.{stat}"),
                Value::String(value.to_owned()),
            );
        };
        for (i, db) in levels.iter().enumerate() {
            put(i + 1, "RMS_level", db);
            put(i + 1, "Peak_level", "-1.0");
        }
        for (i, db) in bands.iter().enumerate() {
            put(FIRST_BAND_CHANNEL + i, "RMS_level", db);
            put(FIRST_BAND_CHANNEL + i, "Peak_level", "-1.0");
        }
        put(TIME_CODE_CHANNEL, "RMS_level", "-10.0");
        put(TIME_CODE_CHANNEL, "Peak_level", &time_code_db(t));
        Value::Object(map)
    }

    fn quiet(t: f64) -> Value {
        metadata(t, ["-60"; 2], ["-60"; 7])
    }

    #[test]
    fn graph_matches_the_bands_and_channels() {
        for (i, hz) in VISUALIZER_BAND_HZ.iter().enumerate() {
            assert!(
                GRAPH.contains(&format!("[b{i}]bandpass=f={hz}:width_type=o:w=1.3[f{i}]")),
                "{hz}"
            );
        }
        assert!(GRAPH.contains("asetnsamples=n=2048:p=0"));
        assert!(GRAPH.contains("amerge=inputs=9"));
        assert!(GRAPH.contains(r"mod(t\,32)/32"));
        assert!(GRAPH.starts_with("@vizfmt:format=channels=stereo,@viz:lavfi=["));
        assert!(GRAPH.ends_with("pan=stereo|c0=c0|c1=c1]"));
        assert_eq!(TIME_CODE_CHANNEL, 10);
        assert_eq!(METADATA_PROPERTY, "af-metadata/viz");
        for label in GRAPH_LABELS.split(',') {
            assert!(GRAPH.contains(&format!("{label}:")), "{label}");
        }
        assert!(!GRAPH.contains(char::is_whitespace));
    }

    #[test]
    fn parses_a_real_reply() {
        // Recorded from mpv 0.41 by the spike (chunk 1024, pink noise).
        let reply = json!({
            "lavfi.astats.1.Peak_level":"-15.454231","lavfi.astats.1.RMS_level":"-24.340211",
            "lavfi.astats.2.Peak_level":"-14.981964","lavfi.astats.2.RMS_level":"-24.900994",
            "lavfi.astats.3.Peak_level":"-29.132624","lavfi.astats.3.RMS_level":"-35.846640",
            "lavfi.astats.4.Peak_level":"-29.592137","lavfi.astats.4.RMS_level":"-36.592126",
            "lavfi.astats.5.Peak_level":"-29.860166","lavfi.astats.5.RMS_level":"-36.835339",
            "lavfi.astats.6.Peak_level":"-28.915372","lavfi.astats.6.RMS_level":"-37.841332",
            "lavfi.astats.7.Peak_level":"-28.517696","lavfi.astats.7.RMS_level":"-37.763228",
            "lavfi.astats.8.Peak_level":"-28.827040","lavfi.astats.8.RMS_level":"-38.007937",
            "lavfi.astats.9.Peak_level":"-26.172216","lavfi.astats.9.RMS_level":"-38.023765",
            "lavfi.astats.10.Peak_level":"-27.838234","lavfi.astats.10.RMS_level":"-38.180717",
            "lavfi.astats.11.Peak_level":"-27.903838","lavfi.astats.11.RMS_level":"-37.701484"
        });
        let sample = parse_metadata(&reply).unwrap();
        // 32 · 10^(-27.838234/20)
        assert!(
            (sample.time_code - 1.30).abs() < 0.01,
            "{}",
            sample.time_code
        );
        let left = sample.frame.levels[0];
        assert!((left - (60.0 - 24.340211) / 60.0).abs() < 1e-4, "{left}");
        let low = sample.frame.bands[0].unwrap();
        assert!((low - (60.0 - 35.84664) / 50.0).abs() < 1e-4, "{low}");
        assert!(sample.frame.bands.iter().all(Option::is_some));
    }

    #[test]
    fn hostile_values_never_panic_and_count_as_silence() {
        let hostile = [
            json!("-inf"),
            json!("inf"),
            json!("+inf"),
            json!("NaN"),
            json!("nan"),
            json!("infinity"),
            json!(""),
            json!("  "),
            json!("12abc"),
            json!("1e400"),
            json!("-1e400"),
            json!("x".repeat(10_000)),
            json!(format!("-{}", "9".repeat(31))),
            json!(null),
            json!(true),
            json!([1, 2]),
            json!({"a": 1}),
        ];
        for value in &hostile {
            let mut map = quiet(1.0).as_object().unwrap().clone();
            map.insert("lavfi.astats.1.RMS_level".into(), value.clone());
            map.insert("lavfi.astats.5.RMS_level".into(), value.clone());
            let sample = parse_metadata(&Value::Object(map)).expect("time code is fine");
            for level in sample
                .frame
                .levels
                .into_iter()
                .chain(sample.frame.bands.into_iter().flatten())
            {
                assert!((0.0..=1.0).contains(&level), "{value}: {level}");
            }
            // Only a finite number can make a level non-zero.
            assert_eq!(sample.frame.levels[0], 0.0, "{value}");
        }
        // Numbers are accepted as well as strings, and clamped.
        let mut map = quiet(1.0).as_object().unwrap().clone();
        map.insert("lavfi.astats.1.RMS_level".into(), json!(12.0));
        map.insert("lavfi.astats.2.RMS_level".into(), json!("-200"));
        let sample = parse_metadata(&Value::Object(map)).unwrap();
        assert_eq!(sample.frame.levels, [1.0, 0.0]);
    }

    #[test]
    fn missing_keys_are_silence_but_a_missing_time_code_drops_the_sample() {
        let only_time_code = json!({"lavfi.astats.10.Peak_level": time_code_db(4.0)});
        let sample = parse_metadata(&only_time_code).unwrap();
        assert_eq!(sample.frame.levels, [0.0, 0.0]);
        assert_eq!(sample.frame.bands, [Some(0.0); 7]);
        assert!((sample.time_code - 4.0).abs() < 1e-4);

        let mut map = quiet(1.0).as_object().unwrap().clone();
        map.remove("lavfi.astats.10.Peak_level");
        assert_eq!(parse_metadata(&Value::Object(map)), None);
        for bad in ["NaN", "inf", "3.0", "garbage"] {
            let value = json!({"lavfi.astats.10.Peak_level": bad});
            assert_eq!(parse_metadata(&value), None, "{bad}");
        }
        // Silence on the time-code channel is t = 0.
        let zero = json!({"lavfi.astats.10.Peak_level": "-inf"});
        assert_eq!(parse_metadata(&zero).unwrap().time_code, 0.0);
        for not_a_map in [json!(null), json!("x"), json!([1]), json!(3)] {
            assert_eq!(parse_metadata(&not_a_map), None);
        }
    }

    #[test]
    fn extra_channels_and_keys_are_ignored() {
        let mut map = quiet(2.0).as_object().unwrap().clone();
        for channel in 11..40 {
            map.insert(format!("lavfi.astats.{channel}.RMS_level"), json!("0.0"));
        }
        map.insert("lavfi.astats.Overall.RMS_level".into(), json!("0.0"));
        map.insert("something.else".into(), json!({"deep": [1, 2]}));
        let sample = parse_metadata(&Value::Object(map)).unwrap();
        assert_eq!(sample.frame.levels, [0.0, 0.0]);
        assert_eq!(sample.frame.bands, [Some(0.0); 7]);
    }

    #[test]
    fn time_codes_unwrap_against_the_position() {
        assert_eq!(unwrap_time_code(1.5, 1.2), 1.5);
        assert_eq!(unwrap_time_code(1.5, 33.0), 33.5);
        // Just past a wrap: the time code restarts near 0.
        assert_eq!(unwrap_time_code(0.1, 31.9), 32.1);
        // The position just past a wrap, the reading from before it.
        assert_eq!(unwrap_time_code(31.9, 32.05), 31.9);
        assert_eq!(
            unwrap_time_code(10.0, 100.0 * 32.0 + 10.2),
            100.0 * 32.0 + 10.0
        );
    }

    fn reading(value: f32) -> VisualizerFrame {
        VisualizerFrame {
            levels: [value; 2],
            bands: [Some(value); 7],
        }
    }

    fn sample(t: f64, value: f32) -> Sample {
        Sample {
            time_code: t % TIME_CODE_PERIOD,
            frame: reading(value),
        }
    }

    #[test]
    fn readings_are_shown_when_the_position_reaches_them() {
        let mut a = Aligner::default();
        // The analysis runs 0.26 s ahead of the position.
        assert!(a.offer(sample(1.26, 0.1), 1.0));
        assert!(a.offer(sample(1.31, 0.2), 1.05));
        assert_eq!(a.take_due(1.2), None);
        assert_eq!(a.take_due(1.26), Some(reading(0.1)));
        // Two readings due at once: only the newest is shown.
        assert!(a.offer(sample(1.36, 0.3), 1.1));
        assert_eq!(a.take_due(1.5), Some(reading(0.3)));
        assert_eq!(a.take_due(1.6), None);
        // The same time code again is a value left over, not a reading.
        assert!(!a.offer(sample(1.36, 0.9), 1.6));
        // A reading older than one already shown is never shown.
        assert!(a.offer(sample(1.6, 0.4), 1.5));
        assert!(!a.offer(sample(1.55, 0.5), 1.5));
    }

    #[test]
    fn implausible_readings_are_dropped() {
        let mut a = Aligner::default();
        assert!(!a.offer(sample(10.0, 0.1), 4.0), "too far ahead");
        assert!(!a.offer(sample(3.0, 0.1), 4.0), "too late");
        assert!(a.offer(sample(4.1, 0.1), 4.0));
        // The queue is bounded.
        for i in 1..=(MAX_PENDING + 10) {
            a.offer(sample(4.1 + i as f64 * 0.01, 0.2), 4.0);
        }
        assert_eq!(a.pending.len(), MAX_PENDING);
    }

    #[test]
    fn the_clock_extrapolates_and_holds_while_paused() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut clock = PlayClock::default();
        assert_eq!(clock.position(t0), None);
        clock.observe(10.0, at(0));
        assert_eq!(clock.position(at(100)), Some(10.1));
        // Never extrapolated beyond the cap.
        assert_eq!(clock.position(at(5000)), Some(11.0));
        clock.set_paused(true, at(200));
        assert_eq!(clock.position(at(3000)), Some(10.2));
        clock.set_paused(false, at(3000));
        assert_eq!(clock.position(at(3050)), Some(10.25));
        clock.clear();
        assert_eq!(clock.position(at(3100)), None);
    }

    /// A visualizer that is wanted, attached and playing load 1, with the
    /// position at `position` at `now`.
    fn playing(now: Instant, position: f64) -> Visualizer {
        let mut v = Visualizer::default();
        v.set_wanted(true);
        v.track_change(Some(LoadId(1)));
        v.started(LoadId(1));
        assert_eq!(v.take_attach().as_deref(), Some(GRAPH));
        assert!(!v.attach_answered(Ok(())));
        v.restarted();
        v.position(position, now);
        v
    }

    fn polled(v: &mut Visualizer, now: Instant) -> u64 {
        v.tick(now).poll.expect("a poll is due")
    }

    #[test]
    fn polls_one_at_a_time_at_most_every_interval() {
        let t0 = Instant::now();
        let mut v = playing(t0, 1.0);
        assert_eq!(v.deadline(t0), Some(t0));
        let generation = polled(&mut v, t0);
        assert_eq!(v.deadline(t0), Some(t0 + POLL_INTERVAL));
        assert_eq!(v.tick(t0 + POLL_INTERVAL / 2), Tick::default());
        // Not answered yet: the next tick polls nothing.
        assert_eq!(v.tick(t0 + POLL_INTERVAL).poll, None);
        v.poll_answered(generation, Ok(Some(quiet(1.2))), t0 + POLL_INTERVAL);
        assert!(v.tick(t0 + POLL_INTERVAL * 2).poll.is_some());
    }

    #[test]
    fn no_polls_while_hidden_paused_stopped_or_unattached() {
        let t0 = Instant::now();
        let mut v = Visualizer::default();
        // Wanted, nothing playing: nothing to attach and no polls.
        v.set_wanted(true);
        assert_eq!(v.take_attach(), None);
        assert_eq!(v.deadline(t0), None);
        assert_eq!(v.tick(t0), Tick::default());
        // Playing but paused: not attached yet.
        v.track_change(Some(LoadId(1)));
        v.started(LoadId(1));
        v.set_paused(true, t0);
        assert_eq!(v.take_attach(), None);
        v.set_paused(false, t0);
        assert!(v.take_attach().is_some());
        // Adding: still no polls.
        assert_eq!(v.deadline(t0), None);
        v.attach_answered(Ok(()));
        assert!(v.deadline(t0).is_some());
        v.set_wanted(false);
        assert_eq!(v.deadline(t0), None, "hidden");
        assert_eq!(v.tick(t0), Tick::default(), "hidden");
        v.set_wanted(true);
        assert!(v.deadline(t0).is_some());
        v.set_paused(true, t0);
        assert_eq!(v.deadline(t0), None, "paused");
        assert_eq!(v.tick(t0), Tick::default(), "paused");
        v.set_paused(false, t0);
        assert!(v.deadline(t0).is_some());
        // Stopped.
        v.track_change(None);
        assert_eq!(v.deadline(t0), None);
        // Ended.
        v.track_change(Some(LoadId(2)));
        v.started(LoadId(2));
        assert!(v.deadline(t0).is_some());
        assert!(!v.ended(LoadId(2), false));
        assert_eq!(v.deadline(t0), None);
    }

    #[test]
    fn readings_become_aligned_events_for_the_playing_load() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut v = playing(t0, 1.0);
        let generation = polled(&mut v, at(0));
        v.poll_answered(
            generation,
            Ok(Some(metadata(1.26, ["0", "-30"], ["-10"; 7]))),
            at(1),
        );
        // Due when the position reaches 1.26 s, i.e. 260 ms later.
        assert_eq!(v.tick(at(40)).event, None);
        let tick = v.tick(at(270));
        let Some(PlaybackEvent::Visualizer { load, frame }) = tick.event else {
            panic!("{tick:?}");
        };
        assert_eq!(load, LoadId(1));
        assert_eq!(frame.levels, [1.0, 0.5]);
        assert_eq!(frame.bands, [Some(1.0); 7]);
    }

    #[test]
    fn bands_at_or_above_nyquist_are_hidden() {
        let t0 = Instant::now();
        let mut v = playing(t0, 1.0);
        v.set_sample_rate(Some(8000.0));
        let generation = polled(&mut v, t0);
        v.poll_answered(
            generation,
            Ok(Some(metadata(1.0, ["-6"; 2], ["-20"; 7]))),
            t0,
        );
        let Some(PlaybackEvent::Visualizer { frame, .. }) = v.tick(t0 + POLL_INTERVAL).event else {
            panic!("no event");
        };
        // 4 kHz Nyquist: 63 Hz … 2.5 kHz stay, 6.3 and 16 kHz go.
        assert!(frame.bands[..5].iter().all(Option::is_some), "{frame:?}");
        assert_eq!(frame.bands[5..], [None, None]);
        // 32 kHz: exactly 16 kHz is hidden too.
        v.set_sample_rate(Some(32000.0));
        assert_eq!(v.hide_unreachable_bands(reading(0.5)).bands[6], None);
        assert!(v.hide_unreachable_bands(reading(0.5)).bands[5].is_some());
        v.set_sample_rate(Some(f64::NAN));
        assert!(
            v.hide_unreachable_bands(reading(0.5))
                .bands
                .iter()
                .all(Option::is_some)
        );
    }

    #[test]
    fn readings_from_before_a_seek_or_load_are_dropped() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut v = playing(t0, 1.0);
        let generation = polled(&mut v, at(0));
        v.poll_answered(generation, Ok(Some(quiet(1.2))), at(1));
        // An answer in flight across a seek is dropped, and so is what was
        // queued.
        let in_flight = polled(&mut v, at(40));
        v.seek_sent();
        v.seeking();
        v.poll_answered(in_flight, Ok(Some(quiet(61.25))), at(41));
        assert!(v.aligner.pending.is_empty());
        // Until playback restarts, neither positions nor readings count.
        v.position(60.0, at(50));
        assert_eq!(v.clock.position(at(50)), None);
        v.restarted();
        v.position(60.0, at(60));
        // mpv still shows the last reading from before the seek (here
        // one that would otherwise look plausible).
        let generation = polled(&mut v, at(80));
        v.poll_answered(generation, Ok(Some(quiet(61.25))), at(81));
        assert!(v.aligner.pending.is_empty());
        let generation = polled(&mut v, at(120));
        v.poll_answered(generation, Ok(Some(quiet(60.3))), at(121));
        assert_eq!(v.aligner.pending.len(), 1);
        // A new track: nothing is shown for load 1 any more.
        assert!(!v.track_change(Some(LoadId(2))));
        assert_eq!(v.tick(at(2000)), Tick::default());
        v.track_change(Some(LoadId(2)));
        v.started(LoadId(2));
        v.restarted();
        v.position(0.0, at(2000));
        let tick = v.tick(at(2500));
        assert_eq!(tick.event, None);
    }

    #[test]
    fn a_seek_mpv_does_not_perform_does_not_freeze_the_readings() {
        // mpv answers `success` but sends neither `seek` nor
        // `playback-restart` (a non-seekable stream past its cache).
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut v = playing(t0, 1.0);
        let before = polled(&mut v, at(0));
        v.seek_sent();
        // The poll already out is dropped…
        v.poll_answered(before, Ok(Some(quiet(1.2))), at(1));
        assert!(v.aligner.pending.is_empty());
        // …but later ones count: playback just carried on.
        v.position(1.05, at(50));
        let after = polled(&mut v, at(50));
        v.poll_answered(after, Ok(Some(quiet(1.3))), at(51));
        let tick = v.tick(at(310));
        assert!(
            matches!(tick.event, Some(PlaybackEvent::Visualizer { .. })),
            "{tick:?}"
        );
        v.poll_answered(tick.poll.expect("polled"), Ok(None), at(311));
        // A seek mpv performs: its `seek` event holds readings until
        // `playback-restart`.
        v.seeking();
        let during = polled(&mut v, at(400));
        v.poll_answered(during, Ok(Some(quiet(30.2))), at(401));
        assert!(v.aligner.pending.is_empty());
        v.restarted();
        v.position(30.0, at(420));
        let next = polled(&mut v, at(450));
        v.poll_answered(next, Ok(Some(quiet(30.3))), at(451));
        assert_eq!(v.aligner.pending.len(), 1);
    }

    #[test]
    fn readings_far_ahead_of_the_position_are_dropped_when_due_is_checked() {
        let mut a = Aligner::default();
        // Position jumped back 60 s without Coda seeing a seek.
        assert!(a.offer(sample(61.0, 0.1), 60.9));
        assert_eq!(a.take_due(61.0), Some(reading(0.1)));
        assert!(a.offer(sample(61.2, 0.2), 61.0));
        assert_eq!(a.take_due(1.0), None);
        assert!(a.pending.is_empty(), "61.2 is beyond the lead at 1.0");
        assert_eq!(a.last_shown, None, "61.0 no longer holds back readings");
        assert!(a.offer(sample(1.2, 0.3), 1.0));
        assert_eq!(a.take_due(1.25), Some(reading(0.3)));
    }

    #[test]
    fn the_start_of_a_superseded_load_is_ignored() {
        let mut v = Visualizer::default();
        v.set_wanted(true);
        v.track_change(Some(LoadId(1)));
        v.track_change(Some(LoadId(2)));
        v.started(LoadId(1));
        assert!(!v.is_playing());
        assert_eq!(v.take_attach(), None);
        v.started(LoadId(2));
        assert!(v.is_playing());
        // After a stop nothing is expected.
        v.track_change(None);
        v.started(LoadId(2));
        assert!(!v.is_playing());
    }

    #[test]
    fn any_reading_mpv_produced_proves_the_graph() {
        let t0 = Instant::now();
        let mut v = playing(t0, 1.0);
        let stale = polled(&mut v, t0);
        v.seek_sent();
        v.poll_answered(stale, Ok(Some(quiet(1.1))), t0);
        assert!(v.aligner.pending.is_empty(), "not shown");
        assert!(!v.ended(LoadId(1), true), "but the graph works");
    }

    #[test]
    fn a_hidden_graph_is_removed_only_at_a_track_change() {
        let t0 = Instant::now();
        let mut v = playing(t0, 1.0);
        v.set_wanted(false);
        // Kept while the track plays (removing it would skip audio).
        v.seek_sent();
        v.seeking();
        assert!(v.track_change(None));
        // Detached: shown again later, it is added again.
        v.set_wanted(true);
        v.track_change(Some(LoadId(2)));
        v.started(LoadId(2));
        assert!(v.take_attach().is_some());
        // Wanted at the change: kept.
        v.attach_answered(Ok(()));
        assert!(!v.track_change(Some(LoadId(2))));
        assert!(v.take_attach().is_none());
    }

    #[test]
    fn a_refused_graph_is_reported_once_and_never_retried() {
        let mut v = Visualizer::default();
        v.set_wanted(true);
        v.track_change(Some(LoadId(1)));
        v.started(LoadId(1));
        assert!(v.take_attach().is_some());
        assert!(v.attach_answered(Err("error running command".into())));
        assert!(!v.attach_answered(Err("again".into())));
        assert_eq!(v.take_attach(), None);
        v.set_wanted(false);
        v.set_wanted(true);
        v.track_change(None);
        v.track_change(Some(LoadId(2)));
        v.started(LoadId(2));
        assert_eq!(v.take_attach(), None);
        assert!(!v.active());
    }

    #[test]
    fn a_failure_before_the_first_reading_blames_the_graph() {
        let t0 = Instant::now();
        let mut v = playing(t0, 1.0);
        assert!(v.ended(LoadId(1), true));
        assert_eq!(v.attach, Attach::Unavailable);
        // Once it produced a reading, failures are the track's own.
        let mut v = playing(t0, 1.0);
        let generation = polled(&mut v, t0);
        v.poll_answered(generation, Ok(Some(quiet(1.1))), t0);
        assert!(!v.ended(LoadId(1), true));
        assert!(!v.ended(LoadId(1), false));
    }
}
