# Common Config Backend Upstream Alignment Plan

## Goal

Align the backend common-config implementation with the upstream backend model
as much as this CLI/TUI repository can safely support.

When this repository and upstream disagree, the backend should learn from and,
where suitable, reuse the upstream implementation. The TUI may keep a different
product shape, but it must not force backend common-config semantics away from
the upstream model.

The practical goal is to make these paths agree:

- provider storage snapshots
- live config writes
- switch backfill
- proxy/takeover live backups
- config import/restore live sync
- CLI export
- TUI provider forms and previews

## Upstream Source Of Truth

Use these upstream paths as the primary implementation reference:

- `.upstream/cc-switch/src-tauri/src/services/provider/live.rs`
- `.upstream/cc-switch/src-tauri/src/services/provider/mod.rs`
- `.upstream/cc-switch/src-tauri/src/provider.rs`
- `.upstream/cc-switch/src-tauri/src/services/proxy.rs`

The most important upstream functions to mirror or adapt are:

- `provider_uses_common_config`
- `settings_contain_common_config`
- `apply_common_config_to_settings`
- `remove_common_config_from_settings`
- `build_effective_settings_with_common_config`
- `write_live_with_common_config`
- `strip_common_config_from_live_settings`
- `normalize_provider_common_config_for_storage`
- `migrate_legacy_common_config_usage`
- `json_is_subset`, `json_deep_merge`, `json_deep_remove`
- `toml_item_is_subset`, `merge_toml_table_like`, `remove_toml_table_like`

## Review Resolution

A review found that the previous plan mixed two incompatible goals:

- preserving the current repository's `commonConfigEnabled` default behavior
- claiming Phase 4 upstream alignment

This revision chooses upstream alignment for backend semantics.

The revised plan therefore treats these items as blocking requirements:

- missing `commonConfigEnabled` must follow upstream subset detection, not
  unconditional enablement
- Codex runtime/provider-local TOML tables must be protected beyond
  auto-extraction
- JSON and TOML stripping must support upstream subset and array-subset rules
- config import/restore live sync must use the same effective settings path
- current-provider drift tests must remain in the verification matrix
- TUI form code must not serialize metadata in a way that defeats backend
  upstream semantics
- legacy migration must run during upgrade/startup and import/restore refresh,
  not only when a user edits a common snippet

## Target Semantics

### Common-config enablement

Adopt the upstream rule:

- if `meta.commonConfigEnabled` is `Some(true)`, apply a non-empty snippet
- if `meta.commonConfigEnabled` is `Some(false)`, do not apply it
- if `meta.commonConfigEnabled` is `None`, apply only when the provider
  snapshot already contains the snippet as a subset
- OpenCode and OpenClaw do not receive normal common-config live merges

This differs from the current repository behavior, where missing
`commonConfigEnabled` effectively behaves as enabled for Claude, Codex, and
Gemini.

### Legacy migration

For upstream, legacy migration can rely on subset detection: if a provider
snapshot already contains the common snippet, mark it as explicitly using common
config and strip the common fields.

This repository needs one extra compatibility rule. The current backend applied
common config by default when `commonConfigEnabled` was missing, and it also
normalized provider snapshots by stripping common fields without requiring an
explicit meta flag. That means some existing providers may no longer contain
the snippet subset even though the old backend would still apply common config
for them.

For providers that already exist at the migration boundary:

1. if the app has a non-empty common snippet and the provider is Claude, Codex,
   or Gemini
2. if `commonConfigEnabled` is missing
3. set `commonConfigEnabled = true`
4. normalize the provider snapshot by removing the common fields when present
5. preserve providers that already have `Some(true)` or `Some(false)`

For providers created after the migration boundary, follow the upstream rule:
missing `commonConfigEnabled` means legacy subset detection, not unconditional
enablement.

The migration must be one-time and idempotent. Store an explicit migration
marker or config/schema version so future imported or newly-created providers
with missing metadata are not accidentally treated as old default-enabled
providers.

### JSON merge/remove behavior

Adopt upstream JSON behavior:

- objects are matched recursively by subset
- arrays can contain common-config subset items
- removing common config from arrays removes matching subset items while
  preserving extra items
- scalar values are removed only when they match the common value

This is stricter and more useful than the current whole-array equality behavior.

Gemini follows the upstream env-scope rule:

- a Gemini common snippet is a JSON object of environment variables
- subset detection checks `settings_config.env`
- apply merges the snippet into `settings_config.env`
- remove strips from `settings_config.env`
- common config must not be merged at the whole provider-settings object level

### Codex TOML merge/remove behavior

Adopt upstream TOML behavior:

