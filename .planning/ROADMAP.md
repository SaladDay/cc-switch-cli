# Roadmap: Per-Model Provider Routing

**Created:** 2026-06-11
**Milestone:** 1
**Total phases:** 6
**Estimated effort:** 17-27 hours (~2.5-4 days)

---

## Phase Dependency Graph

```
Phase 1: Database Layer
    ↓
Phase 2: Router Engine + Proxy Integration
    ↓
┌───────────────┬───────────────┐
↓               ↓               ↓
Phase 3:        Phase 4:        Phase 5:
CLI Commands    TUI Interface   Sync Integration
    ↓               ↓               ↓
└───────────────┴───────────────┘
                  ↓
          Phase 6: Final Testing & PR Prep
```

Phases 3, 4, 5 可并行执行（都只依赖 Phase 2）。

---

## Phase 1: Database Layer

**Goal:** 创建 `model_routes` 表和相关 DAO，完成 Schema v10→v11 迁移

**Depends on:** 无
**Estimated effort:** 2-3 小时
**Files to touch:** ~4 files, ~230 lines

### Tasks

1. **Schema v11 migration**
   - 在 `database/schema.rs` 中实现 `migrate_v10_to_v11()`
   - 创建 `model_routes` 表：id INTEGER PK, app_type TEXT NOT NULL, pattern TEXT NOT NULL, provider_id TEXT NOT NULL, priority INTEGER DEFAULT 0, enabled INTEGER DEFAULT 1, created_at TEXT, updated_at TEXT
   - 添加 FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
   - 更新 `CURRENT_SCHEMA_VERSION` 常量

2. **ModelRoute 类型定义**
   - 在 `provider.rs`（或新建 `model_route.rs`）中定义 `ModelRoute` struct
   - 实现 Serialize/Deserialize/Clone/Debug

3. **model_routes DAO**
   - 新建 `database/dao/model_routes.rs`
   - `list_routes(app_type) → Vec<ModelRoute>` — 按 priority ASC, created_at ASC
   - `create_route(route) → ModelRoute`
   - `update_route(id, updates) → ModelRoute`
   - `delete_route(id)`
   - `toggle_route(id)`
   - `get_route(id) → Option<ModelRoute>`
   - 创建时验证 provider_id 存在且属于同 app_type
   - 在 `database/dao/mod.rs` 中注册模块

4. **Database 集成**
   - 在 `database/mod.rs` 中暴露 DAO 方法
   - 确保 `try_new()` 自动执行迁移

### Verification
- [ ] `cargo test database` — 所有数据库测试通过
- [ ] DAO CRUD 测试覆盖所有操作
- [ ] Schema 迁移测试：v10 数据库升级到 v11 后数据完整
- [ ] 向下兼容：无 model_routes 时所有现有功能正常

**Covers:** DB-01 ~ DB-06, TE-01, TE-03

---

## Phase 2: Router Engine + Proxy Integration

**Goal:** 实现 ModelRouter 通配符匹配引擎，并集成到代理请求处理流程

**Depends on:** Phase 1
**Estimated effort:** 4-6 小时
**Files to touch:** ~8 files, ~500 lines
**Plans:** 1 plan

### Plans

- [ ] 02-01-PLAN.md — ModelRouter engine creation, HandlerContext integration, ProxyServerState wiring, integration tests

**Covers:** RT-01 ~ RT-06, TE-02


---

## Phase 3: CLI Commands

**Goal:** 实现 `cc-switch proxy model-route` 子命令组

**Depends on:** Phase 1（仅需 DAO，可与 Phase 2 并行）
**Estimated effort:** 1-2 小时
**Files to touch:** ~2 files, ~70 lines

### Tasks

1. **Clap 子命令定义**
   - 在 `cli/commands/proxy.rs` 中新增 `ModelRouteCommand` enum
   - 变体：List, Add { pattern, provider_id, priority }, Remove { id }, Toggle { id }, Update { id, pattern?, provider_id?, priority? }
   - 在 `ProxyCommand` enum 中添加 `ModelRoute(ModelRouteCommand)` 变体

2. **命令实现**
   - `list` — 调用 DAO list_routes，表格格式输出（pattern, provider, priority, enabled）
   - `add` — 验证 pattern 和 provider_id 有效性，调用 DAO create
   - `remove` — 验证 id 存在，确认后删除
   - `toggle` — 切换 enabled 状态
   - `update` — 部分更新（只更新提供的字段）
   - 所有命令支持 `--app` 全局标志

3. **CLI mod 集成**
   - 在 `cli/mod.rs` 中添加 model-route 子命令的 dispatch 逻辑

### Verification
- [ ] `cargo run -- proxy model-route list` — 显示空列表或已有规则
- [ ] `cargo run -- proxy model-route add "*-4-5" <provider-id>` — 成功添加
- [ ] `cargo run -- proxy model-route toggle <id>` — 成功切换
- [ ] `cargo run -- proxy model-route remove <id>` — 成功删除
- [ ] 错误处理：无效 provider_id → 友好错误信息

**Covers:** CL-01 ~ CL-06, TE-06

---

## Phase 4: TUI Interface

