//! From a theme's few colours to every UI token (DESIGN.md "Theming").
//!
//! Derived tokens mix the background and foreground in OKLab (the maths of
//! CSS `color-mix(in oklab, …)`), done here so the colour editor can show
//! each computed value and the contrast guard can run once per load. A
//! theme file or the editor can override any token (`coda-<token>`).
//!
//! The contrast guard nudges derived colours (and the accent, danger and
//! text of themes the user did not write) in OKLCH lightness until they
//! pass. A colour the user set explicitly is never changed: it gets a
//! contrast warning instead.

use std::collections::BTreeSet;

use serde::Serialize;

use super::color::{self, BLACK, Rgba, WHITE, contrast, ensure_contrast, text_on};
use super::parse::{AccentSpec, ThemeFile};

/// Base colours: the theme's own (editable in the colour editor; `bg`,
/// `fg` and `palette-N` are the Ghostty keys, `accent`/`danger`/
/// `selection` may be overridden with `coda-*`).
pub const BASE: [&str; 5] = ["bg", "fg", "accent", "danger", "selection"];

/// Tokens computed from the base colours. Each may be overridden.
pub const DERIVED: [&str; 32] = [
    "surface-1",
    "surface-2",
    "surface-3",
    "border",
    "text",
    "text-2",
    "text-3",
    "accent-text",
    "accent-soft",
    "danger-text",
    "danger-soft",
    "success",
    "hover",
    "selected",
    "selection-text",
    "shadow-color",
    "cover-shadow-color",
    "shadow-strong",
    "backdrop",
    "np-bg",
    "np-text",
    "np-scrim",
    "on-cover",
    "on-cover-bg",
    "on-cover-bg-hover",
    "cover-1",
    "cover-2",
    "cover-3",
    "cover-4",
    "cover-5",
    "cover-6",
    "viz",
];

/// Every `coda-<token>` key a theme file may carry (`coda-accent` aside):
/// the derived tokens, plus `danger` and `selection`.
pub const OVERRIDABLE: [&str; 34] = [
    "surface-1",
    "surface-2",
    "surface-3",
    "border",
    "text",
    "text-2",
    "text-3",
    "accent-text",
    "accent-soft",
    "danger-text",
    "danger-soft",
    "success",
    "hover",
    "selected",
    "selection-text",
    "shadow-color",
    "cover-shadow-color",
    "shadow-strong",
    "backdrop",
    "np-bg",
    "np-text",
    "np-scrim",
    "on-cover",
    "on-cover-bg",
    "on-cover-bg-hover",
    "cover-1",
    "cover-2",
    "cover-3",
    "cover-4",
    "cover-5",
    "cover-6",
    "viz",
    "danger",
    "selection",
];

/// Light or dark, from the background's lightness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Scheme {
    Light,
    Dark,
}

impl Scheme {
    pub fn of(background: Rgba) -> Self {
        if background.lightness() > 0.6 {
            Scheme::Light
        } else {
            Scheme::Dark
        }
    }
}

/// A contrast check a token fails.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContrastWarning {
    /// Rounded to one decimal.
    pub ratio: f64,
    pub min: f64,
    /// The token it was measured against.
    pub against: String,
}

/// One token's value.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Token {
    /// The CSS custom property without `--` (`bg`, `palette-3`, `text-2`).
    pub key: String,
    /// `#rrggbb` or `#rrggbbaa`.
    pub value: String,
    /// Set by the theme file's `coda-*` key, the user or the editor
    /// rather than computed.
    pub overridden: bool,
    pub contrast: Option<ContrastWarning>,
}

/// A theme turned into tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub scheme: Scheme,
    /// Base keys first (`bg` … `selection`, `palette-0..15`), then
    /// [`DERIVED`] in order.
    pub tokens: Vec<Token>,
}

impl Derived {
    pub fn get(&self, key: &str) -> Option<Rgba> {
        self.tokens
            .iter()
            .find(|t| t.key == key)
            .and_then(|t| Rgba::parse(&t.value))
    }
}

