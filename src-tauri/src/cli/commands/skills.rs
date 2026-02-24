use clap::Subcommand;
use std::future::Future;

use crate::app_config::AppType;
use crate::cli::ui::{create_table, highlight, info, success};
use crate::error::AppError;
use crate::services::skill::{SkillRepo, SyncMethod};
use crate::services::SkillService;

#[derive(Subcommand)]
pub enum SkillsCommand {
    /// List installed skills (from ~/.cc-switch/skills.json)
    List,
    /// Discover available skills (from enabled repos)
    #[command(alias = "search")]
    Discover {
        /// Optional query filter (matches name/directory)
        query: Option<String>,
    },
    /// Install a skill (SSOT -> app skills dir)
    Install {
        /// Skill directory name or full key (owner/name:directory)
        spec: String,
    },
    /// Uninstall a skill (remove from SSOT and app dirs)
    Uninstall {
        /// Skill directory or id
        spec: String,
    },
    /// Enable a skill for the selected app
    Enable {
        /// Skill directory or id
        spec: String,
    },
    /// Disable a skill for the selected app
    Disable {
        /// Skill directory or id
        spec: String,
    },
    /// Enable all installed skills for the selected app
    EnableAll,
    /// Disable all skills for the selected app
    DisableAll,
    /// Sync enabled skills to app skills dirs
    Sync,
    /// Scan unmanaged skills in app skills dirs
    ScanUnmanaged,
    /// Import unmanaged skills from app skills dirs into SSOT
    ImportFromApps {
        /// One or more skill directories to import
        directories: Vec<String>,
    },
    /// Show skill information
    Info {
        /// Skill directory or id
        spec: String,
    },
    /// Get or set the skills sync method (auto|symlink|copy)
    SyncMethod {
        /// Optional method to set (omit to show current)
        #[arg(value_enum)]
        method: Option<SyncMethod>,
    },
    /// Manage skill repositories
    #[command(subcommand)]
    Repos(SkillReposCommand),
}

#[derive(Subcommand)]
pub enum SkillReposCommand {
    /// List all repositories
    List,
    /// Add a repository
    Add {
        /// Repository (GitHub URL or owner/name[@branch])
        url: String,
    },
    /// Remove a repository
    Remove {
        /// Repository (GitHub URL or owner/name)
        url: String,
    },
}

pub fn execute(cmd: SkillsCommand, app: Option<AppType>) -> Result<(), AppError> {
    let app_type = app.clone().unwrap_or(AppType::Claude);

    match cmd {
        SkillsCommand::List => list_installed(),
        SkillsCommand::Discover { query } => discover_skills(query.as_deref()),
        SkillsCommand::Install { spec } => install_skill(&app_type, &spec),
        SkillsCommand::Uninstall { spec } => uninstall_skill(&spec),
        SkillsCommand::Enable { spec } => toggle_skill(&app_type, &spec, true),
        SkillsCommand::Disable { spec } => toggle_skill(&app_type, &spec, false),
        SkillsCommand::EnableAll => enable_all(&app_type),
        SkillsCommand::DisableAll => disable_all(&app_type),
        SkillsCommand::Sync => sync_skills(app.as_ref()),
        SkillsCommand::ScanUnmanaged => scan_unmanaged(),
        SkillsCommand::ImportFromApps { directories } => import_from_apps(directories),
        SkillsCommand::Info { spec } => show_skill_info(&spec),
        SkillsCommand::SyncMethod { method } => sync_method(method),
        SkillsCommand::Repos(repos_cmd) => execute_repos(repos_cmd),
    }
}

fn run_async<T>(fut: impl Future<Output = Result<T, AppError>>) -> Result<T, AppError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("Failed to create runtime: {e}")))?
        .block_on(fut)
}

