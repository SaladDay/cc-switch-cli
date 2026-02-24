//! Memory service for session context capture and semantic search.
//!
//! Uses SQLite + FTS5 for full-text search. Database is stored in the app config directory
//! (e.g. `~/.cc-switch/memory.db`) as determined by `get_app_config_dir()`.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, Once, OnceLock};

use crate::app_config::AppType;
use crate::config::get_app_config_dir;
use crate::error::AppError;

// ============================================================================
// Data Structures
// ============================================================================

/// Observation types for categorizing memories
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ObservationType {
    /// Important decisions made during development
    Decision,
    /// Errors encountered and their solutions
    Error,
    /// Code patterns and conventions discovered
    Pattern,
    /// User preferences learned from interactions
    Preference,
    /// General observations
    #[default]
    General,
}

impl ObservationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Error => "error",
            Self::Pattern => "pattern",
            Self::Preference => "preference",
            Self::General => "general",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "decision" => Self::Decision,
            "error" => Self::Error,
            "pattern" => Self::Pattern,
            "preference" => Self::Preference,
            _ => Self::General,
        }
    }
}

impl std::fmt::Display for ObservationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for ObservationType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from_str(s))
    }
}

/// A session record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: i64,
    pub app: String,
    pub project_dir: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub summary: Option<String>,
}

/// An observation record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: i64,
    pub session_id: Option<i64>,
    pub title: String,
    pub content: String,
    pub observation_type: ObservationType,
    pub tags: Vec<String>,
    pub tokens: i32,
    /// Reserved for future relevance ranking. Currently always 1.0.
    pub relevance_score: f64,
    pub created_at: DateTime<Utc>,
    pub project_dir: Option<String>,
}

/// Input for creating a new observation
#[derive(Debug, Clone)]
pub struct NewObservation {
    pub session_id: Option<i64>,
    pub title: String,
    pub content: String,
    pub observation_type: ObservationType,
    pub tags: Vec<String>,
    pub project_dir: Option<String>,
}

/// Context item for progressive disclosure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItem {
    pub observation: Observation,
    pub priority: u8, // 1 = highest (FTS match), 2 = project match, 3 = recent
    pub match_reason: String,
}

/// Statistics about the memory database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStats {
    pub total_observations: i64,
    pub total_sessions: i64,
    pub total_tokens: i64,
    pub observations_by_type: Vec<(String, i64)>,
    pub oldest_observation: Option<DateTime<Utc>>,
    pub newest_observation: Option<DateTime<Utc>>,
}

// ============================================================================
// Database Connection Management
// ============================================================================

fn get_db_path() -> PathBuf {
    get_app_config_dir().join("memory.db")
}

static DB_CONNECTION: OnceLock<Mutex<Connection>> = OnceLock::new();
static DB_INIT: Once = Once::new();
static DB_INIT_ERROR: OnceLock<String> = OnceLock::new();

fn get_connection() -> Result<&'static Mutex<Connection>, AppError> {
    DB_INIT.call_once(|| {
        let result = (|| -> Result<Connection, AppError> {
            let path = get_db_path();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
            }
            let conn = Connection::open(&path)
                .map_err(|e| AppError::Message(format!("Failed to open memory database: {e}")))?;
            init_schema(&conn)?;
            Ok(conn)
        })();

        match result {
            Ok(conn) => { let _ = DB_CONNECTION.set(Mutex::new(conn)); }
            Err(e) => { let _ = DB_INIT_ERROR.set(e.to_string()); }
        }
    });

    DB_CONNECTION.get().ok_or_else(|| {
        let msg = DB_INIT_ERROR.get().map(|s| s.as_str()).unwrap_or("Unknown error");
        AppError::Message(format!("Memory database initialization failed: {msg}"))
    })
}