/// Used where a theme has no palette entry (Coda's own palette).
pub const FALLBACK_PALETTE: [Rgba; 16] = [
    Rgba::rgb(0x1d, 0x1f, 0x23),
    Rgba::rgb(0xe5, 0x67, 0x5f),
    Rgba::rgb(0x5c, 0xb8, 0x5c),
    Rgba::rgb(0xf0, 0xa8, 0x3a),
    Rgba::rgb(0x5b, 0x9b, 0xd5),
    Rgba::rgb(0xb4, 0x82, 0xd6),
    Rgba::rgb(0x4f, 0xb8, 0xb0),
    Rgba::rgb(0xa2, 0xa5, 0xad),
    Rgba::rgb(0x6d, 0x71, 0x7a),
    Rgba::rgb(0xf0, 0x7d, 0x75),
    Rgba::rgb(0x7c, 0xcf, 0x7c),
    Rgba::rgb(0xf5, 0xc0, 0x6a),
    Rgba::rgb(0x7f, 0xb4, 0xe6),
    Rgba::rgb(0xc9, 0xa2, 0xe6),
    Rgba::rgb(0x76, 0xcf, 0xc8),
    Rgba::rgb(0xe9, 0xe9, 0xec),
];

/// Accent text and body text need this against their backgrounds.
const TEXT_MIN: f64 = 4.5;
/// Accent, danger and other non-text marks need this.
const MARK_MIN: f64 = 3.0;

