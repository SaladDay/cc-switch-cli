use super::*;

use std::collections::HashMap;

use serde_json::Value;

pub(crate) enum OpenClawDailyMemoryListItem<'a> {
    File(&'a crate::commands::workspace::DailyMemoryFileInfo),
    Search(&'a crate::commands::workspace::DailyMemorySearchResult),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenClawWorkspaceRow {
    File(&'static str),
    DailyMemory,
}

impl OpenClawWorkspaceRow {
    pub(crate) fn all() -> Vec<Self> {
        crate::commands::workspace::ALLOWED_FILES
            .iter()
            .copied()
            .map(Self::File)
            .chain(std::iter::once(Self::DailyMemory))
            .collect()
    }

    pub(crate) fn from_index(index: usize) -> Option<Self> {
        crate::commands::workspace::ALLOWED_FILES
            .get(index)
            .copied()
            .map(Self::File)
            .or_else(|| {
                (index == crate::commands::workspace::ALLOWED_FILES.len())
                    .then_some(Self::DailyMemory)
            })
    }
}

pub(crate) const OPENCLAW_TOOLS_PROFILE_PICKER_VALUES: [Option<&str>; 5] = [
    None,
    Some("minimal"),
    Some("coding"),
    Some("messaging"),
    Some("full"),
];
pub(crate) const OPENCLAW_TOOLS_PROFILE_PICKER_LEN: usize =
    OPENCLAW_TOOLS_PROFILE_PICKER_VALUES.len();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenClawToolsSection {
    Profile,
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenClawModelOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenClawAgentsSection {
    PrimaryModel,
    FallbackModels,
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenClawAgentsRuntimeField {
    Workspace,
    Timeout,
    ContextTokens,
    MaxConcurrent,
}

impl OpenClawAgentsRuntimeField {
    pub(crate) fn from_row(row: usize) -> Option<Self> {
        match row {
            0 => Some(Self::Workspace),
            1 => Some(Self::Timeout),
            2 => Some(Self::ContextTokens),
            3 => Some(Self::MaxConcurrent),
            _ => None,
        }
    }
}

pub(crate) const OPENCLAW_AGENTS_MODEL_PICKER_NONE: usize = usize::MAX;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OpenClawAgentsFormState {
    pub primary_model: String,
    pub fallbacks: Vec<String>,
    pub workspace: String,
    pub timeout: String,
    pub timeout_seconds_seed: Option<Value>,
    pub context_tokens: String,
    pub context_tokens_seed: Option<Value>,
    pub max_concurrent: String,
    pub max_concurrent_seed: Option<Value>,
    pub model_catalog: Option<
        std::collections::HashMap<String, crate::openclaw_config::OpenClawModelCatalogEntry>,
    >,
    pub defaults_extra: HashMap<String, Value>,
    pub model_extra: HashMap<String, Value>,
    pub has_legacy_timeout: bool,
    pub section: OpenClawAgentsSection,
    pub row: usize,
}

impl OpenClawAgentsFormState {
    pub(crate) fn from_snapshot(
        defaults: Option<&crate::openclaw_config::OpenClawAgentsDefaults>,
    ) -> Self {
        let defaults = defaults.cloned().unwrap_or_default();
        let model = defaults
            .model
            .unwrap_or(crate::openclaw_config::OpenClawDefaultModel {
                primary: String::new(),
                fallbacks: Vec::new(),
                extra: HashMap::new(),
            });
        let mut defaults_extra = defaults.extra;
        let timeout_seconds_seed = defaults_extra.remove("timeoutSeconds");
        let legacy_timeout = defaults_extra.remove("timeout");
        let has_legacy_timeout = legacy_timeout.is_some();
        let context_tokens_seed = defaults_extra.remove("contextTokens");
        let max_concurrent_seed = defaults_extra.remove("maxConcurrent");

        let workspace = string_value(defaults_extra.remove("workspace"));
        let timeout = legacy_timeout
            .clone()
            .map(|value| string_value(Some(value)))
            .unwrap_or_else(|| numeric_value(timeout_seconds_seed.clone()));
        let context_tokens = numeric_value(context_tokens_seed.clone());
        let max_concurrent = numeric_value(max_concurrent_seed.clone());

        Self {
            primary_model: model.primary,
            fallbacks: model.fallbacks,
            workspace,
            timeout,
            timeout_seconds_seed,
            context_tokens,
            context_tokens_seed,
            max_concurrent,
            max_concurrent_seed,
            model_catalog: defaults.models,
            defaults_extra,
            model_extra: model.extra,
            has_legacy_timeout,
            section: OpenClawAgentsSection::PrimaryModel,
            row: 0,
        }
    }

    pub(crate) fn move_down(&mut self) {
        self.section = self.clamp_section(self.section);
        let rows = self.rows_in_section(self.section);
        if self.row + 1 < rows {
            self.row += 1;
            return;
        }

        match self.section {
            OpenClawAgentsSection::PrimaryModel => {
                self.section = OpenClawAgentsSection::FallbackModels;
                self.row = 0;
            }
            OpenClawAgentsSection::FallbackModels => {
                self.section = OpenClawAgentsSection::Runtime;
                self.row = 0;
            }
            OpenClawAgentsSection::Runtime => {
                self.section = OpenClawAgentsSection::Runtime;
                self.row = self.rows_in_section(self.section).saturating_sub(1);
            }
        }
    }

    pub(crate) fn move_up(&mut self) {
        self.section = self.clamp_section(self.section);
        if self.row > 0 {
            self.row -= 1;
            return;
        }

        self.section = match self.section {
            OpenClawAgentsSection::PrimaryModel => OpenClawAgentsSection::PrimaryModel,
            OpenClawAgentsSection::FallbackModels => OpenClawAgentsSection::PrimaryModel,
            OpenClawAgentsSection::Runtime => OpenClawAgentsSection::FallbackModels,
        };
        self.row = self.rows_in_section(self.section).saturating_sub(1);
    }

    pub(crate) fn restore_position(&mut self, previous: &Self) {
        self.section = self.clamp_section(previous.section);
        self.row = previous
            .row
            .min(self.rows_in_section(self.section).saturating_sub(1));
    }

    pub(crate) fn primary_model_picker_selection(&self, options: &[OpenClawModelOption]) -> usize {
        model_picker_selection(&self.primary_model, options)
    }

    pub(crate) fn insert_fallback(&mut self, value: String) {
        let index = self.row.min(self.fallbacks.len());
        self.fallbacks.insert(index, value);
        self.row = index;
    }

    pub(crate) fn clear_primary_model(&mut self) {
        self.primary_model.clear();
    }

    pub(crate) fn available_fallback_options(
        &self,
        options: &[OpenClawModelOption],
    ) -> Vec<OpenClawModelOption> {
        let used = self
            .fallbacks
            .iter()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .chain((!self.primary_model.trim().is_empty()).then(|| self.primary_model.clone()))
            .collect::<std::collections::HashSet<_>>();

        options
            .iter()
            .filter(|option| !used.contains(&option.value))
            .cloned()
            .collect()
    }

    pub(crate) fn available_fallback_options_for_row(
        &self,
        row: usize,
        options: &[OpenClawModelOption],
    ) -> Vec<OpenClawModelOption> {
        let current = self
            .fallbacks
            .get(row)
            .map(String::as_str)
            .unwrap_or_default();
        let used = self
            .fallbacks
            .iter()
            .enumerate()
            .filter(|(index, value)| *index != row && !value.trim().is_empty())
            .map(|(_, value)| value.as_str())
            .chain((!self.primary_model.trim().is_empty()).then_some(self.primary_model.as_str()))
            .collect::<std::collections::HashSet<_>>();

        options
            .iter()
            .filter(|option| option.value == current || !used.contains(option.value.as_str()))
            .cloned()
            .collect()
    }

    pub(crate) fn current_fallback_picker_selection(
        &self,
        row: usize,
        options: &[OpenClawModelOption],
    ) -> usize {
        self.fallbacks
            .get(row)
            .map(|current| model_picker_selection(current, options))
            .unwrap_or(OPENCLAW_AGENTS_MODEL_PICKER_NONE)
    }

    pub(crate) fn set_current_fallback(&mut self, row: usize, value: String) {
        let Some(current) = self.fallbacks.get_mut(row) else {
            return;
        };
        *current = value;
    }

    pub(crate) fn remove_current_fallback(&mut self) {
        if self.row >= self.fallbacks.len() {
            return;
        }
        self.fallbacks.remove(self.row);
        self.row = self
            .row
            .min(self.rows_in_section(self.section).saturating_sub(1));
    }

    pub(crate) fn selected_runtime_field(&self) -> Option<OpenClawAgentsRuntimeField> {
        (self.section == OpenClawAgentsSection::Runtime)
            .then(|| OpenClawAgentsRuntimeField::from_row(self.row))
            .flatten()
    }

    pub(crate) fn runtime_field_value(&self, field: OpenClawAgentsRuntimeField) -> &str {
        match field {
            OpenClawAgentsRuntimeField::Workspace => &self.workspace,
            OpenClawAgentsRuntimeField::Timeout => &self.timeout,
            OpenClawAgentsRuntimeField::ContextTokens => &self.context_tokens,
            OpenClawAgentsRuntimeField::MaxConcurrent => &self.max_concurrent,
        }
    }

    pub(crate) fn set_runtime_field(&mut self, field: OpenClawAgentsRuntimeField, value: String) {
        match field {
            OpenClawAgentsRuntimeField::Workspace => self.workspace = value,
            OpenClawAgentsRuntimeField::Timeout => self.timeout = value,
            OpenClawAgentsRuntimeField::ContextTokens => self.context_tokens = value,
            OpenClawAgentsRuntimeField::MaxConcurrent => self.max_concurrent = value,
        }
    }

    pub(crate) fn clear_runtime_field(&mut self, field: OpenClawAgentsRuntimeField) {
        match field {
            OpenClawAgentsRuntimeField::Workspace => self.workspace.clear(),
            OpenClawAgentsRuntimeField::Timeout => {
                self.timeout.clear();
                self.timeout_seconds_seed = None;
                self.has_legacy_timeout = false;
            }
            OpenClawAgentsRuntimeField::ContextTokens => {
                self.context_tokens.clear();
                self.context_tokens_seed = None;
            }
            OpenClawAgentsRuntimeField::MaxConcurrent => {
                self.max_concurrent.clear();
                self.max_concurrent_seed = None;
            }
        }
    }

    pub(crate) fn to_config(&self) -> crate::openclaw_config::OpenClawAgentsDefaults {
        let mut extra = self.defaults_extra.clone();
        update_string_field(&mut extra, "workspace", &self.workspace);
        update_timeout_seconds_field(
            &mut extra,
            &self.timeout,
            self.has_legacy_timeout,
            self.timeout_seconds_seed.as_ref(),
        );
        extra.remove("timeout");
        update_number_field(
            &mut extra,
            "contextTokens",
            &self.context_tokens,
            self.context_tokens_seed.as_ref(),
        );
        update_number_field(
            &mut extra,
            "maxConcurrent",
            &self.max_concurrent,
            self.max_concurrent_seed.as_ref(),
        );

        let fallbacks = self
            .fallbacks
            .iter()
            .filter_map(|value| {
                let trimmed = value.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            })
            .collect::<Vec<_>>();
        let primary_model = self.primary_model.trim().to_string();
        let model =
            (!primary_model.is_empty() || !fallbacks.is_empty() || !self.model_extra.is_empty())
                .then(|| crate::openclaw_config::OpenClawDefaultModel {
                    primary: primary_model,
                    fallbacks,
                    extra: self.model_extra.clone(),
                });

        crate::openclaw_config::OpenClawAgentsDefaults {
            model,
            models: self.model_catalog.clone(),
            extra,
        }
    }

    fn rows_in_section(&self, section: OpenClawAgentsSection) -> usize {
        match section {
            OpenClawAgentsSection::PrimaryModel => 1,
            OpenClawAgentsSection::FallbackModels => self.fallbacks.len() + 1,
            OpenClawAgentsSection::Runtime => 4,
        }
    }

    pub(crate) fn has_unmigratable_legacy_timeout(&self) -> bool {
        self.has_legacy_timeout
            && !self.timeout.trim().is_empty()
            && parse_number(self.timeout.trim()).is_none()
    }

    pub(crate) fn preserved_timeout_seconds(&self) -> Option<&Value> {
        preserved_non_string_runtime_seed(&self.timeout, self.timeout_seconds_seed.as_ref())
    }

    pub(crate) fn preserved_context_tokens(&self) -> Option<&Value> {
        preserved_non_string_runtime_seed(&self.context_tokens, self.context_tokens_seed.as_ref())
    }

    pub(crate) fn preserved_max_concurrent(&self) -> Option<&Value> {
        preserved_non_string_runtime_seed(&self.max_concurrent, self.max_concurrent_seed.as_ref())
    }

    pub(crate) fn has_preserved_non_string_runtime_values(&self) -> bool {
        self.preserved_timeout_seconds().is_some()
            || self.preserved_context_tokens().is_some()
            || self.preserved_max_concurrent().is_some()
    }

    fn clamp_section(&self, section: OpenClawAgentsSection) -> OpenClawAgentsSection {
        section
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OpenClawToolsFormState {
    pub profile: Option<String>,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub extra: HashMap<String, Value>,
    pub section: OpenClawToolsSection,
    pub row: usize,
}

impl OpenClawToolsFormState {
    pub(crate) fn from_snapshot(
        tools: Option<&crate::openclaw_config::OpenClawToolsConfig>,
    ) -> Self {
        let tools = tools.cloned().unwrap_or_default();
        Self {
            profile: tools.profile,
            allow: tools.allow,
            deny: tools.deny,
            extra: tools.extra,
            section: OpenClawToolsSection::Profile,
            row: 0,
        }
    }

    pub(crate) fn unsupported_profile(&self) -> Option<&str> {
        let profile = self.profile.as_deref()?;
        if openclaw_tools_profile_picker_index(Some(profile)).is_some() {
            None
        } else {
            Some(profile)
        }
    }

    pub(crate) fn current_profile_label(&self) -> String {
        if let Some(index) = openclaw_tools_profile_picker_index(self.profile.as_deref()) {
            return openclaw_tools_profile_picker_label(index).to_string();
        }

        let value = self.profile.as_deref().unwrap_or_default();
        format!(
            "{value} ({})",
            texts::tui_openclaw_tools_unsupported_profile_label()
        )
    }

    pub(crate) fn move_down(&mut self) {
        self.section = self.clamp_section(self.section);
        let rows = self.rows_in_section(self.section);
        if self.row + 1 < rows {
            self.row += 1;
            return;
        }

        match self.section {
            OpenClawToolsSection::Profile => {
                self.section = OpenClawToolsSection::Allow;
                self.row = 0;
            }
            OpenClawToolsSection::Allow => {
                self.section = OpenClawToolsSection::Deny;
                self.row = 0;
            }
            OpenClawToolsSection::Deny => {
                self.section = OpenClawToolsSection::Deny;
                self.row = self.rows_in_section(self.section).saturating_sub(1);
            }
        }
    }

    pub(crate) fn move_up(&mut self) {
        self.section = self.clamp_section(self.section);
        if self.row > 0 {
            self.row -= 1;
            return;
        }

        self.section = match self.section {
            OpenClawToolsSection::Profile => OpenClawToolsSection::Profile,
            OpenClawToolsSection::Allow => OpenClawToolsSection::Profile,
            OpenClawToolsSection::Deny => OpenClawToolsSection::Allow,
        };
        self.row = self.rows_in_section(self.section).saturating_sub(1);
    }

    pub(crate) fn restore_position(&mut self, previous: &Self) {
        self.section = self.clamp_section(previous.section);
        self.row = previous
            .row
            .min(self.rows_in_section(self.section).saturating_sub(1));
    }

    pub(crate) fn selected_rule_row(&self) -> Option<usize> {
        let list = self.list(self.section)?;
        (self.row < list.len()).then_some(self.row)
    }

    pub(crate) fn selected_rule_value(&self) -> Option<&str> {
        let row = self.selected_rule_row()?;
        self.list(self.section)
            .and_then(|list| list.get(row))
            .map(String::as_str)
    }

    pub(crate) fn upsert_rule(
        &mut self,
        section: OpenClawToolsSection,
        row: Option<usize>,
        value: String,
    ) {
        let Some(list) = self.list_mut(section) else {
            return;
        };

        let next_row = match row {
            Some(index) if index < list.len() => {
                list[index] = value;
                index
            }
            Some(index) => {
                let index = index.min(list.len());
                list.insert(index, value);
                index
            }
            None => {
                list.push(value);
                list.len().saturating_sub(1)
            }
        };

        self.section = section;
        self.row = next_row;
    }

    pub(crate) fn remove_current_list_item(&mut self) {
        let row = self.row;
        if let Some(list) = self.list_mut(self.section) {
            if row < list.len() {
                list.remove(row);
            }
        }
        self.row = self
            .row
            .min(self.rows_in_section(self.section).saturating_sub(1));
    }

    pub(crate) fn to_config(&self) -> crate::openclaw_config::OpenClawToolsConfig {
        crate::openclaw_config::OpenClawToolsConfig {
            profile: self.profile.clone(),
            allow: self
                .allow
                .iter()
                .filter_map(|value| {
                    let trimmed = value.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_string())
                })
                .collect(),
            deny: self
                .deny
                .iter()
                .filter_map(|value| {
                    let trimmed = value.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_string())
                })
                .collect(),
            extra: self.extra.clone(),
        }
    }

    fn rows_in_section(&self, section: OpenClawToolsSection) -> usize {
        match section {
            OpenClawToolsSection::Profile => 1,
            OpenClawToolsSection::Allow => self.allow.len() + 1,
            OpenClawToolsSection::Deny => self.deny.len() + 1,
        }
    }

    fn list(&self, section: OpenClawToolsSection) -> Option<&Vec<String>> {
        match section {
            OpenClawToolsSection::Allow => Some(&self.allow),
            OpenClawToolsSection::Deny => Some(&self.deny),
            _ => None,
        }
    }

    fn list_mut(&mut self, section: OpenClawToolsSection) -> Option<&mut Vec<String>> {
        match section {
            OpenClawToolsSection::Allow => Some(&mut self.allow),
            OpenClawToolsSection::Deny => Some(&mut self.deny),
            _ => None,
        }
    }

    fn clamp_section(&self, section: OpenClawToolsSection) -> OpenClawToolsSection {
        section
    }
}

pub(crate) fn openclaw_agents_model_options(data: &UiData) -> Vec<OpenClawModelOption> {
    let mut labels = std::collections::BTreeMap::new();

    for row in &data.providers.rows {
        let provider_name = if row.provider.name.trim().is_empty() {
            row.id.as_str()
        } else {
            row.provider.name.as_str()
        };

        let Some(models) = row
            .provider
            .settings_config
            .get("models")
            .and_then(Value::as_array)
        else {
            continue;
        };

        for model in models {
            let Some(model_id) = model.get("id").and_then(Value::as_str) else {
                continue;
            };
            if model_id.trim().is_empty() {
                continue;
            }

            let value = format!("{}/{}", row.id, model_id);
            let model_name = model
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(model_id);
            labels.entry(value).or_insert_with(|| OpenClawModelOption {
                label: format!("{provider_name} / {model_name}"),
                value: format!("{}/{}", row.id, model_id),
            });
        }
    }

    let mut options = labels.into_values().collect::<Vec<_>>();
    options.sort_by(|left, right| left.label.cmp(&right.label));
    options
}

fn model_picker_selection(current: &str, options: &[OpenClawModelOption]) -> usize {
    options
        .iter()
        .position(|option| option.value == current)
        .unwrap_or(OPENCLAW_AGENTS_MODEL_PICKER_NONE)
}

fn string_value(value: Option<Value>) -> String {
    match value {
        Some(Value::String(value)) => value,
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn numeric_value(value: Option<Value>) -> String {
    match value {
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::String(value)) => value,
        _ => String::new(),
    }
}

fn update_string_field(extra: &mut HashMap<String, Value>, key: &str, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        extra.remove(key);
    } else {
        extra.insert(key.to_string(), Value::String(trimmed.to_string()));
    }
}

fn update_number_field(
    extra: &mut HashMap<String, Value>,
    key: &str,
    value: &str,
    seed: Option<&Value>,
) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        if should_preserve_non_string_numeric_seed(seed) {
            extra.insert(key.to_string(), seed.cloned().expect("seed exists"));
            return;
        }
        extra.remove(key);
        return;
    }

    let parsed = parse_number(trimmed);

    if let Some(number) = parsed {
        extra.insert(key.to_string(), Value::Number(number));
    } else {
        extra.insert(key.to_string(), Value::String(trimmed.to_string()));
    }
}

fn update_timeout_seconds_field(
    extra: &mut HashMap<String, Value>,
    value: &str,
    has_legacy_timeout: bool,
    timeout_seconds_seed: Option<&Value>,
) {
    let trimmed = value.trim();
    if let Some(number) = parse_number(trimmed) {
        extra.insert("timeoutSeconds".to_string(), Value::Number(number));
        return;
    }

    if trimmed.is_empty() && has_legacy_timeout {
        if let Some(seed) = timeout_seconds_seed {
            extra.insert("timeoutSeconds".to_string(), seed.clone());
            return;
        }
    }

    if trimmed.is_empty() {
        if should_preserve_non_string_numeric_seed(timeout_seconds_seed) {
            extra.insert(
                "timeoutSeconds".to_string(),
                timeout_seconds_seed.cloned().expect("seed exists"),
            );
            return;
        }
        extra.remove("timeoutSeconds");
    } else {
        extra.insert(
            "timeoutSeconds".to_string(),
            Value::String(trimmed.to_string()),
        );
    }
}

fn parse_number(value: &str) -> Option<serde_json::Number> {
    value
        .parse::<i64>()
        .ok()
        .map(serde_json::Number::from)
        .or_else(|| value.parse::<u64>().ok().map(serde_json::Number::from))
        .or_else(|| {
            value
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
        })
}

fn should_preserve_non_string_numeric_seed(seed: Option<&Value>) -> bool {
    matches!(
        seed,
        Some(Value::Bool(_) | Value::Null | Value::Array(_) | Value::Object(_))
    )
}

fn preserved_non_string_runtime_seed<'a>(
    value: &str,
    seed: Option<&'a Value>,
) -> Option<&'a Value> {
    if value.trim().is_empty() && should_preserve_non_string_numeric_seed(seed) {
        seed
    } else {
        None
    }
}