- tables are matched recursively by subset
- arrays can contain common-config subset items
- removing common config from arrays removes matching subset items while
  preserving extra items
- inline tables use subset matching
- syntax-preserving `toml_edit` handling remains required

Preserve repository-specific safety rules:

- never strip Codex provider identity keys:
  - `model`
  - `model_provider`
  - `model_providers`
- never allow runtime/provider-local Codex tables to become common config:
  - `projects`
  - `trusted_workspaces`

The runtime key protection must apply to validation, auto-extraction, storage
normalization, backfill stripping, live apply, backup generation, and export.
It is not enough to protect only auto-extraction.

For historical snippets already stored with runtime keys, all effective
common-config operations must use a sanitized snippet with denylisted keys
removed. This prevents a historical `projects` table from being merged into
another provider's live `config.toml` while still keeping startup and unrelated
provider operations non-fatal.

### Effective live settings

All backend live write paths should build effective settings through the same
common-config path before writing:

- normal provider switch
- sync current provider to live
- config import/restore live sync
- proxy/takeover backup update
- CLI export when export asks for common config merged output

Backend call sites should not manually reimplement app-specific merge rules.

## Repository-Specific Constraints

This repository differs from upstream in product shape and persistence flow.

Important constraints:

- current state uses both DB and in-memory `MultiAppConfig`
- `state.save()` can overwrite DB current provider if stale in-memory
  `manager.current` is written back
- TUI provider forms currently own part of common-config behavior
- front-end layout and workflows may remain CLI/TUI-specific

Backend alignment must therefore be adapted, not copied blindly:

- direct DB writes are acceptable only when later `state.save()` cannot
  overwrite them
- transaction closures that mutate provider snapshots should also update the
  in-memory config or use preserving-current save paths
- TUI must not serialize `commonConfigEnabled=true` for an unchanged legacy
  provider simply because the checkbox default is true
- direct TUI import paths that bypass `ProviderService::add` still need the
  same storage normalization and migration behavior

## Non-Goals

Do not combine this work with:

- hot-switch current-provider cleanup
- removing switch `refresh_snapshot`
- unrelated provider UI redesign
- broad Codex TOML helper rewrites beyond common-config behavior
- changing OpenCode/OpenClaw common-config support

These can be separate follow-ups.

## Target Module Shape

Create a backend common-config module:

- `src-tauri/src/services/provider/common_config.rs`

Prefer function names close to upstream:

- `provider_uses_common_config(app_type, provider, snippet)`
- `settings_contain_common_config(app_type, settings, snippet)`
- `apply_common_config_to_settings(app_type, settings, snippet)`
- `remove_common_config_from_settings(app_type, settings, snippet, mode)`
- `build_effective_settings_with_common_config(state/db, app_type, provider)`
- `normalize_provider_common_config_for_storage(state/db, app_type, provider)`
- `strip_common_config_from_live_settings(state/db, app_type, provider, live)`
- `migrate_legacy_common_config_usage(state, app_type, legacy_snippet)`

Mode-specific behavior is still useful for repository-specific guardrails:

- `StorageNormalize`
- `BackfillLive`
- `AutoExtract`
- `FormPreview` if the TUI reuses the pure helper

Each mode must make Codex identity-key and runtime-key behavior explicit.

## Implementation Tasks

### Task 1: Add Tests For Upstream Semantics

Add tests before changing behavior.

Required cases:

- explicit `commonConfigEnabled=true` applies a non-empty snippet
- explicit `commonConfigEnabled=false` does not apply even if settings contain
  the snippet
- missing `commonConfigEnabled` applies only when settings contain the snippet
  as a subset
- missing `commonConfigEnabled` does not apply when settings do not contain the
  snippet
- legacy migration marks matching providers as `commonConfigEnabled=true`
  and strips the common snapshot fields
- upgrade/startup migration marks old missing-meta providers as
  `commonConfigEnabled=true` when the app already has a non-empty common
  snippet, even if their snapshots were previously stripped by the old backend
- JSON object-array subset stripping preserves extra array items
- TOML array subset stripping preserves extra array items
- Gemini common snippets are detected, applied, and removed under
  `settings_config.env`, not at the whole settings object
- Codex `model`, `model_provider`, and `model_providers` are preserved
- new Codex common snippets containing `projects` or `trusted_workspaces` are
  rejected with a clear validation error
- existing stored Codex snippets containing `projects` or `trusted_workspaces`
  do not strip or apply those keys during migration; provider snapshots keep
  runtime project trust tables
- historical Codex snippets containing `projects` or `trusted_workspaces` are
  sanitized before live apply, proxy/takeover backup generation, sync-to-live,
  and CLI export
