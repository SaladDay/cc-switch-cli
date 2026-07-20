use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::Size;
use std::collections::{HashMap, HashSet};
use unicode_width::UnicodeWidthChar;

use crate::app_config::AppType;
use crate::cli::i18n::current_language;
use crate::cli::i18n::texts;
use crate::cli::i18n::Language;
use crate::services::skill::SyncMethod;

use super::data::UiData;
use super::form::{
    ClaudeModelPickerColumn, CodexWireApi, FormFocus, FormMode, FormState, GeminiAuthType,
    McpAddField, McpAddFormState, McpTransport, PromptMetaField, PromptMetaFormState,
    ProviderAddField, ProviderAddFormState,
};
use super::route::{NavItem, Route};
use super::text_edit::{TextEditCommand, TextInput, TextInputPolicy};
use super::{data, form};

mod app_state;
mod content_config;
mod content_entities;
mod content_pricing;
mod content_skills;
mod content_usage;
mod editor_handlers;
mod editor_state;
mod form_handlers;
mod helpers;
mod menu;
mod overlay_handlers;
pub(crate) mod paged_list;
#[cfg(test)]
mod tests;
mod types;

pub(crate) use app_state::{
    Action, App, CloudSyncBackend, CloudSyncTransferIntent, ConfigItem, LocalProxySettingsItem,
    MoveDirection, ProxyVisualTransition, S3ConfigItem, SettingsItem, WebDavConfigItem,
    PROXY_HERO_TRANSITION_TICKS,
};
pub(crate) use content_config::HERMES_MEMORY_ROW_COUNT;
pub(crate) use content_usage::usage_active_pane_len;
pub use editor_state::{EditorKind, EditorMode, EditorState, EditorSubmit};
pub(crate) use helpers::*;
pub(crate) use types::{
    model_fetch_filter, retire_session_messages, retire_session_rows, SessionPageSource,
    SessionPageToken, SessionRowIdentity, MODEL_FETCH_QUERY_MAX_BYTES, MODEL_FETCH_QUERY_MAX_CHARS,
};
pub use types::{
    CommonSnippetViewSource, ConfirmAction, ConfirmOverlay, FilterScope, FilterState, Focus,
    LoadingKind, ManagedAuthLoginState, Overlay, PricingState, SessionProjectPickerState,
    SessionsPane, SessionsState, SkillsDiscoverSource, TextInputState, TextSubmit, TextViewAction,
    TextViewState, Toast, ToastKind, UsageMetric, UsagePane, UsageState,
};
#[cfg(test)]
pub(crate) use types::{McpEnvEditorField, McpEnvEntryEditorState};

pub(crate) fn supports_failover_controls(app_type: &AppType) -> bool {
    app_type.supports_failover()
}

const PROVIDER_NOTES_MAX_CHARS: usize = 120;

#[cfg(unix)]
pub(crate) fn supports_temporary_provider_launch(app_type: &AppType) -> bool {
    matches!(app_type, AppType::Claude | AppType::Codex)
}

#[cfg(not(unix))]
pub(crate) fn supports_temporary_provider_launch(_app_type: &AppType) -> bool {
    false
}
