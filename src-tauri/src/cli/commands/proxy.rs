use clap::Subcommand;

use crate::app_config::AppType;
use crate::cli::ui::{highlight, info, success};
use crate::error::AppError;
use crate::{AppState, ProxyConfig};

#[derive(Subcommand, Debug, Clone)]
pub enum ProxyCommand {
    /// Show current proxy configuration and routes
    Show,

    /// Enable the persisted proxy switch
    Enable,

    /// Disable the persisted proxy switch
    Disable,

    /// Start the local proxy in the foreground for debugging
    Serve {
        /// Override listen address for this run only
        #[arg(long)]
        listen_address: Option<String>,

        /// Override listen port for this run only
        #[arg(long)]
        listen_port: Option<u16>,

        /// Enable manual takeover for the given app while this foreground session is running
        #[arg(long = "takeover", value_enum)]
        takeovers: Vec<AppType>,
    },
}

pub fn execute(cmd: ProxyCommand) -> Result<(), AppError> {
    match cmd {
        ProxyCommand::Show => show_proxy(),
        ProxyCommand::Enable => set_proxy_enabled(true),
        ProxyCommand::Disable => set_proxy_enabled(false),
        ProxyCommand::Serve {
            listen_address,
            listen_port,
            takeovers,
        } => serve_proxy(listen_address, listen_port, takeovers),
    }
}

fn get_state() -> Result<AppState, AppError> {
    AppState::try_new()
}

fn create_runtime() -> Result<tokio::runtime::Runtime, AppError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))
}

fn show_proxy() -> Result<(), AppError> {
    let state = get_state()?;
    let runtime = create_runtime()?;
    let global = runtime.block_on(state.proxy_service.get_global_config())?;
    let config = runtime.block_on(state.proxy_service.get_config())?;
    let status = runtime.block_on(state.proxy_service.get_status());
    let takeovers = runtime
        .block_on(state.proxy_service.get_takeover_status())
        .map_err(AppError::Message)?;

    println!("{}", highlight(crate::t!("Local Proxy", "本地代理")));
    for line in build_proxy_overview_lines(&state, &global, &config, &status, &takeovers) {
        println!("{line}");
    }

    Ok(())
}

fn set_proxy_enabled(enabled: bool) -> Result<(), AppError> {
    let state = get_state()?;
    let runtime = create_runtime()?;
    let config = runtime.block_on(state.proxy_service.set_global_enabled(enabled))?;

    println!(
        "{}",
        success(&format!(
            "{}: {}",
            crate::t!("Proxy switch", "代理开关"),
            if config.proxy_enabled {
                crate::t!("enabled", "开启")
            } else {
                crate::t!("disabled", "关闭")
            }
        ))
    );

    Ok(())
}

