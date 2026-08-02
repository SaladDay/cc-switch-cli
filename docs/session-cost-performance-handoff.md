# Sessions Cost 性能任务 Handoff

> 状态：**PAUSED / 暂停实现**
>
> 最后核对日期：2026-07-31
>
> 本文记录暂停时磁盘上的真实状态。它不是完成说明，也不代表当前改动已经可以合并。

## 1. 一句话结论

当前改动已经把 Sessions 的元数据发布与 Cost 索引拆开，因此不会再让完整
session 数量和最新日期等待 Cost 才出现；但手动刷新之后仍会冷扫描约 1.08 GB
的可见 session 正文来计算 lifetime Cost，实测约 63 秒，用户实际启动的 debug
进程更久。因此它仍然达不到“没有 Cost 时的速度等级”。

最重要的事实是：

> 在现有源格式和现有 Usage 保留策略下，“首次启动即精确显示所有历史
> session 的 lifetime Cost”与“完全不扫描历史正文、速度等同无 Cost”
> 不能同时保证。

建议恢复任务时保留 UI 与快速元数据路径，停止从 Sessions refresh 构建全量
Cost 索引；让 Sessions 成为现有 Usage 后端的有界只读投影，无法证明完整的旧
session 继续显示 `-`。不要继续通过增加线程数来优化当前冷扫描器。

## 2. 用户目标与验收标准

### 2.1 Sessions UI

- 左侧 session 列表整体放大，当前目标布局为左侧 55%、右侧 45%。
- 列表增加一列 `Cost`，表示单个 session 的 Cost。
- Overview 增加一行 Tokens 与 Cost。
- Tokens 优先复用首页格式：
  `In: 4.8k • Out: 1.9M • CR: 347.7M • CW: 12.2M`。
- 数据缺失、不完整或无法可信计算时显示 `-`，不能伪装成 `0` 或 `$0.000`。
- 窄终端必须可控降级，Cost 的可见性优先于 Time。

### 2.2 刷新语义

- Sessions 页面**不能自动刷新**。
- 进入页面只读取已经持久化的 manifest/page，不应自动遍历、stat 或重解析
  provider 源目录。
- 只有用户手动刷新才重建 session 元数据。
- 即使手动刷新，最终 session 数量和第一页元数据也不能被 Cost 计算阻塞。
- 用户最新提出的性能目标是：尽可能达到“没有 Cost 时”的速度等级。

### 2.3 架构要求

- “Usage 页面拥有完整能力，主页/其他页面只是读取其聚合结果的简化视图”
  是用户认可的方向；Sessions 也应优先成为 Usage 的子集/投影。
- 尽可能复用项目内现有 Usage import、去重、fresh-input 和定价语义。
- 后端逻辑尽可能与上游一致，不重新发明另一套 provider 计费规则。
- 如果问题在上游也存在，默认以上游行为为准，不额外扩大修复范围。
- 控制代码量、状态机数量和边界；可以拆文件，不要把一个大文件继续堆大。
- 不得改变现有功能或引入性能回退。

## 3. 工作区与 Git 状态

工作 worktree：

```text
/home/fanjingluo/dev/cc-switch-cli-session-cost
```

分支：

```text
codex/session-cost-overview
```

暂停时 HEAD：

```text
57a758e fix(usage): align official subscription queries
```

本地 `origin/main` 追踪引用比 HEAD 多两个后继提交：

```text
ea296a0 feat(settings): add Codex auth preservation CLI
e92c9cc feat(mcp): support authenticated remote servers in TUI (#383)
```

`HEAD` 是本地 `origin/main` 的祖先，尚未合并这两个提交。此处只描述暂停时本地
引用，没有在写本文时额外执行 fetch、merge、commit 或 push。

当前功能改动全部未提交：

- 31 个 tracked Rust 文件被修改；
- tracked diff 约为 `+1671/-141`；
- `src-tauri/src/services/session_metrics/` 是 untracked 目录，含 10 个 Rust
  文件、约 1857 行；
- 本 handoff 文档也是新增文件。

恢复前先运行：

```bash
cd /home/fanjingluo/dev/cc-switch-cli-session-cost
git status --short --branch
git log --oneline HEAD..origin/main
```

不要在 dirty worktree 上直接做未经检查的 merge，也不要用
`git reset --hard`/`git checkout --` 丢弃改动。

## 4. 问题经过

