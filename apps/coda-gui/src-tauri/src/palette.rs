//! The app's fixed colours (DESIGN.md "Colour", ADR 0009): the base
//! behind everything, the house accent and the curated accents offered in
//! Preferences. The colour maths ([`color`]) and the covers' palettes
//! (the music colours, served by `cover::palettes`) are in
//! `coda_core::cover::palette`.
//!
//! The frontend's token layer (`src/styles/tokens.css`) holds the same
//! base and house accent; a test keeps the two in step.

pub use coda_core::cover::palette::color;

use serde::Serialize;

use self::color::Rgba;

/// The base colour (`--bg` in `tokens.css`, ~`hsl(256 14% 7%)`): the
/// native window and webview background, so nothing else shows before
/// the first paint or while resizing. Core's palettes compute accent
/// contrast on the same constant.
pub const BASE: Rgba = coda_core::cover::palette::BASE;

/// The house accent, Coda amber (`--accent` in `tokens.css`).
pub const HOUSE_ACCENT: Rgba = Rgba::rgb(0xf0, 0xa8, 0x3a);

/// One curated accent in Preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Swatch {
    pub name: &'static str,
    /// `"house"` for the house accent, else `#rrggbb`: what choosing it
    /// saves.
    pub value: &'static str,
    /// `#rrggbb`, to draw it.
    pub color: &'static str,
}

/// The curated accents, in the order Preferences shows them. The first
/// is the house accent.
pub const SWATCHES: [Swatch; 6] = [
    Swatch {
        name: "Amber",
        value: "house",
        color: "#f0a83a",
    },
    Swatch {
        name: "Rose",
        value: "#ff7d8e",
        color: "#ff7d8e",
    },
    Swatch {
        name: "Mint",
        value: "#7fd8c4",
        color: "#7fd8c4",
    },
    Swatch {
        name: "Periwinkle",
        value: "#8ea9ff",
        color: "#8ea9ff",
    },
    Swatch {
        name: "Lilac",
        value: "#c99cff",
        color: "#c99cff",
    },
    Swatch {
        name: "Bone",
        value: "#e9e3d2",
        color: "#e9e3d2",
    },
];

#[cfg(test)]
mod tests {
    use super::color::{contrast, text_on};
    use super::*;

    /// A custom property's value in `tokens.css`'s `:root` block.
    fn token(css: &str, name: &str) -> String {
        let needle = format!("--{name}:");
        let start = css
            .find(&needle)
            .unwrap_or_else(|| panic!("--{name} in tokens.css"));
        let rest = &css[start + needle.len()..];
        rest[..rest.find(';').unwrap()].trim().to_owned()
    }

    #[test]
    fn tokens_css_matches_the_base_and_the_house_accent() {
        let css = include_str!("../../src/styles/tokens.css");
        assert_eq!(token(css, "bg"), BASE.to_string());
        assert_eq!(token(css, "accent"), HOUSE_ACCENT.to_string());
        assert_eq!(token(css, "accent-text"), text_on(HOUSE_ACCENT).to_string());
        assert_eq!(SWATCHES[0].color, HOUSE_ACCENT.to_string());
    }

    /// The text levels, the card surface and the swatch alpha the washes
    /// and swatches are computed for (core `cover::palette`).
    #[test]
    fn tokens_css_matches_the_contrast_inputs() {
        use coda_core::cover::palette::{CARD_SURFACE, SWATCH_UNDER_TEXT, TEXT, TEXT_2, TEXT_3};
        let css = include_str!("../../src/styles/tokens.css");
        assert_eq!(token(css, "text"), TEXT.to_string());
        assert_eq!(token(css, "text-2"), TEXT_2.to_string());
        assert_eq!(token(css, "text-3"), TEXT_3.to_string());
        assert_eq!(token(css, "surface-1"), CARD_SURFACE.to_string());
        assert_eq!(
            token(css, "swatch-under-text"),
            format!("{}%", (SWATCH_UNDER_TEXT * 100.0).round())
        );
    }

    /// How the CSS draws the covers under the washes: the opacity and the
    /// filter core's `Surface`s assume.
    #[test]
    fn tokens_css_matches_the_wash_surfaces() {
        use coda_core::cover::palette::{PAGE_GLOW, PLAYER, Surface};
        let css = include_str!("../../src/styles/tokens.css");
        let check = |name: &str, surface: Surface| {
            let opacity: f64 = token(css, &format!("{name}-glow-opacity")).parse().unwrap();
            assert_eq!(opacity, surface.opacity, "{name}");
            let filter = token(css, &format!("{name}-glow-filter"));
            let saturate = filter
                .split_once("saturate(")
                .and_then(|(_, rest)| rest.split_once(')'))
                .map(|(value, _)| value.parse::<f64>().unwrap())
                .unwrap_or_else(|| panic!("{name}: saturate() in {filter}"));
            assert_eq!(saturate, surface.saturate, "{name}");
            // Only a blur besides: nothing else changes the colours.
            let rest = filter.replace(&format!("saturate({saturate})"), "");
            let rest = rest.trim();
            assert!(
                rest.starts_with("blur(") && rest.ends_with(')') && rest.matches('(').count() == 1,
                "{name}: {filter}"
            );
        };
        check("page", PAGE_GLOW);
        check("player", PLAYER);
    }

    #[test]
    fn swatches_are_valid_and_read_on_the_base() {
        for swatch in SWATCHES {
            let color = Rgba::parse_rgb(swatch.color).unwrap();
            assert_eq!(color.to_string(), swatch.color);
            assert!(
                swatch.value == "house" || swatch.value == swatch.color,
                "{swatch:?}"
            );
            // Faint text, accents: at least 3:1 (DESIGN.md "Contrast");
            // the curated ones clear 4.5:1.
            assert!(contrast(color, BASE) >= 4.5, "{swatch:?}");
            assert!(contrast(text_on(color), color) >= 4.5, "{swatch:?}");
        }
    }
}
