# Sessions Cost v3 实施计划（最终语义）

> 状态：**产品语义已按 2026-07-31 后续讨论收敛；实现与验收以本文当前版本为准。**
>
> 定稿日期：2026-08-01
>
> 本文取代 `session-cost-performance-handoff.md` 中的"推荐恢复方向"（§8）。
> 旧文档仍是背景与现状基线的权威记录；两者冲突处**以本文为准**。
> 后续产品讨论又明确取代了本计划早期版本中的两项设计：
> 1. 删除 `Complete / Partial / Unknown` 覆盖证明与 `≥` / `~` 前缀，改为
>    "可靠的本地估算直接显示金额，否则显示 `-`"；
> 2. 删除为覆盖证明引入的 prune 高水位与时间来源字段，不新增数据库写入。
> 手动刷新允许一次性增量 Usage sync 的决定保持不变。

## 0. 一句话目标

**有没有 Cost 列，Sessions 页的打开/翻页/刷新速度都必须在同一量级。**
实现手段：Cost 永远是"当前页（≤100 行）的异步只读投影"；全量 Cost 索引
（当前未提交实现中的 Phase B / `session_metrics::index_manifest`）整体删除。

## 1. 已锁定的产品决策（用户拍板，不得改动）

1. **两态显示**：能从本地证据得到可靠估算时直接显示 `$1.23`；否则显示
   `-`。Cost 列名不变，表格与 Overview 都不添加 `Estimated`、`≥` 或 `~`。
   "估算、不是账单"只放在上下文 `?` 帮助中。
2. **不做周期性自动补数字**：60 秒周期 sync 完成后不自动重查费用；费用只在
   进页、翻页、搜索、locate、手动刷新时更新。
3. **不证明历史完整性**：产品不需要账单级精确金额，也不引入覆盖度状态、
   prune 高水位或第二套时间证据链。不得改主数据库 schema；Cost 投影全程只读。
4. **手动刷新顺带同步**：按 `r` 后列表照常秒出（metadata 发布不被阻塞）；后台
   触发一次增量 usage sync（实测 ~1.7s）；sync 到达任意终态后（Ok/Err 都可能
   已改库）对当前可见页自动重发一次 Cost 查询。一次性行为，无周期链路。

## 2. 语义模型

### 2.1 数值定义（精确措辞，写进 `?` 帮助）

显示值 = **根据当前本地仍可用、经 effective filter 去重、且能通过确定性 ID
归属到该 session 的 usage 记录与模型定价得到的尽力估算**。它不是端到端账单，
也不代表"历史上曾发生的全部费用"。

### 2.2 数据结构

`SessionUsageSummary`（`session_manager/mod.rs`）重构为：

- token 四桶（input / output / cache_read / cache_creation）
- `estimated_cost_usd: Option<f64>`（不得用 0 / NaN / 负数当哨兵）

`SessionMeta.usage` 保持 `#[serde(skip_serializing)]` runtime-only，不写入
manifest。

### 2.3 渲染规则

| 条件 | Cost 显示 | Tokens 显示 |
| --- | --- | --- |
| 存在 token-bearing 归属行，且每一行都能可靠计价 | `$1.23` | 四桶本地统计 |
| Hermes `sessions` 行有 token，且源字段 `estimated_cost_usd` 有效 | `$1.23` | 同一行的四桶统计 |
| 无 token-bearing 归属行 / 任一行无法可靠计价 / ID 歧义 / 查询失败 | `-` | 有可用 token 则显示，否则 `-` |

- **unpriced 判定必须逐行**。`pricing_model` 是写入时的计价证据：
  `NULL` 表示历史行、空串表示明确未计价、非空表示写入时已成功计价。正 token
  行只有在成本文本有效、有限、非负，且满足"非空 `pricing_model`"或"历史
  `NULL` 但成本严格大于零"时才可信；否则该 session 的 Cost 为 `-`。非空
  计价证据下的零费用（包括免费模型、零倍率、实际 token 桶零价）可显示
  `$0.00`。读侧不得用当前 `model_pricing` 重新解释历史记录。零 token 的错误行
  **不得**毒化 session，也不得贡献费用。
- **重复身份歧义**：manifest 身份是 `(provider_id, session_id, source_path)`
  （`paged_manifest.rs:2435`），Usage DB 只有 `(app_type, session_id)`。同一
  session_id 出现多个不同 source_path 的可见行时，所有这些行显示 `-`，
  不得默默把同一小计展示成每个文件各自的费用。
