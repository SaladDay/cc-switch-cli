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
        let is_backtab = matches!(key.code, KeyCode::BackTab)
            || (matches!(key.code, KeyCode::Tab) && key.modifiers.contains(KeyModifiers::SHIFT));
        match key.code {
            KeyCode::Char('1') => {
                self.set_usage_range(data::UsageRangePreset::Today, data);
                Action::None
            }
            KeyCode::Char('2') => {
                self.set_usage_range(data::UsageRangePreset::SevenDays, data);
                Action::None
            }
            KeyCode::Char('3') => {
                self.set_usage_range(data::UsageRangePreset::ThirtyDays, data);
                Action::None
            }
            KeyCode::Char('C') | KeyCode::Char('c') => {
                let input = match self.usage.range {
                    data::UsageRangePreset::Custom(range) => range.label(),
                    _ => data::usage_custom_range_default_input(),
                };
                self.overlay = Overlay::TextInput(TextInputState {
                    title: usage_custom_range_title().to_string(),
                    prompt: usage_custom_range_prompt().to_string(),
                    input: TextInput::new(input),
                    submit: TextSubmit::UsageCustomRange,
                    secret: false,
                });
                Action::None
            }
            _ if is_backtab => {
                self.usage.metric = self.usage.metric.previous();
                Action::None
            }
            KeyCode::Tab => {
                self.usage.metric = self.usage.metric.next();
                Action::None
            }
            KeyCode::Char('L') => {
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
                self.push_route_and_switch(Route::UsageLogs)
            }
            KeyCode::Char('P') | KeyCode::Char('p') => {
                let pricing_len = visible_pricing_rows(&self.filter, data).len();
                self.pricing.selected_idx = if pricing_len == 0 {
                    0
                } else {
                    self.pricing.selected_idx.min(pricing_len - 1)
                };
                self.push_route_and_switch(Route::Pricing)
            }
            KeyCode::Char('r') => Action::ReloadData,
            _ => Action::None,
        }
    }

    pub(crate) fn on_usage_logs_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
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
                self.move_usage_detail_selection(data, -1);
                Action::None
            }
            KeyCode::Down => {
                self.move_usage_detail_selection(data, 1);
                Action::None
            }
            KeyCode::PageUp => {
                self.move_usage_detail_selection(data, -10);
                Action::None
            }
            KeyCode::PageDown => {
                self.move_usage_detail_selection(data, 10);
                Action::None
            }
            KeyCode::Enter if matches!(self.usage.pane, UsagePane::Recent) => {
                self.open_usage_log_detail_from_logs(data)
            }
            KeyCode::Char('r') => Action::ReloadData,
            _ => Action::None,
        }
    }

    pub(crate) fn on_usage_log_detail_key(&mut self, key: KeyEvent, _request_id: &str) -> Action {
        match key.code {
            KeyCode::Char('r') => Action::ReloadData,
            _ => Action::None,
        }
    }

    fn open_usage_log_detail_from_logs(&mut self, data: &UiData) -> Action {
        let Some(row) = data
            .usage
            .recent_logs_for(self.usage.range)
            .get(self.usage.logs_idx)
        else {
            return Action::None;
        };
        self.push_route_and_switch(Route::UsageLogDetail {
            request_id: row.request_id.clone(),
        })
    }

    fn set_usage_range(&mut self, range: data::UsageRangePreset, data: &UiData) {
        self.usage.range = range;
        clamp_usage_selected_idx(&mut self.usage, data);
    }

    fn reset_usage_detail_selection(&mut self, data: &UiData) {
        match self.usage.pane {
            UsagePane::Recent => {
                self.usage.logs_idx = 0;
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
