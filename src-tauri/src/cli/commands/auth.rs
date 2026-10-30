use clap::Subcommand;
use serde::Serialize;
use std::time::{Duration, Instant};

use crate::cli::ui::{create_table, highlight, info, success, to_json, warning};
use crate::error::AppError;
use crate::services::{AuthService, ManagedAuthAccount, ManagedAuthDeviceCodeResponse};

const AUTH_PROVIDER_CODEX_OAUTH: &str = "codex_oauth";
const AUTH_PROVIDER_XAI_OAUTH: &str = "xai_oauth";

#[derive(Subcommand, Debug, Clone)]
pub enum AuthCommand {
    /// Show ChatGPT Codex OAuth authentication status
    Status {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// List signed-in ChatGPT accounts
    List {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Sign in to ChatGPT with the Codex OAuth device flow
    Login {
        /// Reauthenticate this local account while preserving provider bindings
        #[arg(long)]
        account_id: Option<String>,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Set the default ChatGPT account
    Default {
        /// Account id to make default
        account_id: String,
    },
    /// Remove a ChatGPT account
    Remove {
        /// Account id to remove
        account_id: String,
        /// Confirm removal without prompting
        #[arg(long)]
        yes: bool,
    },
    /// Remove all ChatGPT Codex OAuth authentication data
    Logout {
        /// Confirm logout without prompting
        #[arg(long)]
        yes: bool,
    },
    /// Reset Codex rate limit quota using a banked reset credit
    ResetQuota {
        /// Account ID (defaults to active account)
        #[arg(long)]
        account_id: Option<String>,
        /// Specific credit ID to consume (defaults to earliest expiring)
        #[arg(long)]
        credit_id: Option<String>,
        /// Confirm consumption (safety gate; without this, runs in dry-run mode)
        #[arg(long)]
        confirm: bool,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Manage ChatGPT Codex OAuth accounts (same as the bare `auth` subcommands)
    #[command(alias = "openai")]
    Codex {
        #[command(subcommand)]
        cmd: CodexAuthCommand,
    },
    /// Manage xAI (Grok) OAuth accounts
    Xai {
        #[command(subcommand)]
        cmd: XaiAuthCommand,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum CodexAuthCommand {
    /// Show ChatGPT Codex OAuth authentication status
    Status {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// List signed-in ChatGPT accounts
    List {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Sign in to ChatGPT with the Codex OAuth device flow
    Login {
        /// Reauthenticate this local account while preserving provider bindings
        #[arg(long)]
        account_id: Option<String>,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Set the default ChatGPT account
    Default {
        /// Account id to make default
        account_id: String,
    },
    /// Remove a ChatGPT account
    Remove {
        /// Account id to remove
        account_id: String,
        /// Confirm removal without prompting
        #[arg(long)]
        yes: bool,
    },
    /// Remove all ChatGPT Codex OAuth authentication data
    Logout {
        /// Confirm logout without prompting
        #[arg(long)]
        yes: bool,
    },
    /// Reset Codex rate limit quota using a banked reset credit
    ResetQuota {
        /// Account ID (defaults to active account)
        #[arg(long)]
        account_id: Option<String>,
        /// Specific credit ID to consume (defaults to earliest expiring credit)
        #[arg(long)]
        credit_id: Option<String>,
        /// Confirm consumption (safety gate; without this, runs in dry-run mode)
        #[arg(long)]
        confirm: bool,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum XaiAuthCommand {
    /// Show xAI OAuth authentication status
    Status {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// List signed-in xAI accounts
    List {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Sign in to xAI with the OAuth device flow (opens a verification URL)
    Login,
    /// Set the default xAI account
    Default {
        /// Account id to make default
        account_id: String,
    },
    /// Remove an xAI account
    Remove {
        /// Account id to remove
        account_id: String,
        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Remove all xAI OAuth authentication data
    Logout {
        /// Confirm logout without prompting
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginCompleted {
    device: ManagedAuthDeviceCodeResponse,
    account: ManagedAuthAccount,
}

pub fn execute(cmd: AuthCommand) -> Result<(), AppError> {
    crate::services::global_proxy::initialize_http_client_from_disk_best_effort();
    let runtime = create_runtime()?;
    match cmd {
        AuthCommand::Status { json } => status(&runtime, json),
        AuthCommand::List { json } => list_accounts(&runtime, json),
        AuthCommand::Login { json, account_id } => login(&runtime, json, account_id.as_deref()),
        AuthCommand::Default { account_id } => set_default(&runtime, &account_id),
        AuthCommand::Remove { account_id, yes } => remove_account(&runtime, &account_id, yes),
        AuthCommand::Logout { yes } => logout(&runtime, yes),
        AuthCommand::ResetQuota {
            account_id,
            credit_id,
            confirm,
            json,
        } => reset_quota(&runtime, account_id, credit_id, confirm, json),
        AuthCommand::Codex { cmd } => execute_codex(&runtime, cmd),
        AuthCommand::Xai { cmd } => execute_xai(&runtime, cmd),
    }
}

fn execute_codex(runtime: &tokio::runtime::Runtime, cmd: CodexAuthCommand) -> Result<(), AppError> {
    match cmd {
        CodexAuthCommand::Status { json } => status(runtime, json),
        CodexAuthCommand::List { json } => list_accounts(runtime, json),
        CodexAuthCommand::Login { json, account_id } => login(runtime, json, account_id.as_deref()),
        CodexAuthCommand::Default { account_id } => set_default(runtime, &account_id),
        CodexAuthCommand::Remove { account_id, yes } => remove_account(runtime, &account_id, yes),
        CodexAuthCommand::Logout { yes } => logout(runtime, yes),
        CodexAuthCommand::ResetQuota {
            account_id,
            credit_id,
            confirm,
            json,
        } => reset_quota(runtime, account_id, credit_id, confirm, json),
    }
}

fn execute_xai(runtime: &tokio::runtime::Runtime, cmd: XaiAuthCommand) -> Result<(), AppError> {
    match cmd {
        XaiAuthCommand::Status { json } => status_for(runtime, AUTH_PROVIDER_XAI_OAUTH, json),
        XaiAuthCommand::List { json } => list_for(runtime, AUTH_PROVIDER_XAI_OAUTH, json),
        XaiAuthCommand::Login => login_for(runtime, AUTH_PROVIDER_XAI_OAUTH),
        XaiAuthCommand::Default { account_id } => {
            set_default_for(runtime, AUTH_PROVIDER_XAI_OAUTH, &account_id)
        }
        XaiAuthCommand::Remove { account_id, yes } => {
            remove_for(runtime, AUTH_PROVIDER_XAI_OAUTH, &account_id, yes)
        }
        XaiAuthCommand::Logout { yes } => logout_for(runtime, AUTH_PROVIDER_XAI_OAUTH, yes),
    }
}

fn status_for(
    runtime: &tokio::runtime::Runtime,
    provider: &str,
    json: bool,
) -> Result<(), AppError> {
    let status = runtime
        .block_on(AuthService::get_status(provider))
        .map_err(AppError::Message)?;
    if json {
        println!(
            "{}",
            to_json(&status).map_err(|source| AppError::JsonSerialize { source })?
        );
    } else {
        println!(
            "{}: {}",
            info("Provider"),
            match provider {
                AUTH_PROVIDER_XAI_OAUTH => "xAI (Grok) OAuth",
                _ => provider,
            }
        );
        if let Some(default_id) = status.default_account_id.as_ref() {
            println!("{}: {}", info("Default account"), default_id);
        }
        if let Some(error) = status
            .migration_error
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            println!("{}", warning(&format!("Credential store error: {error}")));
        } else if status.accounts.is_empty() {
            println!("{}", info("No accounts signed in. Use `auth xai login`."));
        }
        print_accounts(&status.accounts);
    }
    Ok(())
}

fn list_for(runtime: &tokio::runtime::Runtime, provider: &str, json: bool) -> Result<(), AppError> {
    let accounts = runtime
        .block_on(AuthService::list_accounts(provider))
        .map_err(AppError::Message)?;
    if json {
        println!(
            "{}",
            to_json(&accounts).map_err(|source| AppError::JsonSerialize { source })?
        );
    } else {
        print_accounts(&accounts);
    }
    Ok(())
}

fn login_for(runtime: &tokio::runtime::Runtime, provider: &str) -> Result<(), AppError> {
    let device = runtime
        .block_on(AuthService::start_login(provider))
        .map_err(AppError::Message)?;
    print_device_instructions(&device);
    println!();
    println!("{}", info("Waiting for authorization..."));
    let account = poll_until_authorized(runtime, &device)?;
    println!(
        "{}",
        success(&format!("Signed in as {} ({}).", account.login, account.id))
    );
    Ok(())
}

fn set_default_for(
    runtime: &tokio::runtime::Runtime,
    provider: &str,
    account_id: &str,
) -> Result<(), AppError> {
    let account_id = normalize_account_id(account_id)?;
    runtime
        .block_on(AuthService::set_default_account(provider, account_id))
        .map_err(AppError::Message)?;
    println!("{}", success(&format!("Default xAI account: {account_id}")));
    Ok(())
}

fn remove_for(
    runtime: &tokio::runtime::Runtime,
    provider: &str,
    account_id: &str,
    yes: bool,
) -> Result<(), AppError> {
    let account_id = normalize_account_id(account_id)?;
    if !yes && !confirm("Remove this account?")? {
        println!("{}", info("Aborted."));
        return Ok(());
    }
    runtime
        .block_on(AuthService::remove_account(provider, account_id))
        .map_err(AppError::Message)?;
    println!("{}", success(&format!("Removed account {account_id}.")));
    Ok(())
}

fn create_runtime() -> Result<tokio::runtime::Runtime, AppError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| AppError::Message(format!("failed to create async runtime: {error}")))
}

fn status(runtime: &tokio::runtime::Runtime, json: bool) -> Result<(), AppError> {
    let status = runtime
        .block_on(AuthService::get_status(AUTH_PROVIDER_CODEX_OAUTH))
        .map_err(AppError::Message)?;

    if json {
        println!(
            "{}",
            to_json(&status).map_err(|source| AppError::JsonSerialize { source })?
        );
        return Ok(());
    }

    println!("Provider:      ChatGPT (Codex OAuth)");
    println!(
        "Authenticated: {}",
        if status.authenticated { "yes" } else { "no" }
    );
    println!(
        "Default:       {}",
        status.default_account_id.as_deref().unwrap_or("-")
    );
    if let Some(error) = status
        .migration_error
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        println!("Migration:     {error}");
    }
    println!("Accounts:      {}", status.accounts.len());

    if !status.accounts.is_empty() {
        println!();
        print_accounts(&status.accounts);
    }

    Ok(())
}

fn list_accounts(runtime: &tokio::runtime::Runtime, json: bool) -> Result<(), AppError> {
    let accounts = runtime
        .block_on(AuthService::list_accounts(AUTH_PROVIDER_CODEX_OAUTH))
        .map_err(AppError::Message)?;

    if json {
        println!(
            "{}",
            to_json(&accounts).map_err(|source| AppError::JsonSerialize { source })?
        );
        return Ok(());
    }

    if accounts.is_empty() {
        println!("{}", info("No ChatGPT accounts are signed in."));
        return Ok(());
    }

    print_accounts(&accounts);
    Ok(())
}

fn login(
    runtime: &tokio::runtime::Runtime,
    json: bool,
    account_id: Option<&str>,
) -> Result<(), AppError> {
    let device = runtime
        .block_on(AuthService::start_login_for_account(
            AUTH_PROVIDER_CODEX_OAUTH,
            account_id,
        ))
        .map_err(AppError::Message)?;

    if json {
        eprintln!(
            "Open {} and enter code {}.",
            device.verification_uri, device.user_code
        );
    } else {
        print_device_instructions(&device);
        println!();
        println!("{}", info("Waiting for authorization..."));
    }

    let account = poll_until_authorized(runtime, &device)?;

    if json {
        let completed = LoginCompleted { device, account };
        println!(
            "{}",
            to_json(&completed).map_err(|source| AppError::JsonSerialize { source })?
        );
    } else {
        println!(
            "{}",
            success(&format!("Signed in as {} ({}).", account.login, account.id))
        );
    }

    Ok(())
}

fn poll_until_authorized(
    runtime: &tokio::runtime::Runtime,
    device: &ManagedAuthDeviceCodeResponse,
) -> Result<ManagedAuthAccount, AppError> {
    let expires_at = Instant::now() + Duration::from_secs(device.expires_in);
    let interval = Duration::from_secs(poll_interval_seconds(device.interval));

    loop {
        match runtime
            .block_on(AuthService::poll_for_account(
                &device.provider,
                &device.device_code,
            ))
            .map_err(AppError::Message)?
        {
            Some(account) => return Ok(account),
            None if Instant::now() >= expires_at => {
                return Err(AppError::Message(
                    "Device code expired. Please try again.".to_string(),
                ));
            }
            None => runtime.block_on(sleep_for_next_poll(interval)),
        }
    }
}

async fn sleep_for_next_poll(interval: Duration) {
    tokio::time::sleep(interval).await;
}

fn poll_interval_seconds(server_interval: u64) -> u64 {
    server_interval.max(1)
}

fn set_default(runtime: &tokio::runtime::Runtime, account_id: &str) -> Result<(), AppError> {
    let account_id = normalize_account_id(account_id)?;
    runtime
        .block_on(AuthService::set_default_account(
            AUTH_PROVIDER_CODEX_OAUTH,
            account_id,
        ))
        .map_err(AppError::Message)?;
    println!("{}", success("Default ChatGPT account updated."));
    Ok(())
}

fn remove_account(
    runtime: &tokio::runtime::Runtime,
    account_id: &str,
    yes: bool,
) -> Result<(), AppError> {
    let account_id = normalize_account_id(account_id)?;
    if !yes && !confirm(&format!("Remove ChatGPT account '{account_id}'?"))? {
        println!("{}", info("Cancelled."));
        return Ok(());
    }

    runtime
        .block_on(AuthService::remove_account(
            AUTH_PROVIDER_CODEX_OAUTH,
            account_id,
        ))
        .map_err(AppError::Message)?;
    println!("{}", success("ChatGPT account removed."));
    Ok(())
}

fn logout(runtime: &tokio::runtime::Runtime, yes: bool) -> Result<(), AppError> {
    logout_for(runtime, AUTH_PROVIDER_CODEX_OAUTH, yes)
}

fn logout_for(
    runtime: &tokio::runtime::Runtime,
    provider: &str,
    yes: bool,
) -> Result<(), AppError> {
    let (label, prompt) = if provider == AUTH_PROVIDER_XAI_OAUTH {
        ("xAI OAuth", "Remove all xAI OAuth authentication data?")
    } else {
        (
            "ChatGPT Codex OAuth",
            "Remove all ChatGPT Codex OAuth authentication data?",
        )
    };
    if !yes && !confirm(prompt)? {
        println!("{}", info("Cancelled."));
        return Ok(());
    }

    runtime
        .block_on(AuthService::logout(provider))
        .map_err(AppError::Message)?;
    println!(
        "{}",
        success(&format!("{label} authentication data removed."))
    );
    Ok(())
}

fn normalize_account_id(account_id: &str) -> Result<&str, AppError> {
    let account_id = account_id.trim();
    if account_id.is_empty() {
        return Err(AppError::InvalidInput(
            "account id cannot be empty".to_string(),
        ));
    }
    Ok(account_id)
}

fn confirm(message: &str) -> Result<bool, AppError> {
    inquire::Confirm::new(message)
        .with_default(false)
        .prompt()
        .map_err(|error| match error {
            inquire::error::InquireError::OperationCanceled
            | inquire::error::InquireError::OperationInterrupted => {
                AppError::Message("__cc_switch_auth_cancelled__".to_string())
            }
            other => AppError::Message(format!("Prompt failed: {other}")),
        })
        .or_else(|error| match error {
            AppError::Message(message) if message == "__cc_switch_auth_cancelled__" => Ok(false),
            other => Err(other),
        })
}

fn print_device_instructions(device: &ManagedAuthDeviceCodeResponse) {
    println!("Open this URL: {}", device.verification_uri);
    println!("Enter code:    {}", device.user_code);
    println!("Expires in:    {} seconds", device.expires_in);
}

fn print_accounts(accounts: &[ManagedAuthAccount]) {
    let mut table = create_table();
    table.set_header(vec![
        "Default",
        "Login",
        "Account ID",
        "Authenticated At",
        "Status",
    ]);
    for account in accounts {
        table.add_row(vec![
            if account.is_default { "yes" } else { "" }.to_string(),
            account.login.clone(),
            account.id.clone(),
            account.authenticated_at.to_string(),
            if account.reauth_required {
                "Reauthentication required"
            } else {
                "Ready"
            }
            .to_string(),
        ]);
    }
    println!("{table}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_interval_uses_managed_auth_interval_without_extra_backoff() {
        assert_eq!(poll_interval_seconds(0), 1);
        assert_eq!(poll_interval_seconds(5), 5);
        assert_eq!(poll_interval_seconds(8), 8);
        assert_eq!(poll_interval_seconds(10), 10);
    }

    #[test]
    fn poll_sleep_runs_inside_cli_runtime() {
        let runtime = create_runtime().expect("create runtime");
        runtime.block_on(sleep_for_next_poll(Duration::from_millis(1)));
    }

    #[test]
    fn normalize_account_id_rejects_blank_values() {
        assert!(normalize_account_id("  \n").is_err());
        assert_eq!(normalize_account_id(" acc-123 ").unwrap(), "acc-123");
    }
}

fn reset_quota(
    runtime: &tokio::runtime::Runtime,
    account_id: Option<String>,
    credit_id: Option<String>,
    confirm: bool,
    json: bool,
) -> Result<(), AppError> {
    // Resolve the default exactly once: inspection and consumption must target the same account.
    let account_id = match account_id {
        Some(id) => normalize_account_id(&id)?.to_string(),
        None => runtime
            .block_on(AuthService::get_status(AUTH_PROVIDER_CODEX_OAUTH))
            .map_err(AppError::Message)?
            .default_account_id
            .ok_or_else(|| {
                AppError::localized(
                    "quota.reset.no_account",
                    "没有可用的 ChatGPT 账号",
                    "No ChatGPT account available",
                )
            })?,
    };
    let quota = runtime
        .block_on(AuthService::get_quota(
            AUTH_PROVIDER_CODEX_OAUTH,
            Some(&account_id),
        ))
        .map_err(AppError::Message)?;
    let selected_credit = super::quota_reset::select_credit(&quota, credit_id.as_deref())?;
    let available_count = quota
        .reset_credits
        .as_ref()
        .map(|s| s.available_count)
        .unwrap_or(0);
    let command = super::quota_reset::auth_confirmation(&account_id, &selected_credit.id)?;

    if !confirm {
        if json {
            let dry_run = serde_json::json!({
                "dryRun": true,
                "accountId": account_id,
                "confirmationCommand": command,
                "targetCredit": selected_credit,
                "availableCount": available_count,
                "message": "To redeem this credit and reset rate limits, run again with --confirm.",
            });
            println!(
                "{}",
                to_json(&dry_run).map_err(|source| AppError::JsonSerialize { source })?
            );
            return Ok(());
        }

        println!("{}", highlight("=== Rate Limit Reset Credit (Dry Run) ==="));
        println!(
            "{}: {}",
            if crate::cli::i18n::is_chinese() {
                "账号 ID"
            } else {
                "Account ID"
            },
            account_id
        );
        println!("Credit ID:       {}", selected_credit.id);
        if let Some(title) = &selected_credit.title {
            println!("Title:           {}", title);
        }
        if let Some(expires) = &selected_credit.expires_at {
            println!("Expires at:      {}", expires);
        }
        println!("Available Cards: {}", available_count);
        println!();
        println!(
            "{}",
            warning("⚠ Consuming a reset credit is permanent and cannot be undone.")
        );
        println!(
            "{}",
            info(&format!(
                "{}\n  {command}",
                if crate::cli::i18n::is_chinese() {
                    "确认兑换此卡，请执行："
                } else {
                    "To consume this credit, run:"
                }
            ))
        );
        return Ok(());
    }

    let result = runtime
        .block_on(AuthService::reset_quota(
            AUTH_PROVIDER_CODEX_OAUTH,
            Some(&account_id),
            Some(&selected_credit.id),
        ))
        .map_err(AppError::Message)?;

    if json {
        println!(
            "{}",
            to_json(&result).map_err(|source| AppError::JsonSerialize { source })?
        );
        return Ok(());
    }

    println!(
        "{}",
        success("✓ Rate limit reset credit successfully redeemed!")
    );
    println!("Credit ID:         {}", result.credit_id);
    println!("Redeem Request ID: {}", result.redeem_request_id);
    println!("Upstream Code:     {}", result.code);
    if let Some(win) = result.windows_reset {
        println!("Windows Reset:     {}", win);
    }
    super::quota_reset::print_updated_quota(result.new_quota.as_ref());
    Ok(())
}
