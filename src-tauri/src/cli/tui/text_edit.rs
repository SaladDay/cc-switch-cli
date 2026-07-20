use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Passive TUI classification must never scan an arbitrarily large imported
/// value on every redraw. Values above this budget are treated conservatively
/// (present, non-matching) until the user explicitly edits or saves them.
pub(crate) const PASSIVE_TEXT_SCAN_MAX_BYTES: usize = 8 * 1024;

pub(crate) fn passive_text_is_blank(text: &str) -> bool {
    text.len() <= PASSIVE_TEXT_SCAN_MAX_BYTES && text.trim().is_empty()
}

pub(crate) fn passive_trimmed_eq_ignore_ascii_case(text: &str, expected: &str) -> bool {
    text.len() <= PASSIVE_TEXT_SCAN_MAX_BYTES && text.trim().eq_ignore_ascii_case(expected)
}

pub(crate) fn passive_text_contains(text: &str, needle: &str) -> bool {
    text.len() <= PASSIVE_TEXT_SCAN_MAX_BYTES && text.contains(needle)
}

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
    /// UTF-8 byte offset. Keeping the cursor on a byte boundary lets renderers
    /// take a bounded window around it without scanning from the start of a
    /// potentially huge Unicode value on every frame.
    pub cursor: usize,
}

/// A single in-place text edit. Keeping the target and the complete original
/// input together makes the field identity stable while dynamic forms rebuild
/// their rows, and gives Escape real cancel semantics (including cursor state).
#[derive(Debug, Clone)]
pub(crate) struct TextEditSession<F> {
    target: F,
    original: TextInput,
    original_error: Option<String>,
}

impl<F: Copy> TextEditSession<F> {
    pub(crate) fn new(target: F, original: TextInput, original_error: Option<String>) -> Self {
        Self {
            target,
            original,
            original_error,
        }
    }

    pub(crate) fn target(&self) -> F {
        self.target
    }

    pub(crate) fn into_parts(self) -> (F, TextInput, Option<String>) {
        (self.target, self.original, self.original_error)
    }
}

impl TextInput {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.len();
        Self { value, cursor }
    }

    pub fn set(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.len();
    }

    pub fn is_blank(&self) -> bool {
        self.value.trim().is_empty()
    }

    pub(crate) fn is_blank_for_passive_display(&self) -> bool {
        passive_text_is_blank(&self.value)
    }

    pub(crate) fn clamp_byte_boundary(text: &str, cursor: usize) -> usize {
        let mut cursor = cursor.min(text.len());
        while cursor > 0 && !text.is_char_boundary(cursor) {
            cursor -= 1;
        }
        cursor
    }

    pub(crate) fn previous_char_boundary(text: &str, cursor: usize) -> usize {
        let cursor = Self::clamp_byte_boundary(text, cursor);
        text[..cursor]
            .char_indices()
            .next_back()
            .map(|(byte, _)| byte)
            .unwrap_or(0)
    }

    pub(crate) fn next_char_boundary(text: &str, cursor: usize) -> usize {
        let cursor = Self::clamp_byte_boundary(text, cursor);
        text[cursor..]
            .chars()
            .next()
            .map(|ch| cursor.saturating_add(ch.len_utf8()))
            .unwrap_or(text.len())
    }

    fn reached_char_limit(&self, max_chars: usize) -> bool {
        // Policies are only used for deliberately short fields (currently the
        // 500-character notes field). Stop once the limit is known instead of
        // counting an arbitrarily large imported value in full.
        self.value.chars().take(max_chars).count() >= max_chars
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
                    .is_some_and(|max_chars| self.reached_char_limit(max_chars))
                {
                    false
                } else {
                    self.insert_char(c)
                }
            }
        }
    }

    fn clamp_cursor(&mut self) {
        self.cursor = Self::clamp_byte_boundary(&self.value, self.cursor);
    }

    pub fn move_left(&mut self) -> bool {
        let before = self.cursor;
        self.cursor = Self::previous_char_boundary(&self.value, self.cursor);
        self.cursor != before
    }

    pub fn move_right(&mut self) -> bool {
        let before = self.cursor;
        self.cursor = Self::next_char_boundary(&self.value, self.cursor);
        self.cursor != before
    }

    pub fn move_home(&mut self) -> bool {
        let before = self.cursor;
        self.cursor = 0;
        self.cursor != before
    }

    pub fn move_end(&mut self) -> bool {
        let before = self.cursor;
        self.cursor = self.value.len();
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
        self.clamp_cursor();
        self.value.insert(self.cursor, c);
        self.cursor = self.cursor.saturating_add(c.len_utf8());
        true
    }

    pub fn backspace(&mut self) -> bool {
        if self.cursor == 0 || self.value.is_empty() {
            return false;
        }
        self.clamp_cursor();
        let end = self.cursor;
        let start = Self::previous_char_boundary(&self.value, end);
        self.value.replace_range(start..end, "");
        self.cursor = start;
        true
    }

    pub fn delete(&mut self) -> bool {
        self.clamp_cursor();
        if self.value.is_empty() || self.cursor >= self.value.len() {
            return false;
        }
        let start = self.cursor;
        let end = Self::next_char_boundary(&self.value, start);
        self.value.replace_range(start..end, "");
        true
    }

    pub(crate) fn delete_to_line_start(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.clamp_cursor();
        let end = self.cursor;
        self.value.replace_range(0..end, "");
        self.cursor = 0;
        true
    }

    pub(crate) fn delete_to_line_end(&mut self) -> bool {
        self.clamp_cursor();
        if self.cursor >= self.value.len() {
            return false;
        }
        let start = self.cursor;
        self.value.replace_range(start.., "");
        true
    }

    pub(crate) fn delete_word_backward(&mut self) -> bool {
        self.clamp_cursor();
        let start_cursor = previous_word_boundary(&self.value, self.cursor);
        if start_cursor == self.cursor {
            return false;
        }
        self.value.replace_range(start_cursor..self.cursor, "");
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
    let mut byte = TextInput::clamp_byte_boundary(text, cursor);

    while let Some((start, ch)) = text[..byte].char_indices().next_back() {
        if char_kind(ch) != CharKind::Whitespace {
            break;
        }
        byte = start;
    }

    if byte == 0 {
        return 0;
    }

    let target = text[..byte]
        .chars()
        .next_back()
        .map(char_kind)
        .unwrap_or(CharKind::Whitespace);
    while let Some((start, ch)) = text[..byte].char_indices().next_back() {
        if char_kind(ch) != target {
            break;
        }
        byte = start;
    }

    byte
}

