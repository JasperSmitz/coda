//! A cover's colours (the desktop app's "colours from the music",
//! `apps/coda-gui/docs/DESIGN.md` "Colour"): a deep tint of its dominant
//! hue for backgrounds, its most vivid colour as an accent, and black or
//! white text for the accent. Pure and deterministic: no I/O, nothing
//! frontend-specific; callers load the cover and run this on a blocking
//! thread (cheap: it works on a sample of at most 32 × 32 pixels).
//!
//! How:
//! - The cover is averaged down to at most `SAMPLE` × `SAMPLE` pixels
//!   and each pixel taken to OKLCH.
//! - **Dominant hue**: a histogram of `HUE_BINS` hue bins, each pixel
//!   weighted by its chroma (capped, so a small loud area does not outweigh
//!   a large quiet one) and lightly by mid lightness, smoothed over
//!   neighbouring bins. The tint is that hue at OKLCH L `TINT_L` and at
//!   most `TINT_MAX_C` chroma.
//! - **Accent**: among the hue bins with enough colourful pixels to count
//!   (`populated`: with half of each neighbour's at least `MIN_SHARE` of
//!   the cover, and on its own at least a quarter of that and three
//!   pixels, so a stray pixel never counts, not even next to a populated
//!   hue), each bin's representative is the mean of its most vivid
//!   quarter of pixels; the bin with the most vivid representative, mildly
//!   favouring larger ones, gives the accent. Its lightness is clamped to
//!   `ACCENT_L` and chroma to `ACCENT_C` (then into the sRGB gamut),
//!   and lightness is raised until it has at least `MIN_ACCENT_CONTRAST`
//!   on [`BASE`] and on the tint.
//! - **Greyscale** covers (no populated hue): `vivid` is false, the tint
//!   is nearly neutral with the cover's slight cast (or the base's violet)
//!   and the accent a soft neutral; the frontend shows its house accent.

pub mod color;

use std::f64::consts::TAU;

use self::color::{Rgba, contrast, text_on};
use super::CoverImage;

/// The desktop app's base background, `#110f14` (`--bg` in its
/// `src/styles/tokens.css`). The app's `palette::BASE` is this constant,
/// and a test there checks `tokens.css` against it, so the accents'
/// contrast is computed on the background they are drawn on.
pub const BASE: Rgba = Rgba::rgb(0x11, 0x0f, 0x14);

/// The largest side of the sample the palette is computed from.
const SAMPLE: u32 = 32;
/// Hue histogram bins (15° each).
const HUE_BINS: usize = 24;
/// Pixels below this OKLCH chroma count as grey.
const GREY_CHROMA: f64 = 0.04;
/// The share of the sample a hue needs (with half its neighbours) to be
/// a candidate for the accent; below it on every hue, a cover is greyscale.
const MIN_SHARE: f64 = 0.02;
/// The tint's OKLCH lightness.
const TINT_L: f64 = 0.20;
/// The tint's largest OKLCH chroma.
const TINT_MAX_C: f64 = 0.05;
/// A greyscale cover's tint chroma.
const GREY_TINT_C: f64 = 0.01;
/// The accent's OKLCH lightness range (before the contrast nudge).
const ACCENT_L: (f64, f64) = (0.72, 0.84);
/// The accent's OKLCH chroma range.
const ACCENT_C: (f64, f64) = (0.08, 0.17);
/// The accent's minimum WCAG contrast on [`BASE`] and on its tint.
const MIN_ACCENT_CONTRAST: f64 = 3.0;

/// A greyscale cover's (soft, neutral) accent lightness and chroma.
const GREY_ACCENT_L: f64 = 0.80;
const GREY_ACCENT_C: f64 = 0.02;
/// Chroma above this adds no more weight to the dominant hue.
const WEIGHT_C_CAP: f64 = 0.12;

/// A cover's colours, all opaque.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// A deep dark of the dominant hue, for backgrounds.
    pub tint: Rgba,
    /// The most vivid colour, readable on [`BASE`] and on `tint` (≥ 3:1).
    /// A soft neutral when `vivid` is false.
    pub accent: Rgba,
    /// Black or white, whichever contrasts more with `accent`.
    pub accent_text: Rgba,
    /// `false` for a near-greyscale cover: show the house accent instead.
    pub vivid: bool,
}

