use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use std::io::Write;

mod claude_temp_launch;
mod codex_temp_launch;
pub mod commands;
pub mod editor;
pub(crate) mod failover_policy;
pub mod i18n;
pub mod interactive;
pub(crate) mod openclaw_form_normalization;
pub(crate) mod provider_quota;
pub(crate) mod proxy_settings;
pub mod terminal;
pub mod tui;
pub mod ui;

use crate::app_config::AppType;

#[derive(Parser)]
#[command(
    name = "cc-switch",
    version,
    about = "All-in-One Assistant for Claude Code, Codex, Gemini & OpenCode CLI",
    long_about = "Unified management for Claude Code, Codex, Gemini, and OpenCode CLI provider configurations, MCP servers, skills, prompts, local proxy routes, and environment checks.\n\nRun without arguments to enter interactive mode."
)]
pub struct Cli {
    /// Specify the application type
    #[arg(short, long, global = true, value_enum)]
    pub app: Option<AppType>,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Manage ChatGPT Codex OAuth accounts
    #[command(subcommand)]
    Auth(commands::auth::AuthCommand),

    /// Manage providers (list, switch, export, speedtest, stream-check, fetch-models, quota)
    #[command(subcommand)]
    Provider(commands::provider::ProviderCommand),

    /// Switch to a provider (shortcut for `provider switch <id>`)
    Use {
        /// Provider ID to switch to
        id: String,
    },

    /// Manage MCP servers (list, add, edit, delete, sync)
    #[command(subcommand)]
    Mcp(commands::mcp::McpCommand),

    /// Manage prompts (list, current, live, import, activate, create, rename, edit)
    #[command(subcommand)]
    Prompts(commands::prompts::PromptsCommand),

    /// Manage skills and skill repositories
    #[command(subcommand)]
    Skills(commands::skills::SkillsCommand),

    /// Manage configuration, backups, common snippets, and WebDAV sync
    #[command(subcommand)]
    Config(commands::config::ConfigCommand),

    /// Manage local multi-app proxy
    #[command(subcommand)]
    Proxy(commands::proxy::ProxyCommand),

    /// Manage persisted UI and integration settings
    #[command(subcommand)]
    Settings(commands::settings::SettingsCommand),

    /// Manage automatic failover and provider queue
    #[command(subcommand)]
    Failover(commands::failover::FailoverCommand),

    /// Manage saved assistant sessions
    #[command(subcommand)]
    Sessions(commands::sessions::SessionsCommand),

    /// Hermes-specific commands (memory blobs etc.)
    #[command(subcommand)]
    Hermes(commands::hermes::HermesCommand),

    /// Start an app with a provider selector without switching the global current provider
    #[cfg(unix)]
    #[command(subcommand)]
    Start(commands::start::StartCommand),

    /// Manage the cc-switch supervisor daemon (start/stop/status/logs)
    #[cfg(unix)]
    #[command(subcommand)]
    Daemon(commands::daemon::DaemonCommand),

    /// Manage environment variables and local CLI tool checks
    #[command(subcommand)]
    Env(commands::env::EnvCommand),

    /// Update cc-switch binary to latest release
    Update(commands::update::UpdateCommand),

    /// Enter interactive mode
    #[command(alias = "ui")]
    Interactive,

    /// Generate, install, inspect, or uninstall shell completions
    Completions(commands::completions::CompletionsCommand),

    #[command(name = "internal", hide = true, subcommand)]
    Internal(commands::internal::InternalCommand),
}

/// Generate shell completions
pub fn generate_completions(shell: Shell) {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    generate_completions_to(shell, &mut handle);
}

