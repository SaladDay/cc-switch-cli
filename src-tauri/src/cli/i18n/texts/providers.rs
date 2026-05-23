use super::is_chinese;

pub fn tui_claude_api_format_popup_title() -> &'static str {
    if is_chinese() {
        "API 格式"
    } else {
        "API Format"
    }
}

pub fn tui_label_claude_model_config() -> &'static str {
    if is_chinese() {
        "Claude 模型配置"
    } else {
        "Claude Model Config"
    }
}

pub fn tui_label_provider_package() -> &'static str {
    if is_chinese() {
        "Provider / npm 包"
    } else {
        "Provider / npm"
    }
}

pub fn tui_label_opencode_model_id() -> &'static str {
    if is_chinese() {
        "主模型 ID"
    } else {
        "Main Model ID"
    }
}

pub fn tui_label_opencode_model_name() -> &'static str {
    if is_chinese() {
        "主模型名称"
    } else {
        "Main Model Name"
    }
}

pub fn tui_label_context_limit() -> &'static str {
    if is_chinese() {
        "上下文限制"
    } else {
        "Context Limit"
    }
}

pub fn tui_label_output_limit() -> &'static str {
    if is_chinese() {
        "输出限制"
    } else {
        "Output Limit"
    }
}

pub fn tui_label_command() -> &'static str {
    if is_chinese() {
        "命令"
    } else {
        "Command"
    }
}

pub fn tui_label_mcp_type() -> &'static str {
    if is_chinese() {
        "连接类型"
    } else {
        "Transport"
    }
}

pub fn tui_label_url() -> &'static str {
    if is_chinese() {
        "URL"
    } else {
        "URL"
    }
}

pub fn tui_label_args() -> &'static str {
    if is_chinese() {
        "参数"
    } else {
        "Args"
    }
}

pub fn tui_label_env() -> &'static str {
    if is_chinese() {
        "环境变量"
    } else {
        "Env"
    }
}

pub fn tui_mcp_env_entry_count(count: usize) -> String {
    if is_chinese() {
        format!("{count} 项")
    } else if count == 1 {
        "1 entry".to_string()
    } else {
        format!("{count} entries")
    }
}

pub fn tui_mcp_env_editor_hint() -> &'static str {
    if is_chinese() {
        "按 Enter 管理环境变量"
    } else {
        "Press Enter to manage env entries"
    }
}

pub fn tui_mcp_type_editor_hint() -> &'static str {
    if is_chinese() {
        "按 Enter 选择连接类型"
    } else {
        "Press Enter to choose transport"
    }
}

pub fn tui_mcp_type_title() -> &'static str {
    if is_chinese() {
        "选择 MCP 连接类型"
    } else {
        "Select MCP Transport"
    }
}

pub fn tui_mcp_env_key_label() -> &'static str {
    if is_chinese() {
        "键"
    } else {
        "Key"
    }
}

pub fn tui_mcp_env_value_label() -> &'static str {
    if is_chinese() {
        "值"
    } else {
        "Value"
    }
}

pub fn tui_label_app_claude() -> &'static str {
    if is_chinese() {
        "应用: Claude"
    } else {
        "App: Claude"
    }
}

pub fn tui_label_app_codex() -> &'static str {
    if is_chinese() {
        "应用: Codex"
    } else {
        "App: Codex"
    }
}

pub fn tui_label_app_gemini() -> &'static str {
    if is_chinese() {
        "应用: Gemini"
    } else {
        "App: Gemini"
    }
}

pub fn tui_label_app_opencode() -> &'static str {
    if is_chinese() {
        "应用: OpenCode"
    } else {
        "App: OpenCode"
    }
}

pub fn tui_label_app_hermes() -> &'static str {
    if is_chinese() {
        "应用: Hermes"
    } else {
        "App: Hermes"
    }
}

pub fn tui_form_templates_title() -> &'static str {
    if is_chinese() {
        "模板"
    } else {
        "Templates"
    }
}

pub fn tui_form_common_config_button() -> &'static str {
    if is_chinese() {
        "通用配置"
    } else {
        "Common Config"
    }
}

pub fn tui_form_attach_common_config() -> &'static str {
    if is_chinese() {
        "添加通用配置"
    } else {
        "Attach Common Config"
    }
}

pub fn tui_form_fields_title() -> &'static str {
    if is_chinese() {
        "字段"
    } else {
        "Fields"
    }
}