/// One sampled pixel in OKLCH (and its OKLab a, b).
#[derive(Debug, Clone, Copy)]
struct Px {
    l: f64,
    c: f64,
    a: f64,
    b: f64,
    bin: usize,
}

/// The palette of `image`.
pub fn palette(image: &CoverImage) -> Palette {
    let sample = if image.width() > SAMPLE || image.height() > SAMPLE {
        image.resized(image.width().min(SAMPLE), image.height().min(SAMPLE))
    } else {
        image.clone()
    };
    let pixels: Vec<Px> = sample
        .rgb()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|&[r, g, b]| {
            let [l, a, b] = Rgba::rgb(r, g, b).oklab();
            let c = a.hypot(b);
            Px {
                l,
                c,
                a,
                b,
                bin: hue_bin(b.atan2(a)),
            }
        })
        .collect();
    let total = pixels.len() as f64;

    // Colourful pixels per hue (with half of each neighbour's).
    let mut counts = [0.0f64; HUE_BINS];
    for p in pixels.iter().filter(|p| p.c >= GREY_CHROMA) {
        counts[p.bin] += 1.0;
    }
    let populated: Vec<usize> = (0..HUE_BINS)
        .filter(|&bin| populated(&counts, bin, total))
        .collect();

    if populated.is_empty() {
        return greyscale(&pixels);
    }

    // The dominant hue: chroma- and lightness-weighted, smoothed.
    let mut weights = [0.0f64; HUE_BINS];
    for p in pixels.iter().filter(|p| p.c >= GREY_CHROMA) {
        weights[p.bin] += p.c.min(WEIGHT_C_CAP) * mid_weight(p.l);
    }
    let smoothed = smooth(&weights);
    let dominant = argmax(&smoothed);
    let tint = tint_of(&pixels, dominant);

    let accent = vivid_accent(&pixels, &counts, &populated, total);
    let accent = lift_until_readable(accent, &[BASE, tint]);
    Palette {
        tint,
        accent,
        accent_text: text_on(accent),
        vivid: true,
    }
}

/// The hue bin of a hue in radians.
fn hue_bin(hue: f64) -> usize {
    let turn = hue.rem_euclid(TAU) / TAU;
    ((turn * HUE_BINS as f64) as usize).min(HUE_BINS - 1)
}

/// Neighbouring bins, wrapping.
fn neighbours(bin: usize) -> [usize; 2] {
    [(bin + HUE_BINS - 1) % HUE_BINS, (bin + 1) % HUE_BINS]
}

/// Whether a hue bin has enough colourful pixels to give the accent:
/// with half of each neighbour's, at least [`MIN_SHARE`] of the sample and
/// three pixels; and on its own at least a quarter of that share and three
/// pixels, so a stray pixel next to a populated hue never counts (the
/// representative is built from the bin's own pixels). A cover smaller
/// than three pixels needs only what it has.
fn populated(counts: &[f64; HUE_BINS], bin: usize, total: f64) -> bool {
    let [left, right] = neighbours(bin);
    let with_neighbours = counts[bin] + 0.5 * (counts[left] + counts[right]);
    let own = counts[bin];
    with_neighbours >= (MIN_SHARE * total).max(3.0).min(total)
        && own >= (MIN_SHARE * total / 4.0).max(3.0).min(total)
}

/// A light weight for mid lightness: 1 at L 0.6, down to 0.5 at the ends.
fn mid_weight(l: f64) -> f64 {
    (1.0 - 0.5 * ((l - 0.6) / 0.6).powi(2)).clamp(0.5, 1.0)
}

/// A circular [1 2 1] smoothing of the histogram.
fn smooth(values: &[f64; HUE_BINS]) -> [f64; HUE_BINS] {
    let mut out = [0.0; HUE_BINS];
    for (bin, slot) in out.iter_mut().enumerate() {
        let [left, right] = neighbours(bin);
        *slot = (values[left] + 2.0 * values[bin] + values[right]) / 4.0;
    }
    out
}

/// The first index of the largest value.
fn argmax(values: &[f64]) -> usize {
    let mut best = 0;
    for (i, v) in values.iter().enumerate() {
        if *v > values[best] {
            best = i;
        }
    }
    best
}

