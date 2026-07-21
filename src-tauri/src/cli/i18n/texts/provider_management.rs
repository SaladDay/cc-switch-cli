use super::is_chinese;
pub fn provider_management() -> &'static str {
    if is_chinese() {
        "🔌 供应商管理"
    } else {
        "🔌 Provider Management"
    }
}

pub fn no_providers() -> &'static str {
    if is_chinese() {
        "未找到供应商。"
    } else {
        "No providers found."
    }
}

pub fn view_current_provider() -> &'static str {
    if is_chinese() {
        "📋 查看当前供应商详情"
    } else {
        "📋 View Current Provider Details"
    }
}

pub fn switch_provider() -> &'static str {
    if is_chinese() {
        "🔄 切换供应商"
    } else {
        "🔄 Switch Provider"
    }
}

pub fn add_provider() -> &'static str {
    if is_chinese() {
        "➕ 新增供应商"
    } else {
        "➕ Add Provider"
    }
}

pub fn add_official_provider() -> &'static str {
    if is_chinese() {
        "添加官方供应商"
    } else {
        "Add Official Provider"
    }
}

pub fn add_third_party_provider() -> &'static str {
    if is_chinese() {
        "添加第三方供应商"
    } else {
        "Add Third-Party Provider"
    }
}

pub fn select_provider_add_mode() -> &'static str {
    if is_chinese() {
        "请选择供应商类型："
    } else {
        "Select provider type:"
    }
}

pub fn delete_provider() -> &'static str {
    if is_chinese() {
        "🗑️  删除供应商"
    } else {
        "🗑️  Delete Provider"
    }
}

pub fn back_to_main() -> &'static str {
    if is_chinese() {
        "⬅️  返回主菜单"
    } else {
        "⬅️  Back to Main Menu"
    }
}

pub fn choose_action() -> &'static str {
    if is_chinese() {
        "选择操作："
    } else {
        "Choose an action:"
    }
}

pub fn esc_to_go_back_help() -> &'static str {
    if is_chinese() {
        "Esc 返回上一步"
    } else {
        "Esc to go back"
    }
}

pub fn select_filter_help() -> &'static str {
    if is_chinese() {
        "Esc 返回；输入可过滤"
    } else {
        "Esc to go back; type to filter"
    }
}

pub fn current_provider_details() -> &'static str {
    if is_chinese() {
        "当前供应商详情"
    } else {
        "Current Provider Details"
    }
}

pub fn only_one_provider() -> &'static str {
    if is_chinese() {
        "只有一个供应商，无法切换。"
    } else {
        "Only one provider available. Cannot switch."
    }
}

pub fn no_other_providers() -> &'static str {
    if is_chinese() {
        "没有其他供应商可切换。"
    } else {
        "No other providers to switch to."
    }
}

pub fn select_provider_to_switch() -> &'static str {
    if is_chinese() {
        "选择要切换到的供应商："
    } else {
        "Select provider to switch to:"
    }
}

pub fn switched_to_provider(id: &str) -> String {
    if is_chinese() {
        format!("✓ 已切换到供应商 '{}'", id)
    } else {
        format!("✓ Switched to provider '{}'", id)
    }
}

pub fn restart_note() -> &'static str {
    if is_chinese() {
        "注意：请重启 CLI 客户端以应用更改。"
    } else {
        "Note: Restart your CLI client to apply the changes."
    }
}

pub fn live_sync_skipped_uninitialized_warning(app: &str) -> String {
    if is_chinese() {
        format!("⚠ 未检测到 {app} 客户端本地配置，已跳过写入 live 文件；先运行一次 {app} 初始化后再试。")
    } else {
        format!("⚠ Live sync skipped: {app} client not initialized; run it once to initialize, then retry.")
    }
}

pub fn no_deletable_providers() -> &'static str {
    if is_chinese() {
        "没有可删除的供应商（无法删除当前供应商）。"
    } else {
        "No providers available for deletion (cannot delete current provider)."
    }
}

pub fn select_provider_to_delete() -> &'static str {
    if is_chinese() {
        "选择要删除的供应商："
    } else {
        "Select provider to delete:"
    }
}

