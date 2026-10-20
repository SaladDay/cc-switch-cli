use cc_switch_lib::cli::{Cli, Commands};
use cc_switch_lib::AppError;
use clap::Parser;
use std::process;

fn main() {
    // 解析命令行参数
    let cli = Cli::parse();

    init_logger_if_needed(&cli);

    // 执行命令
    if let Err(e) = run(cli) {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}

fn init_logger_if_needed(cli: &Cli) {
    if command_uses_own_logger(&cli.command) {
        return;
    }

    // 初始化日志（交互模式和命令行模式都避免干扰输出）
    let log_level = if cli.verbose {
        "debug"
    } else {
        "error" // 默认只显示错误日志，避免 INFO 日志干扰命令输出
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level)).init();
}

fn command_uses_own_logger(command: &Option<Commands>) -> bool {
    match command {
        #[cfg(unix)]
        Some(Commands::Daemon(cc_switch_lib::cli::commands::daemon::DaemonCommand::Start {
            ..
        })) => true,
        _ => false,
    }
}

fn run(cli: Cli) -> Result<(), AppError> {
    if database_access_required(&cli.command) {
        // Reject dangerous config roots before any command can initialize state.
        cc_switch_lib::validate_config_dir()?;
    }
    initialize_startup_state_if_needed(&cli.command)?;

    match cli.command {
        // Default to interactive mode if no command is provided
        None | Some(Commands::Interactive) => cc_switch_lib::cli::interactive::run(cli.app),
        Some(Commands::Auth(cmd)) => cc_switch_lib::cli::commands::auth::execute(cmd),
        Some(Commands::Provider(cmd)) => {
            cc_switch_lib::cli::commands::provider::execute(cmd, cli.app)
        }
        Some(Commands::Use { id }) => cc_switch_lib::cli::commands::provider::execute(
            cc_switch_lib::cli::commands::provider::ProviderCommand::Switch { id },
            cli.app,
        ),
        Some(Commands::Mcp(cmd)) => cc_switch_lib::cli::commands::mcp::execute(cmd, cli.app),
        Some(Commands::Prompts(cmd)) => {
            cc_switch_lib::cli::commands::prompts::execute(cmd, cli.app)
        }
        Some(Commands::Skills(cmd)) => cc_switch_lib::cli::commands::skills::execute(cmd, cli.app),
        Some(Commands::Config(cmd)) => cc_switch_lib::cli::commands::config::execute(cmd, cli.app),
        Some(Commands::Proxy(cmd)) => cc_switch_lib::cli::commands::proxy::execute(cmd, cli.app),
        Some(Commands::Settings(cmd)) => cc_switch_lib::cli::commands::settings::execute(cmd),
        Some(Commands::Failover(cmd)) => {
            cc_switch_lib::cli::commands::failover::execute(cmd, cli.app)
        }
        Some(Commands::Sessions(cmd)) => {
            cc_switch_lib::cli::commands::sessions::execute(cmd, cli.app)
        }
        Some(Commands::Hermes(cmd)) => cc_switch_lib::cli::commands::hermes::execute(cmd),
        #[cfg(unix)]
        Some(Commands::Start(cmd)) => cc_switch_lib::cli::commands::start::execute(cmd),
        #[cfg(unix)]
        Some(Commands::Daemon(cmd)) => cc_switch_lib::cli::commands::daemon::execute(cmd),
        Some(Commands::Env(cmd)) => cc_switch_lib::cli::commands::env::execute(cmd, cli.app),
        Some(Commands::Deeplink(cmd)) => {
            cc_switch_lib::cli::commands::deeplink::execute(cmd, cli.app)
        }
        Some(Commands::Update(cmd)) => cc_switch_lib::cli::commands::update::execute(cmd),
        Some(Commands::Completions(cmd)) => cc_switch_lib::cli::commands::completions::execute(cmd),
        Some(Commands::Internal(cmd)) => cc_switch_lib::cli::commands::internal::execute(cmd),
    }
}

