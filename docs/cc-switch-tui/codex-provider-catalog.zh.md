# Codex Provider Catalog 设计说明

最后更新：2026-05-17

本文档记录 cc-switch-tui 对 Codex provider 的 catalog 管理逻辑。这里的
catalog 指的是 Codex live `config.toml` 里的 `[model_providers.*]` 表，而不是
cc-switch-tui 自己的 SQLite provider 列表。本文用于解释这次 Codex-only 优化后的
数据流、边界和维护约束；如果行为变化，以源码为最终依据。

## 目标

这次调整只针对 Codex，不改变 Claude、Gemini、OpenCode、OpenClaw、Hermes 的
provider 行为。

目标有三条：

- TUI 后台保存的所有 Codex 自定义 provider，都应同步出现在 live
  `~/.codex/config.toml` 的 `[model_providers.*]` 中。
- 在 Codex 的 Providers 页面按 `i` 时，应导入当前 live config 里的所有可识别
  provider，而不是只导入一个 `default` 快照。
- 导入行为只负责合并或新增 provider，不负责切换当前 provider。

## 相关源码

- 服务层主逻辑：`src-tauri/src/services/provider/codex.rs`
- provider 提交后置动作：`src-tauri/src/services/provider/mod.rs`
- 配置保存后的 live 同步：`src-tauri/src/services/config.rs`
- TUI 导入动作：`src-tauri/src/cli/tui/runtime_actions/providers.rs`
- TUI 按键入口：`src-tauri/src/cli/tui/app/content_entities.rs`
- TUI 文案和提示：`src-tauri/src/cli/tui/ui/providers.rs`

## 数据模型

cc-switch-tui 现在为 Codex provider 增加了一个额外 metadata 字段：

- `ProviderMeta.codexModelProviderKey`

它表示该 provider 在 Codex live config 中对应的稳定外部 key，也就是
`[model_providers.<key>]` 里的 `<key>`。

这个字段的作用是把两个层面分开：

- cc-switch-tui 内部 provider id：SQLite / JSON 里的 provider 主键。
- Codex live catalog key：写进 `config.toml` 的外部 key。

这两者不再要求相同，也不应该相互覆盖。

## 写回 live config

### 写回目标

Codex live config 里的 `[model_providers.*]` 现在被视为两部分叠加结果：

- 当前 provider 切换后写入的活动配置；
- TUI 保存的全部 Codex 自定义 provider catalog。

也就是说，当前 provider 负责决定 live 顶层的：

- `model_provider`
- `model`
- 以及当前生效 provider 的其他顶层配置

而 catalog 同步负责保证：

- 所有 TUI 管理的自定义 provider 都存在于 `[model_providers.*]`

### 触发时机

下列路径会触发 Codex catalog 写回：

- 新增 Codex provider
- 更新 Codex provider
- 删除 Codex provider
- 切换当前 Codex provider
- 保存整体配置

对应实现是 `PostCommitAction` 新增了：

- `write_live_snapshot`
- `sync_codex_catalog`
- `stale_codex_catalog_keys`

这样可以把“是否重写当前 live snapshot”和“是否补写 catalog”拆开处理。

## catalog 来源

写回 live config 时，catalog 来源只取 cc-switch-tui 当前保存的 Codex provider。

过滤规则：

- 官方 Codex provider 不参与 catalog 写回。
- 没有可解析 `config` 的 provider 跳过。
- 坏掉的旧 snapshot 不会中断整个写回流程，只会记录 warning 并跳过该项。

这里专门做了容错，是因为历史数据库里可能存在不可解析的旧 Codex snapshot；如果因为
一个坏快照就让“设置 common snippet”、“切换 provider”或“保存配置”全部失败，代价太大。

## key 解析规则

单个 provider 写回 catalog 时，key 的来源按顺序是：

1. `meta.codexModelProviderKey`
2. provider 自身 `settings_config.config` 中可解析的 `model_provider`
3. 如果 snapshot 只包含一个 `[model_providers.*]` 项，则回退到那个唯一 key

只要拿到 key，就会把对应的 `[model_providers.<key>]` 整项写回 live config。

## 导入 live config

### 入口

Codex 的 Providers 页面新增了 `i`：

- 空列表时，空态说明会明确提示它会导入当前 `config.toml` 里的全部可识别 provider
- 非空列表时，底部 key bar 也会显示 `i import current config`

运行时入口是 `Action::ProviderImportLiveConfig`，在 Codex 下会调用
`ProviderService::import_codex_providers_from_live()`。

