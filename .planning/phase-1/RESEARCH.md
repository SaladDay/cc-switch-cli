# Phase 1 Research: Database Layer (Schema v11 + model_routes DAO)

**Date:** 2026-06-11
**Status:** Complete

## 1. 当前 Schema 架构

### 版本管理

- **当前版本**: v10
- **常量**: `SCHEMA_VERSION: i32 = 10`（`database/mod.rs:56`）
- **版本存储**: SQLite `PRAGMA user_version`
- **读写方法**: `get_user_version(conn)` / `set_user_version(conn, version)`（`schema.rs:2011-2023`）

### 迁移基础设施

迁移入口 `apply_schema_migrations_on_conn()`（`schema.rs:337-430`）：

```
SAVEPOINT schema_migration
  → while version < SCHEMA_VERSION:
      match version:
        0 → migrate_v0_to_v1 → set_user_version(1)
        1 → migrate_v1_to_v2 → set_user_version(2)
        ...
        9 → migrate_v9_to_v10 → set_user_version(10)
        _ → error "未知的数据库版本"
  → repair_proxy_request_logs_columns
  → create_request_logs_indexes_if_supported
  → normalize_auto_failover_requires_takeover
  → RELEASE schema_migration (or ROLLBACK on error)
```

关键特性：
- 使用 SAVEPOINT 包裹整个迁移链，失败自动回滚
- 逐版本递增迁移，不跳版本
- 迁移前在 `Database::init()` 中自动创建备份（`schema.rs:127-134`）
- 检测 future schema 版本时拒绝启动并提示升级

### v9→v10 迁移参考（`schema.rs:1193-1213`）

```rust
fn migrate_v9_to_v10(conn: &Connection) -> Result<(), AppError> {
    Self::add_column_if_missing(conn, "mcp_servers", "enabled_hermes", "BOOLEAN NOT NULL DEFAULT 0")?;
    if Self::table_exists(conn, "skills")? {
        Self::add_column_if_missing(conn, "skills", "enabled_hermes", "BOOLEAN NOT NULL DEFAULT 0")?;
    }
    log::info!("v9 -> v10 迁移完成：已添加 Hermes Agent 支持");
    Ok(())
}
```

模式：使用 `add_column_if_missing` 安全添加列（幂等），使用 `table_exists` 处理可选表。

### 表创建

`create_tables_on_conn()`（`schema.rs:16-328`）定义所有表：
1. providers (复合主键: id + app_type)
2. provider_endpoints (FOREIGN KEY → providers)
3. mcp_servers (per-app enablement columns)
4. prompts (复合主键: id + app_type)
5. skills (id PRIMARY KEY + per-app enablement)
6. skill_repos
7. settings (key-value)
8. proxy_config (三行结构: claude/codex/gemini)
9. provider_health (FOREIGN KEY → providers)
10. proxy_request_logs
11. model_pricing
12. stream_check_logs
13. proxy_live_backup
14. usage_daily_rollups
15. session_log_sync

每个表使用 `CREATE TABLE IF NOT EXISTS` 保证幂等。

## 2. 上游 PR (#4081) 的 Schema 变更

PR 新增 v11 迁移，创建 `model_routes` 表：

```sql
CREATE TABLE IF NOT EXISTS model_routes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    app_type TEXT NOT NULL,
    pattern TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (provider_id, app_type) REFERENCES providers(id, app_type) ON DELETE CASCADE
);
```

### ⚠️ 关键对齐约束

cc-switch-cli 的 `dao/mod.rs:16-17` 明确声明：
```rust
// NOTE(cc-switch-cli): keep schema aligned with upstream, but only compile the DAOs
// that are currently supported by the CLI build.
```

**这意味着**:
1. `model_routes` 表结构必须与上游完全一致
2. 迁移 v10→v11 必须与上游完全相同
3. 用户可能在 cc-switch 和 cc-switch-cli 之间共用同一个 `cc-switch.db` 文件
4. 任何 schema 差异都会导致数据损坏或启动失败

## 3. DAO 模式分析

### 现有 DAO 结构（`dao/mod.rs`）

```rust
pub mod failover;
pub mod mcp;
pub mod model_pricing;
pub mod prompts;
pub mod providers;
pub mod providers_seed;
pub mod proxy;
pub mod settings;
pub mod skills;
pub mod stream_check;
pub mod usage_rollup;
pub mod universal_providers;  // 注意：这个也在目录中
```

### DAO 方法模式（参考 `dao/failover.rs`）

- DAO 方法直接 `impl Database { ... }`
- 使用 `lock_conn!(self.conn)` 获取连接
- 使用参数化查询（`?1`, `params![]`）
- 错误类型：`AppError::Database(...)`
- 返回类型：`Result<T, AppError>`