最初的 Sessions Cost 版本在 session 扫描过程中同步解析所有正文并写入一个
临时/派生 Usage 数据库，只有整个 Cost 阶段结束后才发布最终 manifest。这导致：

- UI 长时间停留在第一页的 100 行 provisional 数据；
- 用户看到的最新 session 停在 `2026/06/30`；
- 实际当前 session 没有及时显示；
- `Refreshing` 持续非常久。

当前 worktree 已经改为：

1. 进入 Sessions 时，若持久化 manifest 有效，只读第一页并立即结束 entry
   request；
2. 手动刷新先重建并发布完整元数据 manifest；
3. 元数据发布以后再开始独立的 Cost indexing continuation；
4. page load/search/locate 时，从派生 Cost DB 对当前页做 overlay。

这修复了“Cost 阻塞 session 数量与最新日期”的正确性问题，但没有消除 Cost
索引本身的时间。用户最新实际体验仍然是“很慢”。

## 5. 已确认的根因与实测数据

### 5.1 本地 Codex 历史规模

调查时的近似数据：

| 数据 | 规模 |
| --- | ---: |
| Codex 总历史 | 约 1,930 个文件 / 16.36 GB |
| 可见 root session 正文 | 约 653 个 / 1.08 GB |
| subagent 正文 | 约 1,266 个 / 15.23 GB |
| subagent 占总字节 | 约 93% |
| 正确发布的 Codex manifest | 654 rows / 7 pages |

元数据扫描只需要有界读取 head/tail；精确 Cost 则必须理解完整的 usage 记录、
累计 token、model 切换和 provider 特有结构。即使已经排除绝大多数 subagent
正文，可见 root session 仍约 1.08 GB。

### 5.2 性能

- 当前派生 Cost DB 的冷构建基准约 63 秒。
- 用户启动的 `target/debug/cc-switch` 在交接核对时仍运行：
  PID `3355568`，运行约 622 秒，CPU 约 45.6%。
- 这是用户主动启动的进程，不是待清理的测试进程；**不要擅自 kill**。
- 进程状态是瞬时信息，恢复任务时应重新用 `ps` 核对。

### 5.3 用户运行后产生的派生缓存快照

只读诊断时曾观察到：

- `~/.cc-switch/session-metrics-cache-v1.db` 约 53.7 MB；
- 542 个 summary（Claude 48、Codex 494）；
- 96,173 条派生 request log；
- 921 条 source mapping。

这些文件是用户运行当前构建后产生的主机状态。不要删除、改写或拿它做写入型
测试。即使后续移除该架构，也必须把磁盘文件当作用户数据，除非用户明确要求
清理。

## 6. 为什么不能直接从现有 Usage 得到全部 lifetime Cost

主数据库的 Usage 明细表是 `proxy_request_logs`。本地代码中
`USAGE_ROLLUP_RETAIN_DAYS = 30`；更早的明细会被聚合进
`usage_daily_rollups` 后删除。

rollup 保留 date/app/provider/model/token/cost 等聚合维度，但不保留
`session_id`。所以：

- 30 天内的原始 Usage 明细可以按 session 快速聚合；
- 30 天前已经 rollup 的数据无法再准确反推出每个 session 的 lifetime Cost；
- 用旧 session 当前仍残留的少量明细直接求和，会得到“看似精确、实际不完整”
  的 Cost，不能这样展示。

暂停前的只读统计：

| Provider | raw logs | distinct session IDs |
| --- | ---: | ---: |
| Claude | 12,560 | 51 |
| Codex | 103,806 | 1,124 |

把当前 Codex manifest 的 654 个 session ID 与主 Usage 明细比较：

- 全部 654 个里只有 89 个有可用明细覆盖；
- 第一页 100 个里有 86 个被覆盖；
- 被覆盖的 89 个都存在非零 Cost。

这说明主 Usage DB 非常适合即时显示近期 session，但不能无条件替代所有历史
lifetime Cost。

## 7. 当前未提交实现

### 7.1 UI 与数据模型

相关文件：

- `src-tauri/src/cli/tui/ui/sessions.rs`
- `src-tauri/src/cli/tui/ui/usage.rs`
- `src-tauri/src/cli/tui/ui/home_chart.rs`
- `src-tauri/src/cli/i18n.rs`
- `src-tauri/src/session_manager/mod.rs`

当前行为：

