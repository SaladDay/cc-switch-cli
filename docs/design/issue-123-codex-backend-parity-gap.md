# Issue #123 后端差异文档

## 范围

本文只讨论 issue #123 对应的后端链路，不讨论前端表单、文案和交互。

对比范围集中在 Codex provider 切换、live `config.toml` 回填、common config、MCP 同步、proxy takeover 热切换，以及 `current provider` 判定这几条链路。

对比基线：

- 当前仓库：`cc-switch-cli` backend，Cargo 版本 `5.3.4`
- 上游快照：`./.upstream/cc-switch` backend，package 版本 `3.13.0`

核心问题来自 [issue #123](https://github.com/SaladDay/cc-switch-cli/issues/123)：用户在 Codex 中切换 provider 后，再切回原 provider，`~/.codex/config.toml` 里运行期新增的配置会丢失。典型例子是 trusted folders，也包括其他不在 provider 基础字段里的自定义配置。

## 结论

当前 CLI 分支在这一段后端实现上，和上游不是同一条语义链路。

根因不在一个点上，而是几处差异叠在一起：

- 上游把“当前 provider 是谁”的判定建立在 `effective current provider` 上，也就是本地 settings 和数据库视角的真实当前值。
- 当前 CLI 分支的 Codex 回填逻辑仍然依赖内存里的 `manager.current`。
- 当前 CLI 分支在热切换时会额外改内存里的 `manager.current`，但不走完整持久化链路。
- 当前 CLI 分支在正常切换后还会做一次 `refresh_snapshot`，重新读取 live `config.toml`，顺手抽 common config、剥掉 `mcp_servers`，再写回 provider 快照；上游正常切换路径没有这一步。

如果目标是“ccs-cli 的后端和上游完全一致”，那这几处都要收敛，不能只修一个 backfill 函数。

## 差异总表

| 链路 | 上游 | 当前 CLI | 影响 | 结论 |
| --- | --- | --- | --- | --- |
| 当前 provider 判定 | 使用 `get_effective_current_provider()` | Codex 回填使用 `manager.current` | 可能把 live 配置回填到错误 provider，或漏回填 | 必须对齐 |
| 热切换后的状态更新 | 只更新 db/settings，不改内存 config snapshot | 额外改 `state.config` 里的 `manager.current` | 内存态和持久态可能暂时分叉 | 必须对齐 |
| 正常切换前回填 | 统一走 `read_live_settings()` + `strip_common_config_from_live_settings()` + `db.save_provider()` | Codex 走 `backfill_codex_current()` 自定义路径 | Codex 语义和上游分叉，排查成本高 | 必须对齐 |
| 正常切换后的处理 | 写 live，然后同步 MCP | 写 live，然后 `refresh_snapshot` 再回读 live | 会在切换后改写 provider snapshot 形态 | 必须评估后对齐 |
| common config 引擎位置 | 上游集中在 `services/provider/live.rs` | 当前拆到 `mod.rs` / `codex.rs` / `common.rs` | 结构不同，行为不易逐项证明一致 | 建议对齐 |
| Codex live 写盘顺序 | 先写 `auth.json`，再写 `config.toml` | 先写 `config.toml`，再写 `auth.json` | 失败时的中间态不同 | 应对齐 |
| Codex TOML 辅助 API | `codex_config.rs` 提供 field update / proxy cleanup helper | 这些 helper 被移到 `services/proxy/codex_toml.rs` | 位置和接口不一致 | 建议对齐 |
| 切换事务模型 | 上游是直接式 switch flow | 当前是 `run_transaction` + post-commit action + rollback | 容错更强，但不再是上游行为 | 如果目标是完全一致，需要评估是否保留 |

## 详细差异

### 1. 当前 provider 的判定来源不一致

上游在正常切换前，会先取 `effective current provider`：

- 文件：`.upstream/cc-switch/src-tauri/src/services/provider/mod.rs`
- 位置：`switch_normal()`，约 `1466-1497`

上游逻辑很直接：

1. 用 `crate::settings::get_effective_current_provider(&state.db, &app_type)` 取当前 provider。
2. 如果将要切到另一个 provider，就读取当前 live 配置。
3. 把 live 配置去掉 common config 后，保存回“真实当前 provider”的快照。

当前 CLI 分支的 Codex 不走这条链，而是走：

- 文件：`src-tauri/src/services/provider/codex.rs`
- 位置：`backfill_codex_current()`，约 `274-339`

这里拿当前 provider 的方式是：

```rust
let current_id = config
    .get_manager(&AppType::Codex)
    .map(|m| m.current.clone())
    .unwrap_or_default();
```

也就是说，它信的是内存里的 `manager.current`，不是 `effective current provider`。

这和当前仓库自己的测试结论是冲突的。当前仓库已经有测试明确说明：

- `current()` 可以优先返回 local settings / db 的 current provider
- 但不会去自愈 `state.config` 里的 `manager.current`

可见：

- `src-tauri/src/services/provider/tests.rs`
- `current_prefers_effective_local_override_without_mutating_config()`
- `current_falls_back_to_db_current_without_self_healing_config()`

这意味着只要内存态落后于 local settings 或 db，Codex 的 backfill 就可能回填到旧 provider，或者直接跳过。

这条差异和 issue #123 最接近。

### 2. 热切换路径在当前分支里多改了一份内存状态

上游的热切换分支：

- 文件：`.upstream/cc-switch/src-tauri/src/services/provider/mod.rs`
- 位置：约 `1410-1428`

行为是：

1. 调 `proxy_service.hot_switch_provider(...)`
2. 直接返回
3. 不改内存里的 provider snapshot
4. 不写 live config

当前 CLI 分支：

- 文件：`src-tauri/src/services/provider/mod.rs`
- 位置：约 `1584-1596`

在调用 `proxy_service.hot_switch_provider(...)` 之后，还会做这一步：

```rust
let mut guard = state.config.write().map_err(AppError::from)?;
if let Some(manager) = guard.get_manager_mut(&app_type) {
    manager.current = provider_id.to_string();
}
```

这一步和上游不同。

问题不在“多改了一份状态”本身，而在于：

- `proxy_service.hot_switch_provider()` 已经会更新 db 和 local settings
- 但这里对 `state.config` 的修改没有走完整保存链路
- 后面 Codex backfill 又恰好依赖 `manager.current`

于是就出现了一个很危险的组合：

- 上游依赖的是真实 current
- 当前分支依赖的是内存 current
- 当前分支还在热切换时私自改这份内存 current

这会让后续普通切换、common config 更新、snapshot refresh 的行为都变得更难预测。

### 3. 正常切换前的 backfill 语义已经分叉

上游的 backfill 是统一模型：

- 文件：`.upstream/cc-switch/src-tauri/src/services/provider/mod.rs`
- 位置：约 `1466-1497`
- 依赖：`.upstream/cc-switch/src-tauri/src/services/provider/live.rs`
- 位置：`strip_common_config_from_live_settings()`，约 `515-552`

上游模型可以概括成一句话：

“读 live，去掉 common config，把结果保存回真实当前 provider。”

当前 CLI 分支的 Codex 则是单独实现了一套：

- 文件：`src-tauri/src/services/provider/codex.rs`
- 位置：`backfill_codex_current()`，约 `274-339`

它多做了几件上游正常切换路径里没有的事：

- 如果 common config snippet 为空，会尝试从 live `config.toml` 里自动抽一份
- 回填前走 `normalize_settings_config_for_storage(...)`
- 回填使用的是 `MultiAppConfig` 内存模型，不是直接写 db provider

这些动作不一定都是错的，但它们已经不是上游语义。

如果目标是后端完全一致，这里不能停留在“结果差不多”，而要回到上游的统一 backfill 模型。

### 4. 当前分支在切换后多了一次 `refresh_snapshot`

当前 CLI 分支在正常切换完成后，会带着：

- `refresh_snapshot: true`

进入 post-commit action。

可见：

- 文件：`src-tauri/src/services/provider/mod.rs`
- 位置：约 `1644-1650`

随后它会重新读取 live 配置，并刷新当前 provider 的 snapshot：

- 文件：`src-tauri/src/services/provider/mod.rs`
- 位置：`refresh_provider_snapshot()`，Codex 分支约 `409-491`

Codex 这一步会做三件事：

1. 从 live `config.toml` 抽 common config
2. 把 `mcp_servers` 从 snapshot 配置里剥掉
3. 再走一次 `normalize_settings_config_for_storage(...)`

上游正常切换路径没有这一步。上游在 `switch_normal()` 里是写 live 后直接继续，不会立刻回读 live 再改 provider snapshot。

这条差异的影响是：

- 当前 provider snapshot 在一次普通切换之后，会被二次整形
- snapshot 形态不再是“用户保存的 provider 配置”，而更像“写盘后的 live 反解结果”
- 这会把问题边界从“切换”扩大到“切换后的所有后续操作”

就 issue #123 这个问题看，这一步会放大“trusted folders / 自定义字段到底留在 provider snapshot、common snippet，还是只在 live 文件里”的不确定性。

### 5. common config 的实现位置和调用顺序都变了

上游：

- 主要逻辑集中在 `.upstream/cc-switch/src-tauri/src/services/provider/live.rs`
- 关键函数：
  - `build_effective_settings_with_common_config()`
  - `write_live_with_common_config()`
  - `strip_common_config_from_live_settings()`
  - `normalize_provider_common_config_for_storage()`

当前 CLI 分支：

- `live.rs` 只保留了 snapshot capture / restore 和 OpenClaw mirror
- common config 逻辑分散到了：
  - `src-tauri/src/services/provider/mod.rs`
  - `src-tauri/src/services/provider/codex.rs`
  - `src-tauri/src/services/provider/common.rs`

结果不是“代码搬了个地方”这么简单，而是调用顺序变了：

- 上游是“写 live 时应用 common config，回填时去掉 common config”
- 当前分支在“provider 保存、switch 前 backfill、switch 后 refresh”三个时点都会碰 common config

这让语义更强，也更重，但已经偏离上游。

如果要追求完全一致，最好把 Codex / Claude / Gemini 的 common config 链路重新压回上游那种单一入口，而不是继续依赖当前拆分后的多点处理。

### 6. Codex live 写盘顺序不同

上游写 Codex live 时：

- 文件：`.upstream/cc-switch/src-tauri/src/services/provider/live.rs`
- 位置：约 `672-688`

顺序是：

1. 写 `auth.json`
2. 写 `config.toml`

当前 CLI 分支：

- 文件：`src-tauri/src/services/provider/codex.rs`
- 位置：约 `402-410`

顺序是：

1. 写 `config.toml`
2. 写 `auth.json`

两边都没有在这里直接调用 `write_codex_live_atomic()`，但中间态不同。

这不是 issue #123 的主因，不过如果目标是“后端完全一致”，这也要统一。

### 7. Codex TOML helper API 已经从上游接口分叉

上游在 `codex_config.rs` 里保留了两类 helper：

- `update_codex_toml_field(...)`
- `remove_codex_toml_base_url_if(...)`

当前 CLI 分支删掉了这两个 helper，改成：

- `src-tauri/src/services/proxy/codex_toml.rs`
  - `update_toml_base_url(...)`
  - `remove_loopback_base_url_from_toml(...)`

而 `src-tauri/src/codex_config.rs` 现在提供的是另一组 API：

- `strip_codex_provider_config_text(...)`
- `clean_codex_provider_key(...)`

这说明两边在 Codex TOML 的职责拆分上已经不一样了。

从 issue #123 看，这不是最危险的差异；但如果后面真的要“完全一致”，就不能只对齐行为，还得把 helper 的职责边界也对齐，否则以后再从 upstream cherry-pick 相关修复时会继续冲突。

### 8. 当前分支的事务模型比上游更重

上游的 switch flow 是直接式的：

- 找 provider
- backfill 当前 live
- 更新 current provider
- 写 live
- sync MCP

当前 CLI 分支包了一层：

- `run_transaction(...)`
- `PostCommitAction`
- `capture_live_snapshot(...)`
- post-commit 失败时回滚 config + live backup

这套机制本身不是坏事，甚至更稳。

但它确实不是上游行为。

如果目标是“行为上完全一致”，至少要决定一件事：

- 我们要的是“和上游结果一致”
- 还是“连 switch 的事务边界和失败回滚策略都一致”

如果是后者，那当前这套 transaction/post-commit 框架本身也属于差异项。

## 哪些差异最可能解释 issue #123

从当前证据看，优先级最高的是这三条：

1. Codex backfill 依赖 `manager.current`，而不是 `effective current provider`
2. 热切换路径额外修改 `manager.current`
3. 切换后 `refresh_snapshot` 会再次重写 provider snapshot

这三条叠起来，刚好可以形成 issue #123 的典型失败链路：

1. 用户在 provider A 下使用 Codex
2. Codex 在 live `config.toml` 中新增 trusted folders 之类的运行期配置
3. 当前 provider 的判定和 snapshot 保存没有严格跟上游对齐
4. 用户切到 provider B 时，A 的 live 配置没有准确回填到 A
5. 用户再切回 A 时，CLI 用旧 snapshot 覆盖了 live `config.toml`
6. trusted folders 和其他运行期新增配置被写丢

我现在没有直接复现到“trusted folders”这个单一字段，但现有代码和测试已经足够说明，这条失败链在后端上是成立的。

## 对齐计划

### 目标和边界

目标分两层：

1. 先修复 issue #123：Codex 正常切换时，切走前必须把 live `~/.codex/config.toml` 中的运行期新增配置保存回真实当前 provider。
2. 再逐步收敛后端语义：除前端和 CLI/TUI 表层差异外，provider switch / live sync / common config / proxy takeover 这些后端链路尽量贴近 upstream。

本轮不把所有结构差异一次性重写。先固定语义，再决定是否继续压缩当前分支额外的 transaction / post-commit / refresh 机制。

### Phase 0：加测试先锁定现象

新增 issue #123 的后端回归测试，建议放在 `src-tauri/tests/provider_service.rs` 或 `src-tauri/src/services/provider/tests.rs`：

1. 构造 Codex provider A、provider B。
2. 设置 effective current provider 为 A，但让 `state.config` 中的 `manager.current` 与 effective current 分叉，覆盖当前分支最危险的状态。
3. 写入 live `auth.json` 和 `config.toml`，其中 `config.toml` 包含 provider A 的基础字段，以及 Codex 运行期新增字段，例如：
   - `trusted_workspaces` / `projects` / profile 相关配置，按 Codex 实际 TOML 结构选一种稳定样例
   - 一个非 provider 基础字段的普通 top-level key，用来证明不是只保留固定白名单字段
4. 执行 `ProviderService::switch(..., AppType::Codex, "provider-b")`。
5. 再执行 `ProviderService::switch(..., AppType::Codex, "provider-a")`。
6. 断言 provider A 的 snapshot 和最终 live `config.toml` 都保留运行期新增字段。
7. 断言 provider B 的 snapshot 和切到 B 后的 live `config.toml` 没有误吸收 A 的运行期新增字段。
8. 如果测试路径触发 Codex common snippet 自动抽取，断言 common snippet 没有误包含 A 的 provider-local trusted/project 字段。

同时新增一个更小的根因测试：

1. DB / local settings 的 effective current 是 A。
2. `state.config.manager.current` 是空、missing，或 B。
3. 切到 B。
4. 断言 backfill 写回的是 A，不是 `manager.current` 指向的值。
5. 断言这次 backfill 在当前 transaction 模型下不会被随后 `state.save()` 用旧内存 snapshot 覆盖。

这个测试应该先在当前实现上失败，用来证明修复命中根因。

### Phase 1：修复 Codex 正常切换 backfill

把 Codex 切换前 backfill 收回到 upstream 语义：

1. 不再在 `backfill_codex_current()` 中使用 `config.get_manager(...).current` 作为当前 provider 判断来源。
2. 在进入正常 switch flow 前，使用 `crate::settings::get_effective_current_provider(&state.db, &AppType::Codex)` 解析真实当前 provider。
3. 当真实当前 provider 存在且不等于目标 provider 时：
   - 读取 live `auth.json` 和 `config.toml`
   - 对 Codex config 应用当前分支已有的 common config stripping 规则
   - 保存回真实当前 provider 的 snapshot
4. 保存目标 provider current 时，继续同时更新 DB current 和 local settings current，保持当前多设备本地 current 语义。

实现上有两个可选落点：

1. 最贴近 upstream：在 `ProviderService::switch()` 的正常分支里统一做 backfill，Codex / Claude / Gemini 都使用 effective current。
2. 较小改动：让 `prepare_switch_codex()` 接收 effective current id，或新增一个 Codex 专用 backfill 入口，从 `switch()` 外层把 effective current 传进去。

建议优先采用第一个落点。它更接近 upstream，也能顺带处理 Claude / Gemini 目前同样依赖 `manager.current` 的潜在分叉问题。

但这里不能机械照搬 upstream 的 `state.db.save_provider(...)`。

当前 CLI 分支仍然是：

1. `ProviderService::switch()` 进入 `run_transaction(...)`
2. 闭包里修改内存 `MultiAppConfig`
3. `state.save()` 把整份内存 config 持久化到 SQLite

所以 Phase 1 的安全实现必须满足下面任一条件：

1. 继续在 transaction 的 `MultiAppConfig` 中写回 backfill 结果，然后让 `state.save()` 落库。
2. 或者先改掉 `run_transaction` / `state.save()` 的持久化边界，保证直接写入 DB 的 backfill 不会被旧内存 snapshot 覆盖。

短期建议采用第一种：在 `switch()` 外层预先解析 effective current provider，把 current id 传入 transaction；transaction 内部读取 live settings、strip common config，然后更新 `config.get_manager_mut(app_type).providers[current_id].settings_config`。不要在 transaction 中只调用 `state.db.save_provider(...)` 而不更新内存 config。

### Phase 2：去掉热切换里的额外内存 current 写入

对齐 upstream 热切换语义：

1. `proxy_service.hot_switch_provider(...)` 负责更新 DB current 和 local settings current。
2. `ProviderService::switch()` 的 hot-switch 分支不再额外修改 `state.config.manager.current`。
3. 如果 CLI/TUI 后续需要展示最新 current，应该通过 `ProviderService::current()` / reload data 读取 effective current，而不是依赖内存 snapshot 被临时改写。

这一步可以减少“内存 current 看起来已切换，但持久化语义由另一条链路维护”的分叉面。

不过这一步不能孤立执行。

当前 `state.save()` 会把内存里的 `manager.current` 重新写回 DB current。如果 hot-switch 后不再同步内存 current，但后续任意 provider add/update/delete/common-config 路径调用普通 `state.save()`，旧的 `manager.current` 可能把 hot-switch 已更新的 DB current 回滚。

执行 Phase 2 前必须先完成一个保护项：

1. 让相关保存路径使用 `save_preserving_current_providers(...)`，避免 stale `manager.current` 覆盖 DB current。
2. 或者 hot-switch 后刷新内存 config，让内存 snapshot 与 DB/local settings 重新一致。
3. 或者整体调整 `state.save()`，不再默认从 `manager.current` 覆盖非 additive app 的 DB current。

没有这个保护项时，不应单独删除 hot-switch 分支里的内存 current 写入。

### Phase 3：评估并收敛 switch 后 `refresh_snapshot`

当前正常切换后会执行 `refresh_provider_snapshot()`，这不是 upstream 正常 switch 行为。

计划：

1. 先在 Phase 1 修复后跑完整 provider 相关测试，确认 issue #123 已经消失。
2. 梳理 `refresh_snapshot` 目前实际保护的场景：
   - MCP sync 后从 live 反读并剥掉 `mcp_servers`
   - Codex common snippet 自动抽取
   - auth 缺失时保留 DB snapshot auth
3. 如果这些场景可以由 switch 前 backfill、common config 写盘、MCP sync 各自承担，就移除正常 switch 后的 `refresh_snapshot`。
4. 如果短期不能移除，至少限制它不要覆盖刚刚正确回填的当前 provider snapshot，也不要把 live-only 字段错误迁移到目标 provider。

验收标准是：正常 switch 写完目标 live 后，不再立即用 live 反解结果改变目标 provider snapshot，除非有明确、可测试的上游等价理由。

### Phase 4：集中 common config 入口

把 common config 的行为对齐 upstream `services/provider/live.rs` 的模型：

1. 写 live 时：构造 effective settings，再写入 live。
2. 回填 live 时：从 live settings 中剥离 common config，再保存 provider snapshot。
3. provider add / update / import 时：只做必要的 storage normalization，不再分散承担 switch-time 语义。
4. 对齐 upstream 的启用条件：不是“snippet 非空就默认应用到所有 provider”，而是遵循 provider 显式 common-config 标记，或在未显式标记时只对本来包含该 snippet 子集的 provider 应用。

当前分支可以保留 CLI/TUI 的 common config 功能，但后端入口要尽量收敛到一组清晰函数，方便后续和 upstream 比对。

这点和 issue #123 有直接关系：trusted folders / projects 这类运行期字段如果被自动抽进 common snippet，就可能被错误应用到 provider B，或者从 provider A snapshot 中被剥掉。Phase 4 需要明确禁止 provider-local 运行期字段被误归类为 common config。

### Phase 5：Codex live 写盘和 TOML helper 对齐

这批不是 issue #123 的主修点，但属于后端漂移：

1. Codex live 写盘顺序对齐 upstream：先写 `auth.json`，再写 `config.toml`。
2. 评估是否把 proxy takeover 相关 TOML helper 收回 `codex_config.rs`，至少保证接口职责和 upstream 可比。
3. 保留 `toml_edit` 解析和语法保持能力，避免为了对齐路径而退回字符串替换。

### Phase 6：验收测试矩阵

修复完成后至少跑：

1. `cargo test --manifest-path src-tauri/Cargo.toml provider_service`
2. `cargo test --manifest-path src-tauri/Cargo.toml provider_switch_settings_sync`
3. `cargo test --manifest-path src-tauri/Cargo.toml settings_current_provider`
4. `cargo test --manifest-path src-tauri/Cargo.toml proxy_takeover`
5. 如果改到 common config 或 MCP sync，再补跑：
   - `cargo test --manifest-path src-tauri/Cargo.toml mcp_commands`
   - `cargo test --manifest-path src-tauri/Cargo.toml import_export_sync`

最终手工验收：

1. provider A 使用 Codex 官方账号登录。
2. Codex 在 live `config.toml` 写入 trusted folders / projects。
3. 切到 provider B。
4. 切回 provider A。
5. trusted folders / projects 仍存在，且 provider B 没有误吸收 A 的运行期字段。

### 回滚和风险控制

1. Phase 1 应该独立成一个可回滚提交，只改变正常 switch backfill 目标选择和保存路径。
2. Phase 2、Phase 3 会影响 proxy takeover 和 post-commit 行为，应分开提交。
3. common config 和 TOML helper 的结构收敛不应和 issue #123 修复混在同一个提交里。
4. 每个 phase 都要保留对 local settings current 的测试，因为这是当前分支相对 upstream 的重要后端语义。

## 最后的判断

如果只看 issue #123，我认为当前仓库最需要修的不是“trusted folders 这个字段本身”，而是“Codex backfill 选择当前 provider 的依据”。

如果看你给的目标，“ccs-cli 的后端和上游完全一致”，那修复范围要更大：

- 不只是修一个 bug
- 而是把 Codex 切换这段 backend 从当前的 transaction + refresh + custom backfill 模式，收回到 upstream 的 switch / live / common config 语义

否则，这次修掉 trusted folders，后面还会在别的 live-only 字段上再遇到同类问题。
