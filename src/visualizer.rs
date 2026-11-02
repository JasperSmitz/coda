//! Visualizer display state: the bar heights shown, smoothed from the
//! backend's [`VisualizerFrame`]s. Pure: no terminal and no backend
//! details.
//!
//! Smoothing keeps the display calm: a bar rises most of the way to a new
//! louder reading at once ([`ATTACK`]) and falls at most [`DECAY_PER_SEC`]
//! of its height per second, so short dips do not flicker.
//!
//! A wide visualizer shows more bars than there are bands: [`spread_bands`]
//! interpolates between the measured bands on a log-frequency axis.

use std::time::{Duration, Instant};

use crate::playback::{LoadId, VISUALIZER_BAND_HZ, VISUALIZER_BANDS, VisualizerFrame};

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

    /// A gapless change ended: the bars of `from` carry on as `to`'s
    /// instead of starting from silence.
    pub fn hand_over(&mut self, from: LoadId, to: LoadId) {
        if self.load == Some(from) {
            self.load = Some(to);
        }
    }

    /// Drops to silence: paused, stopped, hidden or another track.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Most bars [`spread_bands`] is asked for by the renderer.
pub const MAX_BARS: usize = 48;

/// Band centre `i` on the log-frequency axis.
fn band_position(i: usize) -> f32 {
    (VISUALIZER_BAND_HZ[i] as f32).ln()
}

/// Spreads the band readings over `bars` bars, evenly spaced on a
/// log-frequency axis from the lowest band centre to the highest, so the
/// first and last bars sit on those bands. Values between bands follow
/// [`band_curve`]. A bar beyond the last available band (or next to any
/// `None` band) is `None`: nothing is invented where nothing was measured.
pub fn spread_bands(bands: &[Option<f32>; VISUALIZER_BANDS], bars: usize) -> Vec<Option<f32>> {
    let (low, high) = (band_position(0), band_position(VISUALIZER_BANDS - 1));
    (0..bars)
        .map(|bar| {
            let position = match bar {
                0 => low,
                _ if bar + 1 == bars => high,
                _ => low + (high - low) * bar as f32 / (bars - 1) as f32,
            };
            band_curve(bands, position)
        })
        .collect()
}