pub fn tui_usage_query_title(provider: &str) -> String {
    if provider.trim().is_empty() {
        tui_usage_query_configure_title().to_string()
    } else {
        format!("{} - {provider}", tui_usage_query_configure_title())
    }
}

pub fn tui_usage_query_configure_title() -> &'static str {
    if is_chinese() {
        "配置用量查询"
    } else {
        "Configure Usage Query"
    }
}

pub fn tui_usage_query_notice_title() -> &'static str {
    tui_usage_query_configure_title()
}

pub fn tui_usage_query_notice_message() -> &'static str {
    if is_chinese() {
        "用量查询需要配置专用的查询脚本或 API 参数，请确保您已从供应商处获取相关信息。\n\n如不确定如何配置，请先查阅供应商文档。"
    } else {
        "Usage query requires a custom script or API parameters. Please make sure you have obtained the necessary information from your provider.\n\nIf unsure how to configure, please consult your provider's documentation first."
    }
}

pub fn tui_usage_query_enable() -> &'static str {
    if is_chinese() {
        "启用用量查询"
    } else {
        "Enable usage query"
    }
}

pub fn tui_usage_query_template() -> &'static str {
    if is_chinese() {
        "预设模板"
    } else {
        "Preset template"
    }
}

pub fn tui_usage_query_access_token() -> &'static str {
    if is_chinese() {
        "访问令牌（在个人安全设置里获取）"
    } else {
        "Access Token"
    }
}

pub fn tui_usage_query_user_id() -> &'static str {
    if is_chinese() {
        "用户 ID"
    } else {
        "User ID"
    }
}

pub fn tui_usage_query_timeout_seconds() -> &'static str {
    if is_chinese() {
        "超时时间（秒）"
    } else {
        "Timeout (seconds)"
    }
}

pub fn tui_usage_query_auto_interval() -> &'static str {
    if is_chinese() {
        "自动查询间隔（分钟，0 表示不自动查询）"
    } else {
        "Auto query interval (minutes, 0 to disable)"
    }
}

pub fn tui_usage_query_script() -> &'static str {
    if is_chinese() {
        "提取器代码"
    } else {
        "Extractor Code"
    }
}

pub fn tui_usage_query_script_preview_title() -> &'static str {
    if is_chinese() {
        "提取器代码 | 返回对象需包含剩余额度等字段"
    } else {
        "Extractor code | Return object should include remaining quota fields"
    }
}

pub fn tui_usage_query_script_help_title() -> &'static str {
    if is_chinese() {
        "脚本编写说明："
    } else {
        "Script writing instructions:"
    }
}

pub fn tui_usage_query_copilot_auto_auth() -> &'static str {
    if is_chinese() {
        "自动使用 OAuth 认证，无需手动配置凭证"
    } else {
        "Auto OAuth authentication, no manual credentials needed"
    }
}

pub fn tui_usage_query_token_plan_hint() -> &'static str {
    if is_chinese() {
        "自动使用供应商的 API Key 和 Base URL 查询 Token Plan 额度"
    } else {
        "Automatically uses the provider's API Key and Base URL to query Token Plan quota"
    }
}

pub fn tui_usage_query_balance_hint() -> &'static str {
    if is_chinese() {
        "自动使用供应商的 API Key 查询账户余额"
    } else {
        "Automatically uses the provider's API Key to query account balance"
    }
}

pub fn tui_usage_query_script_empty() -> &'static str {
    if is_chinese() {
        "脚本配置不能为空"
    } else {
        "Script configuration cannot be empty"
    }
}

pub fn tui_usage_query_must_have_return() -> &'static str {
    if is_chinese() {
        "脚本必须包含 return 语句"
    } else {
        "Script must contain return statement"
    }
}

pub fn tui_usage_query_coding_plan_provider() -> &'static str {
    if is_chinese() {
        "Coding Plan 供应商"
    } else {
        "Coding Plan Provider"
    }
}

pub fn tui_usage_query_info() -> &'static str {
    if is_chinese() {
        "说明"
    } else {
        "Info"
    }
}

pub fn tui_usage_query_custom_hint() -> &'static str {
    if is_chinese() {
        "支持变量: {{apiKey}}, {{baseUrl}} | extractor 函数接收 API 响应的 JSON 对象"
    } else {
        "Supported variables: {{apiKey}}, {{baseUrl}} | extractor function receives API response JSON object"
    }
}

pub fn tui_usage_query_credentials_config() -> &'static str {
    if is_chinese() {
        "凭证配置"
    } else {
        "Credentials"
    }
}