- Sessions 布局为 55/45；
- 列表有 `Title / Time / Cost`，较窄时隐藏 Time；
- Overview 增加 token breakdown + Cost 一行；
- token compact formatter 已从 Usage 抽成可复用 helper；
- `SessionMeta` 新增 runtime-only `usage`；
- 新 manifest 不序列化 `usage`，避免把易过期 Cost 固化进元数据；
- 老 manifest 中存在的 legacy usage 仍可反序列化；
- 无数据时 UI 显示 `-`。

这些 UI/模型改动大体可以保留，和后端 Cost 来源并不强耦合。

### 7.2 快速元数据发布

主要文件：

- `src-tauri/src/cli/tui/runtime_systems/workers.rs`
- `src-tauri/src/cli/tui/runtime_systems/handlers.rs`
- `src-tauri/src/cli/tui/app/types.rs`

当前行为：

- 普通进入页面时，只要已有有效 persisted manifest，就读取一页、设置 complete
  并返回，不做 source revalidation；
- 手动刷新时先 `ManifestPublished`，随后才启动 Cost indexing；
- session page/search/locate 都在读取一页后 overlay Cost；
- 删除 session 后异步 invalidates 派生 Cost cache。

“有效 manifest 普通打开必须是固定成本”是值得保留的核心不变量。

### 7.3 派生 lifetime metrics 子系统

目录：

```text
src-tauri/src/services/session_metrics/
├── build_lock.rs
├── cache_db.rs
├── hermes.rs
├── importers.rs
├── mod.rs
├── openclaw.rs
├── overlay.rs
├── source_map.rs
├── store.rs
└── tests.rs
```

文件已按职责拆分，最大文件约 272 行，满足“不要堆成一个大文件”的形式要求。

它目前实现：

- `session-metrics-cache-v1.db`：派生 request ledger、summary、source map 和
  OpenClaw file cache；
- `session-metrics-resume-v1.db`：增量读取提示；
- process mutex + file lock，避免多个构建者并行写；
- 复用 Claude/Codex/Gemini/OpenCode 的 Usage importer；
- Hermes/OpenClaw 专用 adapter；
- 每次手动刷新收集 manifest 中全部 target，分批导入并汇报 Cost progress；
- 当前页只读 overlay；
- sidecar 文件权限、symlink/hardlink 防护。

为了支持该子系统，当前 diff 还大幅修改了：

- `src-tauri/src/services/session_usage*.rs`
- `src-tauri/src/services/session_usage_driver.rs`
- `src-tauri/src/database/mod.rs`
- `src-tauri/src/database/tests.rs`

这些修改主要增加“指定 sources/IDs 导入”“取消”“派生 DB opener”和增量状态
支持。若采用推荐的 Usage 只读投影方案，它们中的大部分会变成无调用价值的
复杂度，应删除，而不是留作“以后也许会用”。

### 7.4 当前运行链路

```text
进入 Sessions
  -> 读取 persisted metadata page
  -> 从派生 metrics DB overlay
  -> 完成，不 revalidate source

手动刷新
  -> 重建 metadata manifest
  -> 立即发布完整 metadata
  -> 遍历整个 manifest
  -> 解析 session 正文并构建派生 lifetime metrics DB
  -> 持续发送 Indexing cost progress
```

最后三步正是用户仍感到慢的原因。

## 8. 推荐的恢复方向

### 8.1 总体选择

不建议丢弃整个 UI/manifest 改动，也不建议继续优化当前全量 Cost builder。

建议：

- **保留** Sessions UI、`SessionMeta.usage` runtime overlay 模型、有效 manifest
  固定成本打开、手动刷新和 metadata-first publication；
- **重做** Cost 数据来源；
- **删除** Sessions refresh 触发的全量 Cost indexing、其 progress 状态机以及
  仅为它增加的 importer/derived-DB 复杂度。

### 8.2 推荐数据流

```text
进入或翻页
  -> 读取最多 100 行 metadata page
  -> 只读查询现有 Usage backend 中这些 session IDs
  -> 使用与 Usage 相同的去重/fresh-input/定价语义
  -> 能证明完整则显示 Tokens/Cost，否则显示 -

手动刷新
  -> 只重建 metadata
  -> 发布后立即结束 refresh
```

实现约束：

1. 查询必须只针对当前 page（当前 hard cap 为 100 rows），不能全表 materialize。
2. 按 provider 分组，用 bounded `IN (...)` 或临时只读批次查询。
3. 复用：
   - `effective_usage_log_filter("l")`
   - `fresh_input_sql("l")`
   - Usage 的 token bucket 与 cost 聚合语义。
