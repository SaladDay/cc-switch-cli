//! Schema-free session identity rules for retained proxy usage.

use crate::app_config::AppType;
use crate::error::AppError;

pub(crate) const CLAUDE_STABLE_SESSION_PROVIDER_TYPE: &str = "ccswitch:claude-session";
pub(crate) const CODEX_NATIVE_SESSION_PROVIDER_TYPE: &str = "ccswitch:codex-session-native";
pub(crate) const CODEX_TRANSPORT_SESSION_PROVIDER_TYPE: &str = "ccswitch:codex-session-transport";
const CODEX_TRANSPORT_PREFIX: &str = "codex_";
const CODEX_IMPORTED_SESSION_KEYS_CTE: &str = "ccswitch_codex_imported_session_keys";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProxySessionStorage {
    pub session_id: String,
    pub provider_type: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProxySessionIdEncoding {
    Native,
    CodexTransport,
}

/// Preserve the upstream `session_id` value and record trustworthy provenance
/// in the existing `provider_type` column. Codex needs separate native and
/// transport markers because both forms may legitimately start with `codex_`.
pub(crate) fn proxy_session_storage(
    app_type: &AppType,
    session_id: &str,
    trustworthy_identity: bool,
    encoding: ProxySessionIdEncoding,
) -> ProxySessionStorage {
    let provider_type = if trustworthy_identity {
        match app_type {
            AppType::Claude => Some(CLAUDE_STABLE_SESSION_PROVIDER_TYPE),
            AppType::Codex => Some(match encoding {
                ProxySessionIdEncoding::Native => CODEX_NATIVE_SESSION_PROVIDER_TYPE,
                ProxySessionIdEncoding::CodexTransport => CODEX_TRANSPORT_SESSION_PROVIDER_TYPE,
            }),
            _ => None,
        }
    } else {
        None
    };

    ProxySessionStorage {
        session_id: session_id.to_string(),
        provider_type,
    }
}

/// Backfill a transcript-proven identity onto an untrusted proxy row with the
/// same request ID. A client-provided identity always wins.
pub(crate) fn attach_imported_session_identity(
    conn: &rusqlite::Connection,
    app_type: &AppType,
    request_id: &str,
    session_id: &str,
) -> Result<bool, AppError> {
    let session_id = session_id.trim();
    if session_id.is_empty() || !matches!(app_type, AppType::Claude | AppType::Codex) {
        return Ok(false);
    }

    let storage = proxy_session_storage(app_type, session_id, true, ProxySessionIdEncoding::Native);
    let untrusted_identity = match app_type {
        AppType::Claude => {
            let session = "TRIM(COALESCE(session_id, ''))";
            let uuid = hyphenated_uuid_sql(session);
            format!(
                "COALESCE(provider_type, '') <> '{CLAUDE_STABLE_SESSION_PROVIDER_TYPE}'
                 AND (
                     NULLIF({session}, '') IS NULL
                     OR ({uuid})
                 )"
            )
        }
        AppType::Codex => format!(
            "COALESCE(provider_type, '') NOT IN (
                '{CODEX_NATIVE_SESSION_PROVIDER_TYPE}',
                '{CODEX_TRANSPORT_SESSION_PROVIDER_TYPE}'
            )"
        ),
        _ => unreachable!("unsupported app type returned above"),
    };
    let sql = format!(
        "UPDATE proxy_request_logs
         SET session_id = ?3,
             provider_type = ?4
         WHERE request_id = ?1
           AND app_type = ?2
           AND COALESCE(data_source, 'proxy') = 'proxy'
           AND {untrusted_identity}"
    );
    let updated = conn.execute(
        &sql,
        rusqlite::params![
            request_id,
            app_type.as_str(),
            storage.session_id,
            storage.provider_type
        ],
    )?;
    Ok(updated > 0)
}

pub(crate) fn is_internal_session_provider_type(value: &str) -> bool {
    matches!(
        value,
        CLAUDE_STABLE_SESSION_PROVIDER_TYPE
            | CODEX_NATIVE_SESSION_PROVIDER_TYPE
            | CODEX_TRANSPORT_SESSION_PROVIDER_TYPE
    )
}

