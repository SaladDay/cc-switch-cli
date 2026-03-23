use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::Size;
use std::collections::HashSet;
use unicode_width::UnicodeWidthChar;

use crate::app_config::AppType;
use crate::cli::i18n::current_language;
use crate::cli::i18n::texts;
use crate::cli::i18n::Language;
use crate::services::skill::SyncMethod;

use super::data::UiData;
use super::form::{
    CodexWireApi, FormFocus, FormMode, FormState, GeminiAuthType, McpAddField, McpAddFormState,
    ProviderAddField, ProviderAddFormState,
};
use super::route::{NavItem, Route};
use super::{data, form};

mod app_state;
mod content_config;
mod content_entities;
mod content_skills;
mod editor_handlers;
mod editor_state;
mod form_handlers;
mod helpers;
mod menu;
mod overlay_handlers;
#[cfg(test)]
mod tests;
mod types;

pub(crate) use app_state::{
    Action, App, ConfigItem, LocalProxySettingsItem, ProxyVisualTransition, SettingsItem,
    WebDavConfigItem, PROXY_HERO_TRANSITION_TICKS,
};
pub use editor_state::{EditorKind, EditorMode, EditorState, EditorSubmit};
pub(crate) use helpers::*;
pub use types::{
    ConfirmAction, ConfirmOverlay, FilterState, Focus, LoadingKind, Overlay, TextInputState,
    TextSubmit, TextViewAction, TextViewState, Toast, ToastKind,
};

const PROVIDER_NOTES_MAX_CHARS: usize = 120;