pub fn confirm_delete(id: &str) -> String {
    if is_chinese() {
        format!("确定要删除供应商 '{}' 吗？", id)
    } else {
        format!("Are you sure you want to delete provider '{}'?", id)
    }
}

pub fn cancelled() -> &'static str {
    if is_chinese() {
        "已取消。"
    } else {
        "Cancelled."
    }
}

pub fn selection_cancelled() -> &'static str {
    if is_chinese() {
        "已取消选择"
    } else {
        "Selection cancelled"
    }
}

pub fn invalid_selection() -> &'static str {
    if is_chinese() {
        "选择无效"
    } else {
        "Invalid selection"
    }
}

pub fn available_backups() -> &'static str {
    if is_chinese() {
        "可用备份"
    } else {
        "Available Backups"
    }
}

pub fn no_backups_found() -> &'static str {
    if is_chinese() {
        "未找到备份。"
    } else {
        "No backups found."
    }
}

pub fn create_backup_first_hint() -> &'static str {
    if is_chinese() {
        "请先创建备份：cc-switch config backup"
    } else {
        "Create a backup first: cc-switch config backup"
    }
}

pub fn found_backups(count: usize) -> String {
    if is_chinese() {
        format!("找到 {} 个备份：", count)
    } else {
        format!("Found {} backup(s):", count)
    }
}

pub fn select_backup_to_restore() -> &'static str {
    if is_chinese() {
        "选择要恢复的备份："
    } else {
        "Select backup to restore:"
    }
}

pub fn warning_title() -> &'static str {
    if is_chinese() {
        "警告："
    } else {
        "Warning:"
    }
}

pub fn config_restore_warning_replace() -> &'static str {
    if is_chinese() {
        "这将用所选备份替换你当前的配置。"
    } else {
        "This will replace your current configuration with the selected backup."
    }
}

pub fn config_restore_warning_pre_backup() -> &'static str {
    if is_chinese() {
        "系统会先创建一次当前状态的备份。"
    } else {
        "A backup of the current state will be created first."
    }
}

pub fn config_restore_confirm_prompt() -> &'static str {
    if is_chinese() {
        "确认继续恢复？"
    } else {
        "Continue with restore?"
    }
}

pub fn deleted_provider(id: &str) -> String {
    if is_chinese() {
        format!("✓ 已删除供应商 '{}'", id)
    } else {
        format!("✓ Deleted provider '{}'", id)
    }
}

// Provider Input - Basic Fields
pub fn provider_name_label() -> &'static str {
    if is_chinese() {
        "供应商名称："
    } else {
        "Provider Name:"
    }
}

pub fn provider_name_help() -> &'static str {
    if is_chinese() {
        "必填，用于显示的友好名称"
    } else {
        "Required, friendly display name"
    }
}

pub fn provider_name_help_edit() -> &'static str {
    if is_chinese() {
        "必填，直接回车保持原值"
    } else {
        "Required, press Enter to keep"
    }
}

pub fn provider_name_placeholder() -> &'static str {
    "OpenAI"
}

pub fn provider_name_empty_error() -> &'static str {
    if is_chinese() {
        "供应商名称不能为空"
    } else {
        "Provider name cannot be empty"
    }
}

pub fn website_url_label() -> &'static str {
    if is_chinese() {
        "官网 URL（可选）："
    } else {
        "Website URL (opt.):"
    }
}

pub fn website_url_help() -> &'static str {
    if is_chinese() {
        "供应商的网站地址，直接回车跳过"
    } else {
        "Provider's website, press Enter to skip"
    }
}

pub fn website_url_help_edit() -> &'static str {
    if is_chinese() {
        "留空则不修改，直接回车跳过"
    } else {
        "Leave blank to keep, Enter to skip"
    }
}

pub fn website_url_placeholder() -> &'static str {
    "https://openai.com"
}

// Provider Commands
pub fn no_providers_hint() -> &'static str {
    "Use 'cc-switch provider add' to create a new provider."
}

pub fn app_config_not_found(app: &str) -> String {
    if is_chinese() {
        format!("应用 {} 配置不存在", app)
    } else {
        format!("Application {} configuration not found", app)
    }
}

