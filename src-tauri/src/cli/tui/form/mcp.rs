use crate::{app_config::McpServer, cli::i18n::texts};
use serde_json::{json, Value};
use std::sync::Arc;

use super::{
    FormFocus, FormMode, McpAddField, McpAddFormState, McpEnvVarRow, McpTransport, TextEditSession,
    TextInput,
};

const MCP_TEMPLATES: [&str; 2] = ["Custom", "Filesystem (npx)"];
const MCP_ARGS_INLINE_MAX_ITEMS: usize = 256;
const MCP_ARGS_INLINE_MAX_BYTES: usize = 64 * 1024;

impl McpTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
            Self::Sse => "sse",
        }
    }

    pub fn label(self) -> &'static str {
        self.as_str()
    }

    pub fn picker_index(self) -> usize {
        match self {
            Self::Stdio => 0,
            Self::Http => 1,
            Self::Sse => 2,
        }
    }

    pub fn from_picker_index(index: usize) -> Self {
        match index {
            1 => Self::Http,
            2 => Self::Sse,
            _ => Self::Stdio,
        }
    }

    pub fn from_server_spec(spec: &Value) -> Self {
        match spec.get("type").and_then(|value| value.as_str()) {
            Some("http") => Self::Http,
            Some("sse") => Self::Sse,
            Some("stdio") => Self::Stdio,
            None if spec.get("url").and_then(|value| value.as_str()).is_some() => Self::Http,
            _ => Self::Stdio,
        }
    }

    pub fn is_remote(self) -> bool {
        matches!(self, Self::Http | Self::Sse)
    }
}

impl McpAddFormState {
    pub fn new() -> Self {
        let source = Arc::new(McpServer {
            id: String::new(),
            name: String::new(),
            server: json!({}),
            apps: Default::default(),
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        });
        let mut form = Self {
            mode: FormMode::Add,
            focus: FormFocus::Templates,
            template_idx: 0,
            field_idx: 0,
            text_edit: None,
            field_errors: Vec::new(),
            source,
            id: TextInput::new(""),
            name: TextInput::new(""),
            server_type: McpTransport::Stdio,
            command: TextInput::new(""),
            args: TextInput::new(""),
            args_state: super::McpArgsState::Materialized(Vec::new()),
            url: TextInput::new(""),
            env_rows: Vec::new(),
            apps: Default::default(),
            json_scroll: 0,
            initial_snapshot: None,
        };
        form.capture_initial_snapshot();
        form
    }

    pub fn from_server(server: &McpServer) -> Self {
        Self::from_shared_server(Arc::new(server.clone()))
    }

    pub(crate) fn from_shared_server(server: Arc<McpServer>) -> Self {
        let mut form = Self::new();
        form.mode = FormMode::Edit {
            id: server.id.clone(),
        };
        form.focus = FormFocus::Fields;
        form.source = Arc::clone(&server);
        form.id.set(server.id.clone());
        form.name.set(server.name.clone());
        form.apps = server.apps.clone();
        form.server_type = McpTransport::from_server_spec(&server.server);

        if let Some(command) = server
            .server
            .get("command")
            .and_then(|value| value.as_str())
        {
            form.command.set(command);
        }
        form.args_state = super::McpArgsState::Imported;
        if let Some(url) = server.server.get("url").and_then(|value| value.as_str()) {
            form.url.set(url);
        }
        form.env_rows = load_env_rows(&server);
        form.capture_initial_snapshot();

        form
    }

    fn capture_initial_snapshot(&mut self) {
        self.initial_snapshot = Some(super::McpFormSnapshot {
            source: Arc::clone(&self.source),
            id: self.id.value.trim().to_string(),
            name: self.name.value.trim().to_string(),
            server_type: self.server_type,
            command: self.command.value.trim().to_string(),
            args_state: self.args_state.clone(),
            url: self.url.value.trim().to_string(),
            env_rows: self.env_rows.clone(),
            apps: self.apps.clone(),
        });
    }

    pub fn rebase_initial_snapshot(&mut self) {
        self.capture_initial_snapshot();
    }

    pub fn has_unsaved_changes(&self) -> bool {
        let Some(initial) = self.initial_snapshot.as_ref() else {
            return true;
        };

        !Arc::ptr_eq(&self.source, &initial.source)
            || self.id.value.trim() != initial.id
            || self.name.value.trim() != initial.name
            || self.server_type != initial.server_type
            || self.command.value.trim() != initial.command
            || self.args_changed_since(initial)
            || self.url.value.trim() != initial.url
            || self.env_rows != initial.env_rows
            || self.apps != initial.apps
            || !self.args_text_is_canonical()
    }