fn list_installed() -> Result<(), AppError> {
    let skills = SkillService::list_installed()?;

    if skills.is_empty() {
        println!("{}", info("No installed skills found."));
        return Ok(());
    }

    let mut table = create_table();
    table.set_header(vec!["Directory", "Name", "Claude", "Codex", "Gemini"]);
    for skill in skills {
        table.add_row(vec![
            skill.directory,
            skill.name,
            if skill.apps.claude { "✓" } else { " " }.to_string(),
            if skill.apps.codex { "✓" } else { " " }.to_string(),
            if skill.apps.gemini { "✓" } else { " " }.to_string(),
        ]);
    }

    println!("{}", table);
    Ok(())
}

fn discover_skills(query: Option<&str>) -> Result<(), AppError> {
    let service = SkillService::new()?;
    let mut skills = run_async(service.list_skills())?;

    if let Some(query) = query.map(str::trim).filter(|q| !q.is_empty()) {
        let q = query.to_lowercase();
        skills.retain(|s| {
            s.name.to_lowercase().contains(&q) || s.directory.to_lowercase().contains(&q)
        });
    }

    if skills.is_empty() {
        println!("{}", info("No skills found."));
        return Ok(());
    }

    let mut table = create_table();
    table.set_header(vec!["", "Directory", "Name"]);
    for skill in skills {
        table.add_row(vec![
            if skill.installed { "✓" } else { " " }.to_string(),
            skill.directory,
            skill.name,
        ]);
    }
    println!("{}", table);
    Ok(())
}

fn install_skill(app_type: &AppType, spec: &str) -> Result<(), AppError> {
    let service = SkillService::new()?;
    let installed = run_async(service.install(spec, app_type))?;
    println!(
        "{}",
        success(&format!(
            "✓ Installed skill '{}' (enabled for {})",
            installed.directory,
            app_type.as_str()
        ))
    );
    Ok(())
}

fn uninstall_skill(spec: &str) -> Result<(), AppError> {
    SkillService::uninstall(spec)?;
    println!("{}", success(&format!("✓ Uninstalled skill '{spec}'")));
    Ok(())
}

fn toggle_skill(app_type: &AppType, spec: &str, enabled: bool) -> Result<(), AppError> {
    SkillService::toggle_app(spec, app_type, enabled)?;
    println!(
        "{}",
        success(&format!(
            "✓ {} '{}' for {}",
            if enabled { "Enabled" } else { "Disabled" },
            spec,
            app_type.as_str()
        ))
    );
    Ok(())
}

fn enable_all(app_type: &AppType) -> Result<(), AppError> {
    let skills = SkillService::list_installed()?;
    if skills.is_empty() {
        println!("{}", info("No installed skills found."));
        return Ok(());
    }

    let mut count = 0;
    for skill in &skills {
        if !skill.apps.is_enabled_for(app_type) {
            SkillService::toggle_app(&skill.directory, app_type, true)?;
            count += 1;
        }
    }

    println!(
        "{}",
        success(&format!(
            "✓ Enabled {} skill(s) for {}",
            count,
            app_type.as_str()
        ))
    );
    Ok(())
}

fn disable_all(app_type: &AppType) -> Result<(), AppError> {
    let skills = SkillService::list_installed()?;
    if skills.is_empty() {
        println!("{}", info("No installed skills found."));
        return Ok(());
    }

    let mut count = 0;
    for skill in &skills {
        if skill.apps.is_enabled_for(app_type) {
            SkillService::toggle_app(&skill.directory, app_type, false)?;
            count += 1;
        }
    }

    println!(
        "{}",
        success(&format!(
            "✓ Disabled {} skill(s) for {}",
            count,
            app_type.as_str()
        ))
    );
    Ok(())
}

fn sync_skills(app: Option<&AppType>) -> Result<(), AppError> {
    SkillService::sync_all_enabled(app)?;
    println!("{}", success("✓ Skills synced successfully"));
    Ok(())
}

fn scan_unmanaged() -> Result<(), AppError> {
    let skills = SkillService::scan_unmanaged()?;
    if skills.is_empty() {
        println!("{}", info("No unmanaged skills found."));
        return Ok(());
    }

    let mut table = create_table();
    table.set_header(vec!["Directory", "Found In", "Name"]);
    for s in skills {
        table.add_row(vec![s.directory, s.found_in.join(", "), s.name]);
    }
    println!("{}", table);
    Ok(())
}