### 新增 DAO 需要

1. 新建 `dao/model_routes.rs`
2. 在 `dao/mod.rs` 添加 `pub mod model_routes;`
3. 可选：在 `database/mod.rs` 中添加类型导出

## 4. 类型定义模式

### Provider 类型（`provider.rs`）

`Provider` struct 使用 `#[derive(Debug, Clone, Serialize, Deserialize)]`。

### ModelRoute 类型设计

参考上游 PR 和现有 `FailoverQueueItem` 模式：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRoute {
    pub id: Option<i64>,         // None for new, Some for persisted
    pub app_type: String,
    pub pattern: String,
    pub provider_id: String,
    pub priority: i32,
    pub enabled: bool,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}
```

### 类型放置位置

有两个选择：
- **选项 A**: 新建 `src-tauri/src/model_route.rs`（独立模块）
- **选项 B**: 放在 `provider.rs` 中（与 Provider 放在一起）

上游 PR 倾向于独立模块。cc-switch-cli 的 `lib.rs` 模式支持独立模块。

## 5. 测试模式

### 迁移测试模式（`database/tests.rs:1225-1272`）

```rust
#[test]
fn schema_migration_v9_adds_hermes_columns() {
    let conn = Connection::open_in_memory().expect("open memory db");
    conn.execute_batch("CREATE TABLE mcp_servers (...); CREATE TABLE skills (...);")
        .expect("seed v9 schema");
    Database::set_user_version(&conn, 9).expect("set user_version=9");
    Database::apply_schema_migrations_on_conn(&conn).expect("apply migrations");
    assert_eq!(Database::get_user_version(&conn).unwrap(), SCHEMA_VERSION);
    assert!(Database::has_column(&conn, "mcp_servers", "enabled_hermes").unwrap());
}
```

### DAO 测试模式

- 使用 `Database::memory()` 创建内存数据库
- 直接在测试中构造 SQL INSERT 准备数据
- 调用 DAO 方法验证 CRUD 正确性
- 测试边界条件（重复插入、缺失外键等）
- 使用 `#[test]` 标记，非 `#[tokio::test]`（数据库操作是同步的）

## 6. 迁移安全性分析

### 用户数据保护

1. **自动备份**: `Database::init()` 在迁移前自动备份数据库文件（`schema.rs:127-134`）
2. **SAVEPOINT**: 迁移包裹在 SAVEPOINT 中，失败自动回滚
3. **幂等建表**: 使用 `CREATE TABLE IF NOT EXISTS`
4. **Future schema 检测**: 新版本数据库不会在旧版本代码中打开

### cc-switch 兼容性

- cc-switch-cli 和 cc-switch 共用同一个 `~/.cc-switch/cc-switch.db`
- **v10→v11 迁移必须完全一致**
- 如果 cc-switch 先升级到 v11，cc-switch-cli 必须能正确打开 v11 数据库
- 如果 cc-switch-cli 先升级，cc-switch 也必须能正确打开

## 7. 决策点

| 决策 | 选项 | 推荐 |
|------|------|------|
| model_routes 表结构 | 与上游 PR 完全一致 | ✅ 必须一致 |
| ModelRoute 类型位置 | `model_route.rs` 独立模块 | ✅ 与上游保持一致 |
| DAO 方法放置 | `impl Database` in `dao/model_routes.rs` | ✅ 与现有模式一致 |
| 迁移版本号 | v11 (SCHEMA_VERSION=11) | ✅ 与上游一致 |
| 外键约束 | `FOREIGN KEY (provider_id, app_type) REFERENCES providers(id, app_type) ON DELETE CASCADE` | ✅ provider 删除时级联删除路由 |

## 8. 实现清单

### 必须修改的文件

| 文件 | 变更 |
|------|------|
| `database/mod.rs` | `SCHEMA_VERSION` 10→11 |
| `database/schema.rs` | ① `create_tables_on_conn` 添加 model_routes 表 ② 添加 `migrate_v10_to_v11` ③ 在 `apply_schema_migrations_on_conn` 添加 version 10 分支 |
| `database/dao/mod.rs` | 添加 `pub mod model_routes;` |
| `database/dao/model_routes.rs` | **新建** — CRUD DAO 实现 |
| `model_route.rs` (src-tauri/src/) | **新建** — ModelRoute 类型定义 |
| `lib.rs` | 添加 `mod model_route;` + 公开导出 |
| `database/tests.rs` | 添加 v10→v11 迁移测试 + DAO 测试 |

### 不需要在此阶段修改的文件

- `provider.rs` — 不需要修改（ModelRoute 独立于 Provider）
- 代理层文件 — Phase 2 处理
- CLI 命令文件 — Phase 3 处理