pub fn provider_not_found(id: &str) -> String {
    if is_chinese() {
        format!("供应商不存在: {}", id)
    } else {
        format!("Provider not found: {}", id)
    }
}

pub fn generated_id(id: &str) -> String {
    if is_chinese() {
        format!("生成的 ID: {}", id)
    } else {
        format!("Generated ID: {}", id)
    }
}

pub fn configure_optional_fields_prompt() -> &'static str {
    if is_chinese() {
        "配置可选字段（备注、排序索引）？"
    } else {
        "Configure optional fields (notes, sort index)?"
    }
}

pub fn current_config_header() -> &'static str {
    if is_chinese() {
        "当前配置："
    } else {
        "Current Configuration:"
    }
}

pub fn modify_provider_config_prompt() -> &'static str {
    if is_chinese() {
        "修改供应商配置（API Key, Base URL 等）？"
    } else {
        "Modify provider configuration (API Key, Base URL, etc.)?"
    }
}

pub fn modify_optional_fields_prompt() -> &'static str {
    if is_chinese() {
        "修改可选字段（备注、排序索引）？"
    } else {
        "Modify optional fields (notes, sort index)?"
    }
}

pub fn current_provider_synced_warning() -> &'static str {
    if is_chinese() {
        "⚠ 此供应商当前已激活，修改已同步到 live 配置"
    } else {
        "⚠ This provider is currently active, changes synced to live config"
    }
}

pub fn input_failed_error(err: &str) -> String {
    if is_chinese() {
        format!("输入失败: {}", err)
    } else {
        format!("Input failed: {}", err)
    }
}

pub fn cannot_delete_current_provider() -> &'static str {
    "Cannot delete the current active provider. Please switch to another provider first."
}

// Provider Input - Basic Fields
pub fn provider_name_prompt() -> &'static str {
    if is_chinese() {
        "供应商名称："
    } else {
        "Provider Name:"
    }
}

// Provider Input - Claude Configuration
pub fn config_claude_header() -> &'static str {
    if is_chinese() {
        "配置 Claude 供应商："
    } else {
        "Configure Claude Provider:"
    }
}

pub fn api_key_label() -> &'static str {
    if is_chinese() {
        "API Key："
    } else {
        "API Key:"
    }
}

pub fn api_key_help() -> &'static str {
    if is_chinese() {
        "留空使用默认值"
    } else {
        "Leave empty to use default"
    }
}

pub fn base_url_label() -> &'static str {
    if is_chinese() {
        "Base URL："
    } else {
        "Base URL:"
    }
}

pub fn base_url_empty_error() -> &'static str {
    if is_chinese() {
        "API 请求地址不能为空"
    } else {
        "API URL cannot be empty"
    }
}

pub fn base_url_placeholder() -> &'static str {
    if is_chinese() {
        "如 https://api.anthropic.com"
    } else {
        "e.g., https://api.anthropic.com"
    }
}

pub fn configure_model_names_prompt() -> &'static str {
    if is_chinese() {
        "配置模型名称？"
    } else {
        "Configure model names?"
    }
}

pub fn model_default_label() -> &'static str {
    if is_chinese() {
        "默认模型："
    } else {
        "Default Model:"
    }
}

pub fn model_default_help() -> &'static str {
    if is_chinese() {
        "留空使用 Claude Code 默认模型"
    } else {
        "Leave empty to use Claude Code default"
    }
}

pub fn model_haiku_label() -> &'static str {
    if is_chinese() {
        "Haiku 模型："
    } else {
        "Haiku Model:"
    }
}

pub fn model_haiku_placeholder() -> &'static str {
    if is_chinese() {
        "如 claude-3-5-haiku-20241022"
    } else {
        "e.g., claude-3-5-haiku-20241022"
    }
}

pub fn model_sonnet_label() -> &'static str {
    if is_chinese() {
        "Sonnet 模型："
    } else {
        "Sonnet Model:"
    }
}

pub fn model_sonnet_placeholder() -> &'static str {
    if is_chinese() {
        "如 claude-3-5-sonnet-20241022"
    } else {
        "e.g., claude-3-5-sonnet-20241022"
    }
}

