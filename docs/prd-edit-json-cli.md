# PRD: edit-json CLI 命令

## Problem Statement

cc-switch 用户目前只能通过 TUI 表单编辑 provider 的 `settings_config`。当用户需要批量修改 JSON 字段、写入自定义 KV（extensions/experimental 配置），或对 JSON 结构做精细调整时，表单操作效率低下且容易受限。用户需要一个绕过表单、直接在外部编辑器中编辑原始 JSON 的 CLI 入口。

## Solution

新增顶级子命令 `cc-switch edit-json provider <id> --app-type <type>`，工作流类似 `git commit`：打开外部编辑器预填当前 JSON → 用户编辑保存 → 校验 → 直接写入 DB。首版仅支持 `provider` 实体，后续可扩展。

## User Stories

1. As a developer, I want to run `cc-switch edit-json provider <id> --app-type claude` so that I can directly edit the raw `settings_config` JSON in my preferred editor.
2. As a developer, I want the editor to be pre-filled with the current `settings_config` (pretty-printed), so that I don't need to manually look up or copy the existing JSON.
3. As a developer, I want JSON syntax validation to catch malformed edits before they hit the database, so that I don't corrupt my provider config.
4. As a developer, I want a clear error message with the edited content echoed back when validation fails, so that I can recover my work instead of losing it.
5. As a developer, I want the command to detect when I haven't made any changes and cancel gracefully, so that I don't accidentally trigger unnecessary DB writes.
6. As a developer, I want the command to enforce that `settings_config` is always a JSON Object (not an array, string, number, or null), so that the app config format remains consistent.
7. As a Codex user, I want non-official Codex providers to be validated for a non-empty `base_url` in their TOML config snippet, so that I don't save a broken Codex config.
8. As a developer, I want my custom JSON keys (not covered by any TUI form field) to be preserved as-is when I edit via this command, so that extensions and experimental configs are not lost.
9. As a developer, I want the command to respect my `$EDITOR` / `$VISUAL` environment variables, with a fallback to `vi`, so that my preferred editor is used.
10. As a developer, I want the command output to clearly indicate success, cancellation, or the specific validation error that occurred.
11. As a developer, I want the command to fail early with a clear error if the specified provider ID does not exist, so that I don't open an editor with empty content by mistake.
12. As a future developer, I want the `edit-json` command structure to easily support other entities (mcp, prompts) by adding new subcommands under the same top-level command, without restructuring the CLI.

## Implementation Decisions

### 模块架构

四个关注点，分层协作：

**CLI 编排层** (`src-tauri/src/cli/commands/edit_json.rs`) — 新建文件，负责解析 clap 参数、调用 editor、编排工作流、格式化输出。不直接操作 DB，不实现校验逻辑。

**通用校验** (同文件内私有逻辑) — JSON 语法校验（`serde_json::from_str::<Value>`）和类型约束校验（必须是 JSON Object）。这两项是通用 one-liner，不涉及业务知识，放在编排层合理。

**业务校验** (委托给 `ProviderService`) — Codex base_url 检查复用 `ProviderService` 中已有的两个函数：
- `is_codex_official_provider(provider: &Provider) -> bool` — 判断是否官方 Codex provider（`src-tauri/src/services/provider/mod.rs:76`）
- `codex_config_has_base_url(config_text: &str) -> bool` — 解析 TOML 配置中的 base_url 并验证非空（`src-tauri/src/services/provider/mod.rs:88`），已覆盖 `base_url` 顶层键和 `model_providers.<key>.base_url` 两种 TOML 结构

这两个函数当前为 `fn`（私有），需改为 `pub(crate)`。

**为什么不在 edit_json.rs 中重新实现业务校验：** `is_codex_official_provider` 已在代码库中定义了 5 份独立副本（`services/provider/mod.rs`、`cli/commands/provider.rs`、`cli/tui/runtime_actions/editor.rs`、`cli/tui/data.rs`、`cli/tui/form/provider_state.rs`），7 个调用点散布在各层。新增第 6 份副本会让未来 Codex 配置格式变化时需要修改 6 处。暴露 ProviderService 中的 canonical 实现，建立单一事实来源。

**编辑-json mcp 扩展时遵循相同模式：** 找到 McpService 中已有的 validate 函数，暴露为 `pub(crate)` 后调用。

