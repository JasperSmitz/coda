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

// ----- washes -----

/// `surface` drawing a solid `color` cover over `under`, washed with
/// `tint` at `alpha` (sRGB compositing, as the webview does).
fn washed(color: [u8; 3], tint: Rgba, under: Rgba, surface: &Surface, alpha: f64) -> Rgba {
    let saturated = saturate(unit(hex(color)), surface.saturate);
    let under = unit(under);
    let tint = unit(tint);
    from_unit(std::array::from_fn(|i| {
        let glow = surface.opacity * saturated[i] + (1.0 - surface.opacity) * under[i];
        alpha * tint[i] + (1.0 - alpha) * glow
    }))
}

fn reads(background: Rgba) -> bool {
    TEXT_LEVELS
        .iter()
        .all(|&(text, min)| contrast(text, background) >= min)
}

/// The text levels keep their contrast at the wash, and the wash is the
/// least that does (or the floor).
fn assert_wash(name: &str, color: [u8; 3]) -> (f64, f64) {
    let p = palette(&solid(color));
    // The page's glow: over the ground, washed with it; the player: its tint.
    for (surface, wash, under, alpha) in [
        (PAGE_GLOW, p.ground, p.ground, p.page_wash),
        (PLAYER, p.tint, BASE, p.player_wash),
    ] {
        assert!(
            (surface.floor..=1.0).contains(&alpha),
            "{name}: {alpha} within the floor"
        );
        let bg = washed(color, wash, under, &surface, alpha);
        assert!(reads(bg), "{name}: wash {alpha} gives {bg}");
        for (text, min) in TEXT_LEVELS {
            assert!(contrast(text, bg) >= min, "{name}: {text} on {bg}");
        }
        // The house accent reads too (≥ 3:1).
        assert!(contrast(Rgba::rgb(0xf0, 0xa8, 0x3a), bg) >= 3.0, "{name}");
        if alpha > surface.floor + 1e-9 {
            let less = washed(color, wash, under, &surface, alpha - 0.01);
            assert!(!reads(less), "{name}: {alpha} is not the least");
        }
    }
    (p.page_wash, p.player_wash)
}

#[test]
fn washes_are_what_each_cover_needs() {
    let (white_page, white_player) = assert_wash("white", [0xff, 0xff, 0xff]);
    let (yellow_page, yellow_player) = assert_wash("yellow", [0xff, 0xff, 0x00]);
    let (black_page, black_player) = assert_wash("black", [0x00, 0x00, 0x00]);
    let (red_page, red_player) = assert_wash("dark red", [0x8b, 0x10, 0x20]);
    let (blue_page, blue_player) = assert_wash("deep blue", [0x1a, 0x23, 0x7e]);
    // Bright covers get a heavy wash; black and dark vivid ones the floor
    // or close to it, so they stay colourful.
    assert!(
        white_page > 0.6 && white_player > 0.6,
        "{white_page} {white_player}"
    );
    assert!(
        yellow_page > 0.6 && yellow_player > 0.6,
        "{yellow_page} {yellow_player}"
    );
    assert_eq!(black_page, PAGE_GLOW.floor);
    assert_eq!(black_player, PLAYER.floor);
    for (name, page, player) in [
        ("red", red_page, red_player),
        ("blue", blue_page, blue_player),
    ] {
        // Over its (lighter) ground the page needs a little more.
        assert!(page <= PAGE_GLOW.floor + 0.15, "{name}: page {page}");
        assert!(player <= PLAYER.floor + 0.15, "{name}: player {player}");
    }
}

#[test]
fn the_brightest_area_sets_the_wash() {
    // A dark cover with a white quarter: washed as a white cover (the
    // quarter is many times the blur's σ).
    let cover = image(64, 64, |x, y| {
        if x < 32 && y < 32 {
            [0xff, 0xff, 0xff]
        } else {
            [0x20, 0x10, 0x30]
        }
    });
    let p = palette(&cover);
    let white = palette(&solid([0xff, 0xff, 0xff]));
    assert!(
        p.page_wash >= white.page_wash - 0.05,
        "{} vs {}",
        p.page_wash,
        white.page_wash
    );
    // A white speck a fraction of the page blur's σ wide is blurred away.
    let speck = image(64, 64, |x, y| {
        if (30..32).contains(&x) && (30..32).contains(&y) {
            [0xff, 0xff, 0xff]
        } else {
            [0x20, 0x10, 0x30]
        }
    });
    assert!(
        palette(&speck).page_wash < 0.4,
        "{}",
        palette(&speck).page_wash
    );
}

// ----- swatches -----

