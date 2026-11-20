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
//! - **Swatches** (for gradients: Home's mood cards, genre tiles): up to
//!   [`MAX_SWATCHES`] colours of the populated hues, the largest first,
//!   at least [`SWATCH_MIN_GAP`] degrees of hue apart, each at its
//!   pixels' mean hue and lightness (clamped to [`SWATCH_L`]) with as much
//!   chroma as sRGB allows (up to [`SWATCH_MAX_C`]). None for a greyscale
//!   cover.
//! - **Washes** ([`Surface`], [`wash_alpha`]): where the app draws the
//!   cover blurred with text over it (the page's glow, the player), how
//!   much of the tint it lays over the cover so that the text levels keep
//!   their contrast over **this** cover's brightest blurred area, not over
//!   the worst cover there is: a dark cover needs little wash and stays
//!   colourful, a white one gets a heavy one.

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

/// Swatches at most.
pub const MAX_SWATCHES: usize = 4;
/// The swatches' OKLCH lightness range.
const SWATCH_L: (f64, f64) = (0.55, 0.70);
/// The swatches' largest OKLCH chroma (sRGB's gamut usually stops first).
const SWATCH_MAX_C: f64 = 0.25;
/// The least hue difference between two swatches, in degrees.
const SWATCH_MIN_GAP: f64 = 40.0;

/// The most of a swatch the desktop app lays over a card
/// ([`CARD_SURFACE`] over the base) where text is drawn on it (`--swatch-
/// under-text` in its `tokens.css`): every text level keeps its contrast
/// over any swatch at this alpha (tested). Where no text is (above a mood
/// card's text, a genre tile's), the swatches may be stronger.
pub const SWATCH_UNDER_TEXT: f64 = 0.24;
/// The desktop app's card surface (`--surface-1`), white at 0x0e.
pub const CARD_SURFACE: Rgba = Rgba {
    r: 0xff,
    g: 0xff,
    b: 0xff,
    a: 0x0e,
};

/// The desktop app's text levels (`--text`, `--text-2`, `--text-3` in its
/// `tokens.css`; a test there keeps them in step) and the contrast each
/// keeps over a washed cover: text 7:1 over these busy backgrounds (above
/// its DESIGN.md "Contrast" 4.5:1), secondary 4.5:1, faint 3:1. The accents are
/// lighter than the faint level (the house accent, and computed accents
/// at OKLCH L ≥ 0.72), so they keep their 3:1 too.
pub const TEXT: Rgba = Rgba::rgb(0xf6, 0xf3, 0xfb);
pub const TEXT_2: Rgba = Rgba::rgb(0xad, 0xaa, 0xb1);
pub const TEXT_3: Rgba = Rgba::rgb(0x88, 0x86, 0x8c);
const TEXT_LEVELS: [(Rgba, f64); 3] = [(TEXT, 7.0), (TEXT_2, 4.5), (TEXT_3, 3.0)];

/// How a surface of the desktop app draws a cover with text over it:
/// the cover blurred (and saturated) at some opacity over `under`, then a
/// wash of the cover's tint at [`wash_alpha`]. The CSS draws it with the
/// same constants (the app's `tokens.css`, `--*-glow-opacity` and
/// `--*-glow-filter`; a test there keeps them in step).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    /// The cover's opacity over `under`.
    pub opacity: f64,
    /// The CSS `saturate()` after the blur.
    pub saturate: f64,
    /// What shows where the cover is translucent.
    pub under: Rgba,
    /// The least wash, so the cover never shows bare.
    pub floor: f64,
    /// The blur's standard deviation (CSS `blur(r)` is a Gaussian with
    /// σ = r) as shares of the cover's width and height at the largest
    /// size the surface draws it (smaller is safer: a sharper blur keeps
    /// bright areas brighter).
    pub sigma: (f64, f64),
}

/// The page's glow (app.css `.page-glow`): the cover at 0.62 under
/// `blur(110px) saturate(1.5)`, over the base. It is drawn 120% of the
/// page's width wide (and as tall: `object-fit: cover` of a square), so
/// at most ~1.2 × 2560px on a large screen: σ = 110 / 3072 of the cover.
pub const PAGE_GLOW: Surface = Surface {
    opacity: 0.62,
    saturate: 1.5,
    under: BASE,
    floor: 0.25,
    sigma: (110.0 / 3072.0, 110.0 / 3072.0),
};