pub fn tui_usage_query_credentials_hint() -> &'static str {
    if is_chinese() {
        "留空则自动使用供应商配置"
    } else {
        "Leave empty to use provider config"
    }
}

pub fn tui_usage_query_optional() -> &'static str {
    if is_chinese() {
        "可选"
    } else {
        "optional"
    }
}

pub fn tui_usage_query_base_url() -> &'static str {
    if is_chinese() {
        "请求地址"
    } else {
        "Base URL"
    }
}

pub fn tui_usage_query_api_key_placeholder() -> &'static str {
    if is_chinese() {
        "留空则使用供应商的 API Key"
    } else {
        "Leave empty to use provider's API Key"
    }
}

pub fn tui_usage_query_base_url_placeholder() -> &'static str {
    if is_chinese() {
        "留空则使用供应商的请求地址"
    } else {
        "Leave empty to use provider's base URL"
    }
}

pub fn tui_usage_query_access_token_placeholder() -> &'static str {
    if is_chinese() {
        "在'安全设置'里生成"
    } else {
        "Generate in 'Security Settings'"
    }
}

pub fn tui_usage_query_user_id_placeholder() -> &'static str {
    if is_chinese() {
        "例如：114514"
    } else {
        "e.g., 114514"
    }
}

pub fn tui_usage_query_config_format() -> &'static str {
    if is_chinese() {
        "配置格式："
    } else {
        "Configuration format:"
    }
}

pub fn tui_usage_query_extractor_format() -> &'static str {
    if is_chinese() {
        "extractor 返回格式（所有字段均为可选）："
    } else {
        "Extractor return format (all fields optional):"
    }
}

pub fn tui_usage_query_tips() -> &'static str {
    if is_chinese() {
        "💡 提示："
    } else {
        "💡 Tips:"
    }
}

pub fn tui_usage_query_field_is_valid() -> &'static str {
    if is_chinese() {
        "• isValid: 布尔值，套餐是否有效"
    } else {
        "• isValid: Boolean, whether plan is valid"
    }
}

pub fn tui_usage_query_field_invalid_message() -> &'static str {
    if is_chinese() {
        "• invalidMessage: 字符串，失效原因说明（当 isValid 为 false 时显示）"
    } else {
        "• invalidMessage: String, reason for expiration (shown when isValid is false)"
    }
}

pub fn tui_usage_query_field_remaining() -> &'static str {
    if is_chinese() {
        "• remaining: 数字，剩余额度"
    } else {
        "• remaining: Number, remaining quota"
    }
}

pub fn tui_usage_query_field_unit() -> &'static str {
    if is_chinese() {
        "• unit: 字符串，单位（如 \"USD\"）"
    } else {
        "• unit: String, unit (e.g., \"USD\")"
    }
}

pub fn tui_usage_query_field_plan_name() -> &'static str {
    if is_chinese() {
        "• planName: 字符串，套餐名称"
    } else {
        "• planName: String, plan name"
    }
}

pub fn tui_usage_query_field_total() -> &'static str {
    if is_chinese() {
        "• total: 数字，总额度"
    } else {
        "• total: Number, total quota"
    }
}

pub fn tui_usage_query_field_used() -> &'static str {
    if is_chinese() {
        "• used: 数字，已用额度"
    } else {
        "• used: Number, used quota"
    }
}

pub fn tui_usage_query_field_extra() -> &'static str {
    if is_chinese() {
        "• extra: 字符串，扩展字段，可自由补充需要展示的文本"
    } else {
        "• extra: String, custom display text"
    }
}

pub fn tui_usage_query_tip1() -> &'static str {
    if is_chinese() {
        "• 变量 {{apiKey}} 和 {{baseUrl}} 会自动替换"
    } else {
        "• Variables {{apiKey}} and {{baseUrl}} are automatically replaced"
    }
}

pub fn tui_usage_query_tip2() -> &'static str {
    if is_chinese() {
        "• extractor 函数在沙箱环境中执行，支持 ES2020+ 语法"
    } else {
        "• Extractor function runs in sandbox environment, supports ES2020+ syntax"
    }
}

pub fn tui_usage_query_tip3() -> &'static str {
    if is_chinese() {
        "• 整个配置必须用 () 包裹，形成对象字面量表达式"
    } else {
        "• Entire config must be wrapped in () to form object literal expression"
    }
}

