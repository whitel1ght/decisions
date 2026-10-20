//! Terminal key events to the app's keys.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::Key;

pub fn map(ev: KeyEvent) -> Option<Key> {
    if ev.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    Some(match ev.code {
        KeyCode::Char('d') if ctrl => Key::CtrlD,
        KeyCode::Char('u') if ctrl => Key::CtrlU,
        KeyCode::Char('c') if ctrl => Key::Interrupt,
        KeyCode::Char(_) if ctrl => return None,
        KeyCode::Char(' ') => Key::Space,
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Backspace => Key::Backspace,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventState, KeyModifiers as M};

    fn ev(code: KeyCode, mods: M, kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind,
            state: KeyEventState::NONE,
        }
    }

    fn press(code: KeyCode, mods: M) -> Option<Key> {
        map(ev(code, mods, KeyEventKind::Press))
    }

    #[test]
    fn maps_the_keys_the_app_knows() {
        assert_eq!(press(KeyCode::Char('j'), M::NONE), Some(Key::Char('j')));
        assert_eq!(press(KeyCode::Char('G'), M::SHIFT), Some(Key::Char('G')));
        assert_eq!(press(KeyCode::Char(' '), M::NONE), Some(Key::Space));
        assert_eq!(press(KeyCode::Char('d'), M::CONTROL), Some(Key::CtrlD));
        assert_eq!(press(KeyCode::Char('u'), M::CONTROL), Some(Key::CtrlU));
        assert_eq!(press(KeyCode::Char('c'), M::CONTROL), Some(Key::Interrupt));
        assert_eq!(press(KeyCode::Enter, M::NONE), Some(Key::Enter));
        assert_eq!(press(KeyCode::Esc, M::NONE), Some(Key::Esc));
        assert_eq!(press(KeyCode::Up, M::NONE), Some(Key::Up));
        assert_eq!(press(KeyCode::Backspace, M::NONE), Some(Key::Backspace));
    }

    #[test]
    fn ignores_releases_other_control_keys_and_unknown_keys() {
        assert_eq!(
            map(ev(KeyCode::Char('j'), M::NONE, KeyEventKind::Release)),
            None
        );
        assert_eq!(press(KeyCode::Char('x'), M::CONTROL), None);
        assert_eq!(press(KeyCode::F(1), M::NONE), None);
    }
}
