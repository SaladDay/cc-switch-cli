1|# Hermes Agent 支持计划 - cc-switch-tui

## 背景

三个 cc-switch 相关项目的定位和 Hermes 支持现状：

| 项目 | 定位 | Hermes 支持 | 设置同步 |
|---|---|---|---|
| cc-switch | Tauri 桌面 GUI (v3.14.1) | 完整一等公民支持 | WebDAV 云同步 |
| cc-switch-tui | TUI 管理器 (v0.0.1) | 一等公民支持 | WebDAV 云同步 |
| cc-switch-web | Web 版 (v0.10.2-1) | 完全没有 | 无 |

用户在无桌面的 Linux 环境下使用，需要在 CLI 端添加 Hermes Agent 的完整支持（包括设置同步功能）。

## 决策：加在 CLI 端

理由：

1. **WebDAV 云同步已就绪** — CLI 已有完整的 WebDAV 基础设施（上传/下载/自动同步/坚果云预设），Web 端完全没有
2. **数据层已有 30-40% 基础** — `enabled_hermes` 数据库列、`McpApps.hermes` 字段已存在，只需补齐逻辑
3. **与桌面版数据兼容** — CLI 使用 SQLite + WebDAV 协议与桌面版一致，同步数据可无缝互通
4. **桌面版代码可大量复用** — `hermes_config.rs`（1947行）和 `mcp/hermes.rs`（574行）可直接移植，同为 Rust 后端
5. **TUI 足够应对无桌面场景** — 交互式终端界面，SSH 中也能用

## Hermes Agent 配置结构

Hermes 使用 YAML 格式配置文件 `~/.hermes/config.yaml`：

```yaml
model:
  default: "anthropic/claude-opus-4-7"
  provider: "openrouter"
  base_url: "https://openrouter.ai/api/v1"

agent:
  max_turns: 50
  reasoning_effort: "high"

custom_providers:
  - name: openrouter
    base_url: https://openrouter.ai/api/v1
    api_key: sk-or-...
    model: anthropic/claude-opus-4-7
    models:
      anthropic/claude-opus-4-7:
        context_length: 200000

mcp_servers:
  filesystem:
    command: npx
    args: ["-y", "@modelcontextprotocol/server-filesystem"]
```

关键特征：
- **累加式供应商管理**（additive mode），所有供应商共存于同一配置文件
- MCP 无显式 `type` 字段，通过 `command`（stdio）vs `url`（HTTP）推断
- MCP 有 Hermes 专有字段：`enabled`, `timeout`, `connect_timeout`, `tools`, `sampling`, `roots`, `auth`
- Memory 文件：`~/.hermes/memories/MEMORY.md` 和 `~/.hermes/memories/USER.md`
- Web UI：`http://127.0.0.1:9119`，或 `hermes dashboard` 命令

## 实现计划

### Tier 1: 核心 AppType 添加（必做）

| 变更 | 文件 | 工作量 |
|---|---|---|
| 添加 `AppType::Hermes` 枚举变体 | `app_config.rs` | 小 |
| 添加 Hermes match 臂（as_str, is_additive_mode, all, FromStr） | `app_config.rs` | 小 |
| `McpApps::is_enabled_for/set_enabled_for/enabled_apps` 补 Hermes 臂 | `app_config.rs` | 小 |
| `SkillApps` 添加 `hermes` 字段 + 方法臂 | `app_config.rs` | 小 |
| `VisibleApps` 添加 `hermes` 字段 + `app_order()` 更新 | `settings.rs` | 小 |
| `AppSettings` 添加 `hermes_config_dir` + `current_provider_hermes` | `settings.rs` | 中 |
| `CommonConfigSnippets` 添加 `hermes` 字段 | `app_config.rs` | 小 |
| `PromptRoot` 添加 `hermes` 字段 | `app_config.rs` | 小 |
| `MultiAppConfig::default()` 插入 hermes app | `app_config.rs` | 小 |
| `sync_policy::should_sync_live()` 补 Hermes 臂 | `sync_policy.rs` | 小 |
| `prompt_file_path()` 补 Hermes 臂 | `prompt_files.rs` | 小 |

### Tier 2: Hermes 配置模块（核心工作，可从桌面版移植）

| 变更 | 文件 | 工作量 |
|---|---|---|
| 新建 `hermes_config.rs` — 配置目录/路径/读写函数 | 新文件，从桌面版移植（1947行） | 大 |
| 新建 `mcp/hermes.rs` — MCP 格式转换与同步 | 新文件，从桌面版移植（574行） | 中 |
| `ProviderService::write_live_snapshot()` 补 Hermes 臂 | `services/provider/mod.rs` | 大 |
| `ProviderService::refresh_provider_snapshot()` 补 Hermes 臂 | `services/provider/mod.rs` | 中 |
| `ProviderService::import_default_config()` 补 Hermes 臂 | `services/provider/mod.rs` | 中 |
| `ProviderService::read_live_settings()` 补 Hermes 臂 | `services/provider/mod.rs` | 中 |
| `McpService::sync_server_to_app_internal()` 补 Hermes | `services/mcp.rs` | 小 |
| `McpService::remove_server_from_app()` 补 Hermes | `services/mcp.rs` | 小 |
| `import_from_hermes()` 导入函数 | `mcp.rs` | 中 |

### Tier 3: TUI 集成