- proxy/takeover backup generation produces the same effective settings as
  normal live writes
- config import/restore live sync uses effective settings
- effective current provider can diverge from in-memory `manager.current`
  without set/clear common snippet rolling current back
- effective current provider can diverge from in-memory `manager.current`
  during switch/backfill; backfill must update the effective current provider
  and later save steps must not overwrite it with stale in-memory state
- provider add/update/import call-site tests prove that missing-meta providers
  follow upstream storage normalization: they are not stripped unless explicitly
  enabled or migrated as old default-enabled providers
- TUI Codex live import, which currently mutates `state.config` directly before
  saving, follows the same storage normalization rules

Expected test files:

- `src-tauri/src/services/provider/tests.rs`
- `src-tauri/tests/provider_service.rs`
- `src-tauri/tests/provider_commands.rs`
- `src-tauri/tests/proxy_takeover.rs`
- `src-tauri/tests/import_export_sync.rs`
- `src-tauri/tests/provider_switch_settings_sync.rs`
- `src-tauri/tests/settings_current_provider.rs`
- TUI provider import/submit tests under `src-tauri/src/cli/tui`

### Task 2: Port Or Adapt Upstream Helper Logic

Port the upstream helper logic where suitable:

- JSON subset detection
- JSON deep merge
- JSON deep remove with array subset removal
- TOML subset detection
- TOML table merge
- TOML table remove with array subset removal
- `provider_uses_common_config`
- `settings_contain_common_config`
- `apply_common_config_to_settings`
- `remove_common_config_from_settings`

Keep repository-specific error types and i18n conventions where needed, but do
not invent a parallel algorithm when the upstream one fits.

Expected files:

- create `src-tauri/src/services/provider/common_config.rs`
- modify `src-tauri/src/services/provider/mod.rs`
- modify `src-tauri/src/services/provider/common.rs`
- modify `src-tauri/src/services/provider/claude.rs`
- modify `src-tauri/src/services/provider/codex.rs`
- modify `src-tauri/src/services/provider/gemini.rs`

### Task 3: Add Codex Common-Config Guardrails

Define an explicit Codex denylist for common-config candidates.

At minimum:

- `projects`
- `trusted_workspaces`

The denylist must be enforced in:

- snippet validation
- auto-extraction
- storage normalization
- switch/live backfill stripping
- live apply / effective settings construction
- proxy/takeover backup construction
- CLI export
- TUI/form preview helper if reused there

Preferred behavior: reject a manually entered snippet containing denylisted
runtime tables with a clear error.

For already-stored historical snippets, do not fail startup or block unrelated
provider operations. During migration, drop denylisted keys from the effective
common-config operation and log a warning. The sanitized snippet must be used
for apply, strip, backup, export, and live sync. The stored snippet can then be
normalized on the next successful common snippet edit.

The choice must be locked by tests.

### Task 4: Replace Backend Call Sites

Route every backend common-config path through the shared module.

Required call sites:

- `ProviderService::set_common_config_snippet`
- `ProviderService::clear_common_config_snippet`
- provider add/update/import storage normalization
- switch backfill stripping
- normal live writes
- `build_effective_live_snapshot`
- proxy/takeover backup construction
- CLI provider export
- `ConfigService::sync_current_providers_to_live`
- `ProviderService::sync_current_to_live`
- TUI Codex live import path
- any config import/restore path that rewrites live app config

`ConfigService::sync_current_providers_to_live` is mandatory. It currently
writes provider snapshots directly; after this phase it should write the same
effective settings as normal provider switch.

`ProviderService::sync_current_to_live` must also route through the same helper
because import/restore and startup recovery can call it after reloading state.

### Task 5: Migrate Legacy Common-Config Usage

Adapt upstream `migrate_legacy_common_config_usage`, with one repository-specific
compatibility layer for providers that were saved under the old
default-enabled behavior.

Trigger it in all paths where existing persisted providers become visible to
the new upstream-style common-config rules:

- application state initialization / upgrade
- DB-to-memory refresh (`refresh_config_from_db`)
- config import or restore after DB refresh
- common snippet set or replace

Implementation options:

- add a DB settings marker such as
  `common_config_upstream_semantics_migrated_v1=true` that runs before normal
  live sync
- or add an idempotent service-level migration called by `AppState::try_new`,
  `refresh_config_from_db`, and config import/restore

The migration must not rely on users editing the common snippet after upgrade.
After the marker is set, imported DB/config data with missing
`commonConfigEnabled` follows the upstream missing-meta subset rule unless that
import path explicitly runs a separate compatibility migration before setting
the marker.

