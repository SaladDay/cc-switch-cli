use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchedModel {
    pub id: String,
    #[serde(rename = "owned_by", skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,
}