fn openclaw_tools_warning_matches_path(
    data: &UiData,
    warning: &crate::openclaw_config::OpenClawHealthWarning,
) -> bool {
    let config_path = data
        .config
        .openclaw_config_path
        .as_ref()
        .map(|path| path.display().to_string());

    match warning.path.as_deref() {
        None => true,
        Some(path) if config_path.as_deref() == Some(path) => true,
        Some("tools") => true,
        Some(path) => path.starts_with("tools."),
    }
}

pub(crate) fn openclaw_tools_load_failed(data: &UiData) -> bool {
    data.config.openclaw_tools.is_none()
        && data
            .config
            .openclaw_warnings
            .as_ref()
            .into_iter()
            .flatten()
            .any(|warning| openclaw_tools_warning_matches_path(data, warning))
}

pub(crate) fn openclaw_tools_has_blocking_warning(data: &UiData) -> bool {
    openclaw_tools_load_failed(data)
        || data
            .config
            .openclaw_warnings
            .as_ref()
            .into_iter()
            .flatten()
            .any(|warning| {
                warning.code == "config_parse_failed"
                    && openclaw_tools_warning_matches_path(data, warning)
            })
}

fn openclaw_agents_warning_matches_path(
    data: &UiData,
    warning: &crate::openclaw_config::OpenClawHealthWarning,
) -> bool {
    let config_path = data
        .config
        .openclaw_config_path
        .as_ref()
        .map(|path| path.display().to_string());

    match warning.path.as_deref() {
        None => true,
        Some(path) if config_path.as_deref() == Some(path) => true,
        Some("agents.defaults") => true,
        Some(path) => path.starts_with("agents.defaults."),
    }
}

