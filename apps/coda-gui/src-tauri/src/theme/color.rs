//! Colours: strict hex parsing, OKLab mixing (the same maths as CSS
//! `color-mix(in oklab, …)`), WCAG contrast and an OKLCH lightness nudge
//! for the contrast guard.

use std::fmt;

/// An sRGB colour with alpha (255 = opaque).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

pub const BLACK: Rgba = Rgba::rgb(0, 0, 0);
pub const WHITE: Rgba = Rgba::rgb(255, 255, 255);

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// `#rrggbb` (Ghostty also accepts it without `#`). Nothing else: no
    /// names, no short form, no alpha.
    pub fn parse_rgb(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#').unwrap_or(text);
        if hex.len() != 6 {
            return None;
        }
        Self::parse_hex_digits(hex)
    }

    /// `#rrggbb` or `#rrggbbaa` (Coda's own keys may carry alpha).
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#').unwrap_or(text);
        match hex.len() {
            6 | 8 => Self::parse_hex_digits(hex),
            _ => None,
        }
    }

    fn parse_hex_digits(hex: &str) -> Option<Self> {
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Self {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a: if hex.len() == 8 { byte(6)? } else { 255 },
        })
    }

    pub fn is_opaque(self) -> bool {
        self.a == 255
    }

    pub fn opaque(self) -> Self {
        Self { a: 255, ..self }
    }

    /// The same colour at `alpha` (0–1).
    pub fn with_alpha(self, alpha: f64) -> Self {
        Self {
            a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
            ..self
        }
    }

    /// This colour drawn over an opaque `below`.
    pub fn over(self, below: Rgba) -> Rgba {
        let a = f64::from(self.a) / 255.0;
        let mix = |top: u8, bottom: u8| {
            (f64::from(top) * a + f64::from(bottom) * (1.0 - a)).round() as u8
        };
        Rgba::rgb(
            mix(self.r, below.r),
            mix(self.g, below.g),
            mix(self.b, below.b),
        )
    }

    /// WCAG relative luminance (alpha ignored).
    pub fn luminance(self) -> f64 {
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
    pub fn oklab(self) -> [f64; 3] {
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

    pub fn from_oklab([ll, aa, bb]: [f64; 3], alpha: u8) -> Self {
        let l = (ll + 0.396_337_777_4 * aa + 0.215_803_757_3 * bb).powi(3);
        let m = (ll - 0.105_561_345_8 * aa - 0.063_854_172_8 * bb).powi(3);
        let s = (ll - 0.089_484_177_5 * aa - 1.291_485_548_0 * bb).powi(3);
        Self::from_linear(
            [
                4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s,
                -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s,
                -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701_0 * s,
            ],
            alpha,
        )
    }

    /// OKLab lightness (0 black – 1 white).
    pub fn lightness(self) -> f64 {
        self.oklab()[0]
    }

    /// `color-mix(in oklab, self, other t)`: `t` = how much of `other`.
    pub fn mix(self, other: Rgba, t: f64) -> Rgba {
        let t = t.clamp(0.0, 1.0);
        let [l1, a1, b1] = self.oklab();
        let [l2, a2, b2] = other.oklab();
        let alpha = f64::from(self.a) * (1.0 - t) + f64::from(other.a) * t;
        Rgba::from_oklab(
            [l1 + (l2 - l1) * t, a1 + (a2 - a1) * t, b1 + (b2 - b1) * t],
            alpha.round() as u8,
        )
    }

    /// The same hue and chroma (OKLCH) at lightness `l`.
    fn at_lightness(self, l: f64) -> Rgba {
        let [_, a, b] = self.oklab();
        Rgba::from_oklab([l.clamp(0.0, 1.0), a, b], self.a)
    }
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

/// WCAG contrast ratio (1–21) of two opaque colours.
pub fn contrast(a: Rgba, b: Rgba) -> f64 {
    let (la, lb) = (a.luminance(), b.luminance());
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// `color` moved in OKLCH lightness (away from `against`) until its
/// contrast with `against` is at least `min`. Unchanged if it passes;
/// the extreme of the range if nothing in it does.
pub fn ensure_contrast(color: Rgba, against: Rgba, min: f64) -> Rgba {
    ensure_contrast_toward(color, against, min, lighter_reads_better(against))
}

/// Whether white contrasts more with `background` than black does.
pub fn lighter_reads_better(background: Rgba) -> bool {
    contrast(WHITE, background) >= contrast(BLACK, background)
}

/// [`ensure_contrast`] in a given direction (lighter or darker).
pub fn ensure_contrast_toward(color: Rgba, against: Rgba, min: f64, lighten: bool) -> Rgba {
    if contrast(color.opaque(), against) >= min {
        return color;
    }
    let start = color.lightness();
    let mut best = color;
    for step in 1..=100 {
        let l = if lighten {
            start + f64::from(step) * 0.01
        } else {
            start - f64::from(step) * 0.01
        };
        if !(0.0..=1.0).contains(&l) {
            break;
        }
        best = color.at_lightness(l);
        if contrast(best.opaque(), against) >= min {
            return best;
        }
    }
    // Chroma kept it short: the extreme itself.
    let extreme = if lighten { WHITE } else { BLACK };
    if contrast(extreme, against) > contrast(best.opaque(), against) {
        best = Rgba {
            a: color.a,
            ..extreme
        };
    }
    best
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
        ] {
            assert_eq!(Rgba::parse_rgb(bad), None, "{bad:?}");
        }
        assert_eq!(
            Rgba::parse("#0000008c"),
            Some(Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 0x8c
            })
        );
        assert_eq!(Rgba::parse("#00000"), None);
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
        }
    }

    #[test]
    fn mix_ends_and_middle() {
        let a = Rgba::rgb(0x10, 0x10, 0x10);
        let b = Rgba::rgb(0xf0, 0xf0, 0xf0);
        assert_eq!(a.mix(b, 0.0), a);
        assert_eq!(a.mix(b, 1.0), b);
        let mid = a.mix(b, 0.5);
        assert!(mid.lightness() > a.lightness() && mid.lightness() < b.lightness());
    }

    #[test]
    fn contrast_matches_wcag() {
        assert!((contrast(BLACK, WHITE) - 21.0).abs() < 1e-9);
        assert!((contrast(WHITE, WHITE) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn contrast_guard_moves_lightness_until_it_passes() {
        let bg = Rgba::rgb(0x0d, 0x0c, 0x0c);
        let dim = Rgba::rgb(0x30, 0x28, 0x50);
        assert!(contrast(dim, bg) < 3.0);
        let fixed = ensure_contrast(dim, bg, 3.0);
        assert!(contrast(fixed, bg) >= 3.0);
        assert!(fixed.lightness() > dim.lightness());
        // On a light background it darkens.
        let light = Rgba::rgb(0xf2, 0xf1, 0xef);
        let pale = Rgba::rgb(0xe0, 0xd0, 0x90);
        let fixed = ensure_contrast(pale, light, 3.0);
        assert!(contrast(fixed, light) >= 3.0);
        assert!(fixed.lightness() < pale.lightness());
        // A passing colour is left alone.
        assert_eq!(ensure_contrast(WHITE, bg, 4.5), WHITE);
    }

    #[test]
    fn text_on_picks_the_readable_one() {
        assert_eq!(text_on(Rgba::rgb(0xf0, 0xa8, 0x3a)), BLACK);
        assert_eq!(text_on(Rgba::rgb(0x88, 0x39, 0xef)), WHITE);
    }
}
