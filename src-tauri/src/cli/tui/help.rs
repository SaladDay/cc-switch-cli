use crate::app_config::AppType;
use crate::cli::i18n;
use crate::cli::i18n::texts;

use super::app::{App, Focus, Overlay, SettingsItem};
use super::data::UiData;
use super::form::{
    CodexLocalRoutingField, CodexModelCatalogField, CodexPreviewSection, FormFocus, FormMode,
    FormState, LocalProxySettingsField, ProviderAddField, ProviderFormPage, S3SyncField,
    UsageQueryField, UsageQueryTemplate, WebDavSyncField,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpContent {
    pub title: String,
    pub eyebrow: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpState {
    pub content: HelpContent,
    pub scroll: usize,
}

impl HelpState {
    pub fn new(content: HelpContent) -> Self {
        Self { content, scroll: 0 }
    }
}

impl HelpContent {
    fn new(title: impl Into<String>, lines: Vec<String>) -> Self {
        Self {
            title: title.into(),
            eyebrow: tr("点在哪，帮助就在哪", "Help follows the focused item").to_string(),
            lines,
        }
    }

    fn empty() -> Self {
        Self::new(
            texts::tui_help_title(),
            vec![tr("此处无提示", "No help here").to_string()],
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HelpTarget {
    Global,
    Sessions,
    FailoverQueue,
    PreferredEditor,
    ProviderTemplate,
    ProviderField {
        app_type: AppType,
        field: ProviderAddField,
    },
    ProviderPreview {
        app_type: AppType,
        section: Option<CodexPreviewSection>,
    },
    UsageQueryField {
        template: UsageQueryTemplate,
        field: UsageQueryField,
    },
    CodexLocalRoutingField {
        field: CodexLocalRoutingField,
    },
    LocalProxySettingsField {
        field: LocalProxySettingsField,
    },
    CodexModelCatalogField {
        field: CodexModelCatalogField,
    },
    UsageQueryExtractor {
        template: UsageQueryTemplate,
    },
    UsageQueryInstructions,
    S3Field {
        field: S3SyncField,
    },
    WebDavField {
        field: WebDavSyncField,
    },
    Empty,
}

pub fn context_help_for_app(app: &App, data: &UiData) -> HelpContent {
    help_for_target(current_help_target(app), app, data)
}

fn current_help_target(app: &App) -> HelpTarget {
    if app.overlay.is_active() {
        return match &app.overlay {
            Overlay::Help(_) => HelpTarget::Global,
            Overlay::UsageQueryTemplatePicker { .. } => {
                provider_usage_query_overlay_target(app, UsageQueryField::Template)
            }
            Overlay::ClaudeApiFormatPicker { .. } => {
                provider_field_overlay_target(app, ProviderAddField::ClaudeApiFormat)
            }
            Overlay::UserAgentPicker { .. } => {
                provider_local_proxy_overlay_target(app, LocalProxySettingsField::UserAgent)
            }
            Overlay::ExternalEditorPicker { .. } => HelpTarget::PreferredEditor,
            Overlay::ClaudeModelPicker { .. } => {
                provider_field_overlay_target(app, ProviderAddField::ClaudeModelConfig)
            }
            Overlay::HermesModelsPicker { .. } => {
                provider_field_overlay_target(app, ProviderAddField::HermesModels)
            }
            Overlay::ManagedAccountPicker { .. } | Overlay::ManagedAccountActionPicker { .. } => {
                provider_field_overlay_target(app, ProviderAddField::CodexOAuthAccount)
            }
            Overlay::FailoverQueueManager { .. } => HelpTarget::FailoverQueue,
            Overlay::SessionProjectPicker(_) => HelpTarget::Sessions,
            Overlay::S3PresetPicker { .. } => HelpTarget::S3Field {
                field: S3SyncField::Preset,
            },
            _ => HelpTarget::Global,
        };
    }

    if app.editor.is_some() || app.filter.active {
        return HelpTarget::Empty;
    }

    if matches!(app.route, super::route::Route::Sessions) {
        return HelpTarget::Sessions;
    }

    if matches!(app.route, super::route::Route::Settings)
        && matches!(app.focus, Focus::Content)
        && matches!(
            SettingsItem::ALL.get(app.settings_idx),
            Some(SettingsItem::PreferredEditor)
        )
    {
        return HelpTarget::PreferredEditor;
    }

    if let Some(FormState::S3Sync(form)) = app.form.as_ref() {
        return HelpTarget::S3Field {
            field: form.selected_field(),
        };
    }

    if let Some(FormState::WebDavSync(form)) = app.form.as_ref() {
        return HelpTarget::WebDavField {
            field: form.selected_field(),
        };
    }

    let Some(FormState::ProviderAdd(provider)) = app.form.as_ref() else {
        return HelpTarget::Global;
    };

    if matches!(provider.page, ProviderFormPage::UsageQuery) {
        return match provider.focus {
            FormFocus::Fields => {
                let field = provider.selected_usage_query_field();
                field.map_or(HelpTarget::Empty, |field| HelpTarget::UsageQueryField {
                    template: provider.usage_query_template,
                    field,
                })
            }
            FormFocus::JsonPreview if provider.usage_query_extractor_available() => {
                HelpTarget::UsageQueryExtractor {
                    template: provider.usage_query_template,
                }
            }
            FormFocus::Content if provider.usage_query_extractor_available() => {
                HelpTarget::UsageQueryInstructions
            }
            _ => HelpTarget::Empty,
        };
    }

    if matches!(provider.page, ProviderFormPage::CodexLocalRouting) {
        return match provider.focus {
            FormFocus::Fields => provider
                .selected_codex_local_routing_field()
                .map_or(HelpTarget::Empty, |field| {
                    HelpTarget::CodexLocalRoutingField { field }
                }),
            _ => HelpTarget::Empty,
        };
    }

    if matches!(provider.page, ProviderFormPage::LocalProxySettings) {
        return match provider.focus {
            FormFocus::Fields => provider
                .selected_local_proxy_settings_field()
                .map_or(HelpTarget::Empty, |field| {
                    HelpTarget::LocalProxySettingsField { field }
                }),
            _ => HelpTarget::Empty,
        };
    }

    if matches!(provider.page, ProviderFormPage::CodexModelCatalog) {
        return match provider.focus {
            FormFocus::Fields => HelpTarget::CodexModelCatalogField {
                field: provider.codex_model_catalog_field,
            },
            _ => HelpTarget::Empty,
        };
    }

    if matches!(provider.page, ProviderFormPage::ClaudeQuickConfig) {
        return match provider.focus {
            FormFocus::Fields => {
                provider
                    .selected_claude_quick_config_field()
                    .map_or(HelpTarget::Empty, |field| HelpTarget::ProviderField {
                        app_type: provider.app_type.clone(),
                        field,
                    })
            }
            _ => HelpTarget::Empty,
        };
    }

    if matches!(provider.page, ProviderFormPage::CodexQuickConfig) {
        return match provider.focus {
            FormFocus::Fields => {
                provider
                    .selected_codex_quick_config_field()
                    .map_or(HelpTarget::Empty, |field| HelpTarget::ProviderField {
                        app_type: provider.app_type.clone(),
                        field,
                    })
            }
            _ => HelpTarget::Empty,
        };
    }

    match provider.focus {
        FormFocus::Templates if matches!(provider.mode, FormMode::Add) => {
            HelpTarget::ProviderTemplate
        }
        FormFocus::Fields => {
            let fields = provider.fields();
            fields
                .get(provider.field_idx.min(fields.len().saturating_sub(1)))
                .copied()
                .filter(|field| !provider_field_is_divider(*field))
                .map_or(HelpTarget::Empty, |field| HelpTarget::ProviderField {
                    app_type: provider.app_type.clone(),
                    field,
                })
        }
        FormFocus::JsonPreview => HelpTarget::ProviderPreview {
            app_type: provider.app_type.clone(),
            section: matches!(provider.app_type, AppType::Codex)
                .then_some(provider.codex_preview_section),
        },
        FormFocus::Content => HelpTarget::Empty,
        FormFocus::Templates => HelpTarget::Empty,
    }
}

fn help_for_target(target: HelpTarget, app: &App, data: &UiData) -> HelpContent {
    match target {
        HelpTarget::Global => {
            HelpContent::new(texts::tui_help_title(), global_help_lines(app, data))
        }
        HelpTarget::Sessions => HelpContent::new(
            texts::tui_sessions_title(),
            help_lines(
                "会话始终只显示当前应用，结果由项目范围 × / 搜索共同决定。\n←/→ 切换列表和详情，h/l 是备用键；↑/↓ 逐项移动，PgUp/PgDn 按页移动。p 打开项目选择器；Home/End 跳到首尾，Shift+←/→ 查看完整目录，Shift+Home/End 直达目录两端。\n“未知目录”位于项目列表末尾，只包含缺少项目目录的旧会话；精确项目按词法规范化后的完整目录匹配。",
                "Sessions always show the current app; results combine Project scope × / Search.\nUse ←/→ to switch between the list and details; h/l are aliases. Use ↑/↓ to move one item and PgUp/PgDn to move by a page. Press p to choose a project; Home/End jumps to either list end, Shift+←/→ reveals the complete directory, and Shift+Home/End jumps to either path end.\nUnknown directory is last and contains only legacy sessions without a project directory; exact projects match the complete lexically normalized directory.",
            ),
        ),
        HelpTarget::FailoverQueue => HelpContent::new(
            crate::t!("Failover Queue", "故障转移队列"),
            help_lines(
                "Enter 将当前供应商加入或移出队列。Ctrl+↑/↓ 调整已加入供应商的优先级；J/K 是移动的备用键。按 f 开关自动故障转移。\nP1、P2… 就是实际尝试顺序。开启自动故障转移后，供应商主页的普通切换会停用，由此队列控制路由。\n“当前目标”来自正在运行的代理。健康状态来自历史请求结果：“无记录”不代表健康检查失败，“失败”是尚未达到不健康阈值的连续失败；这里不显示实时断路器状态。",
                "Press Enter to add or remove the focused provider. Ctrl+Up/Down changes the priority of queued providers; J/K are secondary move aliases. Press f to toggle automatic failover.\nP1, P2, and so on are the actual attempt order. While automatic failover is on, ordinary switching on the Providers page is disabled and this queue controls routing.\nActive target comes from the running proxy. Health is passive history from past request attempts: no record is not a failed health check, and failures means consecutive failures below the unhealthy threshold. This view does not claim to show the live circuit-breaker state.",
            ),
        ),
        HelpTarget::PreferredEditor => HelpContent::new(
            texts::tui_settings_preferred_editor_label(),
            help_lines(
                "打开设置项时，cc-switch 才会检测当前系统中可执行的常见编辑器；检测不会启动任何程序，也不影响启动速度。检测结果只作为候选，必须按 Enter 明确选择后才会保存，不会自动替你选择。有效的 VISUAL 和 EDITOR 命令也会出现在候选中。\n自定义命令会按参数直接执行，不经过 shell，临时文件路径会追加为最后一个参数。带空格的路径或参数需要加引号；自定义输入留空可清除当前选择。\n图形编辑器必须使用等待参数（例如 code --wait），否则进程提前退出后临时文件会被回收。已配置的命令如果启动失败会直接报错，不会静默换用其他编辑器。",
                "cc-switch detects executable common editors only when you open this setting; detection launches nothing and does not affect startup time. Results are choices only: nothing is saved until you explicitly press Enter, and no editor is selected automatically. Valid VISUAL and EDITOR commands also appear in the list.\nCustom commands are executed directly without a shell, with the temporary file path appended as the final argument. Quote paths or arguments that contain spaces; leave custom input empty to clear the selection.\nGUI editors need a wait flag, such as code --wait, or the temporary file could be removed when the launcher exits early. A configured command reports launch failures instead of silently switching editors.",
            ),
        ),
        HelpTarget::ProviderTemplate => HelpContent::new(
            tr("供应商模板", "Provider templates"),
            help_lines(
                "选择一个预设模板后按 Enter 应用。模板会填入供应商常用的地址、模型和元数据。\n如果没有合适模板，保留自定义后直接填写字段。",
                "Choose a preset and press Enter to apply it. Templates fill common provider URLs, models, and metadata.\nKeep Custom selected when no preset fits.",
            ),
        ),
        HelpTarget::ProviderField { app_type, field } => provider_field_help(app_type, field),
        HelpTarget::ProviderPreview { app_type, section } => provider_preview_help(app_type, section),
        HelpTarget::UsageQueryField { template, field } => usage_query_field_help(template, field),
        HelpTarget::CodexLocalRoutingField { field } => codex_local_routing_field_help(field),
        HelpTarget::LocalProxySettingsField { field } => local_proxy_settings_field_help(field),
        HelpTarget::CodexModelCatalogField { field } => codex_model_catalog_field_help(field),
        HelpTarget::UsageQueryExtractor { template } => usage_query_extractor_help(template),
        HelpTarget::UsageQueryInstructions => HelpContent::new(
            texts::tui_usage_query_script_help_title(),
            super::form::ProviderAddFormState::usage_query_script_help_lines(),
        ),
        HelpTarget::S3Field { field } => s3_field_help(field),
        HelpTarget::WebDavField { field } => webdav_field_help(field),
        HelpTarget::Empty => HelpContent::empty(),
    }
}

fn s3_field_help(field: S3SyncField) -> HelpContent {
    match field {
        S3SyncField::Preset => HelpContent::new(
            texts::tui_s3_service_preset(),
            help_lines(
                "预设只提供适合该服务的 Region 默认值和字段说明，不会覆盖已经填写的值。当前公开 AWS S3、MinIO、Cloudflare R2 和 Custom；OSS/COS/OBS 要等待上游确定 URL Style 后再开放。",
                "A preset supplies a suitable Region default and contextual guidance; it never overwrites a value you already entered. AWS S3, MinIO, Cloudflare R2, and Custom are currently available. OSS/COS/OBS remain hidden until upstream settles URL-style handling.",
            ),
        ),
        S3SyncField::Region => HelpContent::new(
            texts::tui_s3_region(),
            help_lines(
                "用于 AWS Signature V4 签名的区域。AWS 请填写存储桶所在区域；Cloudflare R2 通常使用 `auto`；MinIO 常用 `us-east-1`，除非服务端另有配置。",
                "Region used for AWS Signature V4. Use the bucket's region for AWS, usually `auto` for Cloudflare R2, and commonly `us-east-1` for MinIO unless the server says otherwise.",
            ),
        ),
        S3SyncField::Bucket => HelpContent::new(
            texts::tui_s3_bucket(),
            help_lines(
                "用于保存 cc-switch 快照的现有存储桶名称。程序不会自动创建存储桶；凭据需要读取、写入和查询对象的权限。",
                "Name of an existing bucket used for cc-switch snapshots. The app does not create buckets; the credentials need permission to read, write, and inspect objects.",
            ),
        ),
        S3SyncField::AccessKeyId => HelpContent::new(
            texts::tui_s3_access_key_id(),
            help_lines(
                "S3 兼容服务颁发的 Access Key ID。它保存在本机 settings.json 中，并按本项目约定在编辑表单里明文显示。",
                "Access Key ID issued by the S3-compatible service. It is stored in the local settings.json and shown as plain text in this editor, following this project's credential-display convention.",
            ),
        ),
        S3SyncField::SecretAccessKey => HelpContent::new(
            texts::tui_s3_secret_access_key(),
            help_lines(
                "与 Access Key ID 配套的 Secret Access Key。此 TUI 按项目约定明文显示且不会写入日志；共享屏幕或录屏时请注意终端内容。未修改该字段时会保留原凭据。",
                "Secret Access Key paired with the Access Key ID. This TUI shows it as plain text by project convention and does not log it; take care when sharing or recording the terminal. Leaving it untouched preserves the saved credential.",
            ),
        ),
        S3SyncField::Endpoint => HelpContent::new(
            texts::tui_s3_endpoint(),
            help_lines(
                "AWS S3 可留空。R2、MinIO 和其他兼容服务请填写完整的 http/https Endpoint，不要追加存储桶或对象路径。HTTP 可用于本机 MinIO，但远程服务建议使用 HTTPS。",
                "Leave this empty for AWS S3. For R2, MinIO, and other compatible services, enter the complete HTTP/HTTPS endpoint without a bucket or object path. HTTP is suitable for local MinIO; use HTTPS for remote services.",
            ),
        ),
        S3SyncField::RemoteRoot => HelpContent::new(
            texts::tui_s3_remote_root(),
            help_lines(
                "远端对象的根前缀，默认 `cc-switch-sync`。不同产品或环境需要完全隔离时可更改；留空保存时会恢复默认值。",
                "Root prefix for remote objects, defaulting to `cc-switch-sync`. Change it to isolate products or environments completely; an empty value is normalized back to the default when saved.",
            ),
        ),
        S3SyncField::Profile => HelpContent::new(
            texts::tui_s3_profile(),
            help_lines(
                "同一远端根目录下的快照命名空间，默认 `default`。不同设备想共享同一份配置时应使用相同 Profile；想彼此隔离则使用不同值。",
                "Snapshot namespace under the same remote root, defaulting to `default`. Devices that share one configuration should use the same profile; use different values to isolate them.",
            ),
        ),
    }
}

fn webdav_field_help(field: WebDavSyncField) -> HelpContent {
    match field {
        WebDavSyncField::BaseUrl => HelpContent::new(
            texts::tui_webdav_base_url(),
            help_lines(
                "用于存放快照的 WebDAV 目录地址，必须是 http/https URL。请使用专用目录；坚果云通常需要指向 `/dav/...` 下的目录。",
                "HTTP/HTTPS URL of the WebDAV directory used for snapshots. Use a dedicated directory; Jianguoyun URLs normally point to a directory below `/dav/...`.",
            ),
        ),
        WebDavSyncField::Username => HelpContent::new(
            texts::tui_webdav_username(),
            help_lines(
                "WebDAV 用户名。支持匿名访问的服务可以留空。",
                "WebDAV username. Leave it empty for a service that permits anonymous access.",
            ),
        ),
        WebDavSyncField::Password => HelpContent::new(
            texts::tui_webdav_password(),
            help_lines(
                "WebDAV 密码或应用专用密码。表单按项目约定明文显示且不会写入日志；未修改时保留原凭据。",
                "WebDAV password or app-specific password. It is shown as plain text by project convention and is not logged; leaving it untouched preserves the saved credential.",
            ),
        ),
        WebDavSyncField::RemoteRoot => HelpContent::new(
            texts::tui_s3_remote_root(),
            help_lines(
                "WebDAV 目录内的同步根目录，默认 `cc-switch-sync`。留空保存时恢复默认值。",
                "Sync root inside the WebDAV directory, defaulting to `cc-switch-sync`. An empty value is normalized back to the default when saved.",
            ),
        ),
        WebDavSyncField::Profile => HelpContent::new(
            texts::tui_s3_profile(),
            help_lines(
                "快照命名空间，默认 `default`。需要共享配置的设备应使用相同 Profile。",
                "Snapshot namespace, defaulting to `default`. Devices sharing one configuration should use the same profile.",
            ),
        ),
    }
}

/// The global help sheet: the static prelude, then one key line per page.
/// The generated pages (MCP/Prompts/Sessions/Skills/Usage) come from the
/// keymap registry so their hints track dispatch; Providers/Config/Settings
/// (and the Hermes-only Memory line) stay hand-written for their app-scope
/// prose. Order matches the previous static sheet.
fn global_help_lines(app: &App, data: &UiData) -> Vec<String> {
    use super::keymap;

    let hermes = matches!(app.app_type, AppType::Hermes);
    let mut lines: Vec<String> = texts::tui_help_prelude()
        .lines()
        .map(str::to_string)
        .collect();

    lines.push(format!(
        "- {}",
        texts::tui_help_line_providers(&app.app_type)
    ));
    lines.push(keymap_bullet("MCP", keymap::mcp::help_items(app, data)));
    if hermes {
        lines.push(format!("- {}", texts::tui_help_line_memory()));
    } else {
        lines.push(keymap_bullet(
            crate::t!("Prompts", "提示词"),
            keymap::prompts::help_items(app, data),
        ));
    }
    lines.push(keymap_bullet(
        crate::t!("Sessions", "会话"),
        keymap::sessions::help_items(app, data),
    ));
    lines.push(keymap_bullet(
        crate::t!("Skills", "技能"),
        keymap::skills_installed::help_items(app, data),
    ));
    lines.push(keymap_bullet(
        crate::t!("Usage", "使用统计"),
        keymap::usage::help_items(app, data),
    ));
    if !hermes {
        lines.push(format!("- {}", texts::tui_help_line_config()));
    }
    lines.push(format!("- {}", texts::tui_help_line_settings()));
    lines
}

/// Render one generated page-key bullet: `- <name>: <k1> <label1>, ...`,
/// with the locale's list punctuation.
fn keymap_bullet(name: &str, items: Vec<(&'static str, &'static str)>) -> String {
    let (colon, sep) = if i18n::is_chinese() {
        ("：", "，")
    } else {
        (": ", ", ")
    };
    let keys = items
        .iter()
        .map(|(display, label)| format!("{display} {label}"))
        .collect::<Vec<_>>()
        .join(sep);
    format!("- {name}{colon}{keys}")
}

fn provider_field_help(app_type: AppType, field: ProviderAddField) -> HelpContent {
    match field {
        ProviderAddField::Id => HelpContent::new(
            tr("供应商 ID", "Provider ID"),
            help_lines(
                "用于配置文件和数据库里的稳定标识。创建后不要随意改动，避免已有引用找不到这个供应商。",
                "Stable identifier used in config files and storage. Avoid changing it after creation, or existing references may stop matching this provider.",
            ),
        ),
        ProviderAddField::Name => HelpContent::new(
            tr("名称", "Name"),
            help_lines(
                "显示在供应商列表中的名称。新建时如果 ID 为空，会根据名称自动生成 ID。",
                "Display name shown in the provider list. When creating a provider, an empty ID is generated from this name.",
            ),
        ),
        ProviderAddField::WebsiteUrl => HelpContent::new(
            tr("网站", "Website"),
            help_lines(
                "供应商的控制台、文档或充值页面地址，仅用于记录和打开查看，不参与请求转发。",
                "Provider console, docs, or billing URL. It is stored for reference and is not used for request routing.",
            ),
        ),
        ProviderAddField::Notes => HelpContent::new(
            tr("备注", "Notes"),
            help_lines(
                "给自己看的短备注，例如账号用途、套餐或到期信息。它不会写入客户端运行配置。",
                "Short private note, such as account purpose, plan, or renewal info. It is not written to the live client config.",
            ),
        ),
        ProviderAddField::ClaudeBaseUrl
        | ProviderAddField::CodexBaseUrl
        | ProviderAddField::GeminiBaseUrl
        | ProviderAddField::OpenCodeBaseUrl
        | ProviderAddField::HermesBaseUrl => {
            let body = if matches!(app_type, AppType::Codex) {
                help_lines(
                    "Codex 原生只支持 OpenAI Responses API 与 GPT 系列模型。\n如果供应商使用 Chat Completions 协议，或模型不是 GPT 系列（如 DeepSeek、Kimi），需要保持本地路由开启，让 cc-switch 做协议和模型路由转换。",
                    "Codex natively supports OpenAI Responses API and GPT-series models.\nIf this provider uses Chat Completions, or the model is not GPT-series, such as DeepSeek or Kimi, keep local routing enabled so cc-switch can translate protocol and model routing.",
                )
            } else {
                help_lines(
                    "供应商 API 的基础地址。通常不需要在末尾重复补全具体接口路径，除非供应商文档明确要求。",
                    "Base URL for the provider API. Usually you do not need to append the final endpoint path unless the provider docs require it.",
                )
            };
            HelpContent::new(texts::tui_label_base_url(), body)
        }
        ProviderAddField::ClaudeApiKey
        | ProviderAddField::CodexApiKey
        | ProviderAddField::GeminiApiKey
        | ProviderAddField::OpenCodeApiKey
        | ProviderAddField::HermesApiKey => HelpContent::new(
            texts::tui_label_api_key(),
            help_lines(
                "供应商 API Key。保存后会按该应用的配置规则写入存储配置；界面会以明文显示当前值。",
                "Provider API key. After saving, it is written using this app's config rules. The UI shows the current value in plaintext.",
            ),
        ),
        ProviderAddField::CodexModel => HelpContent::new(
            texts::model_label(),
            help_lines(
                "Codex 默认模型。第三方模型如果不是 GPT 系列，通常需要本地路由。\n上游模型映射会生成 model_catalog_json，让 Codex 的 /model 命令看到第三方模型；修改目录后通常需要重启 Codex 才会刷新。",
                "Default Codex model. Third-party non-GPT models usually need local routing.\nUpstream model mapping generates model_catalog_json so Codex /model can show third-party models. Restart Codex after catalog changes.",
            ),
        ),
        ProviderAddField::CodexLocalRouting => HelpContent::new(
            texts::tui_label_codex_model_mapping(),
            help_lines(
                "打开模型映射二级页面。填了才生成模型目录：Chat Completions 会生成兼容路由（走本地代理转换），原生 Responses 会生成 model-catalogs.json 供 Codex 直连显示。留空则不生成。\n使用 Chat 格式时，这里还会显示「思考能力」开关。",
                "Opens the model mapping page. A catalog is generated only when filled: Chat Completions produces compatibility routing (via the local proxy), while native Responses generates model-catalogs.json for Codex direct-connect. Left empty, nothing is generated.\nWith the Chat format, reasoning-capability toggles also appear here.",
            ),
        ),
        ProviderAddField::LocalProxySettings => HelpContent::new(
            texts::tui_label_local_proxy_settings(),
            help_lines(
                "配置仅由本地路由或代理接管使用的出站请求选项，包括自定义 User-Agent、Header 覆盖和 Body 覆盖。直连客户端配置不会受影响。",
                "Configures outbound request options used only by local routing or proxy takeover, including a custom User-Agent plus header and body overrides. Direct client configuration is unaffected.",
            ),
        ),
        ProviderAddField::ClaudeModelConfig => HelpContent::new(
            texts::tui_label_claude_model_config(),
            help_lines(
                "配置 Claude 的模型分层。在模型列按 Enter 编辑，按 Space 从 API 自动获取。Sonnet 和 Opus 可用 ←→ 移到 1M 列并按 Enter 切换；1M 只向 Claude Code 声明百万上下文能力，不会检测上游是否真正支持。底层继续使用模型 ID 的 [1M] 后缀，以兼容现有配置。次要快捷键 a 可将当前模型填充到全部角色。",
                "Configures Claude model tiers. In the model column, press Enter to edit or Space to fetch from the API. For Sonnet and Opus, use ←→ to focus the 1M column and Enter to toggle it. 1M only declares million-token context support to Claude Code; it does not detect upstream capability. The existing [1M] model-ID suffix remains the storage format. The secondary a shortcut fills every role from the current model.",
            ),
        ),
        ProviderAddField::ClaudeApiFormat if matches!(app_type, AppType::Codex) => {
            HelpContent::new(
                texts::tui_label_codex_upstream_format(),
                help_lines(
                    "选择上游供应商的 API 格式。供应商原生是 Responses API 就选 Responses（直连，不转换）；使用 Chat Completions 协议就选 Chat（需通过本地路由转换为 Chat Completions）。",
                    "Select the upstream provider's API format. Choose Responses when the provider is natively Responses API (direct, no conversion); choose Chat when it uses Chat Completions (converted via local routing).",
                ),
            )
        }
        ProviderAddField::ClaudeApiFormat => HelpContent::new(
            texts::tui_label_claude_api_format(),
            help_lines(
                "选择供应商 API 的输入格式。非 Anthropic 协议通常需要开启本地路由做格式转换。",
                "Select the input format for the provider's API. Non-Anthropic formats usually need local routing enabled for conversion.",
            ),
        ),
        ProviderAddField::ClaudeFallbackModel => HelpContent::new(
            texts::tui_label_claude_fallback_model(),
            help_lines(
                "用于未明确落到具体角色模型（Haiku、Sonnet、Opus 等）的请求。使用第三方/中转端点时建议填写：否则这些请求（含 Haiku 后台子任务）会以原始 Claude 模型名透传给上游，可能因上游无此模型而报错。官方端点可留空。",
                "A fallback for requests that don't clearly map to a specific role model (Haiku, Sonnet, Opus, etc.). Recommended for third-party/relay endpoints—otherwise such requests (including Haiku background subtasks) are forwarded under their original Claude model name and may fail if the upstream doesn't host it. Safe to leave blank for official endpoints.",
            ),
        ),
        ProviderAddField::ClaudeQuickConfig => HelpContent::new(
            texts::tui_label_claude_quick_config(),
            help_lines(
                "打开 Claude 快捷配置菜单，集中管理隐藏 AI 署名、Teammates 模式、启用 Tool Search、禁用自动升级等开关。",
                "Opens the Claude quick-config menu that groups the hide-AI-attribution, Teammates, Tool Search, and disable-auto-upgrade toggles.",
            ),
        ),
        ProviderAddField::CodexQuickConfig => HelpContent::new(
            texts::tui_label_codex_quick_config(),
            help_lines(
                "打开 Codex 快捷配置菜单，集中管理启用 Goal mode、启用远程压缩等开关。",
                "Opens the Codex quick-config menu that groups the Goal-mode and remote-compaction toggles.",
            ),
        ),
        ProviderAddField::CodexGoalMode => HelpContent::new(
            texts::tui_label_codex_goal_mode(),
            help_lines(
                "启用 Codex 的 Goal mode，写入 config.toml 的 [features] goals = true；关闭时移除该项。",
                "Enables Codex Goal mode by writing [features] goals = true to config.toml; turning it off removes it.",
            ),
        ),
        ProviderAddField::CodexRemoteCompaction => HelpContent::new(
            texts::tui_label_codex_remote_compaction(),
            help_lines(
                "开启后会将当前 model_providers 条目的 name 写为 OpenAI，使 Codex 尝试使用远程压缩；关闭时恢复原供应商名称。",
                "When enabled, sets the current model_providers entry's name to \"OpenAI\" so Codex attempts remote compaction; turning it off restores the provider name.",
            ),
        ),
        ProviderAddField::ClaudeHideAttribution => HelpContent::new(
            texts::tui_label_claude_hide_attribution(),
            help_lines(
                "控制 Claude Code 写入提交或拉取请求时是否附带署名信息。",
                "Controls whether Claude Code adds attribution when writing commits or pull requests.",
            ),
        ),
        ProviderAddField::ClaudeTeammates => HelpContent::new(
            texts::tui_label_claude_teammates(),
            help_lines(
                "启用 Claude Code 的实验性 Teammates（多智能体协作）模式，写入 env.CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1；关闭时移除该变量。",
                "Enables Claude Code's experimental Teammates (multi-agent) mode by writing env.CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1; turning it off removes the variable.",
            ),
        ),
        ProviderAddField::ClaudeToolSearch => HelpContent::new(
            texts::tui_label_claude_tool_search(),
            help_lines(
                "启用 Claude Code 的 Tool Search（工具搜索）能力，写入 env.ENABLE_TOOL_SEARCH=true；关闭时移除该变量。",
                "Enables Claude Code's Tool Search capability by writing env.ENABLE_TOOL_SEARCH=true; turning it off removes the variable.",
            ),
        ),
        ProviderAddField::ClaudeDisableAutoUpgrade => HelpContent::new(
            texts::tui_label_claude_disable_auto_upgrade(),
            help_lines(
                "禁用 Claude Code 的自动升级，写入 env.DISABLE_AUTOUPDATER=1；关闭时移除该变量。",
                "Disables Claude Code's auto-upgrade by writing env.DISABLE_AUTOUPDATER=1; turning it off removes the variable.",
            ),
        ),
        ProviderAddField::CodexOAuthAccount => HelpContent::new(
            texts::tui_label_chatgpt_account(),
            help_lines(
                "绑定 ChatGPT/Codex 官方登录账号。官方账号可用于 Codex 插件、移动端远程操作等官方能力。",
                "Binds an official ChatGPT/Codex account. Official login can keep Codex plugins, mobile remote control, and other official app features available.",
            ),
        ),
        ProviderAddField::CodexFastMode => HelpContent::new(
            texts::tui_label_codex_fast_mode(),
            help_lines(
                "对 Codex OAuth 请求启用优先服务层。只有支持该能力的官方账号或服务才会生效。",
                "Enables the priority service tier for Codex OAuth requests. It only affects accounts or services that support it.",
            ),
        ),
        ProviderAddField::CommonSnippet => HelpContent::new(
            texts::tui_config_item_common_snippet(),
            help_lines(
                "通用配置片段用于保存插件、环境变量等非敏感 TOML，切换供应商时仍会保留。\n上游建议从当前 config.toml 重新提取通用部分后保存；为空时可以先从已有配置里提取。\n只有勾选添加通用配置时才会写入供应商配置。",
                "Common config snippets store non-sensitive TOML such as plugins and environment variables, so they survive provider switches.\nUpstream suggests re-extracting the common part from the current config.toml before saving. If it is empty, start by extracting from an existing config.\nIt is written only when Attach Common Config is enabled.",
            ),
        ),
        ProviderAddField::IncludeCommonConfig => HelpContent::new(
            texts::tui_form_attach_common_config(),
            help_lines(
                "开启后保存供应商时会把通用配置片段合并进去。关闭后只保存该供应商自己的配置。\n这不会把 API Key 等敏感字段放进通用片段。",
                "When enabled, saving the provider merges the common config snippet into it. When disabled, only provider-specific config is saved.\nAPI keys and other sensitive fields do not belong in the common snippet.",
            ),
        ),
        ProviderAddField::UsageQuery => HelpContent::new(
            texts::tui_config_item_usage_query(),
            help_lines(
                "打开用量查询的二级页面。启用后可按模板查询余额、额度或剩余用量。\n第一次进入会显示一次确认提示。",
                "Opens the Usage Query secondary page. When enabled, templates can query balance, quota, or remaining usage.\nThe first entry shows a one-time notice.",
            ),
        ),
        ProviderAddField::GeminiAuthType => HelpContent::new(
            texts::auth_type_label(),
            help_lines(
                "选择 Gemini 使用 OAuth 还是 API Key。切换后相关字段会按所选认证方式显示。",
                "Selects whether Gemini uses OAuth or API key auth. Related fields change with the selected auth mode.",
            ),
        ),
        ProviderAddField::GeminiModel
        | ProviderAddField::OpenCodeModelId
        | ProviderAddField::OpenCodeModelName => HelpContent::new(
            texts::model_label(),
            help_lines(
                "供应商请求时使用的模型名称。应与供应商文档或模型列表中的 ID 保持一致。",
                "Model name sent to the provider. Keep it aligned with the provider docs or model list.",
            ),
        ),
        ProviderAddField::OpenClawApiProtocol => HelpContent::new(
            texts::tui_label_openclaw_api(),
            help_lines(
                "选择 OpenClaw 使用的协议适配器。不同适配器会影响请求格式。",
                "Selects the protocol adapter used by OpenClaw. Different adapters affect request shape.",
            ),
        ),
        ProviderAddField::OpenCodeNpmPackage => HelpContent::new(
            texts::tui_label_provider_package(),
            help_lines(
                "选择 OpenCode 使用的供应商包。不同包会影响请求格式。",
                "Selects the provider package used by OpenCode. Different packages affect request shape.",
            ),
        ),
        ProviderAddField::OpenClawUserAgent => HelpContent::new(
            texts::tui_label_openclaw_user_agent(),
            help_lines(
                "控制请求中是否发送默认 User-Agent。部分供应商会依赖它识别客户端。",
                "Controls whether requests include the default User-Agent. Some providers use it to identify the client.",
            ),
        ),
        ProviderAddField::OpenClawModels => HelpContent::new(
            texts::tui_label_openclaw_models(),
            help_lines(
                "编辑 OpenClaw 模型列表。这里保存的是该供应商暴露给客户端选择的模型配置。",
                "Edits OpenClaw model entries. These are the models exposed to the client for this provider.",
            ),
        ),
        ProviderAddField::OpenCodeModelContextLimit => HelpContent::new(
            texts::tui_label_context_limit(),
            help_lines(
                "模型上下文限制用于告诉客户端可用上下文上限。留空表示不覆盖默认值。",
                "The model context limit tells the client the usable context cap. Leave blank to avoid overriding defaults.",
            ),
        ),
        ProviderAddField::OpenCodeModelOutputLimit => HelpContent::new(
            texts::tui_label_output_limit(),
            help_lines(
                "模型输出限制用于告诉客户端单次响应输出上限。留空表示不覆盖默认值。",
                "The model output limit tells the client the single-response output cap. Leave blank to avoid overriding defaults.",
            ),
        ),
        ProviderAddField::HermesApiMode => HelpContent::new(
            texts::tui_label_hermes_api_mode(),
            help_lines(
                "选择 Hermes 的 API 模式。该模式会影响写入 Hermes 配置的字段形状。",
                "Selects the Hermes API mode. This affects the field shape written into Hermes config.",
            ),
        ),
        ProviderAddField::HermesModels => HelpContent::new(
            texts::tui_label_hermes_models(),
            help_lines(
                "配置 Hermes 供应商的模型字典。每个模型可包含 ID、显示名和上下文长度。",
                "Configures the Hermes provider model dictionary. Each model can include ID, display name, and context length.",
            ),
        ),
        ProviderAddField::HermesRateLimitDelay => HelpContent::new(
            texts::tui_label_hermes_rate_limit_delay(),
            help_lines(
                "Hermes 请求之间的限速延迟。留空表示不设置；填写时必须是非负数字。",
                "Rate-limit delay between Hermes requests. Leave blank to unset it. When filled, it must be a non-negative number.",
            ),
        ),
        ProviderAddField::CodexWireApi
        | ProviderAddField::CodexRequiresOpenaiAuth
        | ProviderAddField::CodexEnvKey
        | ProviderAddField::ClaudeAdvancedDivider
        | ProviderAddField::CodexAdvancedDivider
        | ProviderAddField::HermesAdvancedDivider
        | ProviderAddField::CommonConfigDivider
        | ProviderAddField::UsageQueryDivider => HelpContent::empty(),
    }
}

fn codex_local_routing_field_help(field: CodexLocalRoutingField) -> HelpContent {
    match field {
        CodexLocalRoutingField::Enabled => HelpContent::new(
            texts::tui_codex_local_routing_enable(),
            help_lines(
                "切换本地路由。开启后该 Codex 供应商会使用 OpenAI Chat Completions 格式，并依赖本地代理映射到 Codex 可用的配置。",
                "Toggles local routing. When enabled, this Codex provider uses OpenAI Chat Completions format and relies on the local proxy to map it into Codex-compatible config.",
            ),
        ),
        CodexLocalRoutingField::SupportsThinking => HelpContent::new(
            texts::tui_codex_reasoning_supports_thinking(),
            help_lines(
                "标记该路由模型是否支持思考模式。这个值会写入 Codex 配置，用于生成模型能力描述。",
                "Marks whether the routed model supports thinking mode. This value is written into Codex config for model capability metadata.",
            ),
        ),
        CodexLocalRoutingField::SupportsEffort => HelpContent::new(
            texts::tui_codex_reasoning_supports_effort(),
            help_lines(
                "标记该路由模型是否支持 reasoning effort。只有供应商实际支持按思考等级调节时才应开启。",
                "Marks whether the routed model supports reasoning effort. Enable it only when the provider actually supports effort levels.",
            ),
        ),
        CodexLocalRoutingField::ModelCatalog => HelpContent::new(
            texts::tui_codex_model_catalog(),
            help_lines(
                "在此把供应商模型映射成 Codex 可见模型。a 添加行，Enter 编辑单元格，←→ 切换列，f 拉取，Del 删除。\n三列：\n· 实际请求模型 —— 发给上游的模型 ID，不能为空（例：deepseek-chat）\n· 显示名 —— /model 菜单里显示的名字，留空用模型 ID（例：DeepSeek Chat）\n· 上下文窗口 —— 该模型的上下文长度，留空不覆盖（例：128000）\n修改后通常需要重启 Codex，/model 列表才会刷新。",
                "Map provider models into models visible to Codex here. Press a to add a row, Enter to edit a cell, ←→ to switch columns, f to fetch, Del to delete.\nThree columns:\n· Request model — the model ID sent upstream, cannot be empty (e.g. deepseek-chat)\n· Display name — the name shown in the /model menu, empty uses the model ID (e.g. DeepSeek Chat)\n· Context window — this model's context length, empty to not override (e.g. 128000)\nRestart Codex after changes so the /model list refreshes.",
            ),
        ),
    }
}

fn local_proxy_settings_field_help(field: LocalProxySettingsField) -> HelpContent {
    match field {
        LocalProxySettingsField::UserAgent => HelpContent::new(
            texts::tui_label_custom_user_agent(),
            help_lines(
                "替换本地代理转发给供应商 API 的 User-Agent。可选用兼容 Claude Code 的预设，也可以手动填写；留空表示不覆盖。控制字符会被标记为无效，但不会阻止保存。",
                "Replaces the User-Agent forwarded to the provider API by the local proxy. Choose a Claude Code-compatible preset or enter a custom value; leave blank for no override. Control characters are flagged as invalid but do not block saving.",
            ),
        ),
        LocalProxySettingsField::HeaderOverrides => HelpContent::new(
            texts::tui_label_local_proxy_header_overrides(),
            help_lines(
                "以 JSON 对象配置协议转换后的上游 Header。名称保存为小写，值必须是字符串；认证、内容类型、连接、转发和追踪等由代理管理的 Header 不能覆盖。",
                "Uses a JSON object to override upstream headers after protocol conversion. Names are stored lowercase and values must be strings; proxy-managed authentication, content, connection, forwarding, and tracing headers cannot be overridden.",
            ),
        ),
        LocalProxySettingsField::BodyOverrides => HelpContent::new(
            texts::tui_label_local_proxy_body_overrides(),
            help_lines(
                "以 JSON 对象覆盖协议转换后的最终上游 Body。适合补充 store: false 等供应商专用字段；顶层 stream 由代理控制，不能覆盖。",
                "Uses a JSON object to override the final upstream body after protocol conversion. This is suitable for provider-specific fields such as store: false; top-level stream is controlled by the proxy and cannot be overridden.",
            ),
        ),
    }
}

fn codex_model_catalog_field_help(field: CodexModelCatalogField) -> HelpContent {
    match field {
        CodexModelCatalogField::Model => HelpContent::new(
            tr("模型 ID", "Model ID"),
            help_lines(
                "供应商实际接收的模型 ID。不能为空；可以手动填写，也可以从供应商模型列表抓取后选择。",
                "Provider model ID sent upstream. It cannot be empty; enter it manually or fetch and select it from the provider model list.",
            ),
        ),
        CodexModelCatalogField::DisplayName => HelpContent::new(
            tr("显示名", "Display Name"),
            help_lines(
                "Codex /model 列表中显示的名称。留空时使用模型 ID。",
                "Name shown in the Codex /model list. Leave empty to use the model ID.",
            ),
        ),
        CodexModelCatalogField::ContextWindow => HelpContent::new(
            tr("上下文窗口", "Context Window"),
            help_lines(
                "告诉 Codex 该模型的上下文窗口。留空表示不覆盖；填写时应与供应商实际模型能力一致。",
                "Tells Codex this model's context window. Leave empty to avoid overriding it; filled values should match the provider's actual model capability.",
            ),
        ),
    }
}

fn provider_preview_help(app_type: AppType, section: Option<CodexPreviewSection>) -> HelpContent {
    match (app_type, section) {
        (AppType::Codex, Some(CodexPreviewSection::Auth)) => HelpContent::new(
            texts::tui_codex_auth_json_title(),
            help_lines(
                "这里预览将保存的 Codex 认证配置。代理接管时，上游会强调这里是存储配置，不是运行中的 auth.json。",
                "This previews the Codex auth config to be saved. During proxy takeover, upstream treats this as stored config, not the live auth.json.",
            ),
        ),
        (AppType::Codex, Some(CodexPreviewSection::Config)) => HelpContent::new(
            texts::tui_codex_config_toml_title(),
            help_lines(
                "这里预览将保存的 Codex config.toml。\n本地路由、思考能力、模型目录、目标模式、远程压缩等高级配置通常会体现在这里。预设供应商多数会自动配置；自定义供应商会按名称和地址推断，只有自动识别不准时才需要手动改。",
                "This previews the Codex config.toml to be saved.\nAdvanced settings such as local routing, reasoning capability, model catalog, Goal mode, and remote compaction are reflected here. Presets are usually configured automatically; custom providers are inferred from name and URL, so manual edits are needed only when detection is wrong.",
            ),
        ),
        (AppType::Claude, _) => HelpContent::new(
            texts::tui_provider_config_title(),
            help_lines(
                "这里预览将保存的供应商配置。Proxy 开启时，Claude 实际读取的 settings.json 会由代理临时接管，因此会显示本地代理地址、占位凭据和模型映射，而不是这里的真实上游配置。按 Enter 可打开编辑器进行高级修改。",
                "This previews the stored provider config. While Proxy is on, Claude's live settings.json is temporarily managed by the proxy, so it contains the local proxy URL, placeholder credentials, and model mappings instead of the real upstream config shown here. Press Enter to open the advanced editor.",
            ),
        ),
        _ => HelpContent::new(
            texts::tui_provider_config_title(),
            help_lines(
                "右侧预览展示将保存的供应商配置。按 Enter 可打开编辑器进行高级修改。",
                "The right preview shows the provider config to be saved. Press Enter to open the advanced editor.",
            ),
        ),
    }
}

fn usage_query_field_help(template: UsageQueryTemplate, field: UsageQueryField) -> HelpContent {
    match field {
        UsageQueryField::Enabled => HelpContent::new(
            texts::tui_usage_query_enable(),
            help_lines(
                "启用后 cc-switch 会按当前模板查询供应商余额或额度。未启用时不会自动查询。",
                "When enabled, cc-switch queries provider balance or quota using the selected template. When disabled, no automatic query runs.",
            ),
        ),
        UsageQueryField::Template => HelpContent::new(
            texts::tui_usage_query_template(),
            help_lines(
                "TUI 只提供自定义、通用、NewAPI、余额四种模板。\n计费计划模板不在 TUI 中显示；模板会决定哪些字段可见，以及提取器代码是否可编辑。",
                "The TUI exposes only custom, general, newapi, and balance.\nToken Plan is not shown in the TUI. The template controls visible fields and whether extractor code can be edited.",
            ),
        ),
        UsageQueryField::ApiKey => HelpContent::new(
            texts::tui_usage_query_credentials_config(),
            help_lines(
                "通用模板的 API Key 和 Base URL 可留空。留空时会自动使用当前供应商配置。",
                "For the general template, API Key and Base URL can be left empty. Empty fields use the current provider config.",
            ),
        ),
        UsageQueryField::BaseUrl if matches!(template, UsageQueryTemplate::NewApi) => {
            HelpContent::new(
                texts::tui_usage_query_base_url(),
                help_lines(
                    "NewAPI 服务地址。模板会请求这个地址下的 `/api/user/self`，并用访问令牌和用户 ID 读取额度。",
                    "NewAPI service URL. The template queries `/api/user/self` under this URL and uses the access token and user ID for quota data.",
                ),
            )
        }
        UsageQueryField::BaseUrl => HelpContent::new(
            texts::tui_usage_query_credentials_config(),
            help_lines(
                "通用模板的 Base URL 可留空。留空时会自动使用当前供应商配置。",
                "For the general template, Base URL can be left empty. Empty fields use the current provider config.",
            ),
        ),
        UsageQueryField::AccessToken | UsageQueryField::UserId => HelpContent::new(
            texts::tui_usage_query_credentials_config(),
            help_lines(
                "NewAPI 模板需要访问令牌和用户 ID 来查询 `/api/user/self`。这些字段只对 NewAPI 模板显示。",
                "The NewAPI template uses access token and user ID to query `/api/user/self`. These fields are shown only for NewAPI.",
            ),
        ),
        UsageQueryField::Timeout => HelpContent::new(
            texts::tui_usage_query_timeout_seconds(),
            help_lines(
                "请求超时时间，单位是秒。默认 10 秒；上游运行时会限制在 2-30 秒。",
                "Request timeout in seconds. Default is 10 seconds; upstream runtime clamps it to 2-30 seconds.",
            ),
        ),
        UsageQueryField::AutoInterval => HelpContent::new(
            texts::tui_usage_query_auto_interval(),
            help_lines(
                "自动查询间隔，单位是分钟。默认 5 分钟；填 0 表示不自动查询，最大保存为 1440 分钟。",
                "Automatic query interval in minutes. Default is 5 minutes. Set 0 to disable automatic queries; saved values are capped at 1440 minutes.",
            ),
        ),
        UsageQueryField::CodingPlanProvider => HelpContent::empty(),
        UsageQueryField::Script => usage_query_extractor_help(template),
    }
}

fn usage_query_extractor_help(template: UsageQueryTemplate) -> HelpContent {
    let body = match template {
        UsageQueryTemplate::Custom => help_lines(
            "自定义模板的提取器可以使用当前供应商变量：`{{baseUrl}}` 和 `{{apiKey}}`。\n返回对象应包含剩余额度字段，例如 `remaining`、`unit`，也可以返回 `planName`、`total`、`used`、`extra`。",
            "Custom extractor code can use current provider variables: `{{baseUrl}}` and `{{apiKey}}`.\nThe return object should include remaining quota fields such as `remaining` and `unit`; it may also return `planName`, `total`, `used`, and `extra`.",
        ),
        UsageQueryTemplate::General => help_lines(
            "通用模板默认请求 `{{baseUrl}}/user/balance`，并从响应中提取余额。\n如供应商返回结构不同，可改写提取器代码。",
            "The general template queries `{{baseUrl}}/user/balance` by default and extracts balance from the response.\nIf the provider response differs, edit the extractor code.",
        ),
        UsageQueryTemplate::NewApi => help_lines(
            "NewAPI 模板默认请求 `{{baseUrl}}/api/user/self`，按 NewAPI 的 quota 字段换算额度。",
            "The NewAPI template queries `{{baseUrl}}/api/user/self` and converts NewAPI quota fields into usage values.",
        ),
        UsageQueryTemplate::Balance => help_lines(
            "余额模板使用内置查询逻辑。通常不需要脚本字段；如果显示提取器预览，只用于说明当前模板行为。",
            "The balance template uses built-in query logic. It usually does not need script fields; any extractor preview only explains template behavior.",
        ),
        UsageQueryTemplate::GitHubCopilot | UsageQueryTemplate::TokenPlan => {
            vec![tr("此处无提示", "No help here").to_string()]
        }
    };
    HelpContent::new(texts::tui_usage_query_script_preview_title(), body)
}

fn help_lines(zh: &str, en: &str) -> Vec<String> {
    tr(zh, en).lines().map(str::to_string).collect()
}

fn tr<'a>(zh: &'a str, en: &'a str) -> &'a str {
    if i18n::is_chinese() {
        zh
    } else {
        en
    }
}

fn provider_field_is_divider(field: ProviderAddField) -> bool {
    matches!(
        field,
        ProviderAddField::ClaudeAdvancedDivider
            | ProviderAddField::CodexAdvancedDivider
            | ProviderAddField::HermesAdvancedDivider
            | ProviderAddField::CommonConfigDivider
            | ProviderAddField::UsageQueryDivider
    )
}

fn provider_field_overlay_target(app: &App, field: ProviderAddField) -> HelpTarget {
    let Some(FormState::ProviderAdd(provider)) = app.form.as_ref() else {
        return HelpTarget::Global;
    };

    HelpTarget::ProviderField {
        app_type: provider.app_type.clone(),
        field,
    }
}

fn provider_local_proxy_overlay_target(app: &App, field: LocalProxySettingsField) -> HelpTarget {
    if !matches!(app.form.as_ref(), Some(FormState::ProviderAdd(_))) {
        return HelpTarget::Global;
    }
    HelpTarget::LocalProxySettingsField { field }
}

fn provider_usage_query_overlay_target(app: &App, field: UsageQueryField) -> HelpTarget {
    let Some(FormState::ProviderAdd(provider)) = app.form.as_ref() else {
        return HelpTarget::Global;
    };

    HelpTarget::UsageQueryField {
        template: provider.usage_query_template,
        field,
    }
}