**Goal:** 在 ratatui TUI 的代理设置区域增加模型路由管理界面

**Depends on:** Phase 1 + Phase 2（需要 DAO 和 ModelRouter 工作正常）
**Estimated effort:** 6-10 小时（最大工作量）
**Files to touch:** ~4 files, ~350 lines

### Tasks

1. **路由列表表格**
   - 在代理设置页面添加 "Model Routes" 区域/标签
   - 表格列：Pattern | Provider | Priority | Enabled | Actions
   - 集成到现有的 TUI 布局系统（`tui/ui/` 或 `tui/app/`）

2. **创建/编辑表单**
   - pattern 输入框（文本）
   - provider 选择器（复用现有 provider picker）
   - priority 数字输入
   - 保存/取消

3. **操作处理**
   - runtime_actions 中新增 model_route 相关 action handler
   - 调用 DAO 的 CRUD 方法
   - 操作后刷新列表

4. **界面一致性**
   - 复用现有 TUI 组件库（form、table、overlay）
   - 配色参考现有 proxy 设置页面的风格
   - 键盘快捷键与现有界面一致

### Verification
- [ ] TUI 中能查看路由规则列表
- [ ] 能创建新规则（输入 pattern + 选 provider + 设 priority）
- [ ] 能编辑已有规则
- [ ] 能删除规则（带确认）
- [ ] 能切换启用/禁用
- [ ] 界面无渲染异常（layout 不溢出、颜色正确）

**Covers:** UI-01 ~ UI-05

---

## Phase 5: Sync Integration

**Goal:** model_routes 变更时触发 WebDAV/S3 自动同步

**Depends on:** Phase 1（仅需 DAO）
**Estimated effort:** 0.5-1 小时
**Files to touch:** ~2 files, ~10 lines

### Tasks

1. **WebDAV 同步触发**
   - 在 `services/webdav_auto_sync.rs` 中添加 model_routes 表变更的触发
   - 在 DAO 的 create/update/delete 方法中调用 sync trigger

2. **S3 同步触发**
   - 在 `services/s3_auto_sync.rs` 中同样添加触发
   - 保持与现有同步机制一致的模式

### Verification
- [ ] 配置 WebDAV 同步后，添加/修改路由规则触发同步
- [ ] 配置 S3 同步后，添加/修改路由规则触发同步

**Covers:** SY-01 ~ SY-02

---

## Phase 6: Final Testing & PR Preparation

**Goal:** 全面测试，清理代码，准备可合并的纯净 PR 分支

**Depends on:** Phase 3, 4, 5（全部完成）
**Estimated effort:** 3-5 小时

### Tasks

1. **Integration Testing**
   - E2E 代理测试：Model matches enabled route → route-selected provider used
   - E2E 代理测试：No matching route → falls back to app-level provider
   - E2E 代理测试：Empty routes → no behavior change
   - E2E 代理测试：Route points to missing provider → warning logged, falls back
   - CLI 命令集成测试

2. **Regression Testing**
   - `cargo test` — 全部测试通过
   - `cargo clippy` — 无新增 warning
   - `cargo fmt --check` — 格式正确

3. **PR Branch Preparation**
   - 创建功能分支 `feat/model-based-routing`
   - 仅提交功能代码（排除 `.planning/` 目录）
   - 写 PR 描述：参考上游 PR #4081 的结构
   - Self-review 检查清单

4. **Documentation**
   - 更新 README（如需要）
   - 确保 CLI help 文本完整

### Verification
- [ ] 全部测试通过（`cargo test`）
- [ ] 无 clippy warning
- [ ] 格式检查通过
- [ ] PR 分支干净（`.planning/` 在 .gitignore 或未提交）
- [ ] 手工 smoke test：启动代理 → 配置路由规则 → 发请求验证

**Covers:** TE-04, TE-05

---

## Risk Register

| Risk | Severity | Mitigation |
|------|----------|------------|
| handler_context 结构与 cc-switch 差异过大，ModelRouter 集成点不匹配 | MEDIUM | Phase 2 开始前详细对比两个项目的 handler_context 结构 |
| TUI 表单组件不够灵活，无法实现 pattern + provider picker 组合输入 | MEDIUM | Phase 4 开始前评估现有 TUI 组件能力，必要时简化输入流程 |
| Schema 迁移与现有备份/恢复机制冲突 | LOW | Phase 1 先研究现有迁移模式和备份逻辑 |
| 上游 PR 的变更在 cc-switch-cli 中路径/API 不同 | LOW | 每个 Phase 对照当前代码库做适配，不盲目复制 |

---

## Traceability

| Phase | Requirements Covered | Est. Effort |
|-------|---------------------|-------------|
| Phase 1: Database | DB-01~06, TE-01, TE-03 | 2-3h |
| Phase 2: Router Engine | RT-01~06, TE-02 | 4-6h |
| Phase 3: CLI Commands | CL-01~06, TE-06 | 1-2h |
| Phase 4: TUI Interface | UI-01~05 | 6-10h |
| Phase 5: Sync | SY-01~02 | 0.5-1h |
| Phase 6: Testing & PR | TE-04~05 | 3-5h |
| **Total** | **31 requirements** | **17-27h** |