/// The tint for the dominant hue bin: the colourful pixels' mean hue there
/// (and in its neighbours) at [`TINT_L`], with half their chroma, at most
/// [`TINT_MAX_C`].
fn tint_of(pixels: &[Px], dominant: usize) -> Rgba {
    let [left, right] = neighbours(dominant);
    let (mut a, mut b, mut c, mut n) = (0.0, 0.0, 0.0, 0.0);
    for p in pixels
        .iter()
        .filter(|p| p.c >= GREY_CHROMA && (p.bin == dominant || p.bin == left || p.bin == right))
    {
        let w = p.c.min(WEIGHT_C_CAP) * mid_weight(p.l);
        a += p.a * w;
        b += p.b * w;
        c += p.c * w;
        n += w;
    }
    let mean_c = if n > 0.0 { c / n } else { TINT_MAX_C };
    let chroma = (mean_c * 0.5).clamp(0.02, TINT_MAX_C);
    Rgba::from_oklch_in_gamut(TINT_L, chroma, b.atan2(a))
}

/// The accent before the contrast nudge: the most vivid populated hue's
/// representative, clamped into the accent band.
fn vivid_accent(pixels: &[Px], counts: &[f64; HUE_BINS], populated: &[usize], total: f64) -> Rgba {
    let mut best: Option<(f64, [f64; 3])> = None;
    for &bin in populated {
        let mut members: Vec<(f64, &Px)> = pixels
            .iter()
            .filter(|p| p.bin == bin && p.c >= GREY_CHROMA)
            .map(|p| (p.c * mid_weight(p.l), p))
            .collect();
        // Most vivid first; equal scores keep the pixel order (stable sort).
        members.sort_by(|x, y| y.0.total_cmp(&x.0));
        let take = members.len().div_ceil(4).max(1);
        let top = &members[..take];
        let n = top.len() as f64;
        let l = top.iter().map(|(_, p)| p.l).sum::<f64>() / n;
        let a = top.iter().map(|(_, p)| p.a).sum::<f64>() / n;
        let b = top.iter().map(|(_, p)| p.b).sum::<f64>() / n;
        let c = a.hypot(b);
        let share = counts[bin] / total;
        let score = c * mid_weight(l) * share.powf(0.25);
        if best.is_none_or(|(s, _)| score > s) {
            best = Some((score, [l, c, b.atan2(a)]));
        }
    }
    let [l, c, h] = best.map(|(_, lch)| lch).unwrap_or([0.78, 0.0, 0.0]);
    Rgba::from_oklch_in_gamut(
        l.clamp(ACCENT_L.0, ACCENT_L.1),
        c.clamp(ACCENT_C.0, ACCENT_C.1),
        h,
    )
}

/// `color` raised in OKLCH lightness (hue and, within gamut, chroma kept)
/// until it has [`MIN_ACCENT_CONTRAST`] on every colour in `against` (all
/// dark); white if nothing below it does.
fn lift_until_readable(color: Rgba, against: &[Rgba]) -> Rgba {
    let reads = |c: Rgba| {
        against
            .iter()
            .all(|&bg| contrast(c, bg) >= MIN_ACCENT_CONTRAST)
    };
    if reads(color) {
        return color;
    }
    let [l, c, h] = color.oklch();
    let mut step = 1;
    while l + f64::from(step) * 0.01 <= 1.0 {
        let lifted = Rgba::from_oklch_in_gamut(l + f64::from(step) * 0.01, c, h);
        if reads(lifted) {
            return lifted;
        }
        step += 1;
    }
    color::WHITE
}

/// A greyscale cover: a nearly neutral tint with its slight cast (the
/// base's violet when it has none) and a soft neutral accent.
fn greyscale(pixels: &[Px]) -> Palette {
    let n = pixels.len() as f64;
    let a = pixels.iter().map(|p| p.a).sum::<f64>() / n;
    let b = pixels.iter().map(|p| p.b).sum::<f64>() / n;
    let hue = if a.hypot(b) > 0.002 {
        b.atan2(a)
    } else {
        BASE.oklch()[2]
    };
    let tint = Rgba::from_oklch_in_gamut(TINT_L, GREY_TINT_C, hue);
    let accent = lift_until_readable(
        Rgba::from_oklch_in_gamut(GREY_ACCENT_L, GREY_ACCENT_C, hue),
        &[BASE, tint],
    );
    Palette {
        tint,
        accent,
        accent_text: text_on(accent),
        vivid: false,
    }
}

#[cfg(test)]
mod tests;
