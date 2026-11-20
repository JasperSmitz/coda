//! Development builds only (`debug_assertions`): `debug_trace` writes
//! trace lines the webview gathered (its `lib/devTrace.ts`: 2.0 R3, the
//! root swipe's wheel events) to the app's log, target
//! `coda_app::trace`, so a misbehaviour on the owner's machine can be read
//! back. The lines hold only numbers and states. Bounded per call; never
//! touches the engine. Release builds do not compile the command (its
//! permission stays declared, so `tests/acl.rs` keeps the three lists in
//! step; calling it there fails as an unknown command).

/// Lines one call may carry.
const MAX_LINES: usize = 500;
/// Characters kept of a line.
const MAX_LINE: usize = 300;

#[tauri::command]
pub async fn debug_trace(lines: Vec<String>) {
    for line in lines.iter().take(MAX_LINES) {
        let line = clean(line);
        tracing::info!(target: "coda_app::trace", "{line}");
    }
}

/// At most [`MAX_LINE`] characters, control characters (line breaks too)
/// as spaces: one log line each, never a forged one.
fn clean(line: &str) -> String {
    line.chars()
        .take(MAX_LINE)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_one_line_and_bounded() {
        assert_eq!(clean("a\nb\rc\td\u{1b}e"), "a b c d e");
        assert_eq!(clean(&"x".repeat(1000)).len(), MAX_LINE);
    }
}
