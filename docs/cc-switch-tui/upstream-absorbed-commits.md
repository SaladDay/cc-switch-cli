# 上游提交吸收记录

本文件长期记录本仓库从上游 `SaladDay/cc-switch-cli` 吸收过的提交。

本仓库的同步方向是：只从上游仓库合入到本仓库，不把本仓库的 fork 改动回推到上游。

## 记录规则

- **精确合入**：上游 commit hash 是当前分支祖先，或 patch-id 与本仓库提交等价。
- **语义吸收**：没有保留上游原 hash，也不是干净 cherry-pick，但本仓库提交明确吸收了该上游功能。
- **部分覆盖**：本仓库用独立提交实现了相近能力。该状态不等于已合入上游提交，后续继续合上游时需要人工对照。
- 记录时优先写清楚上游 commit、本仓库吸收 commit、吸收状态、范围说明和注意事项。

## 2026-05-20 快照

- 上游：`SaladDay/cc-switch-cli main`
- 上游 ref：`saladday/main` at `26360ae3`
- 本仓库分支：`fit/merge-forked`
- 本仓库 HEAD：`f1d3a3ab`
- 核查结论：当时上游新增的 32 个非 merge 提交中，没有任何一个以原 hash 精确合入；已经使用的功能主要由本仓库聚合同步提交 `ab169b4a` 语义吸收。

### 已语义吸收

| 上游提交 | 上游标题 | 本仓库提交 | 状态 | 范围说明 |
| --- | --- | --- | --- | --- |
| `af3b291f` | `feat(cli): add failover management commands (#165)` | `ab169b4a` | 语义吸收 | 新增 CLI 故障转移管理命令，包括查看、启用、禁用、队列增删、排序和清空。当前代码保留在 `src-tauri/src/cli/commands/failover.rs`。 |
| `397741c5` | `(fix) improve DeepSeek model and reasoning compatibility` | `ab169b4a` | 语义吸收 | 吸收 DeepSeek / reasoning 兼容逻辑，包括 OpenAI chat transform、streaming `reasoning_content` alias、模型列表候选 URL 处理等。 |
| `e7725913` | `feat(tui): add readline text editing shortcuts` | `ab169b4a` | 语义吸收 | 吸收 TUI 文本编辑快捷键，包含 `Ctrl+A/E/U/K/W`、`Alt+B/F` 等；当前核心实现为 `src-tauri/src/cli/tui/text_edit.rs`。 |
| `92ab4425` | `fix(database): improve future schema error` | `ab169b4a` | 语义吸收 | 吸收数据库 future schema 检查和错误提示改进，避免新版本数据库被旧程序继续迁移。 |
| `f80a0695` | `Refine provider TUI actions` | `ab169b4a` | 语义吸收 | 吸收供应商页动作区、详情、快捷键和导入当前配置相关体验调整，并在本仓库内适配 Hermes / OpenClaw 等 fork 扩展。 |
| `0c6f9a65` | `Add provider empty state` | `ab169b4a` | 语义吸收 | 吸收供应商空状态，包括无供应商时的导入当前配置和添加供应商入口。后续本仓库提交 `49a0a921` 又针对 Codex 空状态做了本地扩展。 |
| `5c6d373f` | `Fix broken internal documentation links (#167)` | `9b205d20` | 语义吸收 | 手工吸收内部文档链接修复：将不存在的 `CLAUDE.md` 链接改为 README 链接，并修正 v3.6.0 / v3.6.1 中文 release note 指向同目录英文版本。 |
| `d36070bf` | `(tui)refine footer shortcuts` | `9b205d20` | 语义吸收 | 合入 TUI footer 快捷键压缩展示，移除 `NAV` / `ACT` 标签并优先展示 proxy 开关入口，改善窄中文终端可见性。 |
| `371f4222` | `(prompt)stabilize prompt list order` | `9b205d20` | 语义吸收 | 合入 prompt 列表稳定排序：按 `created_at` 正序并用 id 兜底，避免仅因 `updated_at` 变化导致列表跳动。 |
| `73b7c3c1` | `fix(webdav): avoid upload readback checks` | `20c949fa` | 语义吸收 | 合入 WebDAV 上传策略修复：去掉 check connection 的 probe 写读删、去掉 upload 后 manifest GET readback 校验，保留 manifest HEAD 作为 best-effort metadata，并避免普通上传触发旧 V1 远端清理。 |

### 部分覆盖但未合入原上游提交