/// Derives every token of `theme`. `explicit` names the tokens the user
/// set (by editor key: `accent`, `surface-2`, `palette-3`, …): those are
/// never adjusted, only warned about.
pub fn derive(theme: &ThemeFile, explicit: &BTreeSet<String>) -> Derived {
    let bg = theme.background;
    let fg = theme.foreground;
    let scheme = Scheme::of(bg);
    let dark = scheme == Scheme::Dark;
    let palette: [Rgba; 16] =
        std::array::from_fn(|i| theme.palette[i].unwrap_or(FALLBACK_PALETTE[i]));
    let o = |key: &str| theme.overrides.get(key).copied();
    let is_explicit = |key: &str| explicit.contains(key);
    // A computed value is guarded; an overridden one is guarded only if it
    // came from a theme the user did not write.
    let guard = |key: &str, value: Rgba, against: Rgba, min: f64| {
        if is_explicit(key) {
            value
        } else {
            ensure_contrast(value, against, min)
        }
    };

    let mut tokens: Vec<(String, Rgba, bool)> = Vec::new();
    let mut put = |key: &str, value: Rgba, overridden: bool| {
        tokens.push((key.to_owned(), value, overridden));
    };

    // Surfaces: a few steps from the background toward the text (light
    // themes keep panels a touch whiter than the background).
    let surface = |key: &str, t: f64| o(key).unwrap_or_else(|| bg.mix(fg, t));
    let surface_1 = surface("surface-1", 0.04);
    let surface_2 = o("surface-2").unwrap_or_else(|| {
        if dark {
            bg.mix(fg, 0.08)
        } else {
            bg.mix(WHITE, 0.7)
        }
    });
    let surface_3 = surface("surface-3", if dark { 0.14 } else { 0.09 });
    let border = surface("border", if dark { 0.15 } else { 0.12 });

    let mut text = o("text").unwrap_or(fg);
    if !is_explicit("text") {
        // One direction for every surface: the background's.
        let lighten = color::lighter_reads_better(bg);
        for against in [bg, surface_1, surface_2, surface_3] {
            text = color::ensure_contrast_toward(text, against, TEXT_MIN, lighten);
        }
    }
    let text_2 = guard(
        "text-2",
        o("text-2").unwrap_or_else(|| text.mix(bg, 0.35)),
        bg,
        TEXT_MIN,
    );
    let text_3 = guard(
        "text-3",
        o("text-3").unwrap_or_else(|| text.mix(bg, 0.55)),
        bg,
        MARK_MIN,
    );

    let accent_from_file = match theme.accent {
        Some(AccentSpec::Color(c)) => Some(c),
        Some(AccentSpec::Palette(n)) => Some(palette[n]),
        None => None,
    };
    // Palette 3 (yellow), else 4 or 5 if 3 is too faint on this background.
    let accent = accent_from_file.unwrap_or_else(|| {
        [3, 4, 5]
            .into_iter()
            .map(|n| palette[n])
            .find(|c| contrast(*c, bg) >= MARK_MIN)
            .unwrap_or(palette[3])
    });
    let accent = guard("accent", accent, bg, MARK_MIN);
    let accent_text = o("accent-text").unwrap_or_else(|| text_on(accent));
    let danger = guard("danger", o("danger").unwrap_or(palette[1]), bg, MARK_MIN);
    let danger_text = o("danger-text").unwrap_or_else(|| text_on(danger));
    let selection = o("selection")
        .or(theme.selection_background)
        .unwrap_or_else(|| accent.with_alpha(0.3));
    let selection_text = guard(
        "selection-text",
        o("selection-text")
            .or(theme.selection_foreground)
            .unwrap_or(text),
        selection.over(bg),
        TEXT_MIN,
    );

    put("bg", bg, is_explicit("bg"));
    put("fg", fg, is_explicit("fg"));
    put(
        "accent",
        accent,
        accent_from_file.is_some() || is_explicit("accent"),
    );
    put("danger", danger, o("danger").is_some());
    put(
        "selection",
        selection,
        o("selection").is_some() || theme.selection_background.is_some(),
    );
    for (n, color) in palette.iter().enumerate() {
        put(&format!("palette-{n}"), *color, theme.palette[n].is_some());
    }

    let alpha = |color: Rgba, dark_alpha: f64, light_alpha: f64| {
        color.with_alpha(if dark { dark_alpha } else { light_alpha })
    };
    let np_bg = o("np-bg").unwrap_or_else(|| {
        if dark {
            bg.mix(BLACK, 0.3)
        } else {
            fg.mix(BLACK, 0.45)
        }
    });
    // The Now Playing backdrop stays dark whatever the theme's colours.
    let np_bg = if is_explicit("np-bg") {
        np_bg
    } else {
        color::ensure_contrast_toward(np_bg, WHITE, 12.0, false)
    };
    let np_text = o("np-text").unwrap_or_else(|| {
        if dark {
            text.mix(WHITE, 0.3)
        } else {
            bg.mix(WHITE, 0.3)
        }
    });
    let np_text = guard("np-text", np_text, np_bg, TEXT_MIN);
    let on_cover = o("on-cover").unwrap_or(WHITE);

    let derived: Vec<(&str, Rgba)> = vec![
        ("surface-1", surface_1),
        ("surface-2", surface_2),
        ("surface-3", surface_3),
        ("border", border),
        ("text", text),
        ("text-2", text_2),
        ("text-3", text_3),
        ("accent-text", accent_text),
        (
            "accent-soft",
            o("accent-soft").unwrap_or_else(|| alpha(accent, 0.14, 0.13)),
        ),
        ("danger-text", danger_text),
        (
            "danger-soft",
            o("danger-soft").unwrap_or_else(|| alpha(danger, 0.12, 0.1)),
        ),
        (
            "success",
            guard("success", o("success").unwrap_or(palette[2]), bg, MARK_MIN),
        ),
        (
            "hover",
            o("hover").unwrap_or_else(|| alpha(text, 0.05, 0.04)),
        ),
        (
            "selected",
            o("selected").unwrap_or_else(|| alpha(text, 0.08, 0.07)),
        ),
        ("selection-text", selection_text),
        (
            "shadow-color",
            o("shadow-color").unwrap_or_else(|| alpha(BLACK, 0.45, 0.16)),
        ),
        (
            "cover-shadow-color",
            o("cover-shadow-color").unwrap_or_else(|| alpha(BLACK, 0.4, 0.18)),
        ),
        (
            "shadow-strong",
            o("shadow-strong").unwrap_or_else(|| alpha(BLACK, 0.55, 0.3)),
        ),
        (
            "backdrop",
            o("backdrop").unwrap_or_else(|| alpha(BLACK, 0.45, 0.3)),
        ),
        ("np-bg", np_bg),
        ("np-text", np_text),
        (
            "np-scrim",
            o("np-scrim").unwrap_or_else(|| np_bg.with_alpha(0.55)),
        ),
        ("on-cover", on_cover),
        (
            "on-cover-bg",
            o("on-cover-bg").unwrap_or_else(|| BLACK.with_alpha(0.55)),
        ),
        (
            "on-cover-bg-hover",
            o("on-cover-bg-hover").unwrap_or_else(|| BLACK.with_alpha(0.75)),
        ),
    ];
    let mut derived = derived;
    // Cover placeholders: palette 1–6, darkened so the initials read.
    let covers: Vec<(String, Rgba)> = (1..=6)
        .map(|n| {
            let key = format!("cover-{n}");
            let value = o(&key).unwrap_or_else(|| palette[n].mix(BLACK, 0.45));
            let value = if is_explicit(&key) {
                value
            } else {
                ensure_contrast(value, on_cover, MARK_MIN)
            };
            (key, value)
        })
        .collect();
    for (key, value) in &covers {
        derived.push((key.as_str(), *value));
    }
    derived.push(("viz", o("viz").unwrap_or(accent)));

    for (key, value) in derived {
        put(
            key,
            value,
            theme.overrides.contains_key(key) || is_explicit(key),
        );
    }

    let lookup = |key: &str| tokens.iter().find(|(k, _, _)| k == key).map(|(_, v, _)| *v);
    let warning = |key: &str| -> Option<ContrastWarning> {
        let (against, min) = CHECKS
            .iter()
            .find(|(k, _, _)| *k == key)
            .map(|(_, a, m)| (*a, *m))?;
        let value = lookup(key)?;
        let base = lookup(against)?;
        // Translucent colours are measured as drawn over the background.
        let base = base.over(bg);
        let ratio = contrast(value.over(base), base);
        (ratio + 1e-9 < min).then(|| ContrastWarning {
            ratio: (ratio * 10.0).floor() / 10.0,
            min,
            against: against.to_owned(),
        })
    };
    let tokens = tokens
        .iter()
        .map(|(key, value, overridden)| Token {
            key: key.clone(),
            value: value.to_string(),
            overridden: *overridden,
            contrast: warning(key),
        })
        .collect();
    Derived { scheme, tokens }
}