- `?` 帮助必须明确这是本地尽力估算、不是账单，并写明 **Codex 根会话费用
  不含独立 subagent 线程的费用**（见 §4.2）。

## 3. 运行时架构（异步只读投影）

### 3.1 消息协议

删除同步 `enrich_rows` 语义（实测第一页命中 48,064 行明细、聚合 ~200ms，
不允许出现在页加载路径上）。改为：

```text
页加载完成 → 立即发送 PageLoaded（usage 全为 None，UI 先显示 -）
           → 发送 CostOverlayRequest {
                 cost_seq,                    // 独立自增序号
                 page_token,                  // 完整 SessionPageToken（app/types.rs:944：
                                              //   scope_epoch / view_epoch / source / scope / generation）
                 page_index,
                 row_identities,              // (provider_id, session_id, source_path) 列表
             }
Cost worker → CostOverlayResult {
                 cost_seq, page_token, page_index,
                 overlays: identity → SessionUsageSummary 映射   // 不按数组位置回填
             }
handler 校验：cost_seq == active_cost_seq ∧ page_token 逐字段一致
             ∧ page 仍可见 ∧ 逐行 identity 仍匹配，全过才回填。
`scan_seq` 不参与 Cost 协议。
```

请求触发点 = 原 5 个 overlay 调用位置（页加载 / 搜索首页 / 缓存打开首页 /
重建后首页 / locate 结果页，`workers.rs:1212/1915/1970/2090/2143` 附近），
外加手动刷新的 sync 终态一次性重发（§5）。

### 3.2 Cost worker（单槽 latest-wins）

- 独立单线程 worker，收任务时 `recv_latest` 合并积压请求；
- 共享 atomic `active_cost_seq`：发送新请求即更新；SQLite progress handler
  内检查 `active_cost_seq != my_seq || now >= deadline` 即中断——**仅靠
  recv_latest 不够，必须能打断正在执行的旧 SQL**（否则快速翻页时新页要等
  旧页最多 2 秒）；
- 执行 deadline 2s（正常 ~200ms 的 10 倍护栏）；busy_timeout 250ms（只管
  等锁，不管执行时长）；
- 主库连接用现成 `Database::open_readonly_current_schema()`
  （`database/mod.rs:738`——不建目录、不迁移、不 seed、不跑启动维护，并自带
  future-schema 拒绝），**严禁走 `Database::init()`**（init 会触发
  maintenance/prune，是写路径）；
- 当前页聚合的所有 SELECT 在**同一个只读事务快照**内完成，事务保持短促
  （查完即结束，不跨请求持有）。

### 3.3 主库查询（Claude / Codex / Gemini / OpenCode）

- 单条 CTE：`WITH wanted(app_type, session_id) AS (VALUES ...) ... GROUP BY
  app_type, session_id`，四 provider 一次查询、共享同一快照；
- 复用既有 SQL 片段：`effective_usage_log_filter("l")`（usage_stats.rs:228）、
  `fresh_input_sql("l")`（sql_helpers.rs:63）、Usage 页的 token 桶与
  `SUM(CAST(total_cost_usd AS REAL))` 语义——与 Usage 页一致性靠共享代码
  保证，不新写计费规则；
- 费用可信度只消费 `proxy_request_logs.pricing_model` 的既有三态证据，不查询
  `model_pricing`，不在读侧重新做别名规范化、前缀匹配或历史重计价。Claude /
  Codex / Gemini / OpenCode 的既有 importer，以及既有成本 backfill，在成功
  取得本地定价或可信上游费用时写入非空 `pricing_model`，失败时写空串；
  不新增表、列、迁移或 Sessions 专用写路径；
- `total_cost_usd` 只接受 CC Switch writer 会产生的保守十进制文本语法；
  SQLite 会把非法文本静默 `CAST` 为零，因此不符合语法的 token 行必须失败
  关闭为 `-`；
- **Codex 双 ID 合并**：manifest ID `U` 同时查 `U` 与 `codex_U`
  （代理侧前缀见 `proxy/session.rs:68,83`）并合并到 `U`；Generated 随机 ID
  无法映射，天然不属于该小计；
- 实现后必须跑 `EXPLAIN QUERY PLAN` 确认仍由 `idx_request_logs_session`
  驱动（改成 VALUES+JOIN 后不得退化为扫日志表再连接），并重跑 top-100
  基准（当前基线：48,064 行 / ~200ms）。

