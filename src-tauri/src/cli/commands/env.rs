use crate::app_config::AppType;
use crate::cli::ui::{create_table, error, highlight, info, success};
use crate::database::Database;
use crate::error::AppError;
use crate::services::env_checker;
use crate::services::local_env_check::{check_local_environment, ToolCheckStatus};
use clap::Subcommand;
use serde_json::Value;
use std::path::Path;

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
    } else {
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
            let value_display = truncate_value(&conflict.var_value, 30);

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
    }

    run_app_doctor(&app_type)?;

    Ok(())
}

fn run_app_doctor(app_type: &AppType) -> Result<(), AppError> {
    println!("\n{}", highlight("Configuration Doctor"));
    println!("{}", "─".repeat(60));

    match app_type {
        AppType::Claude => check_claude_doctor(),
        AppType::Codex => check_codex_doctor(),
        AppType::Gemini => check_gemini_doctor(),
        AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => {
            println!(
                "{}",
                info(&format!(
                    "No app-specific doctor checks are implemented for {} yet.",
                    app_type.as_str()
                ))
            );
            Ok(())
        }
    }
}

fn check_claude_doctor() -> Result<(), AppError> {
    let db = Database::open_readonly_current_schema()?;
    let current = crate::settings::get_effective_current_provider(&db, &AppType::Claude)?;
    let live_path = crate::config::get_claude_settings_path();
    let mcp_path = crate::config::get_claude_mcp_path();

    let mut rows = Vec::new();
    rows.push(check_file_exists("Claude settings.json", &live_path));
    rows.push(check_claude_onboarding(&mcp_path));

    match current {
        Some(provider_id) => {
            rows.push(ok_row("Current provider", provider_id.clone()));
            match db.get_provider_by_id(&provider_id, AppType::Claude.as_str())? {
                Some(provider) => {
                    rows.extend(check_claude_provider_settings(&provider.settings_config));
                }
                None => rows.push(warn_row(
                    "Provider record",
                    format!("current provider '{provider_id}' is missing from database"),
                )),
            }
        }
        None => rows.push(warn_row(
            "Current provider",
            "no current Claude provider selected".to_string(),
        )),
    }

    print_doctor_rows(rows);
    Ok(())
}

fn check_codex_doctor() -> Result<(), AppError> {
    let db = Database::open_readonly_current_schema()?;
    let current = crate::settings::get_effective_current_provider(&db, &AppType::Codex)?;
    let auth_path = crate::codex_config::get_codex_auth_path();
    let config_path = crate::codex_config::get_codex_config_path();

    let mut rows = vec![
        check_file_exists("Codex auth.json", &auth_path),
        check_file_exists("Codex config.toml", &config_path),
    ];

    match current {
        Some(provider_id) => rows.push(ok_row("Current provider", provider_id)),
        None => rows.push(warn_row(
            "Current provider",
            "no current Codex provider selected".to_string(),
        )),
    }

    print_doctor_rows(rows);
    Ok(())
}

fn check_gemini_doctor() -> Result<(), AppError> {
    let db = Database::open_readonly_current_schema()?;
    let current = crate::settings::get_effective_current_provider(&db, &AppType::Gemini)?;
    let env_path = crate::gemini_config::get_gemini_env_path();
    let settings_path = crate::gemini_config::get_gemini_settings_path();

    let mut rows = vec![
        check_file_exists("Gemini .env", &env_path),
        check_file_exists("Gemini settings.json", &settings_path),
    ];

    match current {
        Some(provider_id) => rows.push(ok_row("Current provider", provider_id)),
        None => rows.push(warn_row(
            "Current provider",
            "no current Gemini provider selected".to_string(),
        )),
    }

    print_doctor_rows(rows);
    Ok(())
}

fn check_file_exists(label: &'static str, path: &Path) -> DoctorRow {
    if path.exists() {
        ok_row(label, path.display().to_string())
    } else {
        warn_row(label, format!("missing: {}", path.display()))
    }
}