pub(crate) fn openclaw_agents_load_failed(data: &UiData) -> bool {
    data.config.openclaw_agents_defaults.is_none()
        && data
            .config
            .openclaw_warnings
            .as_ref()
            .into_iter()
            .flatten()
            .any(|warning| openclaw_agents_warning_matches_path(data, warning))
}

pub(crate) fn openclaw_agents_has_blocking_warning(data: &UiData) -> bool {
    openclaw_agents_load_failed(data)
        || data
            .config
            .openclaw_warnings
            .as_ref()
            .into_iter()
            .flatten()
            .any(|warning| {
                warning.code == "config_parse_failed"
                    && openclaw_agents_warning_matches_path(data, warning)
            })
}

impl<'a> OpenClawDailyMemoryListItem<'a> {
    pub(crate) fn filename(&self) -> &str {
        match self {
            Self::File(row) => &row.filename,
            Self::Search(row) => &row.filename,
        }
    }

    pub(crate) fn preview(&self) -> &str {
        match self {
            Self::File(row) => &row.preview,
            Self::Search(row) => &row.snippet,
        }
    }
}

pub(crate) fn route_has_content_list(route: &Route) -> bool {
    matches!(
        route,
        Route::Providers
            | Route::ProviderDetail { .. }
            | Route::Mcp
            | Route::Prompts
            | Route::Config
            | Route::ConfigOpenClawWorkspace
            | Route::ConfigOpenClawDailyMemory
            | Route::ConfigOpenClawEnv
            | Route::ConfigOpenClawTools
            | Route::ConfigOpenClawAgents
            | Route::ConfigWebDav
            | Route::Skills
            | Route::SkillsDiscover
            | Route::SkillsRepos
            | Route::SkillDetail { .. }
            | Route::Settings
            | Route::SettingsProxy
    )
}

