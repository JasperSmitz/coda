//! Key → [`Action`] mapping (the M7 keymap, `docs/plans/m7-keybindings-qol.md`).
//! Panel-specific keys only map in their panel; while the help overlay is
//! open it swallows every key except the ones that close it, scroll it
//! (`j`/`k`) and quit. While a text input is open, keys are text
//! ([`map_input_key`]); while a y/n question is open, only `y` confirms
//! ([`map_confirm_key`]).

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
            KeyCode::Char('j') | KeyCode::Down => Some(Action::Down),
            KeyCode::Char('k') | KeyCode::Up => Some(Action::Up),
            _ => None,
        };
    }
    let library = focus == Focus::Library;
    // `<`, `>`, `O` and the other shifted keys may arrive with SHIFT set;
    // only the character counts.
    let action = match key.code {
        KeyCode::Char('j') | KeyCode::Down => Action::Down,
        KeyCode::Char('k') | KeyCode::Up => Action::Up,
        KeyCode::Enter => Action::Activate,
        KeyCode::Char('l') | KeyCode::Right if library => Action::Open,
        KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace if library => Action::Parent,
        KeyCode::Char('a') if library => Action::Enqueue,
        KeyCode::Char('A') if library => Action::EnqueueNext,
        KeyCode::Char('s') if library => Action::PlayShuffled,
        KeyCode::Char('s') => Action::ShuffleUpcoming,
        KeyCode::Char('o') if library => Action::CycleSort,
        KeyCode::Char('/') if library => Action::OpenSearch,
        KeyCode::Esc if library => Action::Back,
        KeyCode::Char('R') if library => Action::Refresh,
        KeyCode::Char('P') if library => Action::AddToPlaylist,
        KeyCode::Char('N') if library => Action::NewPlaylist,
        KeyCode::Char('e') if library => Action::RenamePlaylist,
        KeyCode::Char('w') if library => Action::SaveOrder,
        KeyCode::Char('1') => Action::SelectLocal,
        KeyCode::Char('2') => Action::SelectNavidrome,
        // Queue: the entry; library: a Navidrome playlist's song, or (in
        // the Playlists list) the playlist.
        KeyCode::Char('D') => Action::Remove,
        // In the queue: move the entry; in a Navidrome playlist: the song.
        KeyCode::Char('J') => Action::MoveEntryDown,
        KeyCode::Char('K') => Action::MoveEntryUp,
        KeyCode::Char('C') if !library => Action::ClearQueue,
        KeyCode::Char('g') if !library => Action::SelectCurrent,
        KeyCode::Char('S') if !library => Action::SaveQueueAsPlaylist,
        KeyCode::Char('r') => Action::CycleRepeat,
        KeyCode::Char('v') => Action::ToggleVisualizer,
        KeyCode::Char(' ') => Action::TogglePause,
        KeyCode::Char('>') => Action::Next,
        KeyCode::Char('<') => Action::Previous,
        KeyCode::Char(',') => Action::SeekBackward,
        KeyCode::Char('.') => Action::SeekForward,
        KeyCode::Char('+') | KeyCode::Char('=') => Action::VolumeUp,
        KeyCode::Char('-') => Action::VolumeDown,
        KeyCode::Tab => Action::SwitchFocus,
        KeyCode::Char('?') => Action::ToggleHelp,
        KeyCode::Char('O') => Action::OpenSettings,
        KeyCode::Char('q') => Action::Quit,
        _ => return None,
    };
    Some(action)
}