fn init_schema(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        r#"
        -- Sessions table
        CREATE TABLE IF NOT EXISTS sessions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            app TEXT NOT NULL,
            project_dir TEXT,
            started_at INTEGER NOT NULL,
            ended_at INTEGER,
            summary TEXT
        );

        -- Observations table
        CREATE TABLE IF NOT EXISTS observations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id INTEGER REFERENCES sessions(id),
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            observation_type TEXT NOT NULL DEFAULT 'general',
            tags TEXT NOT NULL DEFAULT '',
            tokens INTEGER NOT NULL DEFAULT 0,
            relevance_score REAL NOT NULL DEFAULT 1.0,
            created_at INTEGER NOT NULL,
            project_dir TEXT
        );

        -- Create indexes
        CREATE INDEX IF NOT EXISTS idx_observations_session ON observations(session_id);
        CREATE INDEX IF NOT EXISTS idx_observations_type ON observations(observation_type);
        CREATE INDEX IF NOT EXISTS idx_observations_created ON observations(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_observations_project ON observations(project_dir);
        CREATE INDEX IF NOT EXISTS idx_sessions_app ON sessions(app);
        CREATE INDEX IF NOT EXISTS idx_sessions_started ON sessions(started_at DESC);

        -- FTS5 virtual table for full-text search
        CREATE VIRTUAL TABLE IF NOT EXISTS observations_fts USING fts5(
            title,
            content,
            tags,
            content='observations',
            content_rowid='id'
        );

        -- Triggers to keep FTS5 in sync
        CREATE TRIGGER IF NOT EXISTS observations_ai AFTER INSERT ON observations BEGIN
            INSERT INTO observations_fts(rowid, title, content, tags)
            VALUES (new.id, new.title, new.content, new.tags);
        END;

        CREATE TRIGGER IF NOT EXISTS observations_ad AFTER DELETE ON observations BEGIN
            INSERT INTO observations_fts(observations_fts, rowid, title, content, tags)
            VALUES ('delete', old.id, old.title, old.content, old.tags);
        END;

        CREATE TRIGGER IF NOT EXISTS observations_au AFTER UPDATE ON observations BEGIN
            INSERT INTO observations_fts(observations_fts, rowid, title, content, tags)
            VALUES ('delete', old.id, old.title, old.content, old.tags);
            INSERT INTO observations_fts(rowid, title, content, tags)
            VALUES (new.id, new.title, new.content, new.tags);
        END;
        "#,
    )
    .map_err(|e| AppError::Message(format!("Failed to initialize memory schema: {e}")))?;

    Ok(())
}

// ============================================================================
// Token Estimation
// ============================================================================

/// Safely convert a Unix timestamp to DateTime<Utc>, returning an error for invalid values.
fn timestamp_to_datetime(ts: i64) -> Result<DateTime<Utc>, rusqlite::Error> {
    Utc.timestamp_opt(ts, 0)
        .single()
        .ok_or_else(|| rusqlite::Error::IntegralValueOutOfRange(0, ts))
}

/// Estimate token count for text.
///
/// Uses a rough heuristic of ~4 bytes per token. This is an approximation and may
/// be inaccurate for CJK text or code. Callers should not rely on exact counts.
fn estimate_tokens(text: &str) -> i32 {
    (text.len() as f64 / 4.0).ceil() as i32
}

// ============================================================================
// MemoryService
// ============================================================================

pub struct MemoryService;

impl MemoryService {
    // -------------------------------------------------------------------------
    // Observation CRUD
    // -------------------------------------------------------------------------

    /// Add a new observation
    pub fn add_observation(obs: NewObservation) -> Result<Observation, AppError> {
        let conn = get_connection()?.lock()?;
        let now = Utc::now();
        let tags_str = obs.tags.join(",");
        let full_text = format!("{} {}", obs.title, obs.content);
        let tokens = estimate_tokens(&full_text);

        conn.execute(
            r#"
            INSERT INTO observations (session_id, title, content, observation_type, tags, tokens, relevance_score, created_at, project_dir)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                obs.session_id,
                obs.title,
                obs.content,
                obs.observation_type.as_str(),
                tags_str,
                tokens,
                1.0,
                now.timestamp(),
                obs.project_dir,
            ],
        )
        .map_err(|e| AppError::Message(format!("Failed to add observation: {e}")))?;

        let id = conn.last_insert_rowid();

