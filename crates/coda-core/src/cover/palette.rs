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
//!   [`MAX_SWATCHES`] colours of the populated hues, the heaviest first
//!   (area × chroma², dark pixels counting less, so a large dull region
//!   does not outvote a vivid one),
//!   at least [`SWATCH_MIN_GAP`] degrees of hue apart, each at its
//!   pixels' mean hue and lightness (clamped to [`SWATCH_L`]) with as much
//!   chroma as sRGB allows (up to [`SWATCH_MAX_C`]). None for a greyscale
//!   cover.
//! - **Ground** (the app's "Washed" page colour, 2.0 R2; a field of up to
//!   [`MAX_GROUNDS`] stops since R3, one per heaviest swatch): the colours
//!   a page about this cover is painted with, top to bottom: each stop at
//!   its swatch's hue (a greyscale cover: one stop at its slight cast, or
//!   the base's violet, at a low chroma) at a deep but clearly coloured lightness: the lightest
//!   step from [`GROUND_L_TOP`] down at which every text level keeps its
//!   contrast on it even under the lightest veil the app lays under text
//!   ([`GROUND_VEIL`], a selected row). With it, the **glass** solid: the
//!   ground darkened (mixed with black) as little as keeps the text levels
//!   on the app's floating glass ([`GLASS_ALPHA`]) over pure white. Since
//!   R3 the ground's hue (the heaviest swatch's) can differ from the
//!   tint's (the dominant hue's, by area-weighted chroma).
//! - **Washes** ([`Surface`], [`wash_alpha`]): where the app draws the
//!   cover blurred with text over it (the page's glow, the player), how
//!   much of a colour (the page's ground; the player's tint) it lays over
//!   the cover so that the text levels keep their contrast over **this**
//!   cover's brightest blurred area, not over the worst cover there is: a
//!   dark cover needs little wash and stays colourful, a white one gets a
//!   heavy one.

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
/// The same on a page's ground (`--swatch-under-text-ground`): the card
/// surface over any ground ([`Palette::ground`]), which is lighter than
/// the base, leaves less room (tested over grounds of every hue).
pub const SWATCH_UNDER_TEXT_ON_GROUND: f64 = 0.10;
/// The desktop app's card surface (`--surface-1`), white at 0x0e.
pub const CARD_SURFACE: Rgba = Rgba {
    r: 0xff,
    g: 0xff,
    b: 0xff,
    a: 0x0e,
};

/// The page ground's highest OKLCH lightness: the ground is tried from
/// here down (in steps of 0.005) until the text levels hold under the
/// veil; in practice they settle at 0.24–0.31 (yellows and greys deepest,
/// reds and blues lightest).
const GROUND_L_TOP: f64 = 0.34;
/// A ground stop's chroma: its swatch's pixels' mean chroma × 1.2, within
/// these (then into the sRGB gamut).
const GROUND_C: (f64, f64) = (0.06, 0.14);
/// The ground field's stops at most.
pub const MAX_GROUNDS: usize = 3;
/// A greyscale cover's ground chroma: near neutral, with its cast.
const GREY_GROUND_C: f64 = 0.025;
/// The lightest veil the desktop app lays under text on a page (its
/// `--selected`, white at 0x1f; a test there keeps them in step): the
/// ground keeps the text levels under it too.
pub const GROUND_VEIL: Rgba = Rgba {
    r: 0xff,
    g: 0xff,
    b: 0xff,
    a: 0x1f,
};
/// The most of a cover's bloom (the desktop app's blurred art around a
/// cover, 2.0 R3) where text may sit: the alpha its mask has at the
/// distance from the cover where text can start (`--bloom-edge` in its
/// `tokens.css`; a test there keeps them in step). It is the veil's: any
/// colour at this alpha is no lighter than white at it, and every ground
/// keeps the text levels under white at it.
pub const BLOOM_TEXT_ALPHA: u8 = GROUND_VEIL.a;

/// The desktop app's floating glass's alpha (`--glass`, `#242227e2` on the
/// base): on a page with a ground its colour is the cover's `glass`.
pub const GLASS_ALPHA: u8 = 0xe2;
/// The stronger glass's alpha (`--glass-strong`, `#242227f0` on the base:
/// sticky tool rows, menus); the same colour, so it reads at least as
/// well.
pub const GLASS_STRONG_ALPHA: u8 = 0xf0;

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
/// the cover blurred (and saturated) at some opacity over what is under
/// it, then a wash of a colour at [`wash_alpha`]. The CSS draws it with the
/// same constants (the app's `tokens.css`, `--*-glow-opacity` and
/// `--*-glow-filter`; a test there keeps them in step).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    /// The cover's opacity over what is under it.
    pub opacity: f64,
    /// The CSS `saturate()` after the blur.
    pub saturate: f64,
    /// The least wash, so the cover never shows bare.
    pub floor: f64,
    /// The blur's standard deviation (CSS `blur(r)` is a Gaussian with
    /// σ = r) as shares of the cover's width and height at the largest
    /// size the surface draws it (smaller is safer: a sharper blur keeps
    /// bright areas brighter).
    pub sigma: (f64, f64),
}