fn check_claude_onboarding(path: &Path) -> DoctorRow {
    if !path.exists() {
        return warn_row(
            "Claude onboarding",
            format!("{} is missing; first-run prompt may appear", path.display()),
        );
    }

    match crate::config::read_json_file::<Value>(path) {
        Ok(value) if value.get("hasCompletedOnboarding").and_then(Value::as_bool) == Some(true) => {
            ok_row(
                "Claude onboarding",
                "hasCompletedOnboarding=true".to_string(),
            )
        }
        Ok(_) => warn_row(
            "Claude onboarding",
            "hasCompletedOnboarding is not true; first-run prompt may appear".to_string(),
        ),
        Err(error) => warn_row(
            "Claude onboarding",
            format!("cannot read {}: {error}", path.display()),
        ),
    }
}

fn check_claude_provider_settings(settings: &Value) -> Vec<DoctorRow> {
    let mut rows = Vec::new();
    let env = settings.get("env").and_then(Value::as_object);
    let auth_token = env
        .and_then(|env| env.get("ANTHROPIC_AUTH_TOKEN"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let api_key = env
        .and_then(|env| env.get("ANTHROPIC_API_KEY"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let base_url = env
        .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
        .and_then(Value::as_str)
        .unwrap_or("");

    if auth_token.trim().is_empty() && api_key.trim().is_empty() {
        rows.push(warn_row(
            "Claude auth env",
            "ANTHROPIC_AUTH_TOKEN/ANTHROPIC_API_KEY are empty; Claude may ask to login".to_string(),
        ));
    } else if auth_token.trim().is_empty()
        && env.is_some_and(|env| env.contains_key("ANTHROPIC_AUTH_TOKEN"))
    {
        rows.push(warn_row(
            "Claude auth env",
            "ANTHROPIC_AUTH_TOKEN is present but empty; remove it or set a token".to_string(),
        ));
    } else {
        rows.push(ok_row(
            "Claude auth env",
            "token/key is configured".to_string(),
        ));
    }

    if base_url.trim().is_empty() {
        rows.push(warn_row(
            "Claude base URL",
            "ANTHROPIC_BASE_URL is empty; official Claude endpoint/auth will be used".to_string(),
        ));
    } else if is_probably_url(base_url) {
        rows.push(ok_row("Claude base URL", base_url.to_string()));
    } else {
        rows.push(warn_row(
            "Claude base URL",
            format!("ANTHROPIC_BASE_URL does not look like a URL: {base_url}"),
        ));
    }

    rows
}

#[derive(Debug)]
struct DoctorRow {
    status: &'static str,
    check: &'static str,
    detail: String,
}

fn ok_row(check: &'static str, detail: String) -> DoctorRow {
    DoctorRow {
        status: "ok",
        check,
        detail,
    }
}

fn warn_row(check: &'static str, detail: String) -> DoctorRow {
    DoctorRow {
        status: "warn",
        check,
        detail,
    }
}

fn print_doctor_rows(rows: Vec<DoctorRow>) {
    let mut table = create_table();
    table.set_header(vec!["Status", "Check", "Detail"]);
    for row in rows {
        table.add_row(vec![row.status, row.check, row.detail.as_str()]);
    }
    println!("{}", table);
}

fn is_probably_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

fn truncate_value(value: &str, max_len: usize) -> String {
    if value.chars().count() > max_len {
        let prefix: String = value.chars().take(max_len.saturating_sub(3)).collect();
        format!("{prefix}...")
    } else {
        value.to_string()
    }
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
        ToolCheckStatus::VersionUnavailable { reason } => {
            format!("installed; version unavailable ({reason})")
        }
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

    #[test]
    fn tool_status_summary_keeps_unavailable_version_distinct_from_missing_tool() {
        let summary = tool_status_summary(&ToolCheckStatus::VersionUnavailable {
            reason: "version check timed out after 10s".to_string(),
        });

        assert_eq!(
            summary,
            "installed; version unavailable (version check timed out after 10s)"
        );
    }

    #[test]
    fn truncate_value_handles_multibyte_text() {
        assert_eq!(super::truncate_value("中文中文中文", 5), "中文...");
    }
}