pub(crate) fn generate_completions_to<W: Write>(shell: Shell, writer: &mut W) {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, name, writer);
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};
    use std::ffi::OsString;

    use super::{Cli, Commands};
    use crate::app_config::AppType;
    use crate::cli::commands::completions::{
        CompletionLifecycleCommand, CompletionsAction, ManagedShellSelection,
    };

    #[test]
    fn long_help_mentions_prompts_and_proxy_routes() {
        let mut cmd = Cli::command();
        let help = cmd.render_long_help().to_string();

        assert!(help.contains("prompts, local proxy routes, and environment checks"));
    }

    #[test]
    fn skills_help_uses_current_storage_description() {
        let mut cmd = Cli::command();
        let skills = cmd
            .find_subcommand_mut("skills")
            .expect("skills subcommand should exist");
        let help = skills.render_long_help().to_string();

        assert!(!help.contains("skills.json"));
        assert!(help.contains("SSOT + database state"));
    }

    #[test]
    fn parses_proxy_serve_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "proxy", "serve", "--listen-port", "0"]);

        match cli.command {
            Some(Commands::Proxy(super::commands::proxy::ProxyCommand::Serve {
                listen_port,
                ..
            })) => {
                assert_eq!(listen_port, Some(0));
            }
            _ => panic!("expected proxy serve command"),
        }
    }

    #[test]
    fn parses_use_shortcut_command() {
        let cli = Cli::parse_from(["cc-switch", "use", "demo"]);

        match cli.command {
            Some(Commands::Use { id }) => assert_eq!(id, "demo"),
            _ => panic!("expected use shortcut command"),
        }
    }

    #[test]
    fn parses_use_shortcut_with_app_global() {
        let cli = Cli::parse_from(["cc-switch", "--app", "codex", "use", "demo"]);

        assert_eq!(cli.app, Some(AppType::Codex));
        match cli.command {
            Some(Commands::Use { id }) => assert_eq!(id, "demo"),
            _ => panic!("expected use shortcut command"),
        }
    }

    #[test]
    fn parses_proxy_serve_takeover_flags() {
        let cli = Cli::parse_from([
            "cc-switch",
            "proxy",
            "serve",
            "--takeover",
            "claude",
            "--takeover",
            "codex",
        ]);

        match cli.command {
            Some(Commands::Proxy(super::commands::proxy::ProxyCommand::Serve {
                takeovers,
                ..
            })) => {
                assert_eq!(
                    takeovers,
                    vec![super::AppType::Claude, super::AppType::Codex]
                );
            }
            _ => panic!("expected proxy serve command with takeover flags"),
        }
    }

    #[test]
    fn parses_proxy_enable_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "proxy", "enable"]);

        match cli.command {
            Some(Commands::Proxy(super::commands::proxy::ProxyCommand::Enable)) => {}
            _ => panic!("expected proxy enable command"),
        }
    }

    #[test]
    fn parses_proxy_disable_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "proxy", "disable"]);

        match cli.command {
            Some(Commands::Proxy(super::commands::proxy::ProxyCommand::Disable)) => {}
            _ => panic!("expected proxy disable command"),
        }
    }

    #[test]
    fn parses_proxy_config_listen_port_subcommand() {
        let cli = Cli::parse_from([
            "cc-switch",
            "--app",
            "codex",
            "proxy",
            "config",
            "--listen-port",
            "15722",
        ]);

        assert_eq!(cli.app, Some(super::AppType::Codex));
        match cli.command {
            Some(Commands::Proxy(super::commands::proxy::ProxyCommand::Config {
                listen_port,
                ..
            })) => {
                assert_eq!(listen_port, Some(15722));
            }
            _ => panic!("expected proxy config command"),
        }
    }

    #[test]
    fn parses_proxy_config_listen_address_subcommand() {
        let cli = Cli::parse_from([
            "cc-switch",
            "proxy",
            "config",
            "--listen-address",
            "localhost",
        ]);

        match cli.command {
            Some(Commands::Proxy(super::commands::proxy::ProxyCommand::Config {
                listen_address,
                ..
            })) => {
                assert_eq!(listen_address.as_deref(), Some("localhost"));
            }
            _ => panic!("expected proxy config command"),
        }
    }

    #[test]
    fn parses_update_check_json_flags() {
        let cli = Cli::parse_from(["cc-switch", "update", "--check", "--json"]);

        match cli.command {
            Some(Commands::Update(update)) => {
                assert!(update.check);
                assert!(update.json);
                assert_eq!(update.version, None);
            }
            _ => panic!("expected update command"),
        }
    }

    #[test]
    fn update_check_conflicts_with_explicit_version() {
        let err = match Cli::try_parse_from([
            "cc-switch",
            "update",
            "--check",
            "--version",
            "v999.0.0",
        ]) {
            Ok(_) => panic!("update --check should reject --version"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn update_json_requires_check() {
        let err = match Cli::try_parse_from(["cc-switch", "update", "--json"]) {
            Ok(_) => panic!("update --json should require --check"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn parses_settings_visible_apps_enable_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "settings", "visible-apps", "enable", "gemini"]);

        match cli.command {
            Some(Commands::Settings(super::commands::settings::SettingsCommand::VisibleApps(
                super::commands::settings::VisibleAppsCommand::Enable { app },
            ))) => {
                assert_eq!(app, super::AppType::Gemini);
            }
            _ => panic!("expected settings visible-apps enable command"),
        }
    }

    #[test]
    fn parses_settings_language_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "settings", "language", "zh"]);

        match cli.command {
            Some(Commands::Settings(super::commands::settings::SettingsCommand::Language {
                language: Some(super::commands::settings::LanguageArg::Zh),
            })) => {}
            _ => panic!("expected settings language command"),
        }
    }

    #[test]
    fn parses_failover_enable_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "failover", "enable"]);

        match cli.command {
            Some(Commands::Failover(super::commands::failover::FailoverCommand::Enable)) => {}
            _ => panic!("expected failover enable command"),
        }
    }

    #[test]
    fn parses_failover_disable_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "failover", "disable"]);

        match cli.command {
            Some(Commands::Failover(super::commands::failover::FailoverCommand::Disable)) => {}
            _ => panic!("expected failover disable command"),
        }
    }

    #[test]
    fn parses_failover_list_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "failover", "list"]);

        match cli.command {
            Some(Commands::Failover(super::commands::failover::FailoverCommand::List)) => {}
            _ => panic!("expected failover list command"),
        }
    }

    #[test]
    fn parses_failover_add_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "failover", "add", "p1"]);

        match cli.command {
            Some(Commands::Failover(super::commands::failover::FailoverCommand::Add { id })) => {
                assert_eq!(id, "p1");
            }
            _ => panic!("expected failover add command"),
        }
    }

    #[test]
    fn parses_failover_remove_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "failover", "remove", "p1"]);

        match cli.command {
            Some(Commands::Failover(super::commands::failover::FailoverCommand::Remove { id })) => {
                assert_eq!(id, "p1");
            }
            _ => panic!("expected failover remove command"),
        }
    }

    #[test]
    fn parses_failover_move_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "failover", "move", "p1", "up"]);

        match cli.command {
            Some(Commands::Failover(super::commands::failover::FailoverCommand::Move {
                id,
                direction,
            })) => {
                assert_eq!(id, "p1");
                assert_eq!(
                    direction,
                    super::commands::failover::FailoverMoveDirection::Up
                );
            }
            _ => panic!("expected failover move command"),
        }
    }

    #[test]
    fn parses_failover_clear_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "failover", "clear", "--yes"]);

        match cli.command {
            Some(Commands::Failover(super::commands::failover::FailoverCommand::Clear { yes })) => {
                assert!(yes);
            }
            _ => panic!("expected failover clear command"),
        }
    }

    #[test]
    fn parses_failover_show_with_app() {
        let cli = Cli::parse_from(["cc-switch", "--app", "codex", "failover", "show"]);

        assert_eq!(cli.app, Some(super::AppType::Codex));
        match cli.command {
            Some(Commands::Failover(super::commands::failover::FailoverCommand::Show)) => {}
            _ => panic!("expected failover show command"),
        }
    }

    #[test]
    fn parses_sessions_list_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "sessions", "list", "--all", "--json"]);

        match cli.command {
            Some(Commands::Sessions(super::commands::sessions::SessionsCommand::List {
                provider,
                all,
                json,
            })) => {
                assert_eq!(provider, None);
                assert!(all);
                assert!(json);
            }
            _ => panic!("expected sessions list command"),
        }
    }

    #[test]
    fn parses_sessions_list_with_backend_provider_id() {
        let cli = Cli::parse_from(["cc-switch", "sessions", "list", "--provider", "opencode"]);

        match cli.command {
            Some(Commands::Sessions(super::commands::sessions::SessionsCommand::List {
                provider,
                all,
                ..
            })) => {
                assert_eq!(provider, Some(super::AppType::OpenCode));
                assert!(!all);
            }
            _ => panic!("expected sessions list command"),
        }
    }

    #[test]
    fn parses_sessions_sync_usage_subcommand() {
        let cli = Cli::parse_from([
            "cc-switch",
            "sessions",
            "sync-usage",
            "--provider",
            "codex",
            "--json",
        ]);

        match cli.command {
            Some(Commands::Sessions(super::commands::sessions::SessionsCommand::SyncUsage {
                provider,
                all,
                json,
            })) => {
                assert_eq!(provider, Some(super::AppType::Codex));
                assert!(!all);
                assert!(json);
            }
            _ => panic!("expected sessions sync-usage command"),
        }
    }

    #[test]
    fn parses_sessions_show_with_provider() {
        let cli = Cli::parse_from([
            "cc-switch",
            "sessions",
            "show",
            "abc",
            "--provider",
            "openclaw",
            "--json",
        ]);

        match cli.command {
            Some(Commands::Sessions(super::commands::sessions::SessionsCommand::Show {
                selector,
                provider,
                json,
                ..
            })) => {
                assert_eq!(selector, "abc");
                assert_eq!(provider, Some(super::AppType::OpenClaw));
                assert!(json);
            }
            _ => panic!("expected sessions show command"),
        }
    }

    #[test]
    fn parses_sessions_resume_print_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "sessions", "resume", "abc", "--print"]);

        match cli.command {
            Some(Commands::Sessions(super::commands::sessions::SessionsCommand::Resume {
                selector,
                print,
                ..
            })) => {
                assert_eq!(selector, "abc");
                assert!(print);
            }
            _ => panic!("expected sessions resume command"),
        }
    }

    #[test]
    fn parses_sessions_delete_yes_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "sessions", "delete", "abc", "--yes"]);

        match cli.command {
            Some(Commands::Sessions(super::commands::sessions::SessionsCommand::Delete {
                selector,
                yes,
                ..
            })) => {
                assert_eq!(selector, "abc");
                assert!(yes);
            }
            _ => panic!("expected sessions delete command"),
        }
    }

    #[test]
    fn parses_auth_status_json_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "auth", "status", "--json"]);

        match cli.command {
            Some(Commands::Auth(super::commands::auth::AuthCommand::Status { json })) => {
                assert!(json);
            }
            _ => panic!("expected auth status command"),
        }
    }

    #[test]
    fn parses_auth_login_json_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "auth", "login", "--json"]);

        match cli.command {
            Some(Commands::Auth(super::commands::auth::AuthCommand::Login { json })) => {
                assert!(json);
            }
            _ => panic!("expected auth login command"),
        }
    }

    #[test]
    fn rejects_auth_login_no_poll_dead_end() {
        let result = Cli::try_parse_from(["cc-switch", "auth", "login", "--no-poll"]);

        assert!(result.is_err());
    }

    #[test]
    fn parses_auth_default_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "auth", "default", "acc-123"]);

        match cli.command {
            Some(Commands::Auth(super::commands::auth::AuthCommand::Default { account_id })) => {
                assert_eq!(account_id, "acc-123");
            }
            _ => panic!("expected auth default command"),
        }
    }

    #[test]
    fn parses_auth_remove_yes_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "auth", "remove", "acc-123", "--yes"]);

        match cli.command {
            Some(Commands::Auth(super::commands::auth::AuthCommand::Remove {
                account_id,
                yes,
            })) => {
                assert_eq!(account_id, "acc-123");
                assert!(yes);
            }
            _ => panic!("expected auth remove command"),
        }
    }

    #[test]
    fn parses_auth_logout_yes_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "auth", "logout", "--yes"]);

        match cli.command {
            Some(Commands::Auth(super::commands::auth::AuthCommand::Logout { yes })) => {
                assert!(yes);
            }
            _ => panic!("expected auth logout command"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn parses_start_claude_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "start", "claude", "demo"]);

        match cli.command {
            Some(Commands::Start(super::commands::start::StartCommand::Claude {
                selector,
                dry_run,
                native_args,
            })) => {
                assert_eq!(selector, "demo");
                assert!(!dry_run);
                assert!(native_args.is_empty());
            }
            _ => panic!("expected start claude command"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn parses_start_claude_dry_run_option() {
        let cli = Cli::parse_from(["cc-switch", "start", "claude", "demo", "--dry-run"]);

        match cli.command {
            Some(Commands::Start(super::commands::start::StartCommand::Claude {
                selector,
                dry_run,
                native_args,
            })) => {
                assert_eq!(selector, "demo");
                assert!(dry_run);
                assert!(native_args.is_empty());
            }
            _ => panic!("expected start claude dry-run command"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn parses_start_claude_native_args_after_double_dash() {
        let cli = Cli::parse_from([
            "cc-switch",
            "start",
            "claude",
            "demo",
            "--",
            "--dangerously-skip-permissions",
        ]);

        match cli.command {
            Some(Commands::Start(super::commands::start::StartCommand::Claude {
                selector,
                dry_run,
                native_args,
            })) => {
                assert_eq!(selector, "demo");
                assert!(!dry_run);
                assert_eq!(
                    native_args,
                    vec![OsString::from("--dangerously-skip-permissions")]
                );
            }
            _ => panic!("expected start claude command with native args"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_start_claude_native_args_without_double_dash() {
        let result = Cli::try_parse_from([
            "cc-switch",
            "start",
            "claude",
            "demo",
            "--dangerously-skip-permissions",
        ]);
        let rendered = match result {
            Ok(_) => panic!("native args without `--` should be rejected"),
            Err(err) => err.to_string(),
        };

        assert!(rendered.contains("-- --dangerously-skip-permissions"));
    }

    #[cfg(unix)]
    #[test]
    fn parses_start_codex_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "start", "codex", "demo"]);

        match cli.command {
            Some(Commands::Start(super::commands::start::StartCommand::Codex {
                selector,
                dry_run,
                native_args,
            })) => {
                assert_eq!(selector, "demo");
                assert!(!dry_run);
                assert!(native_args.is_empty());
            }
            _ => panic!("expected start codex command"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn parses_start_codex_dry_run_with_native_args_after_double_dash() {
        let cli = Cli::parse_from([
            "cc-switch",
            "start",
            "codex",
            "demo",
            "--dry-run",
            "--",
            "--model",
            "gpt-5.4",
        ]);

        match cli.command {
            Some(Commands::Start(super::commands::start::StartCommand::Codex {
                selector,
                dry_run,
                native_args,
            })) => {
                assert_eq!(selector, "demo");
                assert!(dry_run);
                assert_eq!(
                    native_args,
                    vec![OsString::from("--model"), OsString::from("gpt-5.4")]
                );
            }
            _ => panic!("expected start codex dry-run command with native args"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn parses_start_codex_multiple_native_args_after_double_dash() {
        let cli = Cli::parse_from([
            "cc-switch",
            "start",
            "codex",
            "demo",
            "--",
            "--model",
            "gpt-5.4",
            "--profile",
            "local",
        ]);

        match cli.command {
            Some(Commands::Start(super::commands::start::StartCommand::Codex {
                selector,
                dry_run,
                native_args,
            })) => {
                assert_eq!(selector, "demo");
                assert!(!dry_run);
                assert_eq!(
                    native_args,
                    vec![
                        OsString::from("--model"),
                        OsString::from("gpt-5.4"),
                        OsString::from("--profile"),
                        OsString::from("local"),
                    ]
                );
            }
            _ => panic!("expected start codex command with native args"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn start_claude_help_mentions_double_dash_passthrough_examples() {
        let mut cmd = Cli::command();
        let start = cmd
            .find_subcommand_mut("start")
            .expect("start subcommand should exist");
        let claude = start
            .find_subcommand_mut("claude")
            .expect("claude subcommand should exist");
        let help = claude.render_long_help().to_string();

        assert!(help.contains("--dry-run"));
        assert!(help.contains("Native Claude CLI arguments to pass through after `--`"));
        assert!(help.contains("cc-switch start claude demo --dry-run"));
        assert!(help.contains("cc-switch start claude demo -- --dangerously-skip-permissions"));
    }

    #[cfg(unix)]
    #[test]
    fn start_codex_help_mentions_double_dash_passthrough_examples() {
        let mut cmd = Cli::command();
        let start = cmd
            .find_subcommand_mut("start")
            .expect("start subcommand should exist");
        let codex = start
            .find_subcommand_mut("codex")
            .expect("codex subcommand should exist");
        let help = codex.render_long_help().to_string();

        assert!(help.contains("--dry-run"));
        assert!(help.contains("Native Codex CLI arguments to pass through after `--`"));
        assert!(help.contains("cc-switch start codex demo --dry-run"));
        assert!(help.contains("cc-switch start codex demo -- --model gpt-5.4"));
    }

    #[test]
    fn parses_prompts_live_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "prompts", "live"]);

        match cli.command {
            Some(Commands::Prompts(super::commands::prompts::PromptsCommand::Live)) => {}
            _ => panic!("expected prompts live command"),
        }
    }

    #[test]
    fn parses_prompts_import_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "prompts", "import"]);

        match cli.command {
            Some(Commands::Prompts(super::commands::prompts::PromptsCommand::Import)) => {}
            _ => panic!("expected prompts import command"),
        }
    }

    #[test]
    fn parses_provider_stream_check_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "provider", "stream-check", "demo"]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::StreamCheck {
                id,
            })) => {
                assert_eq!(id, "demo");
            }
            _ => panic!("expected provider stream-check command"),
        }
    }

    #[test]
    fn parses_provider_add_template_option() {
        let cli = Cli::parse_from(["cc-switch", "provider", "add", "--template", "codex-oauth"]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::Add {
                template,
            })) => {
                assert_eq!(
                    template,
                    Some(super::commands::provider_input::ProviderAddTemplate::CodexOauth)
                );
            }
            _ => panic!("expected provider add command with template"),
        }
    }

    #[test]
    fn parses_provider_duplicate_edit_option() {
        let cli = Cli::parse_from(["cc-switch", "provider", "duplicate", "demo", "--edit"]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::Duplicate {
                id,
                edit,
            })) => {
                assert_eq!(id, "demo");
                assert!(edit);
            }
            _ => panic!("expected provider duplicate command with edit option"),
        }
    }

    #[test]
    fn parses_provider_duplicate_without_edit_option() {
        let cli = Cli::parse_from(["cc-switch", "provider", "duplicate", "demo"]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::Duplicate {
                id,
                edit,
            })) => {
                assert_eq!(id, "demo");
                assert!(!edit);
            }
            _ => panic!("expected provider duplicate command without edit option"),
        }
    }

    #[test]
    fn parses_provider_fetch_models_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "provider", "fetch-models", "demo"]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::FetchModels {
                id,
                base_url,
                api_key,
                auth,
            })) => {
                assert_eq!(id.as_deref(), Some("demo"));
                assert_eq!(base_url, None);
                assert_eq!(api_key, None);
                assert_eq!(auth, None);
            }
            _ => panic!("expected provider fetch-models command"),
        }
    }

    #[test]
    fn parses_provider_fetch_models_one_off_options() {
        let cli = Cli::parse_from([
            "cc-switch",
            "--app",
            "gemini",
            "provider",
            "fetch-models",
            "--base-url",
            "https://gemini.example.com",
            "--api-key",
            "sk-gemini",
            "--auth",
            "google-api-key",
        ]);

        assert_eq!(cli.app, Some(AppType::Gemini));
        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::FetchModels {
                id,
                base_url,
                api_key,
                auth,
            })) => {
                assert_eq!(id, None);
                assert_eq!(base_url.as_deref(), Some("https://gemini.example.com"));
                assert_eq!(api_key.as_deref(), Some("sk-gemini"));
                assert_eq!(
                    auth,
                    Some(super::commands::provider::ModelFetchAuthArg::GoogleApiKey)
                );
            }
            _ => panic!("expected provider fetch-models command"),
        }
    }

    #[test]
    fn provider_fetch_models_requires_id_or_base_url() {
        let err = match Cli::try_parse_from(["cc-switch", "provider", "fetch-models"]) {
            Ok(_) => panic!("provider fetch-models should require id or --base-url"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn provider_fetch_models_rejects_saved_id_with_one_off_base_url() {
        let err = match Cli::try_parse_from([
            "cc-switch",
            "provider",
            "fetch-models",
            "demo",
            "--base-url",
            "https://api.example.com",
        ]) {
            Ok(_) => panic!("saved provider id should conflict with --base-url"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parses_provider_quota_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "provider", "quota", "demo"]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::Quota {
                id,
                json,
            })) => {
                assert_eq!(id, "demo");
                assert!(!json);
            }
            _ => panic!("expected provider quota command"),
        }
    }

    #[test]
    fn parses_provider_quota_json_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "provider", "quota", "demo", "--json"]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::Quota {
                id,
                json,
            })) => {
                assert_eq!(id, "demo");
                assert!(json);
            }
            _ => panic!("expected provider quota json command"),
        }
    }

    #[test]
    fn parses_provider_usage_query_show_json_subcommand() {
        let cli = Cli::parse_from([
            "cc-switch",
            "provider",
            "usage-query",
            "show",
            "demo",
            "--json",
        ]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::UsageQuery(
                super::commands::provider_usage_query::ProviderUsageQueryCommand::Show { id, json },
            ))) => {
                assert_eq!(id, "demo");
                assert!(json);
            }
            _ => panic!("expected provider usage-query show command"),
        }
    }

    #[test]
    fn parses_provider_usage_query_set_subcommand() {
        let cli = Cli::parse_from([
            "cc-switch",
            "provider",
            "usage-query",
            "set",
            "demo",
            "--enabled",
            "--template",
            "newapi",
            "--timeout",
            "12",
            "--auto-query-interval",
            "1441",
            "--base-url",
            "https://usage.example.com",
            "--access-token",
            "token-demo",
            "--user-id",
            "user-demo",
        ]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::UsageQuery(
                super::commands::provider_usage_query::ProviderUsageQueryCommand::Set(command),
            ))) => {
                assert_eq!(command.id, "demo");
                assert!(command.enabled);
                assert_eq!(
                    command.template,
                    Some(super::commands::provider_usage_query::UsageQueryTemplate::Newapi)
                );
                assert_eq!(command.timeout, Some(12));
                assert_eq!(command.auto_query_interval, Some(1441));
                assert_eq!(
                    command.base_url.as_deref(),
                    Some("https://usage.example.com")
                );
                assert_eq!(command.access_token.as_deref(), Some("token-demo"));
                assert_eq!(command.user_id.as_deref(), Some("user-demo"));
            }
            _ => panic!("expected provider usage-query set command"),
        }
    }

    #[test]
    fn parses_provider_usage_query_clear_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "provider", "usage-query", "clear", "demo"]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::UsageQuery(
                super::commands::provider_usage_query::ProviderUsageQueryCommand::Clear { id },
            ))) => {
                assert_eq!(id, "demo");
            }
            _ => panic!("expected provider usage-query clear command"),
        }
    }

    #[test]
    fn parses_provider_import_live_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "provider", "import-live"]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::ImportLive)) => {}
            _ => panic!("expected provider import-live command"),
        }
    }

    #[test]
    fn parses_provider_remove_from_config_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "provider", "remove-from-config", "demo"]);

        match cli.command {
            Some(Commands::Provider(
                super::commands::provider::ProviderCommand::RemoveFromConfig { id },
            )) => {
                assert_eq!(id, "demo");
            }
            _ => panic!("expected provider remove-from-config command"),
        }
    }

    #[test]
    fn parses_provider_set_default_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "provider", "set-default", "demo"]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::SetDefault {
                id,
                model,
            })) => {
                assert_eq!(id, "demo");
                assert_eq!(model, None);
            }
            _ => panic!("expected provider set-default command"),
        }
    }

    #[test]
    fn parses_provider_set_default_with_model_subcommand() {
        let cli = Cli::parse_from([
            "cc-switch",
            "provider",
            "set-default",
            "demo",
            "--model",
            "gpt-5.4",
        ]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::SetDefault {
                id,
                model,
            })) => {
                assert_eq!(id, "demo");
                assert_eq!(model.as_deref(), Some("gpt-5.4"));
            }
            _ => panic!("expected provider set-default command with model"),
        }
    }

    #[test]
    fn parses_provider_export_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "provider", "export", "demo"]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::Export {
                id,
                output,
            })) => {
                assert_eq!(id, "demo");
                assert_eq!(output, None);
            }
            _ => panic!("expected provider export command"),
        }
    }

    #[test]
    fn parses_provider_export_with_output_subcommand() {
        let cli = Cli::parse_from([
            "cc-switch",
            "provider",
            "export",
            "demo",
            "--output",
            "/tmp/provider-settings.json",
        ]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::Export {
                id,
                output,
            })) => {
                assert_eq!(id, "demo");
                assert_eq!(
                    output,
                    Some(std::path::PathBuf::from("/tmp/provider-settings.json"))
                );
            }
            _ => panic!("expected provider export command with output"),
        }
    }

    #[test]
    fn parses_config_webdav_show_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "config", "webdav", "show"]);

        match cli.command {
            Some(Commands::Config(super::commands::config::ConfigCommand::WebDav(
                super::commands::config_webdav::WebDavCommand::Show,
            ))) => {}
            _ => panic!("expected config webdav show command"),
        }
    }

    #[test]
    fn parses_config_webdav_set_subcommand() {
        let cli = Cli::parse_from([
            "cc-switch",
            "config",
            "webdav",
            "set",
            "--base-url",
            "https://dav.example.com/root",
            "--username",
            "demo",
            "--password",
            "secret",
            "--enable",
        ]);

        match cli.command {
            Some(Commands::Config(super::commands::config::ConfigCommand::WebDav(
                super::commands::config_webdav::WebDavCommand::Set {
                    base_url,
                    username,
                    password,
                    enable,
                    ..
                },
            ))) => {
                assert_eq!(base_url.as_deref(), Some("https://dav.example.com/root"));
                assert_eq!(username.as_deref(), Some("demo"));
                assert_eq!(password.as_deref(), Some("secret"));
                assert!(enable);
            }
            _ => panic!("expected config webdav set command"),
        }
    }

    #[test]
    fn parses_config_webdav_check_connection_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "config", "webdav", "check-connection"]);

        match cli.command {
            Some(Commands::Config(super::commands::config::ConfigCommand::WebDav(
                super::commands::config_webdav::WebDavCommand::CheckConnection,
            ))) => {}
            _ => panic!("expected config webdav check-connection command"),
        }
    }

    #[test]
    fn parses_config_openclaw_env_put_subcommand() {
        let cli = Cli::parse_from([
            "cc-switch",
            "config",
            "openclaw",
            "env",
            "put",
            "OPENCLAW_DEBUG",
            "true",
        ]);

        match cli.command {
            Some(Commands::Config(super::commands::config::ConfigCommand::OpenClaw(
                super::commands::config_openclaw::OpenClawCommand::Env(
                    super::commands::config_openclaw::OpenClawEnvCommand::Put { key, value },
                ),
            ))) => {
                assert_eq!(key, "OPENCLAW_DEBUG");
                assert_eq!(value, "true");
            }
            _ => panic!("expected config openclaw env put command"),
        }
    }

    #[test]
    fn parses_config_openclaw_tools_allow_add_subcommand() {
        let cli = Cli::parse_from([
            "cc-switch",
            "config",
            "openclaw",
            "tools",
            "allow",
            "add",
            "Read",
        ]);

        match cli.command {
            Some(Commands::Config(super::commands::config::ConfigCommand::OpenClaw(
                super::commands::config_openclaw::OpenClawCommand::Tools(
                    super::commands::config_openclaw::OpenClawToolsCommand::Allow(
                        super::commands::config_openclaw::OpenClawRuleListCommand::Add { rule },
                    ),
                ),
            ))) => {
                assert_eq!(rule, "Read");
            }
            _ => panic!("expected config openclaw tools allow add command"),
        }
    }

    #[test]
    fn parses_config_openclaw_tools_allow_set_at_subcommand() {
        let cli = Cli::parse_from([
            "cc-switch",
            "config",
            "openclaw",
            "tools",
            "allow",
            "set-at",
            "2",
            "Edit",
        ]);

        match cli.command {
            Some(Commands::Config(super::commands::config::ConfigCommand::OpenClaw(
                super::commands::config_openclaw::OpenClawCommand::Tools(
                    super::commands::config_openclaw::OpenClawToolsCommand::Allow(
                        super::commands::config_openclaw::OpenClawRuleListCommand::SetAt {
                            index,
                            rule,
                        },
                    ),
                ),
            ))) => {
                assert_eq!(index, 2);
                assert_eq!(rule, "Edit");
            }
            _ => panic!("expected config openclaw tools allow set-at command"),
        }
    }

    #[test]
    fn parses_config_openclaw_agents_fallback_remove_at_subcommand() {
        let cli = Cli::parse_from([
            "cc-switch",
            "config",
            "openclaw",
            "agents",
            "fallback",
            "remove-at",
            "3",
        ]);

        match cli.command {
            Some(Commands::Config(super::commands::config::ConfigCommand::OpenClaw(
                super::commands::config_openclaw::OpenClawCommand::Agents(
                    super::commands::config_openclaw::OpenClawAgentsCommand::Fallback(
                        super::commands::config_openclaw::OpenClawFallbackCommand::RemoveAt {
                            index,
                        },
                    ),
                ),
            ))) => {
                assert_eq!(index, 3);
            }
            _ => panic!("expected config openclaw agents fallback remove-at command"),
        }
    }

    #[test]
    fn parses_config_openclaw_agents_runtime_set_subcommand() {
        let cli = Cli::parse_from([
            "cc-switch",
            "config",
            "openclaw",
            "agents",
            "runtime",
            "set",
            "timeout-seconds",
            "120",
        ]);

        match cli.command {
            Some(Commands::Config(super::commands::config::ConfigCommand::OpenClaw(
                super::commands::config_openclaw::OpenClawCommand::Agents(
                    super::commands::config_openclaw::OpenClawAgentsCommand::Runtime(
                        super::commands::config_openclaw::OpenClawAgentsRuntimeCommand::Set {
                            field,
                            value,
                        },
                    ),
                ),
            ))) => {
                assert!(matches!(
                    field,
                    super::commands::config_openclaw::OpenClawRuntimeField::TimeoutSeconds
                ));
                assert_eq!(value, "120");
            }
            _ => panic!("expected config openclaw agents runtime set command"),
        }
    }

    #[test]
    fn parses_config_openclaw_memory_delete_yes_subcommand() {
        let cli = Cli::parse_from([
            "cc-switch",
            "config",
            "openclaw",
            "memory",
            "delete",
            "2026-06-01.md",
            "--yes",
        ]);

        match cli.command {
            Some(Commands::Config(super::commands::config::ConfigCommand::OpenClaw(
                super::commands::config_openclaw::OpenClawCommand::Memory(
                    super::commands::config_openclaw::OpenClawMemoryCommand::Delete {
                        filename,
                        yes,
                    },
                ),
            ))) => {
                assert_eq!(filename, "2026-06-01.md");
                assert!(yes);
            }
            _ => panic!("expected config openclaw memory delete command"),
        }
    }

    #[test]
    fn config_common_set_help_describes_snippet_as_primary_contract() {
        let mut cmd = Cli::command();
        let config = cmd
            .find_subcommand_mut("config")
            .expect("config subcommand should exist");
        let common = config
            .find_subcommand_mut("common")
            .expect("common subcommand should exist");
        let set = common
            .find_subcommand_mut("set")
            .expect("set subcommand should exist");
        let help = set.render_long_help().to_string();

        assert!(help.contains("--snippet <SNIPPET>"));
        assert!(help.contains("Inline snippet text"));
        assert!(!help.contains("Compatibility flag for inline snippet text"));
        assert!(help.contains("Compatibility:"));
        assert!(help.contains("--json <SNIPPET>"));
        assert!(help.contains("Legacy alias for --snippet <SNIPPET>"));
        assert!(help.contains("Claude/Gemini"));
        assert!(help.contains("OpenCode"));
        assert!(help.contains("Codex"));
        assert!(!help.contains("Apply to current provider immediately"));
        assert!(help.contains("live config"));
        assert!(help.contains("applicable"));
    }

    #[test]
    fn parses_config_common_set_legacy_json_alias() {
        let cli = Cli::parse_from(["cc-switch", "config", "common", "set", "--json", "{}"]);

        match cli.command {
            Some(Commands::Config(super::commands::config::ConfigCommand::Common(_))) => {}
            _ => panic!("expected config common set command"),
        }
    }

    #[test]
    fn parses_config_common_format_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "config", "common", "format", "--snippet", "{}"]);

        match cli.command {
            Some(Commands::Config(super::commands::config::ConfigCommand::Common(_))) => {}
            _ => panic!("expected config common format command"),
        }
    }

    #[test]
    fn parses_config_common_extract_subcommand() {
        let cli = Cli::parse_from([
            "cc-switch",
            "config",
            "common",
            "extract",
            "--provider",
            "p1",
            "--save",
        ]);

        match cli.command {
            Some(Commands::Config(super::commands::config::ConfigCommand::Common(_))) => {}
            _ => panic!("expected config common extract command"),
        }
    }

    #[test]
    fn config_common_clear_help_marks_apply_as_compatibility_flag() {
        let mut cmd = Cli::command();
        let config = cmd
            .find_subcommand_mut("config")
            .expect("config subcommand should exist");
        let common = config
            .find_subcommand_mut("common")
            .expect("common subcommand should exist");
        let clear = common
            .find_subcommand_mut("clear")
            .expect("clear subcommand should exist");
        let help = clear.render_long_help().to_string();

        assert!(!help.contains("Apply to current provider immediately"));
        assert!(help.contains("Compatibility flag"));
        assert!(help.contains("live config"));
        assert!(help.contains("applicable"));
    }

    #[test]
    fn parses_env_tools_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "env", "tools"]);

        match cli.command {
            Some(Commands::Env(super::commands::env::EnvCommand::Tools)) => {}
            _ => panic!("expected env tools command"),
        }
    }

    #[test]
    fn parses_mcp_enable_with_apps() {
        let cli = Cli::parse_from(["cc-switch", "mcp", "enable", "s1", "--apps", "claude,codex"]);

        match cli.command {
            Some(Commands::Mcp(super::commands::mcp::McpCommand::Enable { id, apps })) => {
                assert_eq!(id, "s1");
                assert_eq!(apps, vec!["claude", "codex"]);
            }
            _ => panic!("expected mcp enable command"),
        }
    }

    #[test]
    fn parses_mcp_set_apps_repeated_flags() {
        let cli = Cli::parse_from([
            "cc-switch",
            "mcp",
            "set-apps",
            "s1",
            "--apps",
            "opencode",
            "--apps",
            "hermes",
        ]);

        match cli.command {
            Some(Commands::Mcp(super::commands::mcp::McpCommand::SetApps { id, apps })) => {
                assert_eq!(id, "s1");
                assert_eq!(apps, vec!["opencode", "hermes"]);
            }
            _ => panic!("expected mcp set-apps command"),
        }
    }

    #[test]
    fn parses_skills_enable_with_apps() {
        let cli = Cli::parse_from(["cc-switch", "skills", "enable", "hello", "--apps", "codex"]);

        match cli.command {
            Some(Commands::Skills(super::commands::skills::SkillsCommand::Enable {
                spec,
                apps,
            })) => {
                assert_eq!(spec, "hello");
                assert_eq!(apps, vec!["codex"]);
            }
            _ => panic!("expected skills enable command"),
        }
    }

    #[test]
    fn parses_skills_set_apps_repeated_flags() {
        let cli = Cli::parse_from([
            "cc-switch",
            "skills",
            "set-apps",
            "hello",
            "--apps",
            "claude",
            "--apps",
            "hermes",
        ]);

        match cli.command {
            Some(Commands::Skills(super::commands::skills::SkillsCommand::SetApps {
                spec,
                apps,
            })) => {
                assert_eq!(spec, "hello");
                assert_eq!(apps, vec!["claude", "hermes"]);
            }
            _ => panic!("expected skills set-apps command"),
        }
    }

    #[test]
    fn parses_skills_import_from_apps_apps_before_directory() {
        let cli = Cli::parse_from([
            "cc-switch",
            "skills",
            "import-from-apps",
            "--apps",
            "claude,codex",
            "hello-skill",
        ]);

        match cli.command {
            Some(Commands::Skills(super::commands::skills::SkillsCommand::ImportFromApps {
                apps,
                directories,
            })) => {
                assert_eq!(apps, vec!["claude", "codex"]);
                assert_eq!(directories, vec!["hello-skill"]);
            }
            _ => panic!("expected skills import-from-apps command"),
        }
    }

    #[test]
    fn parses_skills_repo_enable_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "skills", "repos", "enable", "foo/bar"]);

        match cli.command {
            Some(Commands::Skills(super::commands::skills::SkillsCommand::Repos(
                super::commands::skills::SkillReposCommand::Enable { url },
            ))) => {
                assert_eq!(url, "foo/bar");
            }
            _ => panic!("expected skills repos enable command"),
        }
    }

    #[test]
    fn parses_skills_repo_disable_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "skills", "repos", "disable", "foo/bar"]);

        match cli.command {
            Some(Commands::Skills(super::commands::skills::SkillsCommand::Repos(
                super::commands::skills::SkillReposCommand::Disable { url },
            ))) => {
                assert_eq!(url, "foo/bar");
            }
            _ => panic!("expected skills repos disable command"),
        }
    }

    #[test]
    fn parses_completions_bash_generator_path() {
        let cli = Cli::parse_from(["cc-switch", "completions", "bash"]);

        match cli.command {
            Some(Commands::Completions(command)) => {
                assert_eq!(command.shell, Some(clap_complete::Shell::Bash));
                assert!(command.action.is_none());
            }
            _ => panic!("expected completions generator command"),
        }
    }

    #[test]
    fn parses_completions_zsh_generator_path() {
        let cli = Cli::parse_from(["cc-switch", "completions", "zsh"]);

        match cli.command {
            Some(Commands::Completions(command)) => {
                assert_eq!(command.shell, Some(clap_complete::Shell::Zsh));
                assert!(command.action.is_none());
            }
            _ => panic!("expected completions generator command"),
        }
    }

    #[test]
    fn parses_completions_install() {
        let cli = Cli::parse_from(["cc-switch", "completions", "install"]);

        match cli.command {
            Some(Commands::Completions(command)) => match command.action {
                Some(CompletionsAction::Install(args)) => {
                    assert_eq!(args.shell, ManagedShellSelection::Auto);
                    assert!(!args.activate);
                }
                _ => panic!("expected completions install subcommand"),
            },
            _ => panic!("expected completions install command"),
        }
    }

    #[test]
    fn parses_completions_install_with_shell_and_activate() {
        let cli = Cli::parse_from([
            "cc-switch",
            "completions",
            "install",
            "--shell",
            "zsh",
            "--activate",
        ]);

        match cli.command {
            Some(Commands::Completions(command)) => match command.action {
                Some(CompletionsAction::Install(args)) => {
                    assert_eq!(args.shell, ManagedShellSelection::Zsh);
                    assert!(args.activate);
                }
                _ => panic!("expected completions install subcommand"),
            },
            _ => panic!("expected completions install command"),
        }
    }

    #[test]
    fn parses_completions_status() {
        let cli = Cli::parse_from(["cc-switch", "completions", "status"]);

        match cli.command {
            Some(Commands::Completions(command)) => match command.action {
                Some(CompletionsAction::Status(CompletionLifecycleCommand { shell })) => {
                    assert_eq!(shell, ManagedShellSelection::Auto);
                }
                _ => panic!("expected completions status subcommand"),
            },
            _ => panic!("expected completions status command"),
        }
    }

    #[test]
    fn parses_completions_uninstall_with_explicit_shell() {
        let cli = Cli::parse_from(["cc-switch", "completions", "uninstall", "--shell", "bash"]);

        match cli.command {
            Some(Commands::Completions(command)) => match command.action {
                Some(CompletionsAction::Uninstall(CompletionLifecycleCommand { shell })) => {
                    assert_eq!(shell, ManagedShellSelection::Bash);
                }
                _ => panic!("expected completions uninstall subcommand"),
            },
            _ => panic!("expected completions uninstall command"),
        }
    }

    #[test]
    fn rejects_completions_generator_with_activate_flag() {
        let err = match Cli::try_parse_from(["cc-switch", "completions", "bash", "--activate"]) {
            Ok(_) => panic!("generator path should reject lifecycle-only flags"),
            Err(err) => err,
        };
        let rendered = err.to_string();

        assert!(rendered.contains("--activate"));
        assert!(rendered.contains("unexpected argument"));
    }
}
