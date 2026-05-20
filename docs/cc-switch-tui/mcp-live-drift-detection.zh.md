# MCP Live Drift Detection Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** 在 MCP 管理界面中暴露 live 配置与 cc-switch 数据库之间的差异，让用户能看到并显式处理 Codex CLI 或手工编辑造成的 MCP 配置漂移。

**Architecture:** 保持 cc-switch 数据库仍是托管 MCP 的默认写入来源，但新增只读 diff 层读取 live 配置并与数据库快照比较。UI 只提示差异，不默认自动导入或覆盖；用户通过显式动作选择“从 live 更新 cc-switch”或“用 cc-switch 覆盖 live”。先修正 Codex provider switch 仍触发全量 MCP sync 的风险，再接入 drift 检测。

**Tech Stack:** Rust、现有 `McpService` / TUI `UiData` / `toml_edit` / `serde_json`，测试使用 `src-tauri/tests/*.rs` 和 TUI 单元测试。

---

最后更新：2026-05-19

本文档记录 cc-switch-tui MCP live drift 检测与显式解决流程的建议方案。这里的
drift 指同一个 app 的 live MCP 配置和 cc-switch 后台保存的 MCP 配置不一致，例如用户
直接编辑了 `~/.codex/config.toml` 的 `[mcp_servers]`，但没有在 cc-switch 里导入。

如果实现行为变化，以源码为最终依据。

## 背景

当前 MCP 数据流主要是单向托管：

- 正常写入：cc-switch 数据库 -> live app 配置。
- 反向导入：live app 配置 -> cc-switch 数据库，需要用户在 TUI 或 CLI 手动触发。

以 Codex 为例：

- 单项写入路径是 `McpService::toggle_app()` / `McpService::upsert_server()` ->
  `sync_single_server_to_codex()`。
- 手动导入路径是 `McpService::import_from_codex()` -> `mcp::import_from_codex()`。
- MCP 页面读取的是 cc-switch 后台保存的 `McpServer` 列表，不会实时读取 live
  `config.toml`。

这会造成一个 UX 问题：如果用户或 Codex CLI 直接改了 `[mcp_servers]`，cc-switch MCP
页面仍显示旧值；用户继续在 cc-switch 里编辑或 toggle 同一个 server 时，会用数据库里的旧
server spec 覆盖 live 中的新内容。

另一个重要前置问题是：当前普通 provider switch 的 post-commit action 仍会执行
`McpService::sync_all_enabled()`。Codex live 写入本身会 merge 并跳过 `[mcp_servers]`，
但后续全量 MCP sync 仍可能覆盖或删除 live 中与 cc-switch 数据库不一致的 MCP server。
因此 drift 检测之前，建议先收窄或关闭 Codex provider switch 的 MCP 全量同步。

## 非目标

本方案不建议立即做自动双向同步。

明确非目标：

- 启动时自动把 live 配置导入数据库。
- 启动时自动用数据库覆盖 live。
- 静默合并同 ID 的复杂字段冲突。
- 在第一版支持所有 app 的完整字段级 diff UI。
- 保证 live 文件里的注释、排序、空白能被导入到数据库；数据库只保存规范化后的 server
  spec。

原因是 live MCP 配置可能是用户临时修改、注释禁用、Codex CLI 自动生成或外部工具写入的
结果。静默同步会把“不知道谁是权威”的冲突变成不可见的数据破坏。

## 目标体验

第一版目标是“看见差异，并能显式解决”。

用户进入 MCP 页面时：

- 如果当前 app 的 live 配置与 cc-switch 数据库一致，界面不额外打扰。
- 如果 live 有 cc-switch 没有的 server，列表显示 `live only`。
- 如果 cc-switch 有启用到当前 app 的 server，但 live 没有，列表显示 `missing live`。
- 如果同 ID server 两边都有但 spec 不一致，列表显示 `live changed`。
- 如果 live MCP 配置无法解析，显示整体 warning，不阻断数据库列表展示。

用户可以对有 drift 的条目执行显式动作：

- `import live`：把 live 的 server spec 写回 cc-switch 数据库。
- `push db`：把 cc-switch 数据库里的 server spec 写回 live。
- `ignore`：暂不处理，只保留标记。

后续可以扩展 `view diff`，但第一版可以先只显示状态和摘要。

## 数据语义

### 权威来源

默认权威来源仍是 cc-switch 数据库。只有用户选择 `import live` 时，live 才会成为该 server
本次操作的来源。

### 比较粒度

第一版按 server id 比较。

状态建议使用：