#[test]
fn greyscale_covers_have_no_swatches() {
    assert!(palette(&solid([0x80, 0x80, 0x80])).swatches.is_empty());
    assert!(palette(&solid([0xff, 0xff, 0xff])).swatches.is_empty());
}

#[test]
fn swatches_are_the_hues_by_weight_vivid_and_distinct() {
    let blue = [0x30, 0x60, 0xa0];
    let red = [0xd0, 0x30, 0x30];
    let green = [0x30, 0xa0, 0x40];
    // Half blue, a third red, a sixth green.
    let cover = image(60, 60, |x, _| match x {
        0..30 => blue,
        30..50 => red,
        _ => green,
    });
    let p = palette(&cover);
    assert_eq!(p.swatches.len(), 3, "{:?}", p.swatches);
    // Heaviest first: share × chroma² × presence.
    let mut order = [(blue, 0.5), (red, 1.0 / 3.0), (green, 1.0 / 6.0)];
    order.sort_by(|a, b| {
        let w = |(c, share): &([u8; 3], f64)| {
            let [l, c, _] = hex(*c).oklch();
            share * c * c * presence(l)
        };
        w(b).total_cmp(&w(a))
    });
    for (swatch, (color, _)) in p.swatches.iter().zip(order) {
        assert!(
            hue_gap(*swatch, hex(color)) < 10.0,
            "{swatch} for {:?}",
            color
        );
        let [l, c, _] = swatch.oklch();
        assert!(
            (SWATCH_L.0 - 0.01..=SWATCH_L.1 + 0.01).contains(&l),
            "{swatch}: L {l}"
        );
        assert!(c >= 0.1, "{swatch}: vivid, C {c}");
        assert_eq!(swatch.a, 255);
    }
    // One hue spread over neighbouring bins gives one swatch.
    let orange = image(64, 64, |x, _| [0xff, 0x70 + x as u8, 0x10]);
    let p = palette(&orange);
    assert_eq!(p.swatches.len(), 1, "{:?}", p.swatches);
    // Never more than four.
    let rainbow = image(64, 64, |x, _| {
        let h = f64::from(x) / 64.0 * 360.0;
        let c = Rgba::from_oklch_in_gamut(0.65, 0.2, h.to_radians());
        [c.r, c.g, c.b]
    });
    let p = palette(&rainbow);
    assert_eq!(p.swatches.len(), MAX_SWATCHES);
    for (i, a) in p.swatches.iter().enumerate() {
        for b in &p.swatches[i + 1..] {
            assert!(hue_gap(*a, *b) >= SWATCH_MIN_GAP - 1.0, "{a} {b}");
        }
    }
}

/// Grounds of every kind: every hue at the most and the least chroma, and
/// greyscale ones (every cast).
fn all_grounds() -> Vec<Rgba> {
    let mut grounds = Vec::new();
    for hue in (0..360).step_by(3) {
        let h = f64::from(hue).to_radians();
        for chroma in [GROUND_C.0, GROUND_C.1, GREY_GROUND_C] {
            grounds.push(ground_of(h, chroma));
        }
    }
    grounds
}

/// Every swatch (the lightest the band allows, every hue, as much chroma
/// as sRGB has) at [`SWATCH_UNDER_TEXT`] over a card on the base, and at
/// [`SWATCH_UNDER_TEXT_ON_GROUND`] over a card on any ground, keeps the
/// text levels' contrast; a little more would not.
#[test]
fn swatches_under_text_keep_the_contrast() {
    let grounds = all_grounds();
    let over_swatch = |alpha: f64, hue: u32, ground: Rgba| {
        let card = over(CARD_SURFACE, ground);
        let swatch =
            Rgba::from_oklch_in_gamut(SWATCH_L.1, SWATCH_MAX_C, f64::from(hue).to_radians());
        over(swatch.with_alpha(alpha), card)
    };
    let all_read = |alpha: f64, grounds: &[Rgba]| {
        grounds.iter().all(|&g| {
            (0..360)
                .step_by(2)
                .all(|hue| reads(over_swatch(alpha, hue, g)))
        })
    };
    // On the base (Library's genre tiles, pages without a subject).
    assert!(all_read(SWATCH_UNDER_TEXT, &[BASE]));
    assert!(!all_read(SWATCH_UNDER_TEXT + 0.02, &[BASE]));
    // On any ground (Home's mood cards).
    assert!(all_read(SWATCH_UNDER_TEXT_ON_GROUND, &grounds));
    assert!(!all_read(SWATCH_UNDER_TEXT_ON_GROUND + 0.02, &grounds));
}

// ----- the ground and the glass -----

