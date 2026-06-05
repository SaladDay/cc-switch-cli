use super::*;

impl App {
    pub(crate) fn on_pricing_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        match key.code {
            KeyCode::Up => {
                self.pricing.selected_idx = self.pricing.selected_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                let len = visible_pricing_rows(&self.filter, data).len();
                if len > 0 {
                    self.pricing.selected_idx = (self.pricing.selected_idx + 1).min(len - 1);
                }
                Action::None
            }
            KeyCode::PageUp => {
                self.pricing.selected_idx = self.pricing.selected_idx.saturating_sub(10);
                Action::None
            }
            KeyCode::PageDown => {
                let len = visible_pricing_rows(&self.filter, data).len();
                if len > 0 {
                    self.pricing.selected_idx = (self.pricing.selected_idx + 10).min(len - 1);
                }
                Action::None
            }
            KeyCode::Enter | KeyCode::Char('d') => self.open_pricing_detail(data),
            KeyCode::Char('r') => Action::ReloadData,
            _ => Action::None,
        }
    }

    pub(crate) fn on_pricing_detail_key(&mut self, key: KeyEvent, _model_id: &str) -> Action {
        match key.code {
            KeyCode::Char('r') => Action::ReloadData,
            _ => Action::None,
        }
    }

    fn open_pricing_detail(&mut self, data: &UiData) -> Action {
        let rows = visible_pricing_rows(&self.filter, data);
        let Some(row) = rows.get(self.pricing.selected_idx) else {
            return Action::None;
        };
        self.push_route_and_switch(Route::PricingDetail {
            model_id: row.model_id.clone(),
        })
    }
}