### 3.4 其他 provider

- **Hermes**：只读其 `state.db` 的会话级 `sessions` 表，`IN (wanted)` 必须
  下推进 SQL，busy_timeout 降到 ~250ms；token 与费用必须来自同一份
  `sessions` 聚合，仅接受其源字段 `estimated_cost_usd`。不读取或融合
  `session_model_usage`（它是按模型/任务拆分的归属明细，不是第二份会话总计）；
  `sessions` 缺表、缺少 token 字段、无 token，或 token-bearing 行缺少有效
  `estimated_cost_usd` 时显示 `-`。
- **OpenCode**：费用一律来自主库 CTE（**不得**从 OpenCode 自身 DB 重新计价，
  那是第二套规则）。
- **OpenClaw**：v1 显示 `-`（主 Usage 后端不导入它；解析正文正是本方案要
  消灭的工作类型）。
- 任何失败（DB 不存在 / busy / future schema / 表缺失）→ `log::debug!` +
  该行 None → `-`。整条链路零写入。

## 4. 可用性判定（两态）

判定只回答"当前本地证据能否给出一个不会隐瞒未计价 token 行的估算"，不回答
"历史是否完整"。缺失、归档或尚未同步的 usage 不会被猜测，也不会通过前缀
伪装成可量化的下界。

### 4.1 可靠估算的必要条件

1. 身份 `(provider_id, session_id, source_path)` 在当前可见页没有歧义；
2. effective filter 后至少存在一行可归属的 token-bearing usage；
3. 每一条 token-bearing 行都有写入时计价证据，或是金额严格大于零的历史
   `pricing_model IS NULL` 兼容行；零 token 错误行不参与该判断；
4. 聚合结果是有限且非负的数。`None`、NaN、无穷或负数都在边界处失败关闭为
   `-`。

不读取 `settings` 水位、`session_log_sync` 新鲜度或 created_at 来源，不把
mtime 与 payload 时间戳跨域比较。

### 4.2 provider 规则与必须钉死的语义

| Provider | 费用来源 | 特殊规则 |
| --- | --- | --- |
| Claude | 主库有效 usage 行 | 已导入且归属到根 ID 的 subagent 行按既有 Usage 语义聚合 |
| Codex | 主库有效 usage 行 | 合并 manifest ID `U` 与代理 ID `codex_U`；独立 child thread 不吸收到根会话 |
| Gemini | 主库有效 usage 行 | 沿用 Usage 页 token 桶与费用字段语义 |
| OpenCode | 主库有效 usage 行 | 不从 OpenCode 自身 DB 建第二套计价规则 |
| Hermes | `state.db.sessions` 的 `estimated_cost_usd` | 仅使用 `sessions` 会话总计；不与 `session_model_usage` 融合；token-bearing 行为 NULL、负值或列不存在时 Cost 为 `-` |
| OpenClaw | 无 | 恒为 `-` |

测试必须覆盖：Codex 双 ID 合并且不吸收 child；Claude 既有 subagent 聚合；
重复 source_path 歧义；任一 token-bearing 未计价行使 Cost 为 `-`；零 token
错误行不毒化；带写入证据的别名/真零价显示 `$0.00`；非法成本文本失败关闭；
历史 `NULL` 正金额兼容、零金额不猜测；Hermes 双表并存或冲突时只采用
`sessions` 的完整会话总计。

## 5. 手动刷新流程（决策 ④ 的实现形状）

```text
按 r（force=true）
  → Phase A 元数据重建（沿用既有 head/tail 有界读与缓存一致性检查；
    不得为 Cost 新增 stat 轮次）
  → ManifestPublished（终态；metadata 发布永远不等任何 Cost/sync 工作）
  → 立即对第一页发 CostOverlayRequest（显示当前库里已有的数字）
  → 同时向既有 usage sync worker 队列投递一次增量 sync 请求
    （复用现有单线程 worker 与请求合并机制，不新建线程/定时器）
  → sync 终态消息（Ok 或 Err 都可能已部分提交，SessionUsageSyncMsg::Finished
    只有 Result<(),String>，workers.rs:2558 会把部分成功折叠成 Err）
  → 若 Sessions 页仍可见：对当前可见页重发一次 CostOverlayRequest（仅一次）
```

60 秒周期 sync 的终态**不**触发重查（决策 ②）。

## 6. 写入边界（最终决定：Cost 不新增写入）

### 6.1 manifest