pub(crate) fn route_default_focus(route: &Route) -> Focus {
    match route {
        Route::Main => Focus::Nav,
        _ => Focus::Content,
    }
}

pub(crate) fn visible_providers<'a>(
    app_type: &AppType,
    filter: &FilterState,
    data: &'a UiData,
) -> Vec<&'a super::data::ProviderRow> {
    let query = filter.query_lower();
    data.providers
        .rows
        .iter()
        .filter(|row| match &query {
            None => true,
            Some(q) => {
                super::data::provider_display_name(app_type, row)
                    .to_lowercase()
                    .contains(q)
                    || row.provider.name.to_lowercase().contains(q)
                    || row.id.to_lowercase().contains(q)
            }
        })
        .collect()
}

pub(crate) fn supports_provider_stream_check(app_type: &AppType) -> bool {
    !matches!(app_type, AppType::OpenClaw)
}

pub(crate) fn visible_mcp<'a>(
    filter: &FilterState,
    data: &'a UiData,
) -> Vec<&'a super::data::McpRow> {
    let query = filter.query_lower();
    data.mcp
        .rows
        .iter()
        .filter(|row| match &query {
            None => true,
            Some(q) => {
                row.server.name.to_lowercase().contains(q) || row.id.to_lowercase().contains(q)
            }
        })
        .collect()
}

