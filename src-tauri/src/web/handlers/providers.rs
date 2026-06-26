//! Provider commands (`src/lib/api/providers.ts`).
//!
//! Follows the [`super::meta`] template. The `provider-switched` SSE event for
//! `switch_provider` is emitted by the dispatch boundary, not here.

use serde_json::{json, Value};

use super::common::{app, from_arg, str_arg, to_value};
use crate::services::provider::ProviderSortUpdate;
use crate::web::error::WebError;
use crate::{AppState, Provider, ProviderService};

pub fn dispatch(state: &AppState, command: &str, args: &Value) -> Option<Result<Value, WebError>> {
    Some(match command {
        "get_providers" => match app(args) {
            Ok(t) => ProviderService::list(state, t)
                .map_err(WebError::Domain)
                .and_then(to_value),
            Err(e) => Err(e),
        },

        "get_current_provider" => match app(args) {
            Ok(t) => ProviderService::current(state, t)
                .map(Value::String)
                .map_err(WebError::Domain),
            Err(e) => Err(e),
        },

        "switch_provider" => match (app(args), str_arg(args, "id")) {
            (Ok(t), Ok(id)) => ProviderService::switch(state, t, id)
                // TS expects SwitchResult = { warnings: string[] }.
                .map(|_| json!({ "warnings": [] }))
                .map_err(WebError::Domain),
            (Err(e), _) | (_, Err(e)) => Err(e),
        },

        "add_provider" => match (app(args), from_arg::<Provider>(args, "provider")) {
            (Ok(t), Ok(provider)) => ProviderService::add(state, t, provider)
                .map(Value::Bool)
                .map_err(WebError::Domain),
            (Err(e), _) | (_, Err(e)) => Err(e),
        },

        "update_provider" => match (app(args), from_arg::<Provider>(args, "provider")) {
            (Ok(t), Ok(provider)) => ProviderService::update(state, t, provider)
                .map(Value::Bool)
                .map_err(WebError::Domain),
            (Err(e), _) | (_, Err(e)) => Err(e),
        },

        "delete_provider" => match (app(args), str_arg(args, "id")) {
            (Ok(t), Ok(id)) => ProviderService::delete(state, t, id)
                .map(|_| Value::Bool(true))
                .map_err(WebError::Domain),
            (Err(e), _) | (_, Err(e)) => Err(e),
        },

        "remove_provider_from_live_config" => match (app(args), str_arg(args, "id")) {
            (Ok(t), Ok(id)) => ProviderService::remove_from_live_config(state, t, id)
                .map(|_| Value::Bool(true))
                .map_err(WebError::Domain),
            (Err(e), _) | (_, Err(e)) => Err(e),
        },

        "import_default_config" => match app(args) {
            Ok(t) => ProviderService::import_default_config(state, t)
                .map(Value::Bool)
                .map_err(WebError::Domain),
            Err(e) => Err(e),
        },

        "update_providers_sort_order" => {
            match (
                app(args),
                from_arg::<Vec<ProviderSortUpdate>>(args, "updates"),
            ) {
                (Ok(t), Ok(updates)) => ProviderService::update_sort_order(state, t, updates)
                    .map(Value::Bool)
                    .map_err(WebError::Domain),
                (Err(e), _) | (_, Err(e)) => Err(e),
            }
        }

        "import_opencode_providers_from_live" => {
            ProviderService::import_opencode_providers_from_live(state)
                .map(|n| json!(n))
                .map_err(WebError::Domain)
        }

        "import_openclaw_providers_from_live" => {
            ProviderService::import_openclaw_providers_from_live(state)
                .map(|n| json!(n))
                .map_err(WebError::Domain)
        }

        "import_hermes_providers_from_live" => {
            ProviderService::import_hermes_providers_from_live(state)
                .map(|n| json!(n))
                .map_err(WebError::Domain)
        }

        // Single source of truth for provider presets: the web dashboard fetches
        // the exact same per-app template set the cc-switch-cli TUI offers,
        // built from the shared `ProviderAddFormState` machinery so the desktop
        // frontend's much larger commercial catalog is replaced by the CLI's.
        "get_provider_presets" => match app(args) {
            Ok(t) => {
                to_value(crate::cli::tui::form::provider_templates::provider_presets_for_app(&t))
            }
            Err(e) => Err(e),
        },

        // update_tray_menu is handled by `meta`. Desktop-only / not-yet-mapped
        // commands (claude_desktop_*, universal_*, open_provider_terminal,
        // get_*_live_provider_ids) fall through to 501.
        _ => return None,
    })
}
