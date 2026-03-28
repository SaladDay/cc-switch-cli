use crate::app_config::McpServer;
use serde_json::{json, Value};

use super::{FormFocus, FormMode, McpAddField, McpAddFormState, McpEnvVarRow, TextInput};

const MCP_TEMPLATES: [&str; 2] = ["Custom", "Filesystem (npx)"];

impl McpAddFormState {
    pub fn new() -> Self {
        Self {
            mode: FormMode::Add,
            focus: FormFocus::Templates,
            template_idx: 0,
            field_idx: 0,
            editing: false,
            extra: json!({}),
            id: TextInput::new(""),
            name: TextInput::new(""),
            command: TextInput::new(""),
            args: TextInput::new(""),
            env_rows: Vec::new(),
            apps: Default::default(),
            json_scroll: 0,
        }
    }

    pub fn from_server(server: &McpServer) -> Self {
        let mut form = Self::new();
        form.mode = FormMode::Edit {
            id: server.id.clone(),
        };
        form.focus = FormFocus::Fields;
        form.extra = serde_json::to_value(server).unwrap_or_else(|_| json!({}));
        form.id.set(server.id.clone());
        form.name.set(server.name.clone());
        form.apps = server.apps.clone();

        if let Some(command) = server
            .server
            .get("command")
            .and_then(|value| value.as_str())
        {
            form.command.set(command);
        }
        if let Some(args) = server.server.get("args").and_then(|value| value.as_array()) {
            let joined = args
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            form.args.set(joined);
        }
        form.env_rows = load_env_rows(server);

        form
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

    pub fn locked_id(&self) -> Option<&str> {
        match &self.mode {
            FormMode::Edit { id } => Some(id.as_str()),
            FormMode::Add => None,
        }
    }

    pub fn has_required_fields(&self) -> bool {
        !self.id.is_blank() && !self.name.is_blank()
    }

    pub fn template_count(&self) -> usize {
        MCP_TEMPLATES.len()
    }

    pub fn template_labels(&self) -> Vec<&'static str> {
        MCP_TEMPLATES.to_vec()
    }

    pub fn fields(&self) -> Vec<McpAddField> {
        vec![
            McpAddField::Id,
            McpAddField::Name,
            McpAddField::Command,
            McpAddField::Args,
            McpAddField::AppClaude,
            McpAddField::AppCodex,
            McpAddField::AppGemini,
        ]
    }

    pub fn input(&self, field: McpAddField) -> Option<&TextInput> {
        match field {
            McpAddField::Id => Some(&self.id),
            McpAddField::Name => Some(&self.name),
            McpAddField::Command => Some(&self.command),
            McpAddField::Args => Some(&self.args),
            McpAddField::AppClaude | McpAddField::AppCodex | McpAddField::AppGemini => None,
        }
    }

    pub fn input_mut(&mut self, field: McpAddField) -> Option<&mut TextInput> {
        match field {
            McpAddField::Id => Some(&mut self.id),
            McpAddField::Name => Some(&mut self.name),
            McpAddField::Command => Some(&mut self.command),
            McpAddField::Args => Some(&mut self.args),
            McpAddField::AppClaude | McpAddField::AppCodex | McpAddField::AppGemini => None,
        }
    }

    pub fn apply_template(&mut self, idx: usize) {
        let idx = idx.min(self.template_count().saturating_sub(1));
        self.template_idx = idx;

        if idx == 0 {
            if matches!(self.mode, FormMode::Add) {
                let defaults = Self::new();
                self.extra = defaults.extra;
                self.name = defaults.name;
                self.command = defaults.command;
                self.args = defaults.args;
                self.env_rows = defaults.env_rows;
                self.json_scroll = defaults.json_scroll;
            }
            return;
        }

        if idx == 1 {
            self.name.set("Filesystem");
            self.command.set("npx");
            self.args
                .set("-y @modelcontextprotocol/server-filesystem /");
        }
    }

    pub fn to_mcp_server_json_value(&self) -> Value {
        let args = self
            .args
            .value
            .split_whitespace()
            .map(|value| Value::String(value.to_string()))
            .collect::<Vec<_>>();

        let mut obj = match self.extra.clone() {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };

        obj.insert("id".to_string(), json!(self.id.value.trim()));
        obj.insert("name".to_string(), json!(self.name.value.trim()));

        let server_value = obj.entry("server".to_string()).or_insert_with(|| json!({}));
        if !server_value.is_object() {
            *server_value = json!({});
        }
        let server_obj = server_value
            .as_object_mut()
            .expect("server must be a JSON object");
        server_obj.insert("command".to_string(), json!(self.command.value.trim()));
        server_obj.insert("args".to_string(), Value::Array(args));
        let env = self
            .env_rows
            .iter()
            .fold(serde_json::Map::new(), |mut map, row| {
                map.insert(row.key.clone(), Value::String(row.value.clone()));
                map
            });
        if env.is_empty() {
            server_obj.remove("env");
        } else {
            server_obj.insert("env".to_string(), Value::Object(env));
        }

        obj.insert(
            "apps".to_string(),
            json!({
                "claude": self.apps.claude,
                "codex": self.apps.codex,
                "gemini": self.apps.gemini,
            }),
        );

        Value::Object(obj)
    }
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