/// A proxy row is safe to group by session when Codex has a canonical native
/// identity, or Claude identity is supported by one of these schema-free
/// proofs:
///
/// - a marker on a new client-supplied row;
/// - a non-UUID historical value (generated fallbacks are UUIDs);
/// - another request with the same value;
/// - a retained imported transcript with the same value;
/// - a newer marked row with the same value.
///
/// Claude's evidence is one uncorrelated list subquery. Building the proven
/// set once avoids running several full-table correlated scans for every
/// candidate proxy row.
pub(crate) fn stable_proxy_session_sql(proxy_alias: &str, app_type: &AppType) -> String {
    let source = format!("COALESCE({proxy_alias}.data_source, 'proxy')");
    let session = format!("TRIM(COALESCE({proxy_alias}.session_id, ''))");
    if matches!(app_type, AppType::Codex) {
        let canonical = canonical_session_sql(proxy_alias);
        return format!(
            "(
                {source} = 'proxy'
                AND {proxy_alias}.app_type = 'codex'
                AND NULLIF({canonical}, '') IS NOT NULL
            )"
        );
    }

    let uuid_shape = hyphenated_uuid_sql(&session);
    let session_key = normalized_native_session_sql(&session);
    let proven_sessions = claude_proven_session_keys_sql();

    format!(
        "(
            {source} = 'proxy'
            AND {proxy_alias}.app_type = 'claude'
            AND NULLIF({session}, '') IS NOT NULL
            AND (
                COALESCE({proxy_alias}.provider_type, '') =
                    '{CLAUDE_STABLE_SESSION_PROVIDER_TYPE}'
                OR NOT ({uuid_shape})
                OR {session_key} IN (
                    {proven_sessions}
                )
            )
        )"
    )
}

pub(crate) fn normalized_native_session_sql(value: &str) -> String {
    let uuid = hyphenated_uuid_sql(value);
    format!(
        "CASE
            WHEN ({uuid}) THEN LOWER({value})
            ELSE {value}
         END"
    )
}

/// CTE shared by Codex session grouping and cross-source deduplication.
///
/// Legacy proxy rows did not record whether a leading `codex_` came from the
/// extractor or belonged to the native ID. A retained transcript ID can prove
/// exactly one interpretation. Materializing this set once keeps both lookups
/// uncorrelated with the outer request-log scan.
pub(crate) fn codex_imported_session_keys_cte_sql(enabled: bool) -> String {
    let identity = "codex_identity";
    let session = format!("TRIM(COALESCE({identity}.session_id, ''))");
    let session_key = normalized_native_session_sql(&session);
    let disabled = if enabled { "" } else { "AND 1 = 0" };
    format!(
        "{CODEX_IMPORTED_SESSION_KEYS_CTE}(session_key) AS MATERIALIZED (
            SELECT DISTINCT {session_key}
            FROM proxy_request_logs {identity}
            WHERE {identity}.app_type = 'codex'
              AND COALESCE({identity}.data_source, 'proxy') = 'codex_session'
              AND NULLIF({session}, '') IS NOT NULL
              {disabled}
        )"
    )
}

fn codex_proxy_canonical_session_sql(proxy_alias: &str) -> String {
    let session = format!("TRIM(COALESCE({proxy_alias}.session_id, ''))");
    let provider_type = format!("COALESCE({proxy_alias}.provider_type, '')");
    let stripped = format!("substr({session}, 7)");
    let has_transport_prefix =
        format!("LOWER(substr({session}, 1, 6)) = '{CODEX_TRANSPORT_PREFIX}'");
    let raw_key = normalized_native_session_sql(&session);
    let stripped_key = normalized_native_session_sql(&stripped);
    let raw_is_imported =
        format!("{raw_key} IN (SELECT session_key FROM {CODEX_IMPORTED_SESSION_KEYS_CTE})");
    let stripped_is_imported =
        format!("{stripped_key} IN (SELECT session_key FROM {CODEX_IMPORTED_SESSION_KEYS_CTE})");

    format!(
        "CASE
            WHEN {provider_type} = '{CODEX_NATIVE_SESSION_PROVIDER_TYPE}'
            THEN NULLIF({raw_key}, '')
            WHEN {provider_type} = '{CODEX_TRANSPORT_SESSION_PROVIDER_TYPE}'
             AND {has_transport_prefix}
            THEN NULLIF({stripped_key}, '')
            WHEN {provider_type} = '{CODEX_TRANSPORT_SESSION_PROVIDER_TYPE}'
            THEN NULL
            WHEN {has_transport_prefix}
             AND ({raw_is_imported})
             AND NOT ({stripped_is_imported})
            THEN {raw_key}
            WHEN {has_transport_prefix}
             AND ({stripped_is_imported})
             AND NOT ({raw_is_imported})
            THEN NULLIF({stripped_key}, '')
            WHEN NOT ({has_transport_prefix})
             AND ({raw_is_imported})
            THEN {raw_key}
            ELSE NULL
         END"
    )
}