fn import_from_apps(directories: Vec<String>) -> Result<(), AppError> {
    if directories.is_empty() {
        return Err(AppError::InvalidInput(
            "Please provide at least one directory".to_string(),
        ));
    }

    let imported = SkillService::import_from_apps(directories)?;
    println!(
        "{}",
        success(&format!("✓ Imported {} skill(s) into SSOT", imported.len()))
    );
    Ok(())
}

fn show_skill_info(spec: &str) -> Result<(), AppError> {
    let index = SkillService::load_index()?;

    let record = index
        .skills
        .values()
        .find(|s| s.directory.eq_ignore_ascii_case(spec) || s.id.eq_ignore_ascii_case(spec))
        .ok_or_else(|| AppError::Message(format!("Skill not found: {spec}")))?;

    println!("{}", highlight("Skill"));
    println!("Directory: {}", record.directory);
    println!("Name:      {}", record.name);
    if let Some(desc) = record
        .description
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        println!("Desc:      {}", desc);
    }
    println!(
        "Enabled:   claude={} codex={} gemini={}",
        record.apps.claude, record.apps.codex, record.apps.gemini
    );

    Ok(())
}

fn execute_repos(cmd: SkillReposCommand) -> Result<(), AppError> {
    match cmd {
        SkillReposCommand::List => list_repos(),
        SkillReposCommand::Add { url } => add_repo(&url),
        SkillReposCommand::Remove { url } => remove_repo(&url),
    }
}

fn list_repos() -> Result<(), AppError> {
    let repos = SkillService::list_repos()?;

    if repos.is_empty() {
        println!("{}", info("No skill repos configured."));
        return Ok(());
    }

    let mut table = create_table();
    table.set_header(vec!["Enabled", "Repo", "Branch"]);
    for repo in repos {
        table.add_row(vec![
            if repo.enabled { "✓" } else { " " }.to_string(),
            format!("{}/{}", repo.owner, repo.name),
            repo.branch,
        ]);
    }
    println!("{}", table);
    Ok(())
}

fn add_repo(_url: &str) -> Result<(), AppError> {
    let repo = parse_repo_spec(_url)?;
    SkillService::upsert_repo(repo)?;
    println!("{}", success("✓ Repository added."));
    Ok(())
}

fn remove_repo(_url: &str) -> Result<(), AppError> {
    let repo = parse_repo_spec(_url)?;
    SkillService::remove_repo(&repo.owner, &repo.name)?;
    println!("{}", success("✓ Repository removed."));
    Ok(())
}

fn sync_method(method: Option<SyncMethod>) -> Result<(), AppError> {
    match method {
        Some(method) => {
            SkillService::set_sync_method(method)?;
            println!(
                "{}",
                success(&format!("✓ Skill sync method set to {method:?}"))
            );
        }
        None => {
            let method = SkillService::get_sync_method()?;
            println!("{}", highlight("Skill Sync Method"));
            println!("{method:?}");
        }
    }
    Ok(())
}

fn parse_repo_spec(raw: &str) -> Result<SkillRepo, AppError> {
    let raw = raw.trim().trim_end_matches('/');
    if raw.is_empty() {
        return Err(AppError::InvalidInput(
            "Repository cannot be empty".to_string(),
        ));
    }

    // Allow: https://github.com/owner/name or owner/name[@branch]
    let without_prefix = raw
        .strip_prefix("https://github.com/")
        .or_else(|| raw.strip_prefix("http://github.com/"))
        .unwrap_or(raw);

    let without_git = without_prefix.trim_end_matches(".git");

    let (path, branch) = if let Some((left, right)) = without_git.rsplit_once('@') {
        (left, Some(right))
    } else {
        (without_git, None)
    };

    let Some((owner, name)) = path.split_once('/') else {
        return Err(AppError::InvalidInput(
            "Invalid repo format. Use owner/name or https://github.com/owner/name".to_string(),
        ));
    };

    Ok(SkillRepo {
        owner: owner.to_string(),
        name: name.to_string(),
        branch: branch.unwrap_or("main").to_string(),
        enabled: true,
    })
}
