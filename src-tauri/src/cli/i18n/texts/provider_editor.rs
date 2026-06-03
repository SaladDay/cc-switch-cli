use super::is_chinese;
pub fn id_label() -> &'static str {
    if is_chinese() {
        "ID"
    } else {
        "ID"
    }
}

pub fn website_label() -> &'static str {
    if is_chinese() {
        "官网"
    } else {
        "Website"
    }
}

pub fn core_config_label() -> &'static str {
    if is_chinese() {
        "核心配置："
    } else {
        "Core Configuration:"
    }
}

pub fn model_label() -> &'static str {
    if is_chinese() {
        "模型"
    } else {
        "Model"
    }
}

pub fn config_toml_lines(count: usize) -> String {
    if is_chinese() {
        format!("Config (TOML): {} 行", count)
    } else {
        format!("Config (TOML): {} lines", count)
    }
}

pub fn optional_fields_label() -> &'static str {
    if is_chinese() {
        "可选字段："
    } else {
        "Optional Fields:"
    }
}

pub fn notes_label_colon() -> &'static str {
    if is_chinese() {
        "备注"
    } else {
        "Notes"
    }
}

pub fn sort_index_label_colon() -> &'static str {
    if is_chinese() {
        "排序索引"
    } else {
        "Sort Index"
    }
}

pub fn id_label_colon() -> &'static str {
    if is_chinese() {
        "ID"
    } else {
        "ID"
    }
}

pub fn url_label_colon() -> &'static str {
    if is_chinese() {
        "网址"
    } else {
        "URL"
    }
}

pub fn api_url_label_colon() -> &'static str {
    if is_chinese() {
        "API 地址"
    } else {
        "API URL"
    }
}

pub fn summary_divider() -> &'static str {
    "======================"
}

// Provider Input - Summary Display
pub fn basic_info_header() -> &'static str {
    if is_chinese() {
        "基本信息"
    } else {
        "Basic Info"
    }
}

pub fn name_display_label() -> &'static str {
    if is_chinese() {
        "名称"
    } else {
        "Name"
    }
}

pub fn app_display_label() -> &'static str {
    if is_chinese() {
        "应用"
    } else {
        "App"
    }
}

pub fn notes_display_label() -> &'static str {
    if is_chinese() {
        "备注"
    } else {
        "Notes"
    }
}

pub fn sort_index_display_label() -> &'static str {
    if is_chinese() {
        "排序"
    } else {
        "Sort Index"
    }
}

pub fn config_info_header() -> &'static str {
    if is_chinese() {
        "配置信息"
    } else {
        "Configuration"
    }
}

pub fn api_key_display_label() -> &'static str {
    if is_chinese() {
        "API Key"
    } else {
        "API Key"
    }
}

pub fn base_url_display_label() -> &'static str {
    if is_chinese() {
        "Base URL"
    } else {
        "Base URL"
    }
}

pub fn model_config_header() -> &'static str {
    if is_chinese() {
        "模型配置"
    } else {
        "Model Configuration"
    }
}

pub fn default_model_display() -> &'static str {
    if is_chinese() {
        "默认"
    } else {
        "Default"
    }
}

pub fn haiku_model_display() -> &'static str {
    if is_chinese() {
        "Haiku"
    } else {
        "Haiku"
    }
}

pub fn sonnet_model_display() -> &'static str {
    if is_chinese() {
        "Sonnet"
    } else {
        "Sonnet"
    }
}

pub fn opus_model_display() -> &'static str {
    if is_chinese() {
        "Opus"
    } else {
        "Opus"
    }
}

pub fn auth_type_display_label() -> &'static str {
    if is_chinese() {
        "认证"
    } else {
        "Auth Type"
    }
}

pub fn project_id_display_label() -> &'static str {
    if is_chinese() {
        "项目 ID"
    } else {
        "Project ID"
    }
}

pub fn location_display_label() -> &'static str {
    if is_chinese() {
        "位置"
    } else {
        "Location"
    }
}

// Interactive Provider - Menu Options
pub fn edit_provider_menu() -> &'static str {
    if is_chinese() {
        "➕ 编辑供应商"
    } else {
        "➕ Edit Provider"
    }
}