#[test]
fn grounds_are_coloured_deep_and_readable() {
    for ground in all_grounds() {
        assert_eq!(ground.a, 255);
        assert!(reads(ground), "{ground}");
        assert!(reads(over(GROUND_VEIL, ground)), "{ground} under the veil");
        let [l, _, _] = ground.oklch();
        assert!(l <= GROUND_L_TOP + 0.005, "{ground}: L {l}");
    }
    // Covers of many hues, and white, black, yellow and grey ones.
    let mut covers: Vec<(String, [u8; 3])> = (0..12)
        .map(|i| {
            let c = Rgba::from_oklch_in_gamut(0.6, 0.18, f64::from(i * 30).to_radians());
            (format!("hue {}", i * 30), [c.r, c.g, c.b])
        })
        .collect();
    for (name, c) in [
        ("white", [0xff, 0xff, 0xff]),
        ("black", [0x00, 0x00, 0x00]),
        ("yellow", [0xff, 0xff, 0x00]),
        ("grey", [0x80, 0x80, 0x80]),
    ] {
        covers.push((name.into(), c));
    }
    for (name, color) in covers {
        let p = palette(&solid(color));
        let g = p.ground;
        let [l, c, _] = g.oklch();
        assert!(reads(over(GROUND_VEIL, g)), "{name}: {g}");
        // Deep but clearly coloured: about the band (yellows and greys a
        // little under it), lighter than the tint and the base.
        assert!(
            (0.24..=GROUND_L_TOP + 0.005).contains(&l),
            "{name}: {g} L {l}"
        );
        assert!(l > p.tint.oklch()[0] && l > BASE.oklch()[0], "{name}");
        if p.vivid {
            // (cyans are gamut-limited this deep: ~0.045)
            assert!(c >= 0.04, "{name}: {g} C {c}");
            assert!(hue_gap(g, p.tint) < 10.0, "{name}: {g} vs {}", p.tint);
        } else {
            // Near neutral, but not a flat grey.
            assert!((0.01..=0.035).contains(&c), "{name}: {g} C {c}");
        }
        // The glass: the ground, darkened, readable over white.
        let glass = Rgba {
            a: GLASS_ALPHA,
            ..p.glass
        };
        assert!(
            reads(over(glass, color::WHITE)),
            "{name}: glass {}",
            p.glass
        );
        // The stronger glass (`--glass-strong`: sticky tool rows)
        // with a control's surface on it: text-2 (an inactive section).
        let strong = over(
            Rgba {
                a: GLASS_STRONG_ALPHA,
                ..p.glass
            },
            color::WHITE,
        );
        let control = over(CARD_SURFACE, strong);
        assert!(contrast(TEXT_2, control) >= 4.5, "{name}: {control}");
        assert!(p.glass.oklch()[0] <= g.oklch()[0], "{name}");
        if p.vivid {
            assert!(
                hue_gap(p.glass, g) < 12.0,
                "{name}: glass {} on {g}",
                p.glass
            );
        }
    }
}

/// erf, Abramowitz and Stegun 7.1.26 (|error| < 1.5e-7).
fn erf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    y.copysign(x)
}

/// A dark cover with a small white square (5/32 of its side): at the
/// square's centre a continuous Gaussian of the surface's σ keeps
/// `erf(side / (2√2 σ))` per axis of the white; text keeps its contrast
/// there at the computed wash (the review's case: a box average over a
/// wide window would have washed it too little).
#[test]
fn a_small_white_square_keeps_the_text_readable() {
    let dark = [0x18, 0x10, 0x28];
    let cover = image(32, 32, |x, y| {
        if (13..18).contains(&x) && (13..18).contains(&y) {
            [0xff, 0xff, 0xff]
        } else {
            dark
        }
    });
    let p = palette(&cover);
    let side = 5.0 / 32.0;
    for (name, surface, alpha) in [
        ("page", PAGE_GLOW, p.page_wash),
        ("player", PLAYER, p.player_wash),
    ] {
        let share = |sigma: f64| erf(side / (2.0 * std::f64::consts::SQRT_2 * sigma));
        let white = share(surface.sigma.0) * share(surface.sigma.1);
        let peak: [f64; 3] = std::array::from_fn(|i| white + (1.0 - white) * unit(hex(dark))[i]);
        let saturated = saturate(peak, surface.saturate);
        let (wash, under) = if name == "page" {
            (p.ground, p.ground)
        } else {
            (p.tint, BASE)
        };
        let under = unit(under);
        let tint = unit(wash);
        let bg = from_unit(std::array::from_fn(|i| {
            let glow = surface.opacity * saturated[i] + (1.0 - surface.opacity) * under[i];
            alpha * tint[i] + (1.0 - alpha) * glow
        }));
        assert!(
            contrast(TEXT_2, bg) >= 4.5,
            "{name}: text-2 on {bg} at {alpha} (white {white:.2})"
        );
        assert!(reads(bg), "{name}: {bg} at {alpha}");
    }
}

