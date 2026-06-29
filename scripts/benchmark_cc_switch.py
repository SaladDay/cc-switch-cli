#!/usr/bin/env python3
"""Benchmark cc-switch CLI/TUI operations with generated local data.

By default the script runs in a temporary sandbox so it can be used regularly
without touching the user's cc-switch, Claude, or Codex config. Pass
--real-env to benchmark the real local environment; in that mode the script
snapshots the files it can touch, seeds data, runs timed operations, and
restores the original state in a finally block.
"""

from __future__ import annotations

import argparse
import fcntl
import json
import os
import pty
import re
import select
import shutil
import signal
import sqlite3
import statistics
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unicodedata
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Callable, Iterable


BENCH = "ccswitch-bench"
SCHEMA_VERSION = 10
OPERATION_SET_FULL = "full"
OPERATION_SET_CI_BLOCKING = "ci-blocking"
CI_CLI_OPERATIONS = {
    "startup_version",
    "startup_provider_current",
    "provider_list",
    "usage_query_show",
    "sessions_list_json",
    "sessions_show_json",
    "sessions_messages_json",
    "provider_switch_a_to_b",
    "provider_duplicate_add",
    "provider_delete_copy",
}
CI_TUI_OPERATIONS = {
    "startup_interactive",
    "open_usage",
    "open_sessions_route",
    "open_sessions_loaded",
    "open_providers",
    "provider_switch_a_to_b",
}
PROVIDER_SWITCH_CONFLICT_INPUT = b"\x1b[B\n" * 32
TUI_PROVIDER_SWITCH_CONFLICT_INPUT = b"c" * 32


class BenchmarkAbort(RuntimeError):
    """Stop the remaining benchmark work after a CI-relevant failure."""


def log(message: str) -> None:
    print(message, flush=True)


def operation_enabled(operation_set: str, surface: str, operation: str) -> bool:
    if operation_set == OPERATION_SET_FULL:
        return True
    if operation_set == OPERATION_SET_CI_BLOCKING:
        if surface == "CLI":
            return operation in CI_CLI_OPERATIONS
        if surface == "TUI":
            return operation in CI_TUI_OPERATIONS
    return False


def display_path(path: Path | None, redact_paths: bool) -> str | None:
    if path is None:
        return None
    if redact_paths:
        return f"<{path.name}>"
    return str(path)


def redact_text(text: str, replacements: dict[str, str]) -> str:
    redacted = text
    for raw, replacement in replacements.items():
        if raw:
            redacted = redacted.replace(raw, replacement)
    return redacted


def redact_summaries(summaries: list[dict], replacements: dict[str, str]) -> list[dict]:
    redacted: list[dict] = []
    for row in summaries:
        next_row = dict(row)
        messages = next_row.get("failure_messages")
        if isinstance(messages, list):
            next_row["failure_messages"] = [
                redact_text(message, replacements) if isinstance(message, str) else message
                for message in messages
            ]
        redacted.append(next_row)
    return redacted


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def resolve_user_path(raw: str | None, home: Path) -> Path | None:
    if raw is None:
        return None
    raw = raw.strip()
    if not raw:
        return None
    if raw == "~":
        return home
    if raw.startswith("~/") or raw.startswith("~\\"):
        return home / raw[2:]
    return Path(raw)


def read_json(path: Path) -> dict:
    if not path.exists():
        return {}
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return {}


def write_json(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    # cc-switch checks that sensitive files (.json, .db) have 0600 on Unix.
    if sys.platform != "win32":
        path.chmod(0o600)


@dataclass
class Paths:
    home: Path
    cc_dir: Path
    db_path: Path
    settings_path: Path
    claude_dir: Path
    claude_mcp_path: Path
    codex_dir: Path
    gemini_dir: Path
    opencode_dir: Path
    hermes_dir: Path
    openclaw_dir: Path


@dataclass
class BenchEnvironment:
    mode: str
    root: Path | None = None
    old_env: dict[str, str | None] = field(default_factory=dict)

    def cleanup(self) -> None:
        for key, value in self.old_env.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        if self.root is not None:
            shutil.rmtree(self.root, ignore_errors=True)


def configure_environment(real_env: bool) -> BenchEnvironment:
    if real_env:
        return BenchEnvironment(mode="real")

    root = Path(tempfile.mkdtemp(prefix="ccswitch-bench-home-"))
    env_updates = {
        "HOME": str(root / "home"),
        "USERPROFILE": str(root / "home"),
        "CC_SWITCH_CONFIG_DIR": str(root / "cc-switch"),
        "CLAUDE_CONFIG_DIR": str(root / "claude"),
        "CODEX_HOME": str(root / "codex"),
        "XDG_CONFIG_HOME": str(root / "xdg-config"),
        "XDG_STATE_HOME": str(root / "xdg-state"),
        "XDG_RUNTIME_DIR": str(root / "xdg-runtime"),
    }
    old_env = {key: os.environ.get(key) for key in env_updates}
    for path in env_updates.values():
        p = Path(path)
        p.mkdir(parents=True, exist_ok=True)
        # cc-switch checks that its config dir has 0700 permissions on Unix.
        if path == env_updates["CC_SWITCH_CONFIG_DIR"]:
            p.chmod(0o700)
    for key, value in env_updates.items():
        os.environ[key] = value
    return BenchEnvironment(mode="sandbox", root=root, old_env=old_env)


def resolve_paths() -> Paths:
    home = Path.home()
    cc_dir = resolve_user_path(os.environ.get("CC_SWITCH_CONFIG_DIR"), home) or (home / ".cc-switch")
    settings_path = cc_dir / "settings.json"
    settings = read_json(settings_path)

    claude_dir = (
        resolve_user_path(os.environ.get("CLAUDE_CONFIG_DIR"), home)
        or resolve_user_path(settings.get("claudeConfigDir"), home)
        or (home / ".claude")
    )
    if settings.get("claudeConfigDir"):
        claude_mcp_path = claude_dir.parent / f"{claude_dir.name}.json"
    else:
        claude_mcp_path = home / ".claude.json"

    codex_dir = (
        resolve_user_path(settings.get("codexConfigDir"), home)
        or (
            resolve_user_path(os.environ.get("CODEX_HOME"), home)
            if resolve_user_path(os.environ.get("CODEX_HOME"), home)
            and resolve_user_path(os.environ.get("CODEX_HOME"), home).is_dir()
            else None
        )
        or (home / ".codex")
    )
    gemini_dir = resolve_user_path(settings.get("geminiConfigDir"), home) or (home / ".gemini")
    opencode_dir = (
        resolve_user_path(settings.get("opencodeConfigDir"), home)
        or (home / ".config" / "opencode")
    )
    hermes_dir = resolve_user_path(settings.get("hermesConfigDir"), home) or (home / ".hermes")
    openclaw_dir = resolve_user_path(settings.get("openclawConfigDir"), home) or (home / ".openclaw")

    return Paths(
        home=home,
        cc_dir=cc_dir,
        db_path=cc_dir / "cc-switch.db",
        settings_path=settings_path,
        claude_dir=claude_dir,
        claude_mcp_path=claude_mcp_path,
        codex_dir=codex_dir,
        gemini_dir=gemini_dir,
        opencode_dir=opencode_dir,
        hermes_dir=hermes_dir,
        openclaw_dir=openclaw_dir,
    )


@dataclass
class SnapshotEntry:
    path: Path
    backup: Path
    existed: bool


class Snapshot:
    def __init__(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="ccswitch-bench-snapshot-"))
        self.entries: list[SnapshotEntry] = []

    def add(self, path: Path) -> None:
        path = path.expanduser()
        if any(entry.path == path for entry in self.entries):
            return
        backup = self.root / f"{len(self.entries):03d}"
        existed = path.exists() or path.is_symlink()
        if existed:
            if path.is_dir() and not path.is_symlink():
                shutil.copytree(path, backup, symlinks=True)
            else:
                backup.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, backup, follow_symlinks=False)
        self.entries.append(SnapshotEntry(path=path, backup=backup, existed=existed))

    def restore(self) -> None:
        for entry in reversed(self.entries):
            if entry.path.exists() or entry.path.is_symlink():
                if entry.path.is_dir() and not entry.path.is_symlink():
                    shutil.rmtree(entry.path)
                else:
                    entry.path.unlink()
            if entry.existed:
                entry.path.parent.mkdir(parents=True, exist_ok=True)
                if entry.backup.is_dir() and not entry.backup.is_symlink():
                    shutil.copytree(entry.backup, entry.path, symlinks=True)
                else:
                    shutil.copy2(entry.backup, entry.path, follow_symlinks=False)
        shutil.rmtree(self.root, ignore_errors=True)