4. Sessions page 不触发 Usage sync，不新增 10 秒或其他自动刷新链路。
5. 主 Usage DB 不存在、schema 不兼容、busy 或查询失败时，优雅显示 `-`。
6. 不要把不完整的旧明细显示成 lifetime Cost。

### 8.3 唯一仍需明确实现的边界：完整性判定

最保守、最诚实的基线：

- session 明确完全落在 raw detail retention window 内，且对应 Usage import 已
  完成时，才把主 Usage 聚合结果作为 lifetime 值；
- 30 天前的 session 没有其他“完整”标记时显示 `-`；
- 同步中、部分解析或无法证明完整时显示 `-`。

“如何以较小代码量确认 importer 已完整同步当前 session”尚未最终实现。恢复
时应先围绕这一点写测试，不要直接把任何 `SUM(...)` 当作 lifetime。

可接受的 UX 取舍是“近期多数 session 立即有 Cost，老 session 为 `-`”；
不可接受的是“所有行都有一个可能低估的精确数字”。

### 8.4 不推荐作为本次默认方案的选择

#### 继续给冷扫描加并发

- 只能缩短 1.08 GB parse 的时间，不能达到无 Cost 的固定成本；
- 会增加磁盘争用、内存、取消和跨 provider 边界；
- 在 HDD、杀毒软件、低核机器上的尾延迟更难控制。

#### 在 Usage rollup 中增加 session_id 维度

- 会改变主数据库 schema/rollup 语义；
- 与“后端尽量和上游一致”冲突；
- 会放大数据量和迁移风险。

#### 新增长期 session ledger sidecar

它可以在 Usage 明细被 prune 前保留 session 维度，为未来 session 提供精确
lifetime Cost，但：

- 无法恢复已经丢失 session_id 的历史 rollup；
- 新增写入、迁移、一致性和清理边界；
- 是额外产品能力，不是本次“低复杂度、立即提速”的首选。

如果未来产品明确要求“所有历史 session 都必须精确”，可以把它作为独立设计，
或提供一个用户显式触发的历史 Cost build 命令；不要绑在 Sessions refresh 上。

## 9. 建议保留/删除矩阵

| 区域 | 建议 |
| --- | --- |
| 55/45 布局、Cost 列、Overview token/cost 行 | 保留 |
| Usage/Home 共用 token formatter | 保留 |
| `SessionMeta.usage` runtime-only，不写 manifest | 保留 |
| 有效 persisted manifest 普通进入不 revalidate | 保留 |
| metadata 在任何 Cost 工作之前发布 | 保留 |
| page/search/locate 的 bounded overlay hook | 保留接口，替换实现 |
| `MetricsProgress` / `MetricsFinished` UI 状态 | 删除 |
| 手动 refresh 后 `index_manifest` | 删除 |
| `session-metrics-cache-v1.db` 新构建链路 | 删除或独立另案 |
| `session-metrics-resume-v1.db` | 若无 builder，删除代码引用 |
| 为 scoped Cost build 改造的 Usage importers | 逐项回退 |
| OpenClaw/Hermes 独立 lifetime parser | 若不再构建历史 Cost，删除 |
| 派生 DB 专用 opener/locks/source map | 若无 writer，删除 |

逐项回退时必须用 diff 验证，不要覆盖本分支基线中与本任务无关的用户改动。

## 10. 已知正确性边界

### 10.1 Token 语义

UI 的四个 bucket 为：

- `In`
- `Out`
- `CR`（cache read）
- `CW`（cache creation/write）

总 token 需要与 Usage 的 fresh-input 语义保持一致，不能把已经包含 cached token
的输入再次相加。不要在 Sessions 新写一套 provider-specific 公式。

### 10.2 Cost 为零与 unavailable

- 真正完整且明确为零的 Cost 可以显示 `$0.000`；
- 没有 assistant usage、缺少 cost 字段、解析不完整、记录过大被跳过或存在坏
  JSON 时应为 unavailable；
- 多来源映射冲突不能随意任选一个值。

### 10.3 OpenClaw 盲审发现

首轮两位独立盲审都指出同一问题：OpenClaw 的 user-only/aborted transcript 或
缺少 `cost.total` 的 assistant usage 会被显示成精确零值。

当前 worktree 已包含本地修复：

- `src-tauri/src/services/session_metrics/openclaw.rs` 跟踪
  `usage_seen`、`cost_complete`、malformed/oversized 状态；
