//! Visualizer display state: the bar heights shown, smoothed from the
//! backend's [`VisualizerFrame`]s. Pure: no terminal and no backend
//! details.
//!
//! Smoothing keeps the display calm: a bar rises most of the way to a new
//! louder reading at once ([`ATTACK`]) and falls at most [`DECAY_PER_SEC`]
//! of its height per second, so short dips do not flicker.

use std::time::{Duration, Instant};

use crate::playback::{LoadId, VISUALIZER_BANDS, VisualizerFrame};

/// Share of the way to a louder reading covered at once.
pub const ATTACK: f32 = 0.7;
/// Full height falls to nothing in about 0.6 s.
pub const DECAY_PER_SEC: f32 = 1.6;
/// Longest gap between readings that still counts for the decay; longer
/// gaps (a stall) do not drop bars all at once.
const MAX_STEP: Duration = Duration::from_millis(250);
/// Without a reading for this long, the bars start to fall on their own
/// ([`Meter::fade`]); readings normally come every 45 ms or so.
pub const STALE_AFTER: Duration = Duration::from_millis(300);

/// The smoothed bars of one load.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Meter {
    load: Option<LoadId>,
    levels: [f32; 2],
    bands: [Option<f32>; VISUALIZER_BANDS],
    /// The last smoothing step (a reading or a fade).
    updated: Option<Instant>,
    /// The last reading.
    read: Option<Instant>,
}

impl Meter {
    /// Left and right level, `0.0..=1.0`.
    pub fn levels(&self) -> [f32; 2] {
        self.levels
    }

    /// Band levels, `0.0..=1.0`; `None` for a band the track cannot
    /// contain.
    pub fn bands(&self) -> [Option<f32>; VISUALIZER_BANDS] {
        self.bands
    }

    /// The load the bars belong to; `None` after a [`Self::reset`].
    pub fn load(&self) -> Option<LoadId> {
        self.load
    }

    /// `true` until the first reading after a [`Self::reset`].
    pub fn is_empty(&self) -> bool {
        self.updated.is_none()
    }

    /// Moves the bars towards `frame`, a reading for `load` at `now`. A
    /// reading of another load starts from zero.
    pub fn update(&mut self, load: LoadId, frame: &VisualizerFrame, now: Instant) {
        if self.load != Some(load) {
            self.reset();
            self.load = Some(load);
        }
        let step = self
            .updated
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last))
            .min(MAX_STEP)
            .as_secs_f32();
        for (shown, &target) in self.levels.iter_mut().zip(&frame.levels) {
            *shown = smooth(*shown, target, step);
        }
        for (shown, &target) in self.bands.iter_mut().zip(&frame.bands) {
            *shown = target.map(|target| smooth(shown.unwrap_or(0.0), target, step));
        }
        self.updated = Some(now);
        self.read = Some(now);
    }

    /// Lets the bars fall towards silence if no reading came for
    /// [`STALE_AFTER`]. Returns `true` if they moved.
    pub fn fade(&mut self, now: Instant) -> bool {
        let (Some(read), Some(updated)) = (self.read, self.updated) else {
            return false;
        };
        if now.saturating_duration_since(read) < STALE_AFTER {
            return false;
        }
        let step = now
            .saturating_duration_since(updated)
            .min(MAX_STEP)
            .as_secs_f32();
        let before = (self.levels, self.bands);
        for shown in &mut self.levels {
            *shown = smooth(*shown, 0.0, step);
        }
        for shown in self.bands.iter_mut().flatten() {
            *shown = smooth(*shown, 0.0, step);
        }
        self.updated = Some(now);
        (self.levels, self.bands) != before
    }

    /// Drops to silence: paused, stopped, hidden or another track.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// One smoothing step of `step` seconds from `shown` towards `target`.
