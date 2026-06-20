use clap::{Subcommand, ValueEnum};
use serde_json::json;

use crate::app_config::AppType;
use crate::cli::i18n::{self, Language};
use crate::cli::ui::{highlight, info, success, to_json, warning};
use crate::error::AppError;

#[derive(Subcommand, Debug, Clone)]
pub enum SettingsCommand {
    /// Show persisted cc-switch settings
    Show {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Get or set the TUI language
    Language {
        /// Language to persist (en|zh)
        #[arg(value_enum)]
        language: Option<LanguageArg>,
    },

    /// Manage visible apps shown in the TUI
    #[command(name = "visible-apps", subcommand)]
    VisibleApps(VisibleAppsCommand),

    /// Show, skip, or require Claude onboarding
    #[command(name = "claude-onboarding", subcommand)]
    ClaudeOnboarding(ClaudeOnboardingCommand),

    /// Show, enable, or disable Claude plugin integration
    #[command(name = "claude-plugin", subcommand)]
    ClaudePlugin(ClaudePluginCommand),

    /// Manage unified Codex session history
    #[command(name = "codex-history", subcommand)]
    CodexHistory(CodexHistoryCommand),
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageArg {
    En,
    Zh,
}

impl From<LanguageArg> for Language {
    fn from(value: LanguageArg) -> Self {
        match value {
            LanguageArg::En => Language::English,
            LanguageArg::Zh => Language::Chinese,
        }
    }
}

#[derive(Subcommand, Debug, Clone)]
pub enum VisibleAppsCommand {
    /// Show visible app settings
    Show {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Set visible apps mode
    Mode {
        /// Mode to persist (auto|manual)
        #[arg(value_enum)]
        mode: VisibleAppsModeArg,
    },

    /// Enable one app and switch visibility to manual mode
    Enable {
        /// App to show
        #[arg(value_enum)]
        app: AppType,
    },

    /// Disable one app and switch visibility to manual mode
    Disable {
        /// App to hide
        #[arg(value_enum)]
        app: AppType,
    },

    /// Replace the visible app list and switch visibility to manual mode
    Set {
        /// Apps to show; at least one app is required
        #[arg(value_enum, required = true)]
        apps: Vec<AppType>,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibleAppsModeArg {
    Auto,
    Manual,
}

impl From<VisibleAppsModeArg> for crate::settings::VisibleAppsMode {
    fn from(value: VisibleAppsModeArg) -> Self {
        match value {
            VisibleAppsModeArg::Auto => crate::settings::VisibleAppsMode::Auto,
            VisibleAppsModeArg::Manual => crate::settings::VisibleAppsMode::Manual,
        }
    }
}

#[derive(Subcommand, Debug, Clone)]
pub enum ClaudeOnboardingCommand {
    /// Show current onboarding skip setting
    Show,
    /// Mark Claude onboarding as completed and skip the first-run prompt
    Skip,
    /// Clear the completed marker so Claude onboarding can run again
    Require,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ClaudePluginCommand {
    /// Show current Claude plugin integration setting
    Show,
    /// Enable Claude plugin integration and sync current provider state
    Enable,
    /// Disable Claude plugin integration and sync current provider state
    Disable,
}

#[derive(Subcommand, Debug, Clone)]
pub enum CodexHistoryCommand {
    /// Show unified Codex session history setting
    Show {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Enable unified Codex session history
    Enable {
        /// Also migrate existing official Codex sessions into the shared history bucket
        #[arg(long)]
        migrate_existing: bool,
    },
    /// Disable unified Codex session history
    Disable {
        /// Restore previously migrated official sessions from backups
        #[arg(long)]
        restore: bool,
    },
    /// Migrate existing official Codex sessions into the shared bucket
    #[command(name = "migrate-existing")]
    MigrateExisting,
    /// Restore migrated official Codex sessions from backups
    Restore,
}

pub fn execute(cmd: SettingsCommand) -> Result<(), AppError> {
    match cmd {
        SettingsCommand::Show { json } => show_settings(json),
        SettingsCommand::Language { language } => language_cmd(language),
        SettingsCommand::VisibleApps(cmd) => visible_apps_cmd(cmd),
        SettingsCommand::ClaudeOnboarding(cmd) => claude_onboarding_cmd(cmd),
        SettingsCommand::ClaudePlugin(cmd) => claude_plugin_cmd(cmd),
        SettingsCommand::CodexHistory(cmd) => codex_history_cmd(cmd),
    }
}

fn show_settings(json_output: bool) -> Result<(), AppError> {
    let settings = crate::settings::get_settings();
    if json_output {
        let payload = json!({
            "language": settings.language.as_deref().unwrap_or(Language::English.code()),
            "visibleApps": settings.visible_apps,
            "visibleAppsMode": settings.visible_apps_settings.mode,
            "skipClaudeOnboarding": settings.skip_claude_onboarding,
            "enableClaudePluginIntegration": settings.enable_claude_plugin_integration,
            "unifyCodexSessionHistory": settings.unify_codex_session_history,
            "unifyCodexMigrateExisting": settings.unify_codex_migrate_existing.unwrap_or(false),
            "hasCodexHistoryUnifyBackup": crate::codex_history_migration::has_codex_official_history_unify_backup(),
            "openclawConfigDir": settings.openclaw_config_dir,
        });
        println!(
            "{}",
            to_json(&payload).map_err(|err| AppError::Message(err.to_string()))?
        );
        return Ok(());
    }

    println!("{}", highlight("Settings"));
    println!("Language: {}", i18n::current_language().code());
    print_visible_apps_summary();
    println!(
        "Skip Claude onboarding: {}",
        yes_no(settings.skip_claude_onboarding)
    );
    println!(
        "Claude plugin integration: {}",
        yes_no(settings.enable_claude_plugin_integration)
    );
    println!(
        "Unified Codex session history: {}",
        yes_no(settings.unify_codex_session_history)
    );
    println!(
        "OpenClaw config dir: {}",
        settings
            .openclaw_config_dir
            .as_deref()
            .unwrap_or("(default)")
    );
    Ok(())
}

fn language_cmd(language: Option<LanguageArg>) -> Result<(), AppError> {
    let Some(language) = language else {
        println!("Language: {}", i18n::current_language().code());
        return Ok(());
    };

    let language = Language::from(language);
    i18n::set_language(language)?;
    println!(
        "{}",
        success(&format!("Language set to {}", language.display_name()))
    );
    Ok(())
}

fn visible_apps_cmd(cmd: VisibleAppsCommand) -> Result<(), AppError> {
    match cmd {
        VisibleAppsCommand::Show { json } => show_visible_apps(json),
        VisibleAppsCommand::Mode { mode } => set_visible_apps_mode(mode.into()),
        VisibleAppsCommand::Enable { app } => mutate_visible_app(app, true),
        VisibleAppsCommand::Disable { app } => mutate_visible_app(app, false),
        VisibleAppsCommand::Set { apps } => set_visible_apps_list(apps),
    }
}

fn show_visible_apps(json_output: bool) -> Result<(), AppError> {
    let settings = crate::settings::get_settings();
    if json_output {
        let payload = json!({
            "mode": settings.visible_apps_settings.mode,
            "apps": settings.visible_apps,
            "enabled": enabled_app_labels(&settings.visible_apps),
        });
        println!(
            "{}",
            to_json(&payload).map_err(|err| AppError::Message(err.to_string()))?
        );
        return Ok(());
    }

    print_visible_apps_summary();
    Ok(())
}

fn set_visible_apps_mode(mode: crate::settings::VisibleAppsMode) -> Result<(), AppError> {
    crate::settings::set_visible_apps_mode(mode)?;
    if mode == crate::settings::VisibleAppsMode::Auto {
        let detection = crate::services::visible_apps::detect_visible_app_installation();
        let outcome = crate::services::visible_apps::apply_startup_policy(&detection)?;
        for notice in outcome.notices {
            println!(
                "{}",
                info(&crate::services::visible_apps::notice_message(&notice))
            );
        }
    }

    println!(
        "{}",
        success(&format!(
            "Visible apps mode set to {}",
            visible_apps_mode_label(mode)
        ))
    );
    Ok(())
}

fn mutate_visible_app(app: AppType, enabled: bool) -> Result<(), AppError> {
    let mut visible_apps = crate::settings::get_visible_apps();
    visible_apps.set_enabled_for(&app, enabled);
    save_manual_visible_apps(visible_apps)?;
    println!(
        "{}",
        success(&format!(
            "{} {}",
            if enabled { "Enabled" } else { "Disabled" },
            app.as_str()
        ))
    );
    Ok(())
}

fn set_visible_apps_list(apps: Vec<AppType>) -> Result<(), AppError> {
    let mut visible_apps = crate::settings::VisibleApps {
        claude: false,
        codex: false,
        gemini: false,
        opencode: false,
        hermes: false,
        openclaw: false,
    };
    for app in apps {
        visible_apps.set_enabled_for(&app, true);
    }
    save_manual_visible_apps(visible_apps)?;
    println!("{}", success("Visible apps updated"));
    Ok(())
}

fn save_manual_visible_apps(visible_apps: crate::settings::VisibleApps) -> Result<(), AppError> {
    visible_apps.validate()?;
    let mut settings = crate::settings::get_settings();
    settings.visible_apps = visible_apps;
    settings.visible_apps_settings.mode = crate::settings::VisibleAppsMode::Manual;
    settings.visible_apps_settings.auto_prompt_decided = true;
    crate::settings::update_settings(settings)
}

fn claude_onboarding_cmd(cmd: ClaudeOnboardingCommand) -> Result<(), AppError> {
    match cmd {
        ClaudeOnboardingCommand::Show => {
            println!(
                "Skip Claude onboarding: {}",
                yes_no(crate::settings::get_skip_claude_onboarding())
            );
            Ok(())
        }
        ClaudeOnboardingCommand::Skip => set_skip_claude_onboarding(true),
        ClaudeOnboardingCommand::Require => set_skip_claude_onboarding(false),
    }
}

fn set_skip_claude_onboarding(enabled: bool) -> Result<(), AppError> {
    crate::settings::set_skip_claude_onboarding(enabled)?;
    println!(
        "{}",
        success(&format!(
            "Skip Claude onboarding {}",
            if enabled { "enabled" } else { "disabled" }
        ))
    );
    Ok(())
}

fn claude_plugin_cmd(cmd: ClaudePluginCommand) -> Result<(), AppError> {
    match cmd {
        ClaudePluginCommand::Show => {
            println!(
                "Claude plugin integration: {}",
                yes_no(crate::settings::get_enable_claude_plugin_integration())
            );
            Ok(())
        }
        ClaudePluginCommand::Enable => set_claude_plugin_integration(true),
        ClaudePluginCommand::Disable => set_claude_plugin_integration(false),
    }
}

fn set_claude_plugin_integration(enabled: bool) -> Result<(), AppError> {
    crate::settings::set_enable_claude_plugin_integration(enabled)?;
    if let Err(err) = crate::claude_plugin::sync_claude_plugin_on_settings_toggle(enabled) {
        println!(
            "{}",
            warning(&format!(
                "Claude plugin integration setting saved, but plugin sync failed: {err}"
            ))
        );
    }
    println!(
        "{}",
        success(&format!(
            "Claude plugin integration {}",
            if enabled { "enabled" } else { "disabled" }
        ))
    );
    Ok(())
}

fn codex_history_cmd(cmd: CodexHistoryCommand) -> Result<(), AppError> {
    match cmd {
        CodexHistoryCommand::Show { json } => show_codex_history(json),
        CodexHistoryCommand::Enable { migrate_existing } => {
            set_codex_history_enabled(true, migrate_existing, false)
        }
        CodexHistoryCommand::Disable { restore } => {
            set_codex_history_enabled(false, false, restore)
        }
        CodexHistoryCommand::MigrateExisting => migrate_codex_history_existing(),
        CodexHistoryCommand::Restore => restore_codex_history(),
    }
}

fn show_codex_history(json_output: bool) -> Result<(), AppError> {
    let settings = crate::settings::get_settings();
    let has_backup = crate::codex_history_migration::has_codex_official_history_unify_backup();
    let migration = settings
        .local_migrations
        .as_ref()
        .and_then(|migrations| migrations.codex_official_history_unify_v1.as_ref());

    if json_output {
        let payload = json!({
            "enabled": settings.unify_codex_session_history,
            "migrateExistingRequested": settings.unify_codex_migrate_existing.unwrap_or(false),
            "hasBackup": has_backup,
            "migration": migration,
        });
        println!(
            "{}",
            to_json(&payload).map_err(|err| AppError::Message(err.to_string()))?
        );
        return Ok(());
    }

    println!("{}", highlight("Codex History"));
    println!(
        "Unified session history: {}",
        yes_no(settings.unify_codex_session_history)
    );
    println!(
        "Migrate existing requested: {}",
        yes_no(settings.unify_codex_migrate_existing.unwrap_or(false))
    );
    println!("Restore backup available: {}", yes_no(has_backup));
    if let Some(migration) = migration {
        println!(
            "Last migration: jsonl_files={}, state_rows={}",
            migration.migrated_jsonl_files, migration.migrated_state_rows
        );
    }
    Ok(())
}

fn set_codex_history_enabled(
    enabled: bool,
    migrate_existing: bool,
    restore: bool,
) -> Result<(), AppError> {
    let existing = crate::settings::get_settings();
    let changed = existing.unify_codex_session_history != enabled;
    if !changed {
        println!(
            "{}",
            info(&format!(
                "Unified Codex session history already {}",
                if enabled { "enabled" } else { "disabled" }
            ))
        );
        return Ok(());
    }

    let mut next = existing.clone();
    next.unify_codex_session_history = enabled;
    next.unify_codex_migrate_existing = if enabled && migrate_existing {
        Some(true)
    } else {
        None
    };

    crate::settings::update_settings(next)?;
    let state = match crate::store::AppState::try_new() {
        Ok(state) => state,
        Err(err) => {
            rollback_codex_history_settings(&existing);
            return Err(AppError::Message(format!(
                "Unified Codex session history setting was rolled back because app state initialization failed: {err}"
            )));
        }
    };
    if let Err(err) = crate::services::provider::reapply_current_codex_official_live(&state) {
        rollback_codex_history_settings(&existing);
        return Err(AppError::Message(format!(
            "Unified Codex session history setting was rolled back because live config rewrite failed: {err}"
        )));
    }

    if enabled {
        if migrate_existing {
            let outcome =
                crate::codex_history_migration::maybe_migrate_codex_official_history_to_unified_bucket()?;
            print_codex_history_migration_outcome(&outcome);
        }
        println!("{}", success("Unified Codex session history enabled"));
    } else {
        crate::settings::clear_codex_official_history_unify_migration()?;
        crate::settings::clear_codex_unify_migrate_existing()?;
        if restore {
            restore_codex_history()?;
        }
        println!("{}", success("Unified Codex session history disabled"));
    }

    Ok(())
}

fn rollback_codex_history_settings(existing: &crate::settings::AppSettings) {
    if let Err(err) = crate::settings::update_settings(existing.clone()) {
        log::error!("Failed to roll back unified Codex session history setting: {err}");
    }
}

fn migrate_codex_history_existing() -> Result<(), AppError> {
    let mut settings = crate::settings::get_settings();
    if !settings.unify_codex_session_history {
        return Err(AppError::InvalidInput(
            "Enable unified Codex session history before migrating existing sessions".to_string(),
        ));
    }
    settings.unify_codex_migrate_existing = Some(true);
    crate::settings::update_settings(settings)?;

    let state = crate::store::AppState::try_new()?;
    crate::services::provider::reapply_current_codex_official_live(&state)?;
    let outcome =
        crate::codex_history_migration::maybe_migrate_codex_official_history_to_unified_bucket()?;
    print_codex_history_migration_outcome(&outcome);
    Ok(())
}

fn restore_codex_history() -> Result<(), AppError> {
    let outcome = crate::codex_history_migration::restore_codex_official_history_from_backups()?;
    if let Some(reason) = outcome.skipped_reason {
        println!(
            "{}",
            info(&format!("Codex official history restore skipped: {reason}"))
        );
    } else {
        println!(
            "{}",
            success(&format!(
                "Codex official history restored: jsonl_files={}, state_rows={}",
                outcome.restored_jsonl_files, outcome.restored_state_rows
            ))
        );
    }
    Ok(())
}

fn print_codex_history_migration_outcome(
    outcome: &crate::codex_history_migration::CodexHistoryProviderBucketMigrationOutcome,
) {
    if let Some(reason) = outcome.skipped_reason.as_deref() {
        println!(
            "{}",
            info(&format!(
                "Codex official history migration skipped: {reason}"
            ))
        );
    } else {
        println!(
            "{}",
            success(&format!(
                "Codex official history migrated: jsonl_files={}, state_rows={}",
                outcome.migrated_jsonl_files, outcome.migrated_state_rows
            ))
        );
    }
}

fn print_visible_apps_summary() {
    let settings = crate::settings::get_settings();
    println!(
        "Visible apps mode: {}",
        visible_apps_mode_label(settings.visible_apps_settings.mode)
    );
    println!(
        "Visible apps: {}",
        enabled_app_labels(&settings.visible_apps).join(", ")
    );
}

fn visible_apps_mode_label(mode: crate::settings::VisibleAppsMode) -> &'static str {
    match mode {
        crate::settings::VisibleAppsMode::Auto => "auto",
        crate::settings::VisibleAppsMode::Manual => "manual",
    }
}

fn enabled_app_labels(visible_apps: &crate::settings::VisibleApps) -> Vec<&'static str> {
    AppType::all()
        .filter(|app| visible_apps.is_enabled_for(app))
        .map(|app| app.as_str())
        .collect()
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

#[cfg(test)]
mod tests {
    use super::save_manual_visible_apps;
    use crate::settings::{VisibleApps, VisibleAppsMode};
    use crate::test_support::{
        lock_test_home_and_settings, set_test_home_override, TestHomeSettingsLock,
    };
    use serial_test::serial;
    use std::path::Path;
    use tempfile::TempDir;

    struct SettingsTestGuard {
        _lock: TestHomeSettingsLock,
        _temp: TempDir,
    }

    impl SettingsTestGuard {
        fn new() -> Self {
            let lock = lock_test_home_and_settings();
            let temp = tempfile::tempdir().expect("create temp dir");
            set_test_home_override(Some(temp.path()));
            crate::settings::reload_test_settings();
            Self {
                _lock: lock,
                _temp: temp,
            }
        }
    }

    impl Drop for SettingsTestGuard {
        fn drop(&mut self) {
            set_test_home_override(None::<&Path>);
            crate::settings::reload_test_settings();
        }
    }

    #[test]
    #[serial(home_settings)]
    fn settings_visible_apps_manual_save_switches_mode_and_marks_prompt_decided() {
        let _guard = SettingsTestGuard::new();
        crate::settings::set_visible_apps_mode(VisibleAppsMode::Auto).expect("set auto mode");

        save_manual_visible_apps(VisibleApps {
            claude: true,
            codex: false,
            gemini: true,
            opencode: false,
            hermes: false,
            openclaw: false,
        })
        .expect("save manual visible apps");

        let settings = crate::settings::get_settings();
        assert_eq!(settings.visible_apps_settings.mode, VisibleAppsMode::Manual);
        assert!(settings.visible_apps_settings.auto_prompt_decided);
        assert!(settings.visible_apps.claude);
        assert!(settings.visible_apps.gemini);
        assert!(!settings.visible_apps.codex);
    }

    #[test]
    #[serial(home_settings)]
    fn settings_visible_apps_manual_save_rejects_empty_selection() {
        let _guard = SettingsTestGuard::new();

        let err = save_manual_visible_apps(VisibleApps {
            claude: false,
            codex: false,
            gemini: false,
            opencode: false,
            hermes: false,
            openclaw: false,
        })
        .expect_err("empty visible apps should be rejected");

        assert!(err.to_string().contains("At least one app"));
    }
}
