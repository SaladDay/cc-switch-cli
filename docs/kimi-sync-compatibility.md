# Kimi configuration and database synchronization

The Kimi adapter uses the existing `providers` table with `app_type = "kimi"` and JSON `settings_config`. It does not add tables or columns, or change schema version 18. The native provider attributes are retained in `providerConfig`; each model retains its local alias in `id` and its native attributes (including the upstream `model` and context size) in `config`. Existing flat provider input remains accepted. Cross-provider alias collisions are rejected before writing live configuration.

The database SQL export/import path carries these provider rows unchanged. The regression test covers export for sync, import into another database and reconstruction in an empty Kimi directory. This demonstrates this CLI's round-trip behavior; it does not claim that another upstream client understands Kimi or that all upstream versions retain unknown app rows.

Kimi MCP/Skills enablement is **local deployment state**. The existing shared database has no Kimi enable columns. As with Pi skills, Kimi skill enablement is reconstructed from the native skill directory. MCP enablement is reconstructed from membership in the local `mcp.json`. Restarting retains these states without a schema fork. SQL/WebDAV synchronization carries the shared MCP definitions and skill catalog, but does **not** copy the Kimi enable flags to another machine. Enable the desired definitions on that machine explicitly. Native OAuth files remain managed by Kimi CLI.

## 中文说明

Kimi Provider 继续使用现有 providers 表和 settings_config JSON，数据库 schema 保持 18，不新增表或列。模型别名与真实模型 ID 分开保存，原始扩展字段随配置保留；跨供应商同名模型冲突会在写入前被拒绝。回归测试覆盖数据库同步导出、导入另一数据库以及空目录恢复，但不代表其他上游客户端已支持 Kimi 或保证保留未知 app 数据。

Kimi 的 MCP/Skills 开关属于本机部署状态，从本机 mcp.json 和 skills 目录恢复。重启不会丢失，但数据库/WebDAV 不会将这些开关自动复制到另一机器；目标机器需显式启用。共享定义与 Provider 配置仍走既有数据库同步。Kimi CLI 继续负责原生 OAuth 文件。

## Original CC-Switch revision checked

The upstream named here is **farion1231/cc-switch**, not the CLI fork. At original main `36d950411e622129f285635f43c17e5f35462413`, `src-tauri/src/database/mod.rs:53` declares schema **19**, while this CLI's base declares **18**. The upstream v18→v19 migration adds `enabled_mcode` to MCP/Skills. Its SQL exporter serializes table rows without filtering the provider `app_type`; its AppType enum does not include Kimi. Provider payload storage is opaque JSON, and the providers table has no app-type CHECK constraint.

This PR does not add a further schema divergence. It does **not** resolve the existing fork-wide v19 compatibility gap: a database migrated by the current original application is rejected as too new by this CLI on return. Aligning the shared schema/migrations requires a separate upstream-coordinated change, not a Kimi-only schema increment. Do not advertise latest-original ↔ CLI bidirectional synchronization as verified by this PR.

核对的原版为 farion1231/cc-switch，固定到上述完整 SHA：原版 schema 19、CLI 基线 schema 18。原版升版后的整库回传会触发 CLI 的新版本拒绝保护。这是两项目既有的版本差异，本 Kimi 修复不私自改共享 schema，也不能据此宣称与原版最新版双向同步已经通过。原版尚无 Kimi AppType；其通用 SQL 导出没有按 app_type 过滤 Provider 行。