/// The player (app.css `.player-backdrop`): the whole cover stretched
/// across the strip, opaque, under `blur(30px) saturate(1.7)`, drawn
/// with a 30px bleed on every side: at most 1060px wide (the 1000px
/// strip) and 136px tall (the 76px strip): σ = 30 / 1060 across, 30 / 136
/// down.
pub const PLAYER: Surface = Surface {
    opacity: 1.0,
    saturate: 1.7,
    under: BASE,
    floor: 0.35,
    sigma: (30.0 / 1060.0, 30.0 / 136.0),
};

/// A cover's colours (all opaque) and its washes.
#[derive(Debug, Clone, PartialEq)]
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
    /// Up to [`MAX_SWATCHES`] vivid colours of the cover, the largest
    /// area first, for gradients over the base. Empty when greyscale.
    pub swatches: Vec<Rgba>,
    /// The tint's alpha over the cover in the page's glow ([`PAGE_GLOW`]).
    pub page_wash: f64,
    /// The tint's alpha over the cover in the player ([`PLAYER`]).
    pub player_wash: f64,
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
        let (tint, accent) = greyscale(&pixels);
        return finish(&sample, tint, accent, false, Vec::new());
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
    let swatches = swatches(&pixels, &counts, &populated);
    finish(&sample, tint, accent, true, swatches)
}

fn finish(
    sample: &CoverImage,
    tint: Rgba,
    accent: Rgba,
    vivid: bool,
    swatches: Vec<Rgba>,
) -> Palette {
    Palette {
        tint,
        accent,
        accent_text: text_on(accent),
        vivid,
        swatches,
        page_wash: wash_alpha(sample, tint, &PAGE_GLOW),
        player_wash: wash_alpha(sample, tint, &PLAYER),
    }
}

/// The least alpha (a multiple of 0.01, at least the surface's floor) of
/// `tint` over `sample` drawn as `surface` draws it at which every text
/// level keeps its contrast ([`TEXT_LEVELS`]) over the brightest blurred
/// area: every pixel of the sample blurred by the surface's Gaussian (on
/// sRGB values, as the webview blurs), saturated (CSS `saturate()`), at the surface's
/// opacity over its `under`, then the tint over that, all composited in
/// sRGB as the webview does. A full wash (the tint alone) always passes:
/// tints are deep (white reads at ≥ 12:1 on them).
pub fn wash_alpha(sample: &CoverImage, tint: Rgba, surface: &Surface) -> f64 {
    let mut glows: Vec<[f64; 3]> = blurred(sample, surface.sigma)
        .into_iter()
        .map(|mean| {
            let saturated = saturate(mean, surface.saturate);
            let under = unit(surface.under);
            std::array::from_fn(|i| {
                surface.opacity * saturated[i] + (1.0 - surface.opacity) * under[i]
            })
        })
        .collect();
    // The brightest first: a failing alpha usually fails on one of them.
    glows.sort_by(|a, b| luma(*b).total_cmp(&luma(*a)));
    let tint_unit = unit(tint);
    let reads = |alpha: f64| {
        glows.iter().all(|glow| {
            let washed = from_unit(std::array::from_fn(|i| {
                alpha * tint_unit[i] + (1.0 - alpha) * glow[i]
            }));
            TEXT_LEVELS
                .iter()
                .all(|&(text, min)| contrast(text, washed) >= min)
        })
    };
    let floor = (surface.floor * 100.0).ceil() as u32;
    (floor..=100)
        .map(|step| f64::from(step) / 100.0)
        .find(|&alpha| reads(alpha))
        .unwrap_or(1.0)
}

