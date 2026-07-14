use clap::Subcommand;
use serde::Serialize;
use std::time::{Duration, Instant};

use crate::cli::ui::{create_table, info, success, to_json};
use crate::error::AppError;
use crate::services::{AuthService, ManagedAuthAccount, ManagedAuthDeviceCodeResponse};

const AUTH_PROVIDER_CODEX_OAUTH: &str = "codex_oauth";

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
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginCompleted {
    device: ManagedAuthDeviceCodeResponse,
    account: ManagedAuthAccount,
}

pub fn execute(cmd: AuthCommand) -> Result<(), AppError> {
    let runtime = create_runtime()?;
    match cmd {
        AuthCommand::Status { json } => status(&runtime, json),
        AuthCommand::List { json } => list_accounts(&runtime, json),
        AuthCommand::Login { json } => login(&runtime, json),
        AuthCommand::Default { account_id } => set_default(&runtime, &account_id),
        AuthCommand::Remove { account_id, yes } => remove_account(&runtime, &account_id, yes),
        AuthCommand::Logout { yes } => logout(&runtime, yes),
    }
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

fn login(runtime: &tokio::runtime::Runtime, json: bool) -> Result<(), AppError> {
    let device = runtime
        .block_on(AuthService::start_login(AUTH_PROVIDER_CODEX_OAUTH))
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
                AUTH_PROVIDER_CODEX_OAUTH,
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
    if !yes && !confirm("Remove all ChatGPT Codex OAuth authentication data?")? {
        println!("{}", info("Cancelled."));
        return Ok(());
    }

    runtime
        .block_on(AuthService::logout(AUTH_PROVIDER_CODEX_OAUTH))
        .map_err(AppError::Message)?;
    println!(
        "{}",
        success("ChatGPT Codex OAuth authentication data removed.")
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
    table.set_header(vec!["Default", "Login", "Account ID", "Authenticated At"]);
    for account in accounts {
        table.add_row(vec![
            if account.is_default { "yes" } else { "" }.to_string(),
            account.login.clone(),
            account.id.clone(),
            account.authenticated_at.to_string(),
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
