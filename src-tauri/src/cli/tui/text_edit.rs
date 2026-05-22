use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextEditCommand {
    MoveLeft,
    MoveRight,
    MoveLineStart,
    MoveLineEnd,
    MoveWordLeft,
    MoveWordRight,
    DeleteBackward,
    DeleteForward,
    DeleteToLineStart,
    DeleteToLineEnd,
    DeleteWordBackward,
    Insert(char),
}

impl TextEditCommand {
    pub(crate) fn from_key(key: KeyEvent) -> Option<Self> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        if control {
            return match key.code {
                KeyCode::Char('a' | 'A') => Some(Self::MoveLineStart),
                KeyCode::Char('b' | 'B') => Some(Self::MoveLeft),
                KeyCode::Char('d' | 'D') => Some(Self::DeleteForward),
                KeyCode::Char('e' | 'E') => Some(Self::MoveLineEnd),
                KeyCode::Char('f' | 'F') => Some(Self::MoveRight),
                KeyCode::Char('k' | 'K') => Some(Self::DeleteToLineEnd),
                KeyCode::Char('u' | 'U') => Some(Self::DeleteToLineStart),
                KeyCode::Char('w' | 'W') => Some(Self::DeleteWordBackward),
                _ => None,
            };
        }

        if alt {
            return match key.code {
                KeyCode::Backspace => Some(Self::DeleteWordBackward),
                KeyCode::Char('b' | 'B') => Some(Self::MoveWordLeft),
                KeyCode::Char('f' | 'F') => Some(Self::MoveWordRight),
                _ => None,
            };
        }

