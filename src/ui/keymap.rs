//! Key → [`Action`] mapping (brief 3.10). Panel-specific keys only map in
//! their panel; while the help overlay is open it swallows every key except
//! the ones that close it and quit. While the search input is open, keys
//! are text ([`map_input_key`]).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{Action, Focus};

pub fn map_key(key: KeyEvent, focus: Focus, help_open: bool) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl {
        // Raw mode delivers Ctrl-C as a key; it always quits. Other
        // Ctrl/Alt combinations are unbound.
        return matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')).then_some(Action::Quit);
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        return None;
    }
    if help_open {
        return match key.code {
            KeyCode::Char('?') => Some(Action::ToggleHelp),
            KeyCode::Esc => Some(Action::CloseHelp),
            KeyCode::Char('q') => Some(Action::Quit),
            _ => None,
        };
    }
    let action = match key.code {
        KeyCode::Char('j') | KeyCode::Down => Action::Down,
        KeyCode::Char('k') | KeyCode::Up => Action::Up,
        KeyCode::Enter => Action::Activate,
        KeyCode::Char('h') | KeyCode::Backspace if focus == Focus::Library => Action::Parent,
        KeyCode::Char('a') if focus == Focus::Library => Action::Enqueue,
        KeyCode::Char('/') if focus == Focus::Library => Action::OpenSearch,
        KeyCode::Char('r') if focus == Focus::Library => Action::Refresh,
        KeyCode::Char('1') => Action::SelectLocal,
        KeyCode::Char('2') => Action::SelectNavidrome,
        KeyCode::Char('d') if focus == Focus::Queue => Action::Remove,
        KeyCode::Char(' ') => Action::TogglePause,
        KeyCode::Char('n') => Action::Next,
        KeyCode::Char('p') => Action::Previous,
        KeyCode::Left => Action::SeekBackward,
        KeyCode::Right => Action::SeekForward,
        KeyCode::Char('+') | KeyCode::Char('=') => Action::VolumeUp,
        KeyCode::Char('-') => Action::VolumeDown,
        KeyCode::Tab => Action::SwitchFocus,
        KeyCode::Char('?') => Action::ToggleHelp,
        KeyCode::Char('q') => Action::Quit,
        _ => return None,
    };
    Some(action)
}

