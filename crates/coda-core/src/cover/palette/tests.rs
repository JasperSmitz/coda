use super::color::{BLACK, WHITE};
use super::*;

/// A `width` × `height` cover drawn by `f(x, y)`.
fn image(width: u32, height: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> CoverImage {
    let mut rgb = Vec::new();
    for y in 0..height {
        for x in 0..width {
            rgb.extend_from_slice(&f(x, y));
        }
    }
    CoverImage::new(width, height, rgb).unwrap()
}

fn solid(color: [u8; 3]) -> CoverImage {
    image(64, 64, |_, _| color)
}

fn hex(c: [u8; 3]) -> Rgba {
    Rgba::rgb(c[0], c[1], c[2])
}

/// The hue distance in degrees.
fn hue_gap(a: Rgba, b: Rgba) -> f64 {
    let d = (a.oklch()[2] - b.oklch()[2]).to_degrees().rem_euclid(360.0);
    d.min(360.0 - d)
}

/// The guarantees every palette keeps (DESIGN.md "Contrast").
fn assert_readable(name: &str, p: &Palette) {
    for c in [p.tint, p.accent, p.accent_text] {
        assert_eq!(c.a, 255, "{name}: opaque");
    }
    assert!(
        contrast(p.accent, BASE) >= MIN_ACCENT_CONTRAST,
        "{name}: accent {} on the base: {:.2}",
        p.accent,
        contrast(p.accent, BASE)
    );
    assert!(
        contrast(p.accent, p.tint) >= MIN_ACCENT_CONTRAST,
        "{name}: accent {} on its tint {}: {:.2}",
        p.accent,
        p.tint,
        contrast(p.accent, p.tint)
    );
    assert!(p.accent_text == BLACK || p.accent_text == WHITE, "{name}");
    assert!(
        contrast(p.accent_text, p.accent) >= contrast(text_on_other(p.accent_text), p.accent),
        "{name}: the better of black and white"
    );
    // Tints are deep: white text reads on them as on the base.
    assert!(
        p.tint.oklch()[0] <= TINT_L + 0.01,
        "{name}: tint {}",
        p.tint
    );
    assert!(contrast(WHITE, p.tint) >= 12.0, "{name}: tint {}", p.tint);
    let [_, chroma, _] = p.tint.oklch();
    assert!(chroma <= TINT_MAX_C + 0.005, "{name}: tint chroma {chroma}");
    if p.vivid {
        let [l, c, _] = p.accent.oklch();
        assert!(l >= ACCENT_L.0 - 0.01, "{name}: accent lightness {l}");
        assert!(c <= ACCENT_C.1 + 0.005, "{name}: accent chroma {c}");
    }
}

fn text_on_other(c: Rgba) -> Rgba {
    if c == BLACK { WHITE } else { BLACK }
}

#[test]
fn solid_colours() {
    let red = [0xe0, 0x20, 0x20];
    let blue = [0x20, 0x40, 0xe0];
    for (name, color) in [("red", red), ("blue", blue)] {
        let p = palette(&solid(color));
        assert_readable(name, &p);
        assert!(p.vivid, "{name}");
        assert!(hue_gap(p.accent, hex(color)) < 8.0, "{name}: {}", p.accent);
        assert!(hue_gap(p.tint, hex(color)) < 8.0, "{name}: {}", p.tint);
    }
    for (name, color) in [
        ("near-black", [0x0c, 0x0b, 0x0e]),
        ("white", [0xff, 0xff, 0xff]),
    ] {
        let p = palette(&solid(color));
        assert_readable(name, &p);
        assert!(!p.vivid, "{name}");
    }
}

#[test]
fn a_greyscale_gradient_is_not_vivid() {
    let gradient = image(64, 64, |x, y| {
        let v = ((x + y) * 2) as u8;
        [v, v, v]
    });
    let p = palette(&gradient);
    assert!(!p.vivid);
    assert_readable("gradient", &p);
    // Nearly neutral, with the base's violet.
    assert!(p.tint.oklch()[1] <= GREY_TINT_C + 0.005);
    assert!(p.accent.oklch()[1] <= 0.03, "soft neutral {}", p.accent);
}

#[test]
fn a_small_vivid_area_gives_the_accent_and_a_stray_pixel_does_not() {
    let orange = [0xff, 0x8c, 0x00];
    // A dark cover with a 16 × 16 orange square (6% of it) and one
    // brighter, more saturated blue 2 × 2 speck (one sample pixel).
    let cover = image(64, 64, |x, y| {
        if (40..56).contains(&x) && (8..24).contains(&y) {
            orange
        } else if (4..6).contains(&x) && (50..52).contains(&y) {
            [0x00, 0x30, 0xff]
        } else {
            [0x14, 0x13, 0x16]
        }
    });
    let p = palette(&cover);
    assert!(p.vivid);
    assert_readable("dark with orange", &p);
    assert!(hue_gap(p.accent, hex(orange)) < 10.0, "accent {}", p.accent);
    assert!(hue_gap(p.tint, hex(orange)) < 10.0, "tint {}", p.tint);
}

#[test]
fn a_stray_pixel_next_to_a_populated_hue_does_not_count() {
    let orange = [0xff, 0x8c, 0x00];
    // #ff4000 is in the hue bin next to orange's and more vivid.
    let speck = [0xff, 0x40, 0x00];
    let [_, _, h_orange] = hex(orange).oklch();
    let [_, _, h_speck] = hex(speck).oklch();
    assert_eq!(
        hue_bin(h_speck),
        (hue_bin(h_orange) + HUE_BINS - 1) % HUE_BINS
    );
    // Its own bin: one pixel, with half the orange bin's 64 as neighbours.
    let mut counts = [0.0; HUE_BINS];
    counts[hue_bin(h_orange)] = 64.0;
    counts[hue_bin(h_speck)] = 1.0;
    assert!(populated(&counts, hue_bin(h_orange), 1024.0));
    assert!(!populated(&counts, hue_bin(h_speck), 1024.0));
    // On a cover: a dark field, a small orange square and the speck.
    let cover = image(64, 64, |x, y| {
        if (40..56).contains(&x) && (8..24).contains(&y) {
            orange
        } else if (38..40).contains(&x) && (8..10).contains(&y) {
            speck
        } else {
            [0x14, 0x13, 0x16]
        }
    });
    let p = palette(&cover);
    assert_readable("orange with a speck", &p);
    assert!(hue_gap(p.accent, hex(orange)) < 8.0, "accent {}", p.accent);
    // A 1 × 1 cover still counts its one pixel.
    let tiny = [1.0; HUE_BINS];
    assert!(populated(&tiny, 0, 1.0));
}

/// Pink and magenta on both sides of hue 0 (bins 23 and 0): the tint and
/// the accent stay in that hue, never pulled to the far side of the wheel.
#[test]
fn hues_wrap_around_zero() {
    let magenta = [0xf0, 0x30, 0xa0];
    let pink = [0xe0, 0x40, 0x80];
    assert_eq!(hue_bin(hex(magenta).oklch()[2]), HUE_BINS - 1);
    assert_eq!(hue_bin(hex(pink).oklch()[2]), 0);
    let cover = image(64, 64, |x, _| if x < 32 { magenta } else { pink });
    let p = palette(&cover);
    assert!(p.vivid);
    assert_readable("magenta and pink", &p);
    // Between the two (351° and 1°): about 356°.
    let middle = Rgba::from_oklch_in_gamut(0.6, 0.15, 356f64.to_radians());
    assert!(hue_gap(p.tint, middle) < 8.0, "tint {}", p.tint);
    assert!(hue_gap(p.accent, middle) < 8.0, "accent {}", p.accent);
}

#[test]
fn the_tint_follows_the_dominant_hue() {
    let blue = [0x30, 0x60, 0xa0];
    let red = [0xd0, 0x30, 0x30];
    // Three quarters blue, a quarter red.
    let cover = image(64, 64, |x, _| if x < 48 { blue } else { red });
    let p = palette(&cover);
    assert!(p.vivid);
    assert_readable("blue and red", &p);
    assert!(hue_gap(p.tint, hex(blue)) < 10.0, "tint {}", p.tint);
    // The accent is one of the two hues, never a mix of them.
    let gap = hue_gap(p.accent, hex(blue)).min(hue_gap(p.accent, hex(red)));
    assert!(gap < 10.0, "accent {}", p.accent);
}

#[test]
fn dark_and_pale_colours_are_brought_into_the_accent_band() {
    for (name, color) in [
        ("navy", [0x10, 0x18, 0x50]),
        ("pastel", [0xf4, 0xd8, 0xe8]),
        ("olive", [0x50, 0x50, 0x10]),
        ("deep purple", [0x30, 0x08, 0x40]),
    ] {
        let p = palette(&solid(color));
        assert_readable(name, &p);
        if p.vivid {
            assert!(hue_gap(p.accent, hex(color)) < 12.0, "{name}: {}", p.accent);
        }
    }
}

#[test]
fn small_and_odd_covers_work() {
    for cover in [
        image(1, 1, |_, _| [0x20, 0xa0, 0x40]),
        image(3, 7, |x, _| [0x20 * x as u8, 0x80, 0x40]),
        image(300, 17, |x, y| [(x % 256) as u8, (y * 15) as u8, 0x80]),
    ] {
        assert_readable("odd", &palette(&cover));
    }
    assert!(palette(&image(1, 1, |_, _| [0x20, 0xa0, 0x40])).vivid);
}

#[test]
fn the_same_cover_gives_the_same_palette() {
    let cover = image(256, 256, |x, y| {
        [
            (x * 7 % 256) as u8,
            (y * 3 % 256) as u8,
            ((x ^ y) % 256) as u8,
        ]
    });
    let first = palette(&cover);
    assert_readable("noise", &first);
    for _ in 0..3 {
        assert_eq!(palette(&cover), first);
    }
}