/// The band readings as a smooth curve over `position` (the natural log
/// of a frequency in Hz): a monotone cubic (Fritsch–Butland slopes) that
/// passes through every available band and never leaves the range of the
/// two bands around `position`, so no bar rises above both of its
/// neighbouring bands. `None` outside the bands, and between two bands
/// unless both are available. Non-finite readings count as silence.
pub fn band_curve(bands: &[Option<f32>; VISUALIZER_BANDS], position: f32) -> Option<f32> {
    let value = |i: usize| -> Option<f32> {
        bands[i].map(|v| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        })
    };
    if !position.is_finite() {
        return None;
    }
    // On a band centre: that band, whatever its neighbours are.
    if let Some(i) = (0..VISUALIZER_BANDS).find(|&i| band_position(i) == position) {
        return value(i);
    }
    // The segment [i, i + 1] that holds `position`.
    let i = (0..VISUALIZER_BANDS - 1)
        .find(|&i| band_position(i) < position && position < band_position(i + 1))?;
    let (y0, y1) = (value(i)?, value(i + 1)?);
    let width = |k: usize| band_position(k + 1) - band_position(k);
    // Slope of the segment [k, k + 1], if both ends are available.
    let secant = |k: usize| -> Option<f32> {
        let (a, b) = (value(k)?, value(k + 1)?);
        Some((b - a) / width(k))
    };
    // Slope at band k: a weighted harmonic mean of the slopes on either
    // side, zero at a peak, a dip or a flat side; one-sided at an end.
    let slope = |k: usize| -> f32 {
        let left = k
            .checked_sub(1)
            .and_then(|j| secant(j).map(|s| (s, width(j))));
        let right = (k + 1 < VISUALIZER_BANDS)
            .then(|| secant(k).map(|s| (s, width(k))))
            .flatten();
        match (left, right) {
            (Some((s0, h0)), Some((s1, h1))) => {
                if s0 * s1 <= 0.0 {
                    0.0
                } else {
                    3.0 * (h0 + h1) / ((2.0 * h1 + h0) / s0 + (h1 + 2.0 * h0) / s1)
                }
            }
            (Some((s, _)), None) | (None, Some((s, _))) => s,
            (None, None) => 0.0,
        }
    };
    let h = width(i);
    let t = (position - band_position(i)) / h;
    let (t2, t3) = (t * t, t * t * t);
    let y = (2.0 * t3 - 3.0 * t2 + 1.0) * y0
        + (t3 - 2.0 * t2 + t) * h * slope(i)
        + (-2.0 * t3 + 3.0 * t2) * y1
        + (t3 - t2) * h * slope(i + 1);
    // Monotone by construction; the clamp only absorbs rounding.
    Some(y.clamp(y0.min(y1), y0.max(y1)))
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

    fn positions() -> [f32; VISUALIZER_BANDS] {
        std::array::from_fn(band_position)
    }

    #[test]
    fn the_curve_hits_every_band_at_its_centre() {
        let bands = [
            Some(0.2),
            Some(0.9),
            Some(0.4),
            Some(0.4),
            Some(1.0),
            Some(0.0),
            Some(0.6),
        ];
        for (i, x) in positions().into_iter().enumerate() {
            assert_eq!(band_curve(&bands, x), bands[i], "band {i}");
        }
        // The first and last bars sit on the outer bands.
        for bars in [2, 7, 8, 13, 48] {
            let spread = spread_bands(&bands, bars);
            assert_eq!(spread.len(), bars);
            assert_eq!(spread[0], bands[0], "{bars}");
            assert_eq!(spread[bars - 1], bands[6], "{bars}");
        }
        assert_eq!(spread_bands(&bands, 1), [bands[0]]);
        assert!(spread_bands(&bands, 0).is_empty());
    }

    #[test]
    fn the_curve_never_overshoots_its_neighbouring_bands() {
        // Peaks, dips, plateaus and steps.
        let cases: [[f32; VISUALIZER_BANDS]; 6] = [
            [0.2, 0.9, 0.4, 0.4, 1.0, 0.0, 0.6],
            [0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0],
            [0.5; 7],
            [0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
            [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7],
            [1.0, 0.99, 0.0, 0.01, 0.5, 0.49, 0.48],
        ];
        let xs = positions();
        for case in cases {
            let bands = case.map(Some);
            for i in 0..VISUALIZER_BANDS - 1 {
                let (lo, hi) = (case[i].min(case[i + 1]), case[i].max(case[i + 1]));
                let mut last = case[i];
                for step in 1..100 {
                    let x = xs[i] + (xs[i + 1] - xs[i]) * step as f32 / 100.0;
                    let y = band_curve(&bands, x).unwrap();
                    assert!(
                        (lo..=hi).contains(&y),
                        "{case:?} segment {i} at {step}: {y}"
                    );
                    // Monotone within the segment.
                    if case[i] <= case[i + 1] {
                        assert!(y >= last - 1e-6, "{case:?} segment {i}: {y} < {last}");
                    } else {
                        assert!(y <= last + 1e-6, "{case:?} segment {i}: {y} > {last}");
                    }
                    last = y;
                }
            }
            // So no spread bar is above both of its neighbouring bands.
            for bars in [8, 13, 27, 48] {
                for (bar, value) in spread_bands(&bands, bars).into_iter().enumerate() {
                    let value = value.unwrap();
                    let max = case.iter().copied().fold(0.0, f32::max);
                    let min = case.iter().copied().fold(1.0, f32::min);
                    assert!((min..=max).contains(&value), "{case:?} {bars}/{bar}");
                }
            }
        }
    }

    #[test]
    fn the_curve_is_smooth_between_bands() {
        // Rising bands: the curve rises between them, not in steps.
        let bands = [0.0, 0.1, 0.3, 0.5, 0.7, 0.9, 1.0].map(Some);
        let spread: Vec<f32> = spread_bands(&bands, 48).into_iter().flatten().collect();
        assert_eq!(spread.len(), 48);
        assert!(spread.windows(2).all(|w| w[1] > w[0]), "{spread:?}");
    }

    #[test]
    fn bars_beyond_the_last_available_band_stay_empty() {
        let bands = [
            Some(0.5),
            Some(0.6),
            Some(0.7),
            Some(0.8),
            Some(0.9),
            None,
            None,
        ];
        let xs = positions();
        // The last available band itself is shown.
        assert_eq!(band_curve(&bands, xs[4]), Some(0.9));
        // Nothing past it, not even just past it.
        assert_eq!(band_curve(&bands, xs[4] + 1e-3), None);
        assert_eq!(band_curve(&bands, xs[6]), None);
        let spread = spread_bands(&bands, 13);
        let shown = spread.iter().take_while(|v| v.is_some()).count();
        assert!(spread[shown..].iter().all(Option::is_none), "{spread:?}");
        // Bars up to the 2500 Hz band, none after it.
        let last_shown = xs[0] + (xs[6] - xs[0]) * (shown - 1) as f32 / 12.0;
        let next = xs[0] + (xs[6] - xs[0]) * shown as f32 / 12.0;
        assert!(last_shown <= xs[4] && next > xs[4], "{spread:?}");
        // No bands at all: no bars.
        assert!(
            spread_bands(&[None; VISUALIZER_BANDS], 20)
                .iter()
                .all(Option::is_none)
        );
        // A gap in the middle (not produced by the backend) leaves a gap.
        let mut gapped = [Some(0.5); VISUALIZER_BANDS];
        gapped[3] = None;
        assert_eq!(band_curve(&gapped, (xs[2] + xs[3]) / 2.0), None);
        assert_eq!(band_curve(&gapped, (xs[3] + xs[4]) / 2.0), None);
        assert_eq!(band_curve(&gapped, (xs[1] + xs[2]) / 2.0), Some(0.5));
    }

    #[test]
    fn the_curve_is_defined_only_within_the_bands() {
        let bands = [Some(0.5); VISUALIZER_BANDS];
        let xs = positions();
        assert_eq!(band_curve(&bands, xs[0] - 0.1), None);
        assert_eq!(band_curve(&bands, xs[6] + 0.1), None);
        assert_eq!(band_curve(&bands, f32::NAN), None);
        let mut hostile = [Some(f32::NAN); VISUALIZER_BANDS];
        hostile[1] = Some(f32::INFINITY);
        hostile[2] = Some(-4.0);
        for value in spread_bands(&hostile, 48) {
            let value = value.unwrap();
            assert!((0.0..=1.0).contains(&value), "{value}");
        }
    }
}
