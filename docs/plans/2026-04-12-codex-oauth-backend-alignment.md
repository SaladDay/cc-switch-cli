# Codex OAuth Backend Alignment Plan

## Goal

Bring the backend-only `codex_oauth` foundation into this repository without
building any new TUI/CLI surface yet.

The requirement is strict:

- reuse the upstream multi-account architecture
- do not ship a simplified single-account substitute
- keep token storage, account binding, default-account fallback, and proxy
  forwarding behavior aligned with the upstream backend design

## Scope

This pass is backend only.

Included:

- Rust auth manager for Codex OAuth multi-account storage and refresh
- provider metadata / account binding backend compatibility
- proxy forwarding integration for `codex_oauth`
- backend quota/subscription path for `codex_oauth`
- backend tests that lock the semantics

Explicitly excluded for this pass:

- TUI forms
- CLI interactive login/account management UX
- frontend account selector panels

## Why This Shape

Issue #90 asks for multiple official-style accounts that do not overwrite each
other. The upstream backend already solves that class of problem through a
managed-account architecture used by `codex_oauth` and `github_copilot`.

We should not invent a parallel model for this repo.

The correct backend path is:

1. multiple persisted accounts
2. one optional default account
3. provider-level binding through `meta.authBinding`
4. request-time token resolution per provider/account
5. `ChatGPT-Account-Id` header injection for Codex OAuth requests

## Required Upstream-Parity Pieces

### 1. Auth manager

Bring over the upstream `CodexOAuthManager` behavior:

- device-code login flow
- refresh-token persistence
- in-memory access-token cache
- per-account refresh lock
- account add/remove/default selection
- disk store format with `version`, `accounts`, `default_account_id`

Canonical source:

- `src-tauri/src/proxy/providers/codex_oauth_auth.rs`

### 2. Provider metadata semantics

Preserve/use:

- `meta.providerType = "codex_oauth"`
- `meta.authBinding = { source: "managed_account", authProvider: "codex_oauth", accountId? }`
- provider-level fallback to default account when `accountId` is absent

Canonical source:

- `src-tauri/src/provider.rs`

### 3. Proxy forwarding integration

At request time, the backend must:

- recognize `codex_oauth` providers
- fetch the correct token from the auth manager
- resolve the provider-bound account id
- inject `ChatGPT-Account-Id`
- use the same auth strategy path as upstream

Canonical sources:

- `src-tauri/src/proxy/providers/mod.rs`
- `src-tauri/src/proxy/providers/auth.rs`
- `src-tauri/src/proxy/providers/claude.rs`
- `src-tauri/src/proxy/forwarder.rs`

### 4. Quota/subscription backend

Mirror the upstream quota lookup path keyed by account id:

- explicit account id if the provider binds one
- otherwise fallback to default account

Canonical sources:

- `src-tauri/src/commands/codex_oauth.rs`
- `src-tauri/src/services/subscription.rs`

### 5. Generic managed-auth command layer

Even though the frontend/TUI is deferred, the backend command/service surface
must exist so later UI work does not require redesigning the backend.

Canonical source:

- `src-tauri/src/commands/auth.rs`

## Repository-Specific Adaptation Rule

This repository currently runs as a CLI application, not the upstream Tauri GUI
shell. That means we cannot blindly wire every UI-facing entrypoint, but we
must still preserve backend semantics.

Rule:

- service/manager/proxy behavior should stay upstream-aligned
- if a UI-facing Tauri command cannot be exercised from current surfaces yet,
  it may exist as backend plumbing without a new CLI flow in this pass
- no alternate simplified storage model is allowed

## Planned File Groups

### New backend modules likely required

- `src-tauri/src/proxy/providers/codex_oauth_auth.rs`
- `src-tauri/src/commands/auth.rs`
- `src-tauri/src/commands/codex_oauth.rs`

### Existing files expected to change

- `src-tauri/src/commands/mod.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/src/proxy/forwarder.rs`
- `src-tauri/src/proxy/providers/mod.rs`
- `src-tauri/src/proxy/providers/auth.rs`
- `src-tauri/src/proxy/providers/claude.rs`
- `src-tauri/src/services/subscription.rs`
- `src-tauri/src/provider.rs` only if backend parity needs small metadata additions

### Tests expected to be added/ported

- manager persistence / multi-account tests
- default account resolution tests
- provider-bound account routing tests
- request header injection tests
- quota lookup tests for explicit-account and default-account paths

## Execution Order

1. Port `CodexOAuthManager` and its persistence/tests.
2. Port generic managed-auth backend commands and `codex_oauth` quota command.
3. Wire runtime state/bootstrap so the proxy forwarder can resolve the manager.
4. Port `codex_oauth` provider-type handling and auth strategy integration.
5. Port request-time account binding and `ChatGPT-Account-Id` injection.
6. Port quota/subscription lookup semantics.
7. Run targeted backend tests, then broader regression checks where touched.

## Non-Goals

- No “temporary local token” fallback
- No “just one global ChatGPT account” shortcut
- No provider-independent token sharing that ignores `authBinding`
- No UI work in this pass

## Acceptance Criteria

- Multiple Codex OAuth accounts can coexist in storage without overwriting each other.
- One default account can be selected and resolved.
- A provider can bind to a specific account id.
- An unbound provider uses the default account.
- Proxy forwarding resolves the correct token per request.
- `ChatGPT-Account-Id` is injected for Codex OAuth requests.
- Backend quota queries can target either a chosen account or the default account.
- Tests cover account persistence, default resolution, provider binding, and forwarding behavior.
