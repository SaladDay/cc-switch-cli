use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKind {
    Plain,
    Json,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditorSubmit {
    PromptEdit { id: String },
    ProviderFormApplyJson,
    ProviderFormApplyOpenClawModels,
    ProviderFormApplyCodexAuth,
    ProviderFormApplyCodexConfigToml,
    ProviderAdd,
    ProviderEdit { id: String },
    McpAdd,
    McpEdit { id: String },
    ConfigCommonSnippet { app_type: AppType },
    OpenClawWorkspaceFile { filename: String },
    OpenClawDailyMemoryFile { filename: String },
    ConfigOpenClawEnv,
    ConfigOpenClawTools,
    ConfigOpenClawAgents,
    ConfigWebDavSettings,
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
    pub cursor_row: usize,
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
        self.cursor_col = self.cursor_col.min(self.line_len_chars(self.cursor_row));
        self.scroll = self.scroll.min(self.cursor_row);
    }

    pub(crate) fn line_len_chars(&self, row: usize) -> usize {
        self.lines.get(row).map(|s| s.chars().count()).unwrap_or(0)
    }

    pub(crate) fn wrap_line_segments(line: &str, width: u16) -> Vec<String> {
        let width = width as usize;
        if width == 0 {
            return vec![String::new()];
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

        if segments.is_empty() {
            segments.push(current);
        } else {
            segments.push(current);
        }

        segments
    }

    pub(crate) fn wrapped_line_height(line: &str, width: u16) -> usize {
        Self::wrap_line_segments(line, width).len().max(1)
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

        let mut subline = 0usize;
        let mut current_width = 0usize;
        let mut col = 0usize;

        for ch in line.chars() {
            if col >= cursor_col {
                break;
            }

            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
            if current_width.saturating_add(ch_width) > width && current_width > 0 {
                subline = subline.saturating_add(1);
                current_width = 0;
            }

            current_width = current_width.saturating_add(ch_width);
            col = col.saturating_add(1);
        }

        let x = current_width.min(width.saturating_sub(1)) as u16;
        (subline, x)
    }

    pub(crate) fn cursor_visual_offset_from_scroll(&self, width: u16) -> (usize, u16) {
        if self.lines.is_empty() {
            return (0, 0);
        }

        let cursor_row = self.cursor_row.min(self.lines.len().saturating_sub(1));
        let scroll = self
            .scroll
            .min(self.lines.len().saturating_sub(1))
            .min(cursor_row);

        let mut y = 0usize;
        for row in scroll..cursor_row {
            y = y.saturating_add(Self::wrapped_line_height(&self.lines[row], width));
        }

        let cursor_col = self.cursor_col.min(self.line_len_chars(cursor_row));
        let (subline, x) =
            Self::wrapped_cursor_subline_and_x(&self.lines[cursor_row], width, cursor_col);
        (y.saturating_add(subline), x)
    }

    pub(crate) fn ensure_cursor_visible(&mut self, viewport: Size) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);
        self.cursor_col = self.cursor_col.min(self.line_len_chars(self.cursor_row));

        if !self.lines.is_empty() {
            self.scroll = self.scroll.min(self.lines.len() - 1);
        } else {
            self.scroll = 0;
        }

        if self.cursor_row < self.scroll {
            self.scroll = self.cursor_row;
        }

        let height = viewport.height as usize;
        if height == 0 {
            return;
        }

        let width = viewport.width.max(1);

        let (mut cursor_y, _) = self.cursor_visual_offset_from_scroll(width);
        while cursor_y >= height && self.scroll < self.cursor_row {
            let removed = Self::wrapped_line_height(&self.lines[self.scroll], width);
            cursor_y = cursor_y.saturating_sub(removed);
            self.scroll = self.scroll.saturating_add(1);
        }
    }

    pub(crate) fn byte_index(line: &str, col: usize) -> usize {
        line.char_indices()
            .nth(col)
            .map(|(i, _)| i)
            .unwrap_or(line.len())
    }

    pub(crate) fn insert_char(&mut self, c: char) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);
        let line = &mut self.lines[self.cursor_row];
        let idx = Self::byte_index(line, self.cursor_col);
        line.insert(idx, c);
        self.cursor_col += 1;
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
        let idx = Self::byte_index(line, self.cursor_col);
        let rest = line.split_off(idx);
        let next_row = self.cursor_row + 1;
        self.lines.insert(next_row, rest);
        self.cursor_row = next_row;
        self.cursor_col = 0;
    }

    pub(crate) fn backspace(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);

        if self.cursor_col > 0 {
            let line = &mut self.lines[self.cursor_row];
            let start = Self::byte_index(line, self.cursor_col.saturating_sub(1));
            let end = Self::byte_index(line, self.cursor_col);
            if start < end && end <= line.len() {
                line.replace_range(start..end, "");
                self.cursor_col -= 1;
            }
            return;
        }

        if self.cursor_row == 0 {
            return;
        }

        let current = self.lines.remove(self.cursor_row);
        self.cursor_row -= 1;
        let prev = &mut self.lines[self.cursor_row];
        self.cursor_col = prev.chars().count();
        prev.push_str(&current);
    }

    pub(crate) fn delete(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);

        let line_len = self.line_len_chars(self.cursor_row);
        if self.cursor_col < line_len {
            let line = &mut self.lines[self.cursor_row];
            let start = Self::byte_index(line, self.cursor_col);
            let end = Self::byte_index(line, self.cursor_col + 1);
            if start < end && end <= line.len() {
                line.replace_range(start..end, "");
            }
            return;
        }

        if self.cursor_row + 1 >= self.lines.len() {
            return;
        }

        let next = self.lines.remove(self.cursor_row + 1);
        self.lines[self.cursor_row].push_str(&next);
    }
}