pub fn no_editable_providers() -> &'static str {
    if is_chinese() {
        "没有可编辑的供应商"
    } else {
        "No providers available for editing"
    }
}

pub fn select_provider_to_edit() -> &'static str {
    if is_chinese() {
        "选择要编辑的供应商："
    } else {
        "Select provider to edit:"
    }
}

pub fn choose_edit_mode() -> &'static str {
    if is_chinese() {
        "选择编辑模式："
    } else {
        "Choose edit mode:"
    }
}

pub fn select_config_file_to_edit() -> &'static str {
    if is_chinese() {
        "选择要编辑的配置文件："
    } else {
        "Select config file to edit:"
    }
}

pub fn provider_missing_auth_field() -> &'static str {
    if is_chinese() {
        "settings_config 中缺少 'auth' 字段"
    } else {
        "Missing 'auth' field in settings_config"
    }
}

pub fn provider_missing_or_invalid_config_field() -> &'static str {
    if is_chinese() {
        "settings_config 中缺少或无效的 'config' 字段"
    } else {
        "Missing or invalid 'config' field in settings_config"
    }
}

pub fn edit_mode_interactive() -> &'static str {
    if is_chinese() {
        "📝 交互式编辑 (分步提示)"
    } else {
        "📝 Interactive editing (step-by-step prompts)"
    }
}

pub fn edit_mode_json_editor() -> &'static str {
    if is_chinese() {
        "✏️  JSON 编辑 (使用外部编辑器)"
    } else {
        "✏️  JSON editing (use external editor)"
    }
}

pub fn cancel() -> &'static str {
    if is_chinese() {
        "❌ 取消"
    } else {
        "❌ Cancel"
    }
}

pub fn opening_external_editor() -> &'static str {
    if is_chinese() {
        "正在打开外部编辑器..."
    } else {
        "Opening external editor..."
    }
}

pub fn invalid_json_syntax() -> &'static str {
    if is_chinese() {
        "无效的 JSON 语法"
    } else {
        "Invalid JSON syntax"
    }
}

pub fn invalid_provider_structure() -> &'static str {
    if is_chinese() {
        "无效的供应商结构"
    } else {
        "Invalid provider structure"
    }
}

pub fn provider_id_cannot_be_changed() -> &'static str {
    if is_chinese() {
        "供应商 ID 不能被修改"
    } else {
        "Provider ID cannot be changed"
    }
}

pub fn provider_id_empty_error() -> &'static str {
    if is_chinese() {
        "供应商 ID 不能为空"
    } else {
        "Provider ID cannot be empty"
    }
}

pub fn retry_editing() -> &'static str {
    if is_chinese() {
        "是否重新编辑？"
    } else {
        "Retry editing?"
    }
}

pub fn no_changes_detected() -> &'static str {
    if is_chinese() {
        "未检测到任何更改"
    } else {
        "No changes detected"
    }
}

pub fn provider_summary() -> &'static str {
    if is_chinese() {
        "供应商信息摘要"
    } else {
        "Provider Summary"
    }
}

pub fn confirm_save_changes() -> &'static str {
    if is_chinese() {
        "确认保存更改？"
    } else {
        "Save changes?"
    }
}

pub fn editor_failed() -> &'static str {
    if is_chinese() {
        "编辑器失败"
    } else {
        "Editor failed"
    }
}

pub fn invalid_selection_format() -> &'static str {
    if is_chinese() {
        "无效的选择格式"
    } else {
        "Invalid selection format"
    }
}

// Provider Display Labels (for show_current and view_provider_detail)
pub fn basic_info_section_header() -> &'static str {
    if is_chinese() {
        "基本信息 / Basic Info"
    } else {
        "Basic Info"
    }
}

pub fn name_label_with_colon() -> &'static str {
    if is_chinese() {
        "名称"
    } else {
        "Name"
    }
}

pub fn app_label_with_colon() -> &'static str {
    if is_chinese() {
        "应用"
    } else {
        "App"
    }
}

