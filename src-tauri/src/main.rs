use cc_switch_lib::cli::{Cli, Commands};
use cc_switch_lib::AppError;
use clap::Parser;
use std::process;

fn main() {
    // 解析命令行参数
    let cli = Cli::parse();

    // 初始化日志（交互模式和命令行模式都避免干扰输出）
    let log_level = if cli.verbose {
        "debug"
    } else {
        "error" // 默认只显示错误日志，避免 INFO 日志干扰命令输出
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level)).init();

    // Best-effort cleanup of orphaned temp credential files from previous sessions
    let _ = cc_switch_lib::cli::orphan_scan::scan_and_clean(&std::env::temp_dir());

    // 执行命令
    if let Err(e) = run(cli) {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), AppError> {
    initialize_startup_state_if_needed(&cli.command)?;

    match cli.command {
        // Default to interactive mode if no command is provided
        None | Some(Commands::Interactive) => cc_switch_lib::cli::interactive::run(cli.app),
        Some(Commands::Provider(cmd)) => {
            cc_switch_lib::cli::commands::provider::execute(cmd, cli.app)
        }
        Some(Commands::Mcp(cmd)) => cc_switch_lib::cli::commands::mcp::execute(cmd, cli.app),
        Some(Commands::Prompts(cmd)) => {
            cc_switch_lib::cli::commands::prompts::execute(cmd, cli.app)
        }
        Some(Commands::Skills(cmd)) => cc_switch_lib::cli::commands::skills::execute(cmd, cli.app),
        Some(Commands::Config(cmd)) => cc_switch_lib::cli::commands::config::execute(cmd, cli.app),
        Some(Commands::Proxy(cmd)) => cc_switch_lib::cli::commands::proxy::execute(cmd),
        #[cfg(any(unix, windows))]
        Some(Commands::Start(cmd)) => cc_switch_lib::cli::commands::start::execute(cmd),
        Some(Commands::Env(cmd)) => cc_switch_lib::cli::commands::env::execute(cmd, cli.app),
        Some(Commands::Update(cmd)) => cc_switch_lib::cli::commands::update::execute(cmd),
        Some(Commands::Completions(cmd)) => cc_switch_lib::cli::commands::completions::execute(cmd),
    }
}

fn command_requires_startup_state(command: &Option<Commands>) -> bool {
    match command {
        Some(Commands::Completions(_)) | Some(Commands::Update(_)) => false,
        _ => true,
    }
}

fn initialize_startup_state_if_needed(command: &Option<Commands>) -> Result<(), AppError> {
    if command_requires_startup_state(command) {
        let _state = cc_switch_lib::AppState::try_new_with_startup_recovery()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{command_requires_startup_state, initialize_startup_state_if_needed};
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
    fn update_and_completions_skip_startup_state() {
        let update = Cli::parse_from(["cc-switch", "update"]);
        let completions_generate = Cli::parse_from(["cc-switch", "completions", "bash"]);
        let completions_install = Cli::parse_from(["cc-switch", "completions", "install"]);
        let completions_status = Cli::parse_from(["cc-switch", "completions", "status"]);
        let completions_uninstall =
            Cli::parse_from(["cc-switch", "completions", "uninstall", "--shell", "bash"]);
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
        assert!(command_requires_startup_state(&provider.command));
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
    fn provider_commands_still_fail_on_future_schema_database() {
        let temp = tempfile::tempdir().expect("create temp dir");
        seed_future_schema_database(temp.path());
        let _guard = ConfigDirEnvGuard::set(temp.path());

        let cli = Cli::parse_from(["cc-switch", "provider", "list"]);
        let err = initialize_startup_state_if_needed(&cli.command)
            .expect_err("provider command should still require startup state");
        assert!(
            err.to_string().contains("数据库版本过新"),
            "unexpected error: {err}"
        );
    }
}
