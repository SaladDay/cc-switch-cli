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

    // 执行命令
    if let Err(e) = run(cli) {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), AppError> {
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
        Some(Commands::Env(cmd)) => cc_switch_lib::cli::commands::env::execute(cmd, cli.app),
        Some(Commands::Update(cmd)) => cc_switch_lib::cli::commands::update::execute(cmd),
        Some(Commands::Completions { shell }) => {
            cc_switch_lib::cli::generate_completions(shell);
            Ok(())
        }
    }
}