| 变更 | 文件 | 工作量 |
|---|---|---|
| TUI app state / tab 切换添加 Hermes | `tui/app/app_state.rs` | 小 |
| Hermes 路由和导航（如需自定义页面） | `tui/route.rs` | 可变 |
| `cc-switch start hermes` 命令（可选） | `cli/commands/start.rs` | 中 |

### Tier 4: 数据库 / WebDAV

| 变更 | 文件 | 工作量 |
|---|---|---|
| 如需新 schema 变更，添加 v10→v11 迁移 | `schema.rs` | 小 |
| WebDAV DB_COMPAT_VERSION 可能需 bump | `webdav_sync/mod.rs` | 小 |
| 更新 `McpApps { ..., hermes: false }` 字面量 | `mcp.rs` 等 | 小 |

## 桌面版可复用代码

| 文件 | 行数 | 用途 |
|---|---|---|
| `cc-switch/src-tauri/src/hermes_config.rs` | 1947 | 配置读写、provider CRUD、model 管理、memory 文件 |
| `cc-switch/src-tauri/src/mcp/hermes.rs` | 574 | MCP 格式转换（stdio/HTTP ↔ Hermes YAML）、merge-on-write |
| `cc-switch/src-tauri/src/commands/hermes.rs` | 143 | Tauri IPC 命令（CLI 不需要，但逻辑可参考） |

## MCP 格式映射

| CC Switch 统一格式 (JSON) | Hermes config.yaml (YAML) |
|---|---|
| `{"type":"stdio","command":"npx","args":[...],"env":{}}` | `command: npx`, `args: [...]`, `env: {}` |
| `{"type":"sse"/"http","url":"...","headers":{}}` | `url: "..."`, `headers: {}` |

差异：
- Hermes 无显式 `type` 字段
- Hermes 有专有字段：`enabled`, `timeout`, `connect_timeout`, `tools`, `sampling`, `roots`, `auth`
- 写入时保留 Hermes 专有字段（merge-on-write），导入时剥离

## 开发方式

在 cc-switch-tui 项目中开发时先检查 `git status`，不要假设工作区干净；若存在用户或其他 agent 的未提交改动，必须保留并在其基础上继续。

- 远程：`git@github.com:handy-sun/cc-switch-tui.git`
- 默认分支：`main`

## 配置目录迁移规则（`~/.cc-switch` → 当前配置目录）

启动入口在 `src-tauri/src/main.rs`。程序启动时会先调用 `prompt_legacy_config_migration()`，再初始化 `AppState`；真正复制逻辑在 `src-tauri/src/config.rs` 的 `migrate_legacy_config_dir_if_needed()`。

迁移目标不是固定 `~/.cc-switch-tui`，而是“当前应用配置目录”：

| 场景 | 迁移目标 | 行为 |
|---|---|---|
| 未设置配置目录环境变量 | `~/.cc-switch-tui` | 若旧目录存在且目标不存在/为空，启动前提示确认 |
| 设置 `CC_SWITCH_TUI_CONFIG_DIR=~/.config/cc-switch-tui` | `~/.config/cc-switch-tui` | 若旧目录存在且目标不存在/为空，启动前提示确认并迁移到该目录 |
| 设置旧变量 `CC_SWITCH_CONFIG_DIR` | 不迁移 | 兼容旧覆盖变量，避免把旧路径误当成迁移目标 |
| 目标目录已有内容 | 不迁移 | 避免覆盖当前程序已有配置 |
| 目标目录存在 `.migrated-from-cc-switch` marker | 不迁移 | 已迁移或用户拒绝迁移后不再提示 |

确认提示目前是进入 TUI 前的终端提示，不是 TUI overlay。用户选择默认 `Y` 时，后续 `get_app_config_dir()` 触发非破坏性复制；旧目录保留。用户选择 `N` 时写入 `.migrated-from-cc-switch` marker，后续不再提示。

迁移复制策略：
- 跳过软链接
- 普通文件直接复制
- 普通目录递归复制
- `backups/` 只复制最近 3 个条目

相关测试集中在 `src-tauri/src/config.rs` 的 `config::tests::migration_*`，重点覆盖：
- 默认迁移到 `~/.cc-switch-tui`
- `CC_SWITCH_TUI_CONFIG_DIR` 作为迁移目标
- 旧变量 `CC_SWITCH_CONFIG_DIR` 跳过迁移
- 目标目录已有内容时跳过
- marker 防止重复迁移
- 旧目录保留
- backups 只复制最近 3 个

## Picker 架构（导航边界与 AppType 映射）

三个 const 切片定义各 picker overlay 的 app 子集（`src-tauri/src/app_config.rs`）：

| Const | 用途 | 包含的 App |
|---|---|---|
| `MCP_PICKER_APPS` | MCP server toggle picker | Claude, Codex, Gemini, OpenCode, Hermes |
| `VISIBLE_PICKER_APPS` | Settings "Visible Apps" picker | 全部 6 个 |
| `SKILLS_PICKER_APPS` | Skills app toggle picker | Claude, Codex, Gemini, OpenCode, Hermes |

Handler（`overlay_handlers/pickers.rs`）使用 `CONST.len() - 1` 作为导航上界，`CONST[*selected]` 做 index→AppType 映射。Render 函数（`ui/overlay/pickers.rs`）引用同一组 const。添加新 AppType 时只需更新 const 数组，无需修改 handler/render 逻辑。

`AppType` 已 derive `Copy`，可按值使用。

`four_app_picker_index()` 用于 MCP/Skills picker 初始光标定位，内部引用 `MCP_PICKER_APPS.len() - 1` 做 clamp。
