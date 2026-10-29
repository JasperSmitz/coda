//! A single-line text input for search queries, shared by the local and
//! Navidrome searches. No Ratatui: keys and pastes arrive as text.

/// Upper bound on the input, in chars.
pub const MAX_QUERY_CHARS: usize = 256;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineInput {
    text: String,
    /// `text.chars().count()`, kept so appends never rescan the text.
    len: usize,
}

impl LineInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Adds a typed char; see [`LineInput::push_str`].
    pub fn push_char(&mut self, c: char) {
        let mut buf = [0u8; 4];
        self.push_str(c.encode_utf8(&mut buf));
    }

    /// Adds pasted (or typed) text. Line breaks and tabs become a single
    /// space (`\r\n` gives one space, not two); other control characters
    /// are dropped, so a paste never submits. Input stops at
    /// [`MAX_QUERY_CHARS`]; work is bounded by the cap, not the paste size.
    pub fn push_str(&mut self, text: &str) {
        let mut previous = None;
        for c in text.chars() {
            if self.len >= MAX_QUERY_CHARS {
                break;
            }
            let crlf = previous == Some('\r') && c == '\n';
            previous = Some(c);
            let c = match c {
                '\n' | '\r' | '\t' => ' ',
                c if c.is_control() => continue,
                c => c,
            };
            if !crlf {
                self.text.push(c);
                self.len += 1;
            }
        }
    }

    pub fn backspace(&mut self) {
        if self.text.pop().is_some() {
            self.len -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_and_pasted_text_is_filtered_and_capped() {
        let mut input = LineInput::new();
        for c in "ab\u{7}c\u{1b}".chars() {
            input.push_char(c);
        }
        assert_eq!(input.as_str(), "abc");
        input.backspace();
        assert_eq!(input.as_str(), "ab");
        input.push_str("blue\nmoon\r\njazz\tbop\u{1b}\r");
        assert_eq!(input.as_str(), "abblue moon jazz bop ");
        input.push_str(&"雨".repeat(1_000_000));
        assert_eq!(input.as_str().chars().count(), MAX_QUERY_CHARS);
        assert!(input.as_str().starts_with("abblue moon"));
        for _ in 0..MAX_QUERY_CHARS + 5 {
            input.backspace();
        }
        assert_eq!(input.as_str(), "");
        input.push_char('x');
        assert_eq!(input.as_str(), "x", "length tracking survives emptying");
    }
}
