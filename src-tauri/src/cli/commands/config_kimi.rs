use clap::Subcommand;
use std::path::PathBuf;

use crate::cli::ui::{create_table, info, success, to_json};
use crate::error::AppError;
use crate::kimi_config::{
    get_active_profile_name, get_kimi_config_dir,
    get_kimi_profiles_dir, list_profiles, read_native_credentials, remove_profile, save_profile,
    switch_profile, KIMI_CONFIG_FILE, KIMI_CREDENTIALS_DIR, KIMI_DEFAULT_CREDENTIAL_FILE,
};

#[derive(Subcommand, Debug, Clone)]
pub enum KimiConfigCommand {
    /// Show Kimi Code configuration and credential paths
    Path {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Show current Kimi Code configuration and account status (active profile, specified profile, or all)
    Status {
        /// Optional profile name to inspect (defaults to currently active profile)
        profile: Option<String>,
        /// Inspect status of all saved profiles
        #[arg(long)]
        all: bool,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Manage Kimi Code configuration profiles (work, personal, etc.)
    #[command(subcommand)]
    Profile(KimiProfileCommand),
}

#[derive(Subcommand, Debug, Clone)]
pub enum KimiProfileCommand {
    /// List all saved Kimi Code profiles
    List {
        /// Query and show live 5-hour quota and reset countdown for each profile
        #[arg(short = 'q', long)]
        quota: bool,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Save current Kimi Code directory configuration as a named profile
    Save {
        /// Profile name (e.g. work, personal)
        name: String,
    },
    /// Switch active Kimi Code directory configuration to a named profile
    Switch {
        /// Profile name to activate
        name: String,
    },
    /// Remove a saved Kimi Code profile
    Remove {
        /// Profile name to remove
        name: String,
        /// Confirm removal without prompting
        #[arg(long)]
        yes: bool,
    },
}

#[derive(serde::Serialize)]
struct KimiPathInfo {
    home_dir: PathBuf,
    config_file: PathBuf,
    credentials_file: PathBuf,
    profiles_dir: PathBuf,
    active_profile: Option<String>,
}

#[derive(serde::Serialize)]
struct KimiStatusInfo {
    home_dir: PathBuf,
    active_profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<String>,
    has_credentials: bool,
    token_expires_at: Option<i64>,
    profiles_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    usages: Option<crate::kimi_config::KimiUsagesResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub fn execute(cmd: KimiConfigCommand) -> Result<(), AppError> {
    match cmd {
        KimiConfigCommand::Path { json } => show_path(json),
        KimiConfigCommand::Status { profile, all, json } => show_status(profile, all, json),
        KimiConfigCommand::Profile(profile_cmd) => execute_profile(profile_cmd),
    }
}

fn show_path(json: bool) -> Result<(), AppError> {
    let home = get_kimi_config_dir();
    let info_obj = KimiPathInfo {
        home_dir: home.clone(),
        config_file: home.join(KIMI_CONFIG_FILE),
        credentials_file: home.join(KIMI_CREDENTIALS_DIR).join(KIMI_DEFAULT_CREDENTIAL_FILE),
        profiles_dir: get_kimi_profiles_dir(),
        active_profile: get_active_profile_name(),
    };

    if json {
        println!("{}", to_json(&info_obj).map_err(|e| AppError::JsonSerialize { source: e })?);
        return Ok(());
    }

    println!("Kimi Code Home:      {}", info_obj.home_dir.display());
    println!("Config File:         {}", info_obj.config_file.display());
    println!("Credentials File:    {}", info_obj.credentials_file.display());
    println!("Profiles Directory:  {}", info_obj.profiles_dir.display());
    println!(
        "Active Profile:      {}",
        info_obj.active_profile.as_deref().unwrap_or("-")
    );

    Ok(())
}

fn print_status_info(info: &KimiStatusInfo) {
    println!("Profile:          {}", info.active_profile.as_deref().unwrap_or("(default / unmanaged)"));
    if let Some(ref acc) = info.account {
        println!("Account:          {}", acc);
    }
    println!("Directory:        {}", info.home_dir.display());
    println!(
        "Credentials:      {}",
        if info.has_credentials { "Present" } else { "None" }
    );
    if let Some(exp) = info.token_expires_at {
        let dt = chrono::DateTime::from_timestamp(exp, 0)
            .map(|d| d.to_rfc3339())
            .unwrap_or_else(|| exp.to_string());
        println!("Token Expires At: {}", dt);
    }
    if let Some(ref err) = info.error {
        println!("Error:            {}", err);
    }

    if let Some(u) = &info.usages {
        if let Some(q) = &u.usages {
            let now = chrono::Utc::now();
            println!();
            println!("Usage & Rate Limits:");

            let five_hour_detail = u.limits.iter().find(|l| {
                l.window.as_ref().and_then(|w| w.duration) == Some(300)
            }).and_then(|l| l.detail.as_ref());

            if let Some(item) = &q.limit_5h {
                let ratio = item.used_ratio.unwrap_or(0.0);
                let pct = ratio * 100.0;
                let status_tag = if ratio >= 1.0 {
                    " [EXCEEDED / 5小时额度已耗尽]"
                } else {
                    ""
                };

                let mut extras = Vec::new();
                if !status_tag.is_empty() {
                    extras.push(status_tag.trim().to_string());
                }
                if let Some(d) = five_hour_detail {
                    let used_count = d.used.as_deref().unwrap_or("0");
                    if let (Some(l), Some(r)) = (&d.limit, &d.remaining) {
                        extras.push(format!("[{}/{}, remaining: {}]", used_count, l, r));
                    } else if let Some(l) = &d.limit {
                        extras.push(format!("[{}/{}]", used_count, l));
                    }
                }

                let extras_str = if extras.is_empty() {
                    String::new()
                } else {
                    format!(" {}", extras.join(" "))
                };

                let reset_str = if let Some(reset_info) = crate::cli::provider_quota::quota_reset_display(item.reset_time.as_deref(), now) {
                    let local_time = reset_info.at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S");
                    let countdown = reset_info.remaining.map(|r| format!("in {r}")).unwrap_or_else(|| "soon".to_string());
                    format!("resets {} at {}", countdown, local_time)
                } else {
                    format!("reset: {}", item.reset_time.as_deref().unwrap_or("-"))
                };

                println!("  5-Hour Limit:   {:.1}% used{} ({})", pct, extras_str, reset_str);
            }
            if let Some(item) = &q.limit_7d {
                let pct = item.used_ratio.unwrap_or(0.0) * 100.0;
                let reset_str = if let Some(reset_info) = crate::cli::provider_quota::quota_reset_display(item.reset_time.as_deref(), now) {
                    let local_time = reset_info.at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S");
                    let countdown = reset_info.remaining.map(|r| format!("in {r}")).unwrap_or_else(|| "soon".to_string());
                    format!("resets {} at {}", countdown, local_time)
                } else {
                    format!("reset: {}", item.reset_time.as_deref().unwrap_or("-"))
                };
                println!("  7-Day Limit:    {:.1}% used ({})", pct, reset_str);
            }
            if let Some(item) = &q.limit_month_total {
                let pct = item.used_ratio.unwrap_or(0.0) * 100.0;
                let reset_str = if let Some(reset_info) = crate::cli::provider_quota::quota_reset_display(item.reset_time.as_deref(), now) {
                    let local_time = reset_info.at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S");
                    let countdown = reset_info.remaining.map(|r| format!("in {r}")).unwrap_or_else(|| "soon".to_string());
                    format!("resets {} at {}", countdown, local_time)
                } else {
                    format!("reset: {}", item.reset_time.as_deref().unwrap_or("-"))
                };
                println!("  Monthly Total:  {:.1}% used ({})", pct, reset_str);
            }
        }
    }
}

fn show_status(profile: Option<String>, all: bool, json: bool) -> Result<(), AppError> {
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| AppError::Message(format!("无法创建 Tokio 运行时: {e}")))?;

    if all {
        let items = rt.block_on(crate::kimi_config::fetch_all_profiles_quota());
        if json {
            println!("{}", to_json(&items).map_err(|e| AppError::JsonSerialize { source: e })?);
            return Ok(());
        }

        if items.is_empty() {
            println!("{}", info("No Kimi Code profiles found."));
            return Ok(());
        }

        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                println!("\n------------------------------------------------------------\n");
            }
            let active_tag = if item.profile.is_active { " (active)" } else { "" };
            let info_obj = KimiStatusInfo {
                home_dir: item.profile.path.clone(),
                active_profile: Some(format!("{}{}", item.profile.name, active_tag)),
                account: item.profile.account.clone(),
                has_credentials: item.profile.has_credentials,
                token_expires_at: None,
                profiles_count: items.len(),
                usages: item.usages.clone(),
                error: item.error.clone(),
            };
            print_status_info(&info_obj);
        }
        return Ok(());
    }

    if let Some(ref name) = profile {
        let profiles = list_profiles().map_err(|e| AppError::Message(e.to_string()))?;
        let p = profiles.into_iter().find(|x| &x.name == name).ok_or_else(|| {
            AppError::Message(format!("Profile '{name}' 不存在"))
        })?;

        let mut creds = crate::kimi_config::read_profile_credentials(name)
            .map_err(|e| AppError::Message(e.to_string()))?;

        let (usages, error) = if let Some(ref mut c) = creds {
            let cred_path = p.path.join(KIMI_CREDENTIALS_DIR).join(KIMI_DEFAULT_CREDENTIAL_FILE);
            match rt.block_on(crate::kimi_config::get_valid_access_token(c, Some(&cred_path))) {
                Ok(token) => match rt.block_on(crate::kimi_config::fetch_kimi_usages(&token)) {
                    Ok(u) => (Some(u), None),
                    Err(e) => (None, Some(e.to_string())),
                },
                Err(e) => (None, Some(e.to_string())),
            }
        } else {
            (None, None)
        };

        let active_tag = if p.is_active { " (active)" } else { "" };
        let info_obj = KimiStatusInfo {
            home_dir: p.path.clone(),
            active_profile: Some(format!("{}{}", p.name, active_tag)),
            account: p.account.clone(),
            has_credentials: creds.is_some(),
            token_expires_at: creds.as_ref().and_then(|c| c.expires_at),
            profiles_count: 1,
            usages,
            error,
        };

        if json {
            println!("{}", to_json(&info_obj).map_err(|e| AppError::JsonSerialize { source: e })?);
            return Ok(());
        }

        print_status_info(&info_obj);
        return Ok(());
    }

    // Default: currently active ~/.kimi-code
    let home = get_kimi_config_dir();
    let active_profile = get_active_profile_name();
    let mut creds = read_native_credentials().map_err(|e| AppError::Message(e.to_string()))?;
    let profiles = list_profiles().map_err(|e| AppError::Message(e.to_string()))?;

    let (usages, error) = if let Some(ref mut c) = creds {
        let cred_path = home.join(KIMI_CREDENTIALS_DIR).join(KIMI_DEFAULT_CREDENTIAL_FILE);
        match rt.block_on(crate::kimi_config::get_valid_access_token(c, Some(&cred_path))) {
            Ok(token) => match rt.block_on(crate::kimi_config::fetch_kimi_usages(&token)) {
                Ok(u) => (Some(u), None),
                Err(e) => (None, Some(e.to_string())),
            },
            Err(e) => (None, Some(e.to_string())),
        }
    } else {
        (None, None)
    };

    let account = creds.as_ref().and_then(|c| crate::kimi_config::resolve_account_nickname(c));

    let info_obj = KimiStatusInfo {
        home_dir: home.clone(),
        active_profile: active_profile.clone(),
        account,
        has_credentials: creds.is_some(),
        token_expires_at: creds.as_ref().and_then(|c| c.expires_at),
        profiles_count: profiles.len(),
        usages,
        error,
    };

    if json {
        println!("{}", to_json(&info_obj).map_err(|e| AppError::JsonSerialize { source: e })?);
        return Ok(());
    }

    print_status_info(&info_obj);
    println!("Profiles Count:   {}", info_obj.profiles_count);

    Ok(())
}

fn format_5h_usage_cell(item: &crate::kimi_config::KimiProfileQuotaItem) -> (String, String) {
    if !item.profile.has_credentials {
        return ("(no credentials)".to_string(), "-".to_string());
    }
    if let Some(ref err) = item.error {
        if err.contains("缺少 refresh_token") || err.contains("未登录") {
            return ("(unauthenticated)".to_string(), "-".to_string());
        }
        return (format!("error: {err}"), "-".to_string());
    }
    if let Some(ref u) = item.usages {
        if let Some(ref q) = u.usages {
            if let Some(ref l5) = q.limit_5h {
                let ratio = l5.used_ratio.unwrap_or(0.0);
                let pct = ratio * 100.0;
                let five_hour_detail = u.limits.iter().find(|l| {
                    l.window.as_ref().and_then(|w| w.duration) == Some(300)
                }).and_then(|l| l.detail.as_ref());

                let count_str = if let Some(d) = five_hour_detail {
                    let used_cnt = d.used.as_deref().unwrap_or("0");
                    let limit_cnt = d.limit.as_deref().unwrap_or("100");
                    let rem_cnt = d.remaining.as_deref().unwrap_or("0");
                    format!(" [{used_cnt}/{limit_cnt}, rem: {rem_cnt}]")
                } else {
                    String::new()
                };

                let status_tag = if ratio >= 1.0 { " [EXCEEDED]" } else { "" };
                let usage_str = format!("{pct:.1}%{status_tag}{count_str}");

                let reset_str = if let Some(reset_info) = crate::cli::provider_quota::quota_reset_display(l5.reset_time.as_deref(), chrono::Utc::now()) {
                    let local = reset_info.at.with_timezone(&chrono::Local).format("%H:%M:%S");
                    let countdown = reset_info.remaining.unwrap_or_else(|| "soon".to_string());
                    format!("{countdown} ({local})")
                } else {
                    l5.reset_time.as_deref().unwrap_or("-").to_string()
                };

                return (usage_str, reset_str);
            }
        }
    }
    ("-".to_string(), "-".to_string())
}

fn format_7d_usage_cell(item: &crate::kimi_config::KimiProfileQuotaItem) -> (String, String) {
    if !item.profile.has_credentials {
        return ("(no credentials)".to_string(), "-".to_string());
    }
    if let Some(ref err) = item.error {
        if err.contains("缺少 refresh_token") || err.contains("未登录") {
            return ("(unauthenticated)".to_string(), "-".to_string());
        }
        return (format!("error: {err}"), "-".to_string());
    }
    if let Some(ref u) = item.usages {
        if let Some(ref q) = u.usages {
            if let Some(ref l7) = q.limit_7d {
                let ratio = l7.used_ratio.unwrap_or(0.0);
                let pct = ratio * 100.0;
                let status_tag = if ratio >= 1.0 { " [EXCEEDED]" } else { "" };
                let usage_str = format!("{pct:.1}%{status_tag}");

                let reset_str = if let Some(reset_info) = crate::cli::provider_quota::quota_reset_display(l7.reset_time.as_deref(), chrono::Utc::now()) {
                    let local = reset_info.at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M");
                    let countdown = reset_info.remaining.unwrap_or_else(|| "soon".to_string());
                    format!("{countdown} ({local})")
                } else {
                    l7.reset_time.as_deref().unwrap_or("-").to_string()
                };

                return (usage_str, reset_str);
            }
        }
    }
    ("-".to_string(), "-".to_string())
}

fn execute_profile(cmd: KimiProfileCommand) -> Result<(), AppError> {
    match cmd {
        KimiProfileCommand::List { quota, json } => {
            if quota {
                let rt = tokio::runtime::Runtime::new()
                    .map_err(|e| AppError::Message(format!("无法创建 Tokio 运行时: {e}")))?;
                let items = rt.block_on(crate::kimi_config::fetch_all_profiles_quota());
                if json {
                    println!(
                        "{}",
                        to_json(&items).map_err(|e| AppError::JsonSerialize { source: e })?
                    );
                    return Ok(());
                }

                if items.is_empty() {
                    println!("{}", info("No Kimi Code profiles found. Use `cc-switch config kimi profile save <name>` to save one."));
                    return Ok(());
                }

                let mut table = create_table();
                table.set_header(vec!["Active", "Profile", "Account", "5-Hour Usage", "Reset In", "7-Day Usage", "7D Reset", "Path"]);
                for item in items {
                    let (usage_5h_str, reset_5h_str) = format_5h_usage_cell(&item);
                    let (usage_7d_str, reset_7d_str) = format_7d_usage_cell(&item);
                    table.add_row(vec![
                        if item.profile.is_active { "*" } else { " " },
                        &item.profile.name,
                        item.profile.account.as_deref().unwrap_or("-"),
                        &usage_5h_str,
                        &reset_5h_str,
                        &usage_7d_str,
                        &reset_7d_str,
                        &item.profile.path.display().to_string(),
                    ]);
                }
                println!("{table}");
                return Ok(());
            }

            let profiles = list_profiles().map_err(|e| AppError::Message(e.to_string()))?;
            if json {
                println!(
                    "{}",
                    to_json(&profiles).map_err(|e| AppError::JsonSerialize { source: e })?
                );
                return Ok(());
            }

            if profiles.is_empty() {
                println!("{}", info("No Kimi Code profiles found. Use `cc-switch config kimi profile save <name>` to save one."));
                return Ok(());
            }

            let mut table = create_table();
            table.set_header(vec!["Active", "Profile", "Account", "Has Config", "Has Credentials", "Path"]);
            for p in profiles {
                table.add_row(vec![
                    if p.is_active { "*" } else { " " },
                    &p.name,
                    p.account.as_deref().unwrap_or("-"),
                    if p.has_config { "yes" } else { "no" },
                    if p.has_credentials { "yes" } else { "no" },
                    &p.path.display().to_string(),
                ]);
            }
            println!("{table}");
            Ok(())
        }
        KimiProfileCommand::Save { name } => {
            let path = save_profile(&name).map_err(|e| AppError::Message(e.to_string()))?;
            println!(
                "{}",
                success(&format!(
                    "Current Kimi Code configuration saved to profile '{}' ({}).",
                    name,
                    path.display()
                ))
            );
            Ok(())
        }
        KimiProfileCommand::Switch { name } => {
            switch_profile(&name).map_err(|e| AppError::Message(e.to_string()))?;
            println!(
                "{}",
                success(&format!(
                    "Switched Kimi Code configuration to profile '{}'.",
                    name
                ))
            );
            Ok(())
        }
        KimiProfileCommand::Remove { name, yes } => {
            if !yes {
                println!(
                    "{}",
                    info(&format!(
                        "Pass --yes to confirm deletion of Kimi Code profile '{}'.",
                        name
                    ))
                );
                return Ok(());
            }

            remove_profile(&name).map_err(|e| AppError::Message(e.to_string()))?;
            println!(
                "{}",
                success(&format!("Profile '{}' has been removed.", name))
            );
            Ok(())
        }
    }
}
