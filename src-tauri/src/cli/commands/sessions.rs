use chrono::{Local, TimeZone};
use clap::Subcommand;
use serde::ser::SerializeSeq;
use serde::Serialize;
use serde::Serializer as _;
use std::io::Write;
use std::process::Command;
use std::str::FromStr;

use crate::app_config::AppType;
use crate::cli::ui::{create_table, info, success, to_json, warning};
use crate::database::Database;
use crate::error::AppError;
use crate::services::session_usage::SessionSyncResult;
use crate::session_manager::{self, SessionMessage, SessionMessageBatch, SessionMeta};

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
    messages_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    messages_error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionMessagesOutput<'a> {
    messages: &'a [SessionMessage],
    messages_truncated: bool,
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
    let include_all = all && provider.is_none();
    let scope = session_scope(app, provider, include_all);
    let reader =
        session_manager::build_fresh_session_manifest(&scope).map_err(AppError::Message)?;
    if json {
        let stdout = std::io::stdout();
        let mut output = stdout.lock();
        return write_manifest_json(&reader, &mut output);
    }

    if reader.total_rows() == 0 {
        println!("{}", info("No sessions found."));
        return Ok(());
    }

    // `comfy_table` needs to retain every row whose widths it computes. Render
    // one immutable manifest page at a time so a million-row list never turns
    // into a million-row in-memory table. Repeating the header at page
    // boundaries also keeps long terminal output readable.
    for page_index in 0..reader.page_count() {
        let page = reader.load_page(page_index).ok_or_else(|| {
            AppError::Message(format!(
                "Session page {page_index} became unavailable while listing."
            ))
        })?;
        let mut table = create_table();
        table.set_header(vec!["Provider", "Session", "Title", "Updated", "Workdir"]);
        for session in &page.rows {
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
    }
    Ok(())
}

fn write_manifest_json(
    reader: &session_manager::paged_manifest::ManifestReader,
    output: &mut dyn Write,
) -> Result<(), AppError> {
    // Publication validated every page before this immutable generation became
    // current. Stream it once so first-byte latency and total I/O remain linear.
    let mut serializer = serde_json::Serializer::pretty(&mut *output);
    let mut sequence = serializer
        .serialize_seq(Some(reader.total_rows()))
        .map_err(|source| AppError::JsonSerialize { source })?;
    for page_index in 0..reader.page_count() {
        let page = reader.load_page(page_index).ok_or_else(|| {
            AppError::Message(format!(
                "Session page {page_index} became unavailable while writing JSON."
            ))
        })?;
        for session in &page.rows {
            sequence
                .serialize_element(session)
                .map_err(|source| AppError::JsonSerialize { source })?;
        }
    }
    sequence
        .end()
        .map_err(|source| AppError::JsonSerialize { source })?;
    drop(serializer);
    output
        .write_all(b"\n")
        .map_err(|source| AppError::IoContext {
            context: "failed to finish session JSON output".to_string(),
            source,
        })
}

