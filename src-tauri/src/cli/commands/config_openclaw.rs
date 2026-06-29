use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::app_config::AppType;
use crate::cli::openclaw_form_normalization::{normalize_tools_config, OpenClawAgentsFormLike};
use crate::cli::ui::{create_table, highlight, info, success, to_json, warning};
use crate::commands::workspace;
use crate::error::AppError;
use crate::openclaw_config::{
    get_agents_defaults, get_env_config, get_openclaw_config_path, get_openclaw_dir,
    get_tools_config, scan_openclaw_config_health, set_agents_defaults, set_env_config,
    set_tools_config, OpenClawAgentsDefaults, OpenClawEnvConfig, OpenClawHealthWarning,
    OpenClawToolsConfig, OpenClawWriteOutcome,
};

#[derive(Subcommand, Debug, Clone)]
pub enum OpenClawCommand {
    /// Show OpenClaw config paths
    Path,

    /// Check OpenClaw config health
    Health {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Manage OpenClaw config directory override
    #[command(subcommand)]
    Dir(OpenClawDirCommand),

    /// Manage OpenClaw env config
    #[command(subcommand)]
    Env(OpenClawEnvCommand),

    /// Manage OpenClaw tools config
    #[command(subcommand)]
    Tools(OpenClawToolsCommand),

    /// Manage OpenClaw agents defaults
    #[command(subcommand)]
    Agents(OpenClawAgentsCommand),

    /// Manage OpenClaw workspace files
    #[command(subcommand)]
    Workspace(OpenClawWorkspaceCommand),

    /// Manage OpenClaw daily memory files
    #[command(subcommand)]
    Memory(OpenClawMemoryCommand),
}

#[derive(Subcommand, Debug, Clone)]
pub enum OpenClawDirCommand {
    /// Show current OpenClaw config directory
    Show {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Set OpenClaw config directory override
    Set {
        /// Directory containing openclaw.json
        path: PathBuf,
    },
    /// Clear OpenClaw config directory override
    Clear,
}

#[derive(Subcommand, Debug, Clone)]
pub enum OpenClawEnvCommand {
    /// Show OpenClaw env config
    Show {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Replace OpenClaw env config from JSON
    Set(JsonInputArgs),
    /// Set one env key to a JSON value, or a string when parsing fails
    Put { key: String, value: String },
    /// Remove one env key
    Unset { key: String },
}

#[derive(Subcommand, Debug, Clone)]
pub enum OpenClawToolsCommand {
    /// Show OpenClaw tools config
    Show {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Replace OpenClaw tools config from JSON
    Set(JsonInputArgs),
    /// Set or clear tools profile
    Profile {
        /// Profile value supported by the TUI
        #[arg(value_enum, conflicts_with = "clear")]
        profile: Option<OpenClawToolsProfile>,

        /// Clear the profile field
        #[arg(long)]
        clear: bool,
    },
    /// Manage tools allow rules
    #[command(subcommand)]
    Allow(OpenClawRuleListCommand),
    /// Manage tools deny rules
    #[command(subcommand)]
    Deny(OpenClawRuleListCommand),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OpenClawToolsProfile {
    Minimal,
    Coding,
    Messaging,
    Full,
}

#[derive(Subcommand, Debug, Clone)]
pub enum OpenClawRuleListCommand {
    /// List rules
    List {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Add a rule
    Add { rule: String },
    /// Remove a rule by exact value
    Remove { rule: String },
    /// Set a rule by 1-based list index
    #[command(name = "set-at")]
    SetAt { index: usize, rule: String },
    /// Insert a rule before a 1-based list index; use len+1 to append
    #[command(name = "insert-at")]
    InsertAt { index: usize, rule: String },
    /// Remove one rule by 1-based list index
    #[command(name = "remove-at")]
    RemoveAt { index: usize },
    /// Clear all rules
    Clear,
}

#[derive(Subcommand, Debug, Clone)]
pub enum OpenClawAgentsCommand {
    /// Show OpenClaw agents defaults
    Show {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Replace OpenClaw agents defaults from JSON
    Set(JsonInputArgs),
    /// Set or clear primary model
    Primary {
        /// Primary model id
        #[arg(conflicts_with = "clear")]
        model: Option<String>,

        /// Clear primary model
        #[arg(long)]
        clear: bool,
    },
    /// Manage fallback model list
    #[command(subcommand)]
    Fallback(OpenClawFallbackCommand),
    /// Manage runtime defaults
    #[command(subcommand)]
    Runtime(OpenClawAgentsRuntimeCommand),
}

#[derive(Subcommand, Debug, Clone)]
pub enum OpenClawFallbackCommand {
    /// List fallback models
    List {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Add a fallback model
    Add { model: String },
    /// Remove a fallback model by exact value
    Remove { model: String },
    /// Set a fallback model by 1-based list index
    #[command(name = "set-at")]
    SetAt { index: usize, model: String },
    /// Insert a fallback model before a 1-based list index; use len+1 to append
    #[command(name = "insert-at")]
    InsertAt { index: usize, model: String },
    /// Remove one fallback model by 1-based list index
    #[command(name = "remove-at")]
    RemoveAt { index: usize },
    /// Clear all fallback models
    Clear,
}

#[derive(Subcommand, Debug, Clone)]
pub enum OpenClawAgentsRuntimeCommand {
    /// Set a runtime field
    Set {
        #[arg(value_enum)]
        field: OpenClawRuntimeField,
        value: String,
    },
    /// Unset a runtime field
    Unset {
        #[arg(value_enum)]
        field: OpenClawRuntimeField,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OpenClawRuntimeField {
    Workspace,
    TimeoutSeconds,
    ContextTokens,
    MaxConcurrent,
}

#[derive(Subcommand, Debug, Clone)]
pub enum OpenClawWorkspaceCommand {
    /// List allowed OpenClaw workspace files
    List {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Check whether an allowed workspace file exists
    Exists {
        filename: String,

        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Show workspace file content
    Show { filename: String },
    /// Write workspace file content, reading stdin when --content is omitted
    Set {
        filename: String,

        /// Inline file content
        #[arg(long)]
        content: Option<String>,
    },
    /// Open the OpenClaw workspace directory in the system file manager
    #[command(name = "open-dir")]
    OpenDir,
}

#[derive(Subcommand, Debug, Clone)]
pub enum OpenClawMemoryCommand {
    /// List daily memory files
    List {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Show daily memory file content
    Show { filename: String },
    /// Write daily memory content, reading stdin when --content is omitted
    Set {
        filename: String,

        /// Inline file content
        #[arg(long)]
        content: Option<String>,
    },
    /// Search daily memory files
    Search {
        query: String,

        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Delete a daily memory file
    Delete {
        filename: String,

        /// Confirm deletion without prompting
        #[arg(long)]
        yes: bool,
    },
    /// Open the OpenClaw daily memory directory in the system file manager
    #[command(name = "open-dir")]
    OpenDir,
}

#[derive(Args, Debug, Clone)]
pub struct JsonInputArgs {
    /// Inline JSON object
    #[arg(long = "json", value_name = "JSON", conflicts_with = "file")]
    json: Option<String>,

    /// Read JSON object from file
    #[arg(long, conflicts_with = "json")]
    file: Option<PathBuf>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenClawPathSummary {
    config_dir: String,
    config_path: String,
    override_dir: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenClawHealthSummary {
    config_dir: String,
    config_path: String,
    warnings: Vec<OpenClawHealthWarning>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceFileInfo {
    filename: &'static str,
    exists: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceExistsSummary {
    filename: String,
    exists: bool,
}

pub fn execute(cmd: OpenClawCommand) -> Result<(), AppError> {
    match cmd {
        OpenClawCommand::Path => show_path(),
        OpenClawCommand::Health { json } => show_health(json),
        OpenClawCommand::Dir(cmd) => execute_dir(cmd),
        OpenClawCommand::Env(cmd) => execute_env(cmd),
        OpenClawCommand::Tools(cmd) => execute_tools(cmd),
        OpenClawCommand::Agents(cmd) => execute_agents(cmd),
        OpenClawCommand::Workspace(cmd) => execute_workspace(cmd),
        OpenClawCommand::Memory(cmd) => execute_memory(cmd),
    }
}

fn execute_dir(cmd: OpenClawDirCommand) -> Result<(), AppError> {
    match cmd {
        OpenClawDirCommand::Show { json } => show_dir(json),
        OpenClawDirCommand::Set { path } => set_dir(Some(path.display().to_string())),
        OpenClawDirCommand::Clear => set_dir(None),
    }
}

fn execute_env(cmd: OpenClawEnvCommand) -> Result<(), AppError> {
    match cmd {
        OpenClawEnvCommand::Show { json } => show_env(json),
        OpenClawEnvCommand::Set(args) => set_env(args),
        OpenClawEnvCommand::Put { key, value } => put_env(&key, &value),
        OpenClawEnvCommand::Unset { key } => unset_env(&key),
    }
}

fn execute_tools(cmd: OpenClawToolsCommand) -> Result<(), AppError> {
    match cmd {
        OpenClawToolsCommand::Show { json } => show_tools(json),
        OpenClawToolsCommand::Set(args) => set_tools(args),
        OpenClawToolsCommand::Profile { profile, clear } => set_tools_profile(profile, clear),
        OpenClawToolsCommand::Allow(cmd) => execute_rule_list(ToolsRuleKind::Allow, cmd),
        OpenClawToolsCommand::Deny(cmd) => execute_rule_list(ToolsRuleKind::Deny, cmd),
    }
}

fn execute_agents(cmd: OpenClawAgentsCommand) -> Result<(), AppError> {
    match cmd {
        OpenClawAgentsCommand::Show { json } => show_agents(json),
        OpenClawAgentsCommand::Set(args) => set_agents(args),
        OpenClawAgentsCommand::Primary { model, clear } => set_agents_primary(model, clear),
        OpenClawAgentsCommand::Fallback(cmd) => execute_fallback(cmd),
        OpenClawAgentsCommand::Runtime(cmd) => execute_runtime(cmd),
    }
}

fn execute_workspace(cmd: OpenClawWorkspaceCommand) -> Result<(), AppError> {
    match cmd {
        OpenClawWorkspaceCommand::List { json } => list_workspace(json),
        OpenClawWorkspaceCommand::Exists { filename, json } => workspace_exists(&filename, json),
        OpenClawWorkspaceCommand::Show { filename } => show_workspace_file(&filename),
        OpenClawWorkspaceCommand::Set { filename, content } => {
            set_workspace_file(&filename, content)
        }
        OpenClawWorkspaceCommand::OpenDir => open_workspace_dir(),
    }
}

fn execute_memory(cmd: OpenClawMemoryCommand) -> Result<(), AppError> {
    match cmd {
        OpenClawMemoryCommand::List { json } => list_memory(json),
        OpenClawMemoryCommand::Show { filename } => show_memory_file(&filename),
        OpenClawMemoryCommand::Set { filename, content } => set_memory_file(&filename, content),
        OpenClawMemoryCommand::Search { query, json } => search_memory(&query, json),
        OpenClawMemoryCommand::Delete { filename, yes } => delete_memory(&filename, yes),
        OpenClawMemoryCommand::OpenDir => open_memory_dir(),
    }
}

fn path_summary() -> OpenClawPathSummary {
    OpenClawPathSummary {
        config_dir: get_openclaw_dir().display().to_string(),
        config_path: get_openclaw_config_path().display().to_string(),
        override_dir: crate::settings::get_settings().openclaw_config_dir,
    }
}

fn show_path() -> Result<(), AppError> {
    let summary = path_summary();

    println!("{}", highlight("OpenClaw Configuration Paths"));
    println!("{}", "=".repeat(50));
    println!("Config dir:   {}", summary.config_dir);
    println!("Config file:  {}", summary.config_path);
    println!(
        "Override dir: {}",
        summary.override_dir.as_deref().unwrap_or("N/A")
    );
    println!(
        "Exists:       {}",
        if Path::new(&summary.config_path).exists() {
            "yes"
        } else {
            "no"
        }
    );

    Ok(())
}

fn show_health(json: bool) -> Result<(), AppError> {
    let summary = OpenClawHealthSummary {
        config_dir: get_openclaw_dir().display().to_string(),
        config_path: get_openclaw_config_path().display().to_string(),
        warnings: openclaw_health_warnings()?,
    };

    if json {
        print_json(&summary)?;
        return Ok(());
    }

    println!("{}", highlight("OpenClaw Config Health"));
    println!("{}", "=".repeat(50));
    println!("Config dir:  {}", summary.config_dir);
    println!("Config file: {}", summary.config_path);
    if summary.warnings.is_empty() {
        println!("{}", success("No warnings."));
    } else {
        print_warnings(&summary.warnings);
    }

    Ok(())
}

fn openclaw_health_warnings() -> Result<Vec<OpenClawHealthWarning>, AppError> {
    let mut warnings = scan_openclaw_config_health()?;
    collect_openclaw_slice_warning("env", get_env_config, &mut warnings)?;
    collect_openclaw_slice_warning("tools", get_tools_config, &mut warnings)?;
    collect_openclaw_slice_warning("agents.defaults", get_agents_defaults, &mut warnings)?;
    Ok(warnings)
}

fn collect_openclaw_slice_warning<T, F>(
    warning_path: &'static str,
    loader: F,
    warnings: &mut Vec<OpenClawHealthWarning>,
) -> Result<(), AppError>
where
    F: FnOnce() -> Result<T, AppError>,
{
    match loader() {
        Ok(_) => Ok(()),
        Err(AppError::Config(message)) => {
            warnings.push(OpenClawHealthWarning {
                code: "config_parse_failed".to_string(),
                message,
                path: Some(warning_path.to_string()),
            });
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn show_dir(json: bool) -> Result<(), AppError> {
    let summary = path_summary();
    if json {
        print_json(&summary)?;
        return Ok(());
    }

    println!("{}", highlight("OpenClaw Config Directory"));
    println!("{}", "=".repeat(50));
    println!("Effective: {}", summary.config_dir);
    println!(
        "Override:  {}",
        summary.override_dir.as_deref().unwrap_or("N/A")
    );
    Ok(())
}

fn set_dir(path: Option<String>) -> Result<(), AppError> {
    let mut settings = crate::settings::get_settings();
    settings.openclaw_config_dir = path;
    crate::settings::update_settings(settings)?;

    println!("{}", success("OpenClaw config directory saved."));

    if crate::sync_policy::should_sync_live(&AppType::OpenClaw) {
        let state = crate::store::AppState::try_new()?;
        if let Err(err) = crate::services::ProviderService::sync_openclaw_to_live(&state) {
            println!(
                "{}",
                warning(&format!(
                    "OpenClaw live config sync failed after directory change: {err}"
                ))
            );
        }
    } else {
        println!("{}", warning("OpenClaw live config sync skipped."));
    }

    Ok(())
}

fn show_env(json: bool) -> Result<(), AppError> {
    let env = get_env_config()?;
    if json {
        print_json(&env)?;
    } else {
        print_json(&env.vars)?;
    }
    Ok(())
}

fn set_env(args: JsonInputArgs) -> Result<(), AppError> {
    let env: OpenClawEnvConfig = read_json_arg(args, "OpenClaw env JSON is required.")?;
    let outcome = set_env_config(&env)?;
    print_write_outcome("OpenClaw env config saved.", &outcome);
    Ok(())
}

fn put_env(key: &str, raw_value: &str) -> Result<(), AppError> {
    let key = non_empty("key", key)?;
    let mut env = get_env_config()?;
    env.vars
        .insert(key.to_string(), parse_json_value_or_string(raw_value));
    let outcome = set_env_config(&env)?;
    print_write_outcome("OpenClaw env config saved.", &outcome);
    Ok(())
}

fn unset_env(key: &str) -> Result<(), AppError> {
    let key = non_empty("key", key)?;
    let mut env = get_env_config()?;
    env.vars.remove(key);
    let outcome = set_env_config(&env)?;
    print_write_outcome("OpenClaw env config saved.", &outcome);
    Ok(())
}

fn show_tools(json: bool) -> Result<(), AppError> {
    let tools = get_tools_config()?;
    if json {
        print_json(&tools)?;
        return Ok(());
    }

    println!("{}", highlight("OpenClaw Tools"));
    println!("{}", "=".repeat(50));
    println!("Profile: {}", tools.profile.as_deref().unwrap_or("N/A"));
    print_string_list("Allow", &tools.allow);
    print_string_list("Deny", &tools.deny);
    if !tools.extra.is_empty() {
        println!();
        println!("Extra:");
        print_json(&tools.extra)?;
    }
    Ok(())
}

fn set_tools(args: JsonInputArgs) -> Result<(), AppError> {
    let tools: OpenClawToolsConfig = read_json_arg(args, "OpenClaw tools JSON is required.")?;
    let tools = normalize_tools_config(&tools);
    let outcome = set_tools_config(&tools)?;
    print_write_outcome("OpenClaw tools config saved.", &outcome);
    Ok(())
}

fn set_tools_profile(profile: Option<OpenClawToolsProfile>, clear: bool) -> Result<(), AppError> {
    if profile.is_none() && !clear {
        return Err(AppError::InvalidInput(
            "Provide a profile value or --clear.".to_string(),
        ));
    }

    let mut tools = get_tools_config()?;
    tools.profile = if clear {
        None
    } else {
        profile.map(|profile| profile.as_str().to_string())
    };
    let tools = normalize_tools_config(&tools);
    let outcome = set_tools_config(&tools)?;
    print_write_outcome("OpenClaw tools config saved.", &outcome);
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum ToolsRuleKind {
    Allow,
    Deny,
}

fn execute_rule_list(kind: ToolsRuleKind, cmd: OpenClawRuleListCommand) -> Result<(), AppError> {
    match cmd {
        OpenClawRuleListCommand::List { json } => list_rules(kind, json),
        OpenClawRuleListCommand::Add { rule } => add_rule(kind, &rule),
        OpenClawRuleListCommand::Remove { rule } => remove_rule(kind, &rule),
        OpenClawRuleListCommand::SetAt { index, rule } => set_rule_at(kind, index, &rule),
        OpenClawRuleListCommand::InsertAt { index, rule } => insert_rule_at(kind, index, &rule),
        OpenClawRuleListCommand::RemoveAt { index } => remove_rule_at(kind, index),
        OpenClawRuleListCommand::Clear => clear_rules(kind),
    }
}

fn list_rules(kind: ToolsRuleKind, json: bool) -> Result<(), AppError> {
    let tools = get_tools_config()?;
    let rules = rule_list(&tools, kind);
    if json {
        print_json(rules)?;
    } else {
        print_indexed_string_list(rule_label(kind), rules);
    }
    Ok(())
}

fn add_rule(kind: ToolsRuleKind, rule: &str) -> Result<(), AppError> {
    let rule = non_empty("rule", rule)?.to_string();
    let mut tools = get_tools_config()?;
    rule_list_mut(&mut tools, kind).push(rule);
    let tools = normalize_tools_config(&tools);
    let outcome = set_tools_config(&tools)?;
    print_write_outcome("OpenClaw tools config saved.", &outcome);
    Ok(())
}

fn remove_rule(kind: ToolsRuleKind, rule: &str) -> Result<(), AppError> {
    let rule = non_empty("rule", rule)?;
    let mut tools = get_tools_config()?;
    rule_list_mut(&mut tools, kind).retain(|value| value != rule);
    let tools = normalize_tools_config(&tools);
    let outcome = set_tools_config(&tools)?;
    print_write_outcome("OpenClaw tools config saved.", &outcome);
    Ok(())
}

fn set_rule_at(kind: ToolsRuleKind, index: usize, rule: &str) -> Result<(), AppError> {
    let rule = non_empty("rule", rule)?.to_string();
    let mut tools = get_tools_config()?;
    set_list_item_at(
        rule_list_mut(&mut tools, kind),
        index,
        rule,
        rule_label(kind),
    )?;
    let tools = normalize_tools_config(&tools);
    let outcome = set_tools_config(&tools)?;
    print_write_outcome("OpenClaw tools config saved.", &outcome);
    Ok(())
}

fn insert_rule_at(kind: ToolsRuleKind, index: usize, rule: &str) -> Result<(), AppError> {
    let rule = non_empty("rule", rule)?.to_string();
    let mut tools = get_tools_config()?;
    insert_list_item_at(
        rule_list_mut(&mut tools, kind),
        index,
        rule,
        rule_label(kind),
    )?;
    let tools = normalize_tools_config(&tools);
    let outcome = set_tools_config(&tools)?;
    print_write_outcome("OpenClaw tools config saved.", &outcome);
    Ok(())
}

fn remove_rule_at(kind: ToolsRuleKind, index: usize) -> Result<(), AppError> {
    let mut tools = get_tools_config()?;
    remove_list_item_at(rule_list_mut(&mut tools, kind), index, rule_label(kind))?;
    let tools = normalize_tools_config(&tools);
    let outcome = set_tools_config(&tools)?;
    print_write_outcome("OpenClaw tools config saved.", &outcome);
    Ok(())
}

fn clear_rules(kind: ToolsRuleKind) -> Result<(), AppError> {
    let mut tools = get_tools_config()?;
    rule_list_mut(&mut tools, kind).clear();
    let tools = normalize_tools_config(&tools);
    let outcome = set_tools_config(&tools)?;
    print_write_outcome("OpenClaw tools config saved.", &outcome);
    Ok(())
}

fn rule_list(tools: &OpenClawToolsConfig, kind: ToolsRuleKind) -> &Vec<String> {
    match kind {
        ToolsRuleKind::Allow => &tools.allow,
        ToolsRuleKind::Deny => &tools.deny,
    }
}

fn rule_list_mut(tools: &mut OpenClawToolsConfig, kind: ToolsRuleKind) -> &mut Vec<String> {
    match kind {
        ToolsRuleKind::Allow => &mut tools.allow,
        ToolsRuleKind::Deny => &mut tools.deny,
    }
}

fn rule_label(kind: ToolsRuleKind) -> &'static str {
    match kind {
        ToolsRuleKind::Allow => "Allow",
        ToolsRuleKind::Deny => "Deny",
    }
}

fn show_agents(json: bool) -> Result<(), AppError> {
    let defaults = get_agents_defaults()?;
    if json {
        print_json(&defaults)?;
        return Ok(());
    }

    println!("{}", highlight("OpenClaw Agents Defaults"));
    println!("{}", "=".repeat(50));
    let form = OpenClawAgentsFormLike::from_snapshot(defaults.as_ref());
    println!("Primary model:  {}", blank_as_na(form.primary_model.trim()));
    print_string_list("Fallbacks", &form.fallbacks);
    println!("Workspace:      {}", blank_as_na(form.workspace.trim()));
    println!("Timeout:        {}", blank_as_na(form.timeout.trim()));
    println!(
        "Context tokens: {}",
        blank_as_na(form.context_tokens.trim())
    );
    println!(
        "Max concurrent: {}",
        blank_as_na(form.max_concurrent.trim())
    );
    if !form.defaults_extra.is_empty()
        || !form.model_extra.is_empty()
        || form.model_catalog.is_some()
    {
        println!();
        println!("Extra:");
        print_json(&defaults)?;
    }
    Ok(())
}

fn set_agents(args: JsonInputArgs) -> Result<(), AppError> {
    let defaults: OpenClawAgentsDefaults =
        read_json_arg(args, "OpenClaw agents defaults JSON is required.")?;
    let defaults = OpenClawAgentsFormLike::from_snapshot(Some(&defaults)).to_config();
    let outcome = set_agents_defaults(&defaults)?;
    print_write_outcome("OpenClaw agents defaults saved.", &outcome);
    Ok(())
}

fn set_agents_primary(model: Option<String>, clear: bool) -> Result<(), AppError> {
    if model.is_none() && !clear {
        return Err(AppError::InvalidInput(
            "Provide a primary model or --clear.".to_string(),
        ));
    }

    let defaults = get_agents_defaults()?;
    let mut form = OpenClawAgentsFormLike::from_snapshot(defaults.as_ref());
    form.primary_model = if clear {
        String::new()
    } else {
        non_empty("model", model.as_deref().unwrap_or_default())?.to_string()
    };
    save_agents_form(form)
}

fn execute_fallback(cmd: OpenClawFallbackCommand) -> Result<(), AppError> {
    match cmd {
        OpenClawFallbackCommand::List { json } => list_fallbacks(json),
        OpenClawFallbackCommand::Add { model } => add_fallback(&model),
        OpenClawFallbackCommand::Remove { model } => remove_fallback(&model),
        OpenClawFallbackCommand::SetAt { index, model } => set_fallback_at(index, &model),
        OpenClawFallbackCommand::InsertAt { index, model } => insert_fallback_at(index, &model),
        OpenClawFallbackCommand::RemoveAt { index } => remove_fallback_at(index),
        OpenClawFallbackCommand::Clear => clear_fallbacks(),
    }
}

fn list_fallbacks(json: bool) -> Result<(), AppError> {
    let defaults = get_agents_defaults()?;
    let form = OpenClawAgentsFormLike::from_snapshot(defaults.as_ref());
    if json {
        print_json(&form.fallbacks)?;
    } else {
        print_indexed_string_list("Fallbacks", &form.fallbacks);
    }
    Ok(())
}

fn add_fallback(model: &str) -> Result<(), AppError> {
    let model = non_empty("model", model)?.to_string();
    let defaults = get_agents_defaults()?;
    let mut form = OpenClawAgentsFormLike::from_snapshot(defaults.as_ref());
    form.fallbacks.push(model);
    save_agents_form(form)
}

fn remove_fallback(model: &str) -> Result<(), AppError> {
    let model = non_empty("model", model)?;
    let defaults = get_agents_defaults()?;
    let mut form = OpenClawAgentsFormLike::from_snapshot(defaults.as_ref());
    form.fallbacks.retain(|value| value != model);
    save_agents_form(form)
}

fn set_fallback_at(index: usize, model: &str) -> Result<(), AppError> {
    let model = non_empty("model", model)?.to_string();
    let defaults = get_agents_defaults()?;
    let mut form = OpenClawAgentsFormLike::from_snapshot(defaults.as_ref());
    set_list_item_at(&mut form.fallbacks, index, model, "Fallbacks")?;
    save_agents_form(form)
}

fn insert_fallback_at(index: usize, model: &str) -> Result<(), AppError> {
    let model = non_empty("model", model)?.to_string();
    let defaults = get_agents_defaults()?;
    let mut form = OpenClawAgentsFormLike::from_snapshot(defaults.as_ref());
    insert_list_item_at(&mut form.fallbacks, index, model, "Fallbacks")?;
    save_agents_form(form)
}

fn remove_fallback_at(index: usize) -> Result<(), AppError> {
    let defaults = get_agents_defaults()?;
    let mut form = OpenClawAgentsFormLike::from_snapshot(defaults.as_ref());
    remove_list_item_at(&mut form.fallbacks, index, "Fallbacks")?;
    save_agents_form(form)
}

fn clear_fallbacks() -> Result<(), AppError> {
    let defaults = get_agents_defaults()?;
    let mut form = OpenClawAgentsFormLike::from_snapshot(defaults.as_ref());
    form.fallbacks.clear();
    save_agents_form(form)
}

fn execute_runtime(cmd: OpenClawAgentsRuntimeCommand) -> Result<(), AppError> {
    match cmd {
        OpenClawAgentsRuntimeCommand::Set { field, value } => set_runtime(field, &value),
        OpenClawAgentsRuntimeCommand::Unset { field } => unset_runtime(field),
    }
}

fn set_runtime(field: OpenClawRuntimeField, value: &str) -> Result<(), AppError> {
    let value = non_empty("value", value)?.to_string();
    let defaults = get_agents_defaults()?;
    let mut form = OpenClawAgentsFormLike::from_snapshot(defaults.as_ref());
    match field {
        OpenClawRuntimeField::Workspace => form.workspace = value,
        OpenClawRuntimeField::TimeoutSeconds => form.timeout = value,
        OpenClawRuntimeField::ContextTokens => form.context_tokens = value,
        OpenClawRuntimeField::MaxConcurrent => form.max_concurrent = value,
    }
    save_agents_form_allow_legacy_timeout_migration(form)
}

fn unset_runtime(field: OpenClawRuntimeField) -> Result<(), AppError> {
    let defaults = get_agents_defaults()?;
    let mut form = OpenClawAgentsFormLike::from_snapshot(defaults.as_ref());
    match field {
        OpenClawRuntimeField::Workspace => {
            form.workspace.clear();
        }
        OpenClawRuntimeField::TimeoutSeconds => {
            form.timeout.clear();
            form.timeout_seconds_seed = None;
            form.has_legacy_timeout = false;
        }
        OpenClawRuntimeField::ContextTokens => {
            form.context_tokens.clear();
            form.context_tokens_seed = None;
        }
        OpenClawRuntimeField::MaxConcurrent => {
            form.max_concurrent.clear();
            form.max_concurrent_seed = None;
        }
    }
    save_agents_form(form)
}

fn save_agents_form(form: OpenClawAgentsFormLike) -> Result<(), AppError> {
    if form.has_unmigratable_legacy_timeout() {
        return Err(AppError::InvalidInput(
            "Legacy agents.defaults.timeout must be numeric before saving.".to_string(),
        ));
    }

    save_agents_form_allow_legacy_timeout_migration(form)
}

fn save_agents_form_allow_legacy_timeout_migration(
    form: OpenClawAgentsFormLike,
) -> Result<(), AppError> {
    let defaults = form.to_config();
    let outcome = set_agents_defaults(&defaults)?;
    print_write_outcome("OpenClaw agents defaults saved.", &outcome);
    Ok(())
}

fn list_workspace(json: bool) -> Result<(), AppError> {
    let mut files = Vec::new();
    for filename in workspace::ALLOWED_FILES {
        let exists =
            workspace::workspace_file_exists((*filename).to_string()).map_err(AppError::Message)?;
        files.push(WorkspaceFileInfo { filename, exists });
    }

    if json {
        print_json(&files)?;
        return Ok(());
    }

    let mut table = create_table();
    table.set_header(vec!["File", "Exists"]);
    for file in files {
        table.add_row(vec![
            file.filename.to_string(),
            if file.exists { "yes" } else { "no" }.to_string(),
        ]);
    }
    println!("{table}");
    Ok(())
}

fn workspace_exists(filename: &str, json: bool) -> Result<(), AppError> {
    let exists =
        workspace::workspace_file_exists(filename.to_string()).map_err(AppError::Message)?;
    if json {
        print_json(&WorkspaceExistsSummary {
            filename: filename.to_string(),
            exists,
        })?;
    } else {
        println!("{}", if exists { "yes" } else { "no" });
    }
    Ok(())
}

fn show_workspace_file(filename: &str) -> Result<(), AppError> {
    match workspace::read_workspace_file(filename.to_string()).map_err(AppError::Message)? {
        Some(content) => print!("{content}"),
        None => println!("{}", info("Workspace file does not exist.")),
    }
    Ok(())
}

fn set_workspace_file(filename: &str, content: Option<String>) -> Result<(), AppError> {
    let content = read_content_arg(content)?;
    workspace::write_workspace_file(filename.to_string(), content).map_err(AppError::Message)?;
    println!("{}", success("OpenClaw workspace file saved."));
    Ok(())
}

fn open_workspace_dir() -> Result<(), AppError> {
    workspace::open_workspace_directory((), String::new()).map_err(AppError::Message)?;
    println!("{}", success("Opened OpenClaw workspace directory."));
    Ok(())
}

fn list_memory(json: bool) -> Result<(), AppError> {
    let files = workspace::list_daily_memory_files().map_err(AppError::Message)?;
    if json {
        print_json(&files)?;
        return Ok(());
    }

    if files.is_empty() {
        println!("{}", info("No daily memory files found."));
        return Ok(());
    }

    let mut table = create_table();
    table.set_header(vec!["File", "Date", "Size", "Preview"]);
    for file in files {
        table.add_row(vec![
            file.filename,
            file.date,
            file.size_bytes.to_string(),
            file.preview,
        ]);
    }
    println!("{table}");
    Ok(())
}

fn show_memory_file(filename: &str) -> Result<(), AppError> {
    match workspace::read_daily_memory_file(filename.to_string()).map_err(AppError::Message)? {
        Some(content) => print!("{content}"),
        None => println!("{}", info("Daily memory file does not exist.")),
    }
    Ok(())
}

fn set_memory_file(filename: &str, content: Option<String>) -> Result<(), AppError> {
    let content = read_content_arg(content)?;
    workspace::write_daily_memory_file(filename.to_string(), content).map_err(AppError::Message)?;
    println!("{}", success("OpenClaw daily memory file saved."));
    Ok(())
}

fn search_memory(query: &str, json: bool) -> Result<(), AppError> {
    let results =
        workspace::search_daily_memory_files(query.to_string()).map_err(AppError::Message)?;
    if json {
        print_json(&results)?;
        return Ok(());
    }

    if results.is_empty() {
        println!("{}", info("No daily memory matches found."));
        return Ok(());
    }

    let mut table = create_table();
    table.set_header(vec!["File", "Matches", "Snippet"]);
    for result in results {
        table.add_row(vec![
            result.filename,
            result.match_count.to_string(),
            result.snippet,
        ]);
    }
    println!("{table}");
    Ok(())
}

fn delete_memory(filename: &str, yes: bool) -> Result<(), AppError> {
    if !yes {
        let confirm = inquire::Confirm::new(&format!("Delete daily memory file '{filename}'?"))
            .with_default(false)
            .prompt()
            .map_err(|e| AppError::Message(format!("Prompt failed: {e}")))?;
        if !confirm {
            println!("{}", info("Cancelled."));
            return Ok(());
        }
    }

    workspace::delete_daily_memory_file(filename.to_string()).map_err(AppError::Message)?;
    println!("{}", success("OpenClaw daily memory file deleted."));
    Ok(())
}

fn open_memory_dir() -> Result<(), AppError> {
    workspace::open_workspace_directory((), "memory".to_string()).map_err(AppError::Message)?;
    println!("{}", success("Opened OpenClaw daily memory directory."));
    Ok(())
}

fn read_json_arg<T: DeserializeOwned>(args: JsonInputArgs, missing: &str) -> Result<T, AppError> {
    let raw = match (args.json, args.file) {
        (Some(json), None) => json,
        (None, Some(path)) => fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?,
        (None, None) => return Err(AppError::InvalidInput(missing.to_string())),
        (Some(_), Some(_)) => {
            return Err(AppError::InvalidInput(
                "Use either --json or --file, not both.".to_string(),
            ))
        }
    };

    serde_json::from_str(&raw).map_err(|e| AppError::InvalidInput(format!("Invalid JSON: {e}")))
}

fn read_content_arg(content: Option<String>) -> Result<String, AppError> {
    if let Some(content) = content {
        return Ok(content);
    }

    let mut content = String::new();
    io::stdin()
        .read_to_string(&mut content)
        .map_err(|e| AppError::IoContext {
            context: "failed to read stdin".to_string(),
            source: e,
        })?;
    Ok(content)
}

fn parse_json_value_or_string(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

fn non_empty<'a>(label: &str, value: &'a str) -> Result<&'a str, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(AppError::InvalidInput(format!("{label} cannot be empty.")))
    } else {
        Ok(trimmed)
    }
}

fn set_list_item_at(
    values: &mut [String],
    index: usize,
    value: String,
    label: &str,
) -> Result<(), AppError> {
    let offset = existing_list_offset(values.len(), index, label)?;
    values[offset] = value;
    Ok(())
}

fn insert_list_item_at(
    values: &mut Vec<String>,
    index: usize,
    value: String,
    label: &str,
) -> Result<(), AppError> {
    let offset = insert_list_offset(values.len(), index, label)?;
    values.insert(offset, value);
    Ok(())
}

fn remove_list_item_at(
    values: &mut Vec<String>,
    index: usize,
    label: &str,
) -> Result<(), AppError> {
    let offset = existing_list_offset(values.len(), index, label)?;
    values.remove(offset);
    Ok(())
}

fn existing_list_offset(len: usize, index: usize, label: &str) -> Result<usize, AppError> {
    if index == 0 || index > len {
        return Err(list_index_error(label, index, len, false));
    }
    Ok(index - 1)
}

fn insert_list_offset(len: usize, index: usize, label: &str) -> Result<usize, AppError> {
    if index == 0 || index > len + 1 {
        return Err(list_index_error(label, index, len, true));
    }
    Ok(index - 1)
}

fn list_index_error(label: &str, index: usize, len: usize, insert: bool) -> AppError {
    let max = if insert { len + 1 } else { len };
    if max == 0 {
        AppError::InvalidInput(format!(
            "{label} index {index} is out of range; list is empty."
        ))
    } else {
        AppError::InvalidInput(format!(
            "{label} index {index} is out of range; expected 1..={max}."
        ))
    }
}

fn print_json<T: Serialize>(value: &T) -> Result<(), AppError> {
    println!(
        "{}",
        to_json(value).map_err(|source| AppError::JsonSerialize { source })?
    );
    Ok(())
}

fn print_string_list(label: &str, values: &[String]) {
    if values.is_empty() {
        println!("{label}: N/A");
        return;
    }

    println!("{label}:");
    for value in values {
        println!("  - {value}");
    }
}

fn print_indexed_string_list(label: &str, values: &[String]) {
    if values.is_empty() {
        println!("{label}: N/A");
        return;
    }

    println!("{label}:");
    for (index, value) in values.iter().enumerate() {
        println!("  {}. {value}", index + 1);
    }
}

fn print_write_outcome(message: &str, outcome: &OpenClawWriteOutcome) {
    println!("{}", success(message));
    if let Some(path) = outcome.backup_path.as_deref() {
        println!("{}", info(&format!("Backup: {path}")));
    }
    if !outcome.warnings.is_empty() {
        print_warnings(&outcome.warnings);
    }
}

fn print_warnings(warnings: &[OpenClawHealthWarning]) {
    for item in warnings {
        let path = item
            .path
            .as_deref()
            .map(|path| format!(" [{path}]"))
            .unwrap_or_default();
        println!(
            "{}",
            warning(&format!("{}{}: {}", item.code, path, item.message))
        );
    }
}

fn blank_as_na(value: &str) -> &str {
    if value.trim().is_empty() {
        "N/A"
    } else {
        value
    }
}

impl OpenClawToolsProfile {
    fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Coding => "coding",
            Self::Messaging => "messaging",
            Self::Full => "full",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        lock_test_home_and_settings, set_test_home_override, TestHomeSettingsLock,
    };
    use serde_json::json;
    use std::ffi::OsString;
    use tempfile::{tempdir, TempDir};

    struct EnvGuard {
        _lock: TestHomeSettingsLock,
        old_home: Option<OsString>,
        old_userprofile: Option<OsString>,
        old_config_dir: Option<OsString>,
        old_disable_open: Option<OsString>,
        _home: TempDir,
    }

    impl EnvGuard {
        fn new() -> Self {
            let lock = lock_test_home_and_settings();
            let home = tempdir().expect("create temp home");
            let old_home = std::env::var_os("HOME");
            let old_userprofile = std::env::var_os("USERPROFILE");
            let old_config_dir = std::env::var_os("CC_SWITCH_CONFIG_DIR");
            let old_disable_open = std::env::var_os("CC_SWITCH_TEST_DISABLE_OPEN");
            std::env::set_var("HOME", home.path());
            std::env::set_var("USERPROFILE", home.path());
            std::env::set_var("CC_SWITCH_CONFIG_DIR", home.path().join(".cc-switch"));
            std::env::set_var("CC_SWITCH_TEST_DISABLE_OPEN", "1");
            set_test_home_override(Some(home.path()));
            crate::settings::reload_test_settings();
            Self {
                _lock: lock,
                old_home,
                old_userprofile,
                old_config_dir,
                old_disable_open,
                _home: home,
            }
        }

        fn workspace_dir(&self) -> std::path::PathBuf {
            self._home.path().join(".openclaw").join("workspace")
        }

        fn memory_dir(&self) -> std::path::PathBuf {
            self.workspace_dir().join("memory")
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.old_userprofile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
            match &self.old_config_dir {
                Some(value) => std::env::set_var("CC_SWITCH_CONFIG_DIR", value),
                None => std::env::remove_var("CC_SWITCH_CONFIG_DIR"),
            }
            match &self.old_disable_open {
                Some(value) => std::env::set_var("CC_SWITCH_TEST_DISABLE_OPEN", value),
                None => std::env::remove_var("CC_SWITCH_TEST_DISABLE_OPEN"),
            }
            set_test_home_override(self.old_home.as_deref().map(Path::new));
            crate::settings::reload_test_settings();
        }
    }

    #[test]
    fn config_openclaw_env_put_stores_json_values() {
        let _env = EnvGuard::new();

        put_env("OPENCLAW_DEBUG", "true").expect("put env");

        let env = get_env_config().expect("load env");
        assert_eq!(env.vars.get("OPENCLAW_DEBUG"), Some(&json!(true)));
    }

    #[test]
    fn config_openclaw_tools_set_normalizes_rules() {
        let _env = EnvGuard::new();

        set_tools(JsonInputArgs {
            json: Some(
                r#"{"profile":"coding","allow":[" Read ",""],"deny":[" Bash(rm*) "]}"#.to_string(),
            ),
            file: None,
        })
        .expect("set tools");

        let tools = get_tools_config().expect("load tools");
        assert_eq!(tools.profile.as_deref(), Some("coding"));
        assert_eq!(tools.allow, vec!["Read"]);
        assert_eq!(tools.deny, vec!["Bash(rm*)"]);
    }

    #[test]
    fn config_openclaw_tools_rule_position_commands_edit_one_row() {
        let _env = EnvGuard::new();

        set_tools(JsonInputArgs {
            json: Some(r#"{"allow":[" Read ","Read","Write"],"deny":[" Bash(rm*) "]}"#.to_string()),
            file: None,
        })
        .expect("seed tools");

        remove_rule_at(ToolsRuleKind::Allow, 2).expect("remove one duplicate by index");
        insert_rule_at(ToolsRuleKind::Allow, 2, " Edit ").expect("insert by index");
        set_rule_at(ToolsRuleKind::Deny, 1, " Bash(ls*) ").expect("set deny by index");

        let tools = get_tools_config().expect("load tools");
        assert_eq!(tools.allow, vec!["Read", "Edit", "Write"]);
        assert_eq!(tools.deny, vec!["Bash(ls*)"]);
    }

    #[test]
    fn config_openclaw_list_position_commands_reject_out_of_range_indexes() {
        let _env = EnvGuard::new();

        set_tools(JsonInputArgs {
            json: Some(r#"{"allow":["Read"]}"#.to_string()),
            file: None,
        })
        .expect("seed tools");

        let zero = remove_rule_at(ToolsRuleKind::Allow, 0).expect_err("index is 1-based");
        assert!(zero.to_string().contains("expected 1..=1"));

        let too_large = insert_rule_at(ToolsRuleKind::Allow, 3, "Write")
            .expect_err("insert index only allows len+1");
        assert!(too_large.to_string().contains("expected 1..=2"));
    }

    #[test]
    fn config_openclaw_health_reports_typed_slice_parse_warnings() {
        let _env = EnvGuard::new();
        crate::openclaw_config::write_openclaw_config_source(
            r#"{
  models: {
    mode: 'merge',
    providers: {},
  },
  tools: 'not-an-object',
}
"#,
        )
        .expect("seed malformed tools config");

        let warnings = openclaw_health_warnings().expect("scan health");

        assert!(warnings.iter().any(|warning| {
            warning.code == "config_parse_failed" && warning.path.as_deref() == Some("tools")
        }));
    }

    #[test]
    fn config_openclaw_agents_runtime_migrates_legacy_timeout() {
        let _env = EnvGuard::new();
        crate::openclaw_config::write_openclaw_config_source(
            r#"{
  models: {
    mode: 'merge',
    providers: {},
  },
  agents: {
    defaults: {
      timeout: 30,
    },
  },
}
"#,
        )
        .expect("seed legacy agents defaults");

        set_runtime(OpenClawRuntimeField::ContextTokens, "4096").expect("set runtime");

        let defaults = get_agents_defaults()
            .expect("load agents defaults")
            .expect("agents defaults should exist");
        assert!(!defaults.extra.contains_key("timeout"));
        assert_eq!(defaults.extra.get("timeoutSeconds"), Some(&json!(30)));
        assert_eq!(defaults.extra.get("contextTokens"), Some(&json!(4096)));
    }

    #[test]
    fn config_openclaw_agents_runtime_set_allows_unrelated_legacy_timeout_strings() {
        let _env = EnvGuard::new();
        crate::openclaw_config::write_openclaw_config_source(
            r#"{
  models: {
    mode: 'merge',
    providers: {},
  },
  agents: {
    defaults: {
      workspace: 'existing-workspace',
      timeout: 'manual-value',
    },
  },
}
"#,
        )
        .expect("seed string legacy timeout");

        set_runtime(OpenClawRuntimeField::Workspace, "next-workspace").expect("set runtime");

        let defaults = get_agents_defaults()
            .expect("load agents defaults")
            .expect("agents defaults should exist");
        assert_eq!(
            defaults.extra.get("workspace"),
            Some(&json!("next-workspace"))
        );
        assert_eq!(
            defaults.extra.get("timeoutSeconds"),
            Some(&json!("manual-value"))
        );
        assert!(!defaults.extra.contains_key("timeout"));
    }

    #[test]
    fn config_openclaw_agents_fallback_position_commands_edit_one_row() {
        let _env = EnvGuard::new();

        set_agents(JsonInputArgs {
            json: Some(
                r#"{"model":{"primary":"demo/primary","fallbacks":["demo/a","demo/a","demo/c"]}}"#
                    .to_string(),
            ),
            file: None,
        })
        .expect("seed agents");

        remove_fallback_at(2).expect("remove one duplicate by index");
        insert_fallback_at(2, " demo/b ").expect("insert by index");
        set_fallback_at(3, " demo/d ").expect("set by index");

        let defaults = get_agents_defaults()
            .expect("load agents defaults")
            .expect("agents defaults should exist");
        let model = defaults.model.expect("model defaults should exist");
        assert_eq!(model.primary, "demo/primary");
        assert_eq!(model.fallbacks, vec!["demo/a", "demo/b", "demo/d"]);
    }

    #[test]
    fn config_openclaw_workspace_rejects_unlisted_file() {
        let _env = EnvGuard::new();

        let err = workspace_exists("NOT_ALLOWED.md", false).expect_err("reject invalid file");

        assert!(err.to_string().contains("Invalid workspace filename"));
    }

    #[test]
    fn config_openclaw_memory_rejects_invalid_filename() {
        let _env = EnvGuard::new();

        let err = show_memory_file("today.md").expect_err("reject invalid file");

        assert!(err.to_string().contains("Invalid daily memory filename"));
    }

    #[test]
    fn config_openclaw_workspace_open_dir_creates_workspace_directory() {
        let env = EnvGuard::new();

        open_workspace_dir().expect("open workspace dir");

        assert!(env.workspace_dir().is_dir());
        assert!(!env.memory_dir().exists());
    }

    #[test]
    fn config_openclaw_memory_open_dir_creates_daily_memory_directory() {
        let env = EnvGuard::new();

        open_memory_dir().expect("open memory dir");

        assert!(env.memory_dir().is_dir());
    }
}
