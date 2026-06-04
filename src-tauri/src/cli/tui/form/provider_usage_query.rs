use crate::app_config::AppType;
use crate::cli::i18n::texts;

use super::{
    FormFocus, ProviderAddFormState, ProviderFormPage, TextInput, UsageQueryField,
    UsageQueryTemplate,
};

impl ProviderAddFormState {
    pub const USAGE_QUERY_GENERAL_PRESET: &'static str = r#"({
  request: {
    url: "{{baseUrl}}/user/balance",
    method: "GET",
    headers: {
      "Authorization": "Bearer {{apiKey}}",
      "User-Agent": "cc-switch/1.0"
    }
  },
  extractor: function(response) {
    return {
      isValid: response.is_active || true,
      remaining: response.balance,
      unit: "USD"
    };
  }
})"#;

    pub const USAGE_QUERY_CUSTOM_PRESET: &'static str = r#"({
  request: {
    url: "",
    method: "GET",
    headers: {}
  },
  extractor: function(response) {
    return {
      remaining: 0,
      unit: "USD"
    };
  }
})"#;

    pub const USAGE_QUERY_NEWAPI_PRESET: &'static str = r#"({
  request: {
    url: "{{baseUrl}}/api/user/self",
    method: "GET",
    headers: {
      "Content-Type": "application/json",
      "Authorization": "Bearer {{accessToken}}",
      "User-Agent": "cc-switch/1.0",
      "New-Api-User": "{{userId}}"
    },
  },
  extractor: function (response) {
    if (response.success && response.data) {
      return {
        planName: response.data.group || "Default Plan",
        remaining: response.data.quota / 500000,
        used: response.data.used_quota / 500000,
        total: (response.data.quota + response.data.used_quota) / 500000,
        unit: "USD",
      };
    }
    return {
      isValid: false,
      invalidMessage: response.message || "Query failed"
    };
  },
})"#;

    pub fn usage_query_fields(&self) -> Vec<UsageQueryField> {
        let mut fields = vec![UsageQueryField::Enabled];

        if !self.usage_query_enabled {
            return fields;
        }

        fields.push(UsageQueryField::Template);

        match self.usage_query_template {
            UsageQueryTemplate::General => {
                fields.extend([
                    UsageQueryField::ApiKey,
                    UsageQueryField::BaseUrl,
                    UsageQueryField::Timeout,
                    UsageQueryField::AutoInterval,
                    UsageQueryField::Script,
                ]);
            }
            UsageQueryTemplate::NewApi => {
                fields.extend([
                    UsageQueryField::BaseUrl,
                    UsageQueryField::AccessToken,
                    UsageQueryField::UserId,
                    UsageQueryField::Timeout,
                    UsageQueryField::AutoInterval,
                    UsageQueryField::Script,
                ]);
            }
            UsageQueryTemplate::Custom => {
                fields.extend([
                    UsageQueryField::Timeout,
                    UsageQueryField::AutoInterval,
                    UsageQueryField::Script,
                ]);
            }
            UsageQueryTemplate::Balance => {
                fields.extend([
                    UsageQueryField::Timeout,
                    UsageQueryField::AutoInterval,
                    UsageQueryField::Script,
                ]);
            }
        }

        fields
    }

    pub fn usage_query_table_fields(&self) -> Vec<UsageQueryField> {
        self.usage_query_fields()
            .into_iter()
            .filter(|field| *field != UsageQueryField::Script)
            .collect()
    }

    pub fn usage_query_input(&self, field: UsageQueryField) -> Option<&TextInput> {
        match field {
            UsageQueryField::ApiKey => Some(&self.usage_query_api_key),
            UsageQueryField::BaseUrl => Some(&self.usage_query_base_url),
            UsageQueryField::AccessToken => Some(&self.usage_query_access_token),
            UsageQueryField::UserId => Some(&self.usage_query_user_id),
            UsageQueryField::Timeout => Some(&self.usage_query_timeout),
            UsageQueryField::AutoInterval => Some(&self.usage_query_auto_interval),
            UsageQueryField::Enabled | UsageQueryField::Template | UsageQueryField::Script => None,
        }
    }

    pub fn usage_query_input_mut(&mut self, field: UsageQueryField) -> Option<&mut TextInput> {
        match field {
            UsageQueryField::ApiKey => Some(&mut self.usage_query_api_key),
            UsageQueryField::BaseUrl => Some(&mut self.usage_query_base_url),
            UsageQueryField::AccessToken => Some(&mut self.usage_query_access_token),
            UsageQueryField::UserId => Some(&mut self.usage_query_user_id),
            UsageQueryField::Timeout => Some(&mut self.usage_query_timeout),
            UsageQueryField::AutoInterval => Some(&mut self.usage_query_auto_interval),
            UsageQueryField::Enabled | UsageQueryField::Template | UsageQueryField::Script => None,
        }
    }

    pub fn open_usage_query_page(&mut self) {
        self.refresh_default_usage_query_template();
        self.page = ProviderFormPage::UsageQuery;
        self.focus = FormFocus::Fields;
        self.editing = false;
        self.usage_query_editing = false;
        let len = self.usage_query_table_fields().len();
        self.usage_query_field_idx = self.usage_query_field_idx.min(len.saturating_sub(1));
    }

    pub fn refresh_default_usage_query_template(&mut self) {
        if self.usage_query_touched || self.has_usage_script_meta() {
            return;
        }

        let template =
            match detect_balance_provider_for_usage_query(&self.current_provider_base_url()) {
                true => UsageQueryTemplate::Balance,
                _ => UsageQueryTemplate::General,
            };

        self.set_usage_query_template(template);
    }

    pub fn close_usage_query_page(&mut self) {
        self.page = ProviderFormPage::Main;
        self.focus = FormFocus::Fields;
        self.usage_query_editing = false;
    }

    pub fn touch_usage_query(&mut self) {
        self.usage_query_touched = true;
    }

    pub fn toggle_usage_query_enabled(&mut self) {
        self.usage_query_enabled = !self.usage_query_enabled;
        self.touch_usage_query();
    }

    pub fn selected_usage_query_field(&self) -> Option<UsageQueryField> {
        let fields = self.usage_query_table_fields();
        fields
            .get(
                self.usage_query_field_idx
                    .min(fields.len().saturating_sub(1)),
            )
            .copied()
    }

    pub fn available_usage_query_templates(&self) -> Vec<UsageQueryTemplate> {
        vec![
            UsageQueryTemplate::Custom,
            UsageQueryTemplate::General,
            UsageQueryTemplate::NewApi,
            UsageQueryTemplate::Balance,
        ]
    }

    pub fn set_usage_query_template(&mut self, template: UsageQueryTemplate) {
        self.usage_query_template = template;
        match template {
            UsageQueryTemplate::Custom => {
                self.usage_query_code = self.usage_query_custom_preset_with_variables();
                self.usage_query_api_key.set("");
                self.usage_query_base_url.set("");
                self.usage_query_access_token.set("");
                self.usage_query_user_id.set("");
            }
            UsageQueryTemplate::General => {
                self.usage_query_code = Self::USAGE_QUERY_GENERAL_PRESET.to_string();
                self.usage_query_access_token.set("");
                self.usage_query_user_id.set("");
            }
            UsageQueryTemplate::NewApi => {
                self.usage_query_code = Self::USAGE_QUERY_NEWAPI_PRESET.to_string();
                self.usage_query_api_key.set("");
            }
            UsageQueryTemplate::Balance => {
                self.usage_query_code.clear();
                self.usage_query_api_key.set("");
                self.usage_query_base_url.set("");
                self.usage_query_access_token.set("");
                self.usage_query_user_id.set("");
            }
        }
        let len = self.usage_query_table_fields().len();
        self.usage_query_field_idx = self.usage_query_field_idx.min(len.saturating_sub(1));
    }

    pub fn refresh_usage_query_custom_variable_comment(&mut self) {
        if self.usage_query_template != UsageQueryTemplate::Custom {
            return;
        }

        let Some(body) = Self::strip_usage_query_custom_variable_comment(&self.usage_query_code)
            .map(str::to_string)
        else {
            return;
        };
        let next = format!("{}{}", self.usage_query_custom_variable_comment(), body);
        if self.usage_query_code != next {
            self.usage_query_code = next;
            self.touch_usage_query();
        }
    }

    pub fn usage_query_script_help_lines() -> Vec<String> {
        vec![
            texts::tui_usage_query_config_format().to_string(),
            "({".to_string(),
            "  request: {".to_string(),
            "    url: \"{{baseUrl}}/api/usage\",".to_string(),
            "    method: \"POST\",".to_string(),
            "    headers: {".to_string(),
            "      \"Authorization\": \"Bearer {{apiKey}}\",".to_string(),
            "      \"User-Agent\": \"cc-switch/1.0\"".to_string(),
            "    }".to_string(),
            "  },".to_string(),
            "  extractor: function(response) {".to_string(),
            "    return {".to_string(),
            "      isValid: !response.error,".to_string(),
            "      remaining: response.balance,".to_string(),
            "      unit: \"USD\"".to_string(),
            "    };".to_string(),
            "  }".to_string(),
            "})".to_string(),
            String::new(),
            texts::tui_usage_query_extractor_format().to_string(),
            texts::tui_usage_query_field_is_valid().to_string(),
            texts::tui_usage_query_field_invalid_message().to_string(),
            texts::tui_usage_query_field_remaining().to_string(),
            texts::tui_usage_query_field_unit().to_string(),
            texts::tui_usage_query_field_plan_name().to_string(),
            texts::tui_usage_query_field_total().to_string(),
            texts::tui_usage_query_field_used().to_string(),
            texts::tui_usage_query_field_extra().to_string(),
            String::new(),
            texts::tui_usage_query_tips().to_string(),
            texts::tui_usage_query_tip1().to_string(),
            texts::tui_usage_query_tip2().to_string(),
            texts::tui_usage_query_tip3().to_string(),
        ]
    }

    pub fn usage_query_template_value(&self) -> &'static str {
        self.usage_query_template.as_str()
    }

    pub fn usage_query_template_label(&self) -> &'static str {
        self.usage_query_template.label()
    }

    pub fn usage_query_extractor_available(&self) -> bool {
        self.usage_query_enabled
    }

    pub(crate) fn should_skip_usage_query_validation(&self) -> bool {
        self.has_usage_script_meta() && !self.usage_query_touched
    }

    fn usage_query_custom_preset_with_variables(&self) -> String {
        format!(
            "{}{}",
            self.usage_query_custom_variable_comment(),
            Self::USAGE_QUERY_CUSTOM_PRESET
        )
    }

    fn usage_query_custom_variable_comment(&self) -> String {
        let (api_key, base_url) = self.usage_query_provider_credentials();
        format!(
            "// 支持的变量\n// {{{{baseUrl}}}}\n// =\n// {base_url}\n// {{{{apiKey}}}}\n// =\n// {api_key}\n\n"
        )
    }

    pub fn current_provider_base_url(&self) -> String {
        if self.is_claude_codex_oauth_provider() {
            return "https://chatgpt.com/backend-api/codex".to_string();
        }

        match self.app_type {
            AppType::Claude => self.claude_base_url.value.clone(),
            AppType::Codex => self.codex_base_url.value.clone(),
            AppType::Gemini => self.gemini_base_url.value.clone(),
            AppType::Hermes => self.hermes_base_url.value.clone(),
            AppType::OpenCode | AppType::OpenClaw => self.opencode_base_url.value.clone(),
        }
    }

    fn usage_query_provider_credentials(&self) -> (String, String) {
        if self.is_claude_codex_oauth_provider() {
            return (
                String::new(),
                "https://chatgpt.com/backend-api/codex".to_string(),
            );
        }

        let (api_key, base_url) = match self.app_type {
            AppType::Claude => (&self.claude_api_key.value, &self.claude_base_url.value),
            AppType::Codex => (&self.codex_api_key.value, &self.codex_base_url.value),
            AppType::Gemini => (&self.gemini_api_key.value, &self.gemini_base_url.value),
            AppType::Hermes => (&self.hermes_api_key.value, &self.hermes_base_url.value),
            AppType::OpenCode | AppType::OpenClaw => {
                (&self.opencode_api_key.value, &self.opencode_base_url.value)
            }
        };
        (
            Self::usage_query_comment_value(api_key),
            Self::usage_query_comment_value(base_url),
        )
    }

    fn usage_query_comment_value(value: &str) -> String {
        value.trim().replace(['\r', '\n'], " ").trim().to_string()
    }

    fn strip_usage_query_custom_variable_comment(code: &str) -> Option<&str> {
        if !code.starts_with("// 支持的变量\n") {
            return None;
        }

        let mut newline_count = 0;
        for (idx, ch) in code.char_indices() {
            if ch == '\n' {
                newline_count += 1;
                if newline_count == 8 {
                    return code.get(idx + ch.len_utf8()..);
                }
            }
        }
        None
    }
}

pub(crate) fn detect_balance_provider_for_usage_query(base_url: &str) -> bool {
    let url = base_url.to_lowercase();
    url.contains("api.deepseek.com")
        || url.contains("api.stepfun.ai")
        || url.contains("api.stepfun.com")
        || url.contains("api.siliconflow.cn")
        || url.contains("api.siliconflow.com")
        || url.contains("openrouter.ai")
        || url.contains("api.novita.ai")
}

impl UsageQueryTemplate {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::General => "general",
            Self::NewApi => "newapi",
            Self::Balance => "balance",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Custom => {
                if crate::cli::i18n::is_chinese() {
                    "自定义"
                } else {
                    "Custom"
                }
            }
            Self::General => {
                if crate::cli::i18n::is_chinese() {
                    "通用模板"
                } else {
                    "General"
                }
            }
            Self::NewApi => "NewAPI",
            Self::Balance => {
                if crate::cli::i18n::is_chinese() {
                    "官方"
                } else {
                    "Official"
                }
            }
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "custom" => Some(Self::Custom),
            "general" => Some(Self::General),
            "newapi" => Some(Self::NewApi),
            "balance" => Some(Self::Balance),
            _ => None,
        }
    }
}