fn serve_proxy(
    listen_address: Option<String>,
    listen_port: Option<u16>,
    takeovers: Vec<AppType>,
) -> Result<(), AppError> {
    let state = get_state()?;
    let runtime = create_runtime()?;

    runtime.block_on(async move {
        let service = state.proxy_service.clone();
        let base_config = service.get_config().await?;
        let effective_config = apply_overrides(&base_config, listen_address, listen_port);

        let result = async {
            let server_info = service
                .start_with_runtime_config(effective_config)
                .await
                .map_err(AppError::Message)?;

            if let Err(err) = apply_takeovers(&service, &takeovers).await {
                let _ = service.stop_with_restore().await;
                return Err(AppError::Message(err));
            }

            if let Err(err) = service.publish_runtime_session_if_needed(&server_info) {
                let _ = service.stop_with_restore().await;
                return Err(AppError::Message(err));
            }
            crate::services::state_coordination::clear_restore_mutation_guard_bypass_env();

            println!("{}", highlight(crate::t!("Local Proxy Running", "本地代理已启动")));
            println!(
                "{}",
                success(&format!(
                    "{} http://{}:{}",
                    crate::t!("Listening on", "监听地址"),
                    server_info.address,
                    server_info.port
                ))
            );
            println!(
                "{}",
                info(crate::t!(
                    "Claude: /v1/messages · Codex: /v1/chat/completions + /v1/responses · Gemini: /v1beta/*",
                    "Claude: /v1/messages · Codex: /v1/chat/completions + /v1/responses · Gemini: /v1beta/*"
                ))
            );
            if !takeovers.is_empty() {
                println!(
                    "{}",
                    success(&format!(
                        "{} {}",
                        crate::t!("Manual takeover enabled for:", "已为以下应用开启手动接管："),
                        takeovers
                            .iter()
                            .map(AppType::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                );
            }
            for line in build_auto_failover_status_lines(&state) {
                println!("{}", info(&line));
            }
            println!(
                "{}",
                info(crate::t!(
                    "Press Ctrl-C to stop the proxy.",
                    "按 Ctrl-C 停止代理。"
                ))
            );

            tokio::signal::ctrl_c()
                .await
                .map_err(|e| AppError::Message(format!("failed to listen for Ctrl-C: {e}")))?;

            service
                .stop_with_restore()
                .await
                .map_err(AppError::Message)?;
            println!(
                "{}",
                success(crate::t!("✓ Proxy stopped.", "✓ 代理已停止。"))
            );

            Ok(())
        }
        .await;

        result
    })
}

async fn apply_takeovers(
    service: &crate::ProxyService,
    takeovers: &[AppType],
) -> Result<(), String> {
    for app in takeovers {
        match app {
            AppType::Claude | AppType::Codex | AppType::Gemini => {
                service.set_takeover_for_app(app.as_str(), true).await?;
            }
            _ => {
                return Err(format!(
                    "proxy takeover is not supported for {}",
                    app.as_str()
                ));
            }
        }
    }

    Ok(())
}

fn apply_overrides(
    original: &ProxyConfig,
    listen_address: Option<String>,
    listen_port: Option<u16>,
) -> ProxyConfig {
    let mut config = original.clone();
    if let Some(address) = listen_address {
        config.listen_address = address;
    }
    if let Some(port) = listen_port {
        config.listen_port = port;
    }
    config
}

fn build_proxy_overview_lines(
    state: &AppState,
    global: &crate::proxy::types::GlobalProxyConfig,
    config: &ProxyConfig,
    status: &crate::ProxyStatus,
    takeovers: &crate::proxy::types::ProxyTakeoverStatus,
) -> Vec<String> {
    let current_providers = AppType::all()
        .map(|app| {
            let current = state
                .db
                .get_current_provider(app.as_str())
                .unwrap_or(None)
                .filter(|id| !id.trim().is_empty())
                .unwrap_or_else(|| crate::t!("(not set)", "（未设置）").to_string());
            format!("- {}: {}", app.as_str(), current)
        })
        .collect::<Vec<_>>();

    let listen_host = if status.running && !status.address.is_empty() {
        status.address.as_str()
    } else {
        config.listen_address.as_str()
    };
    let listen_port = if status.running && status.port > 0 {
        status.port
    } else {
        config.listen_port
    };

    let mut lines = vec![
        format!(
            "{}: {}",
            crate::t!("Running", "运行中"),
            if status.running {
                crate::t!("yes", "是")
            } else {
                crate::t!("no", "否")
            }
        ),
        format!(
            "{}: {}",
            crate::t!("Enabled", "启用状态"),
            if global.proxy_enabled {
                crate::t!("enabled", "开启")
            } else {
                crate::t!("disabled", "关闭")
            }
        ),
        format!(
            "{}: {}:{}",
            crate::t!("Listen", "监听"),
            listen_host,
            listen_port
        ),
        crate::t!(
            "Mode: local proxy (manual takeover and automatic failover follow app settings)",
            "模式：本地代理（手动接管和自动故障转移遵循各应用配置）"
        )
        .to_string(),
        format!(
            "{}: {}",
            crate::t!("Retries", "重试次数"),
            config.max_retries
        ),
        format!(
            "{}: {}",
            crate::t!("Logging", "日志"),
            if config.enable_logging {
                crate::t!("enabled", "开启")
            } else {
                crate::t!("disabled", "关闭")
            }
        ),
        format!(
            "{}: {}s / {}s / {}s",
            crate::t!(
                "Timeouts (first-byte / idle / non-stream)",
                "超时（首字 / 空闲 / 非流式）"
            ),
            config.streaming_first_byte_timeout,
            config.streaming_idle_timeout,
            config.non_streaming_timeout
        ),
        String::new(),
        crate::t!("Takeovers:", "接管状态：").to_string(),
        format!(
            "- Claude: {}",
            if takeovers.claude {
                crate::t!("takeover on", "已接管")
            } else {
                crate::t!("takeover off", "未接管")
            }
        ),
        format!(
            "- Codex: {}",
            if takeovers.codex {
                crate::t!("takeover on", "已接管")
            } else {
                crate::t!("takeover off", "未接管")
            }
        ),
        format!(
            "- Gemini: {}",
            if takeovers.gemini {
                crate::t!("takeover on", "已接管")
            } else {
                crate::t!("takeover off", "未接管")
            }
        ),
        String::new(),
        crate::t!("Auto failover:", "自动故障转移：").to_string(),
    ];
    lines.extend(build_auto_failover_status_lines(state));
    lines.extend([
        String::new(),
        crate::t!("Current providers:", "当前供应商：").to_string(),
    ]);
    lines.extend(current_providers);
    lines.extend([
        String::new(),
        crate::t!("Routes:", "路由：").to_string(),
        "- Claude: /v1/messages, /claude/v1/messages".to_string(),
        "- Codex: /chat/completions, /v1/chat/completions, /responses, /v1/responses".to_string(),
        "- Gemini: /v1beta/*, /gemini/v1beta/*".to_string(),
        String::new(),
        crate::t!(
            "Issue #49 manual Claude setup:",
            "Issue #49 的 Claude 手动接线："
        )
        .to_string(),
        format!(
            "- ANTHROPIC_BASE_URL=http://{}:{}",
            listen_host, listen_port
        ),
        "- ANTHROPIC_AUTH_TOKEN=proxy-placeholder".to_string(),
        crate::t!(
            "- Keep the real upstream base URL and API key in the selected Claude provider inside cc-switch.",
            "- 真实上游 base URL 和 API key 仍保存在 cc-switch 里选中的 Claude provider 中。"
        )
        .to_string(),
        String::new(),
        crate::t!(
            "Manual takeover is controlled with --takeover; automatic failover uses each app's proxy settings and failover queue.",
            "手动接管通过 --takeover 控制；自动故障转移使用各应用的代理配置和故障转移队列。"
        )
        .to_string(),
        String::new(),
        format!(
            "{}: cc-switch proxy serve --listen-address {} --listen-port {}",
            crate::t!("Debug command", "调试命令"),
            config.listen_address,
            config.listen_port
        ),
        format!(
            "{}: cc-switch proxy serve --takeover claude",
            crate::t!("Takeover command", "接管命令")
        ),
    ]);

    lines
}

fn build_auto_failover_status_lines(state: &AppState) -> Vec<String> {
    [
        (AppType::Claude, "Claude"),
        (AppType::Codex, "Codex"),
        (AppType::Gemini, "Gemini"),
    ]
    .into_iter()
    .map(|(app, label)| {
        let (_, auto_failover_enabled) = state.db.get_proxy_flags_sync(app.as_str());
        format!(
            "- {}: {}",
            label,
            if auto_failover_enabled {
                crate::t!("auto failover on", "自动故障转移开启")
            } else {
                crate::t!("auto failover off", "自动故障转移关闭")
            }
        )
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use crate::{
        proxy::types::{GlobalProxyConfig, ProxyStatus, ProxyTakeoverStatus},
        Database, MultiAppConfig, ProxyService,
    };

    use super::build_proxy_overview_lines;

    #[test]
    fn proxy_overview_lines_include_runtime_status_and_takeover_state() {
        let db = Arc::new(Database::memory().expect("create database"));
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db),
        };
        let global = GlobalProxyConfig {
            proxy_enabled: true,
            listen_address: "127.0.0.1".to_string(),
            listen_port: 15721,
            enable_logging: true,
        };
        let config = crate::ProxyConfig::default();
        let status = ProxyStatus {
            running: true,
            address: "127.0.0.1".to_string(),
            port: 24567,
            ..Default::default()
        };
        let takeover = ProxyTakeoverStatus {
            claude: true,
            codex: false,
            gemini: true,
        };

        let lines = build_proxy_overview_lines(&state, &global, &config, &status, &takeover);
        let output = lines.join("\n");

        assert!(
            output.contains("Running: yes") || output.contains("运行中: 是"),
            "proxy show output should include foreground runtime status"
        );
        assert!(
            output.contains("127.0.0.1:24567"),
            "proxy show output should prefer the active runtime listen address when the proxy is running"
        );
        assert!(
            output.contains("Claude: takeover on") || output.contains("Claude: 已接管"),
            "proxy show output should include Claude manual takeover state"
        );
        assert!(
            output.contains("Gemini: takeover on") || output.contains("Gemini: 已接管"),
            "proxy show output should include Gemini manual takeover state"
        );
    }

    #[test]
    fn proxy_overview_lines_report_configured_auto_failover_state() {
        let db = Arc::new(Database::memory().expect("create database"));
        db.set_proxy_flags_sync("codex", false, true)
            .expect("enable codex auto failover");
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db),
        };
        let global = GlobalProxyConfig {
            proxy_enabled: true,
            listen_address: "127.0.0.1".to_string(),
            listen_port: 15721,
            enable_logging: true,
        };
        let config = crate::ProxyConfig::default();
        let status = ProxyStatus::default();
        let takeover = ProxyTakeoverStatus::default();

        let lines = build_proxy_overview_lines(&state, &global, &config, &status, &takeover);
        let output = lines.join("\n");

        assert!(
            output.contains("Codex: auto failover on")
                || output.contains("Codex: 自动故障转移开启"),
            "proxy show output should reflect app-specific auto failover settings"
        );
        assert!(
            !output.contains("automatic failover disabled"),
            "proxy show output should not hard-code automatic failover as disabled"
        );
    }
}
