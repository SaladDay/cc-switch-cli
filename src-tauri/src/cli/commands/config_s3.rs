use clap::Subcommand;

use crate::cli::ui::{highlight, info, success, warning};
use crate::error::AppError;
use crate::services::{ProviderService, S3SyncService};
use crate::settings::{get_s3_sync_settings, set_s3_sync_settings, S3SyncSettings};

#[derive(Subcommand, Debug, Clone)]
pub enum S3Command {
    /// Show current S3 sync settings
    Show,

    /// Create or update S3 sync settings
    Set {
        #[arg(long)]
        region: Option<String>,

        #[arg(long)]
        bucket: Option<String>,

        #[arg(long)]
        access_key_id: Option<String>,

        #[arg(long)]
        secret_access_key: Option<String>,

        #[arg(long)]
        endpoint: Option<String>,

        #[arg(long)]
        remote_root: Option<String>,

        #[arg(long)]
        profile: Option<String>,

        #[arg(long, conflicts_with = "disable")]
        enable: bool,

        #[arg(long, conflicts_with = "enable")]
        disable: bool,
    },

    /// Clear stored S3 sync settings
    Clear,

    /// Check whether the current S3 settings can connect successfully
    CheckConnection,

    /// Upload the current local snapshot to S3
    Upload,

    /// Download the current remote snapshot from S3
    Download,
}

pub fn execute(command: S3Command) -> Result<(), AppError> {
    match command {
        S3Command::Show => show(),
        S3Command::Set {
            region,
            bucket,
            access_key_id,
            secret_access_key,
            endpoint,
            remote_root,
            profile,
            enable,
            disable,
        } => set(
            region,
            bucket,
            access_key_id,
            secret_access_key,
            endpoint,
            remote_root,
            profile,
            enable,
            disable,
        ),
        S3Command::Clear => clear(),
        S3Command::CheckConnection => check_connection(),
        S3Command::Upload => upload(),
        S3Command::Download => download(),
    }
}

fn show() -> Result<(), AppError> {
    let Some(settings) = get_s3_sync_settings() else {
        println!(
            "{}",
            info(crate::t!("S3 sync is not configured.", "S3 同步尚未配置。"))
        );
        return Ok(());
    };

    println!(
        "{}",
        highlight(crate::t!("S3 Compatible Sync", "S3 兼容同步"))
    );
    println!("{}", "═".repeat(60));
    println!("Enabled:           {}", yes_no(settings.enabled));
    println!("Region:            {}", blank_as_na(&settings.region));
    println!("Bucket:            {}", blank_as_na(&settings.bucket));
    println!(
        "Access Key ID:     {}",
        blank_as_na(&settings.access_key_id)
    );
    println!(
        "Secret Access Key: {}",
        blank_as_na(&settings.secret_access_key)
    );
    println!("Endpoint:          {}", blank_as_aws(&settings.endpoint));
    println!("Remote Root:       {}", settings.remote_root);
    println!("Profile:           {}", settings.profile);
    println!(
        "Last Sync:         {}",
        settings
            .status
            .last_sync_at
            .map(|value| value.to_string())
            .unwrap_or_else(|| "N/A".to_string())
    );
    println!(
        "Last Error:        {}",
        settings
            .status
            .last_error
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("N/A")
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn set(
    region: Option<String>,
    bucket: Option<String>,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    endpoint: Option<String>,
    remote_root: Option<String>,
    profile: Option<String>,
    enable: bool,
    disable: bool,
) -> Result<(), AppError> {
    let settings = merged_settings(
        get_s3_sync_settings(),
        region,
        bucket,
        access_key_id,
        secret_access_key,
        endpoint,
        remote_root,
        profile,
        enable,
        disable,
    );
    set_s3_sync_settings(Some(settings))?;
    println!(
        "{}",
        success(crate::t!("✓ S3 settings saved.", "✓ S3 设置已保存。"))
    );
    Ok(())
}

fn clear() -> Result<(), AppError> {
    set_s3_sync_settings(None)?;
    println!(
        "{}",
        success(crate::t!("✓ S3 settings cleared.", "✓ S3 设置已清空。"))
    );
    Ok(())
}

fn check_connection() -> Result<(), AppError> {
    S3SyncService::check_connection()?;
    println!(
        "{}",
        success(crate::t!("✓ S3 connection succeeded.", "✓ S3 连接成功。"))
    );
    Ok(())
}

fn upload() -> Result<(), AppError> {
    let summary = S3SyncService::upload()?;
    println!("{}", success(&summary.message));
    Ok(())
}

fn download() -> Result<(), AppError> {
    let summary = S3SyncService::download()?;
    sync_live_config_after_s3();
    println!("{}", success(&summary.message));
    Ok(())
}

fn sync_live_config_after_s3() {
    let Ok(state) = crate::AppState::try_new() else {
        return;
    };
    if let Err(error) = ProviderService::sync_current_to_live(&state) {
        let en = format!("Live config sync after S3 restore failed: {error}");
        let zh = format!("S3 恢复后同步 live 配置失败: {error}");
        println!("{}", warning(crate::t!(&en, &zh)));
    }
}

#[allow(clippy::too_many_arguments)]
fn merged_settings(
    current: Option<S3SyncSettings>,
    region: Option<String>,
    bucket: Option<String>,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    endpoint: Option<String>,
    remote_root: Option<String>,
    profile: Option<String>,
    enable: bool,
    disable: bool,
) -> S3SyncSettings {
    let mut settings = current.unwrap_or_default();
    if let Some(value) = region {
        settings.region = value;
    }
    if let Some(value) = bucket {
        settings.bucket = value;
    }
    if let Some(value) = access_key_id {
        settings.access_key_id = value;
    }
    if let Some(value) = secret_access_key {
        settings.secret_access_key = value;
    }
    if let Some(value) = endpoint {
        settings.endpoint = value;
    }
    if let Some(value) = remote_root {
        settings.remote_root = value;
    }
    if let Some(value) = profile {
        settings.profile = value;
    }
    if enable {
        settings.enabled = true;
    }
    if disable {
        settings.enabled = false;
    }
    // A CLI process is not resident, so this release must not persist a toggle
    // that has no worker behind it.
    settings.auto_sync = false;
    settings
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn blank_as_na(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "N/A"
    } else {
        trimmed
    }
}

fn blank_as_aws(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "AWS default"
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::merged_settings;
    use crate::settings::{S3SyncSettings, WebDavSyncStatus};

    #[test]
    fn merged_settings_updates_only_supplied_fields_and_preserves_status() {
        let current = S3SyncSettings {
            enabled: false,
            auto_sync: true,
            region: "us-east-1".to_string(),
            bucket: "old-bucket".to_string(),
            access_key_id: "AKID".to_string(),
            secret_access_key: "SECRET".to_string(),
            endpoint: String::new(),
            remote_root: "sync-root".to_string(),
            profile: "default".to_string(),
            status: WebDavSyncStatus {
                last_error: Some("old error".to_string()),
                ..WebDavSyncStatus::default()
            },
        };

        let merged = merged_settings(
            Some(current),
            None,
            Some("new-bucket".to_string()),
            None,
            None,
            Some("https://s3.example.com".to_string()),
            None,
            None,
            true,
            false,
        );

        assert!(merged.enabled);
        assert!(!merged.auto_sync);
        assert_eq!(merged.region, "us-east-1");
        assert_eq!(merged.bucket, "new-bucket");
        assert_eq!(merged.secret_access_key, "SECRET");
        assert_eq!(merged.endpoint, "https://s3.example.com");
        assert_eq!(merged.status.last_error.as_deref(), Some("old error"));
    }
}