fn show_session(
    app: Option<AppType>,
    provider: Option<AppType>,
    all: bool,
    selector: &str,
    json: bool,
) -> Result<(), AppError> {
    let (session, _) = resolve_scanned_session(app, provider, all, selector)?;
    let (message_batch, messages_error) = load_session_messages(&session);

    if json {
        let (messages, messages_truncated) = message_batch
            .map(|batch| (Some(batch.messages), batch.truncated))
            .unwrap_or((None, false));
        let detail = SessionDetail {
            session,
            messages,
            messages_truncated,
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
    match (message_batch, messages_error) {
        (Some(batch), None) => print_message_batch(&batch),
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
    let batch = session_manager::load_messages(&session.provider_id, source_path)
        .map_err(AppError::Message)?;

    if json {
        let output = SessionMessagesOutput {
            messages: &batch.messages,
            messages_truncated: batch.truncated,
        };
        println!(
            "{}",
            to_json(&output).map_err(|source| AppError::JsonSerialize { source })?
        );
        return Ok(());
    }

    print_message_batch(&batch);
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

    // 与 TUI 删除路径一致：清掉扫描 sidecar、首屏快照以及 provider/all 两份
    // 分页 manifest，避免 CLI 删除后重开 TUI 时旧首屏短暂“复活”。这些都是
    // schema-free 的本地缓存；失败只记 debug，不影响真实会话已经删除的结果。
    let _ = session_manager::purge_deleted_session_from_local_caches(
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
    let reader = session_manager::build_fresh_session_manifest("all").map_err(AppError::Message)?;

    if json {
        let stdout = std::io::stdout();
        let mut output = stdout.lock();
        return write_manifest_search_json(&reader, query, &mut output);
    }

    let mut found = false;
    for page_index in 0..reader.page_count() {
        let page = reader.load_page(page_index).ok_or_else(|| {
            AppError::Message(format!(
                "Session page {page_index} became unavailable while searching."
            ))
        })?;
        let hits = session_manager::search_sessions_in(&page.rows, query);
        if hits.is_empty() {
            continue;
        }
        found = true;

        // Both the source page and this table are capped at 100 rows. Print
        // each result page before searching the next one so neither the corpus
        // nor a high-match result set accumulates in memory.
        let mut table = create_table();
        table.set_header(vec!["Provider", "Session", "Title", "Snippets"]);
        for hit in &hits {
            let session = page
                .rows
                .iter()
                .find(|session| {
                    session.provider_id == hit.provider_id
                        && session.session_id == hit.session_id
                        && session.source_path.as_deref() == Some(hit.source_path.as_str())
                })
                .map(session_title)
                .unwrap_or_else(|| short_session_id(&hit.session_id));
            let snippet_preview = hit
                .snippets
                .iter()
                .take(2)
                .map(|snippet| truncate_chars(&snippet.snippet, 80))
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
    }
    if !found {
        println!("{}", info("No matches found."));
    }
    Ok(())
}

fn write_manifest_search_json(
    reader: &session_manager::paged_manifest::ManifestReader,
    query: &str,
    output: &mut dyn Write,
) -> Result<(), AppError> {
    let mut serializer = serde_json::Serializer::pretty(&mut *output);
    let mut sequence = serializer
        .serialize_seq(None)
        .map_err(|source| AppError::JsonSerialize { source })?;
    for page_index in 0..reader.page_count() {
        let page = reader.load_page(page_index).ok_or_else(|| {
            AppError::Message(format!(
                "Session page {page_index} became unavailable while searching."
            ))
        })?;
        for hit in session_manager::search_sessions_in(&page.rows, query) {
            sequence
                .serialize_element(&hit)
                .map_err(|source| AppError::JsonSerialize { source })?;
        }
    }
    sequence
        .end()
        .map_err(|source| AppError::JsonSerialize { source })?;
    drop(serializer);
    output
        .write_all(b"\n")
        .map_err(|source| AppError::IoContext {
            context: "failed to finish session search JSON output".to_string(),
            source,
        })
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
    let include_all = all && forced_provider.is_none();
    let scope = session_scope(app, forced_provider, include_all);
    let reader =
        session_manager::build_fresh_session_manifest(&scope).map_err(AppError::Message)?;
    let session = resolve_session_manifest(&reader, selector)?;
    Ok((session, selector.to_string()))
}

fn session_scope(app: Option<AppType>, provider: Option<AppType>, all: bool) -> String {
    if all {
        return "all".to_string();
    }

    provider
        .or(app)
        .unwrap_or(AppType::Claude)
        .as_str()
        .to_string()
}

#[derive(Default)]
struct SelectorMatches {
    count: usize,
    first: Option<SessionMeta>,
    choices: Vec<String>,
}

impl SelectorMatches {
    const MAX_CHOICES: usize = 8;

    fn push(&mut self, session: SessionMeta) {
        self.count = self.count.saturating_add(1);
        if self.first.is_none() {
            self.first = Some(session.clone());
        }
        if self.choices.len() < Self::MAX_CHOICES {
            self.choices
                .push(format!("{}:{}", session.provider_id, session.session_id));
        }
    }
}

struct SessionSelectorResolver<'a> {
    selector: &'a str,
    exact: SelectorMatches,
    prefixed: SelectorMatches,
}

impl<'a> SessionSelectorResolver<'a> {
    fn new(selector: &'a str) -> Result<Self, AppError> {
        let selector = selector.trim();
        if selector.is_empty() {
            return Err(AppError::InvalidInput(
                "session selector cannot be empty".to_string(),
            ));
        }
        Ok(Self {
            selector,
            exact: SelectorMatches::default(),
            prefixed: SelectorMatches::default(),
        })
    }

    fn push(&mut self, session: SessionMeta) {
        if session.session_id == self.selector {
            self.exact.push(session);
        } else if session.session_id.starts_with(self.selector) {
            self.prefixed.push(session);
        }
    }

    fn finish(self) -> Result<SessionMeta, AppError> {
        if self.exact.count == 1 {
            return self.exact.first.ok_or_else(|| {
                AppError::Message("Session selector state was incomplete.".to_string())
            });
        }
        if self.exact.count > 1 {
            return Err(ambiguous_selector_error(
                self.selector,
                self.exact.count,
                &self.exact.choices,
            ));
        }
        if self.prefixed.count == 1 {
            return self.prefixed.first.ok_or_else(|| {
                AppError::Message("Session selector state was incomplete.".to_string())
            });
        }
        if self.prefixed.count > 1 {
            return Err(ambiguous_selector_error(
                self.selector,
                self.prefixed.count,
                &self.prefixed.choices,
            ));
        }
        Err(AppError::Message(format!(
            "Session '{}' was not found.",
            self.selector
        )))
    }
}

fn resolve_session_manifest(
    reader: &session_manager::paged_manifest::ManifestReader,
    selector: &str,
) -> Result<SessionMeta, AppError> {
    let mut resolver = SessionSelectorResolver::new(selector)?;
    for page_index in 0..reader.page_count() {
        let page = reader.load_page(page_index).ok_or_else(|| {
            AppError::Message(format!(
                "Session page {page_index} is unavailable while resolving '{selector}'."
            ))
        })?;
        for session in page.rows {
            resolver.push(session);
        }
    }
    resolver.finish()
}

#[cfg(test)]
fn resolve_session(sessions: &[SessionMeta], selector: &str) -> Result<SessionMeta, AppError> {
    let mut resolver = SessionSelectorResolver::new(selector)?;
    for session in sessions {
        resolver.push(session.clone());
    }
    resolver.finish()
}

fn ambiguous_selector_error(selector: &str, count: usize, choices: &[String]) -> AppError {
    let selector = selector.trim();
    let choices = choices.join(", ");
    let suffix = if count > SelectorMatches::MAX_CHOICES {
        ", ..."
    } else {
        ""
    };
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

fn load_session_messages(session: &SessionMeta) -> (Option<SessionMessageBatch>, Option<String>) {
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

fn print_message_batch(batch: &SessionMessageBatch) {
    print_message_table(&batch.messages);
    if batch.truncated {
        println!(
            "{}",
            warning("Showing a bounded message preview; additional content was truncated.")
        );
    }
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
    const LIMIT: usize = 160;

    let mut preview = String::with_capacity(LIMIT + 3);
    let mut count = 0usize;
    let mut pending_space = false;
    let mut truncated = false;
    for ch in content.chars() {
        if ch.is_whitespace() {
            pending_space |= !preview.is_empty();
            continue;
        }
        if pending_space {
            if count >= LIMIT {
                truncated = true;
                break;
            }
            preview.push(' ');
            count += 1;
            pending_space = false;
        }
        if count >= LIMIT {
            truncated = true;
            break;
        }
        preview.push(ch);
        count += 1;
    }
    if truncated {
        preview.push_str("...");
    }
    preview
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
    fn selector_resolution_retains_only_bounded_ambiguity_context_at_large_n() {
        let mut resolver = SessionSelectorResolver::new("shared-").expect("resolver");
        for index in 0..50_000 {
            resolver.push(meta("codex", &format!("shared-{index:05}")));
        }

        assert_eq!(resolver.prefixed.count, 50_000);
        assert_eq!(
            resolver.prefixed.choices.len(),
            SelectorMatches::MAX_CHOICES
        );
        let error = resolver.finish().expect_err("prefix must be ambiguous");
        assert!(error.to_string().contains(", ..."));
    }

    #[test]
    fn manifest_json_stream_is_one_valid_array_across_pages() {
        let directory = tempfile::tempdir().expect("manifest directory");
        let store = session_manager::paged_manifest::PagedManifestStore::open_at(directory.path())
            .expect("manifest store");
        let mut builder = store.begin_build("codex").expect("manifest builder");
        for index in 0..205 {
            let mut row = meta("codex", &format!("session-{index:03}"));
            row.last_active_at = Some(index);
            builder.push(row).expect("manifest row");
        }
        let published = builder.publish().expect("published manifest");
        let mut output = Vec::new();

        write_manifest_json(&published.reader, &mut output).expect("stream JSON");

        let decoded: Vec<SessionMeta> = serde_json::from_slice(&output).expect("valid JSON array");
        assert_eq!(decoded.len(), 205);
        assert!(output.ends_with(b"\n"));
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
