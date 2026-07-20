use super::*;

impl UsageMetric {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Cost => Self::Tokens,
            Self::Tokens => Self::Requests,
            Self::Requests => Self::Errors,
            Self::Errors => Self::Cost,
        }
    }

    pub(crate) fn previous(self) -> Self {
        match self {
            Self::Cost => Self::Errors,
            Self::Tokens => Self::Cost,
            Self::Requests => Self::Tokens,
            Self::Errors => Self::Requests,
        }
    }
}

impl UsagePane {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Models => Self::Providers,
            Self::Providers => Self::Recent,
            Self::Recent => Self::Models,
        }
    }

    pub(crate) fn previous(self) -> Self {
        match self {
            Self::Models => Self::Recent,
            Self::Providers => Self::Models,
            Self::Recent => Self::Providers,
        }
    }
}

impl App {
    pub(crate) fn on_usage_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        use crate::cli::tui::keymap::usage::Intent;

        // Shift+Tab may arrive as Tab+SHIFT instead of BackTab; the
        // registry matches key codes only, so pre-check the modifier form.
        if matches!(key.code, KeyCode::Tab) && key.modifiers.contains(KeyModifiers::SHIFT) {
            self.usage.metric = self.usage.metric.previous();
            return Action::None;
        }

        let Some(intent) = crate::cli::tui::keymap::usage::intent_for(key.code) else {
            return Action::None;
        };

