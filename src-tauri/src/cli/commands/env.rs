use crate::app_config::AppType;
use crate::cli::ui::{create_table, error, highlight, info, success};
use crate::error::AppError;
use crate::services::env_checker;
use crate::services::local_env_check::{check_local_environment, ToolCheckStatus};
use clap::Subcommand;

#[derive(Subcommand)]
pub enum EnvCommand {
    /// Check for environment variable conflicts
    Check,
    /// List all relevant environment variables
    List,
    /// Check whether supported app CLIs are installed locally
    Tools,
}

pub fn execute(cmd: EnvCommand, app: Option<AppType>) -> Result<(), AppError> {
    let app_type = app.unwrap_or(AppType::Claude);

    match cmd {
        EnvCommand::Check => check_conflicts(app_type),
        EnvCommand::List => list_env_vars(app_type),
        EnvCommand::Tools => check_local_tools(),
    }
}

fn check_conflicts(app_type: AppType) -> Result<(), AppError> {
    let app_str = app_type.as_str();

    println!(
        "\n{}",
        highlight(&format!("Checking Environment Variables for {}", app_str))
    );
    println!("{}", "═".repeat(60));

    // 检测冲突
    let conflicts = env_checker::check_env_conflicts(app_str)
        .map_err(|e| AppError::Message(format!("Failed to check environment variables: {}", e)))?;

    if conflicts.is_empty() {
        println!(
            "\n{}",
            success("✓ No environment variable conflicts detected")
        );
        println!(
            "{}",
            info(&format!(
                "Your {} configuration should work correctly.",
                app_str
            ))
        );
        return Ok(());
    }

    // 显示冲突
    println!(
        "\n{}",
        error(&format!(
            "⚠ Found {} environment variable(s) that may conflict:",
            conflicts.len()
        ))
    );
    println!();

    let mut table = create_table();
    table.set_header(vec!["Variable", "Value", "Source Type", "Source Location"]);

    for conflict in &conflicts {
        // 截断过长的值
        let value_display = if conflict.var_value.len() > 30 {
            format!("{}...", &conflict.var_value[..27])
        } else {
            conflict.var_value.clone()
        };

        table.add_row(vec![
            conflict.var_name.as_str(),
            &value_display,
            conflict.source_type.as_str(),
            conflict.source_path.as_str(),
        ]);
    }

    println!("{}", table);
    println!();
    println!(
        "{}",
        info("These environment variables may override CC-Switch's configuration.")
    );
    println!(
        "{}",
        info("Please manually remove them from your shell config files or system settings.")
    );

    Ok(())
}

fn list_env_vars(app_type: AppType) -> Result<(), AppError> {
    let app_str = app_type.as_str();

    println!(
        "\n{}",
        highlight(&format!("Environment Variables for {}", app_str))
    );
    println!("{}", "═".repeat(60));

    // 获取所有相关环境变量
    let conflicts = env_checker::check_env_conflicts(app_str)
        .map_err(|e| AppError::Message(format!("Failed to list environment variables: {}", e)))?;

    if conflicts.is_empty() {
        println!("\n{}", info("No related environment variables found."));
        return Ok(());
    }

    println!("\n{} environment variable(s) found:\n", conflicts.len());

    let mut table = create_table();
    table.set_header(vec!["Variable", "Value", "Source Type", "Source Location"]);

    for conflict in &conflicts {
        table.add_row(vec![
            conflict.var_name.as_str(),
            conflict.var_value.as_str(),
            conflict.source_type.as_str(),
            conflict.source_path.as_str(),
        ]);
    }

    println!("{}", table);

    Ok(())
}

fn check_local_tools() -> Result<(), AppError> {
    let results = check_local_environment();

    println!("\n{}", highlight("Local CLI Tools"));
    println!("{}", "═".repeat(60));

    let mut table = create_table();
    table.set_header(vec!["Tool", "Status"]);
    for result in results {
        table.add_row(vec![
            result.display_name.to_string(),
            tool_status_summary(&result.status),
        ]);
    }

    println!("{}", table);

    Ok(())
}

fn tool_status_summary(status: &ToolCheckStatus) -> String {
    match status {
        ToolCheckStatus::Ok { version } => format!("ok ({version})"),
        ToolCheckStatus::NotInstalledOrNotExecutable => "not installed".to_string(),
        ToolCheckStatus::Error { message } => format!("error ({message})"),
    }
}

#[cfg(test)]
mod tests {
    use super::tool_status_summary;
    use crate::services::local_env_check::ToolCheckStatus;

    #[test]
    fn tool_status_summary_formats_ok_version() {
        let summary = tool_status_summary(&ToolCheckStatus::Ok {
            version: "1.2.3".to_string(),
        });

        assert_eq!(summary, "ok (1.2.3)");
    }

    #[test]
    fn tool_status_summary_formats_missing_tool() {
        let summary = tool_status_summary(&ToolCheckStatus::NotInstalledOrNotExecutable);

        assert_eq!(summary, "not installed");
    }
}
