# Session Management TUI Plan

## Scope

Build a local Session Manager for the CLI/TUI build, modeled after upstream
`./.upstream/cc-switch/src-tauri/src/session_manager`, but adapted for ratatui
and this repository's worker-driven TUI runtime.

This implementation follows upstream's runtime scan model. It does not add a
database table, migration, cache schema, or syncable config field.

## Upstream Reference

Keep these upstream ideas:

- Provider adapters return a common `SessionMeta` and `SessionMessage` shape.
- List scanning reads only head/tail chunks for JSONL sessions, deriving title,
  summary, timestamps, cwd, and resume command without reading full transcripts.
- Details are loaded lazily from the selected session source.

The TUI-specific adjustments are:

- `scan_sessions()` runs on a background worker, never inside `UiData::load()` or
  a render path.
- Ratatui renders explicit windows of rows/messages instead of React-style
  virtualization.
- Search/filtering stays as a cheap local metadata filter over scanned rows.

## Data Model

Use the upstream shape as-is:

```rust
pub struct SessionMeta {
    pub provider_id: String,
    pub session_id: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub project_dir: Option<String>,
    pub created_at: Option<i64>,
    pub last_active_at: Option<i64>,
    pub source_path: Option<String>,
    pub resume_command: Option<String>,
}

pub struct SessionMessage {
    pub role: String,
    pub content: String,
    pub ts: Option<i64>,
}
```

## Architecture

Add `src-tauri/src/session_manager` from upstream, including provider modules for
Codex, Claude, OpenCode, OpenClaw, Gemini, and Hermes. The TUI uses a dedicated
`SessionSystem` worker:

```rust
pub enum SessionReq {
    Refresh { request_id: u64 },
    LoadMessages { request_id: u64, key: String, provider_id: String, source_path: String },
}

pub enum SessionMsg {
    ScanFinished { request_id: u64, result: Result<Vec<SessionMeta>, String> },
    MessagesLoaded { request_id: u64, key: String, result: Result<Vec<SessionMessage>, String> },
}
```

On route entry, render immediately, then send `Refresh` if the page has not
loaded and no scan is active. Message loading starts only after the user selects
a session.

## Performance Rules

- Provider scans reuse upstream's parallel provider scan.
- No IO in render, `UiData::load()`, or key handling beyond queuing worker
  messages.
- Full transcript reads happen only for selected detail.
- Large messages render as compact previews; `Enter` opens the existing
  read-only text view for the focused message.
- Stale worker results are dropped by request id and selected session key.

## TUI Layout

Use the existing TUI design language:

- Existing bordered pane style and table highlight style.
- `Tab` switches list/actions/messages panes.
- `Enter` opens the focused session/action/message.
- No new single-letter shortcuts for normal navigation; `r` follows the existing
  refresh pattern.

Left pane:

- Provider, title, relative/absolute time, project basename.
- Global `/` filter searches provider id, session id, title, summary, and
  project dir.

Right pane:

- Metadata header: provider, short id, time, project dir, summary.
- Action rows:
  - `Resume Command        open`
  - `Project Directory     open` or disabled if unknown
- Message preview list below actions.

## Verification

```sh
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml session_manager
cargo test --manifest-path src-tauri/Cargo.toml tui_sessions
```