/// Non-finite or out-of-range targets are clamped (a finite value is
/// guaranteed by the backend, but this is cheap).
fn smooth(shown: f32, target: f32, step: f32) -> f32 {
    let target = if target.is_finite() {
        target.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if target >= shown {
        shown + (target - shown) * ATTACK
    } else {
        (shown - DECAY_PER_SEC * step).max(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(value: f32) -> VisualizerFrame {
        VisualizerFrame {
            levels: [value, value / 2.0],
            bands: [Some(value); VISUALIZER_BANDS],
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn attack_is_fast_and_decay_is_slow() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut meter = Meter::default();
        assert!(meter.is_empty());
        meter.update(LoadId(1), &frame(1.0), at(0));
        assert!(!meter.is_empty());
        assert!(close(meter.levels()[0], 0.7), "{:?}", meter.levels());
        assert!(close(meter.levels()[1], 0.35));
        meter.update(LoadId(1), &frame(1.0), at(40));
        assert!(close(meter.levels()[0], 0.91));
        // Silence: falls by DECAY_PER_SEC per second, not at once.
        meter.update(LoadId(1), &frame(0.0), at(140));
        assert!(
            close(meter.levels()[0], 0.91 - 0.16),
            "{:?}",
            meter.levels()
        );
        meter.update(LoadId(1), &frame(0.0), at(240));
        assert!(close(meter.levels()[0], 0.91 - 0.32));
        // Never below the reading.
        meter.update(LoadId(1), &frame(0.5), at(1240));
        assert!(close(meter.levels()[0], 0.5), "{:?}", meter.levels());
        meter.update(LoadId(1), &frame(0.0), at(3000));
        meter.update(LoadId(1), &frame(0.0), at(3250));
        meter.update(LoadId(1), &frame(0.0), at(3500));
        assert_eq!(meter.levels(), [0.0, 0.0]);
    }

    #[test]
    fn a_stall_does_not_drop_the_bars_at_once() {
        let t0 = Instant::now();
        let mut meter = Meter::default();
        meter.update(LoadId(1), &frame(1.0), t0);
        meter.update(LoadId(1), &frame(1.0), t0);
        meter.update(LoadId(1), &frame(1.0), t0);
        let before = meter.levels()[0];
        meter.update(LoadId(1), &frame(0.0), t0 + Duration::from_secs(10));
        assert!(
            close(meter.levels()[0], before - 0.4),
            "{:?}",
            meter.levels()
        );
    }

    #[test]
    fn bars_fall_when_readings_stop() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut meter = Meter::default();
        assert!(!meter.fade(at(0)), "nothing to fade");
        meter.update(LoadId(1), &frame(1.0), at(0));
        meter.update(LoadId(1), &frame(1.0), at(40));
        // Recent reading: kept.
        assert!(!meter.fade(at(250)));
        assert!(close(meter.levels()[0], 0.91));
        // Stale: falls a capped step per call, down to silence.
        assert!(meter.fade(at(400)));
        assert!(close(meter.levels()[0], 0.91 - 0.4), "{:?}", meter.levels());
        assert!(meter.fade(at(650)));
        assert!(close(meter.levels()[0], 0.91 - 0.8), "{:?}", meter.levels());
        assert!(meter.fade(at(900)));
        assert_eq!(meter.levels(), [0.0, 0.0]);
        assert_eq!(meter.bands(), [Some(0.0); VISUALIZER_BANDS]);
        assert!(!meter.fade(at(1150)), "already silent");
        // A new reading rises again as usual.
        meter.update(LoadId(1), &frame(1.0), at(1200));
        assert!(close(meter.levels()[0], 0.7));
    }

    #[test]
    fn hidden_bands_stay_hidden_and_return_from_zero() {
        let t0 = Instant::now();
        let mut meter = Meter::default();
        let mut reading = frame(1.0);
        reading.bands[6] = None;
        meter.update(LoadId(1), &reading, t0);
        assert_eq!(meter.bands()[6], None);
        assert!(close(meter.bands()[0].unwrap(), 0.7));
        meter.update(LoadId(1), &frame(1.0), t0);
        assert!(close(meter.bands()[6].unwrap(), 0.7));
    }

    #[test]
    fn another_load_or_a_reset_starts_from_zero() {
        let t0 = Instant::now();
        let mut meter = Meter::default();
        meter.update(LoadId(1), &frame(1.0), t0);
        meter.update(LoadId(2), &frame(0.0), t0 + Duration::from_millis(10));
        assert_eq!(meter.levels(), [0.0, 0.0]);
        meter.reset();
        assert!(meter.is_empty());
        assert_eq!(meter, Meter::default());
    }

    #[test]
    fn hostile_targets_are_clamped() {
        for target in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -3.0, 7.0] {
            let value = smooth(0.5, target, 0.1);
            assert!((0.0..=1.0).contains(&value), "{target}: {value}");
        }
    }
}
