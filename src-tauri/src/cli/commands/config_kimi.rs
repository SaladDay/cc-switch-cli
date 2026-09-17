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
    /// Show current Kimi Code configuration and active account status
    Status {
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
    has_credentials: bool,
    token_expires_at: Option<i64>,
    profiles_count: usize,
}

pub fn execute(cmd: KimiConfigCommand) -> Result<(), AppError> {
    match cmd {
        KimiConfigCommand::Path { json } => show_path(json),
        KimiConfigCommand::Status { json } => show_status(json),
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

fn show_status(json: bool) -> Result<(), AppError> {
    let home = get_kimi_config_dir();
    let active_profile = get_active_profile_name();
    let creds = read_native_credentials().map_err(|e| AppError::Message(e.to_string()))?;
    let profiles = list_profiles().map_err(|e| AppError::Message(e.to_string()))?;

    let info_obj = KimiStatusInfo {
        home_dir: home.clone(),
        active_profile: active_profile.clone(),
        has_credentials: creds.is_some(),
        token_expires_at: creds.as_ref().and_then(|c| c.expires_at),
        profiles_count: profiles.len(),
    };

    if json {
        println!("{}", to_json(&info_obj).map_err(|e| AppError::JsonSerialize { source: e })?);
        return Ok(());
    }

    println!("Kimi Code Home:   {}", home.display());
    println!(
        "Active Profile:   {}",
        active_profile.as_deref().unwrap_or("(default / unmanaged)")
    );
    println!(
        "Credentials:      {}",
        if creds.is_some() { "Present" } else { "None" }
    );
    if let Some(exp) = creds.as_ref().and_then(|c| c.expires_at) {
        let dt = chrono::DateTime::from_timestamp(exp, 0)
            .map(|d| d.to_rfc3339())
            .unwrap_or_else(|| exp.to_string());
        println!("Token Expires At: {}", dt);
    }
    println!("Profiles Count:   {}", profiles.len());

    Ok(())
}

fn execute_profile(cmd: KimiProfileCommand) -> Result<(), AppError> {
    match cmd {
        KimiProfileCommand::List { json } => {
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
            table.set_header(vec!["Active", "Name", "Has Config", "Has Credentials", "Path"]);
            for p in profiles {
                table.add_row(vec![
                    if p.is_active { "*" } else { " " },
                    &p.name,
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
            if !yes && !confirm(&format!("Remove Kimi Code profile '{name}'?"))? {
                println!("{}", info("Cancelled."));
                return Ok(());
            }
            remove_profile(&name).map_err(|e| AppError::Message(e.to_string()))?;
            println!("{}", success(&format!("Removed Kimi Code profile '{name}'.")));
            Ok(())
        }
    }
}

fn confirm(prompt: &str) -> Result<bool, AppError> {
    inquire::Confirm::new(prompt)
        .with_default(false)
        .prompt()
        .map_err(|err| AppError::Message(format!("failed to confirm action: {err}")))
}
