//! Terminal key events to the app's keys.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::Key;
use crate::edit::Edit;

pub fn map(ev: KeyEvent) -> Option<Key> {
    if ev.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    Some(match ev.code {
        KeyCode::Char('d') if ctrl => Key::CtrlD,
        KeyCode::Char('u') if ctrl => Key::CtrlU,
        KeyCode::Char('c') if ctrl => Key::Interrupt,
        KeyCode::Char('v') if ctrl => Key::CtrlV,
        KeyCode::Char('a') if ctrl => Key::Edit(Edit::Home),
        KeyCode::Char('e') if ctrl => Key::Edit(Edit::End),
        KeyCode::Char('w') if ctrl => Key::Edit(Edit::DeleteWord),
        KeyCode::Char(_) if ctrl => return None,
        // Option+←/→ arrives as Alt+b / Alt+f in most macOS terminals.
        KeyCode::Char('b') | KeyCode::Left if alt => Key::Edit(Edit::WordLeft),
        KeyCode::Char('f') | KeyCode::Right if alt => Key::Edit(Edit::WordRight),
        KeyCode::Backspace if alt => Key::Edit(Edit::DeleteWord),
        KeyCode::Left => Key::Edit(Edit::Left),
        KeyCode::Right => Key::Edit(Edit::Right),
        KeyCode::Home => Key::Edit(Edit::Home),
        KeyCode::End => Key::Edit(Edit::End),
        KeyCode::Delete => Key::Edit(Edit::Delete),
        KeyCode::Char(' ') => Key::Space,
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
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
        assert_eq!(press(KeyCode::Char('v'), M::CONTROL), Some(Key::CtrlV));
        assert_eq!(press(KeyCode::Tab, M::NONE), Some(Key::Tab));
        assert_eq!(press(KeyCode::BackTab, M::SHIFT), Some(Key::BackTab));
        assert_eq!(press(KeyCode::Enter, M::NONE), Some(Key::Enter));
        assert_eq!(press(KeyCode::Esc, M::NONE), Some(Key::Esc));
        assert_eq!(press(KeyCode::Up, M::NONE), Some(Key::Up));
        assert_eq!(press(KeyCode::Backspace, M::NONE), Some(Key::Backspace));
    }

    #[test]
    fn maps_the_editing_keys_of_a_shell_line() {
        let edit = |e| Some(Key::Edit(e));
        assert_eq!(press(KeyCode::Left, M::NONE), edit(Edit::Left));
        assert_eq!(press(KeyCode::Right, M::NONE), edit(Edit::Right));
        assert_eq!(press(KeyCode::Home, M::NONE), edit(Edit::Home));
        assert_eq!(press(KeyCode::End, M::NONE), edit(Edit::End));
        assert_eq!(press(KeyCode::Delete, M::NONE), edit(Edit::Delete));
        assert_eq!(press(KeyCode::Char('a'), M::CONTROL), edit(Edit::Home));
        assert_eq!(press(KeyCode::Char('e'), M::CONTROL), edit(Edit::End));
        assert_eq!(
            press(KeyCode::Char('w'), M::CONTROL),
            edit(Edit::DeleteWord)
        );
        assert_eq!(press(KeyCode::Backspace, M::ALT), edit(Edit::DeleteWord));
        assert_eq!(press(KeyCode::Left, M::ALT), edit(Edit::WordLeft));
        assert_eq!(press(KeyCode::Right, M::ALT), edit(Edit::WordRight));
        assert_eq!(press(KeyCode::Char('b'), M::ALT), edit(Edit::WordLeft));
        assert_eq!(press(KeyCode::Char('f'), M::ALT), edit(Edit::WordRight));
        assert_eq!(press(KeyCode::Char('b'), M::NONE), Some(Key::Char('b')));
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