pub fn tui_form_json_title() -> &'static str {
    "JSON"
}

pub fn tui_codex_auth_json_title() -> &'static str {
    if is_chinese() {
        "auth.json (JSON) *"
    } else {
        "auth.json (JSON) *"
    }
}

pub fn tui_codex_config_toml_title() -> &'static str {
    if is_chinese() {
        "config.toml (TOML)"
    } else {
        "config.toml (TOML)"
    }
}

pub fn tui_form_input_title() -> &'static str {
    if is_chinese() {
        "输入"
    } else {
        "Input"
    }
}

pub fn tui_form_editing_title() -> &'static str {
    if is_chinese() {
        "编辑中"
    } else {
        "Editing"
    }
}

pub fn tui_claude_model_config_popup_title() -> &'static str {
    if is_chinese() {
        "Claude 模型配置"
    } else {
        "Claude Model Configuration"
    }
}

pub fn tui_claude_model_main_label() -> &'static str {
    if is_chinese() {
        "主模型"
    } else {
        "Main Model"
    }
}

pub fn tui_claude_reasoning_model_label() -> &'static str {
    if is_chinese() {
        "推理模型 (Thinking)"
    } else {
        "Reasoning Model (Thinking)"
    }
}

pub fn tui_claude_default_haiku_model_label() -> &'static str {
    if is_chinese() {
        "默认 Haiku 模型"
    } else {
        "Default Haiku Model"
    }
}

pub fn tui_claude_default_sonnet_model_label() -> &'static str {
    if is_chinese() {
        "默认 Sonnet 模型"
    } else {
        "Default Sonnet Model"
    }
}

pub fn tui_claude_default_opus_model_label() -> &'static str {
    if is_chinese() {
        "默认 Opus 模型"
    } else {
        "Default Opus Model"
    }
}

pub fn tui_claude_model_config_summary(configured_count: usize) -> String {
    if is_chinese() {
        format!("已配置 {configured_count}/5")
    } else {
        format!("Configured {configured_count}/5")
    }
}

pub fn tui_claude_model_config_open_hint() -> &'static str {
    if is_chinese() {
        "按 Enter 配置 Claude 模型"
    } else {
        "Press Enter to configure Claude models"
    }
}

pub fn tui_hint_press() -> &'static str {
    if is_chinese() {
        "按 "
    } else {
        "Press "
    }
}

pub fn tui_hint_auto_fetch_models_from_api() -> &'static str {
    if is_chinese() {
        " 从 API 自动获取模型。"
    } else {
        " to auto-fetch models from API."
    }
}

pub fn tui_model_fetch_popup_title(fetching: bool) -> String {
    if is_chinese() {
        if fetching {
            "选择模型 (获取中...)".to_string()
        } else {
            "选择模型".to_string()
        }
    } else {
        if fetching {
            "Select Model (Fetching...)".to_string()
        } else {
            "Select Model".to_string()
        }
    }
}

pub fn tui_model_fetch_search_placeholder() -> &'static str {
    if is_chinese() {
        "输入过滤 或 直接回车使用输入值..."
    } else {
        "Type to filter, or press Enter to use input..."
    }
}

pub fn tui_model_fetch_search_title() -> &'static str {
    if is_chinese() {
        "模型搜索"
    } else {
        "Model Search"
    }
}

pub fn tui_model_fetch_no_models() -> &'static str {
    if is_chinese() {
        "没有获取到模型 (可直接输入并在此回车)"
    } else {
        "No models found (type custom and press Enter)"
    }
}

pub fn tui_model_fetch_no_matches() -> &'static str {
    if is_chinese() {
        "没有匹配结果 (可直接输入并在此回车)"
    } else {
        "No matching models (press Enter to use input)"
    }
}

pub fn tui_model_fetch_error_hint(err: &str) -> String {
    if is_chinese() {
        format!("获取失败: {}", err)
    } else {
        format!("Fetch failed: {}", err)
    }
}

pub fn tui_provider_not_found() -> &'static str {
    if is_chinese() {
        "未找到该供应商。"
    } else {
        "Provider not found."
    }
}

pub fn tui_provider_title() -> &'static str {
    if is_chinese() {
        "供应商"
    } else {
        "Provider"
    }
}

pub fn tui_provider_detail_title() -> &'static str {
    if is_chinese() {
        "供应商详情"
    } else {
        "Provider Detail"
    }
}

