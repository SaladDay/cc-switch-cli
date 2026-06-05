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
}

impl UsagePane {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Providers => Self::Models,
            Self::Models => Self::Recent,
            Self::Recent => Self::Providers,
        }
    }

    pub(crate) fn previous(self) -> Self {
        match self {
            Self::Providers => Self::Recent,
            Self::Models => Self::Providers,
            Self::Recent => Self::Models,
        }
    }
}

impl App {
    pub(crate) fn on_usage_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
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
            KeyCode::Char('m') => {
                self.usage.metric = self.usage.metric.next();
                Action::None
            }
            KeyCode::Tab | KeyCode::Right => {
                self.usage.pane = self.usage.pane.next();
                self.usage.selected_idx = 0;
                Action::None
            }
            KeyCode::BackTab | KeyCode::Left => {
                self.usage.pane = self.usage.pane.previous();
                self.usage.selected_idx = 0;
                Action::None
            }
            KeyCode::Up => {
                self.usage.selected_idx = self.usage.selected_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                let len = usage_active_pane_len(&self.usage.pane, self.usage.range, data);
                if len > 0 {
                    self.usage.selected_idx = (self.usage.selected_idx + 1).min(len - 1);
                }
                Action::None
            }
            KeyCode::Char('L') => {
                self.usage.logs_idx = self
                    .usage
                    .logs_idx
                    .min(data.usage.recent_logs.len().saturating_sub(1));
                self.push_route_and_switch(Route::UsageLogs)
            }
            KeyCode::Enter => match self.usage.pane {
                UsagePane::Recent => self.open_usage_log_detail(data),
                _ => Action::None,
            },
            KeyCode::Char('r') => Action::ReloadData,
            _ => Action::None,
        }
    }

    pub(crate) fn on_usage_logs_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        match key.code {
            KeyCode::Up => {
                self.usage.logs_idx = self.usage.logs_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !data.usage.recent_logs.is_empty() {
                    self.usage.logs_idx =
                        (self.usage.logs_idx + 1).min(data.usage.recent_logs.len() - 1);
                }
                Action::None
            }
            KeyCode::PageUp => {
                self.usage.logs_idx = self.usage.logs_idx.saturating_sub(10);
                Action::None
            }
            KeyCode::PageDown => {
                if !data.usage.recent_logs.is_empty() {
                    self.usage.logs_idx =
                        (self.usage.logs_idx + 10).min(data.usage.recent_logs.len() - 1);
                }
                Action::None
            }
            KeyCode::Enter | KeyCode::Char('d') => self.open_usage_log_detail_from_logs(data),
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

    fn open_usage_log_detail(&mut self, data: &UiData) -> Action {
        let Some(row) = data.usage.recent_logs.get(self.usage.selected_idx) else {
            return Action::None;
        };
        self.push_route_and_switch(Route::UsageLogDetail {
            request_id: row.request_id.clone(),
        })
    }

    fn open_usage_log_detail_from_logs(&mut self, data: &UiData) -> Action {
        let Some(row) = data.usage.recent_logs.get(self.usage.logs_idx) else {
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
}

pub(crate) fn usage_active_pane_len(
    pane: &UsagePane,
    range: data::UsageRangePreset,
    data: &UiData,
) -> usize {
    match pane {
        UsagePane::Providers => data.usage.top_providers_for(range).len(),
        UsagePane::Models => data.usage.top_models_for(range).len(),
        UsagePane::Recent => data.usage.recent_logs.len().min(8),
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