/// Keys while the search input is open: printable characters are text,
/// Backspace edits, Enter submits, Esc cancels. Ctrl-C still quits; other
/// Ctrl/Alt combinations and control characters are ignored.
pub fn map_input_key(key: KeyEvent) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')).then_some(Action::Quit);
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        return None;
    }
    match key.code {
        KeyCode::Char(c) if !c.is_control() => Some(Action::InputChar(c)),
        KeyCode::Backspace => Some(Action::InputBackspace),
        KeyCode::Enter => Some(Action::InputSubmit),
        KeyCode::Esc => Some(Action::InputCancel),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    fn lib(k: KeyEvent) -> Option<Action> {
        map_key(k, Focus::Library, false)
    }

    fn queue(k: KeyEvent) -> Option<Action> {
        map_key(k, Focus::Queue, false)
    }

    #[test]
    fn shared_keys_map_the_same_in_both_panels() {
        let cases = [
            (ch('j'), Action::Down),
            (key(KeyCode::Down), Action::Down),
            (ch('k'), Action::Up),
            (key(KeyCode::Up), Action::Up),
            (key(KeyCode::Enter), Action::Activate),
            (ch(' '), Action::TogglePause),
            (ch('n'), Action::Next),
            (ch('p'), Action::Previous),
            (key(KeyCode::Left), Action::SeekBackward),
            (key(KeyCode::Right), Action::SeekForward),
            (ch('+'), Action::VolumeUp),
            (ch('='), Action::VolumeUp),
            (ch('-'), Action::VolumeDown),
            (key(KeyCode::Tab), Action::SwitchFocus),
            (ch('?'), Action::ToggleHelp),
            (ch('q'), Action::Quit),
            (ch('1'), Action::SelectLocal),
            (ch('2'), Action::SelectNavidrome),
        ];
        for (k, action) in cases {
            assert_eq!(lib(k), Some(action), "library {k:?}");
            assert_eq!(queue(k), Some(action), "queue {k:?}");
        }
    }

    #[test]
    fn library_only_keys() {
        assert_eq!(lib(ch('h')), Some(Action::Parent));
        assert_eq!(lib(key(KeyCode::Backspace)), Some(Action::Parent));
        assert_eq!(lib(ch('a')), Some(Action::Enqueue));
        assert_eq!(lib(ch('/')), Some(Action::OpenSearch));
        assert_eq!(lib(ch('r')), Some(Action::Refresh));
        assert_eq!(queue(ch('h')), None);
        assert_eq!(queue(key(KeyCode::Backspace)), None);
        assert_eq!(queue(ch('a')), None);
        assert_eq!(queue(ch('/')), None);
        assert_eq!(queue(ch('r')), None);
    }

    #[test]
    fn queue_only_keys() {
        assert_eq!(queue(ch('d')), Some(Action::Remove));
        assert_eq!(lib(ch('d')), None);
    }

    #[test]
    fn shifted_symbols_still_map() {
        // Terminals may report `+` and `?` with SHIFT set.
        let shifted = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT);
        assert_eq!(lib(shifted('+')), Some(Action::VolumeUp));
        assert_eq!(lib(shifted('?')), Some(Action::ToggleHelp));
    }

    #[test]
    fn ctrl_c_quits_everywhere_and_other_modifiers_are_unbound() {
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        for help in [false, true] {
            for focus in [Focus::Library, Focus::Queue] {
                assert_eq!(map_key(ctrl('c'), focus, help), Some(Action::Quit));
                assert_eq!(map_key(ctrl('n'), focus, help), None);
            }
        }
        let alt = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT);
        assert_eq!(lib(alt), None);
    }

    #[test]
    fn unbound_keys_map_to_nothing() {
        for k in [ch('x'), ch('Q'), key(KeyCode::Esc), key(KeyCode::F(1))] {
            assert_eq!(lib(k), None, "{k:?}");
            assert_eq!(queue(k), None, "{k:?}");
        }
    }

    #[test]
    fn help_overlay_swallows_everything_but_close_and_quit() {
        for focus in [Focus::Library, Focus::Queue] {
            let help = |k| map_key(k, focus, true);
            assert_eq!(help(ch('?')), Some(Action::ToggleHelp));
            assert_eq!(help(key(KeyCode::Esc)), Some(Action::CloseHelp));
            assert_eq!(help(ch('q')), Some(Action::Quit));
            for k in [
                ch('j'),
                key(KeyCode::Enter),
                ch(' '),
                ch('n'),
                ch('a'),
                ch('d'),
                ch('h'),
                ch('1'),
                ch('2'),
                ch('/'),
                ch('r'),
                key(KeyCode::Tab),
                key(KeyCode::Right),
                ch('+'),
            ] {
                assert_eq!(help(k), None, "{focus:?} {k:?}");
            }
        }
    }

    #[test]
    fn input_keys_are_text_except_for_editing_keys() {
        let shifted = KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT);
        assert_eq!(map_input_key(shifted), Some(Action::InputChar('Q')));
        for c in ['q', 'j', '?', '1', '2', '/', ' ', 'a', '雨'] {
            assert_eq!(map_input_key(ch(c)), Some(Action::InputChar(c)), "{c:?}");
        }
        assert_eq!(
            map_input_key(key(KeyCode::Enter)),
            Some(Action::InputSubmit)
        );
        assert_eq!(map_input_key(key(KeyCode::Esc)), Some(Action::InputCancel));
        assert_eq!(
            map_input_key(key(KeyCode::Backspace)),
            Some(Action::InputBackspace)
        );
        assert_eq!(map_input_key(ch('\u{7}')), None);
        assert_eq!(map_input_key(key(KeyCode::Tab)), None);
        assert_eq!(map_input_key(key(KeyCode::Up)), None);
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(map_input_key(ctrl('c')), Some(Action::Quit));
        assert_eq!(map_input_key(ctrl('u')), None);
        let alt = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT);
        assert_eq!(map_input_key(alt), None);
    }
}
