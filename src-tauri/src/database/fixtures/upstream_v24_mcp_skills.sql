-- MCP/Skills DDL from farion1231/cc-switch at
-- ca58c0313360eae7c851fbd7553c1614c7af03e7 (schema 24).
CREATE TABLE IF NOT EXISTS mcp_servers (
            id TEXT PRIMARY KEY, name TEXT NOT NULL, server_config TEXT NOT NULL,
            description TEXT, homepage TEXT, docs TEXT, tags TEXT NOT NULL DEFAULT '[]',
            enabled_claude BOOLEAN NOT NULL DEFAULT 0, enabled_codex BOOLEAN NOT NULL DEFAULT 0,
            enabled_gemini BOOLEAN NOT NULL DEFAULT 0, enabled_grokbuild BOOLEAN NOT NULL DEFAULT 0,
            enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
            enabled_mcode BOOLEAN NOT NULL DEFAULT 0,
            enabled_agy BOOLEAN NOT NULL DEFAULT 0,
            enabled_kimi BOOLEAN NOT NULL DEFAULT 0,
            enabled_dsh BOOLEAN NOT NULL DEFAULT 0,
            enabled_hermes BOOLEAN NOT NULL DEFAULT 0,
            enabled_pi BOOLEAN NOT NULL DEFAULT 0,
            enabled_omp BOOLEAN NOT NULL DEFAULT 0
        );

CREATE TABLE IF NOT EXISTS skills (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            directory TEXT NOT NULL,
            repo_owner TEXT,
            repo_name TEXT,
            repo_branch TEXT DEFAULT 'main',
            readme_url TEXT,
            enabled_claude BOOLEAN NOT NULL DEFAULT 0,
            enabled_codex BOOLEAN NOT NULL DEFAULT 0,
            enabled_gemini BOOLEAN NOT NULL DEFAULT 0,
            enabled_grokbuild BOOLEAN NOT NULL DEFAULT 0,
            enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
            enabled_mcode BOOLEAN NOT NULL DEFAULT 0,
            enabled_agy BOOLEAN NOT NULL DEFAULT 0,
            enabled_kimi BOOLEAN NOT NULL DEFAULT 0,
            enabled_dsh BOOLEAN NOT NULL DEFAULT 0,
            enabled_hermes BOOLEAN NOT NULL DEFAULT 0,
            installed_at INTEGER NOT NULL DEFAULT 0,
            content_hash TEXT,
            updated_at INTEGER NOT NULL DEFAULT 0
        );