**持久化层** (复用已有 `Database::update_provider_settings_config`) — 位于 `src-tauri/src/database/dao/providers.rs:413-433`，直接执行 `UPDATE providers SET settings_config = ?1 WHERE id = ?2 AND app_type = ?3`，绕过 in-memory config 模型和全量快照持久化路径，只做单列部分更新。

### 命令参数设计

```
cc-switch edit-json provider <id> --app-type <type>
```

- `<id>` — 位置参数，provider ID
- `--app-type` — 必填选项，严格匹配 `claude | codex | gemini | opencode | openclaw`，复用 `AppType` 的 `FromStr` 实现（`src-tauri/src/app_config.rs:315-337`）

命令作为 `Commands` 枚举的顶级变体 `EditJson(commands::edit_json::EditJsonCommand)`，内部 `EditJsonCommand` 枚举目前只有一个 `Provider` 变体，便于后续扩展 `Mcp`、`Prompts` 等。

### 外部编辑器调用

复用 `crate::cli::editor::open_external_editor(initial_content: &str) -> Result<String, AppError>`（`src-tauri/src/cli/editor.rs:4-7`），底层为 `edit` crate（`edit = "0.1"`），自动读取 `$EDITOR` → `$VISUAL` → `vi` 回退链。

### 临时文件生命周期

`edit` crate（v0.1）在返回 `Ok(String)` 后自动删除临时文件。校验失败时内容已在内存中，无需保留临时文件。设计文档中"保留临时文件"的 recoverability 需求通过错误输出中回显编辑后的 JSON 内容来实现。

### 数据库读取路径

不走 `ProviderService` / in-memory `MultiAppConfig` 路径。直接通过 `Database::get_provider_by_id(app_type: &str, id: &str)` 查询 DB（`src-tauri/src/database/dao/providers.rs:127-176`），只读取 `settings_config` 列。该查询使用复合主键 `(id, app_type)`，返回 `Result<Option<Provider>, AppError>`。未找到时返回 `AppError::InvalidInput`。

### 工作流编排

```
1. 解析 CLI 参数 (provider ID, app_type)
2. 获取 AppState::try_new()，打开 DB
3. db.get_provider_by_id(app_type.as_str(), &id)
   - 未找到 → Err(AppError::InvalidInput("provider '<id>' not found for app '<app_type>'"))
4. serde_json::to_string_pretty(&provider.settings_config) → 初始内容
5. open_external_editor(&initial) → 编辑后内容
6. 比较编辑后内容.trim() == 初始内容.trim()
   - 相同 → println!("未修改，已取消"); return Ok(())
7. serde_json::from_str::<Value>(&edited) → JSON 语法校验
   - 失败 → Err(AppError::Message("JSON 解析失败: <detail>"))
8. 校验编辑后内容为 JSON Object
   - 非 Object → Err(AppError::Message("settingsConfig 必须为 JSON Object"))
9. 业务校验 (仅 Codex 非官方 provider):
   - 调用 ProviderService::is_codex_official_provider(&provider) 判断
   - 非官方时提取 settings_config["config"] 字符串 → ProviderService::codex_config_has_base_url(config_str)
   - 返回 false → Err(AppError::Message("Codex provider 必须配置非空的 base_url"))
10. db.update_provider_settings_config(app_type.as_str(), &id, &new_value)
11. println!("✓ 已更新 provider '<id>' (<app_type>) 的 settingsConfig")
```

### 变更文件清单

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `src-tauri/src/cli/commands/edit_json.rs` | **新建** | CLI 编排：参数定义、workflow 调用、输出格式化；通用校验（JSON 语法、Object 类型约束）|
| `src-tauri/src/cli/commands/mod.rs` | 修改 | 新增 `pub mod edit_json;` |
| `src-tauri/src/cli/mod.rs` | 修改 | `Commands` 枚举新增 `EditJson(commands::edit_json::EditJsonCommand)` 变体 |
| `src-tauri/src/main.rs` | 修改 | `run()` 函数新增 `Commands::EditJson(cmd)` dispatch 分支 |
| `src-tauri/src/services/provider/mod.rs` | 修改 | `is_codex_official_provider` 和 `codex_config_has_base_url` 可见性从 `fn` 改为 `pub(crate)` |

### 命令是否需要启动状态

`edit-json` 需要读取和写入 DB，因此必须初始化 `AppState`。在 `command_requires_startup_state()` 中属于默认的 `true` 分支，**无需修改**该函数。