| 上游提交 | 上游标题 | 本仓库相关提交 | 状态 | 注意事项 |
| --- | --- | --- | --- | --- |
| `d3c240c5` | `feat: add CODEX_HOME support (#179)` | `3c46a327` | 部分覆盖 | 上游原提交未合入；本仓库独立实现了 Codex MCP live sync 对 `CODEX_HOME` 的支持。后续合上游时应避免重复覆盖路径解析逻辑。 |
| `83307151` | `Improve failover proxy UX` | `c0f5cb52`, `f1d3a3ab` | 部分覆盖 | 本仓库已有早期故障转移控制和 proxy inactive guard 修复，但没有完整吸收上游新增的 `failover_policy.rs`、自动开启 proxy+failover、停 proxy 清理 failover 等整套 UX。 |
| `65c4dc75` 到 `d160b168` | provider common config 系列重构 | `0f510eb6`, `c4409be5` 等 | 部分覆盖 | 本仓库已有更早的 common config 体系；2026-05-15 上游 common config 重构系列未作为提交合入。继续合上游时需要逐项对照语义。 |

### 本轮明确暂不处理

| 上游提交 | 上游标题 | 状态 | 原因 |
| --- | --- | --- | --- |
| `64cbca79` | `(docs) update RightCode rebate to 5%` | 暂不合入 | 该提交仅调整 RightCode 赞助/返利文案，从 25% 改为 5%，不涉及功能或兼容性；本 fork 是否沿用上游营销信息需要另行确认，本轮按指令跳过。 |
| `d3c240c5` | `feat: add CODEX_HOME support (#179)` | 暂不合入 | 本仓库已通过 `3c46a327` 部分覆盖 CODEX_HOME live sync 支持，且当前实现有意采用 `CODEX_HOME` 优先于手动覆盖、支持 `~` 展开、且不要求目录预先存在；上游提交的优先级和存在性判断不同，直接合入会改变现有行为。 |

### 本轮高风险暂不处理

| 上游提交 | 上游标题 | 状态 | 原因 |
| --- | --- | --- | --- |
| `83307151` | `Improve failover proxy UX` | 暂不合入 | 覆盖 proxy start/stop、failover policy、数据库 schema/DAO、provider routing 和 TUI 多处入口；本仓库已有独立 failover guard 修复，直接套上游 patch 会与当前 UX 和 fork 扩展产生多处冲突。 |
| `6ff4f888`, `8afd9075`, `d3810be2`, `3fa27235` | prompt 服务和 prompt 编辑系列 | 暂不合入 | 这组涉及 SQLite prompt service、prompt identity、add/edit form 统一和导入确认；当前本仓库缺少上游新增的 prompt form 文件结构，且 `services/prompt.rs` 与 TUI content/form handler 存在冲突，需要作为独立专题迁移。 |
| `65c4dc75` 到 `d160b168` | provider common config 系列重构 | 暂不合入 | 这组改动 provider live/common config 写入、CLI 命令、TUI editor 和 settings 持久化，且与本仓库现有 Hermes/OpenClaw/provider common config 扩展冲突；需要先定义 fork 行为边界再拆分吸收。 |
| `a1dd240a` | `(tui)add usage query configuration` | 暂不合入 | 该提交新增 Copilot auth、balance/coding plan 服务、usage query 配置 UI 和大量 provider form 状态，变更面超过 6000 行并与当前 TUI settings/form 状态冲突，不适合和 WebDAV 修复同批合入。 |

### 本轮很高风险复核

复核方法：逐个查看上游 diff/stat，用 `git apply --check --3way` 在当前分支上试套 patch，并对照当前 fork 的核心实现。结论是本轮不合入代码，只记录后续迁移边界。

