use super::*;

mod dialogs;
mod mcp_key_value;
mod pickers;
mod views;

impl App {
    pub(crate) fn on_overlay_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        if let Some(action) = self.handle_overlay_edit_shortcut(key, data) {
            return action;
        }

        if let Some(action) = self.handle_dialog_overlay_key(key, data) {
            return action;
        }

        if let Some(action) = self.handle_view_overlay_key(key, data) {
            return action;
        }

        if let Some(action) = self.handle_mcp_key_value_overlay_key(key) {
            return action;
        }

        if let Some(action) = self.handle_picker_overlay_key(key, data) {
            return action;
        }

        Action::None
    }
}