pub(crate) fn visible_prompts<'a>(
    filter: &FilterState,
    data: &'a UiData,
) -> Vec<&'a super::data::PromptRow> {
    let query = filter.query_lower();
    data.prompts
        .rows
        .iter()
        .filter(|row| match &query {
            None => true,
            Some(q) => {
                row.prompt.name.to_lowercase().contains(q) || row.id.to_lowercase().contains(q)
            }
        })
        .collect()
}

pub(crate) fn visible_skills_installed<'a>(
    filter: &FilterState,
    data: &'a UiData,
) -> Vec<&'a crate::services::skill::InstalledSkill> {
    let query = filter.query_lower();
    data.skills
        .installed
        .iter()
        .filter(|skill| match &query {
            None => true,
            Some(q) => {
                skill.name.to_lowercase().contains(q)
                    || skill.directory.to_lowercase().contains(q)
                    || skill.id.to_lowercase().contains(q)
            }
        })
        .collect()
}

pub(crate) fn visible_skills_discover<'a>(
    filter: &FilterState,
    skills: &'a [crate::services::skill::Skill],
) -> Vec<&'a crate::services::skill::Skill> {
    let query = filter.query_lower();
    skills
        .iter()
        .filter(|skill| match &query {
            None => true,
            Some(q) => {
                skill.name.to_lowercase().contains(q)
                    || skill.directory.to_lowercase().contains(q)
                    || skill.key.to_lowercase().contains(q)
            }
        })
        .collect()
}