pub fn tui_provider_add_title() -> &'static str {
    if is_chinese() {
        "新增供应商"
    } else {
        "Add Provider"
    }
}

pub fn tui_provider_empty_title() -> &'static str {
    if is_chinese() {
        "还没有添加任何供应商"
    } else {
        "No providers have been added yet"
    }
}

pub fn tui_provider_empty_subtitle() -> &'static str {
    if is_chinese() {
        "如果你已有配置，请点击\"导入当前配置\"，所有数据将安全保存在 default 供应商中"
    } else {
        "If you already have a config, use \"Import Current Config\". Everything will be safely stored in the default provider."
    }
}

pub fn tui_key_import_current_config() -> &'static str {
    if is_chinese() {
        "导入当前配置"
    } else {
        "import current config"
    }
}

pub fn tui_key_add_provider() -> &'static str {
    if is_chinese() {
        "添加供应商"
    } else {
        "add provider"
    }
}

pub fn tui_codex_official_no_api_key_tip() -> &'static str {
    if is_chinese() {
        "官方无需填写 API Key，直接保存即可。"
    } else {
        "Official provider doesn't require an API key. Just save."
    }
}

pub fn tui_toast_codex_official_auth_json_disabled() -> &'static str {
    if is_chinese() {
        "官方模式下不支持编辑 auth.json（切换时会移除）。"
    } else {
        "auth.json editing is disabled for the official provider (it will be removed on switch)."
    }
}

pub fn tui_provider_edit_title(name: &str) -> String {
    if is_chinese() {
        format!("编辑供应商: {name}")
    } else {
        format!("Edit Provider: {name}")
    }
}

pub fn tui_provider_detail_keys() -> &'static str {
    if is_chinese() {
        "按键：Space=切换  e=编辑  t=测试"
    } else {
        "Keys: Space=switch  e=edit  t=test"
    }
}

pub fn tui_key_switch() -> &'static str {
    if is_chinese() {
        "切换"
    } else {
        "switch"
    }
}

pub fn tui_key_edit() -> &'static str {
    if is_chinese() {
        "编辑"
    } else {
        "edit"
    }
}

pub fn tui_key_speedtest() -> &'static str {
    if is_chinese() {
        "测速"
    } else {
        "speedtest"
    }
}

pub fn tui_key_test() -> &'static str {
    if is_chinese() {
        "测试"
    } else {
        "test"
    }
}

pub fn tui_key_stream_check() -> &'static str {
    if is_chinese() {
        "健康检查"
    } else {
        "stream check"
    }
}

pub fn tui_key_launch_temp() -> &'static str {
    if is_chinese() {
        "临时启动"
    } else {
        "launch temp"
    }
}

pub fn tui_temp_launch_failed(message: &str) -> String {
    if is_chinese() {
        format!("临时启动失败: {message}")
    } else {
        format!("Temporary launch failed: {message}")
    }
}

pub fn tui_stream_check_status_operational() -> &'static str {
    if is_chinese() {
        "正常"
    } else {
        "operational"
    }
}

pub fn tui_stream_check_status_degraded() -> &'static str {
    if is_chinese() {
        "降级"
    } else {
        "degraded"
    }
}

pub fn tui_stream_check_status_failed() -> &'static str {
    if is_chinese() {
        "失败"
    } else {
        "failed"
    }
}

pub fn tui_key_details() -> &'static str {
    if is_chinese() {
        "详情"
    } else {
        "details"
    }
}

pub fn tui_key_view() -> &'static str {
    if is_chinese() {
        "查看"
    } else {
        "view"
    }
}

pub fn tui_key_add() -> &'static str {
    if is_chinese() {
        "新增"
    } else {
        "add"
    }
}

pub fn tui_key_delete() -> &'static str {
    if is_chinese() {
        "删除"
    } else {
        "delete"
    }
}

pub fn tui_key_import() -> &'static str {
    if is_chinese() {
        "导入"
    } else {
        "import"
    }
}

pub fn tui_key_failover() -> &'static str {
    if is_chinese() {
        "管理故障转移"
    } else {
        "manage failover"
    }
}

pub fn tui_key_install() -> &'static str {
    if is_chinese() {
        "安装"
    } else {
        "install"
    }
}

pub fn tui_key_uninstall() -> &'static str {
    if is_chinese() {
        "卸载"
    } else {
        "uninstall"
    }
}

pub fn tui_key_discover() -> &'static str {
    if is_chinese() {
        "发现"
    } else {
        "discover"
    }
}

