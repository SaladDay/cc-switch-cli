# Changelog

All notable changes to CC Switch CLI will be documented in this file.

**Note:** This is a CLI fork of the original [CC-Switch](https://github.com/farion1231/cc-switch) project, maintained by [saladday](https://github.com/saladday).

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [5.8.3] - 2026-06-17

### Changed

- **Database / WebDAV**: Raise the supported SQLite schema to v11 so CLI installs can open and sync databases created by the newer CC Switch line. Fixes [#281](https://github.com/SaladDay/cc-switch-cli/issues/281).
- **Database / Backups**: Tighten file permissions for the cc-switch database and backup files, including migration and recovery paths.
- **Release Notes**: Keep the generated release body focused on assets and update metadata while GitHub renders the contributor list.

### Fixed

- **Database / Migration**: Add the v10 -> v11 migration for `proxy_request_logs.pricing_model` and the expanded `usage_daily_rollups` key with `request_model` and `pricing_model`.
- **WebDAV / Import**: Accept current-schema v11 sync exports instead of treating them as future databases, while still rejecting schemas newer than this build before restore work starts.
- **Usage Logs**: Preserve the request model and pricing model dimensions when usage rows are rolled up or restored from sync snapshots.

### Commits (since v5.8.2)

- f620fd3b Secure cc-switch database and backup file permissions (#221)
- 663174ff chore(release): simplify release notes body

### Thanks

- Thanks `@Lei-fly` for opening [#281](https://github.com/SaladDay/cc-switch-cli/issues/281) and spelling out the v5.8.2 to schema v11 compatibility failure.
- Thanks `@FeiYehua` for the database permission hardening and the schema v11 compatibility work in [#221](https://github.com/SaladDay/cc-switch-cli/pull/221).
- Thanks `@SaladDay` for the release workflow cleanup and release coordination.
- Thanks to every contributor who has worked on CC Switch CLI, reported issues, reviewed changes, tested releases, or helped users diagnose upgrade problems.

## [5.8.2] - 2026-06-11

### Added

- **Providers / RunAPI**: Add a RunAPI sponsor preset for provider creation in the CLI and TUI, with matching README sponsor details.
- **CI / Benchmarks**: Add a benchmark workflow and a blocking release benchmark gate so release tags fail before publishing if key TUI paths regress.
- **Release Notes**: Enable GitHub-generated release notes so published releases show the Contributors section with avatars.

### Changed

- **TUI / Performance**: Improve startup responsiveness, provider refresh, and route-opening paths, with benchmark coverage for the flows that were tuned.
- **Benchmarks / CI**: Stabilize benchmark provider selection, fail-fast behavior, and threshold reporting for release and CI runs.
- **README / Release Metadata**: Refresh the README version badges for 5.8.2.

### Fixed

- **Codex / Reasoning Cache**: Restore cross-turn reasoning context for `custom_tool_call` and `tool_search_call`, matching the existing `function_call` handling and fixing missing reasoning errors from Kimi/Moonshot and DeepSeek. Fixes [#258](https://github.com/SaladDay/cc-switch-cli/issues/258). [#263](https://github.com/SaladDay/cc-switch-cli/pull/263)
- **Codex / Model Catalog**: Write `model_catalog_json` as the relative file name `cc-switch-model-catalog.json`, matching Codex's own catalog references and keeping configs more portable. Fixes [#260](https://github.com/SaladDay/cc-switch-cli/issues/260). [#265](https://github.com/SaladDay/cc-switch-cli/pull/265)
- **Codex / Sessions**: Scan `archived_sessions/` alongside active Codex sessions so archived sessions appear in the TUI and CLI session browser. Fixes [#260](https://github.com/SaladDay/cc-switch-cli/issues/260). [#265](https://github.com/SaladDay/cc-switch-cli/pull/265)
- **Proxy / Daemon**: Preserve daemon worker runtime status when proxy state is refreshed.
- **TUI / Tests**: Isolate header layout tests so UI assertions do not leak state between cases.

### Commits (since v5.8.1)

- 580af34d ci: include generated release contributors
- c5d89ede fix(codex): generalize cross-turn reasoning cache to all tool call types (#263)
- e84e5053 fix(codex): use relative filename for catalog path and include archived sessions (#265)
- 6650f36d Gate releases with blocking benchmarks
- 926994fa Stabilize TUI benchmark provider selection
- 85f0558d Stabilize benchmark CI fail-fast
- ae2d90c8 Add benchmark CI gate
- 94a882d3 Fix header layout test isolation
- 1ecfad44 Optimize TUI route open benchmarks
- 7f8f0010 Optimize TUI provider refresh path
- 7384434c Improve TUI startup responsiveness
- 3d8e7c23 Update RunAPI sponsor readmes
- 635964ca Add RunAPI sponsor preset
- a56057e5 fix(proxy): preserve daemon worker runtime status

### Thanks

- Thanks `@thedavidweng` for reporting and fixing the Codex reasoning-cache gap, the relative model catalog path, and archived-session discovery in PRs [#263](https://github.com/SaladDay/cc-switch-cli/pull/263) and [#265](https://github.com/SaladDay/cc-switch-cli/pull/265).
- Thanks `@SaladDay` for the TUI responsiveness work, benchmark release gate, RunAPI sponsor preset, daemon proxy-status fix, release notes integration, and release coordination.
- Thanks to everyone who tested the 5.8.x line and helped keep the release path tight.

## [5.8.1] - 2026-06-07

### Added

- **Codex / TUI**: Add a managed Codex OAuth accounts page under Settings, with account listing, details, login actions, and a persistent login toast that requires confirmation before cancellation.
- **Codex / DeepSeek**: Add an upstream-aligned DeepSeek Codex preset for CLI and TUI provider creation, including model catalog entries for `deepseek-v4-flash` and `deepseek-v4-pro`, reasoning metadata, icon metadata, and the expected Codex TOML shape. Fixes [#250](https://github.com/SaladDay/cc-switch-cli/issues/250).
- **Providers / TUI**: Add provider badges and detail labels for proxy requirements, localized as `Needs Proxy` / `No Proxy Support` and `需要代理` / `不支持代理`.
- **Docs / Agents**: Add root `AGENTS.md` instructions for Codex and other coding agents.

### Changed

- **TUI / Hot Refresh**: Route fast refreshes through read-only snapshot state so background app-data, usage/pricing, quota, proxy, skills, and OpenClaw provider reads do not silently write or repair persistent state.
- **Docs / README**: Refresh README positioning around dual TUI/CLI workflows, managed ChatGPT/Codex OAuth accounts, session history, token/cost usage statistics, and OpenAI-compatible proxy routing.
- **Docs / Maintenance**: Prune stale design notes, old release-note drafts, and refactor plans from `docs/`, keeping the published changelog mirrored under `docs/CHANGELOG.md`.
- **README / Release Metadata**: Refresh the README version badges for 5.8.1.

### Fixed

- **Codex / Chat Bridge**: Build Codex Responses-to-Chat bridge URLs from the configured provider base URL instead of forcing `/v1`, fixing DeepSeek proxy requests that should go to `/chat/completions`. Fixes [#242](https://github.com/SaladDay/cc-switch-cli/issues/242).
- **Codex / Auth Docs**: Clarify that 5.8.0 introduced managed-account plumbing, while the 5.8.1 TUI account page is the first user-facing Codex OAuth account-management screen.
- **TUI / Snapshot Reads**: Avoid clearing invalid runtime session state, creating default proxy rows, rewriting default cost multipliers, persisting OpenClaw live-only providers, or clearing invalid current-provider settings during snapshot-only refreshes.

### Commits (since v5.8.0)

- 1e835c3d chore: add Codex agent instructions (#249)
- 51871934 chore(docs): prune stale documentation (#248)
- 6624869f fix(codex): add DeepSeek preset and chat bridge base handling (#247)
- 8b0ec1c1 feat(tui): label provider proxy requirements (#246)
- e0f8748d docs: highlight dual TUI and CLI workflows (#245)
- 2d2cdd98 fix(tui): use readonly snapshots for hot refresh (#244)
- 8c208420 feat(tui): add Codex OAuth account manager (#243)
- 386d9f50 docs(release): clarify Codex auth scope

### Thanks

- Thanks `@Noodle05` for reporting the missing DeepSeek Codex model catalog and model-switching gap.
- Thanks `@WangHaoZhe` for the DeepSeek proxy base URL report and reproduction details.
- Thanks `@hui-shao` for the detailed proxy dashboard token-counter investigation; that issue remains open while the daemon status path gets a fuller fix.
- Thanks `@fcying` for the OpenCode modalities request now tracked for a later release.
- Thanks `@SaladDay` for the Codex OAuth TUI, DeepSeek preset, proxy requirement labels, read-only refresh path, documentation cleanup, and release integration.
- Thanks to everyone who opened issues, tested 5.8.0, reviewed PRs, and helped narrow compatibility gaps during this patch cycle.

## [5.8.0] - 2026-06-06

### Added

- **Usage Statistics / TUI**: Add a dedicated Usage Statistics page in the main TUI, with day/month/custom ranges, overview metrics, a trend chart, cache hit-rate display, model/provider/request-log tabs, and non-blocking loading for slow usage and pricing queries. Fixes [#230](https://github.com/SaladDay/cc-switch-cli/issues/230).
- **Model Pricing / TUI**: Add Model Pricing as a child page under Usage Statistics, with recent usage context, inline editing through Enter, deletion support, and cost backfill when pricing becomes available.
- **Usage Analytics / Database**: Add SQL-backed usage aggregation, daily rollups, model pricing storage, request-log cost recovery, and session usage import paths for Claude, Codex, Gemini, and OpenCode.
- **Codex / Local Routing**: Expose Codex local routing configuration in the TUI, including API format metadata, model mapping, model fetching, add/delete/edit support, and upstream-aligned persistence. [#235](https://github.com/SaladDay/cc-switch-cli/pull/235)
- **Codex / Auth**: Improve managed-account plumbing for Codex provider flows, including safer token handling and live model fetching when a provider already carries a managed-auth binding. Full Codex official multi-account management remains tracked separately.
- **Providers / CLI**: Add provider shortcut commands, provider start dry-run support, live config commands, provider quota commands, additive provider-key prompts, provider add templates, editable provider duplication, and one-off model fetching.
- **App Config / CLI**: Add settings commands, config directory open commands, OpenClaw config commands, and provider configuration flows for OpenClaw, Hermes, and OpenCode.
- **Proxy / Protocols**: Add Codex Chat routing, Gemini native protocol conversion, GitHub Copilot provider support, Copilot model normalization, managed Copilot auth, and Copilot request optimization.
- **Sessions / TUI**: Add session message filters and improve session spatial navigation. [#214](https://github.com/SaladDay/cc-switch-cli/pull/214)
- **TUI / Help**: Add contextual help overlays for provider fields, Usage Query, Codex Local Routing, proxy details, and global help. [#237](https://github.com/SaladDay/cc-switch-cli/pull/237)

### Changed

- **TUI / Performance**: Move app-switch data loading and usage/pricing refreshes onto async worker paths, keeping normal app switching responsive while heavy usage SQL and pricing SQL run in the background.
- **Update / Homebrew**: Improve Homebrew update handling and refresh README installation guidance. [#219](https://github.com/SaladDay/cc-switch-cli/pull/219)
- **Tests / CI**: Split unit and integration test loops, isolate filesystem state during parallel tests, and add CI coverage for the expanded test set. [#232](https://github.com/SaladDay/cc-switch-cli/pull/232)
- **README / Release Metadata**: Refresh the README version badges for 5.8.0.

### Fixed

- **Proxy / Read Tool**: Drop empty `pages` arguments from streamed and non-streamed `Read` tool payloads before forwarding them, covering the argument snapshot delta path as well as complete tool calls. [#217](https://github.com/SaladDay/cc-switch-cli/pull/217)
- **Proxy / OpenAI Compatibility**: Inject `stream_options.include_usage` where needed, canonicalize OpenAI tool payloads, preserve merged system cache control, preserve redacted thinking placeholders, and normalize native Anthropic tool-thinking blocks.
- **Proxy / Codex Compatibility**: Align Codex proxy auth preservation, request header guards, encoding handling, upstream chat SSE fallback, provider identity migration, and Responses-to-Chat bridging.
- **Provider Config Preservation**: Preserve Claude API key fields, Codex settings siblings, and Gemini settings siblings when editing or switching providers.
- **Provider Templates**: Align Codex sponsor template `apiFormat` metadata between CLI seeding and the TUI serializer. [#238](https://github.com/SaladDay/cc-switch-cli/pull/238)
- **Database / Usage Logs**: Repair request-log schema state before creating usage indexes so older databases can upgrade cleanly.
- **TUI / Usage**: Tighten usage overview spacing, keep cache hit-rate and trend visuals readable across small terminals, stabilize custom range loading, and remove stale shortcut text from nested usage pages.

### Commits (since v5.7.0)

- 368edb05 test(tui): stabilize pricing key bar assertions
- 1b7410ba Merge remote-tracking branch 'origin/main' into codex/session-usage-sync
- 47b0ecb2 feat(tui): edit model pricing inline
- 0715282a feat(tui): nest pricing under usage
- 36bd7d8b feat(tui): show non-blocking usage loading state
- 55f7ff02 fix(tui): tighten usage overview vertical spacing
- c386572b fix(tui): compact usage overview layout
- 638d7148 fix(tui): tighten usage overview metric spacing
- b5796593 fix(tui): align usage overview and trend chart
- c502710a fix(tui): add secondary usage overview metrics
- 576ab85d fix(tui): simplify usage overview layout
- 6cefa359 fix(tui): refine usage overview visuals
- 171da2ca fix(tui): add custom usage range
- d31538da fix(tui): let usage trend chart fill panel
- 68491936 fix(tui): refine usage overview metrics
- 71f2b092 fix(tui): use tab for usage metric switching
- 8cf94dee refactor(tui): move usage details into tabbed page
- ec1cbd0a perf(tui): load app switch data asynchronously
- 3135877e feat(tui): add contextual help (#237)
- 16edfca8 fix(cli): align codex sponsor template metadata (#238)
- 4aaa4eb0 feat(tui): add usage and pricing dashboards
- fff5a530 test(tui): cover Claude API key field loading (#228)
- dcc673f3 feat(codex): expose local routing config (#168) (#235)
- 999302f7 feat(cli): add provider shortcut and start dry run (#234)
- d819bf23 feat(tui): add session message filters (#214)
- 42bf9a6c fix(tui): improve sessions spatial navigation
- b71eefd3 Fix all failed test cases and add into CI loop (#232)
- 42054ce4 fix(database): repair request log schema before indexes
- 894d27ee feat(cli): edit openclaw lists by position
- b6042257 feat(cli): fetch models from one-off config
- 3a3db081 feat(cli): fetch codex oauth provider models
- 3d4f0848 fix(cli): allow hermes memory from fresh config
- c3f65b20 feat(cli): add update check mode
- d8571132 feat(cli): add config directory open commands
- 9cfbf13e fix(proxy): enable copilot request optimizer
- 929afd0b fix(proxy): align model suffix and read pages handling
- 81e447b6 fix(proxy): canonicalize openai tool payloads
- 408c26f5 fix(proxy): preserve merged system cache control
- 119e81ef fix(proxy): normalize native anthropic tool thinking
- 50a9d976 fix(proxy): resolve copilot protocol format
- 1a03de4c fix(proxy): preserve redacted thinking placeholder
- 079832ff fix(proxy): map anthropic tool choice for chat
- cf837839 fix(proxy): preserve codex chat tool identity
- 48f0b132 fix(provider): preserve claude api key field
- 547e2783 fix(cli): preserve gemini settings siblings
- 3051ef9c fix(cli): preserve codex settings siblings
- c2752a42 feat(cli): prompt claude reasoning model
- 2daa33b1 fix(proxy): align claude managed-account takeover
- 485745a0 feat(cli): configure codex oauth provider binding
- fd4f9270 feat(cli): add claude hide attribution prompt
- c3a905fb feat(cli): add editable provider duplicate
- ded77bb1 fix(cli): guard current hermes remove-from-config
- 4de46595 feat(cli): add usage query custom variable hints
- 3cc3ec60 feat(cli): prompt additive provider keys
- 72fbdc19 feat(cli): add provider add templates
- d463498e feat(proxy): align gemini native protocol conversion
- c35f2caa feat(cli): configure opencode providers
- ecfcac6c feat(cli): configure hermes providers
- f269e637 feat(cli): configure openclaw providers
- b0b83c9f feat(cli): add usage query configuration
- 30675761 feat(cli): add prompt live import commands
- 9929c91b feat(cli): add provider quota command
- 5f1f09dc feat(cli): align mcp and skills app matrices
- 25f74ad4 feat(cli): add provider live config commands
- 3069c2d0 feat(cli): add settings commands
- deb50136 feat(cli): add openclaw config commands
- 93147fd9 feat(cli): add codex oauth account commands
- 2c62e964 feat(cli): add session management commands
- aed72529 Update README for homebrew and Improve Homebrew update handling (#219)
- 31b8b53a fix(codex): align proxy auth preservation semantics
- 9b5ca795 fix(codex): align proxy request header guards
- 99f34ae1 fix(codex): align proxy encoding handling
- 81b7dd40 fix(codex): handle upstream chat sse fallback
- 0c856589 fix(codex): project model catalog for live config
- a7607d28 fix(codex): bridge responses to chat providers
- 43af39af fix(codex): align provider identity migration
- 7f30e018 fix(proxy): handle Read argument snapshot deltas (#217)

### Thanks

- Thanks `@feiyehua` for improving Homebrew update handling and keeping the release install path easier to maintain.
- Thanks `@qingliu` for adding session message filters in the TUI.
- Thanks `@unive3sal` for the test isolation work, CI loop improvements, and continued review around proxy behavior.
- Thanks `@thedavidweng` for covering Claude API key field loading in TUI tests.
- Thanks `@SaladDay` for the Usage Statistics and Model Pricing pages, Codex local routing work, CLI/provider expansion, proxy compatibility fixes, and release integration.
- Thanks to everyone who opened issues, tested prerelease builds, reviewed the TUI changes, and reported compatibility gaps during this cycle.

## [5.7.0] - 2026-05-28

### Added

- **Sessions / TUI**: Add session management to the TUI so saved sessions can be browsed and managed from the terminal interface. [#206](https://github.com/SaladDay/cc-switch-cli/pull/206)
- **Providers / TUI**: Add provider duplication, including localized success feedback and copy-state handling through form rebuilds. [#202](https://github.com/SaladDay/cc-switch-cli/pull/202)
- **Providers / Presets**: Add Cubence sponsor copy, logo, and provider presets across Claude Code, Codex, Gemini, OpenCode, Hermes, and OpenClaw. [#209](https://github.com/SaladDay/cc-switch-cli/pull/209)
- **Proxy / Codex**: Add managed proxy support for Codex.

### Changed

- **Model Picker / TUI**: Improve model filling and restore overlay state after model picker interactions. [#195](https://github.com/SaladDay/cc-switch-cli/pull/195)
- **Docs / Agents**: Add Claude Code agent documentation. [#203](https://github.com/SaladDay/cc-switch-cli/pull/203)
- **Rust / Maintenance**: Clean up dead code warnings by removing unused helpers, trimming unused fields, and marking currently dormant paths explicitly. [#210](https://github.com/SaladDay/cc-switch-cli/pull/210)
- **README / Release Metadata**: Refresh the README version badges for 5.7.0.

### Fixed

- **Proxy / GPT Compatibility**: Sanitize empty `Read` pages from GPT responses before forwarding them through the proxy. [#199](https://github.com/SaladDay/cc-switch-cli/pull/199)
- **TUI / App Detection**: Avoid version checks during app detection, preventing detection from doing unnecessary release lookups.
- **Windows / Build**: Keep Unix daemon-only proxy status mapping out of Windows builds while Windows proxy remains unsupported.

### Commits (since v5.6.1)

- 50f47bc0 chore(rust): clean up dead code warnings (#210)
- 05685328 feat: add Cubence sponsor preset (#209)
- 18ad9321 feat: improve model filling and overlay restore in model picker (#195)
- 798977e9 feat: duplicate provider configuration (#202)
- ec15936e fix(tui): avoid version checks during app detection
- 1cd7eb55 feat(proxy): add codex managed proxy support
- 459cc03d feat: add session management tui (#206)
- 5e9f7164 fix(proxy): sanitize empty Read pages from GPT responses (#199)
- b18615b5 docs: add agent doc for claude-code (#203)

### Thanks

- Thanks `@Paulkm2006` for improving model filling and overlay restore behavior in the TUI model picker.
- Thanks `@feiyehua` for adding provider duplication and the follow-up localization/state fixes around that flow.
- Thanks `@unive3sal` for the proxy compatibility fix and the Claude Code agent documentation.
- Thanks `@SaladDay` for the session management TUI, Cubence preset integration, Codex managed proxy support, app-detection fix, Rust warning cleanup, and release integration.
- Thanks to Cubence for sponsoring the project.

## [5.6.1] - 2026-05-24

### Fixed

- **Proxy / Upgrade Compatibility**: Clean up trusted legacy managed proxy sessions before daemon startup and avoid rebinding daemon-known worker ports, fixing proxy enable failures after upgrading from the pre-daemon proxy runtime. Fixes [#200](https://github.com/SaladDay/cc-switch-cli/issues/200).

## [5.6.0] - 2026-05-21

### Added

- **Proxy / Daemon**: Add a daemon-managed proxy process for CLI and TUI proxy workflows, including worker startup, status checks, restart handling, and shutdown commands.
- **Proxy / Apps**: Add per-app proxy workers and default listen ports so Claude Code, Codex, and Gemini can run through separate local proxy endpoints.
- **Usage Query / TUI**: Add the TUI Usage Query configuration page with upstream-aligned templates, field visibility, validation behavior, and result display.
- **Prompts / CLI / TUI**: Move prompt management onto the SQLite prompt service, add prompt identity editing, and confirm before importing over existing prompt files.
- **Codex / Config**: Add `CODEX_HOME` support for Codex installations outside the default config directory.
- **TUI / Editing**: Add readline-style text editing and normalize vim-style navigation across forms and overlays.

### Changed

- **Proxy / Database Compatibility**: Keep the database schema at v10 and store CLI-only proxy preferences in the settings KV entry `proxy_preferences_cli_only`, preserving upstream/WebDAV database compatibility.
- **Proxy / Process Safety**: Replace process-name matching with daemon-owned worker tracking and status proof before stopping existing proxy workers.
- **Provider / Common Config**: Align common config snippets, editor reuse, confirmation flow, Codex extraction, and CLI commands across provider workflows.
- **TUI / Providers**: Refine provider actions, provider empty states, failover proxy UX, Skills page layout, footer shortcuts, and space-toggle behavior.
- **README / Release Metadata**: Refresh the README version badges for 5.6.0.

### Fixed

- **WebDAV / Sync**: Avoid upload readback checks that could fail in compatible WebDAV environments.
- **Database / Compatibility**: Improve the future-schema error path so newer databases fail clearly instead of being modified by older binaries.
- **Failover / Tests**: Stabilize failover proxy setup coverage around the updated proxy workflow.

### Commits (since v5.5.0)

- 23b81d47 refactor: add daemon-managed proxy process (#189)
- 0b1304dd (chore) ignore local review skill
- 26360ae3 feat(tui) normalize vim-style navigation across forms and overlays (#188)
- 64cbca79 (docs) update RightCode rebate to 5%
- 14856f68 feat(tui) add vim-style navigation in form handler (#183)
- a1dd240a (tui)add usage query configuration
- 73b7c3c1 fix(webdav): avoid upload readback checks
- d3c240c5 feat: add CODEX_HOME support (#179)
- d160b168 (tui)streamline common config snippets
- 8e311ee4 (cli)align common config commands
- fa96c245 (provider)persist common config notice confirmation
- a5914cdd (provider)align codex common extraction
- ee155e69 (provider)reuse editor for common config snippets
- 65c4dc75 (provider)align common config handling
- 4a292849 (tui)use space for app toggles
- 564558a2 (test)fix failover e2e proxy setup
- 3fa27235 (prompt)confirm before importing existing prompt
- 50fcb8cd (prompt)toggle prompts with space
- 371f4222 (prompt)stabilize prompt list order
- d3810be2 (prompt)unify prompt add edit
- 8afd9075 (prompts)edit prompt identity
- 6ff4f888 (prompts)use sqlite prompt service
- e3ff1689 (tui)align prompt shortcuts
- 253ce370 (tui)align skills page with mcp style
- d36070bf (tui)refine footer shortcuts
- 83307151 Improve failover proxy UX
- 0c6f9a65 Add provider empty state
- f80a0695 Refine provider TUI actions
- 92ab4425 fix(database): improve future schema error
- e7725913 feat(tui): add readline text editing shortcuts

### Thanks

- Thanks to all developers and contributors who worked on this release, reviewed the proxy changes, tested the TUI flows, and kept the CLI fork aligned with upstream compatibility.
- Special thanks to `@unive3sal` for the daemon-managed proxy process, `CODEX_HOME` support, and the careful follow-up work around multi-user proxy safety.
- Thanks `@feiyehua` for the vim-style navigation work across TUI forms and overlays.
- Thanks `@saladday` for Usage Query, prompt management, common config alignment, WebDAV/database fixes, TUI polish, and release integration across this cycle.

## [5.5.0] - 2026-05-10

### Added

- **Provider Failover**: Add failover management across CLI and TUI, including provider controls and proxy status visibility.
- **Prompts / CLI / TUI**: Add prompt create and rename flows so prompt libraries can be managed without manual file edits.
- **Claude / Config**: Respect `CLAUDE_CONFIG_DIR` for Claude Code installations that keep settings outside the default directory.
- **Packaging**: Add Nix flake packaging for reproducible builds and downstream packaging workflows.

### Changed

- **Claude / TUI**: Add a hide-attribution toggle for Claude provider configuration.
- **Startup / Providers**: Import live provider snapshots during startup to keep stored state aligned with on-disk configuration.
- **CI**: Trigger Rust CI for fork pull requests through `pull_request_target`.
- **README / Release Metadata**: Refresh the README version badges for 5.5.0.

### Fixed

- **Proxy / Streaming**: Emit valid tool stream events without usage payloads and strip Anthropic billing headers from OpenAI-compatible prompts.
- **Proxy / Failover**: Fix proxy failover status output.
- **Codex / Auth**: Persist official temporary auth snapshots and keep Codex session history stable across provider switches.
- **Models / Compatibility**: Improve DeepSeek model and reasoning compatibility.
- **Docs**: Fix broken internal documentation links.

### Commits (since v5.4.0)

- ca1a76b fix(proxy): emit valid tool stream events without usage (#146)
- bccd85a fix(codex): keep history stable across provider switches
- 49b7142 feat(proxy): strip Anthropic billing header from OpenAI prompts (#149)
- 8018bba feat(tui): add Claude hide attribution toggle
- 5a809aa Import live providers on startup
- 27a1c12 feat: respect CLAUDE_CONFIG_DIR env var for Claude Code (#152)
- 0f8c638 ci(workflow): trigger CI on fork PRs via pull_request_target
- 54ae40e style(config): fix cargo fmt formatting
- c0f5cb5 feat(tui): add failover controls (#155)
- f2daf4e feat(prompts): add create and rename flows for CLI and TUI (#160)
- 103f341 fix(codex): persist official temp auth snapshots (#159)
- 6aebff3 build: add Nix flake packaging for cc-switch (#156)
- 84495e3 Fix proxy failover status output (#144)
- af3b291 feat(cli): add failover management commands (#165)
- 5c6d373 Fix broken internal documentation links (#167)
- 397741c (fix) improve DeepSeek model and reasoning compatibility

### Thanks

- Thanks `@unive3sal` for the proxy streaming, Anthropic billing-header, and provider failover contributions across this cycle.
- Thanks `@LuJiansen` for adding `CLAUDE_CONFIG_DIR` support for Claude Code.
- Thanks `@brushax` for the prompt create and rename flows across CLI and TUI.
- Thanks `@haoxianhan` for adding Nix flake packaging.
- Thanks `@TMYTiMidlY` for fixing Codex official login and temporary auth persistence.
- Thanks `@apple-ouyang` for the proxy failover status output fix.
- Thanks `@aqilaziz` for cleaning up broken internal documentation links.
- Thanks `@saladday` for Codex history stability, live provider import, Claude attribution controls, CI updates, DeepSeek compatibility, and release integration across this minor release.

## [5.4.0] - 2026-04-29

### Added

- **Provider / Quota**: Add official provider quota checks so supported first-party accounts can surface quota status directly from cc-switch.
- **MCP / TUI**: Add `stdio` / `http` / `sse` transport selection to the TUI MCP form, including URL-based remote MCP creation and editing.

### Changed

- **MCP / TUI**: Align the MCP add/edit form with the upstream remote-MCP fields while keeping the existing table, keybar, and overlay picker design language.
- **README / Release Metadata**: Refresh the README version badges for 5.4.0.

### Fixed

- **Provider / Common Config**: Preserve Codex runtime trust tables and additive saved-only edits when switching providers, so user-local runtime state is not overwritten by common config extraction.
- **OpenCode / TUI**: Keep OpenCode provider config state aligned when editing provider forms.
- **Proxy / Streaming**: End transformed proxy streams on terminal events so OpenAI-compatible proxy calls do not stall after tool/function-call completion.
- **MCP / Codex Sync**: Write remote MCP headers to Codex as `http_headers` without duplicating the legacy `headers` table.

### Commits (since v5.3.4)

- 8eeb191 fix(tui): support remote MCP server form
- b58caeb fix(proxy): end transformed streams on terminal events
- 8ab7916 feat: add official provider quota checks
- 23f1fd0 test(config): align common config CLI semantics (#123)
- 11df8b6 fix(provider): preserve additive saved-only edits
- 5fcc903 fix(tui): align OpenCode provider config state
- 0f510eb fix(provider): unify common config live handling (#123)
- 83657d7 fix(provider): preserve Codex runtime trust on switch

### Thanks

- Thanks `@iCoresen` for reporting that the TUI MCP form could not add remote URL-based MCP servers.
- Thanks `@unive3sal` for the OpenAI proxy function-call stall report and follow-up validation.
- Thanks `@mushengLenzer` for the Codex `config.toml` trust/common-config report that drove the provider preservation fixes.
- Thanks `@yudar1024` for the OpenCode provider state report that helped tighten provider form handling.
- Thanks `@saladday` for the quota checks, MCP TUI implementation, proxy streaming fix, provider preservation work, and release integration across this cycle.

## [5.3.4] - 2026-04-22

### Changed

- **README / Release Metadata**: Refresh the README version badge for 5.3.4, remove the README patch-highlight section entirely, and link the About section directly to the changelog for release details.

### Fixed

- **WebDAV / Restore**: Keep restore flows from clobbering device-local proxy runtime state and sync active prompt files back into live app directories after restore/download paths.
- **Database / Sync**: Accept upstream schema v10 databases through staged `v8 -> v9 -> v10` migrations, keep WebDAV `db-v6` compatibility intact, refresh the upstream pricing catalog, and create a pre-migration backup before startup upgrades.
- **MCP / Database**: Preserve upstream Hermes MCP enablement when synced databases are loaded and saved locally, so round-tripping MCP entries no longer clears the v10 compatibility flag.
- **Temp Launch / CLI**: Fix temporary-launch model setting switches, align Codex custom defaults and validation, and lower the shell completion activation barrier for the updated command flow.

### Commits (since v5.3.3)

- 8a2a850 Preserve temporary provider launches without leaking native CLI control
- 94c2b13 Trim patch-note links from the README release summaries
- e38a32f Keep WebDAV restore from clobbering local proxy state
- 3284b02 Keep CI green after the WebDAV sync fix
- 5e98f4f （feat）Lower the shell completion activation barrier for issue #113
- 4c2cfed （test）Seed provider state into the test database
- 390cfd3 (fix)sync active prompt files after restore flows (#115)
- 88fa583 (fix)align codex custom defaults and validation
- 7476f2a Fix model settings not switching during temp launch (#117)
- 3bc6c39 (fix)align db v10 migration with upstream sync

### Thanks

- Thanks `@yuanzidev` for the temp-launch follow-up that fixed provider model settings during runtime switching.
- Thanks `@farion1231` for the upstream schema/WebDAV direction that this sync-compatibility release aligns with.
- Thanks `@saladday` for the WebDAV restore hardening, schema v10 migration work, and release integration across this patch cycle.
- Thanks `@aldev814`, `@Hatiaa`, and `@hitsmaxft` for the earlier migration reports that continued to inform the database compatibility follow-up.

## [5.3.3] - 2026-04-13

### Fixed

- **Update / Startup Gate**: Allow `cc-switch update` to bypass startup database initialization so older installed binaries can still self-update even when the local database schema is newer than they support.

### Commits (since v5.3.2)

- 43a952b (update) allow self-update to bypass startup database gates

### Thanks

- Thanks `@saladday` for closing the self-update blocker and pushing the startup-gate fix across the finish line.
- Thanks `@huanghuoguoguo` and `@KKfhfv` for the recent provider and CLI iteration that helped surface and pressure-test this patch cycle.

## [5.3.2] - 2026-04-13

### Added

- **Providers / CLI**: Add `provider export` so Claude provider settings can be exported to a standalone `settings.local.json` path with either the default project-local destination or an explicit output override.
- **Sponsors / Presets**: Expose DDS as a sponsor entry and one-click preset in the provider template flow.

### Changed

- **Rust / CI**: Pin the Rust toolchain to 1.91.1, align release builds to the same compiler baseline, and add a dedicated Rust CI workflow that enforces `cargo fmt --check`.
- **README**: Carry the current patch highlights forward to 5.3.2 by updating the README version badge and `What's New` title without rewriting the existing section body.

### Fixed

- **Codex OAuth / Backend**: Keep managed multi-account Codex OAuth token resolution, provider-bound account routing, and quota lookup aligned in the backend request path.
- **Provider / CLI**: Remove the temporary `provider usage` query command path so the CLI surface stays clean and the Rust fmt CI gate no longer fails on that code path.

### Commits (since v5.3.1)

- c0b370b (auth) preserve Codex OAuth account isolation across provider requests
- b2b0ebc (build) pin Rust formatting and compiler checks to a reproducible baseline
- bad1d44 Feat/issue 97 provider export (#110)
- 7f2811c Expose DDS as a sponsor entry and one-click preset
- 4a1141d (ci) restore Rust fmt checks for provider CLI commands

### Thanks

- Thanks `@huanghuoguoguo` for shipping the provider export workflow in this patch cycle.
- Thanks `@KKfhfv` for the fast CLI/provider follow-up work and iteration around the usage-query path.
- Thanks `@saladday` for the Codex OAuth backend alignment, release hardening, and DDS preset work that rounded out this patch release.

## [5.3.1] - 2026-04-12

### Changed

- **README / Release Notes**: Refresh the README patch highlights for 5.3.1 and add dedicated patch release notes for this release.

### Fixed

- **Codex / Provider Editing**: Preserve official Codex auth snapshots during provider edits and avoid recreating official providers as third-party endpoint configs.
- **Database / Import**: Accept schema v8 databases, add the v6 -> v7 -> v8 migration path, and keep corrected pricing data aligned between migrated and newly created databases.
- **Proxy / Provider Switching**: Update the active proxy target immediately while takeover is running so provider switches no longer require a manual proxy restart to take effect.

### Commits (since v5.3.0)

- c0a01ec fix(proxy): switch running proxy targets without restart (#95)
- b18e8b7 fix(database): support schema v8 imports and staged migrations (#106)
- 81cd431 fix(codex): preserve official auth snapshots during provider edits (#102)

### Thanks

- Thanks `@saladday` for pushing the backend/database compatibility work across this patch cycle.
- Thanks `@aldev814`, `@Hatiaa`, and `@hitsmaxft` for the issue reports that surfaced the migration, proxy switching, and Codex official-provider regressions early.

## [5.3.0] - 2026-04-03

### Added

- **Claude / Codex / CLI**: Add temporary start commands so you can launch Claude or Codex with a selected provider without switching the global current provider.
- **TUI / MCP**: Add overlay-based MCP env editing with dedicated env rows, cleaner summaries, and duplicate-key validation.
- **OpenClaw / TUI**: Add OpenClaw config-dir override support in the TUI.
- **Sponsors / Presets**: Add AICodeMirror sponsor presets and refresh supporting README copy.

### Changed

- **README / Release Notes**: Refresh the README release highlights for 5.3.0 around multi-window temporary launch, MCP env editing, and runtime polish.
- **Runtime / TUI**: Align temporary-launch affordances with real platform support so Unix-only features are routed and advertised consistently.
- **TUI / Forms**: Tighten dirty-form exit confirms and MCP env interaction flows for a more predictable editing experience.

### Fixed

- **Claude / TUI**: Fix temporary launch routing so Claude provider launches dispatch through the Claude runtime handler instead of silently no-oping.
- **MCP / TUI**: Reject duplicate env keys after trimming input and stabilize picker/editing behavior across the env editor flow.
- **Proxy / Runtime**: Publish the managed proxy session after takeover setup and continue aligning runtime behavior with upstream expectations.

### Commits (since v5.2.1)

- 74da3b6 Keep temporary-launch UX aligned with actual platform support
- 71c2757 docs: refine README sponsor copy
- 51077a8 feat: add AICodeMirror sponsor presets
- 7425652 feat(cli): add codex temporary start command
- 86e6c30 feat(tui): add codex temporary launch handoff
- d924dc5 Merge branch 'feat/upstream-runtime-align'
- cdaf60f fix: align upstream runtime behavior
- f750c9b feat(cli): add temporary claude start command
- aa48594 Merge branch 'feature/provider-mcp-unsaved-exit-confirm'
- d9cda98 feat(tui): unify dirty form exit confirms
- 87fa16f fix(tui): tighten Claude temp launch handoff
- f91bdd7 fix(tui): preflight Claude temp launch platform support
- 1d6cc61 fix(tui): secure Claude temp launch settings
- 8314338 fix(tui): guard and recover Claude temp launch
- 377d80e feat(tui): launch Claude with a temporary provider
- 417849f test(tui): cover provider detail launch temp key hint
- 96d31e5 feat(tui): show Claude temporary launch hint
- 07f3b80 test(tui): add provider detail non-claude o-key noop coverage
- bdfa11e feat(tui): add Claude temporary launch action
- 5066c85 docs: add Claude temporary launch design
- d37238e docs: add unsaved form exit confirm spec
- 446da43 feat(openclaw): support config dir override in tui
- 35d8838 test(mcp): lock env import and sync regressions
- b56dcba fix(tui): reject MCP env duplicates with trimmed key comparison
- 541939b fix(tui): tighten MCP env editor esc and add-flow tests
- d6bc552 fix(tui): keep MCP env picker selection stable
- 706a81a feat(tui): add MCP env overlay editor
- 5c0a15e fix(tui): align MCP env hint copy and split env form tests
- fb0853d fix(tui): wire MCP env field summary into form UI
- 2e807c3 feat(tui): add MCP env rows to form state
- caf62e1 docs: add MCP env list editor design
- 12fc7d1 merge: fix managed proxy readiness timeout
- e9c1aee fix(proxy): publish managed session after takeover setup
- 7f26487 docs: add Trendshift badge to README

### Thanks

- Thanks `@saladday` for leading the 5.3.0 cycle across temporary launch, MCP env editing, TUI polish, sponsor integrations, and release/docs updates.
- Thanks `@XyzenSun`, `@1-bytes`, and `@opposj` for the recent fixes and improvements that helped stabilize the current release line.

## [5.2.1] - 2026-03-24

### Changed

- **OpenClaw / TUI**: Refine readability across the OpenClaw config routes, including clearer file-status copy, cleaner config-page hierarchy, and a more readable agents flow.
- **README**: Carry the 5.2.x release highlights forward to 5.2.1 without rewriting the existing What's New section.
- **Docs**: Capture the shared header-summary design principles so future OpenClaw and OpenCode UI changes can reuse the same reasoning.

### Fixed

- **Header / OpenClaw / OpenCode**: Stop rendering unsupported proxy capability as `Proxy: Off`, keep the rightmost status badge focused on each app's real summary field, and make the OpenClaw default-model state easier to read.

### Commits (since v5.2.0)

- 17c32b3 docs: capture header summary design principles
- 48b9186 fix(openclaw): clarify header status badges
- 722c842 fix(openclaw): improve config route readability
- 3941cfc docs: update openclaw agents readability guidance
- ba65a76 fix(openclaw): improve agents config readability
- 71f4951 fix(openclaw): align Exists/Missing status labels in workspace files list

### Thanks

- Thanks `@saladday` for the OpenClaw readability pass across config routes, header summaries, and release docs.
- Thanks to everyone who sent feedback on the OpenClaw config flow and header wording.

## [5.2.0] - 2026-03-23

### Added

- **OpenClaw / Workspace**: Add OpenClaw workspace management across the TUI and CLI, including file actions, daily memory flows, and dedicated config routes for `Workspace`, `Env`, `Tools`, and `Agents` work.
- **Install Script**: Add version selection support so the installer can target a specific release.

### Changed

- **OpenClaw / TUI**: Expand OpenClaw support from basic provider switching into a fuller workspace and config workflow.
- **Docs**: Refresh the README release highlights for 5.2.0 and add an OpenClaw config interaction guideline for follow-up work.
- **Install Script**: Auto-prefix bare version strings with `v` during install selection.

### Fixed

- **TUI**: Remove duplicate meta key handling that could break provider edit saves.
- **Provider**: Clean up stale meta config and fix duplicate common-config field serialization.

### Commits (since v5.1.1)

- 6f349dd Merge branch 'feat/openclaw-workspace-management'
- 998287a docs: add OpenClaw TUI config interaction guidelines
- 3df5baf feat(openclaw): complete workspace management and config flows
- 927411b fix(install): add trailing newline and auto-prefix version with v
- 478f53f feat: add version select support for install script (#72)
- 9a4a475 fix(tui): remove duplicate meta key causing provider edit save failure (#71) (#74)
- a2227df feat:support-version-selection-in-install.sh
- afe1219 Merge pull request #70 from 1-bytes/fix/common-config-alias-cleanup
- 9e73b72 Fix common config alias duplication in provider meta
- a65031d Merge pull request #69 from opposj/main
- c1fe307 fix(provider): remove staled meta config
- 386ff92 docs(readme): revise v5.1.1 highlights
- 7079e29 docs(readme): add OpenClaw emoji to v5.1.1 notes

### Thanks

- Thanks `@saladday` for shipping the OpenClaw workspace and config release, tightening the install flow, and keeping the docs current.
- Thanks `@XyzenSun` for the version-select installer work in PR #72.
- Thanks `@1-bytes` for fixing duplicate common-config field serialization in PR #70.
- Thanks `@opposj` for removing stale provider meta config in PR #69.

## [5.1.1] - 2026-03-20

### Added

- **TUI Settings**: Add a visible apps picker so you can choose which apps appear in the interactive interface, and persist those choices across restarts.

### Changed

- **TUI Navigation**: Apply visible app settings consistently during startup, in the header tabs, and while switching or cycling between apps.

### Fixed

- **Terminal / Header**: Rebalance header tab spacing and keep truecolor rendering in `xterm-256color` terminals, including Termius-style sessions.
- **OpenClaw**: Align provider removal behavior more closely with upstream handling.

### Commits (since v5.1.0)

- b1311cd Merge branch 'feat/tui-app-visibility-settings'
- 5f3beab feat(tui): apply visible app settings across header and switching
- 8db0dff fix(openclaw): align provider removal with upstream
- 16e8a41 feat(tui): add visible apps picker in settings
- 565aaa7 feat(tui): honor visible apps in startup and key cycling
- 6bf3912 feat(tui): persist visible app settings
- 7d0c51f Merge branch 'fix-termius-truecolor-detection'
- 0b0f14f fix(tui): keep truecolor for xterm-256color terminals
- acc8066 fix(tui): rebalance header tab layout

### Thanks

- Thanks `@saladday` for tightening the visible-app flow across the TUI, polishing terminal rendering, and keeping OpenClaw behavior aligned with upstream.

## [5.1.0] - 2026-03-20

### Added

- **OpenClaw**: Add first-class OpenClaw support with upstream-aligned provider flows, additive `~/.openclaw/openclaw.json` sync, default model management, prompt support, and dedicated TUI entries for `Env`, `Tools`, `Agents Defaults`, and config health warnings.

### Changed

- **Provider / Config**: Align OpenClaw provider boundaries, config import/update/remove behavior, and common snippet live-sync handling more closely with upstream behavior.
- **Proxy**: Align Claude backend behavior with upstream and split oversized proxy modules for easier maintenance without changing the release surface.

### Fixed

- **OpenClaw / TUI**: Close remaining live-config parity gaps, improve the ratatui flow, and restore provider add/save after JSON edits.
- **Terminal Compatibility**: Improve ansi256 and `TERM=*-256color` fallback behavior, including SSH xterm sessions.
- **WebDAV**: Reject false-positive sync success results instead of reporting a misleading successful sync.

### Docs & Chore

- **Docs / Repo Hygiene**: Remove branch-only OpenClaw planning notes from the PR payload and ignore the superpowers docs workspace.
- **Internal Refactor**: Split oversized provider, i18n, and Claude proxy modules without intended behavior changes.

### Commits (since v5.0.1)

- 1aca886 feat(openclaw): add upstream-aligned provider support
- dccb70d docs: remove openclaw plan notes from pr
- 5f5c0c9 refactor: split oversized modules without behavior changes
- b6d6564 fix(tui): honor TERM 256color fallbacks
- b42e569 test(tui): isolate test terminals from real tty
- da9298e fix(tui): fallback SSH xterm sessions to ansi256
- 126d082 chore(git): ignore docs superpowers workspace
- 3088a6b fix(tui): unblock provider add save after JSON edits
- c4409be fix(provider): align common snippets with live sync
- 56f8af7 fix(proxy): align Claude backend behavior with upstream
- 4a8dd7c refactor(proxy): split Claude proxy modules and tests
- 078deaf feat(openclaw): align config and provider parity flows
- 969dcd0 feat(openclaw): align upstream provider boundaries
- c4cce7c fix(webdav): reject false-positive sync success
- cdb6c5a fix(openclaw): align live-config parity and TUI behavior

### Thanks

- Thanks `@saladday` for landing OpenClaw support, tightening provider/live-config parity, and cleaning up the upstream alignment work across the proxy and TUI layers.

## [5.0.1] - 2026-03-15

### Added

- **Claude / Codex (TUI)**: Add first-switch safety prompts that detect existing live configs, offer import-before-overwrite, and show a one-time common-config tip after the first real provider switch.

### Fixed

- **Update**: Align self-update target resolution with upstream release metadata, harden signed self-update and installer upgrades, and avoid noisy archive extraction locale issues in the install script.
- **TUI**: Preserve the v5 palette across terminal capabilities, including ansi256 fallback paths such as Apple Terminal compatibility mode.
- **Codex**: Allow importing the current live config when `config.toml` exists without `auth.json`, while still avoiding first-use prompts for auth-only states.

### Docs

- **README**: Refresh the version badges and add a short 5.0.1 highlights section in both English and Chinese docs.

### Commits (since v5.0.0)

- 3ee87c4 fix: align update flow with upstream release resolution
- a0ebe7f fix(tui): preserve v5 colors across terminal capabilities
- 060cd7a fix(update): harden signed self-update and installer upgrades
- d3ff0a7 feat(tui): add Claude provider switch safety prompts
- 5a8d227 feat(tui): add Codex provider switch safety prompts

### Thanks

- Thanks `@saladday` for the 5.0.1 self-update hardening, terminal color compatibility work, and Claude/Codex switch safety UX.

## [5.0.0] - 2026-03-13

### Added

- **OpenCode**: Add first-class provider support plus TUI MCP and skills flows.
- **Proxy (TUI)**: Add multi-app proxy management, token telemetry on the dashboard, and restore the local proxy settings flow.
- **TUI**: Add an external editor shortcut for prompt, provider, and MCP editing.
- **Claude**: Align API format support with upstream.

### Changed

- **TUI**: Remove the legacy TUI and consolidate the interactive experience around the ratatui flow, including shared visual tokens, overlay rules, footer blocks, and connection card spacing.
- **Skills / MCP**: Refine dialogs and align skill import and scan behavior with upstream.
- **Stream Check**: Split the backend module for cleaner maintenance.

### Fixed

- **TUI**: Unify MCP and skill import UX, stop showing the restart notice after provider switch, and polish the Claude API format picker.
- **i18n**: Localize the remaining Chinese TUI copy.
- **Claude / Sync**: Align official provider behavior, db-v6 schema, and WebDAV format with upstream.

### Docs & Chore

- **Repo Hygiene**: Ignore local agent artifacts, worktrees, and workspace docs.
- **Docs / Tests**: Refresh agent guidance, move homepage gifs, and align proxy waveform assertions with baseline rendering.

### Commits (since v4.8.0)

- cb7e878 refactor(tui): unify visual style tokens and overlay rules
- b8a2d3a feat(tui): add external editor shortcut
- 64637f2 refactor(stream-check): split backend module
- 64da960 feat(provider): add OpenCode support
- 65d63c9 feat(opencode): add MCP and skills support to TUI
- e6ab625 feat(tui): refine skill and mcp dialogs
- a107593 fix(tui): unify mcp and skill import ux
- 7eabc5e feat(skill): align import and scan with upstream
- 1d6c54b chore: ignore local agent artifacts
- a20adbb chore: ignore worktrees and lock Chinese TUI copy
- 8c39a3d feat(proxy): add multi-app proxy management to TUI
- 1613650 fix(i18n): localize remaining Chinese TUI copy
- e925eec feat(proxy): add token telemetry to the TUI dashboard
- 2817ba1 refactor(cli): remove legacy TUI and align CLI with interactive flows
- 338e621 test(proxy): align waveform assertions with baseline rendering
- a7b593c fix(claude): align official provider behavior with upstream
- 0438577 fix(sync): align db-v6 schema and WebDAV format with upstream
- 9b6ea26 fix(tui): stop showing restart notice after provider switch
- e2c1cc8 feat(claude): align api format support with upstream
- 55792bb feat(tui): redesign footer with NAV/ACT color blocks and add provider Online indicator
- 0b9c077 style(tui): add top padding before connection info card
- 54baf70 fix(tui): polish claude api format picker ux
- 43e634f feat(tui): restore local proxy settings flow and proxy-required switch notice
- c8c86a5 docs: refresh agent guidance and move homepage gifs
- 4235820 chore: stop tracking local workspace docs

### Thanks

- Thanks `@saladday` for driving the v5.0.0 TUI consolidation, multi-app proxy workflow, and OpenCode support.

## [4.8.0] - 2026-02-28

### Added

- **Install Script**: One-liner `curl | sh` installation for macOS and Linux. (#41)
- **TUI**: Claude VSCode plugin takeover toggle and sync. (#47)
- **TUI**: Model auto-fetch picker with search filtering. (#45)

### Changed

- **Docs**: Revamp installation section with quick install as primary method, manual install in collapsible details.
- **Docs**: Update code structure to reflect current architecture (tui, database, ui directory).
- **Docs**: Fix `skills search` → `skills discover` command name.

### Commits (since v4.7.4)

- bedd43e docs: fix skills search -> discover in README
- a5ce614 feat: add install.sh for one-liner curl | sh installation (#41)
- 3249c29 merge: issue #47 Claude VSCode plugin takeover
- 83212f0 feat(tui): add Claude VSCode plugin takeover toggle and sync
- 20e81a7 feat(tui): add model auto-fetch picker with search filtering (#45)

## [4.7.4] - 2026-02-28

### Added

- **WebDAV Sync**: Add automatic V1 -> V2 migration with legacy artifact cleanup.

### Changed

- **WebDAV Sync**: Align sync protocol implementation with upstream v2.
- **Codex**: Migrate legacy config at startup instead of runtime compatibility fallback.
- **Sponsors**: Update RightCode offer to 25% bonus pay-as-you-go credits and update affiliate link.
- **Docs**: Refresh WebDAV compatibility notes and remove duplicated sponsor descriptions.

### Fixed

- **WebDAV Sync**: Sync live config files after WebDAV download, backup restore, and config import.
- **Codex**: Align provider keys with upstream `model_provider` + `model_providers` format.
- **MCP**: Skip MCP sync when target app is not installed. (#43)
- **Deeplink/Codex**: Fix missing `requires_openai_auth`, strip loop bug, and deduplicate `clean_codex_provider_key`.
- **WebDAV Sync**: Correct `webdav_status_error` parameter order in `delete_collection`.

### Commits (since v4.7.3)

- 16b556f fix: update RightCode affiliate link to ccswitch-cli
- b883a28 feat: update RightCode promotion to 25% bonus credits, remove promo code
- 67f8262 fix: sync live config files after WebDAV download, backup restore, and config import
- faab17f fix: align Codex config with upstream model_provider + model_providers format
- 29b9bb3 fix: skip MCP sync when target app is not installed (#43)
- 5ac3fa5 refactor: align WebDAV sync with upstream v2 protocol
- 6a8ca50 feat: add V1 → V2 WebDAV sync migration with automatic detection and cleanup
- df0fb0e Update README.md
- 5d48583 Update README with WebDAV sync compatibility
- 2c1cfe5 refactor: migrate legacy Codex config at startup instead of runtime compat
- a5d1de9 fix: deeplink missing requires_openai_auth, strip loop bug, dedup clean_codex_provider_key
- 53d5139 fix: correct webdav_status_error param order in delete_collection
- 81eec41 docs: remove duplicate PackyCode README descriptions

## [4.7.3] - 2026-02-24

### Fixed

- **TUI**: Normalize Ctrl+H to Backspace for SSH terminal compatibility (e.g. Xshell sends `\x08` instead of `\x7F`). Fixes #38.

### Changed

- **Providers (TUI)**: Promote RightCode from built-in template to sponsor preset with partner meta, now available for all app types (Claude/Codex/Gemini) with `* ` prefix.
- **Docs**: Add RightCode sponsor section to README (EN/ZH).

## [4.7.2] - 2026-02-08

### Added

- **Settings (TUI)**: Add self-update check/download/apply flow in Settings.

### Fixed

- **Settings (TUI)**: Make update-check cancelable; use ←/→ to switch Update/Cancel; improve update overlay layout and key hints.

## [4.7.1] - 2026-02-07

### Added

- **Providers (TUI)**: Add RightCode provider template.

### Changed

- **TUI**: Tighten layout spacing and reduce list left padding.
- **Codex (TUI)**: Split auth/config previews for clearer navigation.
- **Docs**: Update home screenshots (EN/ZH).

### Fixed

- **Codex**: Align provider config and auth handling with upstream.
- **TUI**: Add left padding between nav border and icons.

## [4.7.0] - 2026-02-07

### Added

- **WebDAV Sync (TUI)**: Add WebDAV sync workflow with TUI controls and home status.
- **CLI**: Add hardened self-update command.
- **Settings**: Add Claude Code onboarding skip toggle.
- **Providers (TUI)**: Add Claude model popup config in provider form.

### Changed

- **Providers (TUI)**: Improve provider JSON editor and modal UX.

### Fixed

- **TUI**: Fix general config editor scroll offset; align editor cursor under soft-wrap; use ASCII PackyCode chip label.
- **Codex**: Stabilize OpenAI auth switching; use TOML snippets and avoid `auth.json`.
- **CLI**: Remove unused global `--json` flag.
- **i18n**: Codex snippet prompt uses TOML.

### Commits (since v4.6.2)

- ec78dad fix(tui): use ASCII PackyCode chip label
- f6ba975 feat(provider): support notes field across cli and tui
- 2f1e516 feat(tui): add Claude model popup config in provider form
- be05cbf feat(tui): improve provider JSON editor and modal UX
- 01897e3 feat(settings): add Claude Code onboarding skip toggle
- 71374dd Merge pull request #25 from SaladDay/sync-upstream-skip-claude-onboarding
- 64dbff8 fix(cli): remove unused global --json flag
- 672299c fix(codex): stabilize OpenAI auth switching
- ada90fc fix(tui): align editor cursor under soft-wrap
- b69ced1 feat(cli): add hardened self-update command
- 108e291 feat(webdav): add sync workflow, tui controls, and home status
- 0624d01 Merge pull request #26 from SaladDay/bugfix-general-config-editor-scroll-offset
- 50fbe19 fix(codex): use TOML snippets and avoid auth.json
- 590c461 fix(i18n): codex snippet prompt uses TOML
- a9d63b5 Merge pull request #27 from skyswordw/codex/add-self-update-command-clean-upstream
- b3cd0ef Merge origin/main into feat/webdav-sync-v1
- 1114997 Merge pull request #28 from SaladDay/feat/webdav-sync-v1

## [4.6.2] - 2026-02-05

### Changed

- **Providers (TUI/CLI)**: Hide the `Notes` field from add/edit flows; optional fields now only include sort index.
- **TUI**: Shorten the Website URL label (remove "(optional)") to avoid layout truncation.

## [4.6.1] - 2026-02-05

### Added

- **Providers (TUI)**: Add sponsor provider presets (PackyCode) for Claude/Codex/Gemini in the "Add Provider" form.
- **Docs**: Add PackyCode sponsor section to README (EN/ZH), including website, registration link, and promo code `cc-switch-cli` (10% off).

## [4.6.0] - 2026-02-05

### Added

- **Interactive (TUI)**: Home now includes a local environment check for installed tool versions (Claude/Codex/Gemini/OpenCode), with `r` to refresh.
- **Providers (TUI)**: Gemini provider forms now support configuring `GEMINI_MODEL` (API Key auth).

### Changed

- **Release**: Publish versionless GitHub Release assets; README download links updated accordingly.
- **Storage**: Align SQLite store implementation with upstream for parity.

### Fixed

- **Codex**: Support provider configs that contain a full `config.toml` (extract provider settings from `model_provider`/`model_providers`).

## [4.5.0] - 2026-02-01

### Added

- **Interactive (TUI)**: New provider/MCP add & edit forms (templates + fields + JSON preview).
- **Providers (TUI)**: Per-provider toggle to attach (or skip) the common config snippet when applying.

### Changed

- **Interactive (TUI)**: API keys are shown in add/edit forms (masked only in provider detail view).
- **Interactive (TUI)**: JSON preview hides cc-switch internal metadata fields.

### Fixed

- **Interactive (TUI)**: Switching from an official template back to Custom now clears template-filled values.

## [4.4.0] - 2026-01-31

### Added

- **Interactive (TUI)**: New ratatui-based TUI (recommended).
- **Interactive (MCP)**: Multi-app selector (`m`) to enable/disable MCP servers per app.
- **Sync**: Safe auto live-sync policy (skip live writes/deletes when the target app is uninitialized).
- **Skills**: Align SSOT skills system with upstream; auto-migrate SSOT on list.

### Changed

- **Alignment**: Upstream backend parity Phase 1/2 (Provider + MCP behavior and data model alignment).

### Fixed

- **Deeplink**: Validate endpoints and homepage during provider import.

## [4.3.1] - 2026-01-29

### Fixed

- **Codex**: Fix provider switching where `base_url` could be lost and `wire_api` could reset to `chat` after switching providers multiple times. Fixes #15.

## [4.3.0] - 2026-01-17

### Added

- **Interactive**: Search/filter functionality for menus:
  - Main menu: Press `/` to enter search mode, filter menu items by keyword
  - All select menus (providers, MCP servers, prompts): Type to filter (fuzzy matching enabled)
  - Help text updated to inform users about filtering capability
  - Esc clears active filter before exiting

## [4.2.11] - 2026-01-17

### Fixed

- **Interactive**: Enable paste functionality in provider add/edit prompts by disabling bracketed paste mode. Fixes issue where paste appeared broken in terminals with bracketed paste enabled (zsh/fish, tmux/zellij).

## [4.2.10] - 2026-01-08

### Added

- **Interactive**: Per-app theme colors in TUI: Codex (green), Claude (cyan), Gemini (magenta), including submenu prompts.

## [4.2.9] - 2026-01-08

### Fixed

- **Codex**: Update the default model to `gpt-5.2-codex` (aligns with latest Codex config docs).

## [4.2.8] - 2026-01-08

### Fixed

- **Providers**: The “Official provider” add mode is now Codex-only (Claude Code uses third-party flow only).
- **Codex (Official provider)**: Skip prompts for website URL / model / wire API; uses `requires_openai_auth = true` and `wire_api = "responses"` by default.

## [4.2.7] - 2026-01-08

### Added

- **Providers (Codex/Claude)**: Under `➕ Add Provider`, add a submenu to choose **Official** vs **Third-Party** providers.

## [4.2.6] - 2026-01-08

### Fixed

- **Codex**: Allow switching providers when `~/.codex/auth.json` is absent (credential store / keyring mode).

## [4.2.5] - 2026-01-08

### Added

- **Interactive**: Use `←/→` on the main menu to switch the current application.

## [4.2.4] - 2026-01-06

### Added

- **Interactive**: Press `Esc` to go back to the previous step (no more “Selection cancelled” errors).

## [4.2.3] - 2026-01-06

### Fixed

- **Interactive**: Clear the terminal between screens/menus to prevent ghosting (“拖影”) and keep the UI clean.

## [4.2.2] - 2026-01-06

### Fixed

- **Interactive**: The common config snippet editor now uses the same external editor flow as provider JSON editing (opens your `$EDITOR` via external editor). Fixes #11.

## [4.2.1] - 2026-01-06

### Added

- **Interactive**: Add a JSON editor in `⚙️ Configuration Management → 🧩 Common Config Snippet` to edit/apply per-app common config snippets (Claude use-case: shared env like `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`). Fixes #11.

## [4.2.0] - 2026-01-06

### Added

- **Common Config Snippet**: Add `cc-switch config common` to manage per-app common config snippets (useful for shared Claude settings like `env.CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` and `includeCoAuthoredBy`). Fixes #11.

### Changed

- **Claude**: Merge the common config snippet into the live `~/.claude/settings.json` when switching providers.
- **Claude**: Strip values matching the common snippet when backfilling provider snapshots, so common settings stay global across providers.

## [4.1.4] - 2026-01-06

### Fixed

- **Providers (Interactive)**: When adding the first provider for an app, auto-set it as the current provider (prevents “current provider is empty” and unlocks switching). Fixes #10.

## [4.1.3] - 2026-01-06

### Fixed

- **Codex (0.63+)**: Avoid writing `env_key = "OPENAI_API_KEY"` into `~/.codex/config.toml` by default (prevents `Missing environment variable: OPENAI_API_KEY`).
- **Codex**: Generate provider config using `requires_openai_auth = true` for OpenAI-auth flows; interactive provider add/edit now lets you choose auth mode.

## [4.1.0] - 2025-11-25

### Added

- **Interactive Provider Management**: Complete implementation of add/edit provider flows in interactive mode
  - Full-featured provider creation with validation
  - In-place provider editing with current values pre-filled
  - ID column display in provider tables for easier reference
- **Port Testing**: Added endpoint connectivity testing for API providers
  - Test reachability of API endpoints before switching
  - Validates base URLs and ports are accessible
- **Prompts Deactivate Command**: New `prompts deactivate` command to disable active prompts
  - Supports multi-app deactivation (Claude/Codex/Gemini)
  - Removes active prompt files from app directories
- **Toggle Prompt Mode**: Added ability to switch between prompt switching modes
  - Configure how prompts are activated and managed
  - Interactive mode support for toggling settings
- **Environment Management Commands**: Full implementation of environment variable detection
  - `env check`: Detect conflicting API keys in system environment
  - `env list`: List all relevant environment variables by app
  - Helps identify issues when provider switching doesn't take effect
- **Delete Commands for Prompts**: Multi-app support for deleting prompts
  - Delete prompts from all configured apps at once
  - Proper cleanup of prompt files and configuration

### Changed

- **Interactive Mode Refactoring**: Reorganized into modular structure (~1,254 lines reorganized)
  - Split into 6 focused submodules: `provider.rs`, `mcp.rs`, `prompts.rs`, `config.rs`, `settings.rs`, `utils.rs`
  - Improved code maintainability and separation of concerns
  - Better error handling and user feedback
- **Command Output Enhancement**: Improved formatting and alignment in command mode
  - Better table formatting for command-line output
  - Consistent status indicators and color coding
- **Backup Management**: Enhanced interactive backup selection and management
  - Improved backup listing with timestamps
  - Better restore flow with confirmation prompts

### Fixed

- Command mode table alignment issues in provider display
- ID column visibility in interactive provider lists
- Provider add/edit validation edge cases

### Removed

- Environment variable set/unset features (removed for safety)
  - Users must manually manage environment variables
  - Tool now focuses on detection only to prevent accidental overwrites

### Technical

- 15 commits since v4.0.1
- Cargo.toml version updated to 4.1.0
- Core business logic preserved at 100%
- All changes maintain backward compatibility with existing configs

---

## [4.0.2-cli] - 2025-11-24

### Changed

- **Interactive CLI Refactoring**: Reorganized interactive mode into modular structure (~1,254 lines)
  - Split functionality into 6 focused submodules: `provider.rs`, `mcp.rs`, `prompts.rs`, `config.rs`, `settings.rs`, `utils.rs`
  - Improved code maintainability and separation of concerns
- **Provider Display Enhancement**: Replaced "Category" field with "API URL" in interactive mode
  - Provider list now shows actual API endpoints instead of category labels
  - Detail view displays full API URL with app-specific extraction logic
  - Added support for Claude (`ANTHROPIC_BASE_URL`), Codex (`base_url` from TOML), Gemini (`GEMINI_BASE_URL`)

### Added

- Configuration management menu with 8 operations (export, import, backup, restore, validate, reset, show full, show path)
- Enhanced MCP management options (delete, enable/disable servers, import from live config, validate command)
- Extended prompts management (view full content, delete prompts, view current prompt)
- ~395 lines of new i18n strings for configuration, MCP, and prompts operations

### Removed

- Category selection prompt in "Add Provider" interactive flow
- Category column from provider list tables in interactive mode

---

## [4.0.1-cli] - 2025-11-24

### Fixed

- Documentation updates and corrections

---

## [4.0.0-cli] - 2025-11-23 (CLI Edition Fork)

### Overview

Complete migration from Tauri GUI application to standalone CLI tool. This is a **CLI-focused fork** of the original CC-Switch project.

### Breaking Changes

- Removed Tauri GUI (desktop window, React frontend, WebView runtime)
- Removed system tray menu, auto-updater, deep link protocol (`ccswitch://`)
- Users must transition to command-line or interactive mode

### New Features

- **Dual Interface Modes**: Command-line mode + Interactive TUI mode
- **Default Interactive Mode**: Run `cc-switch` without arguments to enter interactive mode
- **Provider Management**: list, add, edit, delete, switch, duplicate, speedtest
- **MCP Server Management**: list, add, edit, delete, enable/disable, sync, import/export
- **Prompts Management**: list, activate, show, create, edit, delete
- **Configuration Management**: show, export, import, backup, restore, validate, reset
- **Utilities**: shell completions (bash/zsh/fish/powershell), env check, app switch

### Removed

- Complete React 18 + TypeScript frontend (~50,000 lines)
- Tauri 2.8 desktop runtime and all GUI-specific features
- 200+ npm dependencies

### Preserved

- 100% core business logic (ProviderService, McpService, PromptService, ConfigService)
- Configuration format and file locations
- Multi-app support (Claude/Codex/Gemini)

### Technical

- **New Stack**: clap v4.5, inquire v0.7, comfy-table v7.1, colored v2.1
- **Binary Size**: ~5-8 MB (vs ~15-20 MB GUI)
- **Startup Time**: <50ms (vs 500-1000ms GUI)
- **Dependencies**: ~20 Rust crates (vs 200+ npm + 50+ Rust)

### Credits

- Original Project: [CC-Switch](https://github.com/farion1231/cc-switch) by Jason Young
- CLI Fork Maintainer: saladday

---

## [3.7.1] - 2025-11-22

### Fixed

- Skills third-party repository installation (#268)
- Gemini configuration persistence
- Dialog overlay click protection

### Added

- Gemini configuration directory support (#255)
- ArchLinux installation support (#259)

### Improved

- Skills error messages i18n (28+ messages)
- Download timeout extended to 60s

---

## [3.7.0] - 2025-11-19

### Major Features

- **Gemini CLI Integration** - Third major application support
- **MCP v3.7.0 Unified Architecture** - Single interface for Claude/Codex/Gemini
- **Claude Skills Management System** - GitHub repository integration
- **Prompts Management** - Multi-preset system prompts
- **Deep Link Protocol** - `ccswitch://` URL scheme
- **Environment Variable Conflict Detection**

### New Features

- Provider presets: DouBaoSeed, Kimi For Coding, BaiLing
- Common config migration to `config.json`
- macOS native design color scheme

### Statistics

- 85 commits, 152 files changed
- Skills: 2,034 lines, Prompts: 1,302 lines, Gemini: ~1,000 lines

---

## [3.6.0] - 2025-11-07

### New Features

- Provider Duplicate
- Edit Mode Toggle
- Custom Endpoint Management
- Usage Query Enhancements
- Auto-sync on Directory Change
- New Provider Presets: DMXAPI, Azure Codex, AnyRouter, AiHubMix, MiniMax

### Technical Improvements

- Backend: 5-phase refactoring (error handling, commands, services, concurrency)
- Frontend: 4-stage refactoring (tests, hooks, components, cleanup)
- Hooks unit tests 100% coverage

---

## [3.5.0] - 2025-01-15

### Breaking Changes

- Tauri commands only accept `app` parameter (values: `claude`/`codex`)
- Frontend type unified to `AppId`

### New Features

- MCP (Model Context Protocol) Management
- Configuration Import/Export
- Endpoint Speed Testing

---

## [3.4.0] - 2025-10-01

### Features

- Internationalization (i18next) with Chinese default
- Claude plugin sync
- Extended provider presets
- Portable mode and single instance enforcement

---

## [3.3.0] - 2025-09-22

### Features

- VS Code integration for provider sync _(Removed in 3.4.x)_
- Codex provider wizard enhancements
- Shared common config snippets

---

## [3.2.0] - 2025-09-13

### New Features

- System tray provider switching
- Built-in update flow via Tauri Updater
- Single source of truth for provider configs
- One-time migration from v1 to v2

---

## [3.1.0] - 2025-09-01

### New Features

- **Codex application support** - Manage auth.json and config.toml
- Multi-app config v2 structure
- Automatic v1→v2 migration

---

## [3.0.0] - 2025-08-27

### Major Changes

- **Complete migration from Electron to Tauri 2.0**
- 90% reduction in bundle size (~150MB → ~15MB)
- Significantly improved startup performance

---

## [2.0.0] - Previous Electron Release

- Multi-provider configuration management
- Quick provider switching
- Import/export configurations

---

## [1.0.0] - Initial Release

- Basic provider management
- Claude Code integration