        match intent {
            Intent::RangeToday => {
                self.set_usage_range(data::UsageRangePreset::Today, data);
                Action::None
            }
            Intent::RangeSevenDays => {
                self.set_usage_range(data::UsageRangePreset::SevenDays, data);
                Action::None
            }
            Intent::RangeThirtyDays => {
                self.set_usage_range(data::UsageRangePreset::ThirtyDays, data);
                Action::None
            }
            Intent::CustomRange => {
                let input = match self.usage.range {
                    data::UsageRangePreset::Custom(range) => range.label(),
                    _ => data::usage_custom_range_default_input(),
                };
                self.overlay = Overlay::TextInput(TextInputState {
                    title: usage_custom_range_title().to_string(),
                    prompt: usage_custom_range_prompt().to_string(),
                    input: TextInput::new(input),
                    submit: TextSubmit::UsageCustomRange,
                });
                Action::None
            }
            Intent::PrevMetric => {
                self.usage.metric = self.usage.metric.previous();
                Action::None
            }
            Intent::NextMetric => {
                self.usage.metric = self.usage.metric.next();
                Action::None
            }
            Intent::OpenLogs => {
                self.usage.pane = UsagePane::Models;
                self.usage.selected_idx = self.usage.selected_idx.min(
                    data.usage
                        .top_models_for(self.usage.range)
                        .len()
                        .saturating_sub(1),
                );
                self.usage.logs_idx = self.usage.logs_idx.min(
                    data.usage
                        .recent_logs_for(self.usage.range)
                        .len()
                        .saturating_sub(1),
                );
                self.usage.sync_log_pager(
                    &self.app_type,
                    self.usage.range,
                    data.usage.recent_logs_for(self.usage.range),
                    data.usage.logs_total_for(self.usage.range),
                );
                self.push_route_and_switch(Route::UsageLogs)
            }
            Intent::OpenPricing => {
                let pricing_len = visible_pricing_rows(&self.filter, data).len();
                self.pricing.selected_idx = if pricing_len == 0 {
                    0
                } else {
                    self.pricing.selected_idx.min(pricing_len - 1)
                };
                self.push_route_and_switch(Route::Pricing)
            }
            Intent::Reload => Action::UsageRefresh,
        }
    }

    pub(crate) fn on_usage_logs_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        use super::paged_list::{PageBoundary, PageDirection, PagedListFocus};

        self.usage.sync_log_pager(
            &self.app_type,
            self.usage.range,
            data.usage.recent_logs_for(self.usage.range),
            data.usage.logs_total_for(self.usage.range),
        );
        let is_backtab = matches!(key.code, KeyCode::BackTab)
            || (matches!(key.code, KeyCode::Tab) && key.modifiers.contains(KeyModifiers::SHIFT));
        match key.code {
            _ if is_backtab => {
                self.usage.pane = self.usage.pane.previous();
                self.reset_usage_detail_selection(data);
                Action::None
            }
            KeyCode::Tab => {
                self.usage.pane = self.usage.pane.next();
                self.reset_usage_detail_selection(data);
                Action::None
            }
            KeyCode::Up => {
                if matches!(self.usage.pane, UsagePane::Recent) {
                    self.move_usage_logs_explicit(PageDirection::Previous, 1, false);
                } else {
                    self.move_usage_detail_selection(data, -1);
                }
                Action::None
            }
            KeyCode::Down => {
                if matches!(self.usage.pane, UsagePane::Recent) {
                    self.move_usage_logs_explicit(PageDirection::Next, 1, false);
                } else {
                    self.move_usage_detail_selection(data, 1);
                }
                Action::None
            }
            KeyCode::PageUp => {
                if matches!(self.usage.pane, UsagePane::Recent) {
                    self.move_usage_logs_explicit(PageDirection::Previous, 10, true);
                } else {
                    self.move_usage_detail_selection(data, -10);
                }
                Action::None
            }
            KeyCode::PageDown => {
                if matches!(self.usage.pane, UsagePane::Recent) {
                    self.move_usage_logs_explicit(PageDirection::Next, 10, true);
                } else {
                    self.move_usage_detail_selection(data, 10);
                }
                Action::None
            }
            KeyCode::Enter if matches!(self.usage.pane, UsagePane::Recent) => {
                let boundary = match self.usage.log_pager.gate.focus() {
                    PagedListFocus::Boundary(PageBoundary::Previous) => {
                        Some(PageDirection::Previous)
                    }
                    PagedListFocus::Boundary(PageBoundary::Next) => Some(PageDirection::Next),
                    PagedListFocus::Empty | PagedListFocus::Row => None,
                };
                if let Some(direction) = boundary {
                    if !self.usage_boundary_page_available(direction) {
                        if let Some(page) = self.usage_boundary_target(direction) {
                            self.usage.log_pager.clear_page_error(page);
                        }
                        return Action::None;
                    }
                    self.usage.log_pager.clear_blocked_boundary_gesture();
                    let outcome = self.usage.log_pager.gate.enter();
                    self.apply_usage_log_paged_outcome(outcome);
                    Action::None
                } else {
                    self.open_usage_log_detail_from_logs(data)
                }
            }
            KeyCode::Char('r') => Action::UsageRefresh,
            _ => Action::None,
        }
    }

    pub(crate) fn on_usage_log_detail_key(&mut self, key: KeyEvent, rowid: i64) -> Action {
        match key.code {
            KeyCode::Char('r') => Action::UsageLogDetailRefresh { rowid },
            _ => Action::None,
        }
    }

    fn open_usage_log_detail_from_logs(&mut self, data: &UiData) -> Action {
        let first_page = data.usage.recent_logs_for(self.usage.range);
        let Some(row) = self
            .usage
            .log_pager
            .current_rows(first_page)
            .get(self.usage.logs_idx)
            .cloned()
        else {
            return Action::None;
        };
        let rowid = row.cursor_rowid;
        self.usage
            .remember_log_detail(self.app_type.clone(), self.usage.range, row);
        self.push_route_and_switch(Route::UsageLogDetail { rowid })
    }

    fn set_usage_range(&mut self, range: data::UsageRangePreset, data: &UiData) {
        if self.usage.range != range {
            self.usage.invalidate_log_pages();
        }
        self.usage.range = range;
        clamp_usage_selected_idx(&mut self.usage, data);
    }

    fn reset_usage_detail_selection(&mut self, data: &UiData) {
        match self.usage.pane {
            UsagePane::Recent => {
                self.usage.logs_idx = 0;
                let page_start = self
                    .usage
                    .log_pager
                    .current_page()
                    .saturating_mul(crate::cli::tui::data::USAGE_LOG_PAGE_SIZE);
                self.usage.log_pager.gate.select(page_start);
            }
            UsagePane::Models | UsagePane::Providers => {
                self.usage.selected_idx = 0;
            }
        }
        clamp_usage_selected_idx(&mut self.usage, data);
    }

    fn move_usage_detail_selection(&mut self, data: &UiData, delta: isize) {
        match self.usage.pane {
            UsagePane::Recent => {
                let len = data.usage.recent_logs_for(self.usage.range).len();
                self.usage.logs_idx = move_index(self.usage.logs_idx, len, delta);
            }
            UsagePane::Models | UsagePane::Providers => {
                let len = usage_active_pane_len(&self.usage.pane, self.usage.range, data);
                self.usage.selected_idx = move_index(self.usage.selected_idx, len, delta);
            }
        }
    }

    pub(crate) fn on_usage_logs_wheel(
        &mut self,
        direction: crate::cli::tui::input::ScrollDirection,
        steps: u32,
        gesture: crate::cli::tui::input::WheelGestureId,
        data: &UiData,
    ) -> Action {
        use super::paged_list::PageDirection;

        if !matches!(self.usage.pane, UsagePane::Recent) {
            let delta = isize::try_from(steps).unwrap_or(isize::MAX);
            self.move_usage_detail_selection(
                data,
                match direction {
                    crate::cli::tui::input::ScrollDirection::Up => -delta,
                    crate::cli::tui::input::ScrollDirection::Down => delta,
                },
            );
            return Action::None;
        }

        self.usage.sync_log_pager(
            &self.app_type,
            self.usage.range,
            data.usage.recent_logs_for(self.usage.range),
            data.usage.logs_total_for(self.usage.range),
        );
        let direction = PageDirection::from(direction);
        if let Some(page) = self.usage_boundary_target(direction) {
            if self
                .usage
                .log_pager
                .boundary_gesture_is_blocked(page, gesture)
            {
                return Action::None;
            }
            if !self.usage.log_pager.page_is_available(page) {
                // A failed boundary consumes one fresh gesture to request a
                // retry. Keep that gesture blocked so a fast wheel burst can
                // never cross as soon as the retry completes.
                if self.usage.log_pager.page_error(page).is_some() {
                    self.usage.log_pager.clear_page_error(page);
                }
                self.usage.log_pager.block_boundary_gesture(page, gesture);
                return Action::None;
            }
            self.usage.log_pager.clear_blocked_boundary_gesture();
        }
        let outcome = self.usage.log_pager.gate.wheel(
            direction,
            gesture,
            usize::try_from(steps).unwrap_or(usize::MAX),
        );
        self.apply_usage_log_paged_outcome(outcome);
        Action::None
    }

    fn move_usage_logs_explicit(
        &mut self,
        direction: super::paged_list::PageDirection,
        steps: usize,
        retry_failed: bool,
    ) {
        if let Some(page) = self.usage_boundary_target(direction) {
            if !self.usage.log_pager.page_is_available(page) {
                if retry_failed {
                    self.usage.log_pager.clear_page_error(page);
                }
                return;
            }
            self.usage.log_pager.clear_blocked_boundary_gesture();
        }
        let outcome = self.usage.log_pager.gate.lines(direction, steps);
        self.apply_usage_log_paged_outcome(outcome);
    }

    fn apply_usage_log_paged_outcome(&mut self, _outcome: super::paged_list::PagedListOutcome) {
        let selected = self.usage.log_pager.gate.selected_index().unwrap_or(0);
        self.usage.logs_idx = selected % crate::cli::tui::data::USAGE_LOG_PAGE_SIZE;
    }

    fn usage_boundary_target(&self, direction: super::paged_list::PageDirection) -> Option<usize> {
        use super::paged_list::{PageBoundary, PageDirection, PagedListFocus};

        let matches_boundary = matches!(
            (self.usage.log_pager.gate.focus(), direction),
            (
                PagedListFocus::Boundary(PageBoundary::Previous),
                PageDirection::Previous
            ) | (
                PagedListFocus::Boundary(PageBoundary::Next),
                PageDirection::Next
            )
        );
        if !matches_boundary {
            return None;
        }
        match direction {
            PageDirection::Previous => self.usage.log_pager.current_page().checked_sub(1),
            PageDirection::Next => self.usage.log_pager.current_page().checked_add(1),
        }
    }

    fn usage_boundary_page_available(&self, direction: super::paged_list::PageDirection) -> bool {
        self.usage_boundary_target(direction)
            .is_none_or(|page| self.usage.log_pager.page_is_available(page))
    }
}

