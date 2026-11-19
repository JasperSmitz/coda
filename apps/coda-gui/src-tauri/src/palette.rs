//! The app's fixed colours (DESIGN.md "Colour", ADR 0009): the base
//! behind everything, the house accent and the curated accents offered in
//! Preferences, plus the colour maths ([`color`]) that the accent's
//! contrast note (and, later, the music colours) use.
//!
//! The frontend's token layer (`src/styles/tokens.css`) holds the same
//! base and house accent; a test keeps the two in step.

pub mod color;

use serde::Serialize;

use self::color::Rgba;

/// The base colour (`--bg` in `tokens.css`, ~`hsl(256 14% 7%)`): the
/// native window and webview background, so nothing else shows before
/// the first paint or while resizing.
pub const BASE: Rgba = Rgba::rgb(0x11, 0x0f, 0x14);

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