pub(crate) fn next_word_boundary(text: &str, cursor: usize) -> usize {
    let mut byte = TextInput::clamp_byte_boundary(text, cursor);

    while let Some(ch) = text[byte..].chars().next() {
        if char_kind(ch) != CharKind::Whitespace {
            break;
        }
        byte = byte.saturating_add(ch.len_utf8());
    }

    if byte >= text.len() {
        return text.len();
    }

    let target = text[byte..]
        .chars()
        .next()
        .map(char_kind)
        .unwrap_or(CharKind::Whitespace);
    while let Some(ch) = text[byte..].chars().next() {
        if char_kind(ch) != target {
            break;
        }
        byte = byte.saturating_add(ch.len_utf8());
    }

    byte
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
        assert_eq!(input.cursor, "alpha beta".len());

        input.apply_key(ctrl(KeyCode::Char('w')));
        assert_eq!(input.value, "alpha ");
        assert_eq!(input.cursor, "alpha ".len());

        input.apply_key(ctrl(KeyCode::Char('u')));
        assert_eq!(input.value, "");
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn word_movement_handles_punctuation_and_unicode() {
        let mut input = TextInput::new("你好 model-name 🚀");

        input.apply_key(alt(KeyCode::Char('b')));
        assert_eq!(input.cursor, "你好 model-name ".len());

        input.apply_key(alt(KeyCode::Char('b')));
        assert_eq!(input.cursor, "你好 model-".len());

        input.apply_key(alt(KeyCode::Char('f')));
        assert_eq!(input.cursor, "你好 model-name".len());
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

    #[test]
    fn edits_clamp_a_cursor_inside_a_multibyte_character() {
        let mut input = TextInput {
            value: "你a".to_string(),
            cursor: 1,
        };

        assert!(!input.delete_word_backward());
        assert_eq!(input.value, "你a");
        assert_eq!(input.cursor, 0);

        assert!(input.move_right());
        assert_eq!(input.cursor, "你".len());
        assert!(input.backspace());
        assert_eq!(input.value, "a");
        assert_eq!(input.cursor, 0);
    }
}