pub(crate) fn visible_skills_repos<'a>(
    filter: &FilterState,
    data: &'a UiData,
) -> Vec<&'a crate::services::skill::SkillRepo> {
    let query = filter.query_lower();
    data.skills
        .repos
        .iter()
        .filter(|repo| match &query {
            None => true,
            Some(q) => {
                repo.owner.to_lowercase().contains(q)
                    || repo.name.to_lowercase().contains(q)
                    || repo.branch.to_lowercase().contains(q)
            }
        })
        .collect()
}

pub(crate) fn visible_skills_unmanaged<'a>(
    filter: &FilterState,
    skills: &'a [crate::services::skill::UnmanagedSkill],
) -> Vec<&'a crate::services::skill::UnmanagedSkill> {
    let query = filter.query_lower();
    skills
        .iter()
        .filter(|skill| match &query {
            None => true,
            Some(q) => {
                skill.name.to_lowercase().contains(q)
                    || skill.directory.to_lowercase().contains(q)
                    || skill
                        .description
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(q)
                    || skill.found_in.iter().any(|s| s.to_lowercase().contains(q))
            }
        })
        .collect()
}

pub(crate) fn visible_config_items(filter: &FilterState, app_type: &AppType) -> Vec<ConfigItem> {
    let all = ConfigItem::ALL
        .iter()
        .filter(|item| item.listed_in_config_menu(app_type))
        .cloned()
        .collect::<Vec<_>>();
    let Some(q) = filter.query_lower() else {
        return all;
    };

    all.into_iter()
        .filter(|item| item.label().to_lowercase().contains(&q))
        .collect()
}