pub fn model_opus_label() -> &'static str {
    if is_chinese() {
        "Opus 模型："
    } else {
        "Opus Model:"
    }
}

pub fn model_opus_placeholder() -> &'static str {
    if is_chinese() {
        "如 claude-3-opus-20240229"
    } else {
        "e.g., claude-3-opus-20240229"
    }
}

// Provider Input - Codex Configuration
pub fn config_codex_header() -> &'static str {
    if is_chinese() {
        "配置 Codex 供应商："
    } else {
        "Configure Codex Provider:"
    }
}

pub fn openai_api_key_label() -> &'static str {
    if is_chinese() {
        "OpenAI API Key："
    } else {
        "OpenAI API Key:"
    }
}

pub fn anthropic_api_key_label() -> &'static str {
    if is_chinese() {
        "Anthropic API Key："
    } else {
        "Anthropic API Key:"
    }
}

pub fn config_toml_label() -> &'static str {
    if is_chinese() {
        "配置内容 (TOML)："
    } else {
        "Config Content (TOML):"
    }
}

pub fn config_toml_help() -> &'static str {
    if is_chinese() {
        "按 Esc 后 Enter 提交"
    } else {
        "Press Esc then Enter to submit"
    }
}

pub fn config_toml_placeholder() -> &'static str {
    if is_chinese() {
        "留空使用默认配置"
    } else {
        "Leave empty to use default config"
    }
}

// Codex 0.64+ Configuration
pub fn codex_auth_mode_info() -> &'static str {
    if is_chinese() {
        "⚠ 请选择 Codex 的鉴权方式（决定 API Key 从哪里读取）"
    } else {
        "⚠ Choose how Codex authenticates (where the API key is read from)"
    }
}

pub fn codex_auth_mode_label() -> &'static str {
    if is_chinese() {
        "认证方式："
    } else {
        "Auth Mode:"
    }
}

pub fn codex_auth_mode_help() -> &'static str {
    if is_chinese() {
        "OpenAI 认证：使用 auth.json/凭据存储；环境变量：使用 env_key 指定的变量（未设置会报错）"
    } else {
        "OpenAI auth uses auth.json/credential store; env var mode uses env_key (missing env var will error)"
    }
}

pub fn codex_auth_mode_openai() -> &'static str {
    if is_chinese() {
        "OpenAI 认证（推荐，无需环境变量）"
    } else {
        "OpenAI auth (recommended, no env var)"
    }
}

pub fn codex_auth_mode_env_var() -> &'static str {
    if is_chinese() {
        "环境变量（env_key，需要手动 export）"
    } else {
        "Environment variable (env_key, requires export)"
    }
}

pub fn codex_official_provider_tip() -> &'static str {
    if is_chinese() {
        "提示：官方供应商将使用 Codex 官方登录保存的凭证（codex login 可能会打开浏览器），无需填写 API Key"
    } else {
        "Tip: Official provider uses Codex login credentials (`codex login` may open a browser); no API key required"
    }
}

pub fn codex_env_key_info() -> &'static str {
    if is_chinese() {
        "⚠ 环境变量模式：Codex 将从指定的环境变量读取 API Key"
    } else {
        "⚠ Env var mode: Codex will read the API key from the specified environment variable"
    }
}

pub fn codex_env_key_label() -> &'static str {
    if is_chinese() {
        "环境变量名称："
    } else {
        "Environment Variable Name:"
    }
}

pub fn codex_env_key_help() -> &'static str {
    if is_chinese() {
        "Codex 将从此环境变量读取 API 密钥（默认: OPENAI_API_KEY）"
    } else {
        "Codex will read API key from this env var (default: OPENAI_API_KEY)"
    }
}

pub fn codex_wire_api_label() -> &'static str {
    if is_chinese() {
        "API 格式："
    } else {
        "API Format:"
    }
}

pub fn codex_wire_api_help() -> &'static str {
    if is_chinese() {
        "chat = Chat Completions API (大多数第三方), responses = OpenAI Responses API"
    } else {
        "chat = Chat Completions API (most providers), responses = OpenAI Responses API"
    }
}