/// `image` (sRGB, 0–1) blurred by a Gaussian with standard deviations
/// `sigma` (shares of its width and height), separably, edges extended
/// (as bright as the edge: never darker than the drawn blur, whose edges
/// fade into what is under them or are cut off by the bleed).
fn blurred(image: &CoverImage, sigma: (f64, f64)) -> Vec<[f64; 3]> {
    let (width, height) = (image.width() as usize, image.height() as usize);
    let pixels: Vec<[f64; 3]> = image
        .rgb()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| p.map(|c| f64::from(c) / 255.0))
        .collect();
    let across = kernel(sigma.0 * width as f64);
    let down = kernel(sigma.1 * height as f64);
    let pass = |src: &[[f64; 3]], kernel: &[f64], horizontal: bool| -> Vec<[f64; 3]> {
        let reach = (kernel.len() / 2) as isize;
        let mut out = vec![[0.0; 3]; src.len()];
        for y in 0..height {
            for x in 0..width {
                let mut sum = [0.0; 3];
                for (k, w) in kernel.iter().enumerate() {
                    let offset = k as isize - reach;
                    let (sx, sy) = if horizontal {
                        (
                            (x as isize + offset).clamp(0, width as isize - 1) as usize,
                            y,
                        )
                    } else {
                        (
                            x,
                            (y as isize + offset).clamp(0, height as isize - 1) as usize,
                        )
                    };
                    let p = src[sy * width + sx];
                    for c in 0..3 {
                        sum[c] += w * p[c];
                    }
                }
                out[y * width + x] = sum;
            }
        }
        out
    };
    let rows = pass(&pixels, &across, true);
    pass(&rows, &down, false)
}

/// A normalised Gaussian kernel for `sigma` pixels, out to 3σ (just the
/// pixel itself below a tenth of a pixel).
fn kernel(sigma: f64) -> Vec<f64> {
    if sigma < 0.1 {
        return vec![1.0];
    }
    let reach = (3.0 * sigma).ceil() as isize;
    let weights: Vec<f64> = (-reach..=reach)
        .map(|i| (-(i as f64).powi(2) / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f64 = weights.iter().sum();
    weights.into_iter().map(|w| w / total).collect()
}

/// CSS `saturate(amount)` (the Filter Effects matrix) on sRGB values.
fn saturate([r, g, b]: [f64; 3], s: f64) -> [f64; 3] {
    [
        (0.213 + 0.787 * s) * r + (0.715 - 0.715 * s) * g + (0.072 - 0.072 * s) * b,
        (0.213 - 0.213 * s) * r + (0.715 + 0.285 * s) * g + (0.072 - 0.072 * s) * b,
        (0.213 - 0.213 * s) * r + (0.715 - 0.715 * s) * g + (0.072 + 0.928 * s) * b,
    ]
    .map(|c| c.clamp(0.0, 1.0))
}

/// A rough brightness, only to order the windows.
fn luma([r, g, b]: [f64; 3]) -> f64 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn unit(c: Rgba) -> [f64; 3] {
    [c.r, c.g, c.b].map(|v| f64::from(v) / 255.0)
}

fn from_unit(c: [f64; 3]) -> Rgba {
    let [r, g, b] = c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    Rgba::rgb(r, g, b)
}

/// Up to [`MAX_SWATCHES`] colours of the populated hues, the largest
/// first (by their own colourful pixels), each at least
/// [`SWATCH_MIN_GAP`] degrees of hue from those before it.
fn swatches(pixels: &[Px], counts: &[f64; HUE_BINS], populated: &[usize]) -> Vec<Rgba> {
    let mut bins = populated.to_vec();
    // Largest first; equal ones by bin (stable sort).
    bins.sort_by(|a, b| counts[*b].total_cmp(&counts[*a]));
    let mut hues: Vec<f64> = Vec::new();
    let mut out = Vec::new();
    for bin in bins {
        if out.len() == MAX_SWATCHES {
            break;
        }
        let members: Vec<&Px> = pixels
            .iter()
            .filter(|p| p.bin == bin && p.c >= GREY_CHROMA)
            .collect();
        if members.is_empty() {
            continue;
        }
        let n = members.len() as f64;
        let (a, b) = members
            .iter()
            .fold((0.0, 0.0), |(a, b), p| (a + p.a * p.c, b + p.b * p.c));
        let hue = b.atan2(a);
        let l = members.iter().map(|p| p.l).sum::<f64>() / n;
        let distinct = hues.iter().all(|&h| {
            let gap = (hue - h).to_degrees().rem_euclid(360.0);
            gap.min(360.0 - gap) >= SWATCH_MIN_GAP
        });
        if !distinct {
            continue;
        }
        hues.push(hue);
        out.push(Rgba::from_oklch_in_gamut(
            l.clamp(SWATCH_L.0, SWATCH_L.1),
            SWATCH_MAX_C,
            hue,
        ));
    }
    out
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
fn greyscale(pixels: &[Px]) -> (Rgba, Rgba) {
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
    (tint, accent)
}

#[cfg(test)]
mod tests;