- `InSync`：DB 与 live 都存在，且规范化 spec 一致。
- `LiveOnly`：live 中存在，DB 中不存在，或 DB 中没有启用当前 app。
- `DbOnly`：DB 中存在且启用当前 app，但 live 中不存在。
- `Changed`：DB 与 live 都存在且当前 app 启用，但规范化 spec 不一致。
- `LiveInvalid`：live 文件或 live MCP section 无法解析。
- `Unknown`：当前 app 没有 live 读取器，或未初始化导致跳过检测。

### 规范化

比较时不要直接比较 TOML 文本，应比较规范化后的 JSON spec。

Codex 示例：

- 读取 `[mcp_servers]` 和兼容读取历史错误位置 `[mcp.servers]`。
- 将 TOML table 转换为 cc-switch 内部使用的 `serde_json::Value`。
- 对 Codex 远端 MCP 做现有导入语义一致的推断：有 `url` 且无 `type` 时视为 `http`。
- `http_headers` 和旧 `headers` 统一到内部 `headers`。
- stdio 的 `env`、`cwd`、`args` 按现有导入逻辑转换。

比较前可以递归排序 JSON object key，避免 map 顺序影响结果。数组顺序保持有意义，不排序。

## 导入和覆盖语义

### import live

`import live` 面向单个 app 和单个 server id。

行为：

- 如果 DB 中没有该 server：创建 `McpServer`，`id` 和 `name` 默认使用 live id，当前 app
  enabled，其它 app disabled。
- 如果 DB 中已有该 server：只覆盖 `server` spec，并把当前 app enabled。
- 保留已有 metadata：`name`、`description`、`homepage`、`docs`、`tags` 不因覆盖 spec
  而丢失。
- 不自动同步到其它 app。
- 操作完成后保存数据库，并重新计算 drift。

这与现有 `import_from_codex()` 不同。现有导入对已存在 server 只启用 Codex，不覆盖
`server` spec；drift resolve 需要一个更明确的“用 live 覆盖 DB spec”动作。

### push db

`push db` 面向单个 app 和单个 server id。

行为：

- 如果 DB 中该 server 对当前 app 已启用：调用现有单项 sync 写回 live。
- 如果 DB 中该 server 对当前 app 未启用：不应写入，提示该 app 未启用。
- 如果 live 有同 id server：被 DB spec 覆盖，这是用户显式选择。
- 操作完成后重新计算 drift。

### delete / disable 的关系

第一版不新增“从 live 删除 live-only server”的快捷动作。live-only 代表 cc-switch 尚未管理，
贸然删除风险较高。

已有 disable 或 delete 行为保持：

- 对 DB 管理的 server disable 当前 app，继续从对应 live 配置移除该 server。
- 对 DB 管理的 server delete，继续从所有启用 app 的 live 配置移除该 server。

## UI 设计

### MCP 列表

当前 MCP 表格列是：

- Name
- Claude
- Codex
- Gemini
- OpenCode
- OpenClaw
- Hermes

第一版建议新增一个窄列 `Live`，放在 `Name` 后面：

- 空白：无 drift 或 app 不支持检测。
- `~`：同 ID spec 不一致。
- `+`：live only。
- `-`：DB only / missing live。
- `!`：live invalid。

底部 key bar 增加：

- `r resolve` 或 `l live`

如果空间紧张，也可以先不加列，在 `Name` 后追加短文本，例如 `server-name [live changed]`。
不过新增列更容易测试，也不污染 server name。

### 摘要栏

摘要栏可以在原有各 app enabled 数量后追加 drift 汇总：

- `Live drift: 2 changed, 1 live-only`

如果没有 drift，不显示额外文本。

### Resolve overlay

选中有 drift 的行后按 `r`，打开 overlay：

- 标题：`Resolve MCP Live Drift`
- 展示 app、server id、状态。
- 操作项：
  - `Import live into cc-switch`
  - `Push cc-switch to live`
  - `Cancel`

第一版不需要做完整 diff viewer，但建议显示一行摘要：

- `DB command: old-command`
- `Live command: new-command`

如果是 HTTP server，则显示 URL。

### live-only 行如何展示

如果 live 有 DB 没有的 server，MCP 列表需要能展示一行虚拟 row，否则用户看不到它。

建议 `McpRow` 扩展：

- `id`
- `server: Option<McpServer>`
- `live_status`
- `live_app`
- `live_spec_summary`

但这会影响现有大量代码。更小的第一版改法：

- 保持 `McpRow.server` 不变。
- 在 `McpSnapshot` 增加 `live_only: Vec<McpLiveOnlyRow>`。
- MCP UI 渲染时把 DB rows 和 live-only rows 合并成 display rows。

这样可以减少对编辑、toggle、delete 逻辑的影响。选中 live-only row 时，只允许
`import live` 和 `cancel`，不允许直接编辑 DB server。

## 服务层接口