def snapshot_paths(paths: Paths) -> Snapshot:
    snap = Snapshot()
    snap.add(paths.cc_dir)
    for path in [
        paths.claude_dir / "settings.json",
        paths.claude_dir / "claude.json",
        paths.claude_dir / "config.json",
        paths.claude_dir / "CLAUDE.md",
        paths.claude_dir / "skills",
        paths.claude_dir / "projects" / BENCH,
        paths.claude_mcp_path,
        paths.codex_dir / "auth.json",
        paths.codex_dir / "config.toml",
        paths.codex_dir / "cc-switch-model-catalog.json",
        paths.codex_dir / "models_cache.json",
        paths.codex_dir / "AGENTS.md",
        paths.codex_dir / "skills",
        paths.codex_dir / "sessions" / BENCH,
        paths.gemini_dir / ".env",
        paths.gemini_dir / "settings.json",
        paths.gemini_dir / "GEMINI.md",
        paths.gemini_dir / "skills",
        paths.opencode_dir / "opencode.json",
        paths.opencode_dir / "AGENTS.md",
        paths.opencode_dir / "skills",
        paths.hermes_dir / "config.yaml",
        paths.hermes_dir / "AGENTS.md",
        paths.hermes_dir / "skills",
        paths.openclaw_dir / "openclaw.json",
        paths.openclaw_dir / "AGENTS.md",
        paths.openclaw_dir / "skills",
    ]:
        snap.add(path)
    return snap


def connect_db(paths: Paths) -> sqlite3.Connection:
    paths.cc_dir.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(paths.db_path)
    conn.execute("PRAGMA busy_timeout = 5000")
    # cc-switch checks that .db files have 0600 permissions on Unix.
    if sys.platform != "win32":
        paths.db_path.chmod(0o600)
    return conn


def checkpoint_db(paths: Paths) -> None:
    if not paths.db_path.exists():
        return
    try:
        with connect_db(paths) as conn:
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    except sqlite3.Error:
        pass


def ensure_tables(conn: sqlite3.Connection) -> None:
    conn.executescript(
        """
        CREATE TABLE IF NOT EXISTS providers (
            id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            name TEXT NOT NULL,
            settings_config TEXT NOT NULL,
            website_url TEXT,
            category TEXT,
            created_at INTEGER,
            sort_index INTEGER,
            notes TEXT,
            icon TEXT,
            icon_color TEXT,
            meta TEXT NOT NULL DEFAULT '{}',
            is_current BOOLEAN NOT NULL DEFAULT 0,
            in_failover_queue BOOLEAN NOT NULL DEFAULT 0,
            PRIMARY KEY (id, app_type)
        );
        CREATE TABLE IF NOT EXISTS provider_endpoints (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            provider_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            url TEXT NOT NULL,
            added_at INTEGER
        );
        CREATE TABLE IF NOT EXISTS mcp_servers (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            server_config TEXT NOT NULL,
            description TEXT,
            homepage TEXT,
            docs TEXT,
            tags TEXT NOT NULL DEFAULT '[]',
            enabled_claude BOOLEAN NOT NULL DEFAULT 0,
            enabled_codex BOOLEAN NOT NULL DEFAULT 0,
            enabled_gemini BOOLEAN NOT NULL DEFAULT 0,
            enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
            enabled_hermes BOOLEAN NOT NULL DEFAULT 0
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
            enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
            enabled_hermes BOOLEAN NOT NULL DEFAULT 0,
            installed_at INTEGER NOT NULL DEFAULT 0,
            content_hash TEXT,
            updated_at INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT);
        CREATE TABLE IF NOT EXISTS proxy_config (
            app_type TEXT PRIMARY KEY,
            proxy_enabled INTEGER NOT NULL DEFAULT 0,
            listen_address TEXT NOT NULL DEFAULT '127.0.0.1',
            listen_port INTEGER NOT NULL DEFAULT 15721,
            enable_logging INTEGER NOT NULL DEFAULT 1,
            enabled INTEGER NOT NULL DEFAULT 0,
            auto_failover_enabled INTEGER NOT NULL DEFAULT 0,
            max_retries INTEGER NOT NULL DEFAULT 3,
            streaming_first_byte_timeout INTEGER NOT NULL DEFAULT 60,
            streaming_idle_timeout INTEGER NOT NULL DEFAULT 120,
            non_streaming_timeout INTEGER NOT NULL DEFAULT 600,
            circuit_failure_threshold INTEGER NOT NULL DEFAULT 4,
            circuit_success_threshold INTEGER NOT NULL DEFAULT 2,
            circuit_timeout_seconds INTEGER NOT NULL DEFAULT 60,
            circuit_error_rate_threshold REAL NOT NULL DEFAULT 0.6,
            circuit_min_requests INTEGER NOT NULL DEFAULT 10,
            default_cost_multiplier TEXT NOT NULL DEFAULT '1',
            pricing_model_source TEXT NOT NULL DEFAULT 'response',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS proxy_live_backup (
            app_type TEXT PRIMARY KEY,
            original_config TEXT NOT NULL,
            backed_up_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS proxy_request_logs (
            request_id TEXT PRIMARY KEY,
            provider_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            model TEXT NOT NULL,
            request_model TEXT,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            input_cost_usd TEXT NOT NULL DEFAULT '0',
            output_cost_usd TEXT NOT NULL DEFAULT '0',
            cache_read_cost_usd TEXT NOT NULL DEFAULT '0',
            cache_creation_cost_usd TEXT NOT NULL DEFAULT '0',
            total_cost_usd TEXT NOT NULL DEFAULT '0',
            latency_ms INTEGER NOT NULL,
            first_token_ms INTEGER,
            duration_ms INTEGER,
            status_code INTEGER NOT NULL,
            error_message TEXT,
            session_id TEXT,
            provider_type TEXT,
            is_streaming INTEGER NOT NULL DEFAULT 0,
            cost_multiplier TEXT NOT NULL DEFAULT '1.0',
            created_at INTEGER NOT NULL,
            data_source TEXT NOT NULL DEFAULT 'proxy'
        );
        CREATE TABLE IF NOT EXISTS usage_daily_rollups (
            date TEXT NOT NULL,
            app_type TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            model TEXT NOT NULL,
            request_count INTEGER NOT NULL DEFAULT 0,
            success_count INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            total_cost_usd TEXT NOT NULL DEFAULT '0',
            avg_latency_ms INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (date, app_type, provider_id, model)
        );
        CREATE TABLE IF NOT EXISTS session_log_sync (
            file_path TEXT PRIMARY KEY,
            last_modified INTEGER NOT NULL,
            last_line_offset INTEGER NOT NULL DEFAULT 0,
            last_synced_at INTEGER NOT NULL
        );
        """
    )
    for ddl in [
        "ALTER TABLE proxy_config ADD COLUMN live_takeover_active INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE providers ADD COLUMN in_failover_queue BOOLEAN NOT NULL DEFAULT 0",
        "ALTER TABLE proxy_request_logs ADD COLUMN request_model TEXT",
        "ALTER TABLE proxy_request_logs ADD COLUMN provider_type TEXT",
        "ALTER TABLE proxy_request_logs ADD COLUMN session_id TEXT",
        "ALTER TABLE proxy_request_logs ADD COLUMN data_source TEXT NOT NULL DEFAULT 'proxy'",
    ]:
        try:
            conn.execute(ddl)
        except sqlite3.Error:
            pass
    conn.execute(f"PRAGMA user_version = {SCHEMA_VERSION}")
    for app, retries in [("claude", 6), ("codex", 3), ("gemini", 5)]:
        conn.execute(
            """
            INSERT OR IGNORE INTO proxy_config
            (app_type, max_retries, streaming_first_byte_timeout, streaming_idle_timeout,
             non_streaming_timeout, circuit_failure_threshold, circuit_success_threshold,
             circuit_timeout_seconds, circuit_error_rate_threshold, circuit_min_requests)
            VALUES (?, ?, 60, 120, 600, 4, 2, 60, 0.6, 10)
            """,
            (app, retries),
        )


def usage_meta(api_format: str) -> dict:
    return {
        "apiFormat": api_format,
        "commonConfigEnabled": False,
        "usage_script": {
            "enabled": False,
            "language": "javascript",
            "code": "return { success: true, data: [{ planName: 'bench', remaining: 1, total: 1, unit: 'credit' }] };",
            "timeout": 2,
            "templateType": "custom",
            "autoQueryInterval": 0,
        },
    }


def claude_settings(provider_id: str) -> dict:
    suffix = provider_id.removeprefix(f"{BENCH}-claude-")
    return {
        "env": {
            "ANTHROPIC_AUTH_TOKEN": f"sk-bench-claude-{suffix}",
            "ANTHROPIC_BASE_URL": f"https://bench-claude-{suffix}.example.com",
            "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-3-5-sonnet-bench",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL": "claude-3-haiku-bench",
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-3-opus-bench",
        },
        "permissions": {"allow": ["Bash(ls)", "Read(**)"]},
        "includeCoAuthoredBy": False,
    }


def codex_config(provider_id: str) -> str:
    suffix = provider_id.removeprefix(f"{BENCH}-codex-")
    return f'''model = "gpt-5-bench"
model_provider = "{provider_id}"

[model_providers."{provider_id}"]
name = "Bench Codex {suffix}"
base_url = "https://bench-codex-{suffix}.example.com/v1"
wire_api = "responses"
env_key = "OPENAI_API_KEY"
'''