pub fn tui_key_unmanaged() -> &'static str {
    if is_chinese() {
        "已有"
    } else {
        "existing"
    }
}

pub fn tui_key_repos() -> &'static str {
    if is_chinese() {
        "仓库"
    } else {
        "repos"
    }
}

pub fn tui_key_sync() -> &'static str {
    if is_chinese() {
        "同步"
    } else {
        "sync"
    }
}

pub fn tui_key_sync_method() -> &'static str {
    if is_chinese() {
        "同步方式"
    } else {
        "sync method"
    }
}

pub fn tui_key_search() -> &'static str {
    if is_chinese() {
        "搜索"
    } else {
        "search"
    }
}

pub fn tui_key_refresh() -> &'static str {
    if is_chinese() {
        "刷新"
    } else {
        "refresh"
    }
}

pub fn tui_key_rename() -> &'static str {
    if is_chinese() {
        "重命名"
    } else {
        "rename"
    }
}

pub fn tui_key_start_proxy() -> &'static str {
    if is_chinese() {
        "启动代理"
    } else {
        "start proxy"
    }
}

pub fn tui_key_stop_proxy() -> &'static str {
    if is_chinese() {
        "停止代理"
    } else {
        "stop proxy"
    }
}

pub fn tui_key_proxy_on() -> &'static str {
    if is_chinese() {
        "代理开"
    } else {
        "proxy on"
    }
}

pub fn tui_key_proxy_off() -> &'static str {
    if is_chinese() {
        "代理关"
    } else {
        "proxy off"
    }
}

pub fn tui_key_focus() -> &'static str {
    if is_chinese() {
        "切换窗口"
    } else {
        "next pane"
    }
}

pub fn tui_key_toggle() -> &'static str {
    if is_chinese() {
        "启用/禁用"
    } else {
        "toggle"
    }
}

pub fn tui_key_apps() -> &'static str {
    if is_chinese() {
        "应用"
    } else {
        "apps"
    }
}

pub fn tui_key_activate() -> &'static str {
    if is_chinese() {
        "激活"
    } else {
        "activate"
    }
}

pub fn tui_key_deactivate() -> &'static str {
    if is_chinese() {
        "取消激活"
    } else {
        "deactivate"
    }
}

pub fn tui_key_open() -> &'static str {
    if is_chinese() {
        "打开"
    } else {
        "open"
    }
}

pub fn tui_key_apply() -> &'static str {
    if is_chinese() {
        "应用"
    } else {
        "apply"
    }
}

pub fn tui_key_extract() -> &'static str {
    if is_chinese() {
        "提取"
    } else {
        "extract"
    }
}

pub fn tui_key_format() -> &'static str {
    if is_chinese() {
        "格式化"
    } else {
        "format"
    }
}

pub fn tui_key_edit_snippet() -> &'static str {
    if is_chinese() {
        "编辑片段"
    } else {
        "edit snippet"
    }
}

pub fn tui_key_close() -> &'static str {
    if is_chinese() {
        "关闭"
    } else {
        "close"
    }
}

pub fn tui_key_exit() -> &'static str {
    if is_chinese() {
        "退出"
    } else {
        "exit"
    }
}

pub fn tui_key_cancel() -> &'static str {
    if is_chinese() {
        "取消"
    } else {
        "cancel"
    }
}

pub fn tui_key_submit() -> &'static str {
    if is_chinese() {
        "提交"
    } else {
        "submit"
    }
}

pub fn tui_key_yes() -> &'static str {
    if is_chinese() {
        "确认"
    } else {
        "confirm"
    }
}

pub fn tui_key_no() -> &'static str {
    if is_chinese() {
        "返回"
    } else {
        "back"
    }
}

pub fn tui_key_scroll() -> &'static str {
    if is_chinese() {
        "滚动"
    } else {
        "scroll"
    }
}

pub fn tui_key_restore() -> &'static str {
    if is_chinese() {
        "恢复"
    } else {
        "restore"
    }
}

pub fn tui_key_takeover() -> &'static str {
    if is_chinese() {
        "接管"
    } else {
        "take over"
    }
}

pub fn tui_key_save() -> &'static str {
    if is_chinese() {
        "保存"
    } else {
        "save"
    }
}

pub fn tui_key_external_editor() -> &'static str {
    if is_chinese() {
        "外部编辑器"
    } else {
        "external editor"
    }
}