pub(crate) fn openclaw_workspace_entry_count() -> usize {
    OpenClawWorkspaceRow::all().len()
}

pub(crate) fn openclaw_workspace_rows() -> Vec<OpenClawWorkspaceRow> {
    OpenClawWorkspaceRow::all()
}

pub(crate) fn openclaw_workspace_row(index: usize) -> Option<OpenClawWorkspaceRow> {
    OpenClawWorkspaceRow::from_index(index)
}

pub(crate) fn visible_openclaw_daily_memory<'a>(
    app: &'a App,
    data: &'a UiData,
) -> Vec<OpenClawDailyMemoryListItem<'a>> {
    if !app.openclaw_daily_memory_search_query.trim().is_empty() {
        app.openclaw_daily_memory_search_results
            .iter()
            .map(OpenClawDailyMemoryListItem::Search)
            .collect()
    } else {
        data.config
            .openclaw_workspace
            .daily_memory_files
            .iter()
            .map(OpenClawDailyMemoryListItem::File)
            .collect()
    }
}

pub(crate) fn config_item_label(item: &ConfigItem) -> &'static str {
    item.label()
}

pub(crate) fn visible_webdav_config_items(filter: &FilterState) -> Vec<WebDavConfigItem> {
    let all = WebDavConfigItem::ALL.to_vec();
    let Some(q) = filter.query_lower() else {
        return all;
    };

    all.into_iter()
        .filter(|item| item.label().to_lowercase().contains(&q))
        .collect()
}