def codex_settings(provider_id: str) -> dict:
    suffix = provider_id.removeprefix(f"{BENCH}-codex-")
    return {
        "auth": {"OPENAI_API_KEY": f"sk-bench-codex-{suffix}"},
        "config": codex_config(provider_id),
    }


def bench_provider_name(app: str, suffix: str) -> str:
    normalized = suffix.replace(" ", "").replace("-", "").title()
    return f"Bench{app.title()}{normalized}"


def bench_provider_copy_name(app: str, suffix: str) -> str:
    return f"{bench_provider_name(app, suffix)} copy"


def seed_providers(conn: sqlite3.Connection, app: str, count: int) -> None:
    ids = [f"{BENCH}-{app}-a", f"{BENCH}-{app}-b"]
    ids.extend(f"{BENCH}-{app}-{idx:03d}" for idx in range(max(0, count - 2)))
    now_ms = int(time.time() * 1000)
    for sort_index, provider_id in enumerate(ids):
        settings = claude_settings(provider_id) if app == "claude" else codex_settings(provider_id)
        meta = usage_meta("anthropic" if app == "claude" else "openai_responses")
        conn.execute(
            """
            INSERT OR REPLACE INTO providers
            (id, app_type, name, settings_config, website_url, category, created_at,
             sort_index, notes, icon, icon_color, meta, is_current, in_failover_queue)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)
            """,
            (
                provider_id,
                app,
                bench_provider_name(app, provider_id.rsplit("-", 1)[-1]),
                json.dumps(settings, separators=(",", ":")),
                f"https://bench-{app}.example.com/{provider_id}",
                "custom",
                now_ms + sort_index,
                sort_index,
                "cc-switch benchmark provider",
                None,
                None,
                json.dumps(meta, separators=(",", ":")),
                1 if provider_id.endswith("-a") else 0,
            ),
        )
    conn.execute("UPDATE providers SET is_current = CASE WHEN id = ? THEN 1 ELSE 0 END WHERE app_type = ?", (f"{BENCH}-{app}-a", app))


def seed_mcp_and_skills(conn: sqlite3.Connection, paths: Paths, mcp_rows: int, skill_rows: int) -> None:
    now = int(time.time())
    skills_root = paths.cc_dir / "skills"
    for idx in range(mcp_rows):
        mcp_id = f"{BENCH}-mcp-{idx:03d}"
        config = {"command": "node", "args": [f"/tmp/{mcp_id}.js"], "env": {"BENCH": "1"}}
        conn.execute(
            """
            INSERT OR REPLACE INTO mcp_servers
            (id, name, server_config, description, homepage, docs, tags,
             enabled_claude, enabled_codex, enabled_gemini, enabled_opencode, enabled_hermes)
            VALUES (?, ?, ?, ?, ?, ?, ?, 1, 1, 0, 0, 0)
            """,
            (
                mcp_id,
                f"Bench MCP {idx:03d}",
                json.dumps(config, separators=(",", ":")),
                "cc-switch benchmark MCP server",
                "https://example.com",
                "https://example.com/docs",
                json.dumps(["benchmark", "cc-switch"]),
            ),
        )
    for idx in range(skill_rows):
        skill_id = f"{BENCH}-skill-{idx:03d}"
        directory = skill_id
        skill_dir = skills_root / directory
        skill_dir.mkdir(parents=True, exist_ok=True)
        (skill_dir / "SKILL.md").write_text(
            f"# Bench Skill {idx:03d}\n\nSynthetic skill for cc-switch benchmark.\n",
            encoding="utf-8",
        )
        conn.execute(
            """
            INSERT OR REPLACE INTO skills
            (id, name, description, directory, repo_owner, repo_name, repo_branch,
             readme_url, enabled_claude, enabled_codex, enabled_gemini, enabled_opencode,
             enabled_hermes, installed_at, content_hash, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, 'main', ?, 1, 1, 0, 0, 0, ?, ?, ?)
            """,
            (
                skill_id,
                f"Bench Skill {idx:03d}",
                "cc-switch benchmark skill",
                directory,
                "bench",
                skill_id,
                "https://example.com/skill",
                now,
                f"bench-{idx:03d}",
                now,
            ),
        )


