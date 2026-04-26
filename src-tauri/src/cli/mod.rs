use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use std::io::Write;

mod claude_temp_launch;
mod codex_temp_launch;
pub mod commands;
pub mod editor;
pub mod i18n;
pub mod interactive;
pub mod orphan_scan;
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
    /// Manage providers (list, switch, export, speedtest, stream-check, fetch-models)
    #[command(subcommand)]
    Provider(commands::provider::ProviderCommand),

    /// Manage MCP servers (list, add, edit, delete, sync)
    #[command(subcommand)]
    Mcp(commands::mcp::McpCommand),

    /// Manage prompts (list, activate, edit)
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

    /// Start an app with a provider selector without switching the global current provider
    #[cfg(any(unix, windows))]
    #[command(subcommand)]
    Start(commands::start::StartCommand),

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

    #[cfg(any(unix, windows))]
    #[test]
    fn parses_start_claude_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "start", "claude", "demo"]);

        match cli.command {
            Some(Commands::Start(super::commands::start::StartCommand::Claude {
                selector,
                native_args,
            })) => {
                assert_eq!(selector, "demo");
                assert!(native_args.is_empty());
            }
            _ => panic!("expected start claude command"),
        }
    }

    #[cfg(any(unix, windows))]
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
                native_args,
            })) => {
                assert_eq!(selector, "demo");
                assert_eq!(
                    native_args,
                    vec![OsString::from("--dangerously-skip-permissions")]
                );
            }
            _ => panic!("expected start claude command with native args"),
        }
    }

    #[cfg(any(unix, windows))]
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

    #[cfg(any(unix, windows))]
    #[test]
    fn parses_start_codex_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "start", "codex", "demo"]);

        match cli.command {
            Some(Commands::Start(super::commands::start::StartCommand::Codex {
                selector,
                native_args,
            })) => {
                assert_eq!(selector, "demo");
                assert!(native_args.is_empty());
            }
            _ => panic!("expected start codex command"),
        }
    }

    #[cfg(any(unix, windows))]
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
                native_args,
            })) => {
                assert_eq!(selector, "demo");
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

    #[cfg(any(unix, windows))]
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

        assert!(help.contains("Native Claude CLI arguments to pass through after `--`"));
        assert!(help.contains("cc-switch start claude demo -- --dangerously-skip-permissions"));
    }

    #[cfg(any(unix, windows))]
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

        assert!(help.contains("Native Codex CLI arguments to pass through after `--`"));
        assert!(help.contains("cc-switch start codex demo -- --model gpt-5.4"));
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
    fn parses_provider_fetch_models_subcommand() {
        let cli = Cli::parse_from(["cc-switch", "provider", "fetch-models", "demo"]);

        match cli.command {
            Some(Commands::Provider(super::commands::provider::ProviderCommand::FetchModels {
                id,
            })) => {
                assert_eq!(id, "demo");
            }
            _ => panic!("expected provider fetch-models command"),
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