/// Resolve a stored request-log identity to its canonical native form.
///
/// Marked rows are deterministic. Unmarked historical Codex rows are accepted
/// only when retained transcript evidence selects exactly one interpretation;
/// ambiguous or unproven values remain unattributed.
pub(crate) fn canonical_session_sql(log_alias: &str) -> String {
    let source = format!("COALESCE({log_alias}.data_source, 'proxy')");
    let session = format!("TRIM(COALESCE({log_alias}.session_id, ''))");
    let codex_proxy_canonical = codex_proxy_canonical_session_sql(log_alias);
    let native = normalized_native_session_sql(&session);
    format!(
        "CASE
            WHEN {log_alias}.app_type = 'codex'
             AND {source} = 'proxy'
            THEN ({codex_proxy_canonical})
            ELSE {native}
         END"
    )
}

fn claude_proven_session_keys_sql() -> String {
    let identity = "claude_identity";
    let session = format!("TRIM(COALESCE({identity}.session_id, ''))");
    let session_key = normalized_native_session_sql(&session);
    format!(
        "WITH
            claude_identity_rows AS MATERIALIZED (
                SELECT
                    {session_key} AS session_key,
                    COALESCE({identity}.data_source, 'proxy') AS data_source,
                    COALESCE({identity}.provider_type, '') AS provider_type
                FROM proxy_request_logs {identity}
                WHERE {identity}.app_type = 'claude'
                  AND NULLIF({session}, '') IS NOT NULL
            ),
            claude_proven_sessions AS MATERIALIZED (
                SELECT session_key
                FROM claude_identity_rows
                WHERE data_source = 'session_log'
                   OR (
                       data_source = 'proxy'
                       AND provider_type =
                           '{CLAUDE_STABLE_SESSION_PROVIDER_TYPE}'
                   )
                UNION
                SELECT session_key
                FROM claude_identity_rows
                WHERE data_source = 'proxy'
                GROUP BY session_key
                HAVING COUNT(*) > 1
            )
         SELECT session_key
         FROM claude_proven_sessions"
    )
}

