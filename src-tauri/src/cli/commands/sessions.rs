use chrono::{Local, TimeZone};
use clap::Subcommand;
use serde::Serialize;
use std::process::Command;
use std::str::FromStr;

use crate::app_config::AppType;
use crate::cli::ui::{create_table, info, success, to_json, warning};
use crate::database::Database;
use crate::error::AppError;
use crate::services::session_usage::SessionSyncResult;
use crate::session_manager::{self, SessionMessage, SessionMeta, SessionSearchHit};

#[derive(Subcommand, Debug, Clone)]
pub enum SessionsCommand {
    /// List saved assistant sessions
    List {
        /// Scan a specific provider instead of --app
        #[arg(long, value_parser = parse_session_provider)]
        provider: Option<AppType>,
        /// Scan every supported app instead of the selected app
        #[arg(long)]
        all: bool,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Show session metadata and messages
    Show {
        /// Session id or unique id prefix. Also accepts provider/id.
        selector: String,
        /// Resolve against a specific provider instead of --app
        #[arg(long, value_parser = parse_session_provider)]
        provider: Option<AppType>,
        /// Search every supported app
        #[arg(long)]
        all: bool,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Print messages for a session
    Messages {
        /// Session id or unique id prefix. Also accepts provider/id.
        selector: String,
        /// Resolve against a specific provider instead of --app
        #[arg(long, value_parser = parse_session_provider)]
        provider: Option<AppType>,
        /// Search every supported app
        #[arg(long)]
        all: bool,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Resume a session using its saved resume command
    Resume {
        /// Session id or unique id prefix. Also accepts provider/id.
        selector: String,
        /// Resolve against a specific provider instead of --app
        #[arg(long, value_parser = parse_session_provider)]
        provider: Option<AppType>,
        /// Search every supported app
        #[arg(long)]
        all: bool,
        /// Print the saved command instead of executing it
        #[arg(long)]
        print: bool,
    },
    /// Delete a saved session
    Delete {
        /// Session id or unique id prefix. Also accepts provider/id.
        selector: String,
        /// Resolve against a specific provider instead of --app
        #[arg(long, value_parser = parse_session_provider)]
        provider: Option<AppType>,
        /// Search every supported app
        #[arg(long)]
        all: bool,
        /// Confirm deletion without prompting
        #[arg(long)]
        yes: bool,
    },
    /// Search full conversation content across all sessions
    Search {
        /// Search query (case-insensitive, matches message content)
        query: String,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Sync local session logs into usage statistics
    SyncUsage {
        /// Sync a specific provider instead of --app
        #[arg(long, value_parser = parse_session_provider)]
        provider: Option<AppType>,
        /// Sync every supported provider
        #[arg(long)]
        all: bool,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionDetail {
    session: SessionMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    messages: Option<Vec<SessionMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    messages_error: Option<String>,
}

pub fn execute(cmd: SessionsCommand, app: Option<AppType>) -> Result<(), AppError> {
    match cmd {
        SessionsCommand::List {
            provider,
            all,
            json,
        } => list_sessions(app, provider, all, json),
        SessionsCommand::Show {
            selector,
            provider,
            all,
            json,
        } => show_session(app, provider, all, &selector, json),
        SessionsCommand::Messages {
            selector,
            provider,
            all,
            json,
        } => print_messages(app, provider, all, &selector, json),
        SessionsCommand::Resume {
            selector,
            provider,
            all,
            print,
        } => resume_session(app, provider, all, &selector, print),
        SessionsCommand::Delete {
            selector,
            provider,
            all,
            yes,
        } => delete_session(app, provider, all, &selector, yes),
        SessionsCommand::Search { query, json } => search_sessions_cmd(&query, json),
        SessionsCommand::SyncUsage {
            provider,
            all,
            json,
        } => sync_usage(app, provider, all, json),
    }
}

fn list_sessions(
    app: Option<AppType>,
    provider: Option<AppType>,
    all: bool,
    json: bool,
) -> Result<(), AppError> {
    let sessions = scan_sessions(app, provider.as_ref(), all && provider.is_none());
    if json {
        println!(
            "{}",
            to_json(&sessions).map_err(|source| AppError::JsonSerialize { source })?
        );
        return Ok(());
    }

    if sessions.is_empty() {
        println!("{}", info("No sessions found."));
        return Ok(());
    }

    let mut table = create_table();
    table.set_header(vec!["Provider", "Session", "Title", "Updated", "Workdir"]);
    for session in &sessions {
        table.add_row(vec![
            session.provider_id.clone(),
            short_session_id(&session.session_id),
            session_title(session),
            format_session_time(session.last_active_at.or(session.created_at)),
            session
                .project_dir
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("-")
                .to_string(),
        ]);
    }

    println!("{table}");
    Ok(())
}

fn show_session(
    app: Option<AppType>,
    provider: Option<AppType>,
    all: bool,
    selector: &str,
    json: bool,
) -> Result<(), AppError> {
    let (session, _) = resolve_scanned_session(app, provider, all, selector)?;
    let (messages, messages_error) = load_session_messages(&session);

    if json {
        let detail = SessionDetail {
            session,
            messages,
            messages_error,
        };
        println!(
            "{}",
            to_json(&detail).map_err(|source| AppError::JsonSerialize { source })?
        );
        return Ok(());
    }

    print_session_overview(&session);
    println!();
    match (messages, messages_error) {
        (Some(messages), None) => print_message_table(&messages),
        (_, Some(error)) => println!("{}", warning(&error)),
        _ => println!("{}", info("Messages are not available.")),
    }

    Ok(())
}

fn print_messages(
    app: Option<AppType>,
    provider: Option<AppType>,
    all: bool,
    selector: &str,
    json: bool,
) -> Result<(), AppError> {
    let (session, _) = resolve_scanned_session(app, provider, all, selector)?;
    let source_path = required_source_path(&session)?;
    let messages = session_manager::load_messages(&session.provider_id, source_path)
        .map_err(AppError::Message)?;

    if json {
        println!(
            "{}",
            to_json(&messages).map_err(|source| AppError::JsonSerialize { source })?
        );
        return Ok(());
    }

    print_message_table(&messages);
    Ok(())
}

fn resume_session(
    app: Option<AppType>,
    provider: Option<AppType>,
    all: bool,
    selector: &str,
    print: bool,
) -> Result<(), AppError> {
    let (session, _) = resolve_scanned_session(app, provider, all, selector)?;
    let command = session
        .resume_command
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Message("Session does not have a resume command.".to_string()))?;

    if print {
        println!("{command}");
        return Ok(());
    }

    let status = shell_command(command)
        .current_dir_for_session(session.project_dir.as_deref())?
        .status()
        .map_err(|source| AppError::IoContext {
            context: format!("failed to execute resume command `{command}`"),
            source,
        })?;

    if status.success() {
        Ok(())
    } else {
        Err(AppError::Message(format!(
            "Resume command exited with status {status}"
        )))
    }
}

fn delete_session(
    app: Option<AppType>,
    provider: Option<AppType>,
    all: bool,
    selector: &str,
    yes: bool,
) -> Result<(), AppError> {
    let (session, _) = resolve_scanned_session(app, provider, all, selector)?;
    let source_path = required_source_path(&session)?.to_string();

    if !yes {
        let confirm = inquire::Confirm::new(&format!(
            "Delete session '{}' from {}?",
            session_title(&session),
            session.provider_id
        ))
        .with_default(false)
        .prompt()
        .map_err(|e| AppError::Message(format!("Prompt failed: {e}")))?;
        if !confirm {
            println!("{}", info("Cancelled."));
            return Ok(());
        }
    }

    let deleted =
        session_manager::delete_session(&session.provider_id, &session.session_id, &source_path)
            .map_err(AppError::Message)?;
    if !deleted {
        return Err(AppError::Message("Session was not deleted.".to_string()));
    }

    // 与 TUI 删除路径一致：清掉该会话的 sidecar 扫描缓存行，否则 CLI 删完再开
    // TUI 时，stale-while-revalidate 的秒开快照会让已删会话短暂"复活"。纯缓存
    // 操作，失败只记 debug，不影响删除结果。
    session_manager::scan_cache_store::purge_session(
        &session.provider_id,
        &session.session_id,
        &source_path,
    );

    println!(
        "{}",
        success(&format!(
            "Deleted session '{}' from {}.",
            session_title(&session),
            session.provider_id
        ))
    );
    Ok(())
}

fn search_sessions_cmd(query: &str, json: bool) -> Result<(), AppError> {
    let sessions = session_manager::scan_sessions();
    let hits = session_manager::search_sessions_in(&sessions, query);

    if json {
        println!(
            "{}",
            to_json(&hits).map_err(|source| AppError::JsonSerialize { source })?
        );
        return Ok(());
    }

    if hits.is_empty() {
        println!("{}", info("No matches found."));
        return Ok(());
    }

    let mut table = create_table();
    table.set_header(vec!["Provider", "Session", "Title", "Snippets"]);
    for hit in &hits {
        let session = sessions
            .iter()
            .find(|s| s.source_path.as_deref() == Some(&hit.source_path))
            .map(session_title)
            .unwrap_or_else(|| short_session_id(&hit.session_id));
        let snippet_preview = hit
            .snippets
            .iter()
            .take(2)
            .map(|s| truncate_chars(&s.snippet, 80))
            .collect::<Vec<_>>()
            .join(" | ");
        table.add_row(vec![
            hit.provider_id.clone(),
            short_session_id(&hit.session_id),
            session,
            snippet_preview,
        ]);
    }
    println!("{table}");

    // Show full snippets for each hit
    for hit in &hits {
        println!();
        println!(
            "{}",
            info(&format!(
                "=== {} / {} ===",
                hit.provider_id,
                short_session_id(&hit.session_id)
            ))
        );
        for snippet in &hit.snippets {
            println!(
                "  [{}] {}",
                snippet.role,
                truncate_chars(&snippet.snippet, 200)
            );
        }
    }

    Ok(())
}

fn sync_usage(
    app: Option<AppType>,
    provider: Option<AppType>,
    all: bool,
    json: bool,
) -> Result<(), AppError> {
    let db = Database::init()?;
    let backfill_error = match db.backfill_missing_usage_costs() {
        Ok(updated) if updated > 0 => {
            log::info!("Manual session usage sync backfilled costs for {updated} usage row(s)");
            None
        }
        Ok(_) => None,
        Err(error) => Some(format!("Usage cost backfill failed: {error}")),
    };

    let mut result = if all || (app.is_none() && provider.is_none()) {
        crate::services::session_usage::sync_all_session_usage(&db)?
    } else {
        let app_type = provider.or(app).unwrap_or(AppType::Claude);
        sync_usage_for_provider(&db, app_type)?
    };
    if let Some(message) = backfill_error {
        result.errors.push(message);
    }

    if json {
        println!(
            "{}",
            to_json(&result).map_err(|source| AppError::JsonSerialize { source })?
        );
        return Ok(());
    }

    println!(
        "{}",
        success(&format!(
            "Synced usage: imported {}, skipped {}, scanned {} file(s).",
            result.imported, result.skipped, result.files_scanned
        ))
    );
    for error in result.errors.iter().take(8) {
        println!("{}", warning(error));
    }
    if result.errors.len() > 8 {
        println!(
            "{}",
            warning(&format!(
                "... and {} more error(s)",
                result.errors.len() - 8
            ))
        );
    }

    Ok(())
}

fn sync_usage_for_provider(
    db: &Database,
    app_type: AppType,
) -> Result<SessionSyncResult, AppError> {
    // 与 sync_all_session_usage 相同的导入作用域：进度可见 + 本连接（本
    // 命令独占）在导入期间临时 synchronous=NORMAL，结束恢复 FULL。
    let _progress = crate::services::session_usage::sync_progress::begin();
    let _durability = db.bulk_import_durability_guard();
    match app_type {
        AppType::Claude => crate::services::session_usage::sync_claude_session_logs(db),
        AppType::Codex => crate::services::session_usage_codex::sync_codex_usage(db),
        AppType::Gemini => crate::services::session_usage_gemini::sync_gemini_usage(db),
        AppType::OpenCode => crate::services::session_usage_opencode::sync_opencode_usage(db),
        other => Err(AppError::InvalidInput(format!(
            "session usage sync is only supported for claude, codex, gemini, and opencode; got {}",
            other.as_str()
        ))),
    }
}

fn resolve_scanned_session(
    app: Option<AppType>,
    provider: Option<AppType>,
    all: bool,
    selector: &str,
) -> Result<(SessionMeta, String), AppError> {
    let scoped = parse_scoped_selector(selector);
    let selector = scoped
        .as_ref()
        .map(|(_, selector)| selector.as_str())
        .unwrap_or(selector);
    let forced_provider = scoped
        .as_ref()
        .and_then(|(provider, _)| app_type_from_provider_id(provider))
        .or(provider);
    let sessions = scan_sessions(
        app,
        forced_provider.as_ref(),
        all && forced_provider.is_none(),
    );
    let session = resolve_session(&sessions, selector)?;
    Ok((session.clone(), selector.to_string()))
}

fn scan_sessions(app: Option<AppType>, provider: Option<&AppType>, all: bool) -> Vec<SessionMeta> {
    if all {
        return session_manager::scan_sessions();
    }

    let app_type = provider.cloned().or(app).unwrap_or(AppType::Claude);
    session_manager::scan_sessions_for_provider(app_type.as_str())
}

fn resolve_session<'a>(
    sessions: &'a [SessionMeta],
    selector: &str,
) -> Result<&'a SessionMeta, AppError> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err(AppError::InvalidInput(
            "session selector cannot be empty".to_string(),
        ));
    }