/// The page's glow (app.css `.page-glow`): the cover at 0.62 under
/// `blur(110px) saturate(1.5)`, over the page's ground and washed with it.
/// It is drawn 120% of the
/// page's width wide (and as tall: `object-fit: cover` of a square), so
/// at most ~1.2 × 2560px on a large screen: σ = 110 / 3072 of the cover.
pub const PAGE_GLOW: Surface = Surface {
    opacity: 0.62,
    saturate: 1.5,
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
    /// Up to [`MAX_SWATCHES`] vivid colours of the cover, the heaviest
    /// first (by visual weight, see [`swatches`]), for gradients. Empty
    /// when greyscale.
    pub swatches: Vec<Rgba>,
    /// The colour of a page about this cover (see the module's "Ground"):
    /// the first of `grounds`.
    pub ground: Rgba,
    /// The page's ground field's stops (2.0 R3): one per heaviest swatch
    /// (at most [`MAX_GROUNDS`]), each at the ground's text-safe depth;
    /// one neutral stop for a greyscale cover. Any sRGB mix of them is as
    /// readable as the stops (luminance is convex in sRGB values).
    pub grounds: Vec<Rgba>,
    /// The floating glass's colour on that page (opaque; drawn at
    /// [`GLASS_ALPHA`]).
    pub glass: Rgba,
    /// The ground's alpha over the cover in the page's glow ([`PAGE_GLOW`]).
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
        let (tint, accent, hue) = greyscale(&pixels);
        let grounds = vec![ground_of(hue, GREY_GROUND_C)];
        return finish(&sample, tint, accent, false, Vec::new(), grounds);
    }

    // The dominant hue: chroma- and lightness-weighted, smoothed.
    let mut weights = [0.0f64; HUE_BINS];
    for p in pixels.iter().filter(|p| p.c >= GREY_CHROMA) {
        weights[p.bin] += p.c.min(WEIGHT_C_CAP) * mid_weight(p.l);
    }
    let smoothed = smooth(&weights);
    let dominant = argmax(&smoothed);
    let (tint, hue, mean_c) = tint_of(&pixels, dominant);

    let accent = vivid_accent(&pixels, &counts, &populated, total);
    let accent = lift_until_readable(accent, &[BASE, tint]);
    let swatches = swatches(&pixels, &populated);
    // The ground stops: the heaviest swatches' hues (the dominant hue if
    // none, which cannot happen for a vivid cover), each at the ground's
    // text-safe depth.
    let mut grounds: Vec<Rgba> = swatches
        .iter()
        .take(MAX_GROUNDS)
        .map(|s| ground_of(s.hue, ground_chroma(s.chroma)))
        .collect();
    if grounds.is_empty() {
        grounds.push(ground_of(hue, ground_chroma(mean_c)));
    }
    let swatches = swatches.into_iter().map(|s| s.color).collect();
    finish(&sample, tint, accent, true, swatches, grounds)
}

/// A ground stop's chroma for a hue whose pixels have `mean_c`.
fn ground_chroma(mean_c: f64) -> f64 {
    (mean_c * 1.2).clamp(GROUND_C.0, GROUND_C.1)
}

fn finish(
    sample: &CoverImage,
    tint: Rgba,
    accent: Rgba,
    vivid: bool,
    swatches: Vec<Rgba>,
    grounds: Vec<Rgba>,
) -> Palette {
    let ground = grounds[0];
    Palette {
        tint,
        accent,
        accent_text: text_on(accent),
        vivid,
        swatches,
        ground,
        grounds,
        glass: glass_of(ground),
        page_wash: wash_alpha(sample, ground, ground, &PAGE_GLOW),
        player_wash: wash_alpha(sample, tint, BASE, &PLAYER),
    }
}

/// Whether every text level keeps its contrast on `background`.
fn readable(background: Rgba) -> bool {
    TEXT_LEVELS
        .iter()
        .all(|&(text, min)| contrast(text, background) >= min)
}

/// `top` (its alpha) over the opaque `under`, sRGB compositing.
fn over(top: Rgba, under: Rgba) -> Rgba {
    let a = f64::from(top.a) / 255.0;
    let (t, u) = (unit(top), unit(under));
    from_unit(std::array::from_fn(|i| a * t[i] + (1.0 - a) * u[i]))
}

/// The ground at `hue` and up to `chroma`: the lightest step down from
/// [`GROUND_L_TOP`] at which the text levels hold on it under
/// [`GROUND_VEIL`] (the base if nothing does, which cannot happen: deep
/// enough, every colour passes).
fn ground_of(hue: f64, chroma: f64) -> Rgba {
    let mut step = 0;
    loop {
        let l = GROUND_L_TOP - f64::from(step) * 0.005;
        if l <= 0.0 {
            return BASE;
        }
        let ground = Rgba::from_oklch_in_gamut(l, chroma, hue);
        if readable(over(GROUND_VEIL, ground)) {
            return ground;
        }
        step += 1;
    }
}

