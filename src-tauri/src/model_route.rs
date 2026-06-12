//! 模型路由类型定义 (Model Route type definition)
//!
//! 定义 per-model provider routing 的数据结构，用于根据模型名称模式
//! 将代理请求路由到不同的 provider。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRoute {
    pub id: String,
    pub app_type: String,
    pub pattern: String,
    pub provider_id: String,
    pub priority: i32,
    pub enabled: bool,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_route_serialization_roundtrip_camelcase() {
        let route = ModelRoute {
            id: "test-id-001".into(),
            app_type: "claude".into(),
            pattern: "*-sonnet".into(),
            provider_id: "test-prov".into(),
            priority: 10,
            enabled: true,
            created_at: Some("2025-01-01 00:00:00".into()),
            updated_at: Some("2025-01-01 00:00:00".into()),
        };

        let json = serde_json::to_string(&route).expect("serialize");
        assert!(json.contains("\"appType\""), "camelCase: {}", json);
        assert!(json.contains("\"providerId\""), "camelCase: {}", json);
        assert!(json.contains("\"createdAt\""), "camelCase: {}", json);
        assert!(json.contains("\"updatedAt\""), "camelCase: {}", json);

        let deserialized: ModelRoute = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.id, "test-id-001");
        assert_eq!(deserialized.created_at, Some("2025-01-01 00:00:00".into()));
        assert_eq!(deserialized.updated_at, Some("2025-01-01 00:00:00".into()));
    }
}