    let exact = sessions
        .iter()
        .filter(|session| session.session_id == selector)
        .collect::<Vec<_>>();
    match exact.len() {
        1 => return Ok(exact[0]),
        len if len > 1 => return Err(ambiguous_selector_error(selector, &exact)),
        _ => {}
    }

    let prefixed = sessions
        .iter()
        .filter(|session| session.session_id.starts_with(selector))
        .collect::<Vec<_>>();
    match prefixed.len() {
        1 => Ok(prefixed[0]),
        0 => Err(AppError::Message(format!(
            "Session '{selector}' was not found."
        ))),
        _ => Err(ambiguous_selector_error(selector, &prefixed)),
    }
}

fn ambiguous_selector_error(selector: &str, matches: &[&SessionMeta]) -> AppError {
    let choices = matches
        .iter()
        .take(8)
        .map(|session| format!("{}:{}", session.provider_id, session.session_id))
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = if matches.len() > 8 { ", ..." } else { "" };
    AppError::Message(format!(
        "Session selector '{selector}' is ambiguous: {choices}{suffix}"
    ))
}

fn parse_scoped_selector(selector: &str) -> Option<(String, String)> {
    selector
        .split_once('/')
        .or_else(|| selector.split_once(':'))
        .and_then(|(provider, id)| {
            let provider = provider.trim();
            let id = id.trim();
            if provider.is_empty() || id.is_empty() || app_type_from_provider_id(provider).is_none()
            {
                return None;
            }
            Some((provider.to_string(), id.to_string()))
        })
}