### 错误输出约定

- 使用 `crate::cli::ui` 模块的 `success()`（绿色）、`info()`（青色）、`error()`（红色）进行格式化输出
- 校验失败时，使用 `error()` 输出错误详情，并回显编辑后的 JSON 内容（满足 recoverability 需求）
- 成功时使用 `success()` 输出确认信息
- 未修改时使用 `info()` 输出取消提示

## Testing Decisions

### 测试原则

只测试外部可观测行为：给定输入，验证输出/副作用。不测试临时文件路径、编辑器调用过程（`edit` crate 自身已测）。不测试 `ProviderService` 内部校验逻辑（应在 service 层单独测试）。

### 测试模块

**集成测试** — `src-tauri/src/cli/commands/edit_json.rs` 内的 `#[cfg(test)] mod tests`，使用 `Database::memory()` 创建内存 SQLite：

1. **成功更新** — 写入一个 provider，调用 workflow 核心函数（跳过编辑器交互，直接传入编辑后的 JSON 字符串），验证 DB 中 settings_config 已更新为预期值
2. **provider 不存在** — 查询不存在的 ID，验证返回 `AppError::InvalidInput`，且未触发编辑器调用
3. **JSON 语法错误** — 传入非 JSON 字符串（如 `{broken`），验证返回 `AppError` 且包含描述性错误信息
4. **非 Object 校验** — 分别传入 JSON array (`[]`)、string (`"hello"`)、number (`42`)、null (`null`)，验证均返回 `AppError`
5. **未修改检测** — 传入与 DB 中原始 JSON 完全相同的字符串，验证返回 `Ok(())` 且 DB 中数据未被 UPDATE
6. **边界：编辑器清空后保存 `{}`** — 传入 `{}`，验证通过校验并成功写入 DB（空 Object 合法）

> 注意：Codex base_url 校验的正确性测试属于 `ProviderService::codex_config_has_base_url` 的单元测试范围，不在 edit-json 命令的集成测试中覆盖。edit-json 只需验证校验被正确**编排调用**（正式 provider 跳过、非正式 provider 触发），不对 TOML 解析内部路径做重复测试。

### 测试参考

- `Database::memory()` 构造器：`src-tauri/src/database/mod.rs`
- 已有测试模式：`src-tauri/src/database/tests.rs`（内存 DB + 直接构造测试数据）
- DAO 层测试：`src-tauri/src/database/dao/settings.rs:243`（`#[test]` 标注）

## Out of Scope

- 编辑 `mcp`、`prompts`、`skills` 等其他实体的 `edit-json` 子命令
- 编辑 `meta` 字段（仅限 `settings_config` 列）
- 在编辑器中做 JSON Schema 自动补全或实时校验
- 支持通过 stdin / pipe 传入 JSON（仅支持外部编辑器交互）
- 批量编辑多个 provider
- 将校验失败的临时文件持久化到磁盘（`edit` crate 自动清理；recoverability 通过错误输出回显内容实现）
- 回滚 / 撤销功能（依赖 DB 备份机制，非本命令职责）
- 同步更新 in-memory `MultiAppConfig`（edit-json 直接写 DB 单列；in-memory config 在下次 `AppState::try_new()` 时从 DB 重新加载，或 TUI 下次刷新时自然同步）

## Further Notes

- `edit` crate v0.1 在 macOS 上默认使用 `$EDITOR` → `$VISUAL` → `vi` 回退链，与设计文档中的 fallback 策略一致
- 不限制 JSON 内部的 key 集合。TUI 表单已有 `form.extra` 机制保留无法匹配表单字段的自定义 KV（`src-tauri/src/cli/tui/form/provider_state.rs`），本次改动不与 TUI 流程冲突
- `update_provider_settings_config` 是已有方法，现有调用方为 `migrate_legacy_codex_configs`（`src-tauri/src/store.rs:435`），本次新增调用不会改变其语义
- 命令输出使用 ASCII 字符（`✓`），不依赖 emoji，与现有 CLI 输出风格一致
- `is_codex_official_provider` 在 `ProviderService` 中的实现比 TUI/CLI 层的 4 份副本更简洁：只检查 `meta.codex_official` 和 `category == "official"` 两个条件，不含 `website_url` 和 `name` 的硬编码判断。暴露后所有调用方应逐步迁移到此实现