fn usage_custom_range_title() -> &'static str {
    if crate::cli::i18n::is_chinese() {
        "自定义时间区间"
    } else {
        "Custom Range"
    }
}

fn usage_custom_range_prompt() -> &'static str {
    if crate::cli::i18n::is_chinese() {
        "格式：YYYY-MM-DD..YYYY-MM-DD"
    } else {
        "Format: YYYY-MM-DD..YYYY-MM-DD"
    }
}

pub(crate) fn usage_active_pane_len(
    pane: &UsagePane,
    range: data::UsageRangePreset,
    data: &UiData,
) -> usize {
    match pane {
        UsagePane::Providers => data.usage.top_providers_for(range).len(),
        UsagePane::Models => data.usage.top_models_for(range).len(),
        UsagePane::Recent => data.usage.recent_logs_for(range).len(),
    }
}

pub(crate) fn clamp_usage_selected_idx(usage: &mut UsageState, data: &UiData) {
    let len = usage_active_pane_len(&usage.pane, usage.range, data);
    if len == 0 {
        usage.selected_idx = 0;
    } else {
        usage.selected_idx = usage.selected_idx.min(len - 1);
    }
}

fn move_index(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }

    if delta.is_negative() {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        current.saturating_add(delta as usize).min(len - 1)
    }
}
