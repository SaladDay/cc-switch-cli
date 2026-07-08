# Phase 2 Research: Router Engine + Proxy Integration

**Date:** 2026-06-12
**Status:** Complete

## 1. 请求处理流程分析

### 当前流程

```
Handler (handlers.rs)
  → HandlerContext::load(state, app_type, headers, body)  // handler_context.rs:35
    → provider_router.select_providers(app_type)          // handler_context.rs:51
      → 如果 auto_failover: 返回 failover 队列中的 provider 列表
      → 否则: 返回当前 provider（单个）
    → 提取 request_model from body["model"]              // handler_context.rs:66-70
  → RequestForwarder::new(context.provider_router)        // handlers.rs:137
  → forwarder.forward_response_detailed(..., context.providers().to_vec(), ...)  // handlers.rs:166
    → 遍历 providers 列表，逐个尝试
```

### 目标流程（集成 ModelRouter 后）

```
Handler
  → HandlerContext::load(state, app_type, headers, body)
    → 提取 request_model from body["model"]
    → CHECK model_router.match_route(app_type, request_model)  // 新增
      → 匹配成功: providers = [matched_provider]（单 provider，无 failover）
      → 匹配失败: providers = provider_router.select_providers(app_type)（回退现有逻辑）
  → RequestForwarder::new(context.provider_router)       // 不变
  → forwarder.forward_response_detailed(..., context.providers().to_vec(), ...)  // 不变
    → 单 provider 模式: 只尝试一次（成功/失败即返回）
    → 多 provider 模式: failover 逐个尝试（现有行为）
```

## 2. 关键集成点

### 2.1 ProxyServerState (server.rs:31-40)

当前字段：
```rust
pub struct ProxyServerState {
    pub db: Arc<Database>,
    pub config: Arc<RwLock<ProxyConfig>>,
    pub status: Arc<RwLock<ProxyStatus>>,
    pub start_time: Arc<RwLock<Option<Instant>>>,
    pub current_providers: Arc<RwLock<HashMap<String, (String, String)>>>,
    pub provider_router: Arc<ProviderRouter>,
    pub codex_chat_history: Arc<CodexChatHistoryStore>,
    pub gemini_shadow: Arc<GeminiShadowStore>,
}
```

需要新增：`pub model_router: Arc<ModelRouter>`

ModelRouter 创建时机：在 ProxyService 启动时（services/proxy.rs 中的 start 方法），创建 ProxyServerState 时同时创建 ModelRouter。

### 2.2 HandlerContext (handler_context.rs:18-32)

当前字段需要新增：
```rust
pub model_router: Arc<ModelRouter>,     // 新增
pub route_source: Option<String>,       // 新增，用于日志/调试
```

`load()` 方法的修改（handler_context.rs:35-88）：
- 在提取 `request_model` 之后（line 70）
- 调用 `state.model_router.match_route(app_type.as_str(), &request_model)`
- 匹配成功：绕过 `provider_router.select_providers()`，使用匹配的 provider
- 匹配失败：回退到现有 `select_providers()` 逻辑
- 设置 `route_source` 字段（匹配成功时记录 pattern）
- **重要**: `current_provider_id_at_start` 仍需在 route 匹配之前获取（与现有行为一致）

### 2.3 RequestForwarder (forwarder.rs)

**无需修改**。Forwarder 已经接受 `providers: Vec<Provider>` 参数，单 provider 或多 provider 都由 HandlerContext 传入。

### 2.4 代理模块注册 (proxy/mod.rs)

新增：`pub mod model_router;`

### 2.5 ProxyService 启动 (services/proxy.rs)

在 ProxyService 启动函数中创建 `ModelRouter` 实例并注入到 `ProxyServerState`。

## 3. ModelRouter 引擎设计

### 通配符匹配逻辑

```
pattern "*sonnet*"  → regex "(?i).*sonnet.*" 
pattern "claude-*"  → regex "(?i)claude-.*"
pattern "*-4-5"     → regex "(?i).*-4-5"
pattern "exact"     → 精确匹配（不转 regex）
```

- 仅 `*` 是通配符，其他字符按字面匹配
- 大小写不敏感
- 多个匹配规则时，取 priority 最小的

### ModelRouter 结构

```rust
pub struct ModelRouter {
    db: Arc<Database>,
}

impl ModelRouter {
    pub fn new(db: Arc<Database>) -> Self { Self { db } }
    
    pub async fn match_route(&self, app_type: &str, model: &str) 
        -> Result<Option<Provider>, ProxyError>
}
```

## 4. 上游 PR (#4081) 对比

上游 PR 的文件结构（供参考）：
- `proxy/model_router.rs` — 新建 202 行
- `proxy/handler_context.rs` — +150/-12 行
- `proxy/forwarder.rs` — +38/-10 行
- `proxy/server.rs` — +5 行

cc-switch-cli 的差异：
- handler_context 结构类似但字段名可能不同
- forwarder 签名可能不同（需仔细对比）
- server.rs 中 ProxyServerState 字段不同

## 5. 服务层集成

### proxy_service.rs 中的启动流程

需要在 ProxyService::start() 方法中（约 line 463）创建 ModelRouter 并注入到 ProxyServerState。

查看现有模式：
```rust
let provider_router = Arc::new(ProviderRouter::new(db.clone()));
let state = ProxyServerState {
    db: db.clone(),
    provider_router,
    // ... other fields
};
```

需要类似地创建：
```rust
let model_router = Arc::new(ModelRouter::new(db.clone()));
let state = ProxyServerState {
    db: db.clone(),
    provider_router,
    model_router,       // 新增
    // ... other fields
};
```

## 6. 测试策略

### ModelRouter 单元测试
- 精确匹配测试
- 通配符 `*sonnet*` 匹配
- 通配符 `claude-*` 匹配
- 通配符 `*-4-5` 匹配
- 多规则优先级测试（priority 小的优先）
- enabled=false 规则被跳过
- 无匹配返回 None
- 大小写不敏感匹配
- 特殊字符（regex 元字符在 pattern 中视为字面量）

### 代理集成测试
- 匹配规则 → router-selected provider 被使用
- 无匹配规则 → fallback 到 ProviderRouter
- 空路由表 → 行为不变

## 7. 需要处理的边界情况

1. **model 字段缺失**: body 中没有 "model" 字段 → `request_model = "unknown"` → 回退到 ProviderRouter
2. **路由指向的 provider 不存在**: ModelRoute 指向的 provider_id 已被删除 → 记录 warning 日志，回退到 ProviderRouter（外键级联删除已处理大部分情况）
3. **并发路由更新**: 请求处理过程中路由表被修改 → 每次请求都从 DB 重新读取（不使用缓存）
4. **app_type 过滤**: 只匹配与请求 app_type 相同的路由规则

## 8. 文件变更清单

| 文件 | 变更类型 | 估计行数 |
|------|----------|----------|
| `proxy/model_router.rs` | **新建** | ~180 |
| `proxy/mod.rs` | 修改 | +1 |
| `proxy/handler_context.rs` | 修改 | ~+60/-5 |
| `proxy/server.rs` | 修改 | +2 |
| `services/proxy.rs` | 修改 | +3 |
| `lib.rs` | 修改 | +1 |
| `proxy/handler_context.rs` (tests) | 修改 | ~+80 |
| `proxy/forwarder.rs` | **无需修改** | 0 |
