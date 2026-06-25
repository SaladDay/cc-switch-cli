//! 供应商连通性检查服务（reachability）
//!
//! 仅探测供应商 `base_url` 是否可达，不发送真实大模型请求。
//! 收到任意 HTTP 响应即判定可达，只有 DNS、连接、TLS、超时等网络级错误判定失败。

mod provider_extract;
mod service;
#[cfg(test)]
mod tests;
mod types;

pub use service::StreamCheckService;
pub use types::{HealthStatus, StreamCheckConfig, StreamCheckResult};