- `SessionMeta.usage` 继续 `#[serde(skip)]`，费用与 token overlay 只存在内存中；
- `source_mtime_ns` 可保留为扫描缓存一致性证据，但 Cost 投影不得读取它；
- 删除仅为费用覆盖证明存在的 `created_at_kind`；
- **禁止 bump manifest format_version**，避免普通进入 Sessions 时触发自动全量
  重建。

### 6.2 主数据库

- 不创建或更新 `usage_prune_high_watermark`，不在 `settings` 写 Cost 专用状态；
- 不改 `rollup_and_prune` 的输入、输出、事务或 WebDAV 本地设置白名单；
- `session_log_sync` 仍是设备本地的权威文件游标，必须继续从 WebDAV 导出中
  排除并在导入时保留本地值。"Cost 不读取该表"绝不意味着可以导入远端游标；
- 既有 usage importer / cost backfill 只补齐现有 `pricing_model` 证据列，不改变
  金额算法，不属于 Cost 投影新增写入；
- 主库与 Hermes 投影都使用只读连接 / query-only 事务。手动刷新触发的增量
  Usage sync 是既有 importer 写入路径，不属于 Cost 投影新增写入。

## 7. 删除清单 / 保留清单

### 删除（相对当前 dirty worktree）

- `services/session_metrics/` 整目录（先把 hermes.rs 的只读聚合逻辑按 §3.4
  改造移植，openclaw.rs 一并删除）；
- `index_manifest` 及手动刷新后的整个 Phase B 调用链（workers.rs:2099-2124）；
- `MetricsProgress`/`MetricsFinished` 全套：runtime_systems/types.rs:212-223、
  workers.rs 两处发送（:2108/:2119）、handlers.rs 两个分支（:593-617）、
  app/types.rs 四个字段两个方法与 reset（:1166-1169/:3612-3670/:3596-3599）、
  ui/sessions.rs 两处（:43/:51-52）、i18n.rs 两条（:9425/:9844）、相关测试；
- 为 scoped import 加的 5 个 `sync_*_sources`/`sync_opencode_session_ids`
  入口及其私有重构（session_usage.rs / _codex / _gemini / _opencode，
  合计约 +889/−87）；
- `Database::derived_cache_at`（database/mod.rs:800 附近）；
- `scan_jsonl_incremental` 的 `is_cancelled` 第 7 参回退（含其测试）。
- `SessionCostCoverage` / `SessionCostKind` / `SessionCreatedAtKind`、
  `SessionCostTarget` 及全部覆盖度渲染和证明逻辑；
- `usage_prune_watermark` 模块、初始化 / prune / WebDAV 集成及其测试；
- Cost 查询对 `settings`、`session_log_sync` 与 manifest 时间证据的依赖。
  此处只删除查询依赖，不删除 `session_log_sync` 的 WebDAV 设备本地保护。
- 逐项回退必须用 diff 核对，不得覆盖分支基线中与本任务无关的改动。

### 保留

- 55/45 布局、Cost 列、Overview token/cost 行、共用 token formatter；
- `SessionMeta.usage` runtime-only（skip_serializing）；
- `source_mtime_ns` 仅作为扫描缓存自身的一致性证据；
- 有效 manifest 固定成本打开（无源 revalidation）；metadata-first 发布；
- 5 个 overlay 触发位置（改为异步请求）；
- 磁盘上用户已生成的 `session-metrics-cache-v1.db` / `session-metrics-resume-v1.db`
  是用户数据：代码不再打开即可，**不得删除文件**。

## 8. 性能要求与回退防线（硬性验收）

性能是本任务的核心关注点，分两面：新功能自身要快，且不得拖累任何既有功能。

### 8.1 新功能自身

| 路径 | 要求 |
| --- | --- |
| 有效 manifest 普通进入 | 只读 1 个 page 文件 + 异步发一次 Cost 请求；PageLoaded 不等 Cost；无源目录 walk/stat（用测试断言代码路径） |
| 翻页 / 搜索 / locate | 同上，每次至多一个在途 Cost 查询（latest-wins 吞并旧请求） |
| Cost 查询 | 后台 ~200ms 级（top-100 基准复测）；2s deadline；可被新请求即时打断 |
| 手动刷新 | metadata 发布时间与"无 Cost 版本"相同（Phase A 不新增 stat 轮次）；后台增量 sync ~1.7s 级；终态后一次重查；全程不再出现 minutes 级 `Indexing cost` |

