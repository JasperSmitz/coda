//! A single-line text input for search queries, shared by the local and
//! Navidrome searches. No Ratatui: keys and pastes arrive as text.

/// Upper bound on the input, in chars, unless set otherwise.
pub const MAX_QUERY_CHARS: usize = 256;
/// Upper bound for inputs holding a file system path (Linux `PATH_MAX`).
pub const MAX_PATH_CHARS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineInput {
    text: String,
    /// `text.chars().count()`, kept so appends never rescan the text.
    len: usize,
    /// Most chars the input holds.
    limit: usize,
}

impl Default for LineInput {
    fn default() -> Self {
        Self::with_limit(MAX_QUERY_CHARS)
    }
}

impl LineInput {
    /// An empty input of at most [`MAX_QUERY_CHARS`].
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty input of at most `limit` chars.
    pub fn with_limit(limit: usize) -> Self {
        Self {
            text: String::new(),
            len: 0,
            limit,
        }
    }

    /// An input holding `text`, filtered like a paste.
    pub fn with_text(text: &str) -> Self {
        Self::with_limit_and_text(MAX_QUERY_CHARS, text)
    }

    /// An input of at most `limit` chars holding `text`, filtered like a
    /// paste.
    pub fn with_limit_and_text(limit: usize, text: &str) -> Self {
        let mut input = Self::with_limit(limit);
        input.push_str(text);
        input
    }

    /// `true` if `text` would be held exactly as it is: no longer than
    /// `limit` and nothing that filtering would change.
    pub fn holds_exactly(limit: usize, text: &str) -> bool {
        Self::with_limit_and_text(limit, text).as_str() == text
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The masked display mode for secrets: one `•` per char, so the
    /// text itself never reaches the screen.
    pub fn masked(&self) -> String {
        "•".repeat(self.len)
    }

    /// Adds a typed char; see [`LineInput::push_str`].
    pub fn push_char(&mut self, c: char) {
        let mut buf = [0u8; 4];
        self.push_str(c.encode_utf8(&mut buf));
    }

    /// Adds pasted (or typed) text. Line breaks and tabs become a single
    /// space (`\r\n` gives one space, not two); other control characters
    /// are dropped, so a paste never submits. Input stops at the limit
    /// ([`MAX_QUERY_CHARS`] by default); work is bounded by the cap, not the
    /// paste size.
    pub fn push_str(&mut self, text: &str) {
        let mut previous = None;
        for c in text.chars() {
            if self.len >= self.limit {
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

    #[test]
    fn limits_can_be_raised_and_exact_holding_is_checked() {
        let path = format!("/{}", "a".repeat(1000));
        let input = LineInput::with_limit_and_text(MAX_PATH_CHARS, &path);
        assert_eq!(input.as_str(), path);
        assert_eq!(
            LineInput::with_text(&path).as_str().chars().count(),
            MAX_QUERY_CHARS
        );
        let mut input = LineInput::with_limit(3);
        input.push_str("abcd");
        assert_eq!(input.as_str(), "abc");
        assert!(LineInput::holds_exactly(MAX_PATH_CHARS, &path));
        assert!(!LineInput::holds_exactly(MAX_QUERY_CHARS, &path));
        assert!(!LineInput::holds_exactly(MAX_PATH_CHARS, "/a\tb"));
        assert!(!LineInput::holds_exactly(MAX_PATH_CHARS, "/a\u{1b}b"));
        assert!(LineInput::holds_exactly(MAX_PATH_CHARS, "~/Mü sic"));
    }

    #[test]
    fn masked_mode_shows_one_bullet_per_char() {
        let mut input = LineInput::with_text("pä ss\n");
        assert_eq!(input.as_str(), "pä ss ");
        assert_eq!(input.masked(), "••••••");
        assert!(!input.is_empty());
        for _ in 0..6 {
            input.backspace();
        }
        assert!(input.is_empty());
        assert_eq!(input.masked(), "");
    }
}
