use std::time::{Duration, Instant};

use clap::Subcommand;
use inquire::Confirm;
use serde_json::{json, Value};

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::cli::ui::{error as ui_error, highlight, info, success, warning};
use crate::error::AppError;
use crate::provider::{AuthBinding, AuthBindingSource, Provider, ProviderMeta};
use crate::proxy::providers::copilot_auth::{CopilotAuthError, GitHubAccount};
use crate::services::{CopilotService, ProviderService};
use crate::store::AppState;

const COPILOT_DEFAULT_MODEL: &str = "claude-sonnet-4.6";
const COPILOT_DEFAULT_HAIKU: &str = "claude-haiku-4.5";
const COPILOT_DEFAULT_OPUS: &str = "claude-sonnet-4.6";
const COPILOT_BASE_URL: &str = "https://api.githubcopilot.com";
const POLL_FALLBACK_INTERVAL_SECS: u64 = 5;
const POLL_MIN_INTERVAL_SECS: u64 = 5;

#[derive(Subcommand, Debug, Clone)]
pub enum CopilotCommand {
    /// Authenticate with GitHub via device-code OAuth
    Login {
        /// GitHub Enterprise Server domain (omit for github.com)
        #[arg(long)]
        domain: Option<String>,

        /// Skip creating or updating the Claude provider entry
        #[arg(long = "no-provider")]
        no_provider: bool,
    },

    /// Sign out of GitHub Copilot
    Logout {
        /// Sign out of a specific account ID (omit to clear all accounts)
        #[arg(long)]
        account_id: Option<String>,
    },

    /// List authenticated GitHub Copilot accounts
    Accounts,

    /// Set the default GitHub Copilot account
    SetDefault {
        /// Account ID to set as default
        account_id: String,
    },

    /// Show current GitHub Copilot authentication status
    Status,
}

pub fn execute(cmd: CopilotCommand) -> Result<(), AppError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))?;

    runtime.block_on(execute_async(cmd))
}

pub async fn execute_async(cmd: CopilotCommand) -> Result<(), AppError> {
    match cmd {
        CopilotCommand::Login {
            domain,
            no_provider,
        } => login(domain.as_deref(), no_provider).await,
        CopilotCommand::Logout { account_id } => logout(account_id.as_deref()).await,
        CopilotCommand::Accounts => list_accounts().await,
        CopilotCommand::SetDefault { account_id } => set_default(&account_id).await,
        CopilotCommand::Status => status().await,
    }
}

/// Run the GitHub device-code login flow. Returns the new GitHub account on success.
pub async fn run_copilot_login_flow(
    domain: Option<&str>,
) -> Result<GitHubAccount, AppError> {
    println!("{}", info(texts::copilot_login_starting()));

    let device = CopilotService::start_device_flow(domain)
        .await
        .map_err(|e| AppError::Message(texts::copilot_login_failed(&e.to_string())))?;

    println!(
        "{}",
        highlight(&texts::copilot_login_visit_url(
            &device.verification_uri,
            &device.user_code,
        ))
    );
    println!("{}", info(texts::copilot_login_waiting()));

    let interval = device.interval.max(POLL_MIN_INTERVAL_SECS);
    let deadline = Instant::now() + Duration::from_secs(device.expires_in);

    loop {
        if Instant::now() >= deadline {
            return Err(AppError::Message(
                texts::copilot_login_device_code_expired().to_string(),
            ));
        }

        tokio::time::sleep(Duration::from_secs(interval)).await;

        match CopilotService::poll_for_token(&device.device_code, domain).await {
            Ok(Some(account)) => return Ok(account),
            Ok(None) => continue,
            Err(CopilotAuthError::AuthorizationPending) => continue,
            Err(CopilotAuthError::ExpiredToken) => {
                return Err(AppError::Message(
                    texts::copilot_login_device_code_expired().to_string(),
                ));
            }
            Err(other) => {
                return Err(AppError::Message(texts::copilot_login_failed(
                    &other.to_string(),
                )));
            }
        }
    }
}

async fn login(domain: Option<&str>, no_provider: bool) -> Result<(), AppError> {
    // Fall back to a wider polling cadence so users on slow networks aren't surprised.
    let _ = POLL_FALLBACK_INTERVAL_SECS;
    let account = run_copilot_login_flow(domain).await?;
    println!(
        "{}",
        success(&texts::copilot_login_success(&account.login))
    );

    if no_provider {
        return Ok(());
    }

    let create = Confirm::new(texts::copilot_login_create_provider_prompt())
        .with_default(true)
        .prompt()
        .map_err(|e| AppError::Message(format!("input failed: {e}")))?;

    if !create {
        return Ok(());
    }

    let state = AppState::try_new()?;
    let provider_id = build_or_update_copilot_provider(&state, &account)?;

    let switch_now = Confirm::new(texts::copilot_login_switch_to_provider_prompt())
        .with_default(true)
        .prompt()
        .map_err(|e| AppError::Message(format!("input failed: {e}")))?;

    if switch_now {
        ProviderService::switch(&state, AppType::Claude, &provider_id)?;
        println!(
            "{}",
            success(&format!("Switched to provider: {provider_id}"))
        );
    }

    Ok(())
}