def seed_usage(conn: sqlite3.Connection, rows: int) -> None:
    now = int(time.time())
    apps = ["claude", "codex"]
    models = {"claude": "claude-3-5-sonnet-bench", "codex": "gpt-5-bench"}
    for idx in range(rows):
        app = apps[idx % 2]
        provider_id = f"{BENCH}-{app}-{'a' if idx % 3 else 'b'}"
        created_at = now - (idx % (30 * 24 * 3600))
        input_tokens = 500 + (idx % 2000)
        output_tokens = 120 + (idx % 800)
        cache_read = 20 + (idx % 120)
        cache_create = idx % 60
        total_cost = (input_tokens * 0.000003 + output_tokens * 0.000015) * (1.0 + (idx % 5) / 10)
        status = 500 if idx % 37 == 0 else 200
        conn.execute(
            """
            INSERT OR REPLACE INTO proxy_request_logs
            (request_id, provider_id, app_type, model, request_model, input_tokens, output_tokens,
             cache_read_tokens, cache_creation_tokens, input_cost_usd, output_cost_usd,
             cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd, latency_ms,
             first_token_ms, duration_ms, status_code, error_message, session_id, provider_type,
             is_streaming, cost_multiplier, created_at, data_source)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '1.0', ?, 'proxy')
            """,
            (
                f"{BENCH}-usage-{idx:06d}",
                provider_id,
                app,
                models[app],
                models[app],
                input_tokens,
                output_tokens,
                cache_read,
                cache_create,
                f"{input_tokens * 0.000003:.8f}",
                f"{output_tokens * 0.000015:.8f}",
                f"{cache_read * 0.0000003:.8f}",
                f"{cache_create * 0.000001:.8f}",
                f"{total_cost:.8f}",
                250 + idx % 1800,
                80 + idx % 400,
                600 + idx % 5000,
                status,
                "synthetic benchmark error" if status >= 400 else None,
                f"{BENCH}-{app}-session-{idx % 20:03d}",
                "benchmark",
                1 if idx % 4 else 0,
                created_at,
            ),
        )
    # Daily rollups keep the usage dashboard realistic even when old rows are compacted.
    for app in apps:
        for day in range(30):
            date = (datetime.now(timezone.utc) - timedelta(days=day)).date().isoformat()
            conn.execute(
                """
                INSERT OR REPLACE INTO usage_daily_rollups
                (date, app_type, provider_id, model, request_count, success_count,
                 input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                 total_cost_usd, avg_latency_ms)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    date,
                    app,
                    f"{BENCH}-{app}-a",
                    models[app],
                    40 + day,
                    38 + day,
                    25000 + day * 500,
                    9000 + day * 150,
                    1200 + day * 20,
                    400 + day * 10,
                    f"{0.25 + day * 0.01:.6f}",
                    700 + day * 8,
                ),
            )


def iso_ts(dt: datetime) -> str:
    return dt.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


def write_jsonl(path: Path, rows: Iterable[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(json.dumps(row, separators=(",", ":")) for row in rows) + "\n", encoding="utf-8")


def seed_sessions(paths: Paths, sessions_per_app: int) -> tuple[str, str]:
    now = datetime.now(timezone.utc)
    claude_root = paths.claude_dir / "projects" / BENCH
    codex_root = paths.codex_dir / "sessions" / BENCH
    shutil.rmtree(claude_root, ignore_errors=True)
    shutil.rmtree(codex_root, ignore_errors=True)
    claude_first = f"{BENCH}-claude-session-000"
    codex_first = f"{BENCH}-codex-session-000"

    for idx in range(sessions_per_app):
        dt = now - timedelta(minutes=idx)
        sid = f"{BENCH}-claude-session-{idx:03d}"
        path = claude_root / f"{sid}.jsonl"
        write_jsonl(
            path,
            [
                {
                    "sessionId": sid,
                    "cwd": str(repo_root()),
                    "timestamp": iso_ts(dt),
                    "type": "user",
                    "message": {"role": "user", "content": f"{BENCH}-session-claude-{idx:03d} benchmark prompt"},
                },
                {
                    "sessionId": sid,
                    "cwd": str(repo_root()),
                    "timestamp": iso_ts(dt + timedelta(seconds=2)),
                    "type": "assistant",
                    "message": {"role": "assistant", "content": f"{BENCH}-detail-message-claude-{idx:03d} benchmark answer"},
                },
                {
                    "sessionId": sid,
                    "timestamp": iso_ts(dt + timedelta(seconds=3)),
                    "type": "custom-title",
                    "customTitle": f"{BENCH}-session-claude-{idx:03d}",
                },
            ],
        )
        os.utime(path, (dt.timestamp(), dt.timestamp()))

    for idx in range(sessions_per_app):
        dt = now - timedelta(minutes=idx)
        sid = f"{BENCH}-codex-session-{idx:03d}"
        path = codex_root / f"{sid}.jsonl"
        write_jsonl(
            path,
            [
                {"timestamp": iso_ts(dt), "type": "session_meta", "payload": {"id": sid, "cwd": str(repo_root()), "timestamp": iso_ts(dt)}},
                {
                    "timestamp": iso_ts(dt + timedelta(seconds=1)),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": f"{BENCH}-session-codex-{idx:03d} benchmark prompt"}],
                    },
                },
                {
                    "timestamp": iso_ts(dt + timedelta(seconds=2)),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": f"{BENCH}-detail-message-codex-{idx:03d} benchmark answer"}],
                    },
                },
            ],
        )
        os.utime(path, (dt.timestamp(), dt.timestamp()))
    return claude_first, codex_first


def update_settings(paths: Paths) -> None:
    settings = read_json(paths.settings_path)
    settings["currentProviderClaude"] = f"{BENCH}-claude-a"
    settings["currentProviderCodex"] = f"{BENCH}-codex-a"
    settings["commonConfigConfirmed"] = True
    settings["usageConfirmed"] = True
    settings["enableClaudePluginIntegration"] = False
    visible = settings.get("visibleApps") if isinstance(settings.get("visibleApps"), dict) else {}
    visible.update({"claude": True, "codex": True})
    settings["visibleApps"] = visible
    visible_settings = settings.get("visibleAppsSettings") if isinstance(settings.get("visibleAppsSettings"), dict) else {}
    visible_settings.update({"mode": "manual", "autoPromptDecided": True})
    settings["visibleAppsSettings"] = visible_settings
    migrations = settings.get("localMigrations") if isinstance(settings.get("localMigrations"), dict) else {}
    completed_at = iso_ts(datetime.now(timezone.utc))
    migrations["codexThirdPartyHistoryProviderBucketV1"] = {
        "completedAt": completed_at,
        "targetProviderId": "custom",
        "sourceProviderIds": [],
        "migratedJsonlFiles": 0,
        "migratedStateRows": 0,
        "scannedHistoryFiles": True,
    }
    migrations["codexProviderTemplateV1"] = {"completedAt": completed_at, "migratedProviderIds": []}
    settings["localMigrations"] = migrations
    write_json(paths.settings_path, settings)


def seed_data(paths: Paths, providers_per_app: int, usage_rows: int, sessions_per_app: int, mcp_rows: int, skill_rows: int) -> tuple[str, str]:
    with connect_db(paths) as conn:
        ensure_tables(conn)
        conn.execute("DELETE FROM provider_endpoints WHERE provider_id LIKE ?", (f"{BENCH}-%",))
        conn.execute("DELETE FROM providers WHERE id LIKE ?", (f"{BENCH}-%",))
        conn.execute("DELETE FROM mcp_servers WHERE id LIKE ?", (f"{BENCH}-%",))
        conn.execute("DELETE FROM skills WHERE id LIKE ?", (f"{BENCH}-%",))
        conn.execute("DELETE FROM proxy_request_logs WHERE request_id LIKE ?", (f"{BENCH}-%",))
        conn.execute("DELETE FROM usage_daily_rollups WHERE provider_id LIKE ?", (f"{BENCH}-%",))
        conn.execute("DELETE FROM session_log_sync WHERE file_path LIKE ?", (f"%{BENCH}%",))
        conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('official_providers_seeded', 'true')")
        conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('common_config_upstream_semantics_migrated_v1', 'true')")
        conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('skills_ssot_migration_pending', 'false')")
        seed_providers(conn, "claude", providers_per_app)
        seed_providers(conn, "codex", providers_per_app)
        seed_mcp_and_skills(conn, paths, mcp_rows, skill_rows)
        seed_usage(conn, usage_rows)
    first_sessions = seed_sessions(paths, sessions_per_app)
    update_settings(paths)
    return first_sessions


@dataclass
class RunResult:
    code: int
    ms: float
    stdout: str
    stderr: str


def run_command(
    args: list[str],
    timeout: float = 60.0,
    input_text: str | None = None,
    capture_timeout: bool = False,
) -> RunResult:
    start = time.perf_counter()
    try:
        proc = subprocess.run(args, input=input_text, text=True, capture_output=True, timeout=timeout)
        return RunResult(proc.returncode, (time.perf_counter() - start) * 1000, proc.stdout, proc.stderr)
    except subprocess.TimeoutExpired as exc:
        if not capture_timeout:
            raise
        stdout = exc.stdout if isinstance(exc.stdout, str) else (exc.stdout or b"").decode("utf-8", errors="replace")
        stderr = exc.stderr if isinstance(exc.stderr, str) else (exc.stderr or b"").decode("utf-8", errors="replace")
        message = f"timed out after {timeout:.1f}s"
        stderr = f"{message}\n{stderr}".strip()
        return RunResult(-1, (time.perf_counter() - start) * 1000, stdout, stderr)


def run_pty_command(args: list[str], send: bytes = b"", timeout: float = 60.0) -> RunResult:
    master, slave = pty.openpty()
    try:
        attrs = termios.tcgetattr(slave)
        attrs[0] &= ~getattr(termios, "IXON", 0)
        termios.tcsetattr(slave, termios.TCSANOW, attrs)
    except Exception:
        pass
    start = time.perf_counter()
    proc = subprocess.Popen(args, stdin=slave, stdout=slave, stderr=slave, close_fds=True)
    os.close(slave)
    output = bytearray()
    deadline = time.time() + timeout
    sent = False
    try:
        while time.time() < deadline:
            if send and not sent and time.perf_counter() - start > 0.15:
                os.write(master, send)
                sent = True
            ready, _, _ = select.select([master], [], [], 0.05)
            if ready:
                try:
                    chunk = os.read(master, 8192)
                except OSError:
                    break
                if not chunk:
                    break
                output.extend(chunk)
            if proc.poll() is not None:
                while True:
                    ready, _, _ = select.select([master], [], [], 0)
                    if not ready:
                        break
                    try:
                        chunk = os.read(master, 8192)
                    except OSError:
                        break
                    if not chunk:
                        break
                    output.extend(chunk)
                break
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                proc.kill()
        code = proc.wait()
    finally:
        os.close(master)
    text = output.decode("utf-8", errors="replace")
    return RunResult(code, (time.perf_counter() - start) * 1000, text, "")


def strip_ansi(text: str) -> str:
    text = re.sub(r"\x1b\][^\x07]*(?:\x07|\x1b\\)", "", text)
    text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text)
    text = text.replace("\r", "\n")
    return text


def set_pty_size(fd: int, rows: int = 40, cols: int = 120) -> None:
    try:
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    except Exception:
        pass


class TuiScreen:
    """Small ANSI screen model for ratatui snapshots.

    Ratatui redraws by moving the cursor around the alternate screen. A plain
    stripped output buffer mixes old and new frames, so benchmark markers should
    read a current screen snapshot instead.
    """

    def __init__(self, rows: int = 40, cols: int = 120) -> None:
        self.rows = rows
        self.cols = cols
        self.saved_row = 0
        self.saved_col = 0
        self.reset()

    def reset(self) -> None:
        self.grid = [[" "] * self.cols for _ in range(self.rows)]
        self.row = 0
        self.col = 0

    def text(self) -> str:
        return "\n".join("".join(row).rstrip() for row in self.grid)

    def feed(self, data: str) -> None:
        i = 0
        while i < len(data):
            ch = data[i]
            if ch == "\x1b":
                i = self._consume_escape(data, i)
                continue
            if ch == "\r":
                self.col = 0
            elif ch == "\n":
                self._linefeed()
            elif ch == "\b":
                self.col = max(0, self.col - 1)
            elif ch == "\t":
                self.col = min(self.cols - 1, ((self.col // 8) + 1) * 8)
            elif ch >= " ":
                self._put(ch)
            i += 1

    def _consume_escape(self, data: str, i: int) -> int:
        if i + 1 >= len(data):
            return i + 1
        kind = data[i + 1]
        if kind == "[":
            j = i + 2
            while j < len(data) and not ("@" <= data[j] <= "~"):
                j += 1
            if j < len(data):
                self._handle_csi(data[i + 2 : j], data[j])
                return j + 1
            return len(data)
        if kind == "]":
            j = i + 2
            while j < len(data):
                if data[j] == "\x07":
                    return j + 1
                if data[j] == "\x1b" and j + 1 < len(data) and data[j + 1] == "\\":
                    return j + 2
                j += 1
            return len(data)
        if kind == "7":
            self.saved_row, self.saved_col = self.row, self.col
        elif kind == "8":
            self.row, self.col = self.saved_row, self.saved_col
        elif kind == "c":
            self.reset()
        return i + 2

    def _params(self, raw: str) -> tuple[bool, list[int | None]]:
        raw = raw.strip()
        private = raw.startswith("?")
        raw = raw.lstrip("?")
        if not raw:
            return private, []
        params: list[int | None] = []
        for part in raw.split(";"):
            if not part:
                params.append(None)
                continue
            match = re.match(r"\d+", part)
            params.append(int(match.group(0)) if match else None)
        return private, params

    def _handle_csi(self, raw: str, final: str) -> None:
        private, params = self._params(raw)
        first = params[0] if params else None
        n = first or 1

        if final in ("H", "f"):
            row = (params[0] if len(params) >= 1 and params[0] is not None else 1) - 1
            col = (params[1] if len(params) >= 2 and params[1] is not None else 1) - 1
            self.row = self._clamp(row, 0, self.rows - 1)
            self.col = self._clamp(col, 0, self.cols - 1)
        elif final == "A":
            self.row = max(0, self.row - n)
        elif final == "B":
            self.row = min(self.rows - 1, self.row + n)
        elif final == "C":
            self.col = min(self.cols - 1, self.col + n)
        elif final == "D":
            self.col = max(0, self.col - n)
        elif final == "E":
            self.row = min(self.rows - 1, self.row + n)
            self.col = 0
        elif final == "F":
            self.row = max(0, self.row - n)
            self.col = 0
        elif final == "G":
            self.col = self._clamp(n - 1, 0, self.cols - 1)
        elif final == "d":
            self.row = self._clamp(n - 1, 0, self.rows - 1)
        elif final == "J":
            mode = first or 0
            if mode in (2, 3):
                self.reset()
            elif mode == 0:
                self._clear_to_screen_end()
            elif mode == 1:
                self._clear_to_screen_start()
        elif final == "K":
            mode = first or 0
            if mode == 0:
                self.grid[self.row][self.col :] = [" "] * (self.cols - self.col)
            elif mode == 1:
                self.grid[self.row][: self.col + 1] = [" "] * (self.col + 1)
            elif mode == 2:
                self.grid[self.row] = [" "] * self.cols
        elif final == "s":
            self.saved_row, self.saved_col = self.row, self.col
        elif final == "u":
            self.row, self.col = self.saved_row, self.saved_col
        elif final in ("h", "l") and private and 1049 in [p for p in params if p is not None]:
            self.reset()

    def _clear_to_screen_end(self) -> None:
        self.grid[self.row][self.col :] = [" "] * (self.cols - self.col)
        for row in range(self.row + 1, self.rows):
            self.grid[row] = [" "] * self.cols

    def _clear_to_screen_start(self) -> None:
        for row in range(0, self.row):
            self.grid[row] = [" "] * self.cols
        self.grid[self.row][: self.col + 1] = [" "] * (self.col + 1)

    def _put(self, ch: str) -> None:
        if self.col >= self.cols:
            self._linefeed()
            self.col = 0
        width = self._char_width(ch)
        self.grid[self.row][self.col] = ch
        if width == 2 and self.col + 1 < self.cols:
            self.grid[self.row][self.col + 1] = " "
        self.col += width
        if self.col >= self.cols:
            self.col = self.cols - 1

    def _linefeed(self) -> None:
        if self.row >= self.rows - 1:
            self.grid.pop(0)
            self.grid.append([" "] * self.cols)
        else:
            self.row += 1

    @staticmethod
    def _char_width(ch: str) -> int:
        return 2 if unicodedata.east_asian_width(ch) in {"F", "W"} else 1

    @staticmethod
    def _clamp(value: int, low: int, high: int) -> int:
        return max(low, min(high, value))


class TuiSession:
    def __init__(self, args: list[str], timeout: float = 30.0) -> None:
        self.master, self.slave = pty.openpty()
        set_pty_size(self.slave)
        try:
            attrs = termios.tcgetattr(self.slave)
            attrs[0] &= ~getattr(termios, "IXON", 0)
            termios.tcsetattr(self.slave, termios.TCSANOW, attrs)
        except Exception:
            pass
        env = os.environ.copy()
        env.setdefault("TERM", "xterm-256color")
        self.proc = subprocess.Popen(args, stdin=self.slave, stdout=self.slave, stderr=self.slave, env=env, close_fds=True)
        os.close(self.slave)
        self.buffer = ""
        self.screen_state = TuiScreen()
        self.timeout = timeout

    def send(self, data: bytes | str) -> None:
        if isinstance(data, str):
            data = data.encode("utf-8")
        os.write(self.master, data)

    def clear(self) -> None:
        self.buffer = ""
        self.screen_state.reset()
        while True:
            ready, _, _ = select.select([self.master], [], [], 0)
            if not ready:
                return
            try:
                chunk = os.read(self.master, 65536)
            except OSError:
                return
            if not chunk:
                return

    def key(self, data: bytes | str, delay: float = 0.05) -> None:
        self.send(data)
        if delay > 0:
            time.sleep(delay)

    def read_some(self, wait: float = 0.05) -> bool:
        ready, _, _ = select.select([self.master], [], [], wait)
        if not ready:
            return False
        try:
            chunk = os.read(self.master, 65536)
        except OSError:
            return False
        if chunk:
            decoded = chunk.decode("utf-8", errors="replace")
            self.buffer += decoded
            self.buffer = self.buffer[-200000:]
            self.screen_state.feed(decoded)
            return True
        return False

    def screen(self) -> str:
        screen = self.screen_state.text()
        return screen if screen.strip() else strip_ansi(self.buffer)

    def wait_for(self, predicate: Callable[[str], bool], timeout: float | None = None) -> float:
        start = time.perf_counter()
        deadline = time.time() + (timeout or self.timeout)
        while time.time() < deadline:
            self.read_some(0.05)
            if predicate(self.screen()):
                return (time.perf_counter() - start) * 1000
            if self.proc.poll() is not None:
                raise RuntimeError(f"TUI exited with code {self.proc.returncode}")
        screen = self.screen()[-4000:]
        raise TimeoutError(screen if screen else "<empty TUI screen>")

    def close(self) -> None:
        try:
            self.send(b"\x03")
        except OSError:
            pass
        if self.proc.poll() is None:
            try:
                self.proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self.proc.terminate()
                try:
                    self.proc.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    self.proc.kill()
        try:
            os.close(self.master)
        except OSError:
            pass


@dataclass
class Metric:
    surface: str
    app: str
    operation: str
    samples: list[float] = field(default_factory=list)
    failures: list[str] = field(default_factory=list)

    def add(self, ms: float) -> None:
        self.samples.append(ms)

    def fail(self, message: str) -> None:
        lines = [line.strip() for line in message.splitlines() if line.strip()]
        first = " | ".join(lines[:3]) if lines else (message.strip() or "unknown failure")
        self.failures.append(first[:240])

    def summary(self) -> dict:
        if self.samples:
            p95 = (
                max(self.samples)
                if len(self.samples) < 2
                else statistics.quantiles(self.samples, n=20, method="inclusive")[18]
            )
        else:
            p95 = None
        return {
            "surface": self.surface,
            "app": self.app,
            "operation": self.operation,
            "samples": len(self.samples),
            "failures": len(self.failures),
            "median_ms": round(statistics.median(self.samples), 2) if self.samples else None,
            "p95_ms": round(p95, 2) if self.samples else None,
            "min_ms": round(min(self.samples), 2) if self.samples else None,
            "max_ms": round(max(self.samples), 2) if self.samples else None,
            "failure_messages": self.failures[:3],
        }


class Metrics:
    def __init__(self) -> None:
        self._items: dict[tuple[str, str, str], Metric] = {}

    def metric(self, surface: str, app: str, operation: str) -> Metric:
        key = (surface, app, operation)
        if key not in self._items:
            self._items[key] = Metric(surface, app, operation)
        return self._items[key]

    def add(self, surface: str, app: str, operation: str, ms: float) -> None:
        self.metric(surface, app, operation).add(ms)

    def fail(self, surface: str, app: str, operation: str, message: str) -> None:
        self.metric(surface, app, operation).fail(message)

    def summaries(self) -> list[dict]:
        return [m.summary() for m in sorted(self._items.values(), key=lambda x: (x.surface, x.app, x.operation))]


def current_provider(paths: Paths, app: str) -> str | None:
    with connect_db(paths) as conn:
        row = conn.execute("SELECT id FROM providers WHERE app_type = ? AND is_current = 1 LIMIT 1", (app,)).fetchone()
        return row[0] if row else None


def current_provider_setting(paths: Paths, app: str) -> str | None:
    key = f"currentProvider{app.capitalize()}"
    settings = read_json(paths.settings_path)
    value = settings.get(key)
    return value if isinstance(value, str) and value else None


def effective_current_provider(paths: Paths, app: str) -> str | None:
    return current_provider_setting(paths, app) or current_provider(paths, app)


def provider_exists(paths: Paths, app: str, provider_id: str) -> bool:
    with connect_db(paths) as conn:
        row = conn.execute("SELECT 1 FROM providers WHERE app_type = ? AND id = ?", (app, provider_id)).fetchone()
        return row is not None


def find_provider_by_name_prefix(paths: Paths, app: str, name: str, id_prefix: str) -> str | None:
    with connect_db(paths) as conn:
        row = conn.execute(
            """
            SELECT id FROM providers
            WHERE app_type = ? AND name = ? AND id LIKE ?
            ORDER BY created_at DESC, id DESC
            LIMIT 1
            """,
            (app, name, f"{id_prefix}%"),
        ).fetchone()
        return row[0] if row else None


def find_provider_by_prefix(paths: Paths, app: str, id_prefix: str) -> str | None:
    with connect_db(paths) as conn:
        row = conn.execute(
            """
            SELECT id FROM providers
            WHERE app_type = ? AND id LIKE ?
            ORDER BY created_at DESC, id DESC
            LIMIT 1
            """,
            (app, f"{id_prefix}%"),
        ).fetchone()
        return row[0] if row else None


def remove_provider_prefix_direct(paths: Paths, app: str, provider_id_prefix: str) -> None:
    with connect_db(paths) as conn:
        conn.execute(
            "DELETE FROM provider_endpoints WHERE app_type = ? AND provider_id LIKE ?",
            (app, f"{provider_id_prefix}%"),
        )
        conn.execute(
            "DELETE FROM providers WHERE app_type = ? AND id LIKE ?",
            (app, f"{provider_id_prefix}%"),
        )


def provider_row_index(paths: Paths, app: str, provider_id: str) -> int:
    with connect_db(paths) as conn:
        rows = conn.execute(
            """
            SELECT id FROM providers
            WHERE app_type = ?
            ORDER BY COALESCE(sort_index, 999999), created_at ASC, id ASC
            """,
            (app,),
        ).fetchall()
    for idx, (row_id,) in enumerate(rows):
        if row_id == provider_id:
            return idx
    raise RuntimeError(f"provider not found in visible order: {app}/{provider_id}")


def remove_provider_direct(paths: Paths, app: str, provider_id: str) -> None:
    with connect_db(paths) as conn:
        conn.execute("DELETE FROM provider_endpoints WHERE app_type = ? AND provider_id = ?", (app, provider_id))
        conn.execute("DELETE FROM providers WHERE app_type = ? AND id = ?", (app, provider_id))


def wait_until(predicate: Callable[[], bool], timeout: float = 10.0) -> float:
    start = time.perf_counter()
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return (time.perf_counter() - start) * 1000
        time.sleep(0.03)
    raise TimeoutError("condition was not reached")


def wait_until_tui(session: TuiSession, predicate: Callable[[], bool], timeout: float = 10.0) -> float:
    start = time.perf_counter()
    deadline = time.time() + timeout
    while time.time() < deadline:
        session.read_some(0.03)
        if predicate():
            return (time.perf_counter() - start) * 1000
        time.sleep(0.02)
    raise TimeoutError("condition was not reached")


def timed_cli(
    metrics: Metrics,
    binary: Path,
    surface: str,
    app: str,
    operation: str,
    args: list[str],
    timeout: float = 60.0,
    fail_fast: bool = False,
    runner: Callable[[list[str], float, bool], RunResult] | None = None,
) -> RunResult:
    if runner is None:
        result = run_command([str(binary), *args], timeout=timeout, capture_timeout=fail_fast)
    else:
        result = runner([str(binary), *args], timeout, fail_fast)
    if result.code == 0:
        metrics.add(surface, app, operation, result.ms)
    else:
        message = result.stderr or result.stdout or f"exit {result.code}"
        metrics.fail(surface, app, operation, message)
        if fail_fast:
            raise BenchmarkAbort(f"{surface} {app} {operation} failed: {message}")
    return result


def run_provider_switch_cli(args: list[str], timeout: float = 60.0, _capture_timeout: bool = False) -> RunResult:
    return run_pty_command(args, send=PROVIDER_SWITCH_CONFLICT_INPUT, timeout=timeout)


def reset_provider_cli(binary: Path, app: str, provider_id: str) -> None:
    result = run_provider_switch_cli([str(binary), "--app", app, "provider", "switch", provider_id], timeout=60)
    if result.code != 0:
        raise RuntimeError(f"failed to reset {app} to {provider_id}: {result.stderr or result.stdout}")


def benchmark_cli(
    metrics: Metrics,
    binary: Path,
    paths: Paths,
    iterations: int,
    warmup: int,
    first_sessions: tuple[str, str],
    operation_set: str,
    fail_fast: bool,
) -> None:
    total = warmup + iterations
    cli_enabled = lambda operation: operation_enabled(operation_set, "CLI", operation)
    if cli_enabled("startup_version"):
        if operation_set == OPERATION_SET_CI_BLOCKING:
            for idx in range(total):
                record = idx >= warmup
                version_metrics = metrics if idx >= warmup else Metrics()
                timed_cli(
                    version_metrics,
                    binary,
                    "CLI",
                    "global",
                    "startup_version",
                    ["--version"],
                    fail_fast=fail_fast and record,
                )
        else:
            timed_cli(metrics, binary, "CLI", "global", "startup_version", ["--version"], fail_fast=fail_fast)
    for app, session_id in [("claude", first_sessions[0]), ("codex", first_sessions[1])]:
        a = f"{BENCH}-{app}-a"
        b = f"{BENCH}-{app}-b"
        reset_provider_cli(binary, app, a)
        for idx in range(total):
            record = idx >= warmup
            log(f"  CLI {app} iteration {idx + 1}/{total}{' (warmup)' if not record else ''}...")
            prefix_metrics = metrics if record else Metrics()
            if cli_enabled("startup_provider_current"):
                timed_cli(prefix_metrics, binary, "CLI", app, "startup_provider_current", ["--app", app, "provider", "current"], fail_fast=fail_fast and record)
            if cli_enabled("provider_list"):
                timed_cli(prefix_metrics, binary, "CLI", app, "provider_list", ["--app", app, "provider", "list"], fail_fast=fail_fast and record)
            if cli_enabled("usage_query_show"):
                timed_cli(prefix_metrics, binary, "CLI", app, "usage_query_show", ["--app", app, "provider", "usage-query", "show", a, "--json"], fail_fast=fail_fast and record)
            if cli_enabled("sessions_list_json"):
                timed_cli(prefix_metrics, binary, "CLI", app, "sessions_list_json", ["--app", app, "sessions", "list", "--json"], fail_fast=fail_fast and record)
            if cli_enabled("sessions_show_json"):
                timed_cli(prefix_metrics, binary, "CLI", app, "sessions_show_json", ["--app", app, "sessions", "show", session_id, "--json"], fail_fast=fail_fast and record)
            if cli_enabled("sessions_messages_json"):
                timed_cli(prefix_metrics, binary, "CLI", app, "sessions_messages_json", ["--app", app, "sessions", "messages", session_id, "--json"], fail_fast=fail_fast and record)

            if cli_enabled("provider_switch_a_to_b"):
                reset_provider_cli(binary, app, a)
                switched = timed_cli(
                    prefix_metrics,
                    binary,
                    "CLI",
                    app,
                    "provider_switch_a_to_b",
                    ["--app", app, "provider", "switch", b],
                    fail_fast=fail_fast and record,
                    runner=run_provider_switch_cli,
                )
                if switched.code != 0:
                    reset_provider_cli(binary, app, a)
                    continue

            if cli_enabled("provider_duplicate_add") or cli_enabled("provider_delete_copy"):
                copy_id = f"{a}-copy"
                remove_provider_prefix_direct(paths, app, copy_id)
                add_metrics = prefix_metrics if cli_enabled("provider_duplicate_add") else Metrics()
                add = timed_cli(add_metrics, binary, "CLI", app, "provider_duplicate_add", ["--app", app, "provider", "duplicate", a], fail_fast=fail_fast and record)
                if add.code == 0 and cli_enabled("provider_delete_copy"):
                    delete = run_pty_command([str(binary), "--app", app, "provider", "delete", copy_id], send=b"y\n", timeout=30)
                    if record:
                        if delete.code == 0:
                            metrics.add("CLI", app, "provider_delete_copy", delete.ms)
                        else:
                            message = delete.stdout or f"exit {delete.code}"
                            metrics.fail("CLI", app, "provider_delete_copy", message)
                            if fail_fast and record:
                                raise BenchmarkAbort(f"CLI {app} provider_delete_copy failed: {message}")
            reset_provider_cli(binary, app, a)


def tui_wait_until_quiet(session: TuiSession, quiet: float = 0.25, timeout: float = 2.0) -> None:
    deadline = time.time() + timeout
    quiet_deadline = time.time() + quiet
    while time.time() < deadline:
        if session.read_some(0.02):
            quiet_deadline = time.time() + quiet
        elif time.time() >= quiet_deadline:
            return


def tui_prepare_nav(session: TuiSession, target_idx: int) -> None:
    session.clear()
    session.key(b"\x1b[D", 0.12)  # Left: nav focus, or sessions pane back toward nav.
    for _ in range(20):
        session.key(b"\x1b[A", 0.01)
    for _ in range(max(0, target_idx)):
        session.key(b"\x1b[B", 0.08)
    tui_wait_until_quiet(session)


def tui_open_prepared_route(session: TuiSession, marker: Callable[[str], bool]) -> float:
    session.clear()
    start = time.perf_counter()
    session.send(b"\r")
    session.wait_for(marker, timeout=20)
    return (time.perf_counter() - start) * 1000


def tui_goto(session: TuiSession, current_idx: int, target_idx: int, marker: Callable[[str], bool]) -> tuple[int, float]:
    _ = current_idx
    tui_prepare_nav(session, target_idx)
    return target_idx, tui_open_prepared_route(session, marker)


def tui_clear_filter(session: TuiSession) -> None:
    session.send("/")
    time.sleep(0.03)
    session.send(b"\x1b")
    time.sleep(0.05)


def tui_filter(session: TuiSession, text: str, marker: Callable[[str], bool]) -> float:
    session.clear()
    start = time.perf_counter()
    session.send("/")
    time.sleep(0.03)
    session.send(b"\x15")  # Ctrl+U: clear any previous filter text.
    time.sleep(0.03)
    session.send(text)
    compact_text = text.replace(" ", "")
    session.wait_for(lambda s: text in s or compact_text in s, timeout=8)
    session.send(b"\r")
    time.sleep(0.15)
    session.wait_for(lambda s: not tui_filter_mode_active(s) and marker(s), timeout=20)
    return (time.perf_counter() - start) * 1000


def screen_contains_compact(screen: str, text: str) -> bool:
    return text in screen or text.replace(" ", "") in screen.replace(" ", "")


def tui_filter_mode_active(screen: str) -> bool:
    return (
        "Type to filter, Enter apply, Esc clear & exit" in screen
        or "输入关键字过滤，Enter 应用，Esc 清空并退出" in screen
    )


def tui_provider_action_marker(screen: str) -> bool:
    compact = screen.replace(" ", "")
    return (
        "Space=switch" in screen
        or "Space=切换" in screen
        or "Spaceswitch" in compact
        or "Space切换" in compact
        or "Space=add/remove" in screen
        or "Spaceadd/remove" in compact
    )


def tui_provider_row_marker(screen: str, provider_id: str, marker_text: str | None) -> bool:
    if marker_text is not None and screen_contains_compact(screen, marker_text):
        return True
    prefix = f"{BENCH}-"
    if provider_id.startswith(prefix):
        suffix_parts = provider_id.removeprefix(prefix).rsplit("-", 1)
        if len(suffix_parts) == 2:
            app, suffix = suffix_parts
            return f"https://bench-{app}-{suffix}.example.com" in screen
    return False


def tui_select_provider(
    session: TuiSession,
    provider_id: str,
    marker_text: str | None = None,
    query: str | None = None,
) -> float:
    session.send(b"\x1b[C")
    time.sleep(0.08)
    return tui_filter(
        session,
        query or provider_id,
        lambda s: tui_provider_row_marker(s, provider_id, marker_text) and tui_provider_action_marker(s),
    )


def tui_select_row(session: TuiSession, index: int) -> None:
    for _ in range(200):
        session.key(b"\x1b[A", 0.005)
    for _ in range(max(0, index)):
        session.key(b"\x1b[B", 0.04)


def tui_providers_marker(screen: str, app: str | None = None) -> bool:
    def has_provider_pair(target_app: str) -> bool:
        compact_screen = screen.replace(" ", "")
        app_title = target_app.title()
        current_url = f"https://bench-{target_app}-a." in screen
        alternate_provider = (
            f"bench-{target_app}-b." in screen
            or f"Bench{app_title}B" in compact_screen
            or f"{BENCH}-{target_app}-b" in screen
        )
        return current_url and alternate_provider

    if app is not None:
        return has_provider_pair(app)
    return has_provider_pair("claude") or has_provider_pair("codex")


def tui_sessions_loaded_marker(screen: str, app: str | None = None) -> bool:
    if app is not None and f"{BENCH}-session-{app}" not in screen:
        return False
    return (
        (
            "Sessions" in screen
            and "Title" in screen
            and "Time" in screen
            and "Messages" in screen
        )
        or (
            "会话管理" in screen
            and "标题" in screen
            and "时间" in screen
            and "消息" in screen
        )
    )


def tui_sessions_route_marker(screen: str, app: str) -> bool:
    return (
        "Scanning local sessions" in screen
        or "正在扫描本地会话" in screen
        or tui_sessions_loaded_marker(screen, app)
    )


def benchmark_tui(
    metrics: Metrics,
    binary: Path,
    paths: Paths,
    iterations: int,
    warmup: int,
    operation_set: str,
    fail_fast: bool,
) -> None:
    total = warmup + iterations
    nav = {"providers": 1, "sessions": 4, "usage": 6}
    tui_enabled = lambda operation: operation_enabled(operation_set, "TUI", operation)

    def start_session(app: str, timeout: float = 12.0) -> tuple[TuiSession, float]:
        start = time.perf_counter()
        session = TuiSession([str(binary), "--app", app, "interactive"], timeout=timeout)
        session.wait_for(
            lambda s: "CC-Switch交互模式" in s or "CC-Switch Interactive" in s or "首页" in s or "Home" in s,
            timeout=timeout,
        )
        current_name = bench_provider_name(app, "a")
        session.wait_for(
            lambda s: current_name in s
            or current_name.replace(" ", "") in s
            or f"{BENCH}-{app}-a" in s,
            timeout=timeout,
        )
        return session, (time.perf_counter() - start) * 1000

    def run_tui_op(app: str, operation: str, record: bool, body: Callable[[TuiSession], float | None]) -> None:
        session: TuiSession | None = None
        try:
            log(f"  TUI {app} {operation}{' (warmup)' if not record else ''}...")
            session, _ = start_session(app)
            ms = body(session)
            if record and ms is not None:
                metrics.add("TUI", app, operation, ms)
        except Exception as exc:
            if record:
                message = str(exc)
                metrics.fail("TUI", app, operation, message)
                if fail_fast:
                    raise BenchmarkAbort(f"TUI {app} {operation} failed: {message}") from exc
        finally:
            if session is not None:
                session.close()

    for app in ["claude", "codex"]:
        a = f"{BENCH}-{app}-a"
        b = f"{BENCH}-{app}-b"
        a_name = bench_provider_name(app, "a")
        b_name = bench_provider_name(app, "b")
        for idx in range(total):
            record = idx >= warmup
            try:
                reset_provider_cli(binary, app, a)
            except Exception as exc:
                if record:
                    message = str(exc)
                    metrics.fail("TUI", app, "provider_switch_a_to_b", message)
                    if fail_fast:
                        raise BenchmarkAbort(f"TUI {app} provider_switch_a_to_b setup failed: {message}") from exc
                continue

            session: TuiSession | None = None
            if tui_enabled("startup_interactive"):
                try:
                    log(f"  TUI {app} startup_interactive iteration {idx + 1}/{total}{' (warmup)' if not record else ''}...")
                    session, startup_ms = start_session(app)
                    if record:
                        metrics.add("TUI", app, "startup_interactive", startup_ms)
                except Exception as exc:
                    if record:
                        message = str(exc)
                        metrics.fail("TUI", app, "startup_interactive", message)
                        if fail_fast:
                            raise BenchmarkAbort(f"TUI {app} startup_interactive failed: {message}") from exc
                finally:
                    if session is not None:
                        session.close()

            if tui_enabled("open_usage"):
                run_tui_op(
                    app,
                    "open_usage",
                    record,
                    lambda session: tui_goto(
                        session,
                        0,
                        nav["usage"],
                        lambda s: "Usage Statistics" in s or "使用统计" in s or "Usage Trend" in s or "使用趋势" in s,
                    )[1],
                )

            if tui_enabled("open_sessions_route"):
                run_tui_op(
                    app,
                    "open_sessions_route",
                    record,
                    lambda session: tui_goto(
                        session,
                        0,
                        nav["sessions"],
                        lambda s: tui_sessions_route_marker(s, app),
                    )[1],
                )

            if tui_enabled("open_sessions_loaded"):
                run_tui_op(
                    app,
                    "open_sessions_loaded",
                    record,
                    lambda session: tui_goto(
                        session,
                        0,
                        nav["sessions"],
                        lambda s: tui_sessions_loaded_marker(s, app),
                    )[1],
                )

            def detail_body(session: TuiSession) -> float:
                tui_goto(session, 0, nav["sessions"], lambda s: tui_sessions_loaded_marker(s, app))
                session.wait_for(
                    lambda s: f"{BENCH}-session" in s
                    and ("sessions" in s or "个会话" in s or "Sessions" in s or "会话" in s),
                    timeout=20,
                )
                start_detail = time.perf_counter()
                session.clear()
                session.send(b"\x1b[C")
                session.wait_for(
                    lambda s: (
                        f"{BENCH}-detail-message-{app}-000" in s
                        or "Loading messages" in s
                        or "正在加载消息" in s
                    ),
                    timeout=12,
                )
                return (time.perf_counter() - start_detail) * 1000

            if tui_enabled("open_session_detail"):
                run_tui_op(app, "open_session_detail", record, detail_body)

            def providers_body(session: TuiSession) -> float:
                return tui_goto(
                    session,
                    0,
                    nav["providers"],
                    lambda s: tui_providers_marker(s, app),
                )[1]

            if tui_enabled("open_providers"):
                run_tui_op(app, "open_providers", record, providers_body)

            def switch_body(session: TuiSession) -> float:
                tui_goto(
                    session,
                    0,
                    nav["providers"],
                    lambda s: tui_providers_marker(s, app),
                )
                tui_select_provider(session, b, b_name)
                session.clear()
                start_switch = time.perf_counter()
                session.send(b" ")
                time.sleep(0.15)
                session.send(TUI_PROVIDER_SWITCH_CONFLICT_INPUT)
                wait_until_tui(session, lambda: effective_current_provider(paths, app) == b, timeout=8)
                return (time.perf_counter() - start_switch) * 1000

            if tui_enabled("provider_switch_a_to_b"):
                reset_provider_cli(binary, app, a)
                run_tui_op(app, "provider_switch_a_to_b", record, switch_body)
                try:
                    reset_provider_cli(binary, app, a)
                except Exception:
                    pass

            def copy_delete_body(session: TuiSession) -> float:
                copy_id_prefix = f"{a}-copy"
                copy_name = bench_provider_copy_name(app, "a")
                remove_provider_prefix_direct(paths, app, copy_id_prefix)
                reset_provider_cli(binary, app, b)
                tui_goto(
                    session,
                    0,
                    nav["providers"],
                    lambda s: tui_providers_marker(s, app),
                )
                tui_select_provider(session, a, a_name)
                sequence_start = time.perf_counter()
                start_copy = time.perf_counter()
                session.send("c")
                session.wait_for(
                    lambda s: (
                        "复制供应商" in s
                        or "Duplicate(copy)Provider" in s
                        or "Copy Provider" in s
                        or "confirm" in s.lower()
                        or "确认" in s
                    ),
                    timeout=8,
                )
                session.send(b"\r")
                session.wait_for(
                    lambda s: copy_name in s
                    or copy_name.replace(" ", "") in s
                    or "Ctrl+S" in s
                    and ("Add Provider" in s or "添加供应商" in s),
                    timeout=8,
                )
                time.sleep(0.25)
                session.send(b"\x13")
                copied_id: list[str | None] = [None]

                def copied_provider_exists() -> bool:
                    copied_id[0] = find_provider_by_prefix(paths, app, copy_id_prefix)
                    return copied_id[0] is not None

                wait_until_tui(session, copied_provider_exists, timeout=8)
                copy_id = copied_id[0]
                if copy_id is None:
                    raise TimeoutError("copied provider id was not found")
                copy_ms = (time.perf_counter() - start_copy) * 1000
                if record:
                    metrics.add("TUI", app, "provider_copy_add", copy_ms)

                session.wait_for(
                    lambda s: copy_name in s or copy_name.replace(" ", "") in s or copy_id in s,
                    timeout=8,
                )
                session.key(b"\x1b[B", 0.12)
                start_delete = time.perf_counter()
                session.send("d")
                session.wait_for(
                    lambda s: (
                        "删除供应商" in s
                        or "Delete Provider" in s
                        or "confirm" in s.lower()
                        or "确认" in s
                    ),
                    timeout=8,
                )
                session.send(b"\r")
                wait_until_tui(session, lambda: not provider_exists(paths, app, copy_id), timeout=8)
                delete_ms = (time.perf_counter() - start_delete) * 1000
                if record:
                    metrics.add("TUI", app, "provider_delete_copy", delete_ms)
                return (time.perf_counter() - sequence_start) * 1000

            if tui_enabled("provider_copy_delete_sequence"):
                run_tui_op(app, "provider_copy_delete_sequence", record, copy_delete_body)
            try:
                reset_provider_cli(binary, app, a)
            except Exception:
                pass

            # The TUI operations above intentionally use fresh processes. Keeping the
            # older single-process sequence disabled avoids focus/filter state bleed
            # between routes while preserving a realistic per-operation measurement.
            continue


def print_summary(summaries: list[dict]) -> None:
    headers = ["surface", "app", "operation", "samples", "failures", "median_ms", "p95_ms", "min_ms", "max_ms"]
    print("\n| " + " | ".join(headers) + " |")
    print("| " + " | ".join(["---"] * len(headers)) + " |")
    for row in summaries:
        print("| " + " | ".join(str(row.get(h, "")) for h in headers) + " |")
    failures = [row for row in summaries if row["failures"]]
    if failures:
        print("\nFailures:")
        for row in failures:
            print(f"- {row['surface']} {row['app']} {row['operation']}: {row['failure_messages']}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Benchmark cc-switch CLI and TUI operations with generated realistic data.")
    parser.add_argument("--binary", type=Path, default=repo_root() / "src-tauri" / "target" / "release" / "cc-switch")
    parser.add_argument("--iterations", type=int, default=5)
    parser.add_argument("--warmup", type=int, default=1)
    parser.add_argument("--providers-per-app", type=int, default=30)
    parser.add_argument("--usage-rows", type=int, default=2000)
    parser.add_argument("--sessions-per-app", type=int, default=20)
    parser.add_argument("--mcp-rows", type=int, default=8)
    parser.add_argument("--skill-rows", type=int, default=8)
    parser.add_argument("--skip-cli", action="store_true")
    parser.add_argument("--skip-tui", action="store_true")
    parser.add_argument("--operation-set", choices=[OPERATION_SET_FULL, OPERATION_SET_CI_BLOCKING], default=OPERATION_SET_FULL)
    parser.add_argument("--real-env", action="store_true", help="Run against the real user environment after taking a snapshot. Default is a temporary sandbox.")
    parser.add_argument("--redact-paths", action="store_true", help="Redact local filesystem paths from the JSON result.")
    parser.add_argument("--fail-fast", action="store_true", help="Stop after the first recorded benchmark failure but still write results.")
    parser.add_argument("--json-output", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.real_env and os.environ.get("CI"):
        print("--real-env is disabled when CI is set", file=sys.stderr)
        return 2

    binary = args.binary.resolve()
    if not binary.exists():
        print(f"Binary not found: {binary}", file=sys.stderr)
        return 2

    env: BenchEnvironment | None = None
    paths: Paths | None = None
    metrics = Metrics()
    snap: Snapshot | None = None
    restored = False
    cleaned = False
    interrupted = False
    result: dict | None = None
    summaries: list[dict] = []
    return_code = 0

    def handle_signal(signum: int, _frame: object) -> None:
        nonlocal interrupted
        interrupted = True
        raise KeyboardInterrupt(f"received signal {signum}")

    signal.signal(signal.SIGINT, handle_signal)
    signal.signal(signal.SIGTERM, handle_signal)

    try:
        env = configure_environment(args.real_env)
        if env.mode == "sandbox":
            log(f"Using temporary benchmark sandbox: {env.root}")
        else:
            log("Using real cc-switch/app environment with snapshot restore.")

        paths = resolve_paths()
        if env.mode == "real":
            log("Checkpointing and snapshotting real cc-switch/app state...")
            checkpoint_db(paths)
            snap = snapshot_paths(paths)
            log(f"Snapshot stored at {snap.root}")

        log("Seeding benchmark providers, usage, sessions, MCP servers, and skills...")
        first_sessions = seed_data(paths, args.providers_per_app, args.usage_rows, args.sessions_per_app, args.mcp_rows, args.skill_rows)
        try:
            if not args.skip_cli:
                log("Running CLI benchmarks...")
                benchmark_cli(metrics, binary, paths, args.iterations, args.warmup, first_sessions, args.operation_set, args.fail_fast)
            if not args.skip_tui:
                log("Running TUI benchmarks...")
                benchmark_tui(metrics, binary, paths, args.iterations, args.warmup, args.operation_set, args.fail_fast)
        except BenchmarkAbort as exc:
            log(f"Benchmark stopped after recorded failure: {exc}")
        summaries = metrics.summaries()
        if args.redact_paths:
            replacements = {
                str(binary): "<binary>",
                str(repo_root()): "<repo>",
            }
            if env.root is not None:
                replacements[str(env.root)] = "<sandbox>"
            if snap is not None:
                replacements[str(snap.root)] = "<snapshot>"
            summaries = redact_summaries(summaries, replacements)
        result = {
            "binary": display_path(binary, args.redact_paths),
            "operationSet": args.operation_set,
            "environment": {
                "mode": env.mode,
                "sandboxRoot": display_path(env.root, args.redact_paths),
                "snapshotRestored": False,
                "sandboxCleaned": False,
            },
            "generated": {
                "providersPerApp": args.providers_per_app,
                "usageRows": args.usage_rows,
                "sessionsPerApp": args.sessions_per_app,
                "mcpRows": args.mcp_rows,
                "skillRows": args.skill_rows,
            },
            "iterations": args.iterations,
            "warmup": args.warmup,
            "summaries": summaries,
        }
        return_code = 130 if interrupted else 0
    finally:
        if snap is not None:
            log("Restoring original cc-switch/app state...")
            snap.restore()
            if paths is not None:
                checkpoint_db(paths)
            restored = True
            log("Restore complete.")
        elif args.real_env:
            log("No snapshot was restored.")

        if env is not None and env.mode == "sandbox":
            env.cleanup()
            cleaned = True
            log("Sandbox cleanup complete.")

    if result is not None:
        result["environment"]["snapshotRestored"] = restored
        result["environment"]["sandboxCleaned"] = cleaned
        if args.json_output:
            args.json_output.parent.mkdir(parents=True, exist_ok=True)
            args.json_output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
            log(f"Wrote JSON results to {args.json_output}")
        print_summary(summaries)
    return return_code


if __name__ == "__main__":
    raise SystemExit(main())
