# Requirements: Per-Model Provider Routing

**Defined:** 2026-06-11
**Core Value:** 一键切换 AI 编程工具的底层 provider，零配置摩擦

## v1 Requirements (Milestone 1)

### 数据存储 (DB)

- [x] **DB-01**: 数据库 Schema 从 v10 升级到 v11，新增 `model_routes` 表
- [x] **DB-02**: `model_routes` 表包含字段：id, app_type, pattern (通配符), provider_id, priority (排序), enabled (开关), created_at, updated_at
- [x] **DB-03**: 支持 CRUD 操作：创建路由规则、列出所有规则、更新规则、删除规则
- [x] **DB-04**: 规则按 priority 排序，同 priority 按创建时间排序
- [x] **DB-05**: 创建规则时验证 provider_id 存在且属于同一 app_type
- [x] **DB-06**: Schema 升级向下兼容：空 model_routes 表 = 行为不变

### 路由引擎 (Router)

- [x] **RT-01**: ModelRouter 在代理请求处理流程中先于 ProviderRouter 执行
- [x] **RT-02**: 支持 `*` 通配符匹配 model 名称（如 `*sonnet*`、`claude-*`、`*-4-5`）
- [x] **RT-03**: 多个规则匹配时，选择 priority 最高（数字最小）的 enabled 规则
- [x] **RT-04**: 无匹配规则时，回退到现有的 ProviderRouter 逻辑（行为不变）
- [x] **RT-05**: 规则指向的 provider 不存在时，记录 warning 日志并回退
- [x] **RT-06**: 路由选中的 provider 为单 provider（不使用 failover 队列）

### CLI 命令 (CLI)

- [ ] **CL-01**: `cc-switch proxy model-route list [--app <app>]` — 列出所有路由规则
- [ ] **CL-02**: `cc-switch proxy model-route add <pattern> <provider-id> [--priority <n>] [--app <app>]` — 添加路由
- [ ] **CL-03**: `cc-switch proxy model-route remove <id>` — 删除路由
- [ ] **CL-04**: `cc-switch proxy model-route toggle <id>` — 切换启用/禁用
- [ ] **CL-05**: `cc-switch proxy model-route update <id> [--pattern] [--provider] [--priority]` — 更新路由
- [ ] **CL-06**: 命令输出人类可读的表格格式（与现有 proxy 命令风格一致）

### TUI 界面 (TUI)

- [ ] **UI-01**: 在代理设置页面中增加模型路由管理入口
- [ ] **UI-02**: 路由规则列表表格：显示 pattern、目标 provider、优先级、启用状态
- [ ] **UI-03**: 支持创建新规则：输入 pattern + 选择 provider + 设置优先级
- [ ] **UI-04**: 支持编辑/删除/切换启用状态
- [ ] **UI-05**: 界面风格与现有 TUI 一致（配色、布局、快捷键）

### 同步 (Sync)

- [ ] **SY-01**: model_routes 变更时触发 WebDAV 自动同步（若已配置）
- [ ] **SY-02**: model_routes 变更时触发 S3 自动同步（若已配置）

### 测试 (TEST)

- [x] **TE-01**: model_routes DAO 的 CRUD 单元测试
- [x] **TE-02**: ModelRouter 通配符匹配逻辑的单元测试
- [x] **TE-03**: Schema v10→v11 迁移测试
- [ ] **TE-04**: 代理路由集成测试：匹配规则→选中正确 provider
- [ ] **TE-05**: 代理回退集成测试：无匹配→回退到现有逻辑
- [ ] **TE-06**: CLI 命令集成测试

## Out of Scope

| Feature | Reason |
|---------|--------|
| 正则表达式匹配（仅支持 `*` 通配符） | 与上游 cc-switch PR 保持一致，`*` 覆盖 95% 用例 |
| 多 provider failover for model routes | 设计决策：路由规则选中单 provider，匹配失败回退到现有 failover |
| 基于请求内容的动态路由（非 model 名称） | 复杂度高，无明确用例 |
| 路由规则导入/导出 | 可通过 WebDAV/S3 同步覆盖此需求 |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| DB-01 ~ DB-06 | Phase 1: Database | Pending |
| RT-01 ~ RT-06 | Phase 2: Router Engine | Pending |
| CL-01 ~ CL-06 | Phase 3: CLI Commands | Pending |
| UI-01 ~ UI-05 | Phase 4: TUI Interface | Pending |
| SY-01 ~ SY-02 | Phase 5: Sync Integration | Pending |
| TE-01 ~ TE-06 | Phase 6: Testing | Pending |