- 无法证明完整时返回 `None`；
- `src-tauri/src/services/session_metrics/tests.rs` 增加
  `openclaw_without_complete_usage_is_unavailable`。

但修复后**尚未完成一轮新的独立盲审**。如果后续删除整套历史 parser，该 finding
会随代码消失；否则仍需按 AGENTS.md 重新审查。

## 11. 测试与检查状态

以下是在当前架构开发过程中已经跑过并通过的检查：

```text
cargo check
cargo test --lib session_metrics
cargo test --lib legacy_usage_summary_without_breakdown_remains_readable
cargo test --lib session_usage -- --nocapture
cargo test --lib sessions -- --nocapture
```

当时结果：

- `session_metrics`：3 passed；
- `session_usage`：102 passed，1 ignored；
- `sessions`：109 passed。

Lint 状态：

- 默认 `cargo clippy` 在既有
  `home_chart.rs` 的 `BAR_AXIS_INSET_ROWS = 0` +
  `for _ in 0..BAR_AXIS_INSET_ROWS` 上触发
  `clippy::reversed_empty_ranges`；
- 该问题在本分支 HEAD 的原有代码中同样存在，不属于 Sessions Cost 修复；
- `cargo clippy -- --allow clippy::reversed_empty_ranges` 曾通过，仅剩 warning。

完整 `cargo test --quiet` 没有得到绿色结果，因为两个现有 integration target
在编译阶段失败：

- `settings_current_provider`
- `settings_visible_apps`

当时错误包括 unresolved `crate::test_support` 和 test-local `AppError` 缺少
`localized`。这些与本任务无关，在暂停时的 HEAD 上也存在。恢复后应先检查
`origin/main` 的两个新提交是否改变了测试基线，不要把无关修复混入本 PR。

注意：

- 在最终重构后必须重新运行 `cargo fmt --check`、目标测试和可运行的全套检查；
- 现有通过记录不能替代重构后的验证；
- 所有 Cargo 命令都从 `src-tauri/` 执行；
- 写 app 配置的测试必须使用 isolated home/config。

## 12. 盲审状态

已完成的两个 reviewer：

- `/root/session_cost_blind_review_a`
- `/root/session_cost_blind_review_b`

它们属于同一首轮，均发现 OpenClaw unavailable 被错误显示为零。当前代码已针对
该 finding 修改，所以这两个结果不能作为“修复后通过”的证明。

恢复并完成实现及本地验证后，需要遵循 AGENTS.md：

1. 启动两名新的独立 reviewer；
2. 只提供用户目标、预期行为、验收标准与边界；
3. 不告知实现方式、既有 finding 或另一 reviewer 的意见；
4. 验证 finding，修复后再进行新一轮盲审；
5. 不要在 PR 描述中写“两名独立盲审均为 No findings”。

暂停期间不继续启动 reviewer。

## 13. 外部与项目内参考资料

以下是本任务实际查阅或用于设计对照的资料。外部项目的 `main` 会变化；若以后
需要做源码级行为对齐，应先固定到具体 commit，而不是只引用 README。

### 13.1 Tokscale

- 项目与 README：
  <https://github.com/junhoyeo/tokscale>

参考点：

- 多 provider session usage/cost 的展示方式与数据路径；
- native Rust core、并行扫描和 SIMD JSON parsing；
- 可再生成的 TUI data cache、source-message cache 与 lock file；
- `autoRefreshEnabled` 默认 `false`；
- minutely aggregation 默认关闭，因为大历史上有非平凡成本；
- 默认 native timeout 为 5 分钟。

得到的结论：

- Tokscale 证明“缓存 + 显式刷新 + 可选昂贵聚合”是成熟方向；
- 它的并行全文扫描不能证明“无缓存时精确扫描 1 GB 也等同 metadata-only”；
- 因此可以借鉴 UI、cache ownership 和显式刷新，不能把“加并发”当作本任务的
  最终性能保证。

### 13.2 ccboard

- 项目与 README：
  <https://github.com/florianbruniaux/ccboard>

参考点：

- Sessions/Costs 分离展示；
- `r` 为显式 refresh；
- heavy activity 分析采用有界的 4-way batch scan。

得到的结论：

- 重分析适合用户显式触发、并限制并发；
- Sessions 的普通浏览不应隐式拥有一个无限期后台历史分析任务。

### 13.3 Ratatui

- 官方 crate 文档：
  <https://docs.rs/ratatui/latest/ratatui/>