    pub fn upsert_env_row(&mut self, row: Option<usize>, key: String, value: String) {
        let next = McpEnvVarRow { key, value };
        if let Some(idx) = row.filter(|idx| *idx < self.env_rows.len()) {
            self.env_rows[idx] = next;
        } else {
            self.env_rows.push(next);
        }
        self.env_rows
            .sort_by(|left, right| left.key.cmp(&right.key));
    }

    pub fn remove_env_row(&mut self, row: usize) {
        if row < self.env_rows.len() {
            self.env_rows.remove(row);
        }
    }

    pub fn env_summary(&self) -> String {
        match self.env_rows.len() {
            0 => texts::none().to_string(),
            1 => texts::tui_mcp_env_entry_count(1),
            count => texts::tui_mcp_env_entry_count(count),
        }
    }

    pub fn locked_id(&self) -> Option<&str> {
        match &self.mode {
            FormMode::Edit { id } => Some(id.as_str()),
            FormMode::Add => None,
        }
    }

    pub fn template_count(&self) -> usize {
        MCP_TEMPLATES.len()
    }

    pub fn template_labels(&self) -> Vec<&'static str> {
        MCP_TEMPLATES.to_vec()
    }

    pub fn fields(&self) -> Vec<McpAddField> {
        let mut fields = vec![McpAddField::Id, McpAddField::Name, McpAddField::Type];

        if self.server_type.is_remote() {
            fields.push(McpAddField::Url);
        } else {
            fields.extend([McpAddField::Command, McpAddField::Args, McpAddField::Env]);
        }

        fields.extend([
            McpAddField::AppClaude,
            McpAddField::AppCodex,
            McpAddField::AppGemini,
            McpAddField::AppOpenCode,
            McpAddField::AppHermes,
        ]);

        fields
    }

    pub fn input(&self, field: McpAddField) -> Option<&TextInput> {
        match field {
            McpAddField::Id => Some(&self.id),
            McpAddField::Name => Some(&self.name),
            McpAddField::Command => Some(&self.command),
            McpAddField::Args => Some(&self.args),
            McpAddField::Url => Some(&self.url),
            McpAddField::Type
            | McpAddField::Env
            | McpAddField::AppClaude
            | McpAddField::AppCodex
            | McpAddField::AppGemini
            | McpAddField::AppOpenCode
            | McpAddField::AppHermes => None,
        }
    }

    pub fn input_mut(&mut self, field: McpAddField) -> Option<&mut TextInput> {
        match field {
            McpAddField::Id => Some(&mut self.id),
            McpAddField::Name => Some(&mut self.name),
            McpAddField::Command => Some(&mut self.command),
            McpAddField::Args => Some(&mut self.args),
            McpAddField::Url => Some(&mut self.url),
            McpAddField::Type
            | McpAddField::Env
            | McpAddField::AppClaude
            | McpAddField::AppCodex
            | McpAddField::AppGemini
            | McpAddField::AppOpenCode
            | McpAddField::AppHermes => None,
        }
    }

    pub fn text_edit_target(&self) -> Option<McpAddField> {
        self.text_edit.as_ref().map(TextEditSession::target)
    }

    pub fn can_edit_field(&self, field: McpAddField) -> bool {
        self.input(field).is_some()
            && (field != McpAddField::Id || self.locked_id().is_none())
            && (field != McpAddField::Args || self.args_inline_edit_available())
    }

    pub fn begin_text_edit(&mut self, field: McpAddField) -> bool {
        if !self.can_edit_field(field) {
            return false;
        }
        if field == McpAddField::Args && !self.materialize_imported_args_for_edit() {
            return false;
        }
        let Some(original) = self.input(field).cloned() else {
            return false;
        };
        let original_error = self.field_error(field).map(str::to_string);
        self.clear_field_error(field);
        self.text_edit = Some(TextEditSession::new(field, original, original_error));
        if let Some(index) = self
            .fields()
            .iter()
            .position(|candidate| *candidate == field)
        {
            self.field_idx = index;
        }
        true
    }

    pub fn take_text_edit(&mut self) -> Option<TextEditSession<McpAddField>> {
        self.text_edit.take()
    }

    pub fn cancel_text_edit(&mut self) -> Option<McpAddField> {
        let (field, original, original_error) = self.text_edit.take()?.into_parts();
        if let Some(input) = self.input_mut(field) {
            *input = original;
        }
        if let Some(index) = self
            .fields()
            .iter()
            .position(|candidate| *candidate == field)
        {
            self.field_idx = index;
        }
        if let Some(message) = original_error {
            self.set_field_error(field, message);
        } else {
            self.clear_field_error(field);
        }
        Some(field)
    }

    pub fn clear_text_edit(&mut self) {
        self.text_edit = None;
    }

    pub fn field_error(&self, field: McpAddField) -> Option<&str> {
        self.field_errors
            .iter()
            .find(|error| error.field == field)
            .map(|error| error.message.as_str())
    }

    pub fn set_field_error(&mut self, field: McpAddField, message: impl Into<String>) {
        self.clear_field_error(field);
        self.field_errors.push(super::InlineFieldError {
            field,
            message: message.into(),
        });
    }

    pub fn clear_field_error(&mut self, field: McpAddField) {
        self.field_errors.retain(|error| error.field != field);
    }

    pub fn apply_template(&mut self, idx: usize) {
        let idx = idx.min(self.template_count().saturating_sub(1));
        self.template_idx = idx;
        self.clear_text_edit();
        self.field_errors.clear();

        if idx == 0 {
            if matches!(self.mode, FormMode::Add) {
                let defaults = Self::new();
                self.name = defaults.name;
                self.server_type = defaults.server_type;
                self.command = defaults.command;
                self.args = defaults.args;
                self.args_state = defaults.args_state;
                self.url = defaults.url;
                self.env_rows = defaults.env_rows;
                self.json_scroll = defaults.json_scroll;
            }
            return;
        }

        if idx == 1 {
            self.name.set("Filesystem");
            self.server_type = McpTransport::Stdio;
            self.command.set("npx");
            self.set_args_values(vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
                "/".to_string(),
            ]);
            self.url.set("");
        }
    }

    pub fn set_args_values(&mut self, values: Vec<String>) {
        if Self::args_values_fit_inline_budget(values.iter().map(String::as_str)) {
            if let Ok(text) = shlex::try_join(values.iter().map(String::as_str)) {
                self.args.set(text);
            } else {
                self.args.set("");
            }
        } else {
            self.args.set("");
        }
        self.args_state = super::McpArgsState::Materialized(values);
    }

    /// Returns a bounded, borrow-only view of the current argv for passive UI
    /// previews. Imported arguments stay lazy so opening a large MCP config
    /// does not clone or join the complete array.
    pub(crate) fn args_preview(&self, max_items: usize) -> (Vec<&str>, usize) {
        let (items, total) = match &self.args_state {
            super::McpArgsState::Imported => {
                let values = self
                    .source
                    .server
                    .get("args")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                (
                    values
                        .iter()
                        .take(max_items)
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>(),
                    values.len(),
                )
            }
            super::McpArgsState::Materialized(values) => (
                values
                    .iter()
                    .take(max_items)
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                values.len(),
            ),
        };
        let hidden = total.saturating_sub(items.len());
        (items, hidden)
    }

    pub fn args_input_is_valid(&self) -> bool {
        matches!(self.args_state, super::McpArgsState::Imported)
            || !self.args_inline_edit_available()
            || self.parse_args_input_within_inline_budget().is_some()
    }

    pub fn commit_args_input(&mut self) -> bool {
        if matches!(self.args_state, super::McpArgsState::Imported)
            || !self.args_inline_edit_available()
        {
            return true;
        }
        let Some(values) = self.parse_args_input_within_inline_budget() else {
            return false;
        };
        self.set_args_values(values);
        true
    }

    /// Parses inline argv only after the raw input has passed the hard byte
    /// limit. This keeps malformed or otherwise expensive shell input bounded
    /// before `shlex` scans or allocates, and rejects token explosions before
    /// they can replace the last valid argv.
    fn parse_args_input_within_inline_budget(&self) -> Option<Vec<String>> {
        if self.args.value.len() > MCP_ARGS_INLINE_MAX_BYTES {
            return None;
        }

        let mut parser = shlex::Shlex::new(&self.args.value);
        let values = parser
            .by_ref()
            .take(MCP_ARGS_INLINE_MAX_ITEMS.saturating_add(1))
            .collect::<Vec<_>>();
        if values.len() > MCP_ARGS_INLINE_MAX_ITEMS
            || parser.had_error
            || !Self::args_values_fit_inline_budget(values.iter().map(String::as_str))
        {
            return None;
        }

        Some(values)
    }

    fn args_text_is_canonical(&self) -> bool {
        let super::McpArgsState::Materialized(values) = &self.args_state else {
            return true;
        };
        if !Self::args_values_fit_inline_budget(values.iter().map(String::as_str)) {
            return true;
        }
        shlex::try_join(values.iter().map(String::as_str))
            .is_ok_and(|canonical| canonical == self.args.value)
    }

    fn args_values_fit_inline_budget<'a>(values: impl Iterator<Item = &'a str>) -> bool {
        let mut count = 0usize;
        let mut bytes = 0usize;
        for value in values {
            count = count.saturating_add(1);
            if count > MCP_ARGS_INLINE_MAX_ITEMS {
                return false;
            }
            bytes = bytes.saturating_add(value.len());
            if bytes > MCP_ARGS_INLINE_MAX_BYTES || value.as_bytes().contains(&0) {
                return false;
            }
        }
        true
    }

    fn imported_args_fit_inline_budget(&self) -> bool {
        let Some(values) = self.source.server.get("args").and_then(Value::as_array) else {
            return true;
        };
        if values.len() > MCP_ARGS_INLINE_MAX_ITEMS {
            return false;
        }
        Self::args_values_fit_inline_budget(values.iter().map_while(Value::as_str))
            && values.iter().all(Value::is_string)
    }

    fn args_inline_edit_available(&self) -> bool {
        match &self.args_state {
            super::McpArgsState::Imported => self.imported_args_fit_inline_budget(),
            super::McpArgsState::Materialized(values) => {
                Self::args_values_fit_inline_budget(values.iter().map(String::as_str))
            }
        }
    }

    fn materialize_imported_args_for_edit(&mut self) -> bool {
        if !matches!(self.args_state, super::McpArgsState::Imported) {
            return self.args_inline_edit_available();
        }
        if !self.imported_args_fit_inline_budget() {
            return false;
        }

        let values = self
            .source
            .server
            .get("args")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        let Ok(text) = shlex::try_join(values.iter().map(String::as_str)) else {
            return false;
        };
        self.args.set(text);
        self.args_state = super::McpArgsState::Materialized(values);
        true
    }

    #[cfg(test)]
    pub(crate) fn args_text_is_materialized_for_test(&self) -> bool {
        matches!(self.args_state, super::McpArgsState::Materialized(_))
    }

    #[cfg(test)]
    pub(crate) fn shares_source_for_test(&self, source: &Arc<McpServer>) -> bool {
        Arc::ptr_eq(&self.source, source)
    }

    pub(crate) fn source_server(&self) -> &McpServer {
        &self.source
    }

    fn args_changed_since(&self, initial: &super::McpFormSnapshot) -> bool {
        match (&initial.args_state, &self.args_state) {
            (super::McpArgsState::Imported, super::McpArgsState::Imported) => false,
            (
                super::McpArgsState::Materialized(before),
                super::McpArgsState::Materialized(after),
            ) => before != after,
            (super::McpArgsState::Imported, super::McpArgsState::Materialized(current)) => {
                !server_args_equal(&initial.source.server, current)
            }
            (super::McpArgsState::Materialized(_), super::McpArgsState::Imported) => true,
        }
    }

    pub fn to_mcp_server_json_value(&self) -> Value {
        let mut output = self.source.as_ref().clone();
        output.id = self.id.value.trim().to_string();
        output.name = self.name.value.trim().to_string();
        output.apps = self.apps.clone();

        let mut server_value = std::mem::take(&mut output.server);
        if !server_value.is_object() {
            server_value = json!({});
        }
        let server_obj = server_value
            .as_object_mut()
            .expect("server must be a JSON object");

        for key in ["type", "command", "env", "url"] {
            server_obj.remove(key);
        }
        if self.server_type.is_remote() {
            server_obj.remove("cwd");
        } else {
            server_obj.remove("headers");
        }

        server_obj.insert("type".to_string(), json!(self.server_type.as_str()));
        if self.server_type.is_remote() {
            server_obj.remove("args");
            server_obj.insert("url".to_string(), json!(self.url.value.trim()));
        } else {
            server_obj.insert("command".to_string(), json!(self.command.value.trim()));
            if let super::McpArgsState::Materialized(values) = &self.args_state {
                server_obj.insert(
                    "args".to_string(),
                    Value::Array(values.iter().cloned().map(Value::String).collect()),
                );
            }
            let env = self
                .env_rows
                .iter()
                .fold(serde_json::Map::new(), |mut map, row| {
                    map.insert(row.key.clone(), Value::String(row.value.clone()));
                    map
                });
            server_obj.remove("env");
            if !env.is_empty() {
                server_obj.insert("env".to_string(), Value::Object(env));
            }
        }

        output.server = server_value;
        serde_json::to_value(output).unwrap_or_else(|_| json!({}))
    }
}

fn server_args_equal(server: &Value, current: &[String]) -> bool {
    let Some(original) = server.get("args").and_then(Value::as_array) else {
        return current.is_empty();
    };
    original.len() == current.len()
        && original
            .iter()
            .zip(current)
            .all(|(left, right)| left.as_str() == Some(right.as_str()))
}

fn load_env_rows(server: &McpServer) -> Vec<McpEnvVarRow> {
    let mut rows = server
        .server
        .get("env")
        .and_then(|value| value.as_object())
        .into_iter()
        .flat_map(|env| env.iter())
        .filter_map(|(key, value)| {
            value.as_str().map(|value| McpEnvVarRow {
                key: key.clone(),
                value: value.to_string(),
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| left.key.cmp(&right.key));
    rows
}