pub fn api_config_section_header() -> &'static str {
    if is_chinese() {
        "API 配置 / API Configuration"
    } else {
        "API Configuration"
    }
}

pub fn model_config_section_header() -> &'static str {
    if is_chinese() {
        "模型配置 / Model Configuration"
    } else {
        "Model Configuration"
    }
}

pub fn main_model_label_with_colon() -> &'static str {
    if is_chinese() {
        "主模型"
    } else {
        "Main Model"
    }
}

pub fn updated_config_header() -> &'static str {
    if is_chinese() {
        "修改后配置："
    } else {
        "Updated Configuration:"
    }
}

// Provider Add/Edit Messages
pub fn generated_id_message(id: &str) -> String {
    if is_chinese() {
        format!("生成的 ID: {}", id)
    } else {
        format!("Generated ID: {}", id)
    }
}

pub fn edit_fields_instruction() -> &'static str {
    if is_chinese() {
        "逐个编辑字段（直接回车保留当前值）：\n"
    } else {
        "Edit fields one by one (press Enter to keep current value):\n"
    }
}

// ============================================
// MCP SERVER MANAGEMENT (MCP 服务器管理)
// ============================================

pub fn mcp_management() -> &'static str {
    if is_chinese() {
        "🛠️  MCP 服务器管理"
    } else {
        "🛠️  MCP Server Management"
    }
}

pub fn no_mcp_servers() -> &'static str {
    if is_chinese() {
        "未找到 MCP 服务器。"
    } else {
        "No MCP servers found."
    }
}

pub fn sync_all_servers() -> &'static str {
    if is_chinese() {
        "🔄 同步所有服务器"
    } else {
        "🔄 Sync All Servers"
    }
}

pub fn synced_successfully() -> &'static str {
    if is_chinese() {
        "✓ 所有 MCP 服务器同步成功"
    } else {
        "✓ All MCP servers synced successfully"
    }
}

// ============================================
// PROMPT MANAGEMENT (提示词管理)
// ============================================

pub fn prompts_management() -> &'static str {
    if is_chinese() {
        "💬 提示词管理"
    } else {
        "💬 Prompt Management"
    }
}

pub fn no_prompts() -> &'static str {
    if is_chinese() {
        "未找到提示词预设。"
    } else {
        "No prompt presets found."
    }
}

pub fn switch_active_prompt() -> &'static str {
    if is_chinese() {
        "🔄 切换活动提示词"
    } else {
        "🔄 Switch Active Prompt"
    }
}

pub fn no_prompts_available() -> &'static str {
    if is_chinese() {
        "没有可用的提示词。"
    } else {
        "No prompts available."
    }
}

pub fn select_prompt_to_activate() -> &'static str {
    if is_chinese() {
        "选择要激活的提示词："
    } else {
        "Select prompt to activate:"
    }
}

pub fn activated_prompt(id: &str) -> String {
    if is_chinese() {
        format!("✓ 已激活提示词 '{}'", id)
    } else {
        format!("✓ Activated prompt '{}'", id)
    }
}

pub fn deactivated_prompt(id: &str) -> String {
    if is_chinese() {
        format!("✓ 已取消激活提示词 '{}'", id)
    } else {
        format!("✓ Deactivated prompt '{}'", id)
    }
}

pub fn prompt_cleared_note() -> &'static str {
    if is_chinese() {
        "实时文件已清空"
    } else {
        "Live prompt file has been cleared"
    }
}

pub fn prompt_synced_note() -> &'static str {
    if is_chinese() {
        "注意：提示词已同步到实时配置文件。"
    } else {
        "Note: The prompt has been synced to the live configuration file."
    }
}

// Configuration View
pub fn current_configuration() -> &'static str {
    if is_chinese() {
        "👁️  当前配置"
    } else {
        "👁️  Current Configuration"
    }
}

pub fn provider_label() -> &'static str {
    if is_chinese() {
        "供应商："
    } else {
        "Provider:"
    }
}

pub fn mcp_servers_label() -> &'static str {
    if is_chinese() {
        "MCP 服务器："
    } else {
        "MCP Servers:"
    }
}

pub fn tui_label_mcp_short() -> &'static str {
    "MCP:"
}