pub fn tui_key_save_and_exit() -> &'static str {
    if is_chinese() {
        "保存并退出"
    } else {
        "save & exit"
    }
}

pub fn tui_key_exit_without_save() -> &'static str {
    if is_chinese() {
        "不保存退出"
    } else {
        "exit w/o save"
    }
}

pub fn tui_key_edit_mode() -> &'static str {
    if is_chinese() {
        "编辑"
    } else {
        "edit"
    }
}

pub fn tui_key_clear() -> &'static str {
    if is_chinese() {
        "清除"
    } else {
        "clear"
    }
}

pub fn tui_key_move() -> &'static str {
    if is_chinese() {
        "移动"
    } else {
        "move"
    }
}

pub fn tui_key_exit_edit() -> &'static str {
    if is_chinese() {
        "退出编辑"
    } else {
        "exit edit"
    }
}

pub fn tui_key_select() -> &'static str {
    if is_chinese() {
        "选择"
    } else {
        "select"
    }
}

pub fn tui_key_fetch_model() -> &'static str {
    if is_chinese() {
        "获取模型"
    } else {
        "fetch model"
    }
}

pub fn tui_key_deactivate_active() -> &'static str {
    if is_chinese() {
        "取消激活(当前)"
    } else {
        "deactivate active"
    }
}

pub fn tui_provider_list_keys() -> &'static str {
    if is_chinese() {
        "按键：a=新增  e=编辑  Enter=详情  Space=切换  /=搜索"
    } else {
        "Keys: a=add  e=edit  Enter=details  Space=switch  /=filter"
    }
}

pub fn tui_home_ascii_logo() -> &'static str {
    // Same ASCII art across languages.
    r#"                                  _  _         _
   ___  ___        ___ __      __(_)| |_  ___ | |__
  / __|/ __|_____ / __|\ \ /\ / /| || __|/ __|| '_ \
 | (__| (__|_____|\__ \ \ V  V / | || |_| (__ | | | |
  \___|\___|      |___/  \_/\_/  |_| \__|\___||_| |_|
                                                  "#
}

pub fn tui_common_snippet_keys() -> &'static str {
    if is_chinese() {
        "按键：e=编辑  c=清除  a=应用  Esc=返回"
    } else {
        "Keys: e=edit  c=clear  a=apply  Esc=back"
    }
}

pub fn tui_view_config_app(app: &str) -> String {
    if is_chinese() {
        format!("应用: {}", app)
    } else {
        format!("App: {}", app)
    }
}

pub fn tui_view_config_provider(provider: &str) -> String {
    if is_chinese() {
        format!("供应商: {}", provider)
    } else {
        format!("Provider: {}", provider)
    }
}

pub fn tui_view_config_api_url(url: &str) -> String {
    if is_chinese() {
        format!("API URL:  {}", url)
    } else {
        format!("API URL:  {}", url)
    }
}

pub fn tui_view_config_mcp_servers(enabled: usize, total: usize) -> String {
    if is_chinese() {
        format!("MCP 服务器: {} 启用 / {} 总数", enabled, total)
    } else {
        format!("MCP servers: {} enabled / {} total", enabled, total)
    }
}

pub fn tui_view_config_prompts(active: &str) -> String {
    if is_chinese() {
        format!("提示词: {}", active)
    } else {
        format!("Prompts: {}", active)
    }
}

pub fn tui_view_config_config_file(path: &str) -> String {
    if is_chinese() {
        format!("配置文件: {}", path)
    } else {
        format!("Config file: {}", path)
    }
}

pub fn tui_settings_header_language() -> &'static str {
    if is_chinese() {
        "语言"
    } else {
        "Language"
    }
}

pub fn tui_settings_header_setting() -> &'static str {
    if is_chinese() {
        "设置项"
    } else {
        "Setting"
    }
}

pub fn tui_settings_header_value() -> &'static str {
    if is_chinese() {
        "值"
    } else {
        "Value"
    }
}

pub fn tui_settings_title() -> &'static str {
    if is_chinese() {
        "设置"
    } else {
        "Settings"
    }
}

pub fn tui_settings_proxy_title() -> &'static str {
    if is_chinese() {
        "本地代理"
    } else {
        "Local Proxy"
    }
}

pub fn tui_settings_proxy_listen_address_label() -> &'static str {
    if is_chinese() {
        "监听地址"
    } else {
        "Listen Address"
    }
}

pub fn tui_settings_proxy_listen_port_label() -> &'static str {
    if is_chinese() {
        "监听端口"
    } else {
        "Listen Port"
    }
}