pub(crate) fn webdav_config_item_label(item: &WebDavConfigItem) -> &'static str {
    item.label()
}

pub(crate) fn cycle_app_type(current: &AppType, dir: i8) -> Option<AppType> {
    let visible_apps = crate::settings::get_visible_apps();
    if visible_apps.ordered_enabled().len() <= 1 {
        return None;
    }

    crate::settings::next_visible_app(&visible_apps, current, dir).filter(|next| next != current)
}

pub(crate) fn app_type_picker_index(app_type: &AppType) -> usize {
    match app_type {
        AppType::Claude => 0,
        AppType::Codex => 1,
        AppType::Gemini => 2,
        AppType::OpenCode => 3,
        AppType::OpenClaw => 4,
    }
}

pub(crate) fn four_app_picker_index(app_type: &AppType) -> usize {
    app_type_picker_index(app_type).min(3)
}

pub(crate) fn app_type_for_picker_index(index: usize) -> AppType {
    match index {
        1 => AppType::Codex,
        2 => AppType::Gemini,
        3 => AppType::OpenCode,
        4 => AppType::OpenClaw,
        _ => AppType::Claude,
    }
}

pub(crate) fn snippet_picker_index_for_app_type(app_type: &AppType) -> usize {
    app_type_picker_index(app_type)
}

pub(crate) fn snippet_picker_app_type(index: usize) -> AppType {
    app_type_for_picker_index(index)
}

pub(crate) fn sync_method_picker_index(method: SyncMethod) -> usize {
    match method {
        SyncMethod::Auto => 0,
        SyncMethod::Symlink => 1,
        SyncMethod::Copy => 2,
    }
}

pub(crate) fn sync_method_for_picker_index(index: usize) -> SyncMethod {
    match index {
        1 => SyncMethod::Symlink,
        2 => SyncMethod::Copy,
        _ => SyncMethod::Auto,
    }
}

pub(crate) fn openclaw_tools_profile_picker_index(profile: Option<&str>) -> Option<usize> {
    OPENCLAW_TOOLS_PROFILE_PICKER_VALUES
        .iter()
        .position(|value| *value == profile)
}

pub(crate) fn openclaw_tools_profile_for_picker_index(index: usize) -> Option<&'static str> {
    OPENCLAW_TOOLS_PROFILE_PICKER_VALUES
        .get(index)
        .copied()
        .flatten()
}

pub(crate) fn openclaw_tools_profile_picker_label(index: usize) -> &'static str {
    match index {
        1 => texts::tui_openclaw_tools_profile_minimal(),
        2 => texts::tui_openclaw_tools_profile_coding(),
        3 => texts::tui_openclaw_tools_profile_messaging(),
        4 => texts::tui_openclaw_tools_profile_full(),
        _ => texts::tui_openclaw_tools_profile_unset(),
    }
}

pub(crate) fn is_save_shortcut(key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('s' | 'S') => key.modifiers.contains(KeyModifiers::CONTROL),
        KeyCode::Char('\u{13}') => true,
        _ => false,
    }
}

pub(crate) fn is_open_external_editor_shortcut(key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('o' | 'O') => key.modifiers.contains(KeyModifiers::CONTROL),
        _ => false,
    }
}