        Ok(Observation {
            id,
            session_id: obs.session_id,
            title: obs.title,
            content: obs.content,
            observation_type: obs.observation_type,
            tags: obs.tags,
            tokens,
            relevance_score: 1.0,
            created_at: now,
            project_dir: obs.project_dir,
        })
    }

    /// Get an observation by ID
    pub fn get_observation(id: i64) -> Result<Option<Observation>, AppError> {
        let conn = get_connection()?.lock()?;

        conn.query_row(
            r#"
            SELECT id, session_id, title, content, observation_type, tags, tokens, relevance_score, created_at, project_dir
            FROM observations WHERE id = ?1
            "#,
            params![id],
            |row| {
                Ok(Observation {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    title: row.get(2)?,
                    content: row.get(3)?,
                    observation_type: ObservationType::from_str(&row.get::<_, String>(4)?),
                    tags: row
                        .get::<_, String>(5)?
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect(),
                    tokens: row.get(6)?,
                    relevance_score: row.get(7)?,
                    created_at: timestamp_to_datetime(row.get(8)?)?,
                    project_dir: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(|e| AppError::Message(format!("Failed to get observation: {e}")))
    }

    /// List observations with optional filters
    pub fn list_observations(
        limit: Option<i64>,
        observation_type: Option<ObservationType>,
        project_dir: Option<&str>,
    ) -> Result<Vec<Observation>, AppError> {
        let conn = get_connection()?.lock()?;
        let limit = limit.unwrap_or(50);

        let mut sql = String::from(
            r#"
            SELECT id, session_id, title, content, observation_type, tags, tokens, relevance_score, created_at, project_dir
            FROM observations
            WHERE 1=1
            "#,
        );

        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![];

        if let Some(ref obs_type) = observation_type {
            sql.push_str(" AND observation_type = ?");
            params_vec.push(Box::new(obs_type.as_str().to_string()));
        }

        if let Some(ref proj) = project_dir {
            sql.push_str(" AND project_dir = ?");
            params_vec.push(Box::new(proj.to_string()));
        }

        sql.push_str(" ORDER BY created_at DESC LIMIT ?");
        params_vec.push(Box::new(limit));

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AppError::Message(format!("Failed to prepare query: {e}")))?;

        let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(Observation {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    title: row.get(2)?,
                    content: row.get(3)?,
                    observation_type: ObservationType::from_str(&row.get::<_, String>(4)?),
                    tags: row
                        .get::<_, String>(5)?
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect(),
                    tokens: row.get(6)?,
                    relevance_score: row.get(7)?,
                    created_at: timestamp_to_datetime(row.get(8)?)?,
                    project_dir: row.get(9)?,
                })
            })
            .map_err(|e| AppError::Message(format!("Failed to list observations: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| AppError::Message(format!("Row error: {e}")))?);
        }

        Ok(results)
    }

    /// Delete an observation
    pub fn delete_observation(id: i64) -> Result<bool, AppError> {
        let conn = get_connection()?.lock()?;

        let rows = conn
            .execute("DELETE FROM observations WHERE id = ?1", params![id])
            .map_err(|e| AppError::Message(format!("Failed to delete observation: {e}")))?;

        Ok(rows > 0)
    }

    // -------------------------------------------------------------------------
    // FTS5 Search
    // -------------------------------------------------------------------------

    /// Search observations using FTS5
    pub fn search(query: &str, limit: Option<i64>) -> Result<Vec<Observation>, AppError> {
        let conn = get_connection()?.lock()?;
        let limit = limit.unwrap_or(20);

        // Sanitize query for FTS5: strip special characters and wrap each token in quotes
        let fts_query: String = query
            .split_whitespace()
            .map(|token| {
                let clean: String = token.chars().filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect();
                format!("\"{}\"", clean)
            })
            .filter(|t| t != "\"\"")
            .collect::<Vec<_>>()
            .join(" ");

        if fts_query.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = conn
            .prepare(
                r#"
                SELECT o.id, o.session_id, o.title, o.content, o.observation_type, o.tags,
                       o.tokens, o.relevance_score, o.created_at, o.project_dir
                FROM observations o
                JOIN observations_fts fts ON o.id = fts.rowid
                WHERE observations_fts MATCH ?1
                ORDER BY rank
                LIMIT ?2
                "#,
            )
            .map_err(|e| AppError::Message(format!("Failed to prepare search: {e}")))?;

        let rows = stmt
            .query_map(params![fts_query, limit], |row| {
                Ok(Observation {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    title: row.get(2)?,
                    content: row.get(3)?,
                    observation_type: ObservationType::from_str(&row.get::<_, String>(4)?),
                    tags: row
                        .get::<_, String>(5)?
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect(),
                    tokens: row.get(6)?,
                    relevance_score: row.get(7)?,
                    created_at: timestamp_to_datetime(row.get(8)?)?,
                    project_dir: row.get(9)?,
                })
            })
            .map_err(|e| AppError::Message(format!("Search failed: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| AppError::Message(format!("Row error: {e}")))?);
        }

        Ok(results)
    }

    // -------------------------------------------------------------------------
    // Progressive Disclosure
    // -------------------------------------------------------------------------

    /// Get context with progressive disclosure and token budget
    pub fn get_context(
        query: Option<&str>,
        max_tokens: i32,
        project_dir: Option<&str>,
    ) -> Result<Vec<ContextItem>, AppError> {
        let mut items: Vec<ContextItem> = Vec::new();
        let mut used_tokens = 0;
        let mut seen_ids = std::collections::HashSet::new();

        // Layer 1: FTS5 matches (highest priority)
        if let Some(q) = query {
            if !q.trim().is_empty() {
                let search_results = Self::search(q, Some(10))?;
                for obs in search_results {
                    if used_tokens + obs.tokens > max_tokens {
                        continue;
                    }
                    if seen_ids.contains(&obs.id) {
                        continue;
                    }
                    seen_ids.insert(obs.id);
                    used_tokens += obs.tokens;
                    items.push(ContextItem {
                        observation: obs,
                        priority: 1,
                        match_reason: "FTS match".to_string(),
                    });
                }
            }
        }

        // Layer 2: Project-specific observations
        if let Some(proj) = project_dir {
            let project_obs = Self::list_observations(Some(20), None, Some(proj))?;
            for obs in project_obs {
                if used_tokens + obs.tokens > max_tokens {
                    continue;
                }
                if seen_ids.contains(&obs.id) {
                    continue;
                }
                seen_ids.insert(obs.id);
                used_tokens += obs.tokens;
                items.push(ContextItem {
                    observation: obs,
                    priority: 2,
                    match_reason: "Project match".to_string(),
                });
            }
        }

        // Layer 3: Recent observations (lowest priority)
        let recent = Self::list_observations(Some(50), None, None)?;
        for obs in recent {
            if used_tokens + obs.tokens > max_tokens {
                continue;
            }
            if seen_ids.contains(&obs.id) {
                continue;
            }
            seen_ids.insert(obs.id);
            used_tokens += obs.tokens;
            items.push(ContextItem {
                observation: obs,
                priority: 3,
                match_reason: "Recent".to_string(),
            });
        }

        // Sort by priority (lower number = higher priority)
        items.sort_by(|a, b| a.priority.cmp(&b.priority));

        Ok(items)
    }

    // -------------------------------------------------------------------------
    // Sessions
    // -------------------------------------------------------------------------

    /// Start a new session
    pub fn start_session(app: &AppType, project_dir: Option<&str>) -> Result<Session, AppError> {
        let conn = get_connection()?.lock()?;
        let now = Utc::now();

        conn.execute(
            r#"
            INSERT INTO sessions (app, project_dir, started_at)
            VALUES (?1, ?2, ?3)
            "#,
            params![app.as_str(), project_dir, now.timestamp()],
        )
        .map_err(|e| AppError::Message(format!("Failed to start session: {e}")))?;

        let id = conn.last_insert_rowid();

        Ok(Session {
            id,
            app: app.as_str().to_string(),
            project_dir: project_dir.map(String::from),
            started_at: now,
            ended_at: None,
            summary: None,
        })
    }

    /// End a session
    pub fn end_session(id: i64, summary: Option<&str>) -> Result<bool, AppError> {
        let conn = get_connection()?.lock()?;
        let now = Utc::now();

        let rows = conn
            .execute(
                r#"
                UPDATE sessions SET ended_at = ?1, summary = ?2 WHERE id = ?3
                "#,
                params![now.timestamp(), summary, id],
            )
            .map_err(|e| AppError::Message(format!("Failed to end session: {e}")))?;

        Ok(rows > 0)
    }

    /// List recent sessions
    pub fn list_sessions(limit: Option<i64>) -> Result<Vec<Session>, AppError> {
        let conn = get_connection()?.lock()?;
        let limit = limit.unwrap_or(10);

        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, app, project_dir, started_at, ended_at, summary
                FROM sessions
                ORDER BY started_at DESC
                LIMIT ?1
                "#,
            )
            .map_err(|e| AppError::Message(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(params![limit], |row| {
                Ok(Session {
                    id: row.get(0)?,
                    app: row.get(1)?,
                    project_dir: row.get(2)?,
                    started_at: timestamp_to_datetime(row.get(3)?)?,
                    ended_at: row
                        .get::<_, Option<i64>>(4)?
                        .map(|ts| timestamp_to_datetime(ts))
                        .transpose()?,
                    summary: row.get(5)?,
                })
            })
            .map_err(|e| AppError::Message(format!("Failed to list sessions: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| AppError::Message(format!("Row error: {e}")))?);
        }

        Ok(results)
    }

    // -------------------------------------------------------------------------
    // Statistics
    // -------------------------------------------------------------------------

    /// Get memory statistics
    pub fn stats() -> Result<MemoryStats, AppError> {
        let conn = get_connection()?.lock()?;

        let total_observations: i64 = conn
            .query_row("SELECT COUNT(*) FROM observations", [], |row| row.get(0))
            .unwrap_or(0);

        let total_sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap_or(0);

        let total_tokens: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(tokens), 0) FROM observations",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let mut stmt = conn
            .prepare("SELECT observation_type, COUNT(*) FROM observations GROUP BY observation_type")
            .map_err(|e| AppError::Message(format!("Failed to prepare stats query: {e}")))?;

        let by_type: Vec<(String, i64)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| AppError::Message(format!("Stats query failed: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        let oldest: Option<DateTime<Utc>> = conn
            .query_row(
                "SELECT MIN(created_at) FROM observations",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten()
            .and_then(|ts| timestamp_to_datetime(ts).ok());

        let newest: Option<DateTime<Utc>> = conn
            .query_row(
                "SELECT MAX(created_at) FROM observations",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten()
            .and_then(|ts| timestamp_to_datetime(ts).ok());

        Ok(MemoryStats {
            total_observations,
            total_sessions,
            total_tokens,
            observations_by_type: by_type,
            oldest_observation: oldest,
            newest_observation: newest,
        })
    }

    // -------------------------------------------------------------------------
    // Hook Integration
    // -------------------------------------------------------------------------

    /// Check if hooks are registered in Claude Code settings
    pub fn hooks_status() -> Result<HooksStatus, AppError> {
        let settings_path = crate::config::get_claude_settings_path();
        if !settings_path.exists() {
            return Ok(HooksStatus {
                registered: false,
                session_start: false,
                post_tool_use: false,
            });
        }

        let content =
            std::fs::read_to_string(&settings_path).map_err(|e| AppError::io(&settings_path, e))?;

        let value: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| AppError::json(&settings_path, e))?;

        let hooks = value.get("hooks");
        let session_start = hooks
            .and_then(|h| h.get("SessionStart"))
            .map(|v| has_cc_switch_hook(v))
            .unwrap_or(false);
        let post_tool_use = hooks
            .and_then(|h| h.get("PostToolUse"))
            .map(|v| has_cc_switch_hook(v))
            .unwrap_or(false);

        Ok(HooksStatus {
            registered: session_start || post_tool_use,
            session_start,
            post_tool_use,
        })
    }

    /// Register hooks in Claude Code settings
    pub fn register_hooks() -> Result<(), AppError> {
        let settings_path = crate::config::get_claude_settings_path();

        let mut value: serde_json::Value = if settings_path.exists() {
            let content = std::fs::read_to_string(&settings_path)
                .map_err(|e| AppError::io(&settings_path, e))?;
            serde_json::from_str(&content).map_err(|e| AppError::json(&settings_path, e))?
        } else {
            serde_json::json!({})
        };

        let hooks = value
            .as_object_mut()
            .ok_or_else(|| AppError::Message("Settings is not an object".to_string()))?
            .entry("hooks")
            .or_insert(serde_json::json!({}));

        let hooks_obj = hooks
            .as_object_mut()
            .ok_or_else(|| AppError::Message("Hooks is not an object".to_string()))?;

        // SessionStart hook - outputs context to Claude
        let session_start_hook = serde_json::json!([{
            "matcher": "",
            "hooks": [{
                "type": "command",
                "command": "cc-switch memory hooks ingest --hook session-start"
            }]
        }]);

        // PostToolUse hook - captures observations
        let post_tool_use_hook = serde_json::json!([{
            "matcher": "",
            "hooks": [{
                "type": "command",
                "command": "cc-switch memory hooks ingest --hook post-tool-use"
            }]
        }]);

        // Check and add hooks if not present
        if !has_cc_switch_hook(hooks_obj.get("SessionStart").unwrap_or(&serde_json::Value::Null)) {
            hooks_obj.insert("SessionStart".to_string(), session_start_hook);
        }

        if !has_cc_switch_hook(hooks_obj.get("PostToolUse").unwrap_or(&serde_json::Value::Null)) {
            hooks_obj.insert("PostToolUse".to_string(), post_tool_use_hook);
        }

        crate::config::write_json_file(&settings_path, &value)?;
        Ok(())
    }

    /// Unregister hooks from Claude Code settings
    pub fn unregister_hooks() -> Result<(), AppError> {
        let settings_path = crate::config::get_claude_settings_path();

        if !settings_path.exists() {
            return Ok(());
        }

        let content =
            std::fs::read_to_string(&settings_path).map_err(|e| AppError::io(&settings_path, e))?;

        let mut value: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| AppError::json(&settings_path, e))?;

        if let Some(hooks) = value.get_mut("hooks").and_then(|h| h.as_object_mut()) {
            // Remove our hooks from SessionStart
            if let Some(session_start) = hooks.get_mut("SessionStart") {
                remove_cc_switch_hook(session_start);
            }

            // Remove our hooks from PostToolUse
            if let Some(post_tool_use) = hooks.get_mut("PostToolUse") {
                remove_cc_switch_hook(post_tool_use);
            }
        }

        crate::config::write_json_file(&settings_path, &value)?;
        Ok(())
    }

    /// Process incoming hook event (called from hooks ingest)
    pub fn ingest_hook_event(
        event_json: &str,
        hook_type: crate::cli::commands::memory::HookType,
    ) -> Result<Option<String>, AppError> {
        let event: serde_json::Value = serde_json::from_str(event_json)
            .map_err(|e| AppError::Message(format!("Invalid hook event JSON: {e}")))?;

        use crate::cli::commands::memory::HookType;
        match hook_type {
            HookType::SessionStart => Self::handle_session_start(&event),
            HookType::PostToolUse => Self::handle_post_tool_use(&event),
        }
    }

    fn handle_session_start(event: &serde_json::Value) -> Result<Option<String>, AppError> {
        // Claude Code sends: { "session_id": "...", "cwd": "..." }
        let project_dir = event
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(String::from);

        // Start a new session
        let _ = Self::start_session(&AppType::Claude, project_dir.as_deref())?;

        // Get context for this session
        let context = Self::get_context(None, 4000, project_dir.as_deref())?;

        if context.is_empty() {
            return Ok(None);
        }

        // Format context for output
        let mut output = String::from("## Memory Context\n\n");
        for item in context.iter().take(5) {
            output.push_str(&format!(
                "### {} ({})\n{}\n\n",
                item.observation.title,
                item.observation.observation_type,
                item.observation.content
            ));
        }

        Ok(Some(output))
    }

    fn handle_post_tool_use(event: &serde_json::Value) -> Result<Option<String>, AppError> {
        // Claude Code sends: { "session_id", "cwd", "tool_name", "tool_input", "tool_response" }
        let tool_name = event
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let tool_input = event.get("tool_input");
        let tool_output = event.get("tool_response");

        // Filter for interesting events
        let (observation_type, title) = match tool_name {
            "Write" | "Edit" => (Some(ObservationType::Pattern), format!("{} operation", tool_name)),
            "Bash" => {
                // Check exit code first if available, then fall back to output heuristics
                let has_error_exit = event
                    .get("exit_code")
                    .and_then(|v| v.as_i64())
                    .map(|code| code != 0)
                    .unwrap_or(false);

                if has_error_exit {
                    (Some(ObservationType::Error), "Bash error".to_string())
                } else if let Some(output) = tool_output.and_then(|v| v.as_str()) {
                    // Use line-start patterns to reduce false positives
                    let has_error = output.lines().any(|line| {
                        let trimmed = line.trim();
                        trimmed.starts_with("error:")
                            || trimmed.starts_with("Error:")
                            || trimmed.starts_with("ERROR:")
                            || trimmed.starts_with("FAILED")
                            || trimmed.starts_with("fatal:")
                            || trimmed.starts_with("panic:")
                    });
                    if has_error {
                        (Some(ObservationType::Error), "Bash error".to_string())
                    } else {
                        (None, String::new())
                    }
                } else {
                    (None, String::new())
                }
            }
            // GitHub MCP tools
            "mcp__github__create_pull_request" => {
                let pr_title = tool_input
                    .and_then(|v| v.get("title"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                (Some(ObservationType::Decision), format!("PR created: {}", pr_title))
            }
            "mcp__github__merge_pull_request" => {
                let pr_num = tool_input
                    .and_then(|v| v.get("pull_number"))
                    .and_then(|v| v.as_i64())
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                (Some(ObservationType::Decision), format!("PR #{} merged", pr_num))
            }
            "mcp__github__create_issue" => {
                let issue_title = tool_input
                    .and_then(|v| v.get("title"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                (Some(ObservationType::General), format!("Issue created: {}", issue_title))
            }
            "mcp__github__create_branch" => {
                let branch = tool_input
                    .and_then(|v| v.get("branch"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                (Some(ObservationType::Pattern), format!("Branch created: {}", branch))
            }
            "mcp__github__push_files" | "mcp__github__create_or_update_file" => {
                let msg = tool_input
                    .and_then(|v| v.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                (Some(ObservationType::Pattern), format!("Pushed: {}", msg))
            }
            _ => (None, String::new()),
        };

        let Some(obs_type) = observation_type else {
            return Ok(None);
        };
        let content = format!(
            "Tool: {}\nInput: {}\nOutput: {}",
            tool_name,
            tool_input
                .map(|v| v.to_string())
                .unwrap_or_else(|| "N/A".to_string()),
            tool_output
                .map(|v| v.to_string())
                .unwrap_or_else(|| "N/A".to_string())
        );

        let project_dir = event
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(String::from);

        Self::add_observation(NewObservation {
            session_id: None,
            title,
            content,
            observation_type: obs_type,
            tags: vec![tool_name.to_string()],
            project_dir,
        })?;

        Ok(None)
    }

    // -------------------------------------------------------------------------
    // SQL Export / Import (for WebDAV sync)
    // -------------------------------------------------------------------------

    /// Export memory database as SQL bytes. Returns `None` if the database is empty.
    pub fn export_sql_bytes() -> Result<Option<Vec<u8>>, AppError> {
        let conn = get_connection()?.lock()?;

        let total: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM sessions) + (SELECT COUNT(*) FROM observations)",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if total == 0 {
            return Ok(None);
        }

        let mut output = String::new();
        output.push_str("-- CC Switch Memory 导出\n");
        output.push_str("PRAGMA foreign_keys=OFF;\nBEGIN TRANSACTION;\n");

        // Schema for sessions and observations (regular tables only)
        let mut stmt = conn
            .prepare(
                "SELECT type, name, sql FROM sqlite_master \
                 WHERE sql NOT NULL AND name IN ('sessions','observations') \
                 ORDER BY type='table' DESC, name",
            )
            .map_err(|e| AppError::Message(format!("Memory export schema query failed: {e}")))?;

        let mut tables = Vec::new();
        let mut rows = stmt.query([]).map_err(|e| AppError::Message(e.to_string()))?;
        while let Some(row) = rows.next().map_err(|e| AppError::Message(e.to_string()))? {
            let obj_type: String = row.get(0).map_err(|e| AppError::Message(e.to_string()))?;
            let name: String = row.get(1).map_err(|e| AppError::Message(e.to_string()))?;
            let sql: String = row.get(2).map_err(|e| AppError::Message(e.to_string()))?;
            output.push_str(&sql);
            output.push_str(";\n");
            if obj_type == "table" {
                tables.push(name);
            }
        }

        // Export indexes for these tables
        let mut idx_stmt = conn
            .prepare(
                "SELECT sql FROM sqlite_master \
                 WHERE type='index' AND sql NOT NULL AND tbl_name IN ('sessions','observations')",
            )
            .map_err(|e| AppError::Message(e.to_string()))?;
        let mut idx_rows = idx_stmt.query([]).map_err(|e| AppError::Message(e.to_string()))?;
        while let Some(row) = idx_rows.next().map_err(|e| AppError::Message(e.to_string()))? {
            let sql: String = row.get(0).map_err(|e| AppError::Message(e.to_string()))?;
            output.push_str(&sql);
            output.push_str(";\n");
        }

        // Export triggers
        let mut trig_stmt = conn
            .prepare(
                "SELECT sql FROM sqlite_master \
                 WHERE type='trigger' AND sql NOT NULL AND tbl_name='observations'",
            )
            .map_err(|e| AppError::Message(e.to_string()))?;
        let mut trig_rows = trig_stmt.query([]).map_err(|e| AppError::Message(e.to_string()))?;
        while let Some(row) = trig_rows.next().map_err(|e| AppError::Message(e.to_string()))? {
            let sql: String = row.get(0).map_err(|e| AppError::Message(e.to_string()))?;
            output.push_str(&sql);
            output.push_str(";\n");
        }

        // FTS5 virtual table
        output.push_str(
            "CREATE VIRTUAL TABLE IF NOT EXISTS observations_fts USING fts5(\
             title,content,tags,content='observations',content_rowid='id');\n",
        );

        // Data
        for table in &tables {
            let col_names = memory_table_columns(&conn, table)?;
            if col_names.is_empty() {
                continue;
            }
            let mut data_stmt = conn
                .prepare(&format!("SELECT * FROM \"{table}\""))
                .map_err(|e| AppError::Message(e.to_string()))?;
            let mut data_rows = data_stmt.query([]).map_err(|e| AppError::Message(e.to_string()))?;
            while let Some(row) = data_rows.next().map_err(|e| AppError::Message(e.to_string()))? {
                let mut values = Vec::with_capacity(col_names.len());
                for idx in 0..col_names.len() {
                    let val = row
                        .get_ref(idx)
                        .map_err(|e| AppError::Message(e.to_string()))?;
                    values.push(format_memory_sql_value(val));
                }
                let cols = col_names
                    .iter()
                    .map(|c| format!("\"{c}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                output.push_str(&format!(
                    "INSERT INTO \"{table}\" ({cols}) VALUES ({});\n",
                    values.join(", ")
                ));
            }
        }

        // Rebuild FTS5 index
        output.push_str(
            "INSERT INTO observations_fts(observations_fts) VALUES('rebuild');\n",
        );
        output.push_str("COMMIT;\nPRAGMA foreign_keys=ON;\n");

        Ok(Some(output.into_bytes()))
    }

    /// Import memory database from SQL bytes (full replace).
    pub fn import_sql_bytes(bytes: &[u8]) -> Result<(), AppError> {
        let sql = std::str::from_utf8(bytes)
            .map_err(|e| AppError::Message(format!("Memory SQL is not valid UTF-8: {e}")))?;

        if !sql.trim_start().starts_with("-- CC Switch Memory") {
            return Err(AppError::Message(
                "Invalid memory SQL export format".to_string(),
            ));
        }

        let conn = get_connection()?.lock()?;

        // Drop existing data
        conn.execute_batch(
            "DELETE FROM observations;\n\
             DELETE FROM sessions;\n\
             INSERT INTO observations_fts(observations_fts) VALUES('rebuild');",
        )
        .map_err(|e| AppError::Message(format!("Failed to clear memory tables: {e}")))?;

        // Execute import — the SQL already contains CREATE TABLE IF NOT EXISTS + INSERT
        conn.execute_batch(sql)
            .map_err(|e| AppError::Message(format!("Memory SQL import failed: {e}")))?;

        Ok(())
    }

    /// Format context for display
    pub fn format_context(items: &[ContextItem]) -> String {
        if items.is_empty() {
            return String::from("No relevant context found.");
        }

        let mut output = String::new();
        for (i, item) in items.iter().enumerate() {
            output.push_str(&format!(
                "{}. [{}] {} ({})\n   {}\n\n",
                i + 1,
                item.observation.observation_type,
                item.observation.title,
                item.match_reason,
                item.observation
                    .content
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(100)
                    .collect::<String>()
            ));
        }
        output
    }
}

// ============================================================================
// Hook Helpers
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HooksStatus {
    pub registered: bool,
    pub session_start: bool,
    pub post_tool_use: bool,
}

fn has_cc_switch_hook(value: &serde_json::Value) -> bool {
    if let Some(arr) = value.as_array() {
        for item in arr {
            if let Some(hooks) = item.get("hooks").and_then(|h| h.as_array()) {
                for hook in hooks {
                    if let Some(cmd) = hook.get("command").and_then(|c| c.as_str()) {
                        if cmd.contains("cc-switch memory hooks ingest") {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

fn remove_cc_switch_hook(value: &mut serde_json::Value) {
    if let Some(arr) = value.as_array_mut() {
        // Remove cc-switch hooks from each item, then drop items with empty hooks
        for item in arr.iter_mut() {
            if let Some(hooks) = item.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                hooks.retain(|hook| {
                    hook.get("command")
                        .and_then(|c| c.as_str())
                        .map(|cmd| !cmd.contains("cc-switch memory hooks ingest"))
                        .unwrap_or(true)
                });
            }
        }
        // Remove items whose hooks array is now empty
        arr.retain(|item| {
            item.get("hooks")
                .and_then(|h| h.as_array())
                .map(|hooks| !hooks.is_empty())
                .unwrap_or(true)
        });
    }
}

// ============================================================================
// SQL Export Helpers
// ============================================================================

fn memory_table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, AppError> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .map_err(|e| AppError::Message(e.to_string()))?;
    let iter = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| AppError::Message(e.to_string()))?;
    let mut cols = Vec::new();
    for c in iter {
        cols.push(c.map_err(|e| AppError::Message(e.to_string()))?);
    }
    Ok(cols)
}

fn format_memory_sql_value(value: rusqlite::types::ValueRef<'_>) -> String {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => "NULL".to_string(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => f.to_string(),
        ValueRef::Text(t) => {
            let text = std::str::from_utf8(t).unwrap_or("");
            let escaped = text.replace('\'', "''");
            format!("'{escaped}'")
        }
        ValueRef::Blob(bytes) => {
            let mut s = String::from("X'");
            for b in bytes {
                use std::fmt::Write;
                let _ = write!(&mut s, "{b:02X}");
            }
            s.push('\'');
            s
        }
    }
}