pub fn tui_settings_proxy_listen_address_prompt() -> &'static str {
    if is_chinese() {
        "输入监听地址（如 127.0.0.1 / localhost / 0.0.0.0）"
    } else {
        "Enter listen address (for example 127.0.0.1 / localhost / 0.0.0.0)"
    }
}

pub fn tui_settings_proxy_listen_port_prompt() -> &'static str {
    if is_chinese() {
        "输入监听端口（1024-65535）"
    } else {
        "Enter listen port (1024-65535)"
    }
}

pub fn tui_settings_openclaw_config_dir_label() -> &'static str {
    if is_chinese() {
        "OpenClaw 配置目录"
    } else {
        "OpenClaw Config Directory"
    }
}

pub fn tui_settings_openclaw_config_dir_prompt() -> &'static str {
    if is_chinese() {
        "输入 OpenClaw 配置目录；留空恢复默认 ~/.openclaw"
    } else {
        "Enter the OpenClaw config directory; leave empty to use ~/.openclaw"
    }
}

pub fn tui_settings_openclaw_config_dir_default_value() -> &'static str {
    "Default (~/.openclaw)"
}

// --- GitHub Copilot ---

pub fn copilot_template_label() -> &'static str {
    "GitHub Copilot"
}

pub fn copilot_template_hint_run_login() -> &'static str {
    if is_chinese() {
        "保存后请运行 `cc-switch copilot login` 完成 GitHub 设备码授权"
    } else {
        "After saving, run `cc-switch copilot login` to complete GitHub device-code authorization"
    }
}

pub fn copilot_login_starting() -> &'static str {
    if is_chinese() {
        "正在向 GitHub 申请设备码..."
    } else {
        "Requesting device code from GitHub..."
    }
}

pub fn copilot_login_visit_url(url: &str, code: &str) -> String {
    if is_chinese() {
        format!("请在浏览器打开 {url} 并输入授权码：{code}")
    } else {
        format!("Open {url} in your browser and enter the user code: {code}")
    }
}

pub fn copilot_login_waiting() -> &'static str {
    if is_chinese() {
        "等待 GitHub 授权完成..."
    } else {
        "Waiting for GitHub authorization to complete..."
    }
}

pub fn copilot_login_success(login: &str) -> String {
    if is_chinese() {
        format!("已成功登录 GitHub 账号：{login}")
    } else {
        format!("Successfully signed in to GitHub account: {login}")
    }
}

pub fn copilot_login_failed(error: &str) -> String {
    if is_chinese() {
        format!("登录失败：{error}")
    } else {
        format!("Login failed: {error}")
    }
}

pub fn copilot_login_create_provider_prompt() -> &'static str {
    if is_chinese() {
        "是否在 Claude 下创建 GitHub Copilot 供应商？"
    } else {
        "Create a GitHub Copilot provider under Claude?"
    }
}

pub fn copilot_login_switch_to_provider_prompt() -> &'static str {
    if is_chinese() {
        "是否立即切换到该供应商？"
    } else {
        "Switch to this provider now?"
    }
}

pub fn copilot_logout_success() -> &'static str {
    if is_chinese() {
        "已注销所有 GitHub Copilot 账号"
    } else {
        "Signed out of all GitHub Copilot accounts"
    }
}

pub fn copilot_logout_account_success(account_id: &str) -> String {
    if is_chinese() {
        format!("已注销账号：{account_id}")
    } else {
        format!("Signed out of account: {account_id}")
    }
}

pub fn copilot_no_accounts() -> &'static str {
    if is_chinese() {
        "暂无已认证的 GitHub Copilot 账号。运行 `cc-switch copilot login` 进行登录。"
    } else {
        "No authenticated GitHub Copilot accounts. Run `cc-switch copilot login` to sign in."
    }
}

pub fn copilot_default_set(account_id: &str) -> String {
    if is_chinese() {
        format!("默认账号已设置为：{account_id}")
    } else {
        format!("Default account set to: {account_id}")
    }
}

pub fn copilot_account_list_header() -> &'static str {
    if is_chinese() {
        "已认证的 GitHub 账号："
    } else {
        "Authenticated GitHub accounts:"
    }
}

pub fn copilot_login_device_code_expired() -> &'static str {
    if is_chinese() {
        "设备码已过期，请重新运行登录命令。"
    } else {
        "Device code expired. Please re-run the login command."
    }
}
