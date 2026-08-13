use clap::Subcommand;

use crate::app_config::AppType;
use crate::cli::proxy_settings::{validate_proxy_listen_address, validate_proxy_listen_port};
use crate::cli::ui::{highlight, info, success};
use crate::error::AppError;
use crate::model_route::ModelRoute;
use crate::{AppState, ProxyConfig};

#[cfg(unix)]
use crate::daemon::ipc::client as daemon_client;
#[cfg(unix)]
use crate::daemon::ipc::protocol::{Request as DaemonRequest, Response as DaemonResponse};
#[cfg(unix)]
use crate::daemon::supervisor::{DAEMON_SOCKET_ENV, SESSION_TOKEN_ENV};

#[derive(Subcommand, Debug, Clone)]
pub enum ModelRouteCommand {
    /// List model routing rules
    List,
    /// Add a model routing rule
    Add {
        /// Wildcard pattern (e.g., *sonnet*, claude-*)
        pattern: String,
        /// Provider ID to route matching models to
        provider_id: String,
        /// Priority (lower = higher priority)
        #[arg(long, default_value = "0")]
        priority: i32,
    },
    /// Remove a model routing rule
    Remove { id: String },
    /// Toggle a model routing rule on/off
    Toggle { id: String },
    /// Update a model routing rule
    Update {
        id: String,
        #[arg(long)]
        pattern: Option<String>,
        #[arg(long = "provider")]
        provider_id: Option<String>,
        #[arg(long)]
        priority: Option<i32>,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum ProxyCommand {
    /// Show current proxy configuration and routes
    Show,

    /// Manage model-based routing rules
    #[command(subcommand)]
    ModelRoute(ModelRouteCommand),

    /// Enable the persisted proxy switch
    Enable,

    /// Disable the persisted proxy switch
    Disable,

    /// Configure the selected app's proxy route
    Config {
        /// Set the global proxy listen address
        #[arg(long)]
        listen_address: Option<String>,

        /// Set the selected app's daemon worker listen port
        #[arg(long)]
        listen_port: Option<u16>,
    },

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

pub fn execute(cmd: ProxyCommand, app: Option<AppType>) -> Result<(), AppError> {
    let app_type = app.unwrap_or(AppType::Claude);
    match cmd {
        ProxyCommand::ModelRoute(subcmd) => {
            let state = get_state()?;
            handle_model_route(&state, &app_type, subcmd)
        }
        ProxyCommand::Show => show_proxy(),
        ProxyCommand::Enable => set_proxy_enabled(app_type, true),
        ProxyCommand::Disable => set_proxy_enabled(app_type, false),
        ProxyCommand::Config {
            listen_address,
            listen_port,
        } => configure_proxy(app_type, listen_address, listen_port),
        ProxyCommand::Serve {
            listen_address,
            listen_port,
            takeovers,
        } => serve_proxy(listen_address, listen_port, takeovers),
    }
}

fn print_model_routes(routes: &[ModelRoute]) {
    if routes.is_empty() {
        println!("{}", info("No model routing rules found."));
        return;
    }
    let mut table = comfy_table::Table::new();
    table.load_preset(comfy_table::presets::UTF8_FULL);
    table.set_header(vec!["ID", "Pattern", "Provider", "Priority", "Enabled"]);
    for r in routes {
        table.add_row(vec![
            r.id.clone(),
            r.pattern.clone(),
            r.provider_id.clone(),
            r.priority.to_string(),
            if r.enabled { "yes" } else { "no" }.to_string(),
        ]);
    }
    println!("{table}");
}

fn handle_model_route(
    state: &AppState,
    app: &AppType,
    cmd: ModelRouteCommand,
) -> Result<(), AppError> {
    match cmd {
        ModelRouteCommand::List => {
            let routes = state.db.list_model_routes(app.as_str())?;
            print_model_routes(&routes);
        }
        ModelRouteCommand::Add {
            pattern,
            provider_id,
            priority,
        } => {
            let route = ModelRoute {
                id: String::new(),
                app_type: app.as_str().to_string(),
                pattern: pattern.clone(),
                provider_id: provider_id.clone(),
                priority,
                enabled: true,
                created_at: None,
                hit_count: 0,
                last_hit_at: None,
                updated_at: None,
            };
            let created = state.db.create_model_route(&route)?;
            println!(
                "{}",
                success(&format!(
                    "Model route created: id={}, pattern=\"{}\" → provider={}, priority={}",
                    created.id, created.pattern, created.provider_id, created.priority
                ))
            );
        }
        ModelRouteCommand::Remove { id } => {
            require_route_for_app(state, &id, app)?;
            state.db.delete_model_route(&id)?;
            println!("{}", success(&format!("Model route {id} removed.")));
        }
        ModelRouteCommand::Toggle { id } => {
            require_route_for_app(state, &id, app)?;
            let toggled = state.db.toggle_model_route(&id)?;
            let status = if toggled.enabled {
                "enabled"
            } else {
                "disabled"
            };
            println!(
                "{}",
                success(&format!(
                    "Model route {id} toggled: pattern=\"{}\" now {status}.",
                    toggled.pattern
                ))
            );
        }
        ModelRouteCommand::Update {
            id,
            pattern,
            provider_id,
            priority,
        } => {
            let existing = require_route_for_app(state, &id, app)?;
            let updated = ModelRoute {
                id: existing.id.clone(),
                app_type: existing.app_type.clone(),
                pattern: pattern.unwrap_or(existing.pattern),
                provider_id: provider_id.unwrap_or(existing.provider_id),
                priority: priority.unwrap_or(existing.priority),
                enabled: existing.enabled,
                created_at: None,
                hit_count: 0,
                last_hit_at: None,
                updated_at: None,
            };
            let result = state.db.update_model_route(&id, &updated)?;
            println!(
                "{}",
                success(&format!(
                    "Model route {id} updated: pattern=\"{}\" → provider={}, priority={}.",
                    result.pattern, result.provider_id, result.priority
                ))
            );
        }
    }
    Ok(())
}

/// 取出路由并校验它属于当前 app，避免 `--app claude` 误删/误改其他 app 的路由。
fn require_route_for_app(
    state: &AppState,
    id: &str,
    app: &AppType,
) -> Result<ModelRoute, AppError> {
    let existing = state
        .db
        .get_model_route(id)?
        .ok_or_else(|| AppError::Database(format!("model route {id} not found")))?;
    if existing.app_type != app.as_str() {
        return Err(AppError::Database(format!(
            "model route {id} belongs to app '{}', not the current app '{}'",
            existing.app_type, app
        )));
    }
    Ok(existing)
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
    let config = runtime.block_on(state.proxy_service.get_config())?;
    let status = runtime.block_on(state.proxy_service.get_status());
    let app_ports = load_proxy_app_ports(&state)?;
    let takeovers = runtime
        .block_on(state.proxy_service.get_takeover_status())
        .map_err(AppError::Message)?;

    println!("{}", highlight(crate::t!("Local Proxy", "本地代理")));
    for line in build_proxy_overview_lines(&state, &config, &status, &app_ports, &takeovers) {
        println!("{line}");
    }

    Ok(())
}

fn set_proxy_enabled(app_type: AppType, enabled: bool) -> Result<(), AppError> {
    if !matches!(app_type, AppType::Claude | AppType::Codex | AppType::Gemini) {
        return Err(AppError::InvalidInput(format!(
            "proxy takeover is not supported for {}",
            app_type.as_str()
        )));
    }
    let state = get_state()?;
    let runtime = create_runtime()?;
    runtime
        .block_on(
            state
                .proxy_service
                .set_managed_session_for_app(app_type.as_str(), enabled),
        )
        .map_err(AppError::Message)?;

    println!(
        "{}",
        success(&format!(
            "{} {}: {}",
            crate::t!("Proxy route", "代理路由"),
            app_type.as_str(),
            if enabled {
                crate::t!("enabled", "开启")
            } else {
                crate::t!("disabled", "关闭")
            }
        ))
    );

    Ok(())
}

fn configure_proxy(
    app_type: AppType,
    listen_address: Option<String>,
    listen_port: Option<u16>,
) -> Result<(), AppError> {
    if listen_address.is_none() && listen_port.is_none() {
        return show_proxy();
    }
    let listen_address = listen_address.map(|address| address.trim().to_string());
    if let Some(address) = &listen_address {
        validate_proxy_listen_address(address)?;
    }
    if let Some(port) = listen_port {
        validate_proxy_listen_port(port)?;
    }
    if listen_port.is_some()
        && !matches!(app_type, AppType::Claude | AppType::Codex | AppType::Gemini)
    {
        return Err(AppError::InvalidInput(format!(
            "proxy takeover is not supported for {}",
            app_type.as_str()
        )));
    }
    let state = get_state()?;
    let runtime = create_runtime()?;
    let status = runtime.block_on(state.proxy_service.get_status());
    if listen_address.is_some() && status.running {
        return Err(AppError::Message(
            "stop the proxy before changing its listen address".to_string(),
        ));
    }
    let app_running = status
        .active_workers
        .iter()
        .any(|worker| worker.app_type == app_type.as_str());
    if listen_port.is_some() && app_running {
        return Err(AppError::Message(format!(
            "stop the {} proxy route before changing its listen port",
            app_type.as_str()
        )));
    }

    if let Some(address) = listen_address {
        let mut config = runtime.block_on(state.proxy_service.get_config())?;
        config.listen_address = address.clone();
        runtime.block_on(state.proxy_service.update_config(&config))?;
        println!(
            "{}",
            success(&format!(
                "{}: {}",
                crate::t!("Proxy listen address", "代理监听地址"),
                address
            ))
        );
    }

    if let Some(port) = listen_port {
        state
            .db
            .set_app_proxy_preferred_port(app_type.as_str(), port)?;
        println!(
            "{}",
            success(&format!(
                "{} {}: {}",
                crate::t!("Proxy listen port", "代理监听端口"),
                app_type.as_str(),
                port
            ))
        );
    }
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
        if !takeovers.is_empty() {
            let status = service.get_status().await;
            if status.running && !status.active_workers.is_empty() {
                return Err(AppError::Message(
                    "cannot run foreground proxy takeover while a daemon-managed proxy session is active; disable daemon-managed proxy routes first"
                        .to_string(),
                ));
            }
        }
        let base_config = service.get_config().await?;
        let effective_config = apply_overrides(&base_config, listen_address, listen_port)?;

        let result = async {
            let server_info = service
                .start_with_runtime_config(effective_config)
                .await
                .map_err(AppError::Message)?;

            let announced_to_daemon = {
                #[cfg(unix)]
                {
                    match announce_to_daemon_if_managed(&server_info) {
                        Ok(announced) => announced,
                        Err(err) => {
                            let _ = service.stop_with_restore().await;
                            return Err(AppError::Message(err));
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    false
                }
            };

            if let Err(err) = apply_takeovers(&service, &takeovers).await {
                let _ = service.stop_with_restore().await;
                return Err(AppError::Message(err));
            }

            if !announced_to_daemon {
                if let Err(err) = service.publish_runtime_session_if_needed(&server_info) {
                    let _ = service.stop_with_restore().await;
                    return Err(AppError::Message(err));
                }
            }
            crate::services::state_coordination::clear_restore_mutation_guard_bypass_env();
            let session_sync_task =
                crate::services::session_usage::spawn_periodic_session_usage_sync(
                    state.db.clone(),
                    "foreground-proxy",
                );
            let usage_maintenance_task = crate::database::Database::spawn_periodic_usage_maintenance(
                state.db.clone(),
                "foreground-proxy",
            );

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
            session_sync_task.abort();
            usage_maintenance_task.abort();

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

#[cfg(unix)]
fn announce_to_daemon_if_managed(
    info: &crate::proxy::types::ProxyServerInfo,
) -> Result<bool, String> {
    let Some(socket_os) = std::env::var_os(DAEMON_SOCKET_ENV) else {
        return Ok(false);
    };
    let socket_path = std::path::PathBuf::from(socket_os);
    let session_token = std::env::var(SESSION_TOKEN_ENV)
        .map_err(|_| "missing CC_SWITCH_PROXY_SESSION_TOKEN env from daemon".to_string())?;
    let request = DaemonRequest::WorkerHello {
        pid: std::process::id(),
        address: info.address.clone(),
        port: info.port,
        session_token,
    };
    let response = daemon_client::round_trip(&socket_path, &request)
        .map_err(|err| format!("worker hello to daemon failed: {err}"))?;
    match response {
        DaemonResponse::Ok => Ok(true),
        DaemonResponse::Error { message } => {
            Err(format!("daemon rejected worker hello: {message}"))
        }
        other => Err(format!(
            "daemon returned unexpected response to worker hello: {other:?}"
        )),
    }
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
) -> Result<ProxyConfig, AppError> {
    let mut config = original.clone();
    if let Some(address) = listen_address {
        config.listen_address = address;
    }
    if let Some(port) = listen_port {
        config.listen_port = port;
    }
    Ok(config)
}

fn load_proxy_app_ports(state: &AppState) -> Result<Vec<(AppType, u16)>, AppError> {
    [AppType::Claude, AppType::Codex, AppType::Gemini]
        .into_iter()
        .map(|app| {
            state
                .db
                .get_app_proxy_preferred_port(app.as_str())
                .map(|port| (app, port))
        })
        .collect()
}

fn build_proxy_route_lines(
    config: &ProxyConfig,
    status: &crate::ProxyStatus,
    app_ports: &[(AppType, u16)],
    takeovers: &crate::proxy::types::ProxyTakeoverStatus,
) -> Vec<String> {
    [
        (AppType::Claude, "Claude", takeovers.claude),
        (AppType::Codex, "Codex", takeovers.codex),
        (AppType::Gemini, "Gemini", takeovers.gemini),
    ]
    .into_iter()
    .map(|(app, label, enabled)| {
        let configured_port = app_configured_port(app_ports, &app).unwrap_or(config.listen_port);
        let worker = status
            .active_workers
            .iter()
            .find(|worker| worker.app_type == app.as_str());
        let state = if enabled {
            crate::t!("enabled", "开启")
        } else {
            crate::t!("disabled", "关闭")
        };

        match worker {
            Some(worker) => format!(
                "- {label}: {state}, {} {}, {} {}:{}{}",
                crate::t!("configured", "配置"),
                configured_port,
                crate::t!("running", "运行"),
                worker.address,
                worker.port,
                worker
                    .pid
                    .map(|pid| format!(" pid={pid}"))
                    .unwrap_or_default()
            ),
            None => format!(
                "- {label}: {state}, {} {}",
                crate::t!("configured", "配置"),
                configured_port
            ),
        }
    })
    .collect()
}

fn app_configured_port(app_ports: &[(AppType, u16)], app: &AppType) -> Option<u16> {
    app_ports
        .iter()
        .find(|(candidate, _)| candidate == app)
        .map(|(_, port)| *port)
}

fn build_proxy_overview_lines(
    state: &AppState,
    config: &ProxyConfig,
    status: &crate::ProxyStatus,
    app_ports: &[(AppType, u16)],
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
    let route_lines = build_proxy_route_lines(config, status, app_ports, takeovers);

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
            "{}: Claude={}, Codex={}, Gemini={}",
            crate::t!("Active routes", "活动路由"),
            if takeovers.claude {
                crate::t!("on", "开启")
            } else {
                crate::t!("off", "关闭")
            },
            if takeovers.codex {
                crate::t!("on", "开启")
            } else {
                crate::t!("off", "关闭")
            },
            if takeovers.gemini {
                crate::t!("on", "开启")
            } else {
                crate::t!("off", "关闭")
            }
        ),
        format!(
            "{}: {}",
            crate::t!("Listen address", "监听地址"),
            listen_host
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
        crate::t!("Proxy app routes:", "代理应用路由：").to_string(),
    ];
    lines.extend(route_lines);
    lines.extend([
        String::new(),
        crate::t!("Auto failover:", "自动故障转移：").to_string(),
    ]);
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
            listen_host,
            app_configured_port(app_ports, &AppType::Claude).unwrap_or(config.listen_port)
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
        proxy::types::{ActiveWorker, ProxyStatus, ProxyTakeoverStatus},
        Database, MultiAppConfig, ProxyService,
    };

    use super::{
        apply_overrides, build_proxy_overview_lines, handle_model_route, load_proxy_app_ports,
        ModelRouteCommand,
    };
    use crate::app_config::AppType;
    use crate::cli::proxy_settings::validate_proxy_listen_port;
    use crate::database::lock_conn;
    use crate::error::AppError;
    use crate::model_route::ModelRoute;

    #[test]
    fn cli_proxy_listen_port_validation_rejects_reserved_ports() {
        let error =
            validate_proxy_listen_port(0).expect_err("port 0 should not be accepted from CLI");

        assert!(error.to_string().contains("1024"));
    }

    #[test]
    fn apply_overrides_allows_ephemeral_listen_port_for_foreground_serve() {
        let config = crate::ProxyConfig::default();
        let updated = apply_overrides(&config, None, Some(0))
            .expect("foreground serve should allow an ephemeral port");

        assert_eq!(updated.listen_port, 0);
    }

    #[test]
    fn apply_overrides_accepts_user_listen_port_range() {
        let config = crate::ProxyConfig::default();
        let updated = apply_overrides(&config, None, Some(1024)).expect("1024 is allowed");

        assert_eq!(updated.listen_port, 1024);
    }

    #[test]
    fn proxy_overview_lines_include_runtime_status_and_takeover_state() {
        let db = Arc::new(Database::memory().expect("create database"));
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db.clone()),
        };
        let config = crate::ProxyConfig {
            listen_port: 15721,
            ..Default::default()
        };
        db.set_proxy_flags_sync("claude", true, false)
            .expect("enable claude proxy route");
        db.set_app_proxy_preferred_port("codex", 15722)
            .expect("save codex preferred proxy port");
        db.set_proxy_flags_sync("gemini", true, false)
            .expect("enable gemini proxy route");
        db.set_app_proxy_preferred_port("gemini", 15723)
            .expect("save gemini preferred proxy port");
        let app_ports = load_proxy_app_ports(&state).expect("load app proxy ports");
        let status = ProxyStatus {
            running: true,
            address: "127.0.0.1".to_string(),
            port: 24567,
            active_workers: vec![
                ActiveWorker {
                    app_type: "claude".to_string(),
                    address: "127.0.0.1".to_string(),
                    port: 15721,
                    pid: Some(1001),
                    started_at: None,
                },
                ActiveWorker {
                    app_type: "gemini".to_string(),
                    address: "127.0.0.1".to_string(),
                    port: 15723,
                    pid: Some(1003),
                    started_at: None,
                },
            ],
            ..Default::default()
        };
        let takeover = ProxyTakeoverStatus {
            claude: true,
            codex: false,
            gemini: true,
        };

        let lines = build_proxy_overview_lines(&state, &config, &status, &app_ports, &takeover);
        let output = lines.join("\n");

        assert!(
            output.contains("Running: yes") || output.contains("运行中: 是"),
            "proxy show output should include foreground runtime status"
        );
        assert!(
            output.contains("Listen address: 127.0.0.1")
                || output.contains("监听地址: 127.0.0.1"),
            "proxy show output should show the active runtime listen address separately from app ports"
        );
        assert!(
            output.contains("Claude: enabled, configured 15721, running 127.0.0.1:15721 pid=1001")
                || output.contains("Claude: 开启, 配置 15721, 运行 127.0.0.1:15721 pid=1001"),
            "proxy show output should include Claude configured and runtime ports"
        );
        assert!(
            output.contains("Codex: disabled, configured 15722")
                || output.contains("Codex: 关闭, 配置 15722"),
            "proxy show output should include Codex configured port even when stopped"
        );
        assert!(
            output.contains("Gemini: enabled, configured 15723, running 127.0.0.1:15723 pid=1003")
                || output.contains("Gemini: 开启, 配置 15723, 运行 127.0.0.1:15723 pid=1003"),
            "proxy show output should include Gemini configured and runtime ports"
        );
        assert!(
            output.contains("Active routes: Claude=on, Codex=off, Gemini=on")
                || output.contains("活动路由: Claude=开启, Codex=关闭, Gemini=开启"),
            "proxy show output should summarize app-specific active routes"
        );
        assert!(
            !output.contains("Listen: 127.0.0.1:24567")
                && !output.contains("监听: 127.0.0.1:24567"),
            "proxy show output should not collapse per-app ports into one listen line"
        );
        assert!(
            !output.contains("Enabled:") && !output.contains("启用状态:"),
            "proxy show output should not present proxy state as a single global enabled flag"
        );
    }

    #[test]
    fn proxy_overview_lines_report_configured_auto_failover_state() {
        let db = Arc::new(Database::memory().expect("create database"));
        let provider = crate::Provider::with_id(
            "codex-p1".to_string(),
            "Codex P1".to_string(),
            serde_json::json!({}),
            None,
        );
        db.save_provider("codex", &provider)
            .expect("save codex failover provider");
        db.add_to_failover_queue("codex", &provider.id)
            .expect("queue codex failover provider");
        db.set_proxy_flags_sync("codex", true, true)
            .expect("enable codex auto failover");
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db.clone()),
        };
        let config = crate::ProxyConfig::default();
        let status = ProxyStatus::default();
        let takeover = ProxyTakeoverStatus::default();
        let app_ports = load_proxy_app_ports(&state).expect("load app proxy ports");

        let lines = build_proxy_overview_lines(&state, &config, &status, &app_ports, &takeover);
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

    // ---------------------------------------------------------------------------
    // Model-route command tests
    // ---------------------------------------------------------------------------

    fn seed_provider(db: &Database, app_type: &str, id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(db.conn);
        conn.execute(
            "INSERT INTO providers (id, app_type, name, settings_config, meta)
             VALUES (?1, ?2, ?3, '{}', '{}')",
            rusqlite::params![id, app_type, id],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    #[test]
    fn model_route_list_empty_shows_no_routes_message() {
        let db = Arc::new(Database::memory().expect("create database"));
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db.clone()),
        };
        let app = AppType::Claude;

        let result = handle_model_route(&state, &app, ModelRouteCommand::List);
        assert!(result.is_ok(), "list should succeed");
    }

    #[test]
    fn model_route_add_and_list_roundtrip() {
        let db = Arc::new(Database::memory().expect("create database"));
        seed_provider(&db, "claude", "test-prov").expect("seed provider");
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db.clone()),
        };
        let app = AppType::Claude;

        // Add a route
        let result = handle_model_route(
            &state,
            &app,
            ModelRouteCommand::Add {
                pattern: "*-4-5".to_string(),
                provider_id: "test-prov".to_string(),
                priority: 0,
            },
        );
        assert!(result.is_ok(), "add should succeed");

        // Verify via list
        let routes = db.list_model_routes("claude").expect("list routes");
        assert_eq!(routes.len(), 1);
        let route = &routes[0];
        assert_eq!(route.pattern, "*-4-5");
        assert_eq!(route.provider_id, "test-prov");
        assert!(route.enabled);
    }

