//! Colours: strict hex parsing and formatting, OKLab / OKLCH (the same
//! maths as CSS `oklab()` / `oklch()`), mapping into the sRGB gamut and
//! WCAG contrast. Pure; used by the cover palettes ([`super::palette`])
//! and by the desktop app's accent preferences.

use std::fmt;

/// An sRGB colour with alpha (255 = opaque).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

pub(crate) const BLACK: Rgba = Rgba::rgb(0, 0, 0);
pub(crate) const WHITE: Rgba = Rgba::rgb(255, 255, 255);

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// `#rrggbb` (or without `#`). Nothing else: no names, no short form,
    /// no alpha.
    pub fn parse_rgb(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#').unwrap_or(text);
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Self::rgb(byte(0)?, byte(2)?, byte(4)?))
    }

    /// The same colour at `alpha` (0–1).
    pub fn with_alpha(self, alpha: f64) -> Self {
        Self {
            a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
            ..self
        }
    }

    /// WCAG relative luminance (alpha ignored).
    pub(crate) fn luminance(self) -> f64 {
        let [r, g, b] = self.linear();
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    fn linear(self) -> [f64; 3] {
        let f = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        [f(self.r), f(self.g), f(self.b)]
    }

    fn from_linear([r, g, b]: [f64; 3], a: u8) -> Self {
        let f = |c: f64| {
            let c = if c <= 0.003_130_8 {
                12.92 * c
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            };
            (c.clamp(0.0, 1.0) * 255.0).round() as u8
        };
        Self {
            r: f(r),
            g: f(g),
            b: f(b),
            a,
        }
    }

    /// OKLab `[L, a, b]`.
    pub(crate) fn oklab(self) -> [f64; 3] {
        let [r, g, b] = self.linear();
        let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
        let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
        let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
        [
            0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
            1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
            0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
        ]
    }

    /// The colour at OKLab `[L, a, b]`, each channel clipped to sRGB.
    pub(crate) fn from_oklab(lab: [f64; 3], alpha: u8) -> Self {
        Self::from_linear(oklab_to_linear(lab), alpha)
    }

    /// OKLCH `[L, C, h]`, the hue in radians (`atan2(b, a)`).
    pub(crate) fn oklch(self) -> [f64; 3] {
        let [l, a, b] = self.oklab();
        [l, a.hypot(b), b.atan2(a)]
    }

    /// The opaque colour at OKLCH lightness `l` and hue `h` (radians) with
    /// the most chroma up to `c` that sRGB can show: chroma is reduced
    /// (lightness and hue kept) until the colour is in gamut.
    pub(crate) fn from_oklch_in_gamut(l: f64, c: f64, h: f64) -> Self {
        let l = l.clamp(0.0, 1.0);
        let lab = |c: f64| [l, c * h.cos(), c * h.sin()];
        let fits = |c: f64| {
            oklab_to_linear(lab(c))
                .iter()
                .all(|v| (-1e-4..=1.0 + 1e-4).contains(v))
        };
        let c = c.max(0.0);
        if fits(c) {
            return Self::from_oklab(lab(c), 255);
        }
        // Grey always fits; bisect the largest chroma that does.
        let (mut lo, mut hi) = (0.0, c);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            if fits(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Self::from_oklab(lab(lo), 255)
    }
}

fn oklab_to_linear([ll, aa, bb]: [f64; 3]) -> [f64; 3] {
    let l = (ll + 0.396_337_777_4 * aa + 0.215_803_757_3 * bb).powi(3);
    let m = (ll - 0.105_561_345_8 * aa - 0.063_854_172_8 * bb).powi(3);
    let s = (ll - 0.089_484_177_5 * aa - 1.291_485_548_0 * bb).powi(3);
    [
        4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s,
        -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s,
        -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701_0 * s,
    ]
}

impl fmt::Display for Rgba {
    /// `#rrggbb`, or `#rrggbbaa` when not opaque.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)?;
        if self.a != 255 {
            write!(f, "{:02x}", self.a)?;
        }
        Ok(())
    }
}

/// WCAG contrast ratio (1–21) of two opaque colours (alpha ignored).
pub fn contrast(a: Rgba, b: Rgba) -> f64 {
    let (la, lb) = (a.luminance(), b.luminance());
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Black or white, whichever reads better on `background`.
pub fn text_on(background: Rgba) -> Rgba {
    if contrast(BLACK, background) >= contrast(WHITE, background) {
        BLACK
    } else {
        WHITE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_hex() {
        assert_eq!(
            Rgba::parse_rgb("#1f1f28"),
            Some(Rgba::rgb(0x1f, 0x1f, 0x28))
        );
        assert_eq!(Rgba::parse_rgb("1F1F28"), Some(Rgba::rgb(0x1f, 0x1f, 0x28)));
        for bad in [
            "",
            "#fff",
            "#1f1f2",
            "#1f1f288",
            "red",
            "#gggggg",
            "#1f1f28ff",
            " #1f1f28",
            "#+1f1f2",
        ] {
            assert_eq!(Rgba::parse_rgb(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn formats_hex_with_alpha_only_when_needed() {
        assert_eq!(Rgba::rgb(1, 2, 255).to_string(), "#0102ff");
        assert_eq!(BLACK.with_alpha(0.55).to_string(), "#0000008c");
    }

    #[test]
    fn oklab_round_trips() {
        for c in [
            Rgba::rgb(0xf0, 0xa8, 0x3a),
            Rgba::rgb(0x1e, 0x1e, 0x2e),
            WHITE,
            BLACK,
        ] {
            assert_eq!(Rgba::from_oklab(c.oklab(), 255), c);
            let [l, ch, h] = c.oklch();
            assert_eq!(Rgba::from_oklch_in_gamut(l, ch, h), c);
        }
    }

    #[test]
    fn out_of_gamut_chroma_is_reduced_keeping_lightness() {
        // A very saturated blue at a high lightness does not exist in sRGB.
        let h = 264f64.to_radians();
        let c = Rgba::from_oklch_in_gamut(0.8, 0.3, h);
        let [l, ch, hue] = c.oklch();
        assert!((l - 0.8).abs() < 0.01, "{l}");
        assert!(ch < 0.3 && ch > 0.05, "{ch}");
        let gap = (hue - h).rem_euclid(std::f64::consts::TAU);
        assert!(gap.min(std::f64::consts::TAU - gap) < 0.1, "{hue}");
        // Not clipped to an extreme channel value that shifts the hue.
        assert_ne!(c, WHITE);
    }

    #[test]
    fn contrast_matches_wcag() {
        assert!((contrast(BLACK, WHITE) - 21.0).abs() < 1e-9);
        assert!((contrast(WHITE, WHITE) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn text_on_picks_the_readable_one() {
        assert_eq!(text_on(Rgba::rgb(0xf0, 0xa8, 0x3a)), BLACK);
        assert_eq!(text_on(Rgba::rgb(0x88, 0x39, 0xef)), WHITE);
    }
}
