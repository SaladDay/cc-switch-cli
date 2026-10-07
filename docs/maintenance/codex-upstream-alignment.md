# Codex provider switching: upstream alignment

The Codex authentication and live-file implementation is ported from
[farion1231/cc-switch at d35726e28695844deaf0098450b34911f5be7b78](https://github.com/farion1231/cc-switch/tree/d35726e28695844deaf0098450b34911f5be7b78/src-tauri/src).

Keep these modules aligned with their upstream paths, including their tests:

- `services/provider/codex_login.rs`: authentication decisions and device-local login stash.
- `live/project/codex.rs`: provider route projection, scoped credentials, reserved-provider cleanup and preservation of user settings. The shared `PROXY_MANAGED` constant is defined locally instead of importing the unrelated Claude projector.
- `live/{engine,floor}.rs`, `live/patch/*`: file planning, guarded writes and format-preserving patches.
- `mode/{operation,state}.rs`: durable pending operations, crash recovery and device-local state.
- `config.rs`: `StagedWrite`, `stage_write` and `commit_staged` are upstream's staging primitives; existing CLI one-file writers remain available to other apps.
- `codex_config.rs`: credential classification, auth-store mode and JWT identity helpers are copied from upstream, with the relevant tests.

`services/provider/codex_live.rs` connects those modules to the CLI's existing
provider transactions. It follows `codex_direct.rs` for login-stash loading,
credential placement, `requires_openai_auth`, retired route tables and guarded
multi-file writes. The CLI has no upstream managed Codex account manager or Stack
mode, so those entry points are not connected. The pure authentication planner
retains their upstream cases and tests.

CLI integration details:

- Provider rows and common snippets still use the CLI's existing SQLite/state APIs. Explicit common-snippet edits are expressed through upstream `TomlPatch`, then the upstream Codex projection applies the provider fields.
- Direct switches defer the current-provider pointer until the durable file operation commits. Interrupted operations recover using the upstream pending journal; the older CLI rollback must not undo a pending Codex operation.
- MCP synchronization remains CLI housekeeping after the durable commit. Codex switching no longer refreshes provider snapshots from live files. Errors are logged and must not roll back the committed switch.
- Switching keeps stored provider templates instead of backfilling live routes or credentials. Config restore also keeps an official provider’s stored auth template. The CLI's separate temporary-home launch/capture feature is unchanged.
- Login stash, pending state and first-write backups live in the device's `~/.cc-switch`, independently of `CC_SWITCH_CONFIG_DIR`, matching upstream. Login-containing files are private and are not added to WebDAV state.

Third-party API keys belong in the selected provider's
`experimental_bearer_token`, not `auth.json`. Disabling preservation temporarily
moves a native login into the local stash; switching back restores it. Only keys
identifiable from third-party provider records count as old third-party residue;
independent OpenAI API-key logins and rotated OAuth tokens must survive.