fn parse_session_provider(value: &str) -> Result<AppType, String> {
    app_type_from_provider_id(value).ok_or_else(|| {
        format!("unsupported provider '{value}'. Allowed: claude, codex, gemini, opencode, openclaw, hermes")
    })
}

fn app_type_from_provider_id(provider_id: &str) -> Option<AppType> {
    let normalized = provider_id.trim().to_lowercase().replace('-', "");
    AppType::from_str(&normalized).ok()
}

fn load_session_messages(session: &SessionMeta) -> (Option<Vec<SessionMessage>>, Option<String>) {
    let Some(source_path) = session
        .source_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return (
            None,
            Some("Session source path is not available; messages cannot be loaded.".to_string()),
        );
    };

    match session_manager::load_messages(&session.provider_id, source_path) {
        Ok(messages) => (Some(messages), None),
        Err(error) => (None, Some(error)),
    }
}

fn required_source_path(session: &SessionMeta) -> Result<&str, AppError> {
    session
        .source_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            AppError::Message(
                "Session source path is not available; this operation cannot continue.".to_string(),
            )
        })
}

fn print_session_overview(session: &SessionMeta) {
    println!("Provider: {}", session.provider_id);
    println!("Session:  {}", session.session_id);
    println!("Title:    {}", session_title(session));
    println!(
        "Summary:  {}",
        session
            .summary
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("-")
    );
    println!(
        "Time:     {}",
        format_session_time(session.last_active_at.or(session.created_at))
    );
    println!(
        "Workdir:  {}",
        session
            .project_dir
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("-")
    );
    println!(
        "Source:   {}",
        session
            .source_path
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("-")
    );
    println!(
        "Resume:   {}",
        session
            .resume_command
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("-")
    );
}