fn command_requires_startup_state(command: &Option<Commands>) -> bool {
    if matches!(
        command,
        Some(Commands::Skills(
            cc_switch_lib::cli::commands::skills::SkillsCommand::InstallLocal { .. }
                | cc_switch_lib::cli::commands::skills::SkillsCommand::LocalInfo { .. }
        ))
    ) {
        return false;
    }
    #[cfg(unix)]
    if std::env::var_os(cc_switch_lib::daemon::supervisor::DAEMON_SOCKET_ENV).is_some() {
        return false;
    }

    match command {
        Some(Commands::Completions(_))
        | Some(Commands::Auth(_))
        | Some(Commands::Update(_))
        | Some(Commands::Internal(_))
        | Some(Commands::Sessions(_))
        | Some(Commands::Skills(_))
        | Some(Commands::Config(cc_switch_lib::cli::commands::config::ConfigCommand::Backup {
            ..
        }))
        | Some(Commands::Settings(
            cc_switch_lib::cli::commands::settings::SettingsCommand::CodexAuthPreservation(_),
        )) => false,
        #[cfg(unix)]
        Some(Commands::Daemon(_)) => false,
        _ => true,
    }
}

fn initialize_startup_state_if_needed(command: &Option<Commands>) -> Result<(), AppError> {
    if matches!(
        command,
        Some(Commands::Skills(_))
            | Some(Commands::Config(
                cc_switch_lib::cli::commands::config::ConfigCommand::Backup { .. }
            ))
    ) {
        guard_legacy_first_start(command, &cc_switch_lib::get_app_config_dir())?;
    }
    use cc_switch_lib::cli::commands::skills::SkillsCommand;
    if matches!(
        command,
        Some(Commands::Skills(
            SkillsCommand::Discover { .. }
                | SkillsCommand::Market { .. }
                | SkillsCommand::Install { .. }
                | SkillsCommand::CheckUpdates
                | SkillsCommand::Update { .. }
        ))
    ) {
        // Retain the saved outbound proxy without startup migrations/recovery
        // or clearing an invalid setting in an otherwise read-only command.
        cc_switch_lib::initialize_http_client_from_disk_best_effort();
    }
    if command_requires_startup_state(command) {
        let _state = if command_uses_deferred_codex_migration(command) {
            // Mirrors upstream's non-blocking GUI startup for the long-lived TUI.
            cc_switch_lib::AppState::try_new_with_startup_recovery_deferred()?
        } else {
            // A one-shot CLI process may exit before a detached migration thread
            // completes, so finish the same upstream migration sequence inline.
            cc_switch_lib::AppState::try_new_with_startup_recovery()?
        };
    }
    Ok(())
}

fn guard_legacy_first_start(
    command: &Option<Commands>,
    config_dir: &std::path::Path,
) -> Result<(), AppError> {
    use cc_switch_lib::cli::commands::skills::SkillsCommand;

    // Only newly exempted commands that can initialize SQLite need this gate.
    // Keep upstream exemptions and commands that never create a database usable.
    let may_initialize_database = match command {
        Some(Commands::Skills(
            SkillsCommand::LocalInfo { .. }
            | SkillsCommand::Market { .. }
            | SkillsCommand::SyncMethod { .. }
            | SkillsCommand::StorageLocation { location: None },
        )) => false,
        Some(Commands::Skills(_))
        | Some(Commands::Config(cc_switch_lib::cli::commands::config::ConfigCommand::Backup {
            ..
        })) => true,
        _ => false,
    };
    if may_initialize_database
        && !config_dir.join("cc-switch.db").exists()
        && ["config.json", "skills.json"]
            .iter()
            .any(|name| config_dir.join(name).exists())
    {
        return Err(AppError::Config(
            "Legacy config.json or skills.json requires migration before this command; run cc-switch normally (without a subcommand) to migrate first."
                .to_string(),
        ));
    }
    Ok(())
}

fn command_uses_deferred_codex_migration(command: &Option<Commands>) -> bool {
    matches!(command, None | Some(Commands::Interactive))
}

fn database_access_required(command: &Option<Commands>) -> bool {
    !matches!(
        command,
        Some(Commands::Completions(_)) | Some(Commands::Update(_))
    )
}

