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
            KeyCode::Enter => {
                self.open_pricing_edit_editor(data);
                Action::None
            }
            KeyCode::Char('d') => {
                self.open_pricing_delete_confirm(data);
                Action::None
            }
            KeyCode::Char('r') => Action::UsageRefresh,
            _ => Action::None,
        }
    }

    fn selected_pricing_row<'a>(&self, data: &'a UiData) -> Option<&'a data::ModelPricingRow> {
        let rows = visible_pricing_rows(&self.filter, data);
        rows.get(self.pricing.selected_idx).copied()
    }

    fn open_pricing_edit_editor(&mut self, data: &UiData) {
        let Some(row) = self.selected_pricing_row(data) else {
            return;
        };
        let initial = serde_json::json!({
            "model_id": row.model_id,
            "display_name": row.display_name,
            "input_cost_per_million": row.input_cost_per_million,
            "output_cost_per_million": row.output_cost_per_million,
            "cache_read_cost_per_million": row.cache_read_cost_per_million,
            "cache_creation_cost_per_million": row.cache_creation_cost_per_million,
        });
        let initial = serde_json::to_string_pretty(&initial).unwrap_or_else(|_| "{}".to_string());
        self.open_editor(
            pricing_edit_title(&row.model_id),
            EditorKind::Json,
            initial,
            EditorSubmit::PricingEdit {
                model_id: row.model_id.clone(),
            },
        );
    }

    fn open_pricing_delete_confirm(&mut self, data: &UiData) {
        let Some(row) = self.selected_pricing_row(data) else {
            return;
        };
        self.overlay = Overlay::Confirm(ConfirmOverlay {
            title: crate::t!("Delete Model Pricing", "删除模型定价").to_string(),
            message: pricing_delete_message(&row.model_id),
            action: ConfirmAction::PricingDelete {
                model_id: row.model_id.clone(),
            },
        });
    }
}

fn pricing_edit_title(model_id: &str) -> String {
    if crate::cli::i18n::is_chinese() {
        format!("编辑模型定价: {model_id}")
    } else {
        format!("Edit Model Pricing: {model_id}")
    }
}

fn pricing_delete_message(model_id: &str) -> String {
    if crate::cli::i18n::is_chinese() {
        format!("确定删除模型定价 '{model_id}'？删除后会从定价列表中隐藏。")
    } else {
        format!("Delete model pricing '{model_id}'? It will be hidden from the pricing list.")
    }
}