/// Keys while a y/n question is open: only `y` confirms (as
/// [`Action::InputChar`]); Ctrl-C quits; every other key, including
/// Enter, Esc and `n`, cancels ([`Action::InputCancel`]).
pub fn map_confirm_key(key: KeyEvent) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') | KeyCode::Char('C') if ctrl => Action::Quit,
        KeyCode::Char('y')
            if !ctrl
                && !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SUPER) =>
        {
            Action::InputChar('y')
        }
        _ => Action::InputCancel,
    }
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

    fn shifted(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)
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
            (ch('>'), Action::Next),
            (shifted('>'), Action::Next),
            (ch('<'), Action::Previous),
            (shifted('<'), Action::Previous),
            (ch(','), Action::SeekBackward),
            (ch('.'), Action::SeekForward),
            (ch('+'), Action::VolumeUp),
            (ch('='), Action::VolumeUp),
            (ch('-'), Action::VolumeDown),
            (key(KeyCode::Tab), Action::SwitchFocus),
            (ch('?'), Action::ToggleHelp),
            (ch('q'), Action::Quit),
            (ch('1'), Action::SelectLocal),
            (ch('2'), Action::SelectNavidrome),
            (ch('r'), Action::CycleRepeat),
            (ch('v'), Action::ToggleVisualizer),
            (ch('O'), Action::OpenSettings),
            (shifted('O'), Action::OpenSettings),
            (ch('D'), Action::Remove),
            (shifted('D'), Action::Remove),
            (ch('J'), Action::MoveEntryDown),
            (ch('K'), Action::MoveEntryUp),
        ];
        for (k, action) in cases {
            assert_eq!(lib(k), Some(action), "library {k:?}");
            assert_eq!(queue(k), Some(action), "queue {k:?}");
        }
    }

    #[test]
    fn library_only_keys() {
        let cases = [
            (ch('l'), Action::Open),
            (key(KeyCode::Right), Action::Open),
            (ch('h'), Action::Parent),
            (key(KeyCode::Left), Action::Parent),
            (key(KeyCode::Backspace), Action::Parent),
            (ch('a'), Action::Enqueue),
            (ch('A'), Action::EnqueueNext),
            (shifted('A'), Action::EnqueueNext),
            (ch('/'), Action::OpenSearch),
            (key(KeyCode::Esc), Action::Back),
            (ch('R'), Action::Refresh),
            (shifted('R'), Action::Refresh),
            (ch('o'), Action::CycleSort),
            (ch('P'), Action::AddToPlaylist),
            (shifted('P'), Action::AddToPlaylist),
            (ch('N'), Action::NewPlaylist),
            (ch('e'), Action::RenamePlaylist),
            (ch('w'), Action::SaveOrder),
        ];
        for (k, action) in cases {
            assert_eq!(lib(k), Some(action), "{k:?}");
            assert_eq!(queue(k), None, "{k:?}");
        }
    }

    #[test]
    fn queue_only_keys() {
        let cases = [
            (ch('C'), Action::ClearQueue),
            (ch('g'), Action::SelectCurrent),
            (ch('S'), Action::SaveQueueAsPlaylist),
            (shifted('S'), Action::SaveQueueAsPlaylist),
        ];
        for (k, action) in cases {
            assert_eq!(queue(k), Some(action), "{k:?}");
            assert_eq!(lib(k), None, "{k:?}");
        }
    }

    #[test]
    fn s_plays_shuffled_in_the_library_and_shuffles_upcoming_in_the_queue() {
        assert_eq!(lib(ch('s')), Some(Action::PlayShuffled));
        assert_eq!(queue(ch('s')), Some(Action::ShuffleUpcoming));
    }

    #[test]
    fn old_bindings_are_gone() {
        for focus in [Focus::Library, Focus::Queue] {
            let map = |k| map_key(k, focus, false);
            // `n`, `p` and lowercase `d` are unbound.
            for c in ['n', 'p', 'd'] {
                assert_eq!(map(ch(c)), None, "{focus:?} {c:?}");
            }
            // The arrows no longer seek.
            for code in [KeyCode::Left, KeyCode::Right] {
                let action = map(key(code));
                assert!(
                    !matches!(action, Some(Action::SeekBackward | Action::SeekForward)),
                    "{focus:?} {code:?}: {action:?}"
                );
            }
            // `,` seeks rather than opening settings; `R` is not repeat.
            assert_ne!(map(ch(',')), Some(Action::OpenSettings));
            assert_ne!(map(ch('R')), Some(Action::CycleRepeat));
        }
        // In the queue the arrows and `l` do nothing at all.
        assert_eq!(queue(key(KeyCode::Left)), None);
        assert_eq!(queue(key(KeyCode::Right)), None);
        assert_eq!(queue(ch('l')), None);
        assert_eq!(queue(ch('R')), None);
    }

    #[test]
    fn editing_keys_are_text_in_an_input() {
        for c in [
            'P', 'N', 'e', 'D', 'd', 'w', 'S', 'J', 'K', 'C', 'g', 's', 'r', 'R', 'O', 'l',
        ] {
            assert_eq!(map_input_key(ch(c)), Some(Action::InputChar(c)), "{c:?}");
        }
        for c in ['<', '>', ',', '.'] {
            assert_eq!(map_input_key(ch(c)), Some(Action::InputChar(c)), "{c:?}");
        }
    }

    #[test]
    fn ctrl_c_quits_rather_than_clearing_the_queue() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('C'), KeyModifiers::CONTROL);
        assert_eq!(queue(ctrl_c), Some(Action::Quit));
        let ctrl_shift_c = KeyEvent::new(
            KeyCode::Char('C'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(queue(ctrl_shift_c), Some(Action::Quit));
    }

    #[test]
    fn only_y_confirms_a_question() {
        assert_eq!(map_confirm_key(ch('y')), Action::InputChar('y'));
        for k in [
            ch('Y'),
            ch('n'),
            ch('D'),
            ch('q'),
            key(KeyCode::Enter),
            key(KeyCode::Esc),
            key(KeyCode::Tab),
            key(KeyCode::Left),
            key(KeyCode::F(1)),
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::ALT),
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
        ] {
            assert_eq!(map_confirm_key(k), Action::InputCancel, "{k:?}");
        }
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(map_confirm_key(ctrl_c), Action::Quit);
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
                assert_eq!(map_key(ctrl('s'), focus, help), None);
            }
        }
        let alt = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT);
        assert_eq!(lib(alt), None);
    }

    #[test]
    fn unbound_keys_map_to_nothing() {
        for k in [
            ch('x'),
            ch('Q'),
            ch('n'),
            ch('p'),
            ch('d'),
            key(KeyCode::F(1)),
        ] {
            assert_eq!(lib(k), None, "{k:?}");
            assert_eq!(queue(k), None, "{k:?}");
        }
    }

    #[test]
    fn help_overlay_swallows_everything_but_close_scroll_and_quit() {
        for focus in [Focus::Library, Focus::Queue] {
            let help = |k| map_key(k, focus, true);
            assert_eq!(help(ch('?')), Some(Action::ToggleHelp));
            assert_eq!(help(key(KeyCode::Esc)), Some(Action::CloseHelp));
            assert_eq!(help(ch('q')), Some(Action::Quit));
            assert_eq!(help(ch('j')), Some(Action::Down));
            assert_eq!(help(key(KeyCode::Down)), Some(Action::Down));
            assert_eq!(help(ch('k')), Some(Action::Up));
            assert_eq!(help(key(KeyCode::Up)), Some(Action::Up));
            for k in [
                key(KeyCode::Enter),
                ch(' '),
                ch('>'),
                ch('<'),
                ch(','),
                ch('l'),
                ch('O'),
                ch('D'),
                ch('a'),
                ch('A'),
                ch('o'),
                ch('d'),
                ch('h'),
                ch('1'),
                ch('2'),
                ch('/'),
                ch('r'),
                ch('R'),
                ch('J'),
                ch('K'),
                ch('C'),
                ch('g'),
                ch('s'),
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