fn print_message_table(messages: &[SessionMessage]) {
    if messages.is_empty() {
        println!("{}", info("No messages found."));
        return;
    }

    let mut table = create_table();
    table.set_header(vec!["Role", "Time", "Content"]);
    for message in messages {
        table.add_row(vec![
            message.role.clone(),
            format_session_time(message.ts),
            collapse_message_preview(&message.content),
        ]);
    }
    println!("{table}");
}

fn session_title(session: &SessionMeta) -> String {
    session
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            session
                .project_dir
                .as_deref()
                .map(path_basename)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| short_session_id(&session.session_id))
}

fn short_session_id(session_id: &str) -> String {
    session_id.chars().take(12).collect()
}

fn path_basename(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return String::new();
    }
    std::path::Path::new(trimmed)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(trimmed)
        .to_string()
}

fn format_session_time(timestamp_ms: Option<i64>) -> String {
    timestamp_ms
        .and_then(|timestamp| Local.timestamp_millis_opt(timestamp).single())
        .map(|dt| dt.format("%Y/%m/%d %H:%M").to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn collapse_message_preview(content: &str) -> String {
    let single_line = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    truncate_chars(&single_line, 160)
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

trait SessionCommandExt {
    fn current_dir_for_session(self, cwd: Option<&str>) -> Result<Self, AppError>
    where
        Self: Sized;
}

impl SessionCommandExt for Command {
    fn current_dir_for_session(mut self, cwd: Option<&str>) -> Result<Self, AppError> {
        if let Some(cwd) = cwd.filter(|value| !value.trim().is_empty()) {
            self.current_dir(cwd);
        }
        Ok(self)
    }
}

#[cfg(unix)]
fn shell_command(command: &str) -> Command {
    let mut process = Command::new("sh");
    process.arg("-c").arg(command);
    process
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut process = Command::new("cmd");
    process.arg("/C").arg(command);
    process
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(provider_id: &str, session_id: &str) -> SessionMeta {
        SessionMeta {
            provider_id: provider_id.to_string(),
            session_id: session_id.to_string(),
            title: None,
            summary: None,
            project_dir: None,
            created_at: None,
            last_active_at: None,
            source_path: Some(format!("/tmp/{provider_id}/{session_id}.jsonl")),
            resume_command: Some(format!("{provider_id} resume {session_id}")),
        }
    }

    #[test]
    fn resolves_exact_session_id_before_prefix_matches() {
        let sessions = vec![meta("codex", "abc"), meta("codex", "abcdef")];

        let resolved = resolve_session(&sessions, "abc").expect("exact match should resolve");

        assert_eq!(resolved.session_id, "abc");
    }

    #[test]
    fn resolves_unique_session_id_prefix() {
        let sessions = vec![meta("codex", "abcdef"), meta("codex", "xyz")];

        let resolved = resolve_session(&sessions, "abc").expect("prefix should resolve");

        assert_eq!(resolved.session_id, "abcdef");
    }

    #[test]
    fn rejects_ambiguous_session_id_prefix() {
        let sessions = vec![meta("codex", "abcdef"), meta("codex", "abc999")];

        let error = resolve_session(&sessions, "abc").expect_err("prefix should be ambiguous");

        assert!(error.to_string().contains("ambiguous"));
        assert!(error.to_string().contains("codex:abcdef"));
        assert!(error.to_string().contains("codex:abc999"));
    }

    #[test]
    fn parses_provider_scoped_selectors() {
        assert_eq!(
            parse_scoped_selector("codex/abc"),
            Some(("codex".to_string(), "abc".to_string()))
        );
        assert_eq!(
            parse_scoped_selector("claude:xyz"),
            Some(("claude".to_string(), "xyz".to_string()))
        );
        assert_eq!(
            parse_scoped_selector("open-code/abc"),
            Some(("open-code".to_string(), "abc".to_string()))
        );
        assert_eq!(parse_scoped_selector("unknown/abc"), None);
        assert_eq!(parse_scoped_selector("not-scoped"), None);
    }

    #[test]
    fn parses_session_provider_backend_ids_and_clap_aliases() {
        assert_eq!(parse_session_provider("opencode"), Ok(AppType::OpenCode));
        assert_eq!(parse_session_provider("open-code"), Ok(AppType::OpenCode));
        assert_eq!(parse_session_provider("openclaw"), Ok(AppType::OpenClaw));
        assert_eq!(parse_session_provider("open-claw"), Ok(AppType::OpenClaw));
        assert!(parse_session_provider("unknown").is_err());
    }

    #[test]
    fn session_title_prefers_title_project_then_id() {
        let titled = SessionMeta {
            title: Some("Fix bug".to_string()),
            summary: Some("Summary".to_string()),
            project_dir: Some("/tmp/project".to_string()),
            ..meta("codex", "abcdef123456")
        };
        assert_eq!(session_title(&titled), "Fix bug");

        let project = SessionMeta {
            title: None,
            ..titled.clone()
        };
        assert_eq!(session_title(&project), "project");

        let fallback = SessionMeta {
            title: None,
            summary: None,
            project_dir: None,
            ..titled
        };
        assert_eq!(session_title(&fallback), "abcdef123456");
    }
}