        match key.code {
            KeyCode::Left => Some(Self::MoveLeft),
            KeyCode::Right => Some(Self::MoveRight),
            KeyCode::Home => Some(Self::MoveLineStart),
            KeyCode::End => Some(Self::MoveLineEnd),
            KeyCode::Backspace => Some(Self::DeleteBackward),
            KeyCode::Delete => Some(Self::DeleteForward),
            KeyCode::Char(c) if !c.is_control() => Some(Self::Insert(c)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TextInputPolicy {
    pub max_chars: Option<usize>,
    pub sanitize: Option<fn(char) -> Option<char>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextInputEdit {
    pub changed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TextInput {
    pub value: String,
    pub cursor: usize,
}

impl TextInput {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.chars().count();
        Self { value, cursor }
    }

    pub fn set(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.chars().count();
    }

    pub fn is_blank(&self) -> bool {
        self.value.trim().is_empty()
    }

    pub(crate) fn len_chars(&self) -> usize {
        self.value.chars().count()
    }

    pub(crate) fn byte_index(line: &str, col: usize) -> usize {
        line.char_indices()
            .nth(col)
            .map(|(i, _)| i)
            .unwrap_or(line.len())
    }

    pub(crate) fn apply_key(&mut self, key: KeyEvent) -> Option<TextInputEdit> {
        self.apply_key_with_policy(key, TextInputPolicy::default())
    }

    pub(crate) fn apply_key_with_policy(
        &mut self,
        key: KeyEvent,
        policy: TextInputPolicy,
    ) -> Option<TextInputEdit> {
        let command = TextEditCommand::from_key(key)?;
        Some(TextInputEdit {
            changed: self.apply_command(command, policy),
        })
    }

    pub(crate) fn apply_command(
        &mut self,
        command: TextEditCommand,
        policy: TextInputPolicy,
    ) -> bool {
        self.clamp_cursor();
        match command {
            TextEditCommand::MoveLeft => self.move_left(),
            TextEditCommand::MoveRight => self.move_right(),
            TextEditCommand::MoveLineStart => self.move_home(),
            TextEditCommand::MoveLineEnd => self.move_end(),
            TextEditCommand::MoveWordLeft => self.move_word_left(),
            TextEditCommand::MoveWordRight => self.move_word_right(),
            TextEditCommand::DeleteBackward => self.backspace(),
            TextEditCommand::DeleteForward => self.delete(),
            TextEditCommand::DeleteToLineStart => self.delete_to_line_start(),
            TextEditCommand::DeleteToLineEnd => self.delete_to_line_end(),
            TextEditCommand::DeleteWordBackward => self.delete_word_backward(),
            TextEditCommand::Insert(c) => {
                let Some(c) = policy.sanitize.map_or(Some(c), |sanitize| sanitize(c)) else {
                    return false;
                };
                if policy
                    .max_chars
                    .is_some_and(|max_chars| self.len_chars() >= max_chars)
                {
                    false
                } else {
                    self.insert_char(c)
                }
            }
        }
    }

    fn clamp_cursor(&mut self) {
        self.cursor = self.cursor.min(self.len_chars());
    }

    pub fn move_left(&mut self) -> bool {
        let before = self.cursor;
        self.cursor = self.cursor.saturating_sub(1);
        self.cursor != before
    }

    pub fn move_right(&mut self) -> bool {
        let before = self.cursor;
        let len = self.len_chars();
        self.cursor = (self.cursor + 1).min(len);
        self.cursor != before
    }

    pub fn move_home(&mut self) -> bool {
        let before = self.cursor;
        self.cursor = 0;
        self.cursor != before
    }

    pub fn move_end(&mut self) -> bool {
        let before = self.cursor;
        self.cursor = self.len_chars();
        self.cursor != before
    }

    pub(crate) fn move_word_left(&mut self) -> bool {
        let before = self.cursor;
        self.cursor = previous_word_boundary(&self.value, self.cursor);
        self.cursor != before
    }

    pub(crate) fn move_word_right(&mut self) -> bool {
        let before = self.cursor;
        self.cursor = next_word_boundary(&self.value, self.cursor);
        self.cursor != before
    }

    pub fn insert_char(&mut self, c: char) -> bool {
        let idx = Self::byte_index(&self.value, self.cursor);
        self.value.insert(idx, c);
        self.cursor += 1;
        true
    }

    pub fn backspace(&mut self) -> bool {
        if self.cursor == 0 || self.value.is_empty() {
            return false;
        }
        let start = Self::byte_index(&self.value, self.cursor.saturating_sub(1));
        let end = Self::byte_index(&self.value, self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor = self.cursor.saturating_sub(1);
        true
    }

    pub fn delete(&mut self) -> bool {
        let len = self.len_chars();
        if self.value.is_empty() || self.cursor >= len {
            return false;
        }
        let start = Self::byte_index(&self.value, self.cursor);
        let end = Self::byte_index(&self.value, self.cursor + 1);
        self.value.replace_range(start..end, "");
        true
    }

    pub(crate) fn delete_to_line_start(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let end = Self::byte_index(&self.value, self.cursor);
        self.value.replace_range(0..end, "");
        self.cursor = 0;
        true
    }

    pub(crate) fn delete_to_line_end(&mut self) -> bool {
        let len = self.len_chars();
        if self.cursor >= len {
            return false;
        }
        let start = Self::byte_index(&self.value, self.cursor);
        self.value.replace_range(start.., "");
        true
    }

    pub(crate) fn delete_word_backward(&mut self) -> bool {
        let start_cursor = previous_word_boundary(&self.value, self.cursor);
        if start_cursor == self.cursor {
            return false;
        }
        let start = Self::byte_index(&self.value, start_cursor);
        let end = Self::byte_index(&self.value, self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor = start_cursor;
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharKind {
    Whitespace,
    Word,
    Other,
}

fn char_kind(c: char) -> CharKind {
    if c.is_whitespace() {
        CharKind::Whitespace
    } else if c.is_alphanumeric() || c == '_' {
        CharKind::Word
    } else {
        CharKind::Other
    }
}

pub(crate) fn previous_word_boundary(text: &str, cursor: usize) -> usize {
    let chars = text.chars().collect::<Vec<_>>();
    let mut idx = cursor.min(chars.len());

    while idx > 0 && char_kind(chars[idx - 1]) == CharKind::Whitespace {
        idx -= 1;
    }

    if idx == 0 {
        return 0;
    }

    let target = char_kind(chars[idx - 1]);
    while idx > 0 && char_kind(chars[idx - 1]) == target {
        idx -= 1;
    }

    idx
}

pub(crate) fn next_word_boundary(text: &str, cursor: usize) -> usize {
    let chars = text.chars().collect::<Vec<_>>();
    let mut idx = cursor.min(chars.len());

    while idx < chars.len() && char_kind(chars[idx]) == CharKind::Whitespace {
        idx += 1;
    }

    if idx >= chars.len() {
        return chars.len();
    }

    let target = char_kind(chars[idx]);
    while idx < chars.len() && char_kind(chars[idx]) == target {
        idx += 1;
    }

    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    #[test]
    fn readline_line_movement_and_deletion_work() {
        let mut input = TextInput::new("alpha beta");
        input.apply_key(ctrl(KeyCode::Char('a')));
        assert_eq!(input.cursor, 0);

        input.apply_key(ctrl(KeyCode::Char('e')));
        assert_eq!(input.cursor, "alpha beta".chars().count());

        input.apply_key(ctrl(KeyCode::Char('w')));
        assert_eq!(input.value, "alpha ");
        assert_eq!(input.cursor, "alpha ".chars().count());

        input.apply_key(ctrl(KeyCode::Char('u')));
        assert_eq!(input.value, "");
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn word_movement_handles_punctuation_and_unicode() {
        let mut input = TextInput::new("你好 model-name 🚀");

        input.apply_key(alt(KeyCode::Char('b')));
        assert_eq!(input.cursor, "你好 model-name ".chars().count());

        input.apply_key(alt(KeyCode::Char('b')));
        assert_eq!(input.cursor, "你好 model-".chars().count());

        input.apply_key(alt(KeyCode::Char('f')));
        assert_eq!(input.cursor, "你好 model-name".chars().count());
    }

    #[test]
    fn max_chars_policy_handles_insert_without_changing() {
        let mut input = TextInput::new("abc");
        let edit = input
            .apply_key_with_policy(
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
                TextInputPolicy {
                    max_chars: Some(3),
                    sanitize: None,
                },
            )
            .expect("printable input should be handled");

        assert!(!edit.changed);
        assert_eq!(input.value, "abc");
    }

    #[test]
    fn apply_command_clamps_external_cursor_state() {
        let mut input = TextInput {
            value: "abc".to_string(),
            cursor: 99,
        };

        input.apply_key(ctrl(KeyCode::Char('w')));

        assert_eq!(input.value, "");
        assert_eq!(input.cursor, 0);
    }
}