| 上游提交 | 风险焦点 | 复核结论 |
| --- | --- | --- |
| `83307151` | failover / proxy UX、proxy 持久化开关、provider 路由、数据库 DAO 和 TUI 设置页。 | 不直接合入。该提交改动 37 个文件，新增 `cli/failover_policy.rs`，并把“开启自动故障转移”升级为可能自动开启 proxy、切换到队列头 provider、关闭 proxy 时清理 auto failover。试套时 `cli/commands/failover.rs`、`cli/i18n.rs`、`content_entities.rs`、`runtime_actions/settings.rs`、`ui/providers.rs` 等关键 TUI/命令入口冲突；更重要的是它会改变当前 fork 已有的 proxy inactive guard、managed external proxy session、live config/current provider 同步语义。后续应作为独立 failover 迁移：先定行为规格，再迁 service/DAO 测试，最后迁 TUI。 |
| `6ff4f888`, `8afd9075`, `d3810be2`, `3fa27235` | prompt 存储从 config 快照转向 SQLite、prompt identity 编辑、add/edit form 统一、导入前确认。 | 不直接合入。`6ff4f888` 会删除 `store.rs` 对 prompts 的持久化同步，并让 `PromptService` 直接读写 DB；当前 fork 虽已有 `prompts` 表和 store 同步，但 service 仍以 `MultiAppConfig` 快照为主。后续 UI 提交还新增 `cli/tui/app/form_handlers/prompt.rs`、`cli/tui/form/prompt.rs`、`cli/tui/ui/forms/prompt.rs`，这些文件当前树不存在，试套后在 prompt service、content_entities、form/tab/runtime_actions 多处冲突。后续应先把 PromptService DB-first 作为单独迁移并补齐 stale-config/DB 优先级测试，再处理 prompt 表单结构。 |
| `65c4dc75` 到 `d160b168` | provider common config 语义、provider snapshot 归一化、live config 写入、CLI common-config 命令、TUI editor/settings。 | 不直接合入。该系列从 `65c4dc75` 起就会重写 `common_config.rs` 的 `provider_uses_common_config` 判定、Codex common snippet 处理、startup live import 和 provider snapshot 迁移。当前 fork 已有 `common_config_upstream_semantics_migrated_v1` 迁移标记、Hermes/OpenClaw 扩展、Codex runtime-local key 处理和大量 provider tests；试套冲突集中在 `app_state.rs`、`provider_state.rs`、`services/provider/codex.rs`、`services/provider/common_config.rs`、`services/provider/mod.rs`、`store.rs`。其中 `a5914cdd` 虽小，但依赖前序 common config 语义，不适合单独摘。后续需要独立设计“上游 common config 语义”和本 fork additive app/provider 扩展的边界。 |
| `a1dd240a` | usage query 配置、GitHub Copilot 托管认证、balance/coding plan 网络服务、provider form 状态、settings 持久化。 | 不直接合入。该提交改动 46 个文件，新增约 6800 行，包括 `proxy/providers/copilot_auth.rs`、`services/balance.rs`、`services/coding_plan.rs`，并改造 TUI provider 表单、usage script credential 解析和 settings。当前 fork 只有 `services/subscription.rs`、`services/provider/usage.rs` 和既有 usage_script 边界校验；试套冲突落在 TUI 状态机、overlay、settings、provider tests 等位置。该功能还引入外部网络认证、token/account 持久化和多个第三方 API 查询路径，安全与产品行为都需要单独审查，不应作为上游吸收子项混入。 |

### 尚未吸收的上游新增提交

以下提交截至本快照未发现精确合入或明确语义吸收记录：

`fc3b95d1`, `83307151`, `253ce370`, `e3ff1689`, `6ff4f888`, `8afd9075`, `d3810be2`, `50fcb8cd`, `3fa27235`, `564558a2`, `4a292849`, `65c4dc75`, `ee155e69`, `a5914cdd`, `fa96c245`, `8e311ee4`, `d160b168`, `a1dd240a`, `14856f68`, `26360ae3`。

补充说明：`83307151`、`65c4dc75` 到 `d160b168`、`d3c240c5` 在上方标为“部分覆盖”，表示本仓库存在相关能力，但不视为已合入这些上游提交；`64cbca79`、`d3c240c5` 在本轮明确暂不处理；高风险暂不处理项见上表。

## 维护方法

新增记录前建议执行：

```bash
git fetch --no-tags https://github.com/SaladDay/cc-switch-cli.git main
git update-ref refs/remotes/saladday/main FETCH_HEAD
git cherry -v HEAD saladday/main
git log --reverse --no-merges --date=short --format='%h %ad %s' $(git merge-base HEAD saladday/main)..saladday/main
```

判断某个上游提交是否已被吸收时，按以下顺序核查：

1. `git merge-base --is-ancestor <upstream-commit> HEAD`，确认是否精确合入。
2. `git cherry -v HEAD saladday/main`，确认是否有 patch-id 等价 cherry-pick。
3. `git log --all -S '<关键符号或文案>'`，确认是否由本仓库提交语义吸收。
4. 对照相关文件的当前实现，区分“已吸收”和“部分覆盖”。