pub fn codex_env_reminder(env_key: &str) -> String {
    if is_chinese() {
        format!(
            "⚠ 请确保已设置环境变量 {} 并包含您的 API 密钥\n  例如: export {}=\"your-api-key\"",
            env_key, env_key
        )
    } else {
        format!(
            "⚠ Make sure to set the {} environment variable with your API key\n  Example: export {}=\"your-api-key\"",
            env_key, env_key
        )
    }
}

pub fn codex_openai_auth_info() -> &'static str {
    if is_chinese() {
        "✓ OpenAI 认证模式：Codex 将使用 auth.json/系统凭据存储，无需设置 OPENAI_API_KEY 环境变量"
    } else {
        "✓ OpenAI auth mode: Codex will use auth.json/credential store; no OPENAI_API_KEY env var required"
    }
}

pub fn codex_dual_write_info(env_key: &str, _api_key: &str) -> String {
    if is_chinese() {
        format!(
            "✓ 双写模式已启用（兼容所有 Codex 版本）\n\
              • 旧版本 Codex: 将使用 auth.json 中的 API Key\n\
              • Codex 0.64+: 可使用环境变量 {} (更安全)\n\
                例如: export {}=\"your-api-key\"",
            env_key, env_key
        )
    } else {
        format!(
            "✓ Dual-write mode enabled (compatible with all Codex versions)\n\
              • Legacy Codex: Will use API Key from auth.json\n\
              • Codex 0.64+: Can use env variable {} (more secure)\n\
                Example: export {}=\"your-api-key\"",
            env_key, env_key
        )
    }
}

pub fn use_current_config_prompt() -> &'static str {
    if is_chinese() {
        "使用当前配置？"
    } else {
        "Use current configuration?"
    }
}

pub fn use_current_config_help() -> &'static str {
    if is_chinese() {
        "选择 No 将进入自定义输入模式"
    } else {
        "Select No to enter custom input mode"
    }
}

pub fn input_toml_config() -> &'static str {
    if is_chinese() {
        "输入 TOML 配置（多行，输入空行结束）："
    } else {
        "Enter TOML config (multiple lines, empty line to finish):"
    }
}

pub fn direct_enter_to_finish() -> &'static str {
    if is_chinese() {
        "直接回车结束输入"
    } else {
        "Press Enter to finish"
    }
}

pub fn current_config_label() -> &'static str {
    if is_chinese() {
        "当前配置："
    } else {
        "Current Config:"
    }
}

pub fn config_toml_header() -> &'static str {
    if is_chinese() {
        "Config.toml 配置："
    } else {
        "Config.toml Configuration:"
    }
}

// Provider Input - Gemini Configuration
pub fn config_gemini_header() -> &'static str {
    if is_chinese() {
        "配置 Gemini 供应商："
    } else {
        "Configure Gemini Provider:"
    }
}

pub fn auth_type_label() -> &'static str {
    if is_chinese() {
        "认证类型："
    } else {
        "Auth Type:"
    }
}

pub fn auth_type_api_key() -> &'static str {
    if is_chinese() {
        "API Key"
    } else {
        "API Key"
    }
}

pub fn auth_type_service_account() -> &'static str {
    if is_chinese() {
        "Service Account (ADC)"
    } else {
        "Service Account (ADC)"
    }
}

pub fn gemini_api_key_label() -> &'static str {
    if is_chinese() {
        "Gemini API Key："
    } else {
        "Gemini API Key:"
    }
}

pub fn gemini_base_url_label() -> &'static str {
    if is_chinese() {
        "Base URL："
    } else {
        "Base URL:"
    }
}

pub fn gemini_base_url_help() -> &'static str {
    if is_chinese() {
        "留空使用官方 API"
    } else {
        "Leave empty to use official API"
    }
}

pub fn gemini_base_url_placeholder() -> &'static str {
    if is_chinese() {
        "如 https://generativelanguage.googleapis.com"
    } else {
        "e.g., https://generativelanguage.googleapis.com"
    }
}

pub fn adc_project_id_label() -> &'static str {
    if is_chinese() {
        "GCP Project ID："
    } else {
        "GCP Project ID:"
    }
}

pub fn adc_location_label() -> &'static str {
    if is_chinese() {
        "GCP Location："
    } else {
        "GCP Location:"
    }
}