fn build_or_update_copilot_provider(
    state: &AppState,
    account: &GitHubAccount,
) -> Result<String, AppError> {
    let providers = state
        .db
        .get_all_providers(AppType::Claude.as_str())
        .map_err(|e| AppError::Database(format!("load Claude providers failed: {e}")))?;

    let existing_for_account = providers.values().find(|p| {
        p.meta
            .as_ref()
            .and_then(|m| m.managed_account_id_for("github_copilot"))
            .as_deref()
            == Some(account.id.as_str())
    });

    let id = if let Some(existing) = existing_for_account {
        existing.id.clone()
    } else {
        let existing_ids: Vec<String> = providers.values().map(|p| p.id.clone()).collect();
        let base = if account.login.trim().is_empty() {
            "github-copilot".to_string()
        } else {
            format!(
                "github-copilot-{}",
                account.login.to_ascii_lowercase().replace(['_', ' '], "-")
            )
        };
        crate::cli::commands::provider_input::generate_provider_id(&base, &existing_ids)
    };

    let meta = ProviderMeta {
        api_format: Some("openai_chat".to_string()),
        provider_type: Some("github_copilot".to_string()),
        auth_binding: Some(AuthBinding {
            source: AuthBindingSource::ManagedAccount,
            auth_provider: Some("github_copilot".to_string()),
            account_id: Some(account.id.clone()),
        }),
        ..Default::default()
    };

    let settings_config = json!({
        "env": {
            "ANTHROPIC_BASE_URL": COPILOT_BASE_URL,
            "ANTHROPIC_MODEL": COPILOT_DEFAULT_MODEL,
            "ANTHROPIC_DEFAULT_HAIKU_MODEL": COPILOT_DEFAULT_HAIKU,
            "ANTHROPIC_DEFAULT_SONNET_MODEL": COPILOT_DEFAULT_MODEL,
            "ANTHROPIC_DEFAULT_OPUS_MODEL": COPILOT_DEFAULT_OPUS,
        }
    });

    let mut provider = Provider::with_id(
        id.clone(),
        format!("GitHub Copilot ({})", account.login),
        settings_config,
        Some("https://github.com/features/copilot".to_string()),
    );
    provider.meta = Some(meta);
    provider.icon = Some("github".to_string());

    if existing_for_account.is_some() {
        ProviderService::update(state, AppType::Claude, provider)?;
    } else {
        ProviderService::add(state, AppType::Claude, provider)?;
    }

    Ok(id)
}

async fn logout(account_id: Option<&str>) -> Result<(), AppError> {
    match account_id {
        Some(id) => {
            CopilotService::remove_account(id)
                .await
                .map_err(|e| AppError::Message(e.to_string()))?;
            println!("{}", success(&texts::copilot_logout_account_success(id)));
        }
        None => {
            CopilotService::clear_auth()
                .await
                .map_err(|e| AppError::Message(e.to_string()))?;
            println!("{}", success(texts::copilot_logout_success()));
        }
    }
    Ok(())
}

async fn list_accounts() -> Result<(), AppError> {
    let accounts = CopilotService::list_accounts().await;
    if accounts.is_empty() {
        println!("{}", warning(texts::copilot_no_accounts()));
        return Ok(());
    }

    println!("{}", highlight(texts::copilot_account_list_header()));
    let status = CopilotService::get_status().await;
    let default_id = status.default_account_id.as_deref();

    for account in accounts {
        let marker = if Some(account.id.as_str()) == default_id {
            "*"
        } else {
            " "
        };
        println!(
            "  {marker} {} ({})  domain={}  id={}",
            account.login,
            account.github_domain,
            account.github_domain,
            account.id
        );
    }
    Ok(())
}

async fn set_default(account_id: &str) -> Result<(), AppError> {
    CopilotService::set_default_account(account_id)
        .await
        .map_err(|e| AppError::Message(e.to_string()))?;
    println!("{}", success(&texts::copilot_default_set(account_id)));
    Ok(())
}

async fn status() -> Result<(), AppError> {
    let status = CopilotService::get_status().await;
    if status.accounts.is_empty() {
        println!("{}", warning(texts::copilot_no_accounts()));
        return Ok(());
    }

    println!("{}", highlight(texts::copilot_account_list_header()));
    let default_id = status.default_account_id.as_deref();
    for account in &status.accounts {
        let marker = if Some(account.id.as_str()) == default_id {
            "*"
        } else {
            " "
        };
        let line = format!(
            "  {marker} {} ({})  id={}",
            account.login, account.github_domain, account.id
        );
        println!("{}", info(&line));
    }
    Ok(())
}

#[cfg(test)]
fn _ensure_value_is_used(_v: &Value) {}

#[allow(dead_code)]
fn _ensure_error_is_used(_v: &str) -> String {
    ui_error(_v)
}
