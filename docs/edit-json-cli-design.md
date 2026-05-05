# edit-json CLI 命令设计文档

## 需求背景

cc-switch 的 TUI 入口本质工作是：接受用户数据 → 生成 JSON → 写入 DB → 提供给后端使用。

现有流程是通过 TUI 表单收集字段，由表单逻辑拼接 JSON 后入库。现需提供一个新入口，绕过表单，允许用户直接编辑原始 JSON（在外部编辑器中），校验合法后直接保存到 DB。

## 范围

首个版本仅支持 `provider` 实体（`settings_config` 列），后续可扩展至 `mcp`、`prompts` 等其他实体。

## 命令格式

```
cc-switch edit-json provider <id> --app-type <type>
```

- `<id>` — provider ID
- `--app-type` — 必填，严格匹配：`claude` | `codex` | `gemini` | `opencode` | `openclaw`

## 工作流（类似 git commit）

```
            打开外部编辑器
             │
  ┌──────────▼──────────┐
  │ 临时文件预填 JSON   │
  │ (settings_config,   │
  │  pretty-printed)    │
  └──────────┬──────────┘
             │ 用户编辑、保存、关闭
             │
  ┌──────────▼──────────┐
  │ 读取临时文件内容    │
  │ 校验 JSON           │
  │ ↓ 失败 → 报错退出  │
  │ ↓ 未改 → 提示取消  │
  │ ↓ 成功 → 写入 DB   │
  └─────────────────────┘
```

## 编辑目标

只编辑 `providers.settings_config` 列（TEXT，JSON blob）。不编辑 `meta` 及其他字段。

## 编辑器

使用 `$EDITOR` 环境变量，未设置则 fallback 到 `$VISUAL`，再 fallback 到系统默认（macOS: `vi`）。

复用已有的 `crate::cli::editor::open_external_editor()` 函数（`edit` crate 封装）。

## JSON 格式

临时文件中使用 `serde_json::to_string_pretty()` 格式化，保持可读性。

## 校验规则

分三层：

1. **JSON 语法** — `serde_json::from_str::<Value>()` 成功
2. **类型约束** — 必须为 JSON Object（不能是 array/string/null/数字）
3. **业务规则（app-type 特定）** — 复用现有校验逻辑：
   - Codex 非官方 provider：`settings_config.config` 中的 TOML 必须能解析出非空的 `base_url`
   - 其他 app-type 暂无额外业务校验

> 注意：不限制 JSON 内部的 key 集合。用户写入的自定义 KV 会被原样保存，不会丢失。

## 临时文件生命周期

| 场景 | 行为 |
|------|------|
| 保存成功 | 清理临时文件 |
| 校验失败 | 保留临时文件，错误信息中显示文件路径 |
| 未修改退出 | 清理临时文件，输出「未修改，已取消」 |

## 输出

| 场景 | 输出 |
|------|------|
| 保存成功 | `✓ 已更新 provider '<id>' (<app-type>) 的 settingsConfig` |
| 未修改 | `未修改，已取消` |
| JSON 语法错误 | `JSON 解析失败: <error detail>\n临时文件保留在: <path>` |
| 非 Object | `settingsConfig 必须为 JSON Object\n临时文件保留在: <path>` |
| 业务校验失败 | `<具体校验错误信息>\n临时文件保留在: <path>` |

## 命令层级

作为顶级子命令挂载到 `Commands` 枚举下，而非 `ProviderCommand` 下（方便后续扩展 `mcp`、`prompts` 等实体）。

```
cc-switch
├── provider ...
├── mcp ...
├── edit-json <entity-type> <id>   ← 新增
└── ...
```

## 实现涉及的文件

| 文件 | 改动 |
|------|------|
| `src-tauri/src/cli/commands/mod.rs` | 新增 `edit_json` 模块 |
| `src-tauri/src/cli/commands/edit_json.rs` | 新建，核心逻辑 |
| `src-tauri/src/cli/mod.rs` | `Commands` 枚举新增 `EditJson` 变体 |
| `src-tauri/src/main.rs` | `run()` 函数新增 dispatch 分支 |

## 自定义 KV 安全性

现有校验逻辑 `validate_provider_submit` 只检查：
- provider name 非空
- Codex（非官方）的 `settings_config.config` TOML 中 `base_url` 可解析且非空

`update_provider_settings_config` 方法直接序列化 `serde_json::Value` 写入 DB，不做 key 过滤。

TUI 表单回读时，无法匹配到表单字段的 JSON key 存储在 `form.extra` 中，表单保存时重新合并写入，不会丢失。