#[cfg(test)]
mod tests {
    use super::{
        command_requires_startup_state, command_uses_deferred_codex_migration,
        command_uses_own_logger, database_access_required, guard_legacy_first_start,
        initialize_startup_state_if_needed,
    };
    use cc_switch_lib::cli::Cli;
    use clap::Parser;
    use serial_test::serial;
    use std::{env, ffi::OsString, path::Path};

    struct ConfigDirEnvGuard {
        original: Option<OsString>,
    }

    impl ConfigDirEnvGuard {
        fn set(path: &Path) -> Self {
            let original = env::var_os("CC_SWITCH_CONFIG_DIR");
            unsafe {
                env::set_var("CC_SWITCH_CONFIG_DIR", path);
            }
            Self { original }
        }
    }

    impl Drop for ConfigDirEnvGuard {
        fn drop(&mut self) {
            match self.original.as_ref() {
                Some(value) => unsafe { env::set_var("CC_SWITCH_CONFIG_DIR", value) },
                None => unsafe { env::remove_var("CC_SWITCH_CONFIG_DIR") },
            }
        }
    }

    fn seed_future_schema_database(config_dir: &Path) {
        std::fs::create_dir_all(config_dir).expect("create config dir");
        let db_path = config_dir.join("cc-switch.db");
        let conn = rusqlite::Connection::open(&db_path).expect("open sqlite db");
        conn.execute("PRAGMA user_version = 999;", [])
            .expect("set future schema version");
    }

    #[test]
    fn legacy_first_start_rejects_database_initializers_without_writes() {
        for legacy in ["config.json", "skills.json"] {
            let temp = tempfile::tempdir().expect("create temp dir");
            let path = temp.path().join(legacy);
            let original = b"{\"synthetic-legacy\":true}\n";
            std::fs::write(&path, original).expect("write legacy sentinel");
            for args in [
                vec!["cc-switch", "skills", "list"],
                vec!["cc-switch", "skills", "repos", "list"],
                vec![
                    "cc-switch",
                    "skills",
                    "install-local",
                    "--source-dir",
                    "/synthetic",
                    "--name",
                    "demo",
                    "--source-repo",
                    "owner/repo",
                    "--source-path",
                    "demo",
                    "--source-commit",
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                ],
                vec!["cc-switch", "skills", "install", "owner/repo:demo"],
                vec![
                    "cc-switch",
                    "skills",
                    "update",
                    "demo",
                    "--source-commit",
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                ],
                vec!["cc-switch", "skills", "storage-location", "unified"],
                vec!["cc-switch", "config", "backup"],
            ] {
                let cli = Cli::try_parse_from(args).expect("parse guarded command");
                let error = guard_legacy_first_start(&cli.command, temp.path())
                    .expect_err("legacy files must be migrated before database initialization");
                assert!(error.to_string().contains("run cc-switch normally"));
                assert_eq!(
                    std::fs::read(&path).expect("read legacy sentinel"),
                    original.as_slice()
                );
                assert_eq!(
                    std::fs::read_dir(temp.path())
                        .expect("list fixture")
                        .count(),
                    1
                );
                assert!(!temp.path().join("cc-switch.db").exists());
            }
        }
    }

    #[test]
    fn legacy_first_start_preserves_existing_exemptions_and_readonly_commands() {
        let temp = tempfile::tempdir().expect("create temp dir");
        std::fs::write(temp.path().join("config.json"), b"{}").expect("write legacy sentinel");
        for args in [
            vec![
                "cc-switch",
                "skills",
                "local-info",
                "--name",
                "demo",
                "--json",
            ],
            vec!["cc-switch", "skills", "market", "demo"],
            vec!["cc-switch", "skills", "sync-method"],
            vec!["cc-switch", "skills", "sync-method", "copy"],
            vec!["cc-switch", "skills", "storage-location"],
            vec!["cc-switch", "auth", "status"],
            vec!["cc-switch", "sessions", "list"],
            vec!["cc-switch", "update"],
            vec!["cc-switch", "completions", "bash"],
            vec![
                "cc-switch",
                "internal",
                "capture-codex-temp",
                "official",
                "/synthetic",
            ],
            vec!["cc-switch", "settings", "codex-auth-preservation", "show"],
            vec!["cc-switch", "provider", "list"],
            vec!["cc-switch"],
        ] {
            let cli = Cli::try_parse_from(args).expect("parse unguarded command");
            guard_legacy_first_start(&cli.command, temp.path()).expect("preserve command behavior");
        }
        assert_eq!(
            std::fs::read_dir(temp.path())
                .expect("list fixture")
                .count(),
            1
        );
    }