pub(crate) fn hyphenated_uuid_sql(value: &str) -> String {
    format!(
        "length({value}) = 36
         AND substr({value}, 9, 1) = '-'
         AND substr({value}, 14, 1) = '-'
         AND substr({value}, 19, 1) = '-'
         AND substr({value}, 24, 1) = '-'
         AND length(replace({value}, '-', '')) = 32
         AND replace({value}, '-', '') NOT GLOB '*[^0-9A-Fa-f]*'"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_preserves_upstream_session_ids() {
        assert_eq!(
            proxy_session_storage(
                &AppType::Claude,
                "session-a",
                true,
                ProxySessionIdEncoding::Native,
            ),
            ProxySessionStorage {
                session_id: "session-a".to_string(),
                provider_type: Some(CLAUDE_STABLE_SESSION_PROVIDER_TYPE),
            }
        );
        assert_eq!(
            proxy_session_storage(
                &AppType::Claude,
                "generated",
                false,
                ProxySessionIdEncoding::Native,
            ),
            ProxySessionStorage {
                session_id: "generated".to_string(),
                provider_type: None,
            }
        );
        assert_eq!(
            proxy_session_storage(
                &AppType::Codex,
                "codex_AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE",
                true,
                ProxySessionIdEncoding::CodexTransport,
            ),
            ProxySessionStorage {
                session_id: "codex_AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE".to_string(),
                provider_type: Some(CODEX_TRANSPORT_SESSION_PROVIDER_TYPE),
            }
        );
        assert_eq!(
            proxy_session_storage(
                &AppType::Codex,
                "native-thread",
                true,
                ProxySessionIdEncoding::Native,
            ),
            ProxySessionStorage {
                session_id: "native-thread".to_string(),
                provider_type: Some(CODEX_NATIVE_SESSION_PROVIDER_TYPE),
            }
        );
        assert_eq!(
            proxy_session_storage(
                &AppType::Codex,
                "codex_native-thread",
                true,
                ProxySessionIdEncoding::Native,
            ),
            ProxySessionStorage {
                session_id: "codex_native-thread".to_string(),
                provider_type: Some(CODEX_NATIVE_SESSION_PROVIDER_TYPE),
            }
        );
    }

    #[test]
    fn all_session_identity_markers_are_internal() {
        assert!(is_internal_session_provider_type(
            CLAUDE_STABLE_SESSION_PROVIDER_TYPE
        ));
        assert!(is_internal_session_provider_type(
            CODEX_NATIVE_SESSION_PROVIDER_TYPE
        ));
        assert!(is_internal_session_provider_type(
            CODEX_TRANSPORT_SESSION_PROVIDER_TYPE
        ));
        assert!(!is_internal_session_provider_type("codex_oauth"));
    }

    #[test]
    fn transcript_identity_backfills_an_existing_proxy_row() -> Result<(), AppError> {
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE proxy_request_logs (
                request_id TEXT PRIMARY KEY,
                app_type TEXT NOT NULL,
                session_id TEXT,
                provider_type TEXT,
                data_source TEXT
            );
            INSERT INTO proxy_request_logs (
                request_id, app_type, session_id, provider_type, data_source
            ) VALUES (
                'session:message-1',
                'claude',
                'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
                NULL,
                'proxy'
            );",
        )?;

        assert!(attach_imported_session_identity(
            &conn,
            &AppType::Claude,
            "session:message-1",
            "real-session"
        )?);
        let stored = conn.query_row(
            "SELECT session_id, provider_type
             FROM proxy_request_logs
             WHERE request_id = 'session:message-1'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;
        assert_eq!(
            stored,
            (
                "real-session".to_string(),
                CLAUDE_STABLE_SESSION_PROVIDER_TYPE.to_string()
            )
        );
        assert!(!attach_imported_session_identity(
            &conn,
            &AppType::Claude,
            "missing",
            "real-session"
        )?);
        Ok(())
    }

    #[test]
    fn transcript_identity_backfills_empty_claude_proxy_identity() -> Result<(), AppError> {
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE proxy_request_logs (
                request_id TEXT PRIMARY KEY,
                app_type TEXT NOT NULL,
                session_id TEXT,
                provider_type TEXT,
                data_source TEXT
            );
            INSERT INTO proxy_request_logs (
                request_id, app_type, session_id, provider_type, data_source
            ) VALUES
                ('null-session', 'claude', NULL, NULL, 'proxy'),
                ('blank-session', 'claude', '   ', NULL, 'proxy'),
                ('marked-blank', 'claude', '', 'ccswitch:claude-session', 'proxy');",
        )?;

        assert!(attach_imported_session_identity(
            &conn,
            &AppType::Claude,
            "null-session",
            "transcript-null"
        )?);
        assert!(attach_imported_session_identity(
            &conn,
            &AppType::Claude,
            "blank-session",
            "transcript-blank"
        )?);
        assert!(!attach_imported_session_identity(
            &conn,
            &AppType::Claude,
            "marked-blank",
            "transcript-marked"
        )?);

        let sessions = conn.query_row(
            "SELECT
                MAX(CASE WHEN request_id = 'null-session' THEN session_id END),
                MAX(CASE WHEN request_id = 'blank-session' THEN session_id END),
                MAX(CASE WHEN request_id = 'marked-blank' THEN session_id END)
             FROM proxy_request_logs",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        assert_eq!(
            sessions,
            (
                "transcript-null".to_string(),
                "transcript-blank".to_string(),
                String::new()
            )
        );
        let marked_backfills: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM proxy_request_logs
             WHERE request_id IN ('null-session', 'blank-session')
               AND provider_type = ?1",
            [CLAUDE_STABLE_SESSION_PROVIDER_TYPE],
            |row| row.get(0),
        )?;
        assert_eq!(marked_backfills, 2);
        Ok(())
    }

    #[test]
    fn transcript_identity_does_not_replace_a_client_provided_identity() -> Result<(), AppError> {
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE proxy_request_logs (
                request_id TEXT PRIMARY KEY,
                app_type TEXT NOT NULL,
                session_id TEXT,
                provider_type TEXT,
                data_source TEXT
            );
            INSERT INTO proxy_request_logs (
                request_id, app_type, session_id, provider_type, data_source
            ) VALUES (
                'session:message-1',
                'claude',
                'client-session',
                'ccswitch:claude-session',
                'proxy'
            );",
        )?;

        assert!(!attach_imported_session_identity(
            &conn,
            &AppType::Claude,
            "session:message-1",
            "transcript-session"
        )?);
        let session_id = conn.query_row(
            "SELECT session_id
             FROM proxy_request_logs
             WHERE request_id = 'session:message-1'",
            [],
            |row| row.get::<_, String>(0),
        )?;
        assert_eq!(session_id, "client-session");
        Ok(())
    }

    #[test]
    fn transcript_identity_preserves_historical_non_uuid_identity() -> Result<(), AppError> {
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE proxy_request_logs (
                request_id TEXT PRIMARY KEY,
                app_type TEXT NOT NULL,
                session_id TEXT,
                provider_type TEXT,
                data_source TEXT
            );
            INSERT INTO proxy_request_logs (
                request_id, app_type, session_id, provider_type, data_source
            ) VALUES (
                'session:message-1',
                'claude',
                'historical-session',
                NULL,
                'proxy'
            );",
        )?;

        assert!(!attach_imported_session_identity(
            &conn,
            &AppType::Claude,
            "session:message-1",
            "transcript-session"
        )?);
        let session_id = conn.query_row(
            "SELECT session_id
             FROM proxy_request_logs
             WHERE request_id = 'session:message-1'",
            [],
            |row| row.get::<_, String>(0),
        )?;
        assert_eq!(session_id, "historical-session");
        Ok(())
    }

    #[test]
    fn codex_transcript_backfills_only_an_untrusted_proxy_identity() -> Result<(), AppError> {
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE proxy_request_logs (
                request_id TEXT PRIMARY KEY,
                app_type TEXT NOT NULL,
                session_id TEXT,
                provider_type TEXT,
                data_source TEXT
            );
            INSERT INTO proxy_request_logs (
                request_id, app_type, session_id, provider_type, data_source
            ) VALUES
                ('generated', 'codex',
                 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee', NULL, 'proxy'),
                ('legacy-prefix', 'codex',
                 'codex_legacy-session', NULL, 'proxy'),
                ('native-marked', 'codex', 'codex_client-session',
                 'ccswitch:codex-session-native', 'proxy'),
                ('transport-marked', 'codex', 'codex_transport-session',
                 'ccswitch:codex-session-transport', 'proxy');",
        )?;

        assert!(attach_imported_session_identity(
            &conn,
            &AppType::Codex,
            "generated",
            "native-session"
        )?);
        assert!(attach_imported_session_identity(
            &conn,
            &AppType::Codex,
            "legacy-prefix",
            "codex_native-session"
        )?);
        assert!(!attach_imported_session_identity(
            &conn,
            &AppType::Codex,
            "native-marked",
            "different-native-session"
        )?);
        assert!(!attach_imported_session_identity(
            &conn,
            &AppType::Codex,
            "transport-marked",
            "different-session"
        )?);
        let sessions = conn.query_row(
            "SELECT
                MAX(CASE WHEN request_id = 'generated' THEN session_id END),
                MAX(CASE WHEN request_id = 'legacy-prefix' THEN session_id END),
                MAX(CASE WHEN request_id = 'native-marked' THEN session_id END),
                MAX(CASE WHEN request_id = 'transport-marked' THEN session_id END),
                SUM(CASE
                    WHEN request_id IN ('generated', 'legacy-prefix')
                     AND provider_type = 'ccswitch:codex-session-native'
                    THEN 1 ELSE 0
                END)
             FROM proxy_request_logs",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?;
        assert_eq!(
            sessions,
            (
                "native-session".to_string(),
                "codex_native-session".to_string(),
                "codex_client-session".to_string(),
                "codex_transport-session".to_string(),
                2
            )
        );
        Ok(())
    }

    #[test]
    fn claude_stable_session_plan_materializes_evidence_once() -> Result<(), AppError> {
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE proxy_request_logs (
                request_id TEXT PRIMARY KEY,
                app_type TEXT NOT NULL,
                session_id TEXT,
                provider_type TEXT,
                data_source TEXT
            );",
        )?;

        let stable = stable_proxy_session_sql("candidate", &AppType::Claude);
        let mut stmt = conn.prepare(&format!(
            "EXPLAIN QUERY PLAN
             SELECT candidate.request_id
             FROM proxy_request_logs candidate
             WHERE {stable}"
        ))?;
        let plan = stmt
            .query_map([], |row| row.get::<_, String>(3))?
            .collect::<Result<Vec<_>, _>>()?;

        assert!(
            plan.iter().all(|detail| !detail.contains("CORRELATED")),
            "stable-session evidence must not be rebuilt per candidate: {plan:#?}"
        );
        assert_eq!(
            plan.iter()
                .filter(|detail| detail.contains("MATERIALIZE claude_identity_rows"))
                .count(),
            1,
            "Claude identity evidence must be materialized once: {plan:#?}"
        );
        Ok(())
    }
}