    #[test]
    fn model_route_add_rejects_nonexistent_provider() {
        let db = Arc::new(Database::memory().expect("create database"));
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db.clone()),
        };
        let app = AppType::Claude;

        let result = handle_model_route(
            &state,
            &app,
            ModelRouteCommand::Add {
                pattern: "*-4-5".to_string(),
                provider_id: "nonexistent".to_string(),
                priority: 0,
            },
        );
        assert!(result.is_err(), "add with nonexistent provider should fail");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("provider") && err.contains("not found"),
            "expected provider not found error, got: {err}"
        );
    }

    #[test]
    fn model_route_add_with_explicit_priority() {
        let db = Arc::new(Database::memory().expect("create database"));
        seed_provider(&db, "claude", "test-prov").expect("seed provider");
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db.clone()),
        };
        let app = AppType::Claude;

        let result = handle_model_route(
            &state,
            &app,
            ModelRouteCommand::Add {
                pattern: "*-sonnet".to_string(),
                provider_id: "test-prov".to_string(),
                priority: 7,
            },
        );
        assert!(result.is_ok(), "add with priority should succeed");

        let routes = db.list_model_routes("claude").expect("list routes");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].priority, 7);
    }

    #[test]
    fn model_route_remove_deletes_by_id() {
        let db = Arc::new(Database::memory().expect("create database"));
        seed_provider(&db, "claude", "test-prov").expect("seed provider");
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db.clone()),
        };
        let app = AppType::Claude;

        // Add then remove
        let route_id = db
            .create_model_route(&ModelRoute {
                id: String::new(),
                app_type: "claude".to_string(),
                pattern: "*-sonnet".to_string(),
                provider_id: "test-prov".to_string(),
                priority: 0,
                enabled: true,
                created_at: None,
                hit_count: 0,
                last_hit_at: None,
                updated_at: None,
            })
            .expect("create route")
            .id;

        let result = handle_model_route(
            &state,
            &app,
            ModelRouteCommand::Remove {
                id: route_id.clone(),
            },
        );
        assert!(result.is_ok(), "remove should succeed");

        let routes = db.list_model_routes("claude").expect("list routes");
        assert!(routes.is_empty(), "route should be deleted");
    }

    #[test]
    fn model_route_remove_nonexistent_id_errors() {
        let db = Arc::new(Database::memory().expect("create database"));
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db.clone()),
        };
        let app = AppType::Claude;

        let result = handle_model_route(
            &state,
            &app,
            ModelRouteCommand::Remove {
                id: "missing-route".to_string(),
            },
        );
        assert!(result.is_err(), "remove nonexistent should fail");
    }

    #[test]
    fn model_route_toggle_flips_enabled() {
        let db = Arc::new(Database::memory().expect("create database"));
        seed_provider(&db, "claude", "test-prov").expect("seed provider");
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db.clone()),
        };
        let app = AppType::Claude;

        // Create an enabled route
        let route_id = db
            .create_model_route(&ModelRoute {
                id: String::new(),
                app_type: "claude".to_string(),
                pattern: "*-sonnet".to_string(),
                provider_id: "test-prov".to_string(),
                priority: 0,
                enabled: true,
                created_at: None,
                hit_count: 0,
                last_hit_at: None,
                updated_at: None,
            })
            .expect("create route")
            .id;

        // Toggle off
        let result = handle_model_route(
            &state,
            &app,
            ModelRouteCommand::Toggle {
                id: route_id.clone(),
            },
        );
        assert!(result.is_ok(), "toggle should succeed");

        let route = db
            .get_model_route(&route_id)
            .expect("get route")
            .expect("route exists");
        assert!(!route.enabled, "should be disabled after toggle");

        // Toggle on
        handle_model_route(
            &state,
            &app,
            ModelRouteCommand::Toggle {
                id: route_id.clone(),
            },
        )
        .expect("toggle back");
        let route = db
            .get_model_route(&route_id)
            .expect("get route")
            .expect("route exists");
        assert!(route.enabled, "should be enabled after second toggle");
    }

    #[test]
    fn model_route_toggle_nonexistent_id_errors() {
        let db = Arc::new(Database::memory().expect("create database"));
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db.clone()),
        };
        let app = AppType::Claude;

        let result = handle_model_route(
            &state,
            &app,
            ModelRouteCommand::Toggle {
                id: "missing-route".to_string(),
            },
        );
        assert!(result.is_err(), "toggle nonexistent should fail");
    }

    #[test]
    fn model_route_update_changes_pattern_only() {
        let db = Arc::new(Database::memory().expect("create database"));
        seed_provider(&db, "claude", "test-prov").expect("seed provider");
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db.clone()),
        };
        let app = AppType::Claude;

        let route_id = db
            .create_model_route(&ModelRoute {
                id: String::new(),
                app_type: "claude".to_string(),
                pattern: "original-*".to_string(),
                provider_id: "test-prov".to_string(),
                priority: 5,
                enabled: true,
                created_at: None,
                hit_count: 0,
                last_hit_at: None,
                updated_at: None,
            })
            .expect("create route")
            .id;

        let result = handle_model_route(
            &state,
            &app,
            ModelRouteCommand::Update {
                id: route_id.clone(),
                pattern: Some("new-pattern-*".to_string()),
                provider_id: None,
                priority: None,
            },
        );
        assert!(result.is_ok(), "update pattern should succeed");

        let route = db
            .get_model_route(&route_id)
            .expect("get route")
            .expect("route exists");
        assert_eq!(route.pattern, "new-pattern-*");
        assert_eq!(route.provider_id, "test-prov"); // unchanged
        assert_eq!(route.priority, 5); // unchanged
    }

    #[test]
    fn model_route_update_changes_provider_only() {
        let db = Arc::new(Database::memory().expect("create database"));
        seed_provider(&db, "claude", "test-prov").expect("seed provider");
        seed_provider(&db, "claude", "other-prov").expect("seed provider");
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db.clone()),
        };
        let app = AppType::Claude;

        let route_id = db
            .create_model_route(&ModelRoute {
                id: String::new(),
                app_type: "claude".to_string(),
                pattern: "*-sonnet".to_string(),
                provider_id: "test-prov".to_string(),
                priority: 5,
                enabled: true,
                created_at: None,
                hit_count: 0,
                last_hit_at: None,
                updated_at: None,
            })
            .expect("create route")
            .id;

        let result = handle_model_route(
            &state,
            &app,
            ModelRouteCommand::Update {
                id: route_id.clone(),
                pattern: None,
                provider_id: Some("other-prov".to_string()),
                priority: None,
            },
        );
        assert!(result.is_ok(), "update provider should succeed");

        let route = db
            .get_model_route(&route_id)
            .expect("get route")
            .expect("route exists");
        assert_eq!(route.provider_id, "other-prov");
        assert_eq!(route.pattern, "*-sonnet"); // unchanged
    }

    #[test]
    fn model_route_update_changes_priority_only() {
        let db = Arc::new(Database::memory().expect("create database"));
        seed_provider(&db, "claude", "test-prov").expect("seed provider");
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db.clone()),
        };
        let app = AppType::Claude;

        let route_id = db
            .create_model_route(&ModelRoute {
                id: String::new(),
                app_type: "claude".to_string(),
                pattern: "*-sonnet".to_string(),
                provider_id: "test-prov".to_string(),
                priority: 5,
                enabled: true,
                created_at: None,
                hit_count: 0,
                last_hit_at: None,
                updated_at: None,
            })
            .expect("create route")
            .id;

        let result = handle_model_route(
            &state,
            &app,
            ModelRouteCommand::Update {
                id: route_id.clone(),
                pattern: None,
                provider_id: None,
                priority: Some(99),
            },
        );
        assert!(result.is_ok(), "update priority should succeed");

        let route = db
            .get_model_route(&route_id)
            .expect("get route")
            .expect("route exists");
        assert_eq!(route.priority, 99);
    }

    #[test]
    fn model_route_update_nonexistent_id_errors() {
        let db = Arc::new(Database::memory().expect("create database"));
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db.clone()),
        };
        let app = AppType::Claude;

        let result = handle_model_route(
            &state,
            &app,
            ModelRouteCommand::Update {
                id: "missing-route".to_string(),
                pattern: Some("new-*".to_string()),
                provider_id: None,
                priority: None,
            },
        );
        assert!(result.is_err(), "update nonexistent should fail");
    }

    #[test]
    fn model_route_with_codex_app_type() {
        let db = Arc::new(Database::memory().expect("create database"));
        seed_provider(&db, "codex", "codex-prov").expect("seed provider");
        let state = crate::AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db.clone()),
        };
        let app = AppType::Codex;

        // Add a codex route
        let result = handle_model_route(
            &state,
            &app,
            ModelRouteCommand::Add {
                pattern: "gpt-*".to_string(),
                provider_id: "codex-prov".to_string(),
                priority: 0,
            },
        );
        assert!(result.is_ok(), "add codex route should succeed");

        // Verify stored under codex
        let routes = db.list_model_routes("codex").expect("list codex routes");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].app_type, "codex");
        assert_eq!(routes[0].pattern, "gpt-*");

        // Codex routes should NOT appear in claude listing
        let claude_routes = db.list_model_routes("claude").expect("list claude routes");
        assert!(
            claude_routes.is_empty(),
            "codex routes should not leak to claude"
        );
    }
}