    #[test]
    fn legacy_first_start_allows_fresh_and_existing_databases() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let cli = Cli::parse_from(["cc-switch", "skills", "list"]);
        guard_legacy_first_start(&cli.command, temp.path()).expect("fresh installation allowed");
        seed_future_schema_database(temp.path());
        std::fs::write(temp.path().join("skills.json"), b"{}").expect("write legacy sentinel");
        // Existing/future databases retain their own schema handling downstream.
        guard_legacy_first_start(&cli.command, temp.path()).expect("existing database allowed");
    }

    #[test]
    #[serial]
    fn legacy_first_start_entrypoint_refuses_writers_and_allows_local_info() {
        for legacy in ["config.json", "skills.json"] {
            let temp = tempfile::tempdir().expect("create temp dir");
            std::fs::write(temp.path().join(legacy), b"{}").expect("write legacy sentinel");
            let _guard = ConfigDirEnvGuard::set(temp.path());
            let writer = Cli::parse_from(["cc-switch", "skills", "list"]);
            initialize_startup_state_if_needed(&writer.command).expect_err("reject legacy writer");
            let reader = Cli::parse_from([
                "cc-switch",
                "skills",
                "local-info",
                "--name",
                "demo",
                "--json",
            ]);
            initialize_startup_state_if_needed(&reader.command).expect("allow readonly info");
            assert_eq!(
                std::fs::read_dir(temp.path())
                    .expect("list fixture")
                    .count(),
                1
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn daemon_start_uses_daemon_file_logger() {
        let cli = Cli::parse_from(["cc-switch", "daemon", "start"]);

        assert!(command_uses_own_logger(&cli.command));
    }

    #[test]
    fn normal_commands_use_env_logger() {
        let cli = Cli::parse_from(["cc-switch", "provider", "list"]);

        assert!(!command_uses_own_logger(&cli.command));
    }

    #[test]
    fn only_long_lived_interactive_mode_defers_codex_migration() {
        let default_interactive = Cli::parse_from(["cc-switch"]);
        let explicit_interactive = Cli::parse_from(["cc-switch", "interactive"]);
        let provider = Cli::parse_from(["cc-switch", "provider", "list"]);

        assert!(command_uses_deferred_codex_migration(
            &default_interactive.command
        ));
        assert!(command_uses_deferred_codex_migration(
            &explicit_interactive.command
        ));
        assert!(!command_uses_deferred_codex_migration(&provider.command));
    }

    #[test]
    fn update_and_completions_skip_startup_state() {
        let update = Cli::parse_from(["cc-switch", "update"]);
        let completions_generate = Cli::parse_from(["cc-switch", "completions", "bash"]);
        let completions_install = Cli::parse_from(["cc-switch", "completions", "install"]);
        let completions_status = Cli::parse_from(["cc-switch", "completions", "status"]);
        let completions_uninstall =
            Cli::parse_from(["cc-switch", "completions", "uninstall", "--shell", "bash"]);
        let internal_capture = Cli::parse_from([
            "cc-switch",
            "internal",
            "capture-codex-temp",
            "official",
            "/tmp/codex-home",
        ]);
        let sessions = Cli::parse_from(["cc-switch", "sessions", "list"]);
        let auth = Cli::parse_from(["cc-switch", "auth", "status"]);
        let codex_auth_preservation =
            Cli::parse_from(["cc-switch", "settings", "codex-auth-preservation", "show"]);
        let provider = Cli::parse_from(["cc-switch", "provider", "list"]);

        assert!(!command_requires_startup_state(&update.command));
        assert!(!command_requires_startup_state(
            &completions_generate.command
        ));
        assert!(!command_requires_startup_state(
            &completions_install.command
        ));
        assert!(!command_requires_startup_state(&completions_status.command));
        assert!(!command_requires_startup_state(
            &completions_uninstall.command
        ));
        assert!(!command_requires_startup_state(&internal_capture.command));
        assert!(!command_requires_startup_state(&sessions.command));
        assert!(!command_requires_startup_state(&auth.command));
        assert!(!command_requires_startup_state(
            &codex_auth_preservation.command
        ));
        assert!(command_requires_startup_state(&provider.command));
    }

    #[test]
    fn completions_update_skip_database_access() {
        let update = Cli::parse_from(["cc-switch", "update"]);
        let completions = Cli::parse_from(["cc-switch", "completions", "bash"]);

        assert!(!database_access_required(&update.command));
        assert!(!database_access_required(&completions.command));
    }

    #[test]
    fn normal_commands_require_database_access() {
        let provider = Cli::parse_from(["cc-switch", "provider", "list"]);
        let mcp = Cli::parse_from(["cc-switch", "mcp", "list"]);
        let config = Cli::parse_from(["cc-switch", "config", "validate"]);
        let proxy = Cli::parse_from(["cc-switch", "proxy", "show"]);
        let interactive = Cli::parse_from(["cc-switch"]);
        let internal = Cli::parse_from([
            "cc-switch",
            "internal",
            "capture-codex-temp",
            "official",
            "/tmp/codex-home",
        ]);

        assert!(database_access_required(&provider.command));
        assert!(database_access_required(&mcp.command));
        assert!(database_access_required(&config.command));
        assert!(database_access_required(&proxy.command));
        assert!(database_access_required(&interactive.command));
        assert!(database_access_required(&internal.command));
    }

    #[test]
    #[serial]
    fn update_bypasses_future_schema_database_gate() {
        let temp = tempfile::tempdir().expect("create temp dir");
        seed_future_schema_database(temp.path());
        let _guard = ConfigDirEnvGuard::set(temp.path());

        let cli = Cli::parse_from(["cc-switch", "update"]);
        initialize_startup_state_if_needed(&cli.command)
            .expect("update should not touch startup state");
    }

    #[test]
    #[serial]
    fn internal_commands_bypass_future_schema_database_gate() {
        let temp = tempfile::tempdir().expect("create temp dir");
        seed_future_schema_database(temp.path());
        let _guard = ConfigDirEnvGuard::set(temp.path());

        let cli = Cli::parse_from([
            "cc-switch",
            "internal",
            "capture-codex-temp",
            "official",
            "/tmp/codex-home",
        ]);
        initialize_startup_state_if_needed(&cli.command)
            .expect("internal commands should not touch startup state");
    }

    #[test]
    #[serial]
    fn codex_auth_preservation_bypasses_future_schema_database_gate() {
        let temp = tempfile::tempdir().expect("create temp dir");
        seed_future_schema_database(temp.path());
        let _guard = ConfigDirEnvGuard::set(temp.path());

        let cli = Cli::parse_from(["cc-switch", "settings", "codex-auth-preservation", "show"]);
        initialize_startup_state_if_needed(&cli.command)
            .expect("Codex auth preservation should not touch startup state");
    }

    #[test]
    #[serial]
    fn provider_commands_still_fail_on_future_schema_database() {
        let temp = tempfile::tempdir().expect("create temp dir");
        seed_future_schema_database(temp.path());
        let _guard = ConfigDirEnvGuard::set(temp.path());

        let cli = Cli::parse_from(["cc-switch", "provider", "list"]);
        let err = initialize_startup_state_if_needed(&cli.command)
            .expect_err("provider command should still require startup state");
        assert!(
            err.to_string().contains("由较新版本的 CC Switch 创建"),
            "unexpected error: {err}"
        );
    }
}