/// Which contrast each token needs, against which token.
const CHECKS: &[(&str, &str, f64)] = &[
    ("fg", "bg", TEXT_MIN),
    ("text", "bg", TEXT_MIN),
    ("text-2", "bg", TEXT_MIN),
    ("text-3", "bg", MARK_MIN),
    ("accent", "bg", MARK_MIN),
    ("accent-text", "accent", TEXT_MIN),
    ("danger", "bg", MARK_MIN),
    ("danger-text", "danger", TEXT_MIN),
    ("success", "bg", MARK_MIN),
    ("selection-text", "selection", TEXT_MIN),
    ("np-text", "np-bg", TEXT_MIN),
    ("cover-1", "on-cover", MARK_MIN),
    ("cover-2", "on-cover", MARK_MIN),
    ("cover-3", "on-cover", MARK_MIN),
    ("cover-4", "on-cover", MARK_MIN),
    ("cover-5", "on-cover", MARK_MIN),
    ("cover-6", "on-cover", MARK_MIN),
];

/// Every editor key: the base colours, the palette, then [`DERIVED`].
pub fn editable_keys() -> impl Iterator<Item = String> {
    BASE.iter()
        .map(|k| (*k).to_owned())
        .chain((0..16).map(|n| format!("palette-{n}")))
        .chain(DERIVED.iter().map(|k| (*k).to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::parse::parse;

    fn theme(text: &str) -> ThemeFile {
        parse(text).unwrap()
    }

    fn none() -> BTreeSet<String> {
        BTreeSet::new()
    }

    #[test]
    fn every_token_is_present_once() {
        let d = derive(&theme(include_str!("../../themes/Kanagawabones")), &none());
        let keys: Vec<&str> = d.tokens.iter().map(|t| t.key.as_str()).collect();
        let want: Vec<String> = editable_keys().collect();
        assert_eq!(keys.len(), want.len());
        for key in &want {
            assert_eq!(keys.iter().filter(|k| **k == key).count(), 1, "{key}");
        }
    }

    #[test]
    fn detects_light_and_dark() {
        let latte = derive(
            &theme(include_str!("../../themes/Catppuccin Latte")),
            &none(),
        );
        let mocha = derive(
            &theme(include_str!("../../themes/Catppuccin Mocha")),
            &none(),
        );
        assert_eq!(latte.scheme, Scheme::Light);
        assert_eq!(mocha.scheme, Scheme::Dark);
    }

    #[test]
    fn accent_defaults_to_palette_3_then_4_or_5() {
        let d = derive(&theme(include_str!("../../themes/Kanagawabones")), &none());
        assert_eq!(d.get("accent"), Some(Rgba::rgb(0xe5, 0xc2, 0x83)));
        // Palette 3 too faint on white: palette 4 is used.
        let d = derive(
            &theme(
                "background = #ffffff\nforeground = #000000\npalette = 3=#ffff00\npalette = 4=#1e66f5\n",
            ),
            &none(),
        );
        assert_eq!(d.get("accent"), Some(Rgba::rgb(0x1e, 0x66, 0xf5)));
    }

    #[test]
    fn faint_accent_is_adjusted_unless_explicit() {
        let file = theme("background = #ffffff\nforeground = #000000\ncoda-accent = #fff0c0\n");
        let bg = Rgba::rgb(255, 255, 255);
        let d = derive(&file, &none());
        assert!(contrast(d.get("accent").unwrap(), bg) >= 3.0);
        assert!(
            d.tokens
                .iter()
                .find(|t| t.key == "accent")
                .unwrap()
                .contrast
                .is_none()
        );
        // The user chose it: kept, with a warning.
        let explicit = BTreeSet::from(["accent".to_owned()]);
        let d = derive(&file, &explicit);
        assert_eq!(d.get("accent"), Some(Rgba::rgb(0xff, 0xf0, 0xc0)));
        let warning = d
            .tokens
            .iter()
            .find(|t| t.key == "accent")
            .unwrap()
            .contrast
            .clone();
        let warning = warning.unwrap();
        assert_eq!(warning.min, 3.0);
        assert_eq!(warning.against, "bg");
        assert!(warning.ratio < 3.0);
    }

    #[test]
    fn body_text_is_nudged_to_pass_on_every_surface() {
        let file = theme("background = #202020\nforeground = #5a5a5a\n");
        let d = derive(&file, &none());
        let text = d.get("text").unwrap();
        for surface in ["bg", "surface-1", "surface-2", "surface-3"] {
            assert!(contrast(text, d.get(surface).unwrap()) >= 4.5, "{surface}");
        }
        // The theme's own foreground is kept as `fg`.
        assert_eq!(d.get("fg"), Some(Rgba::rgb(0x5a, 0x5a, 0x5a)));
    }

    #[test]
    fn overrides_win_and_are_marked() {
        let file = theme(
            "background = #000000\nforeground = #ffffff\ncoda-surface-2 = #123456\ncoda-hover = #ffffff0d\n",
        );
        let d = derive(&file, &none());
        let s2 = d.tokens.iter().find(|t| t.key == "surface-2").unwrap();
        assert_eq!((s2.value.as_str(), s2.overridden), ("#123456", true));
        assert_eq!(
            d.tokens.iter().find(|t| t.key == "hover").unwrap().value,
            "#ffffff0d"
        );
        assert!(
            !d.tokens
                .iter()
                .find(|t| t.key == "surface-1")
                .unwrap()
                .overridden
        );
    }

    #[test]
    fn danger_and_success_come_from_the_palette() {
        let d = derive(
            &theme(include_str!("../../themes/Catppuccin Mocha")),
            &none(),
        );
        assert_eq!(d.get("danger"), Some(Rgba::rgb(0xf3, 0x8b, 0xa8)));
        assert_eq!(d.get("success"), Some(Rgba::rgb(0xa6, 0xe3, 0xa1)));
        assert_eq!(d.get("accent"), Some(Rgba::rgb(0xcb, 0xa6, 0xf7)));
    }

    #[test]
    fn built_in_coda_keeps_the_current_look() {
        let d = derive(&theme(include_str!("../../themes/Coda")), &none());
        for (key, value) in [
            ("bg", "#0f1012"),
            ("surface-1", "#16171a"),
            ("surface-2", "#1d1f23"),
            ("surface-3", "#26292e"),
            ("border", "#2a2d33"),
            ("text", "#e9e9ec"),
            ("text-2", "#a2a5ad"),
            ("text-3", "#6d717a"),
            ("accent", "#f0a83a"),
            ("accent-text", "#1a1206"),
            ("danger", "#e5675f"),
        ] {
            assert_eq!(
                d.tokens.iter().find(|t| t.key == key).unwrap().value,
                value,
                "{key}"
            );
        }
        let light = derive(&theme(include_str!("../../themes/Coda Light")), &none());
        assert_eq!(light.scheme, Scheme::Light);
        assert_eq!(light.get("accent"), Some(Rgba::rgb(0xc9, 0x7d, 0x0e)));
        assert_eq!(light.get("surface-2"), Some(Rgba::rgb(255, 255, 255)));
    }

    #[test]
    fn built_ins_pass_the_contrast_checks() {
        for (name, text) in super::super::sources::BUILT_IN {
            let d = derive(&theme(text), &none());
            for t in &d.tokens {
                assert!(t.contrast.is_none(), "{name}: {} {:?}", t.key, t.contrast);
            }
        }
    }
}
