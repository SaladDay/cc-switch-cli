//! Small, pure values shared by database publication and Skills recovery.
//!
//! Restore paths are derived exclusively from [`RestoreOperationId`]. The
//! database persists only these bounded values; it never accepts a filesystem
//! path from an imported database or an external journal.

use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

pub(crate) const RESTORE_INTENT_KEY: &str = "restore_intent_v1";
pub(crate) const RESTORE_GENERATION_KEY: &str = "restore_generation_v1";
pub(crate) const RESTORE_OPERATION_ID_KEY: &str = "restore_operation_id_v1";
pub(crate) const RESTORE_POSTCOMMIT_KEY: &str = "restore_postcommit_v1";
pub(crate) const RESTORE_SKILLS_MODE_KEY: &str = "restore_skills_mode_v1";
pub(crate) const RESTORE_PENDING_RETRY_KEY: &str = "restore_projection_pending_v1";
pub(crate) const LEGACY_RESTORE_PUBLICATION_TOKEN_KEY: &str = "restore_publication_token_v1";
pub(crate) const SKILLS_GENERATION_MARKER: &str = ".cc-switch-restore-generation";

pub(crate) const RESERVED_RESTORE_SETTING_KEYS: &[&str] = &[
    RESTORE_INTENT_KEY,
    RESTORE_GENERATION_KEY,
    RESTORE_OPERATION_ID_KEY,
    RESTORE_POSTCOMMIT_KEY,
    RESTORE_SKILLS_MODE_KEY,
    RESTORE_PENDING_RETRY_KEY,
    LEGACY_RESTORE_PUBLICATION_TOKEN_KEY,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RestoreOperationId(Uuid);

impl RestoreOperationId {
    pub(crate) fn fresh() -> Self {
        Self(Uuid::new_v4())
    }

    pub(crate) fn parse(value: &str) -> Result<Self, AppError> {
        let parsed = Uuid::parse_str(value).map_err(|error| {
            AppError::InvalidInput(format!("invalid restore operation id {value:?}: {error}"))
        })?;
        let operation = Self(parsed);
        if operation.to_string() != value {
            return Err(AppError::InvalidInput(format!(
                "restore operation id is not in canonical UUID form: {value:?}"
            )));
        }
        Ok(operation)
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Self {
        Self::parse(value).expect("valid canonical restore operation UUID")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RestoreSkillsMode {
    Preserve,
    Replace,
}

impl RestoreSkillsMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::Replace => "replace",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, AppError> {
        match value {
            "preserve" => Ok(Self::Preserve),
            "replace" => Ok(Self::Replace),
            _ => Err(AppError::InvalidInput(format!(
                "invalid restore Skills mode {value:?}"
            ))),
        }
    }

    pub(crate) fn replaces_skills(self) -> bool {
        self == Self::Replace
    }
}

impl fmt::Display for RestoreOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0.hyphenated())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRestoreIntent {
    operation_id: String,
    skills_mode: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RestoreIntent {
    pub(crate) operation_id: RestoreOperationId,
    pub(crate) skills_mode: RestoreSkillsMode,
}

impl RestoreIntent {
    pub(crate) fn encode(self) -> Result<String, AppError> {
        serde_json::to_string(&StoredRestoreIntent {
            operation_id: self.operation_id.to_string(),
            skills_mode: self.skills_mode.as_str().to_string(),
        })
        .map_err(|source| AppError::JsonSerialize { source })
    }

    pub(crate) fn decode(value: &str) -> Result<Self, AppError> {
        let stored: StoredRestoreIntent = serde_json::from_str(value).map_err(|error| {
            AppError::InvalidInput(format!("invalid persisted restore intent: {error}"))
        })?;
        Ok(Self {
            operation_id: RestoreOperationId::parse(&stored.operation_id)?,
            skills_mode: RestoreSkillsMode::parse(&stored.skills_mode)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{RestoreIntent, RestoreOperationId, RestoreSkillsMode};

    const OPERATION: &str = "00112233-4455-4677-8899-aabbccddeeff";

    #[test]
    fn intent_roundtrip_contains_no_path_material() {
        let intent = RestoreIntent {
            operation_id: RestoreOperationId::for_test(OPERATION),
            skills_mode: RestoreSkillsMode::Replace,
        };
        let encoded = intent.encode().expect("encode restore intent");
        assert_eq!(
            RestoreIntent::decode(&encoded).expect("decode intent"),
            intent
        );
        assert!(!encoded.contains('/'));
        assert!(!encoded.contains('\\'));
    }

    #[test]
    fn operation_id_requires_canonical_uuid_text() {
        assert!(RestoreOperationId::parse("../../escape").is_err());
        assert!(RestoreOperationId::parse("00112233445546778899aabbccddeeff").is_err());
        assert!(RestoreOperationId::parse(OPERATION).is_ok());
    }

    #[test]
    fn intent_rejects_unknown_path_field() {
        let hostile = format!(
            r#"{{"operation_id":"{OPERATION}","skills_mode":"replace","skills_old":"/tmp/escape"}}"#
        );
        assert!(RestoreIntent::decode(&hostile).is_err());
    }
}
