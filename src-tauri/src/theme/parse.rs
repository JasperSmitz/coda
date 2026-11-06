//! Ghostty theme files: `key = value` lines and `#` comments. Theme files
//! are untrusted: the size is capped, colours must be hex, a malformed
//! value for a key we use rejects the file, unknown keys are ignored.
//!
//! Coda's own keys ride along (Ghostty ignores them): `coda-accent =
//! #rrggbb | palette:N`, and `coda-<token> = #rrggbb[aa]` overrides for
//! every derived token ([`super::derive::OVERRIDABLE`]); `coda-surface`
//! is an alias of `coda-surface-1`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::color::Rgba;
use super::derive::OVERRIDABLE;

/// Larger theme files are not read.
pub const MAX_THEME_BYTES: u64 = 64 * 1024;

/// What `coda-accent` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccentSpec {
    Color(Rgba),
    Palette(usize),
}

/// One theme's colours as written in its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeFile {
    pub background: Rgba,
    pub foreground: Rgba,
    pub palette: [Option<Rgba>; 16],
    pub cursor_color: Option<Rgba>,
    pub cursor_text: Option<Rgba>,
    pub selection_background: Option<Rgba>,
    pub selection_foreground: Option<Rgba>,
    pub accent: Option<AccentSpec>,
    /// `coda-<token>` overrides, by token name (`surface-1`, `danger`, …).
    pub overrides: BTreeMap<String, Rgba>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("the file is larger than {MAX_THEME_BYTES} bytes")]
    TooLarge,
    #[error("line {0}: not `key = value`")]
    Syntax(usize),
    #[error("line {line}: `{key}` is not a hex colour")]
    BadColor { line: usize, key: String },
    #[error("line {0}: `palette` is not `N=#rrggbb` with N 0–15")]
    BadPalette(usize),
    #[error("line {0}: `coda-accent` is not a hex colour or `palette:N`")]
    BadAccent(usize),
    #[error("no `background`")]
    NoBackground,
    #[error("no `foreground`")]
    NoForeground,
}

/// Parses a theme file's text (see the module docs).
pub fn parse(text: &str) -> Result<ThemeFile, ParseError> {
    if text.len() as u64 > MAX_THEME_BYTES {
        return Err(ParseError::TooLarge);
    }
    let mut background = None;
    let mut foreground = None;
    let mut theme = ThemeFile {
        background: Rgba::rgb(0, 0, 0),
        foreground: Rgba::rgb(0, 0, 0),
        palette: [None; 16],
        cursor_color: None,
        cursor_text: None,
        selection_background: None,
        selection_foreground: None,
        accent: None,
        overrides: BTreeMap::new(),
    };
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let (key, value) = trimmed.split_once('=').ok_or(ParseError::Syntax(line))?;
        let (key, value) = (key.trim(), unquote(value.trim()));
        let color = |value: &str| {
            Rgba::parse_rgb(value).ok_or_else(|| ParseError::BadColor {
                line,
                key: key.to_owned(),
            })
        };
        match key {
            "background" => background = Some(color(value)?),
            "foreground" => foreground = Some(color(value)?),
            "cursor-color" => theme.cursor_color = Some(color(value)?),
            "cursor-text" => theme.cursor_text = Some(color(value)?),
            "selection-background" => theme.selection_background = Some(color(value)?),
            "selection-foreground" => theme.selection_foreground = Some(color(value)?),
            "palette" => {
                let (n, hex) = value.split_once('=').ok_or(ParseError::BadPalette(line))?;
                let n: usize = n.trim().parse().map_err(|_| ParseError::BadPalette(line))?;
                let hex = Rgba::parse_rgb(hex.trim()).ok_or(ParseError::BadPalette(line))?;
                *theme
                    .palette
                    .get_mut(n)
                    .ok_or(ParseError::BadPalette(line))? = Some(hex);
            }
            "coda-accent" => {
                theme.accent = Some(parse_accent(value).ok_or(ParseError::BadAccent(line))?)
            }
            _ => {
                let Some(token) = key.strip_prefix("coda-") else {
                    continue; // Another Ghostty setting.
                };
                let token = if token == "surface" {
                    "surface-1"
                } else {
                    token
                };
                // Unknown Coda keys are skipped (a newer app's file).
                if let Some(token) = OVERRIDABLE.iter().find(|t| **t == token) {
                    let value = Rgba::parse(value).ok_or_else(|| ParseError::BadColor {
                        line,
                        key: key.to_owned(),
                    })?;
                    theme.overrides.insert((*token).to_owned(), value);
                }
            }
        }
    }
    theme.background = background.ok_or(ParseError::NoBackground)?;
    theme.foreground = foreground.ok_or(ParseError::NoForeground)?;
    Ok(theme)
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

fn parse_accent(value: &str) -> Option<AccentSpec> {
    if let Some(n) = value.strip_prefix("palette:") {
        let n: usize = n.trim().parse().ok()?;
        return (n < 16).then_some(AccentSpec::Palette(n));
    }
    Rgba::parse_rgb(value).map(AccentSpec::Color)
}