/// The glass colour on `ground`: the ground mixed with black (in sRGB, as
/// CSS `color-mix` does) as little as keeps the text levels on it at
/// [`GLASS_ALPHA`] over pure white, the worst thing glass floats over.
fn glass_of(ground: Rgba) -> Rgba {
    let g = unit(ground);
    for step in (0..=100).rev() {
        let share = f64::from(step) / 100.0;
        let glass = from_unit(g.map(|c| c * share));
        let solid = Rgba {
            a: GLASS_ALPHA,
            ..glass
        };
        if readable(over(solid, color::WHITE)) {
            return glass;
        }
    }
    color::BLACK
}

/// The least alpha (a multiple of 0.01, at least the surface's floor) of
/// `wash` over `sample` drawn as `surface` draws it (over `under`) at which every text
/// level keeps its contrast ([`TEXT_LEVELS`]) over the brightest blurred
/// area: every pixel of the sample blurred by the surface's Gaussian (on
/// sRGB values, as the webview blurs), saturated (CSS `saturate()`), at the surface's
/// opacity over `under`, then the wash over that, all composited in sRGB
/// as the webview does. A full wash (the colour alone) passes for the
/// colours used: tints are deep (white reads at ≥ 12:1 on them) and a
/// ground is made to pass.
pub fn wash_alpha(sample: &CoverImage, wash: Rgba, under: Rgba, surface: &Surface) -> f64 {
    let mut glows: Vec<[f64; 3]> = blurred(sample, surface.sigma)
        .into_iter()
        .map(|mean| {
            let saturated = saturate(mean, surface.saturate);
            let under = unit(under);
            std::array::from_fn(|i| {
                surface.opacity * saturated[i] + (1.0 - surface.opacity) * under[i]
            })
        })
        .collect();
    // The brightest first: a failing alpha usually fails on one of them.
    glows.sort_by(|a, b| luma(*b).total_cmp(&luma(*a)));
    let wash = unit(wash);
    let reads = |alpha: f64| {
        glows.iter().all(|glow| {
            readable(from_unit(std::array::from_fn(|i| {
                alpha * wash[i] + (1.0 - alpha) * glow[i]
            })))
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

/// Up to [`MAX_SWATCHES`] colours of the populated hues, the heaviest
/// first (their colourful pixels' summed chroma², dark ones counting less), each at least
/// [`SWATCH_MIN_GAP`] degrees of hue from those before it.
fn swatches(pixels: &[Px], populated: &[usize]) -> Vec<Swatch> {
    // Visual weight: each hue's colourful pixels' summed chroma squared,
    // dark ones counting less (`presence`): a large dull, dark region (a
    // dark orange road) does not outvote a smaller vivid one (a ring of
    // bright hues around it).
    let mut weights = [0.0f64; HUE_BINS];
    for p in pixels.iter().filter(|p| p.c >= GREY_CHROMA) {
        weights[p.bin] += p.c * p.c * presence(p.l);
    }
    let mut bins = populated.to_vec();
    // Heaviest first; equal ones by bin (stable sort).
    bins.sort_by(|a, b| weights[*b].total_cmp(&weights[*a]));
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
        let chroma = members.iter().map(|p| p.c).sum::<f64>() / n;
        let distinct = hues.iter().all(|&h| {
            let gap = (hue - h).to_degrees().rem_euclid(360.0);
            gap.min(360.0 - gap) >= SWATCH_MIN_GAP
        });
        if !distinct {
            continue;
        }
        hues.push(hue);
        out.push(Swatch {
            color: Rgba::from_oklch_in_gamut(l.clamp(SWATCH_L.0, SWATCH_L.1), SWATCH_MAX_C, hue),
            hue,
            chroma,
        });
    }
    out
}

/// How much a pixel of lightness `l` counts towards its hue's weight: 1
/// from L 0.6, down to a quarter for the darkest.
fn presence(l: f64) -> f64 {
    ((l - 0.15) / 0.45).clamp(0.25, 1.0)
}

/// A swatch and what the ground stops are made from: its pixels' mean hue
/// and chroma.
struct Swatch {
    color: Rgba,
    hue: f64,
    chroma: f64,
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
/// [`TINT_MAX_C`]; with that hue and their mean chroma.
fn tint_of(pixels: &[Px], dominant: usize) -> (Rgba, f64, f64) {
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
    let hue = b.atan2(a);
    (Rgba::from_oklch_in_gamut(TINT_L, chroma, hue), hue, mean_c)
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
/// base's violet when it has none) and a soft neutral accent; and that hue.
fn greyscale(pixels: &[Px]) -> (Rgba, Rgba, f64) {
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
    (tint, accent, hue)
}

#[cfg(test)]
mod tests;
