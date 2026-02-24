use clap::{Subcommand, ValueEnum};
use std::io::{self, BufRead};

use crate::app_config::AppType;
use crate::cli::ui::{create_table, highlight, info, success, warning};
use crate::error::AppError;
use crate::services::memory::{MemoryService, NewObservation, ObservationType};

#[derive(Subcommand)]
pub enum MemoryCommand {
    /// Add a new observation
    Add {
        /// Title of the observation
        title: String,
        /// Content of the observation
        #[arg(short, long)]
        content: Option<String>,
        /// Type of observation (decision, error, pattern, preference, general)
        #[arg(short = 't', long, value_enum, default_value = "general")]
        r#type: ObservationTypeArg,
        /// Comma-separated tags
        #[arg(long)]
        tags: Option<String>,
        /// Project directory this observation relates to
        #[arg(short, long)]
        project: Option<String>,
    },
    /// List observations
    List {
        /// Maximum number of observations to show
        #[arg(short, long, default_value = "20")]
        limit: i64,
        /// Filter by type
        #[arg(short = 't', long, value_enum)]
        r#type: Option<ObservationTypeArg>,
        /// Filter by project directory
        #[arg(short, long)]
        project: Option<String>,
    },
    /// Show a specific observation
    Show {
        /// Observation ID
        id: i64,
    },
    /// Search observations using full-text search
    Search {
        /// Search query
        query: String,
        /// Maximum results
        #[arg(short, long, default_value = "10")]
        limit: i64,
    },
    /// Delete an observation
    Delete {
        /// Observation ID
        id: i64,
    },
    /// Show memory statistics
    Stats,
    /// Get context with progressive disclosure
    Context {
        /// Optional search query
        query: Option<String>,
        /// Maximum tokens in context
        #[arg(long, default_value = "4000")]
        max_tokens: i32,
        /// Project directory for context
        #[arg(short, long)]
        project: Option<String>,
    },
    /// Manage Claude Code hooks integration
    #[command(subcommand)]
    Hooks(HooksCommand),
    /// List recent sessions
    Sessions {
        /// Maximum number of sessions to show
        #[arg(short, long, default_value = "10")]
        limit: i64,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum HookType {
    SessionStart,
    PostToolUse,
}

#[derive(Subcommand)]
pub enum HooksCommand {
    /// Register hooks in Claude Code settings
    Register,
    /// Unregister hooks from Claude Code settings
    Unregister,
    /// Check hook registration status
    Status,
    /// Process hook event (internal use, called by Claude Code hooks)
    Ingest {
        /// Which hook triggered this
        #[arg(long)]
        hook: HookType,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ObservationTypeArg {
    Decision,
    Error,
    Pattern,
    Preference,
    General,
}

impl From<ObservationTypeArg> for ObservationType {
    fn from(arg: ObservationTypeArg) -> Self {
        match arg {
            ObservationTypeArg::Decision => ObservationType::Decision,
            ObservationTypeArg::Error => ObservationType::Error,
            ObservationTypeArg::Pattern => ObservationType::Pattern,
            ObservationTypeArg::Preference => ObservationType::Preference,
            ObservationTypeArg::General => ObservationType::General,
        }
    }
}

pub fn execute(cmd: MemoryCommand, _app: Option<AppType>) -> Result<(), AppError> {
    match cmd {
        MemoryCommand::Add {
            title,
            content,
            r#type,
            tags,
            project,
        } => add_observation(title, content, r#type.into(), tags, project),
        MemoryCommand::List {
            limit,
            r#type,
            project,
        } => list_observations(limit, r#type.map(Into::into), project),
        MemoryCommand::Show { id } => show_observation(id),
        MemoryCommand::Search { query, limit } => search_observations(&query, limit),
        MemoryCommand::Delete { id } => delete_observation(id),
        MemoryCommand::Stats => show_stats(),
        MemoryCommand::Context {
            query,
            max_tokens,
            project,
        } => show_context(query.as_deref(), max_tokens, project.as_deref()),
        MemoryCommand::Hooks(hooks_cmd) => execute_hooks(hooks_cmd),
        MemoryCommand::Sessions { limit } => list_sessions(limit),
    }
}

fn add_observation(
    title: String,
    content: Option<String>,
    observation_type: ObservationType,
    tags: Option<String>,
    project: Option<String>,
) -> Result<(), AppError> {
    let content = content.unwrap_or_default();
    let tags: Vec<String> = tags
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    let obs = MemoryService::add_observation(NewObservation {
        session_id: None,
        title: title.clone(),
        content,
        observation_type,
        tags,
        project_dir: project,
    })?;

    println!(
        "{}",
        success(&format!("Added observation #{} '{}'", obs.id, title))
    );
    Ok(())
}

fn list_observations(
    limit: i64,
    observation_type: Option<ObservationType>,
    project: Option<String>,
) -> Result<(), AppError> {
    let observations =
        MemoryService::list_observations(Some(limit), observation_type, project.as_deref())?;

    if observations.is_empty() {
        println!("{}", info("No observations found."));
        return Ok(());
    }

    let mut table = create_table();
    table.set_header(vec!["ID", "Type", "Title", "Tokens", "Created"]);

    for obs in observations {
        table.add_row(vec![
            obs.id.to_string(),
            obs.observation_type.to_string(),
            truncate(&obs.title, 40),
            obs.tokens.to_string(),
            obs.created_at.format("%Y-%m-%d %H:%M").to_string(),
        ]);
    }

    println!("{}", table);
    Ok(())
}

fn show_observation(id: i64) -> Result<(), AppError> {
    let obs = MemoryService::get_observation(id)?;

    match obs {
        Some(obs) => {
            println!("{}", highlight(&format!("Observation #{}", obs.id)));
            println!("Title:   {}", obs.title);
            println!("Type:    {}", obs.observation_type);
            println!("Tokens:  {}", obs.tokens);
            println!("Created: {}", obs.created_at.format("%Y-%m-%d %H:%M:%S"));
            if !obs.tags.is_empty() {
                println!("Tags:    {}", obs.tags.join(", "));
            }
            if let Some(ref proj) = obs.project_dir {
                println!("Project: {}", proj);
            }
            println!("\n{}", highlight("Content:"));
            println!("{}", obs.content);
            Ok(())
        }
        None => {
            println!("{}", warning(&format!("Observation #{} not found", id)));
            Ok(())
        }
    }
}

fn search_observations(query: &str, limit: i64) -> Result<(), AppError> {
    let results = MemoryService::search(query, Some(limit))?;

    if results.is_empty() {
        println!("{}", info(&format!("No results for '{}'", query)));
        return Ok(());
    }

    println!(
        "{}",
        highlight(&format!("Found {} result(s) for '{}':", results.len(), query))
    );
    println!();

    let mut table = create_table();
    table.set_header(vec!["ID", "Type", "Title", "Tokens"]);

    for obs in results {
        table.add_row(vec![
            obs.id.to_string(),
            obs.observation_type.to_string(),
            truncate(&obs.title, 50),
            obs.tokens.to_string(),
        ]);
    }

    println!("{}", table);
    Ok(())
}

fn delete_observation(id: i64) -> Result<(), AppError> {
    let deleted = MemoryService::delete_observation(id)?;

    if deleted {
        println!("{}", success(&format!("Deleted observation #{}", id)));
    } else {
        println!("{}", warning(&format!("Observation #{} not found", id)));
    }
    Ok(())
}

fn show_stats() -> Result<(), AppError> {
    let stats = MemoryService::stats()?;

    println!("{}", highlight("Memory Statistics"));
    println!();
    println!("Total observations: {}", stats.total_observations);
    println!("Total sessions:     {}", stats.total_sessions);
    println!("Total tokens:       {}", stats.total_tokens);

    if !stats.observations_by_type.is_empty() {
        println!();
        println!("{}", highlight("By Type:"));
        for (obs_type, count) in &stats.observations_by_type {
            println!("  {}: {}", obs_type, count);
        }
    }

    if let Some(oldest) = stats.oldest_observation {
        println!();
        println!(
            "Oldest: {}",
            oldest.format("%Y-%m-%d %H:%M:%S")
        );
    }
    if let Some(newest) = stats.newest_observation {
        println!(
            "Newest: {}",
            newest.format("%Y-%m-%d %H:%M:%S")
        );
    }

    Ok(())
}

fn show_context(query: Option<&str>, max_tokens: i32, project: Option<&str>) -> Result<(), AppError> {
    let context = MemoryService::get_context(query, max_tokens, project)?;

    if context.is_empty() {
        println!("{}", info("No relevant context found."));
        return Ok(());
    }

    let total_tokens: i32 = context.iter().map(|c| c.observation.tokens).sum();

    println!(
        "{}",
        highlight(&format!(
            "Context ({} items, {} tokens):",
            context.len(),
            total_tokens
        ))
    );
    println!();

    for (i, item) in context.iter().enumerate() {
        let priority_label = match item.priority {
            1 => "[FTS]",
            2 => "[Project]",
            _ => "[Recent]",
        };

        println!(
            "{}. {} [{}] {}",
            i + 1,
            priority_label,
            item.observation.observation_type,
            item.observation.title
        );
        println!(
            "   {}",
            truncate(&item.observation.content.replace('\n', " "), 80)
        );
        println!();
    }

    Ok(())
}

fn execute_hooks(cmd: HooksCommand) -> Result<(), AppError> {
    match cmd {
        HooksCommand::Register => {
            MemoryService::register_hooks()?;
            println!("{}", success("Hooks registered in Claude Code settings"));
            Ok(())
        }
        HooksCommand::Unregister => {
            MemoryService::unregister_hooks()?;
            println!("{}", success("Hooks unregistered from Claude Code settings"));
            Ok(())
        }
        HooksCommand::Status => {
            let status = MemoryService::hooks_status()?;

            println!("{}", highlight("Hook Status"));
            println!(
                "Registered:    {}",
                if status.registered { "Yes" } else { "No" }
            );
            println!(
                "SessionStart:  {}",
                if status.session_start { "Yes" } else { "No" }
            );
            println!(
                "PostToolUse:   {}",
                if status.post_tool_use { "Yes" } else { "No" }
            );
            Ok(())
        }
        HooksCommand::Ingest { hook } => {
            // Read event JSON from stdin
            let stdin = io::stdin();
            let mut input = String::new();
            for line in stdin.lock().lines() {
                let line = line.map_err(|e| AppError::Message(format!("Failed to read stdin: {e}")))?;
                input.push_str(&line);
            }

            if input.trim().is_empty() {
                return Ok(());
            }

            match MemoryService::ingest_hook_event(&input, hook) {
                Ok(Some(output)) => {
                    // Output context to stdout for Claude to see
                    print!("{}", output);
                }
                Ok(None) => {}
                Err(e) => {
                    // Write to stderr so the user can see hook failures
                    // without interfering with stdout used by Claude
                    eprintln!("[cc-switch memory] hook ingest error: {}", e);
                    log::warn!("Hook ingest error: {}", e);
                }
            }
            Ok(())
        }
    }
}

fn list_sessions(limit: i64) -> Result<(), AppError> {
    let sessions = MemoryService::list_sessions(Some(limit))?;

    if sessions.is_empty() {
        println!("{}", info("No sessions found."));
        return Ok(());
    }

    let mut table = create_table();
    table.set_header(vec!["ID", "App", "Project", "Started", "Ended", "Summary"]);

    for session in sessions {
        table.add_row(vec![
            session.id.to_string(),
            session.app,
            session
                .project_dir
                .map(|p| truncate(&p, 30))
                .unwrap_or_else(|| "-".to_string()),
            session.started_at.format("%Y-%m-%d %H:%M").to_string(),
            session
                .ended_at
                .map(|e| e.format("%H:%M").to_string())
                .unwrap_or_else(|| "ongoing".to_string()),
            session
                .summary
                .map(|s| truncate(&s, 30))
                .unwrap_or_else(|| "-".to_string()),
        ]);
    }

    println!("{}", table);
    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }

    let ellipsis = "...";
    if max_len <= ellipsis.len() {
        return ellipsis.chars().take(max_len).collect();
    }

    let char_limit = max_len.saturating_sub(ellipsis.len());
    let mut end = 0;
    for (idx, _) in s.char_indices() {
        if idx > char_limit {
            break;
        }
        end = idx;
    }

    format!("{}{}", &s[..end], ellipsis)
}
