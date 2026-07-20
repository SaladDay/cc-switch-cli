use super::*;

/// Exact soft wrapping is useful for ordinary configuration and prompt lines,
/// but it must not turn a pathological single physical line into an O(n)
/// render loop. Larger lines keep their complete text and edit semantics while
/// the TUI shows a bounded horizontal window around the cursor.
const EDITOR_EXACT_WRAP_MAX_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKind {
    Plain,
    Json,
    Toml,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditorSubmit {
    #[allow(dead_code)]
    PromptCreate {
        id: String,
        name: String,
        description: Option<String>,
    },
    PromptEdit {
        id: String,
    },
    ProviderFormApplyJson,
    ProviderFormApplyOpenClawModels,
    ProviderFormApplyLocalProxyHeaders,
    ProviderFormApplyLocalProxyBody,
    ProviderFormApplyUsageScriptCode,
    ProviderFormApplyCodexAuth,
    ProviderFormApplyCodexConfigToml,
    ProviderAdd,
    ProviderEdit {
        id: String,
    },
    PricingEdit {
        model_id: String,
    },
    McpAdd,
    McpEdit {
        id: String,
    },
    ConfigCommonSnippet {
        app_type: AppType,
        source: CommonSnippetViewSource,
    },
    OpenClawWorkspaceFile {
        filename: String,
    },
    OpenClawDailyMemoryFile {
        filename: String,
    },
    HermesMemory {
        kind: crate::hermes_config::MemoryKind,
    },
    ConfigOpenClawEnv,
    ConfigOpenClawTools,
    ConfigOpenClawAgents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorMode {
    Edit,
}

#[derive(Debug, Clone)]
pub struct EditorState {
    pub title: String,
    pub kind: EditorKind,
    pub submit: EditorSubmit,
    pub mode: EditorMode,
    pub lines: Vec<String>,
    pub scroll: usize,
    /// Wrapped-line offset within `scroll`. A physical line can occupy many
    /// terminal rows, so a row-only scroll position cannot keep the cursor
    /// visible on long unbroken input.
    pub scroll_subline: usize,
    pub cursor_row: usize,
    /// UTF-8 byte offset within `cursor_row`.
    pub cursor_col: usize,
    pub initial_text: String,
}

impl EditorState {
    pub fn new(
        title: impl Into<String>,
        kind: EditorKind,
        submit: EditorSubmit,
        initial: impl Into<String>,
    ) -> Self {
        let initial_text = initial.into();
        let mut lines = initial_text
            .lines()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        if lines.is_empty() {
            lines.push(String::new());
        }

        Self {
            title: title.into(),
            kind,
            submit,
            mode: EditorMode::Edit,
            lines,
            scroll: 0,
            scroll_subline: 0,
            cursor_row: 0,
            cursor_col: 0,
            initial_text,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.text().trim_end() != self.initial_text.trim_end()
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub(crate) fn replace_text(&mut self, updated: impl Into<String>) {
        let updated = updated.into();
        let mut lines = updated.lines().map(|s| s.to_string()).collect::<Vec<_>>();
        if lines.is_empty() {
            lines.push(String::new());
        }

        self.lines = lines;
        self.cursor_row = self.cursor_row.min(self.lines.len().saturating_sub(1));
        self.cursor_col = self.clamped_cursor_byte(self.cursor_row, self.cursor_col);
        self.scroll = self.scroll.min(self.cursor_row);
        self.scroll_subline = 0;
    }

    fn line_len_bytes(&self, row: usize) -> usize {
        self.lines.get(row).map(String::len).unwrap_or(0)
    }

    fn clamped_cursor_byte(&self, row: usize, cursor: usize) -> usize {
        self.lines
            .get(row)
            .map_or(0, |line| TextInput::clamp_byte_boundary(line, cursor))
    }

    fn cursor_byte_for_vertical_move(&self, target_row: usize) -> usize {
        let Some(source) = self.lines.get(self.cursor_row) else {
            return 0;
        };
        let Some(target) = self.lines.get(target_row) else {
            return 0;
        };
        let source_cursor = TextInput::clamp_byte_boundary(source, self.cursor_col);

        if !Self::uses_bounded_line_window(source) && !Self::uses_bounded_line_window(target) {
            let char_column = source[..source_cursor].chars().count();
            return target
                .char_indices()
                .nth(char_column)
                .map(|(byte, _)| byte)
                .unwrap_or(target.len());
        }

        // Exact character-column mapping would defeat the giant-line safety
        // budget. A byte-clamped column is deterministic and keeps navigation
        // bounded for those exceptional lines.
        TextInput::clamp_byte_boundary(target, source_cursor)
    }

    fn uses_bounded_line_window(line: &str) -> bool {
        line.len() > EDITOR_EXACT_WRAP_MAX_BYTES
    }

    fn bounded_line_window(line: &str, cursor: usize, width: u16) -> (String, u16) {
        let width = width as usize;
        if width == 0 {
            return (String::new(), 0);
        }

        let cursor = TextInput::clamp_byte_boundary(line, cursor);
        let mut start = cursor;
        let mut before_width = 0usize;
        for (byte, ch) in line[..cursor].char_indices().rev() {
            let char_width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
            if before_width.saturating_add(char_width) > width.saturating_sub(1) {
                break;
            }
            start = byte;
            before_width = before_width.saturating_add(char_width);
        }

        let mut visible = String::new();
        let mut visible_width = 0usize;
        for ch in line[start..].chars().take(width.saturating_add(16)) {
            let char_width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
            if visible_width.saturating_add(char_width) > width {
                break;
            }
            visible.push(ch);
            visible_width = visible_width.saturating_add(char_width);
        }

        (visible, before_width.min(width.saturating_sub(1)) as u16)
    }

    pub(crate) fn wrap_line_segments(line: &str, width: u16) -> Vec<String> {
        let width = width as usize;
        if width == 0 {
            return vec![String::new()];
        }
        if Self::uses_bounded_line_window(line) {
            return vec![Self::bounded_line_window(line, 0, width as u16).0];
        }

        let mut segments = Vec::new();
        let mut current = String::new();
        let mut current_width = 0usize;

        for ch in line.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
            if current_width.saturating_add(ch_width) > width && !current.is_empty() {
                segments.push(current);
                current = String::new();
                current_width = 0;
            }

            current.push(ch);
            current_width = current_width.saturating_add(ch_width);
        }

        segments.push(current);

        segments
    }

    pub(crate) fn wrapped_line_height(line: &str, width: u16) -> usize {
        let width = width as usize;
        if width == 0 || line.is_empty() {
            return 1;
        }
        if Self::uses_bounded_line_window(line) {
            return 1;
        }
        if line.is_ascii() {
            return line.len().div_ceil(width).max(1);
        }

        let mut height = 1usize;
        let mut current_width = 0usize;
        for ch in line.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
            if current_width.saturating_add(ch_width) > width && current_width > 0 {
                height = height.saturating_add(1);
                current_width = 0;
            }
            current_width = current_width.saturating_add(ch_width);
        }
        height
    }

    pub(crate) fn wrapped_cursor_subline_and_x(
        line: &str,
        width: u16,
        cursor_col: usize,
    ) -> (usize, u16) {
        let width = width as usize;
        if width == 0 {
            return (0, 0);
        }

        let cursor_col = TextInput::clamp_byte_boundary(line, cursor_col);
        if Self::uses_bounded_line_window(line) {
            return (
                0,
                Self::bounded_line_window(line, cursor_col, width as u16).1,
            );
        }

        if line.is_ascii() {
            let cursor_col = cursor_col.min(line.len());
            if cursor_col == 0 {
                return (0, 0);
            }
            let subline = cursor_col.saturating_sub(1) / width;
            let current_width = ((cursor_col.saturating_sub(1) % width) + 1).min(width);
            return (subline, current_width.min(width.saturating_sub(1)) as u16);
        }

        let mut subline = 0usize;
        let mut current_width = 0usize;
        for ch in line[..cursor_col].chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
            if current_width.saturating_add(ch_width) > width && current_width > 0 {
                subline = subline.saturating_add(1);
                current_width = 0;
            }
            current_width = current_width.saturating_add(ch_width);
        }

        let x = current_width.min(width.saturating_sub(1)) as u16;
        (subline, x)
    }

    #[cfg(test)]
    pub(crate) fn cursor_visual_offset_from_scroll(&self, width: u16) -> (usize, u16) {
        self.cursor_visual_offset_from_origin(width, self.scroll, self.scroll_subline)
    }

    pub(crate) fn cursor_visual_offset_from_origin(
        &self,
        width: u16,
        scroll: usize,
        scroll_subline: usize,
    ) -> (usize, u16) {
        if self.lines.is_empty() {
            return (0, 0);
        }

        let cursor_row = self.cursor_row.min(self.lines.len().saturating_sub(1));
        let scroll = scroll
            .min(self.lines.len().saturating_sub(1))
            .min(cursor_row);

        let first_line_height = Self::wrapped_line_height(&self.lines[scroll], width);
        let scroll_subline = scroll_subline.min(first_line_height.saturating_sub(1));

        let mut y = 0usize;
        for row in scroll..cursor_row {
            let line_height = Self::wrapped_line_height(&self.lines[row], width);
            y = y.saturating_add(if row == scroll {
                line_height.saturating_sub(scroll_subline)
            } else {
                line_height
            });
        }

        let cursor_col = self.clamped_cursor_byte(cursor_row, self.cursor_col);
        let (subline, x) =
            Self::wrapped_cursor_subline_and_x(&self.lines[cursor_row], width, cursor_col);
        if cursor_row == scroll {
            (subline.saturating_sub(scroll_subline), x)
        } else {
            (y.saturating_add(subline), x)
        }
    }

    #[cfg(test)]
    pub(crate) fn visible_wrapped_lines(&self, width: u16, height: usize) -> Vec<String> {
        self.visible_wrapped_lines_from(width, height, self.scroll, self.scroll_subline)
    }

    pub(crate) fn visible_wrapped_lines_from(
        &self,
        width: u16,
        height: usize,
        scroll: usize,
        scroll_subline: usize,
    ) -> Vec<String> {
        if self.lines.is_empty() || height == 0 {
            return Vec::new();
        }

        let start = scroll.min(self.lines.len().saturating_sub(1));
        let mut shown = Vec::with_capacity(height);
        for (row, line) in self.lines.iter().enumerate().skip(start) {
            let skip = if row == start {
                scroll_subline.min(Self::wrapped_line_height(line, width).saturating_sub(1))
            } else {
                0
            };
            let cursor = (row == self.cursor_row).then_some(self.cursor_col);
            Self::push_visible_line_segments(line, width, skip, height, cursor, &mut shown);
            if shown.len() >= height {
                return shown;
            }
        }
        shown
    }

    fn push_visible_line_segments(
        line: &str,
        width: u16,
        skip: usize,
        max_lines: usize,
        cursor: Option<usize>,
        out: &mut Vec<String>,
    ) {
        if out.len() >= max_lines {
            return;
        }
        let width = width as usize;
        if width == 0 {
            if skip == 0 {
                out.push(String::new());
            }
            return;
        }

        if Self::uses_bounded_line_window(line) {
            if skip == 0 {
                out.push(Self::bounded_line_window(line, cursor.unwrap_or(0), width as u16).0);
            }
            return;
        }

        if line.is_ascii() {
            if line.is_empty() {
                if skip == 0 {
                    out.push(String::new());
                }
                return;
            }
            let start = skip.saturating_mul(width).min(line.len());
            for chunk in line.as_bytes()[start..].chunks(width) {
                if out.len() >= max_lines {
                    break;
                }
                out.push(String::from_utf8_lossy(chunk).into_owned());
            }
            return;
        }

        let mut segment = 0usize;
        let mut current = String::new();
        let mut current_width = 0usize;
        for ch in line.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
            if current_width.saturating_add(ch_width) > width && current_width > 0 {
                if segment >= skip {
                    out.push(std::mem::take(&mut current));
                    if out.len() >= max_lines {
                        return;
                    }
                } else {
                    current.clear();
                }
                segment = segment.saturating_add(1);
                current_width = 0;
            }
            if segment >= skip {
                current.push(ch);
            }
            current_width = current_width.saturating_add(ch_width);
        }
        if segment >= skip && out.len() < max_lines {
            out.push(current);
        }
    }

    pub(crate) fn viewport_origin(&self, viewport: Size) -> (usize, usize) {
        if self.lines.is_empty() {
            return (0, 0);
        }

        let cursor_row = self.cursor_row.min(self.lines.len() - 1);
        let cursor_col = self.clamped_cursor_byte(cursor_row, self.cursor_col);
        let mut scroll = self.scroll.min(self.lines.len() - 1).min(cursor_row);
        let mut scroll_subline = if scroll == self.scroll {
            self.scroll_subline
        } else {
            0
        };
        let height = viewport.height as usize;
        if height == 0 {
            return (scroll, scroll_subline);
        }

        // Every physical line occupies at least one terminal row. If the
        // stored origin is farther than one viewport behind the cursor, it
        // cannot possibly still be visible. Jump close to the cursor before
        // measuring wrapped heights so a stale origin (for example after a
        // programmatic cursor move in a very large prompt) never turns a
        // passive redraw into a scan of the complete document.
        if cursor_row.saturating_sub(scroll) >= height {
            scroll = cursor_row.saturating_sub(height.saturating_sub(1));
            scroll_subline = 0;
        }

        let width = viewport.width.max(1);
        let first_line_height = Self::wrapped_line_height(&self.lines[scroll], width);
        scroll_subline = scroll_subline.min(first_line_height.saturating_sub(1));
        let (cursor_subline, _) =
            Self::wrapped_cursor_subline_and_x(&self.lines[cursor_row], width, cursor_col);
        if cursor_row == scroll && cursor_subline < scroll_subline {
            scroll_subline = cursor_subline;
        }

        let (mut cursor_y, _) =
            self.cursor_visual_offset_from_origin(width, scroll, scroll_subline);
        while cursor_y >= height && scroll < cursor_row {
            let removed = Self::wrapped_line_height(&self.lines[scroll], width)
                .saturating_sub(scroll_subline);
            cursor_y = cursor_y.saturating_sub(removed);
            scroll = scroll.saturating_add(1);
            scroll_subline = 0;
        }
        if cursor_y >= height && scroll == cursor_row {
            scroll_subline = cursor_subline.saturating_sub(height.saturating_sub(1));
        }
        (scroll, scroll_subline)
    }

    pub(crate) fn ensure_cursor_visible(&mut self, viewport: Size) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);
        self.cursor_col = self.clamped_cursor_byte(self.cursor_row, self.cursor_col);

        (self.scroll, self.scroll_subline) = self.viewport_origin(viewport);
    }

    fn apply_current_line_command(&mut self, command: TextEditCommand) -> bool {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);

        let mut input = TextInput {
            value: std::mem::take(&mut self.lines[self.cursor_row]),
            cursor: self.cursor_col,
        };
        let changed = input.apply_command(command, TextInputPolicy::default());
        self.lines[self.cursor_row] = input.value;
        self.cursor_col = input.cursor;
        changed
    }

    pub(crate) fn apply_text_command(&mut self, command: TextEditCommand) -> bool {
        match command {
            TextEditCommand::MoveLeft => self.move_left(),
            TextEditCommand::MoveRight => self.move_right(),
            TextEditCommand::MoveLineStart
            | TextEditCommand::MoveLineEnd
            | TextEditCommand::DeleteToLineStart
            | TextEditCommand::DeleteToLineEnd
            | TextEditCommand::Insert(_) => self.apply_current_line_command(command),
            TextEditCommand::MoveWordLeft => self.move_word_left(),
            TextEditCommand::MoveWordRight => self.move_word_right(),
            TextEditCommand::DeleteBackward => self.backspace(),
            TextEditCommand::DeleteForward => self.delete(),
            TextEditCommand::DeleteWordBackward => self.delete_word_backward(),
        }
    }

    pub(crate) fn apply_editor_key(&mut self, key: KeyEvent, viewport: Size) -> bool {
        if let Some(command) = TextEditCommand::from_key(key) {
            self.apply_text_command(command);
            self.ensure_cursor_visible(viewport);
            return true;
        }

        let jump_rows = viewport.height as usize;
        match key.code {
            KeyCode::Up => {
                let target = self.cursor_row.saturating_sub(1);
                self.cursor_col = self.cursor_byte_for_vertical_move(target);
                self.cursor_row = target;
                self.ensure_cursor_visible(viewport);
                true
            }
            KeyCode::Down => {
                let mut target = self.cursor_row;
                if !self.lines.is_empty() {
                    target = (self.cursor_row + 1).min(self.lines.len() - 1);
                }
                self.cursor_col = self.cursor_byte_for_vertical_move(target);
                self.cursor_row = target;
                self.ensure_cursor_visible(viewport);
                true
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(jump_rows);
                self.scroll_subline = 0;
                let target = self.cursor_row.saturating_sub(jump_rows);
                self.cursor_col = self.cursor_byte_for_vertical_move(target);
                self.cursor_row = target;
                self.ensure_cursor_visible(viewport);
                true
            }
            KeyCode::PageDown => {
                if !self.lines.is_empty() {
                    self.scroll = (self.scroll + jump_rows).min(self.lines.len() - 1);
                    self.scroll_subline = 0;
                    let target = (self.cursor_row + jump_rows).min(self.lines.len() - 1);
                    self.cursor_col = self.cursor_byte_for_vertical_move(target);
                    self.cursor_row = target;
                }
                self.ensure_cursor_visible(viewport);
                true
            }
            KeyCode::Enter => {
                self.newline();
                self.ensure_cursor_visible(viewport);
                true
            }
            KeyCode::Tab => {
                self.insert_str("  ");
                self.ensure_cursor_visible(viewport);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn move_left(&mut self) -> bool {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);

        if self.cursor_col > 0 {
            self.cursor_col =
                TextInput::previous_char_boundary(&self.lines[self.cursor_row], self.cursor_col);
            return true;
        }

        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.line_len_bytes(self.cursor_row);
            return true;
        }

        false
    }

    pub(crate) fn move_right(&mut self) -> bool {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);

        let line_len = self.line_len_bytes(self.cursor_row);
        if self.cursor_col < line_len {
            self.cursor_col =
                TextInput::next_char_boundary(&self.lines[self.cursor_row], self.cursor_col);
            return true;
        }

        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
            return true;
        }

        false
    }

    fn move_word_left(&mut self) -> bool {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);
        let before = (self.cursor_row, self.cursor_col);

        loop {
            if self.cursor_col > 0 {
                let line = &self.lines[self.cursor_row];
                self.cursor_col =
                    super::super::text_edit::previous_word_boundary(line, self.cursor_col);
                break;
            }

            if self.cursor_row == 0 {
                break;
            }

            self.cursor_row -= 1;
            self.cursor_col = self.line_len_bytes(self.cursor_row);
        }

        (self.cursor_row, self.cursor_col) != before
    }

    fn move_word_right(&mut self) -> bool {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);
        let before = (self.cursor_row, self.cursor_col);

        loop {
            let line_len = self.line_len_bytes(self.cursor_row);
            if self.cursor_col < line_len {
                let line = &self.lines[self.cursor_row];
                self.cursor_col =
                    super::super::text_edit::next_word_boundary(line, self.cursor_col);
                break;
            }

            if self.cursor_row + 1 >= self.lines.len() {
                break;
            }

            self.cursor_row += 1;
            self.cursor_col = 0;
        }

        (self.cursor_row, self.cursor_col) != before
    }

    pub(crate) fn insert_char(&mut self, c: char) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);
        let line = &mut self.lines[self.cursor_row];
        self.cursor_col = TextInput::clamp_byte_boundary(line, self.cursor_col);
        line.insert(self.cursor_col, c);
        self.cursor_col = self.cursor_col.saturating_add(c.len_utf8());
    }

    pub(crate) fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert_char(c);
        }
    }

    pub(crate) fn newline(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);
        let line = &mut self.lines[self.cursor_row];
        self.cursor_col = TextInput::clamp_byte_boundary(line, self.cursor_col);
        let rest = line.split_off(self.cursor_col);
        let next_row = self.cursor_row + 1;
        self.lines.insert(next_row, rest);
        self.cursor_row = next_row;
        self.cursor_col = 0;
    }

    pub(crate) fn backspace(&mut self) -> bool {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);

        if self.cursor_col > 0 {
            let line = &mut self.lines[self.cursor_row];
            let end = TextInput::clamp_byte_boundary(line, self.cursor_col);
            let start = TextInput::previous_char_boundary(line, end);
            if start < end && end <= line.len() {
                line.replace_range(start..end, "");
                self.cursor_col = start;
                return true;
            }
            return false;
        }

        if self.cursor_row == 0 {
            return false;
        }

        let current = self.lines.remove(self.cursor_row);
        self.cursor_row -= 1;
        let prev = &mut self.lines[self.cursor_row];
        self.cursor_col = prev.len();
        prev.push_str(&current);
        true
    }

    pub(crate) fn delete(&mut self) -> bool {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);

        let line_len = self.line_len_bytes(self.cursor_row);
        if self.cursor_col < line_len {
            let line = &mut self.lines[self.cursor_row];
            let start = TextInput::clamp_byte_boundary(line, self.cursor_col);
            let end = TextInput::next_char_boundary(line, start);
            if start < end && end <= line.len() {
                line.replace_range(start..end, "");
                return true;
            }
            return false;
        }

        if self.cursor_row + 1 >= self.lines.len() {
            return false;
        }

        let next = self.lines.remove(self.cursor_row + 1);
        self.lines[self.cursor_row].push_str(&next);
        true
    }

    fn delete_word_backward(&mut self) -> bool {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);

        if self.cursor_col > 0 {
            return self.apply_current_line_command(TextEditCommand::DeleteWordBackward);
        }

        if self.cursor_row == 0 {
            return false;
        }

        self.backspace();
        self.apply_current_line_command(TextEditCommand::DeleteWordBackward)
    }
}