### 8.2 不得拖累别的功能（逐项防线）

1. **主库争用**：overlay 只读事务必须短促（单批查询即结束）；不长期持有
   snapshot（长读事务会推迟 WAL checkpoint、放大 WAL）；busy 250ms 超时即
   优雅 `-`，不重试风暴。与 60s sync 写事务、proxy usage logger 并存时不得
   造成写侧可感知延迟。
2. **usage sync worker**：刷新触发的一次性 sync 走既有队列与合并机制，
   与周期 sync 自然去重；不得新增线程、定时器或在 TUI 启动/进页时加同步工作。
3. **UI 线程**：渲染与按键路径零 SQL、零文件 IO（参照
   docs/tui-blocking-performance-risks.md 的既有纪律）。
4. **proxy / prune 热路径**：零改动；Cost 不新增 per-request 或 maintenance
   写入。
5. **Phase A**：Cost 不新增 stat；`source_mtime_ns` 继续复用扫描缓存既有证据。
6. **manifest 体积**：usage overlay 不序列化；保留字段仍须满足 8MiB page
   上限与 64KiB 行上限。
7. **启动路径**：AppState/TUI 启动零新增工作；删除 Phase B 后总体是净改善
   （不再构建 53.7MB 派生库）。
8. **回归测量**：改动前后各测一次并记录进 PR：进入 Sessions 页耗时、翻页
   耗时、手动刷新 metadata 发布耗时、Usage 页聚合耗时（应无变化）、
  `cargo test` 目标测试时长（应无量级变化）。按普通用户机器假设评估
  （HDD / 杀软 / 低核数），不得只以本机 NVMe 结论为准。

## 9. 残留上限（威胁模型，写进文档与 `?` 帮助，不阻塞实现）

Gemini 插入失败仍可能推进同步状态（session_usage_gemini.rs:320-338）、Claude/Codex
malformed 行跳过后推进游标、代理 Generated ID 无法归属、同 mtime 内容替换、
proxy 实时日志无即时通知、SQLite REAL 求和精度。这些是 Usage 数据源自身的
正确性上限；因此显示值始终只是**本地尽力估算，不是账单级金额**。上游同样
存在的问题不在本任务扩大修复。

## 10. 实现顺序

1. 核对 `git status`、用户进程（不得 kill 用户启动的 cc-switch）、
   `origin/main`（本分支落后 2 个提交：ea296a0、e92c9cc）。
2. 先写测试：§4.2 两态语义、异步协议过期校验（cost_seq / token / identity /
   页切换）、DB 缺失/busy/future-schema 降级、投影与 Usage 页对账、
   unpriced 逐行判定、`pricing_model` 三态、别名真零价、非法成本文本、
   importer / backfill 计价证据、Hermes 缺列与 NULL/负 token、WebDAV 本地
   `session_log_sync` 保留、无写入断言，以及 UI / 帮助中不出现 `≥` / `~`。
3. 实现 §3 异步链路与 §6 只读边界；随后按 §7 清单删除。
4. `EXPLAIN QUERY PLAN` + top-100 基准 + §8.2.8 回归测量。
5. 隔离目录跑 `cargo fmt --check` / `cargo clippy` / 目标测试；对照基线已知
   失败（§11），不混入无关修复。
6. 确认与 `origin/main` 的基线关系；如需合并，逐文件处理冲突并优先复核
   settings / TUI 边界。
7. 按仓库 CLAUDE.md / AGENTS.md 的盲审协议：两名全新独立盲审 → 逐条实证 →
   修复 → 新一轮，收敛前不 commit / push / PR。

## 11. 基线已知问题（不要顺手修）

- `cargo clippy` 在 home_chart.rs 既有代码上触发 `reversed_empty_ranges`；
- 集成目标 `settings_current_provider` / `settings_visible_apps` 在基线 HEAD
  即编译失败；
- 均与本任务无关，不得混入本 PR。

## 12. 禁区（沿用 handoff §14，全文有效）

不改主机 `$CC_SWITCH_CONFIG_DIR` / `$CLAUDE_CONFIG_DIR` / `$CODEX_HOME`；
写入型测试一律隔离 home/temp dir；诊断读真实历史保持只读；不删用户 sidecar；
不改主库 schema / rollup / 去重 / 定价语义；Cost 投影不写主库或 provider
状态；Sessions 无周期自动刷新；所有 Cargo 命令在 `src-tauri/` 下执行。