建议新增类型放在 `src-tauri/src/services/mcp.rs`，或拆成
`src-tauri/src/services/mcp_drift.rs` 后由 `services/mod.rs` re-export。

核心结构：

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpLiveDriftKind {
    InSync,
    LiveOnly,
    DbOnly,
    Changed,
    LiveInvalid,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct McpLiveDriftEntry {
    pub app: AppType,
    pub id: String,
    pub kind: McpLiveDriftKind,
    pub db_spec: Option<serde_json::Value>,
    pub live_spec: Option<serde_json::Value>,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct McpLiveDriftReport {
    pub app: AppType,
    pub entries: Vec<McpLiveDriftEntry>,
}
```

服务方法：

```rust
impl McpService {
    pub fn get_live_drift(
        state: &AppState,
        app: AppType,
    ) -> Result<McpLiveDriftReport, AppError>;

    pub fn import_live_server(
        state: &AppState,
        app: AppType,
        id: &str,
    ) -> Result<(), AppError>;

    pub fn push_db_server_to_live(
        state: &AppState,
        app: AppType,
        id: &str,
    ) -> Result<(), AppError>;
}
```

读取 live 的 app-specific helper：

```rust
fn read_live_mcp_servers(app: &AppType)
    -> Result<HashMap<String, serde_json::Value>, AppError>;
```

第一版可以只实现 Codex。其它 app 返回 `Unknown` 或空 report，后续再逐步接入 Claude、
Gemini、OpenCode、OpenClaw、Hermes。

## 实施步骤

### Task 1: 修正 Codex provider switch 的 MCP 全量同步

**Objective:** 避免用户只是切换 Codex provider 时，live `[mcp_servers]` 被 DB 旧值覆盖。

**Status:** 已完成（2026-05-19）

**Files:**

- Modify: `src-tauri/src/services/provider/mod.rs`
- Modify: `src-tauri/src/services/provider/codex.rs`
- Test: `src-tauri/tests/provider_service.rs`

**Steps:**

1. 添加失败测试：live `config.toml` 中同 ID MCP 的 command 与 DB 不同。
2. 执行 `ProviderService::switch(&state, AppType::Codex, "p2")`。
3. 断言 switch 后 live 中该 MCP command 仍是 live 原值。
4. 修改普通 switch 的 `PostCommitAction.sync_mcp`：Codex 切换不触发全量 MCP sync。
5. 同时修正 Codex catalog sync 与 live 稳定 `model_provider` alias 的冲突：当非当前 provider
   的 catalog key 与 live 当前稳定 alias 冲突时，先把该非当前 provider 重写到唯一 key，
   避免 switch 后 live `model_providers.<stable>` 又被旧 provider 快照覆盖。
6. 运行相关测试。

Command:

```bash
cd src-tauri && cargo test switch_codex_provider_preserves_live_mcp_server_edits -q
```

Additional verification:

```bash
cd src-tauri && cargo test --test provider_service provider_service_switch_codex -q
cd src-tauri && cargo test codex_switch_syncs_all_managed_provider_catalog_entries_into_live_config -q
```

### Task 2: 抽取 Codex live MCP 读取器

**Objective:** 复用现有导入转换语义，提供只读 live MCP map。

**Files:**

- Modify: `src-tauri/src/mcp.rs`
- Test: `src-tauri/tests/import_export_sync.rs`

**Steps:**

1. 新增 helper，例如 `read_codex_live_mcp_servers_map()`。
2. 支持 `[mcp_servers]`。
3. 保留对 `[mcp.servers]` 的兼容读取。
4. 复用或抽取 `import_from_codex()` 中 TOML -> JSON spec 的转换逻辑。
5. 添加测试覆盖 stdio、http、env、http_headers。

Command:

```bash
cd src-tauri && cargo test read_codex_live_mcp_servers_map_parses_supported_shapes -q
```

### Task 3: 实现 drift report

**Objective:** 比较 DB enabled state 和 live map，产出稳定 report。

**Files:**

- Modify: `src-tauri/src/services/mcp.rs`
- Test: `src-tauri/tests/mcp_commands.rs`

**Steps:**

1. 新增 drift enum 和 report structs。
2. 实现 `McpService::get_live_drift(state, AppType::Codex)`。
3. DB 侧只比较 `apps.codex == true` 的 server。
4. live-only id 也进入 report。
5. 添加测试覆盖 `InSync`、`Changed`、`LiveOnly`、`DbOnly`。

Command:

```bash
cd src-tauri && cargo test codex_mcp_live_drift_reports_changed_live_only_and_db_only -q
```

### Task 4: 实现 import live / push db

**Objective:** 提供显式 resolve 动作。

**Files:**

- Modify: `src-tauri/src/services/mcp.rs`
- Test: `src-tauri/tests/mcp_commands.rs`

**Steps:**

1. 实现 `import_live_server()`。
2. 已存在 server：覆盖 `server` spec，保留 metadata，设置当前 app enabled。
3. 不存在 server：创建最小 `McpServer`。
4. 实现 `push_db_server_to_live()`，内部调用现有单项 sync。
5. 添加测试覆盖 changed 和 live-only 两种 import。
6. 添加测试覆盖 push db 覆盖 live 同 ID server。

Command:

```bash
cd src-tauri && cargo test codex_mcp_resolve_live_drift -q
```

### Task 5: 把 drift report 接入 TUI 数据层

**Objective:** MCP 页面加载时带上 drift 信息。

**Files:**

- Modify: `src-tauri/src/cli/tui/data.rs`
- Modify: `src-tauri/src/cli/tui/runtime_actions/mcp.rs`
- Test: `src-tauri/src/cli/tui/tests.rs`

**Steps:**

1. `McpSnapshot` 增加 drift report 或按 id 索引的 drift map。
2. `UiData::load()` 调用 `McpService::get_live_drift()`，失败时保存 warning 状态，不阻断页面加载。
3. toggle、import、edit 后重新加载数据，确保 drift 状态刷新。
4. 添加数据层测试，覆盖 live 解析失败不阻断 DB rows。

### Task 6: MCP 表格显示 drift 标记

**Objective:** 用户能在列表上直接看见 live drift。

**Files:**

- Modify: `src-tauri/src/cli/tui/ui/mcp.rs`
- Modify: `src-tauri/src/cli/i18n/texts/*.rs`
- Test: `src-tauri/src/cli/tui/ui/tests.rs`

**Steps:**

1. 表格新增 `Live` 列或 name 后缀。
2. 为 `Changed`、`LiveOnly`、`DbOnly`、`LiveInvalid` 显示不同标记。
3. 摘要栏显示 drift 计数。
4. 更新 UI snapshot / rendering tests。

### Task 7: 添加 resolve overlay 和动作

**Objective:** 用户能从 TUI 显式选择 import live 或 push db。

**Files:**

- Modify: `src-tauri/src/cli/tui/app/types.rs`
- Modify: `src-tauri/src/cli/tui/app/content_entities.rs`
- Modify: `src-tauri/src/cli/tui/runtime_actions/mcp.rs`
- Modify: `src-tauri/src/cli/tui/ui/overlay/*`
- Test: `src-tauri/src/cli/tui/app/tests.rs`

**Steps:**

1. 新增 overlay state，记录 app、server id、drift kind、当前选项。
2. MCP 页面 key bar 增加 resolve 入口。
3. 选中 `Import live into cc-switch` 调用 `McpService::import_live_server()`。
4. 选中 `Push cc-switch to live` 调用 `McpService::push_db_server_to_live()`。
5. live-only row 禁用 push db。
6. resolve 成功后重新加载 `UiData` 并显示 toast。

## 启动检测

启动时自动检测是可选增强，不建议第一阶段就强依赖。

如果要做，建议只做 best-effort 提醒：

- 应用启动或进入 MCP 页面时异步/惰性检测。
- 有 drift 时显示一次 toast：`Codex MCP live config has changes not imported into cc-switch`。
- 不自动修改 DB 或 live。
- 检测失败只记录 warning，不影响主流程。

更保守的做法是只在进入 MCP 页面时检测。这样成本低，也避免 TUI 启动时读取多个外部 app 配置
造成性能波动。

## 风险和边界

- Codex TOML 注释不会进入 DB。`import live` 只保存规范化 spec。
- live-only row 不是完整 DB server，不能复用所有现有 row 操作。
- 同 ID 多 app 共享 server spec。对 Codex 执行 `import live` 覆盖 DB spec 后，其它 app
  如果也启用同一个 server，后续同步可能使用新的 spec。这是统一 MCP 结构的既有语义，需要在
  resolve overlay 中提示。
- `sync_all_enabled()` 仍是强覆盖语义。任何调用它的路径都可能把 drift 清掉。实现 drift
  功能时应审计 provider switch、配置恢复、WebDAV 下载后的同步路径。

## 验证清单

- Codex provider switch 不再覆盖 live `[mcp_servers]` 同 ID 手工修改。
- MCP 页面能显示 DB rows，即使 live config 解析失败。
- live-only server 能显示并导入成 DB server。
- changed server 能用 live 覆盖 DB spec，metadata 保留。
- changed server 能用 DB 覆盖 live spec。
- 普通 MCP edit / toggle 后 drift 状态刷新。
- `cd src-tauri && cargo test` 通过。
