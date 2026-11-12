//! Line editing for the deck's text boxes. The cursor is kept as `back`, the number of
//! characters after it, so text added at the end (a paste, a picked path) leaves it at the end.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,
    /// The character under the cursor.
    Delete,
    /// The word before the cursor.
    DeleteWord,
    /// The whole text.
    Clear,
}

/// The byte offset of the cursor in `text` when `back` characters follow it.
pub fn at(text: &str, back: usize) -> usize {
    let n = text.chars().count();
    text.char_indices()
        .nth(n - back.min(n))
        .map_or(text.len(), |(i, _)| i)
}

pub fn insert(text: &mut String, back: usize, s: &str) {
    let i = at(text, back);
    text.insert_str(i, s);
}

/// Deletes the character before the cursor; false when there is none.
pub fn backspace(text: &mut String, back: &mut usize) -> bool {
    *back = (*back).min(text.chars().count());
    let i = at(text, *back);
    match text[..i].chars().next_back() {
        Some(c) => {
            text.remove(i - c.len_utf8());
            true
        }
        None => false,
    }
}

pub fn apply(text: &mut String, back: &mut usize, e: Edit) {
    let n = text.chars().count();
    *back = (*back).min(n);
    let chars: Vec<char> = text.chars().collect();
    let cur = n - *back;
    match e {
        Edit::Left => *back = (*back + 1).min(n),
        Edit::Right => *back = back.saturating_sub(1),
        Edit::WordLeft => *back = n - word_start(&chars, cur),
        Edit::WordRight => *back = n - word_end(&chars, cur),
        Edit::Home => *back = n,
        Edit::End => *back = 0,
        Edit::Delete => {
            if *back > 0 {
                text.remove(at(text, *back));
                *back -= 1;
            }
        }
        Edit::DeleteWord => {
            let from = word_start(&chars, cur);
            let (a, b) = (at(text, n - from), at(text, *back));
            text.replace_range(a..b, "");
        }
        Edit::Clear => {
            text.clear();
            *back = 0;
        }
    }
}

/// Where the word before `cur` starts, skipping the spaces right before it.
fn word_start(chars: &[char], mut cur: usize) -> usize {
    while cur > 0 && chars[cur - 1].is_whitespace() {
        cur -= 1;
    }
    while cur > 0 && !chars[cur - 1].is_whitespace() {
        cur -= 1;
    }
    cur
}

/// Where the word after `cur` ends, skipping the spaces right after it.
fn word_end(chars: &[char], mut cur: usize) -> usize {
    while cur < chars.len() && chars[cur].is_whitespace() {
        cur += 1;
    }
    while cur < chars.len() && !chars[cur].is_whitespace() {
        cur += 1;
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, back: usize, e: Edit) -> (String, usize) {
        let (mut t, mut b) = (text.to_string(), back);
        apply(&mut t, &mut b, e);
        (t, b)
    }

    #[test]
    fn left_and_right_stop_at_the_ends() {
        assert_eq!(run("ab", 0, Edit::Left), ("ab".into(), 1));
        assert_eq!(run("ab", 2, Edit::Left), ("ab".into(), 2));
        assert_eq!(run("ab", 1, Edit::Right), ("ab".into(), 0));
        assert_eq!(run("ab", 0, Edit::Right), ("ab".into(), 0));
        assert_eq!(run("ab", 0, Edit::Home), ("ab".into(), 2));
        assert_eq!(run("ab", 2, Edit::End), ("ab".into(), 0));
    }

    #[test]
    fn typing_goes_in_at_the_cursor_and_keeps_it_after_the_new_text() {
        let mut t = "helo".to_string();
        insert(&mut t, 1, "l");
        assert_eq!(t, "hello");
        insert(&mut t, 0, "!");
        assert_eq!(t, "hello!");
    }

    #[test]
    fn backspace_and_delete_work_around_the_cursor_with_wide_characters() {
        let (mut t, mut b) = ("añb".to_string(), 1);
        assert!(backspace(&mut t, &mut b));
        assert_eq!((t.as_str(), b), ("ab", 1));
        let (mut t, mut b) = ("ab".to_string(), 2);
        assert!(!backspace(&mut t, &mut b));
        assert_eq!(run("añb", 2, Edit::Delete), ("ab".into(), 1));
        assert_eq!(run("ab", 0, Edit::Delete), ("ab".into(), 0));
    }

    #[test]
    fn words_jump_over_spaces_then_the_word() {
        assert_eq!(run("one two  three", 0, Edit::WordLeft).1, 5);
        assert_eq!(run("one two  three", 5, Edit::WordLeft).1, 10);
        assert_eq!(run("one two  three", 14, Edit::WordRight).1, 11);
        assert_eq!(run("one two  three", 11, Edit::WordRight).1, 7);
        assert_eq!(
            run("one two  three", 0, Edit::DeleteWord),
            ("one two  ".into(), 0)
        );
        assert_eq!(
            run("one two  three", 5, Edit::DeleteWord),
            ("one three".into(), 5)
        );
    }

    #[test]
    fn clear_empties_the_text_and_a_stale_cursor_is_clamped() {
        assert_eq!(run("anything at all", 4, Edit::Clear), (String::new(), 0));
        assert_eq!(run("ab", 9, Edit::Right), ("ab".into(), 1));
        assert_eq!(at("ab", 9), 0);
    }
}