Migration steps:

1. read all providers for the app
2. skip providers that already have explicit `commonConfigEnabled`
3. if the provider predates the migration boundary and the app has a non-empty
   snippet, set `commonConfigEnabled=true`
4. otherwise, use the upstream subset detection rule
5. strip the common fields from stored snapshots when present
6. write an idempotent migration marker or schema version
7. preserve current-provider DB/local settings semantics
8. ensure the migration is not rerun for newly-created or newly-imported
   missing-meta providers after the marker is set

This prevents the upstream default semantics from breaking existing providers
created under the current repository's old default-enable behavior, without
making future missing-meta providers behave like the old default.

### Task 6: Minimal TUI Compatibility Work

This phase is backend-led, but some TUI code must change if it would otherwise
force backend divergence.

Required checks:

- editing an existing provider with missing `commonConfigEnabled` must not
  automatically serialize `commonConfigEnabled=true` unless the user changes
  the common-config control
- TUI preview must match backend effective settings for explicit true/false and
  legacy subset-detection cases
- TUI strip-on-disable must use the same JSON/TOML subset removal semantics or
  delegate to the shared pure helper
- common snippet editor should surface Codex denylist validation errors
- Codex TUI preview must enforce the same denylist behavior whether it calls the
  shared backend helper directly or uses a small TUI wrapper

Expected files if needed:

- `src-tauri/src/cli/tui/form/provider_state.rs`
- `src-tauri/src/cli/tui/form/provider_json.rs`
- `src-tauri/src/cli/tui/app/form_handlers/provider.rs`
- `src-tauri/src/cli/tui/runtime_actions/editor.rs`
- `src-tauri/src/cli/tui/runtime_actions/config.rs`
- `src-tauri/src/cli/tui/ui/forms/provider.rs`

Do not redesign the TUI. The UI can remain a checkbox if the data model also
tracks whether the user explicitly changed it.

### Task 7: Verification Matrix

Run at minimum:

- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`
- `cargo test --manifest-path src-tauri/Cargo.toml services::provider::tests::`
- `cargo test --manifest-path src-tauri/Cargo.toml --test provider_service`
- `cargo test --manifest-path src-tauri/Cargo.toml --test provider_commands`
- `cargo test --manifest-path src-tauri/Cargo.toml --test provider_switch_settings_sync`
- `cargo test --manifest-path src-tauri/Cargo.toml --test settings_current_provider`
- `cargo test --manifest-path src-tauri/Cargo.toml --test proxy_takeover`
- `cargo test --manifest-path src-tauri/Cargo.toml --test import_export_sync`
- `cargo test --manifest-path src-tauri/Cargo.toml tui::form`
- `cargo test --manifest-path src-tauri/Cargo.toml tui::app`

If proxy internals are touched directly:

- `cargo test --manifest-path src-tauri/Cargo.toml services::proxy::tests::`

If MCP-related Codex live writes change:

- `cargo test --manifest-path src-tauri/Cargo.toml --test mcp_commands`

If the migration marker is stored in DB settings or config state touched by
sync/restore:

- run WebDAV download / V1 migration regression tests that reload through
  `AppState::try_new`

## Commit Slicing

Recommended commit order:

1. Add tests that document upstream target semantics and current gaps.
2. Port/adapt upstream JSON/TOML common-config helpers.
3. Adopt upstream `provider_uses_common_config` and legacy migration.
4. Add Codex runtime-key guardrails.
5. Replace backend live write, backfill, backup, export, and config sync call
   sites.
6. Add minimal TUI compatibility changes so forms do not defeat backend
   semantics.
7. Run the full verification matrix and fix regressions.

Do not mix this with unrelated hot-switch or refresh-snapshot cleanup.

## Acceptance Criteria

This phase is complete when:

- backend common-config enablement follows upstream explicit/legacy-subset
  semantics
- upstream JSON and TOML subset/array behavior is ported or faithfully adapted
- provider storage normalization, switch backfill, live writes, proxy backups,
  and config restore live sync share the same effective-settings logic
- Codex runtime provider-local tables are protected from common-config stripping
  and extraction
- existing legacy providers that used common config continue working after
  migration
- TUI forms no longer force missing `commonConfigEnabled` into explicit true
  unless the user makes that choice
- current-provider drift tests pass
- targeted backend, import/export, proxy, and TUI form tests pass

## Recommended Next Step

Start with Task 1 and Task 2.

The first implementation commit should make the upstream target behavior
executable in tests and port the pure helper algorithms. Avoid changing TUI
behavior or transaction/current-provider behavior until the helper behavior is
locked down.