### 导入范围

导入会读取：

- `~/.codex/config.toml`
- `~/.codex/auth.json`

然后枚举当前 live config 里的全部 `[model_providers.*]`。

每个 catalog entry 会被转换成一个临时 provider snapshot，形态是：

- `auth`
- `config`

其中：

- 当前活跃 provider 会带上 live `auth.json`
- 非当前 provider 默认没有 auth，只会得到空对象

这是有意设计。因为 Codex live config 本身只保存当前会话正在使用的 auth，不能凭空推断
其他 catalog 项的凭证。

### 合并规则

导入时按下面顺序查找目标 provider：

1. 先按 `codexModelProviderKey` 精确匹配
2. 如果没有唯一 key 匹配，再按 provider 名称精确匹配
3. 两者都没有时，新建 provider

结果统计在 `CodexImportReport` 中：

- `created`
- `merged_by_key`
- `merged_by_name`
- `needs_auth`
- `conflicts`
- `used_default_fallback`

### 同名和冲突

如果按 key 或按 name 匹配时出现多个候选，导入不会猜测，直接计入 `conflicts` 并跳过。

同 key 只允许由一个 provider 持有。写回 live catalog 时如果发现两个 provider 想写同一个
key，会直接报错。

## 当前 provider 语义

导入 catalog 不会自动切换当前 provider。

也就是说：

- live config 当前正在使用哪个 provider，不会因为按了 `i` 而改变
- cc-switch-tui 当前 provider 记录，也不会因为导入 catalog 而改写

这是本次设计里最重要的边界之一。导入动作只同步“可选 provider 集合”，不改变“当前选择”。

## stable alias 去重

Codex 现有逻辑为了保持 resume/history 的连续性，可能会把 live 当前 provider 的
`model_provider` 规范化成一个稳定 alias。

这会带来一个问题：

- live config 里可能同时存在“当前稳定 alias”与“真实 provider key”
- 但这两个条目内容其实是同一个 provider

导入时如果不处理，会把同一个 provider 导入两遍。

现在的处理方式是：

- 先找当前活跃 key 对应的 provider table
- 再扫描所有 `[model_providers.*]`
- 如果发现另一个 table 内容完全相同，就把它视为同一个 provider 的 canonical key
- 当前活跃的 stable alias 会被折叠掉，不再单独导入

这样可以保证：

- 当前 provider 只导入一次
- 保存到 TUI 的 key 仍然保持真实 catalog key，而不是把稳定 alias 反写回数据库

## 删除和改 key

Codex provider 更新或删除时，需要处理旧 key 残留问题。

为此提交后置动作里额外维护了 `stale_codex_catalog_keys`：

- 删除 provider 时，把它旧的 catalog key 从 live config 删掉
- 更新 provider 且 key 发生变化时，也会移除旧 key

这样能避免 live config 里一直残留已经失效的 `[model_providers.old_key]`。

## 异常和回退

### 没有 catalog 时

如果 live `config.toml` 里没有任何 `[model_providers.*]`，Codex 导入会回退到旧行为：

- `import_default_config()`

这保证老用户只有单一 live 配置时，仍然能导入成 `default` provider。

### 坏 snapshot 时

catalog 写回会跳过坏掉的旧 provider snapshot，不阻塞其他正常 provider。

这是专门为了兼容历史脏数据。否则下面这些已有能力都会被一个旧坏数据拖死：

- 设置或清理 Codex common config snippet
- 切换 Codex provider
- 保存配置

## 维护约束

- 这套逻辑是 Codex-only。不要把 `i` 的新行为直接扩散到其他 app。
- `codexModelProviderKey` 是外部 key，不是内部主键；不要拿它替代 provider id。
- 导入动作不能自动写 current provider。
- 如果以后再改 Codex stable provider alias 逻辑，必须同时检查 import 去重逻辑是否仍成立。
- 如果以后允许非当前 provider 保存独立 auth，导入逻辑里“非当前 provider auth 为空”的约束也要一起重审。

## 相关测试

服务层：

- `codex_switch_syncs_all_managed_provider_catalog_entries_into_live_config`
- `import_codex_providers_from_live_merges_catalog_and_skips_active_alias_duplicate`
- 原有的 Codex common snippet / broken snapshot 回归测试

TUI：

- `codex_providers_i_key_imports_current_config`
- `codex_providers_empty_state_shows_catalog_import_copy_and_i_hint`
- `codex_provider_list_key_bar_shows_import_current_config_hint`