// ----- the ground field (2.0 R3) -----

/// A cover like Muse's *The Resistance*: a large dull dark-orange field
/// (60%) with a ring of four vivid hues (10% each).
fn ring_cover() -> CoverImage {
    let road = [0x5a, 0x34, 0x1c];
    let ring = [
        [0x20, 0x90, 0xe0],
        [0xe0, 0x30, 0x90],
        [0x30, 0xc0, 0x60],
        [0xe0, 0xc0, 0x20],
    ];
    image(80, 80, |x, y| {
        if y < 48 {
            road
        } else {
            ring[(x / 20) as usize]
        }
    })
}

#[test]
fn vivid_hues_outweigh_a_large_dull_region() {
    let p = palette(&ring_cover());
    // The ring's colours lead the swatches; the dull road comes after them.
    let road = hex([0x5a, 0x34, 0x1c]);
    assert!(p.swatches.len() >= 3, "{:?}", p.swatches);
    assert!(
        hue_gap(p.swatches[0], road) > 20.0,
        "first {}",
        p.swatches[0]
    );
    // Three ground stops, of different hues.
    assert_eq!(p.grounds.len(), MAX_GROUNDS, "{:?}", p.grounds);
    assert_eq!(p.ground, p.grounds[0]);
    for (i, a) in p.grounds.iter().enumerate() {
        for b in &p.grounds[i + 1..] {
            assert!(hue_gap(*a, *b) >= 30.0, "{a} {b}");
        }
    }
}

#[test]
fn single_colour_and_greyscale_covers_have_one_stop() {
    for (name, color, vivid) in [
        ("red", [0xd0, 0x30, 0x30], true),
        ("blue", [0x20, 0x40, 0xe0], true),
        ("white", [0xff, 0xff, 0xff], false),
        ("grey", [0x80, 0x80, 0x80], false),
    ] {
        let p = palette(&solid(color));
        assert_eq!(p.grounds.len(), 1, "{name}: {:?}", p.grounds);
        assert_eq!(p.ground, p.grounds[0], "{name}");
        assert_eq!(p.vivid, vivid, "{name}");
    }
}

/// The sRGB mix of `a` and `b` at `t` (as a CSS gradient between them).
fn mix(a: Rgba, b: Rgba, t: f64) -> Rgba {
    let (a, b) = (unit(a), unit(b));
    from_unit(std::array::from_fn(|i| (1.0 - t) * a[i] + t * b[i]))
}

/// Every point of the field mixes text-safe stops: checked at the stops,
/// the pairwise mixes and the three-way middle, under the veil too.
#[test]
fn every_point_of_the_ground_field_is_readable() {
    let mut covers = vec![ring_cover()];
    for i in 0..12 {
        let a = Rgba::from_oklch_in_gamut(0.6, 0.18, f64::from(i * 30).to_radians());
        let b = Rgba::from_oklch_in_gamut(0.7, 0.15, f64::from(i * 30 + 120).to_radians());
        let c = Rgba::from_oklch_in_gamut(0.5, 0.2, f64::from(i * 30 + 220).to_radians());
        covers.push(image(60, 60, move |x, _| {
            let p = match x {
                0..25 => a,
                25..45 => b,
                _ => c,
            };
            [p.r, p.g, p.b]
        }));
    }
    for cover in covers {
        let g = palette(&cover).grounds;
        let mut points = g.clone();
        for (i, a) in g.iter().enumerate() {
            for b in &g[i + 1..] {
                for t in [0.25, 0.5, 0.75] {
                    points.push(mix(*a, *b, t));
                }
            }
        }
        if g.len() == 3 {
            points.push(mix(mix(g[0], g[1], 0.5), g[2], 1.0 / 3.0));
        }
        for p in points {
            assert!(reads(p), "{p} of {g:?}");
            assert!(reads(over(GROUND_VEIL, p)), "{p} under the veil, of {g:?}");
            // The bloom's faded edge, of any colour, over it.
            for c in [
                color::WHITE,
                Rgba::rgb(0xff, 0xff, 0x00),
                Rgba::rgb(0x00, 0xff, 0xff),
            ] {
                let bloom = Rgba {
                    a: BLOOM_TEXT_ALPHA,
                    ..c
                };
                assert!(reads(over(bloom, p)), "bloom {c} on {p}");
            }
        }
    }
}