/// The theme as a Ghostty file: Ghostty's keys for the base colours, then
/// Coda's keys. `based_on` goes in the header comment.
pub fn write(theme: &ThemeFile, based_on: Option<&str>) -> String {
    let mut out = String::from("# A Coda custom theme");
    if let Some(base) = based_on {
        // Keep the comment on one line whatever the name holds.
        let base: String = base.chars().filter(|c| !c.is_control()).collect();
        let _ = write!(out, ", based on \"{base}\"");
    }
    out.push_str(".\n# Ghostty reads the base colours; the coda-* keys are for Coda.\n");
    for (n, color) in theme.palette.iter().enumerate() {
        if let Some(color) = color {
            let _ = writeln!(out, "palette = {n}={color}");
        }
    }
    let _ = writeln!(out, "background = {}", theme.background);
    let _ = writeln!(out, "foreground = {}", theme.foreground);
    for (key, value) in [
        ("cursor-color", theme.cursor_color),
        ("cursor-text", theme.cursor_text),
        ("selection-background", theme.selection_background),
        ("selection-foreground", theme.selection_foreground),
    ] {
        if let Some(value) = value {
            let _ = writeln!(out, "{key} = {}", value.opaque());
        }
    }
    match theme.accent {
        Some(AccentSpec::Color(c)) => {
            let _ = writeln!(out, "coda-accent = {}", c.opaque());
        }
        Some(AccentSpec::Palette(n)) => {
            let _ = writeln!(out, "coda-accent = palette:{n}");
        }
        None => {}
    }
    for (token, value) in &theme.overrides {
        let _ = writeln!(out, "coda-{token} = {value}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const KANAGAWABONES: &str = include_str!("../../themes/Kanagawabones");

    #[test]
    fn parses_a_ghostty_theme() {
        let theme = parse(KANAGAWABONES).unwrap();
        assert_eq!(theme.background, Rgba::rgb(0x1f, 0x1f, 0x28));
        assert_eq!(theme.foreground, Rgba::rgb(0xdd, 0xd8, 0xbb));
        assert_eq!(theme.palette[3], Some(Rgba::rgb(0xe5, 0xc2, 0x83)));
        assert!(theme.palette.iter().all(Option::is_some));
        assert_eq!(
            theme.selection_background,
            Some(Rgba::rgb(0x49, 0x47, 0x3e))
        );
    }

    #[test]
    fn ignores_comments_unknown_keys_and_quotes() {
        let theme = parse(
            "# a comment\n\nbackground = \"#000000\"\nforeground=ffffff\nfont-size = 13\nwindow-padding-x = 4\n",
        )
        .unwrap();
        assert_eq!(theme.background, Rgba::rgb(0, 0, 0));
        assert_eq!(theme.foreground, Rgba::rgb(255, 255, 255));
        assert_eq!(theme.palette, [None; 16]);
    }

    #[test]
    fn rejects_malformed_files() {
        let base = "background = #000000\nforeground = #ffffff\n";
        for (extra, want) in [
            ("background = black\n", "BadColor"),
            ("palette = 16=#000000\n", "BadPalette"),
            ("palette = 1 #000000\n", "BadPalette"),
            ("palette = x=#000000\n", "BadPalette"),
            ("just words\n", "Syntax"),
            ("coda-accent = palette:16\n", "BadAccent"),
            ("coda-accent = blue\n", "BadAccent"),
            ("coda-surface-1 = #12\n", "BadColor"),
        ] {
            let error = parse(&format!("{base}{extra}")).unwrap_err();
            assert!(
                format!("{error:?}").starts_with(want),
                "{extra:?} → {error:?}"
            );
        }
        assert_eq!(
            parse("foreground = #ffffff\n"),
            Err(ParseError::NoBackground)
        );
        assert_eq!(
            parse("background = #ffffff\n"),
            Err(ParseError::NoForeground)
        );
    }

    #[test]
    fn rejects_oversized_files() {
        let mut text = String::from("background = #000000\nforeground = #ffffff\n");
        while text.len() as u64 <= MAX_THEME_BYTES {
            text.push_str("# padding padding padding padding padding padding padding\n");
        }
        assert_eq!(parse(&text), Err(ParseError::TooLarge));
    }

    #[test]
    fn reads_coda_keys_and_palette_refs() {
        let theme = parse(
            "background = #000000\nforeground = #ffffff\ncoda-accent = palette:4\n\
             coda-surface = #111111\ncoda-border = #222222\ncoda-danger = #ff0000\n\
             coda-hover = #ffffff0d\ncoda-from-the-future = #123456\n",
        )
        .unwrap();
        assert_eq!(theme.accent, Some(AccentSpec::Palette(4)));
        assert_eq!(theme.overrides["surface-1"], Rgba::rgb(0x11, 0x11, 0x11));
        assert_eq!(theme.overrides["border"], Rgba::rgb(0x22, 0x22, 0x22));
        assert_eq!(theme.overrides["danger"], Rgba::rgb(0xff, 0, 0));
        assert_eq!(
            theme.overrides["hover"],
            Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 0x0d
            }
        );
        assert_eq!(theme.overrides.len(), 4);
        let theme =
            parse("background = #000000\nforeground = #ffffff\ncoda-accent = #cba6f7\n").unwrap();
        assert_eq!(
            theme.accent,
            Some(AccentSpec::Color(Rgba::rgb(0xcb, 0xa6, 0xf7)))
        );
    }

    #[test]
    fn written_files_parse_back_the_same() {
        let mut theme = parse(KANAGAWABONES).unwrap();
        theme.accent = Some(AccentSpec::Color(Rgba::rgb(1, 2, 3)));
        theme
            .overrides
            .insert("surface-2".into(), Rgba::rgb(4, 5, 6));
        theme.overrides.insert(
            "hover".into(),
            Rgba {
                r: 7,
                g: 8,
                b: 9,
                a: 10,
            },
        );
        let text = write(&theme, Some("Kanagawabones\n# evil"));
        assert_eq!(text.lines().filter(|l| l.contains("evil")).count(), 1);
        assert_eq!(parse(&text).unwrap(), theme);
    }
}