pub fn adc_location_placeholder() -> &'static str {
    if is_chinese() {
        "如 us-central1"
    } else {
        "e.g., us-central1"
    }
}

pub fn google_oauth_official() -> &'static str {
    if is_chinese() {
        "Google OAuth（官方）"
    } else {
        "Google OAuth (Official)"
    }
}

pub fn packycode_api_key() -> &'static str {
    if is_chinese() {
        "PackyCode API Key"
    } else {
        "PackyCode API Key"
    }
}

pub fn generic_api_key() -> &'static str {
    if is_chinese() {
        "通用 API Key"
    } else {
        "Generic API Key"
    }
}

pub fn select_auth_method_help() -> &'static str {
    if is_chinese() {
        "选择 Gemini 的认证方式"
    } else {
        "Select authentication method for Gemini"
    }
}

pub fn use_google_oauth_warning() -> &'static str {
    if is_chinese() {
        "使用 Google OAuth，将清空 API Key 配置"
    } else {
        "Using Google OAuth, API Key config will be cleared"
    }
}

pub fn packycode_api_key_help() -> &'static str {
    if is_chinese() {
        "从 PackyCode 获取的 API Key"
    } else {
        "API Key obtained from PackyCode"
    }
}

pub fn packycode_endpoint_help() -> &'static str {
    if is_chinese() {
        "PackyCode API 端点"
    } else {
        "PackyCode API endpoint"
    }
}

pub fn generic_api_key_help() -> &'static str {
    if is_chinese() {
        "通用的 Gemini API Key"
    } else {
        "Generic Gemini API Key"
    }
}

// Provider Input - Optional Fields
pub fn notes_label() -> &'static str {
    if is_chinese() {
        "备注："
    } else {
        "Notes:"
    }
}

pub fn notes_placeholder() -> &'static str {
    if is_chinese() {
        "可选的备注信息"
    } else {
        "Optional notes"
    }
}

pub fn sort_index_label() -> &'static str {
    if is_chinese() {
        "排序索引："
    } else {
        "Sort Index:"
    }
}

pub fn sort_index_help() -> &'static str {
    if is_chinese() {
        "数字越小越靠前，留空使用创建时间排序"
    } else {
        "Lower numbers appear first, leave empty to sort by creation time"
    }
}

pub fn sort_index_placeholder() -> &'static str {
    if is_chinese() {
        "如 1, 2, 3..."
    } else {
        "e.g., 1, 2, 3..."
    }
}

pub fn invalid_sort_index() -> &'static str {
    if is_chinese() {
        "排序索引必须是有效的数字"
    } else {
        "Sort index must be a valid number"
    }
}

pub fn optional_fields_config() -> &'static str {
    if is_chinese() {
        "可选字段配置："
    } else {
        "Optional Fields Configuration:"
    }
}

pub fn notes_example_placeholder() -> &'static str {
    if is_chinese() {
        "自定义供应商，用于测试"
    } else {
        "Custom provider for testing"
    }
}

pub fn notes_help_edit() -> &'static str {
    if is_chinese() {
        "关于此供应商的额外说明，直接回车保持原值"
    } else {
        "Additional notes about this provider, press Enter to keep current value"
    }
}

pub fn notes_help_new() -> &'static str {
    if is_chinese() {
        "关于此供应商的额外说明，直接回车跳过"
    } else {
        "Additional notes about this provider, press Enter to skip"
    }
}

pub fn sort_index_help_edit() -> &'static str {
    if is_chinese() {
        "数字，用于控制显示顺序，直接回车保持原值"
    } else {
        "Number for display order, press Enter to keep current value"
    }
}

pub fn sort_index_help_new() -> &'static str {
    if is_chinese() {
        "数字，用于控制显示顺序，直接回车跳过"
    } else {
        "Number for display order, press Enter to skip"
    }
}

pub fn invalid_sort_index_number() -> &'static str {
    if is_chinese() {
        "排序索引必须是数字"
    } else {
        "Sort index must be a number"
    }
}

pub fn provider_config_summary() -> &'static str {
    if is_chinese() {
        "=== 供应商配置摘要 ==="
    } else {
        "=== Provider Configuration Summary ==="
    }
}