- 官方仓库：
  <https://github.com/ratatui/ratatui>

参考点：

- immediate-mode render/event-loop 模型；
- UI state 应消费已经完成的消息/快照；
- 长时间文件解析应留在 worker，不应进入 render 或 key handling 热路径。

本项目已经有 session workers，所以本任务不需要再引入另一套异步 runtime。

### 13.4 SQLite 官方文档

- Isolation：
  <https://www.sqlite.org/isolation.html>
- Write-Ahead Logging：
  <https://www.sqlite.org/wal.html>
- File locking/concurrency：
  <https://www.sqlite.org/lockingv3.html>

参考点：

- WAL 下 reader 可以看到稳定的已提交 snapshot；
- 有界只读 Usage projection 可以与既有 writer 共存；
- 仍需设置短 busy timeout 并在 busy/schema mismatch 时优雅降级；
- 不应让 Sessions page 成为第二个长期 writer。

### 13.5 项目内已有设计文档

- [Sessions & Usage Scan Performance Plan](perf-session-usage-scan-plan.md)
- [TUI Blocking Performance Risks](tui-blocking-performance-risks.md)

第一份文档描述了更早一轮对 Usage import、session scan cache 和增量 resume 的
基准与方案。它是背景材料，不等于当前 Sessions lifetime Cost 的最终方案。

### 13.6 没有依赖的“外部规范”

Claude/Codex/Gemini/OpenCode/Hermes/OpenClaw 的本地 session 文件没有在本任务
中找到并采用一套统一、稳定的官方 schema 规范。当前解析语义来自：

- 项目已有 `session_usage*` importer；
- provider parser；
- 本地 fixture/test；
- 对真实数据的只读诊断。

因此不要仅凭第三方 README 改 provider 计费语义；应继续以上游项目代码和本地
测试为准。

## 14. 安全与范围边界

- **永远不要修改主机** `$CC_SWITCH_CONFIG_DIR`。
- **永远不要修改主机** `$CLAUDE_CONFIG_DIR`。
- **永远不要修改主机** `$CODEX_HOME`。
- 读取真实历史用于诊断时保持只读；写入型测试必须使用 temp dir 和
  `TestEnvGuard`/test support。
- 不要删除用户运行当前 build 后创建的 metrics sidecar。
- 不要 kill 用户主动启动的 cc-switch 进程。
- Sessions 不增加自动刷新。
- 不改变主 `cc-switch.db` schema，不为了本任务 bump schema version。
- 不修改 Usage 的去重、fresh-input、pricing、rollup 语义。
- 不把部分 Cost 当作完整 lifetime Cost。
- 不在本任务顺手修上游共有的 clippy/integration-test 问题。
- 暂停期间不 commit、不 merge、不 push、不开 PR。

## 15. 推荐恢复顺序

1. 重新核对 `git status`、用户进程和本地 `origin/main`。
2. 先写“Usage page projection 完整性”的单元测试：
   - 当前页最多 100 IDs；
   - 近期完整 session 正确聚合四个 token bucket 与 Cost；
   - 旧/部分 session 为 `None`；
   - DB 不存在、busy、future schema 时为 `None`；
   - 不触发写入或 Usage sync。
3. 用最小只读 query 实现 overlay，并复用 Usage SQL helpers。
4. 删除 `index_manifest`、metrics progress state/message/i18n。
5. 删除没有调用者的 derived DB/importer/build-lock/source-map 代码。
6. 保留 metadata-only 手动 refresh，验证进入页面不 walk/stat sources。
7. 在隔离目录跑 fmt/check/目标测试；再核对全套测试的基线失败。
8. 做性能验收：
   - 有效 manifest 打开只读一页；
   - 翻页只增加一个 bounded Usage query；
   - 手动 refresh 的 terminal state 在 metadata publish 后结束；
   - 不再出现 minutes-long `Indexing cost`。
9. 更新/合并 `origin/main` 时逐文件处理冲突，尤其是 settings/TUI 相关新提交。
10. 完成两名新的独立盲审，修复确认问题后再决定 commit/PR。

## 16. 暂停点

本文写入后，本任务应保持暂停。当前没有：

- 最终性能方案的代码实现；
- 修复后完整测试结论；
- 修复后的两名独立盲审；
- commit；
- merge；
- push；
- PR。

恢复任务时，应从第 8 节的数据路径决策继续，而不是默认沿当前派生 Cost builder
继续优化。
