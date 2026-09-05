use cc_switch_core::{
    builtin_app_adapter, codex::ProviderTableSyntax, AppType, CodexImportClassification,
    CodexImportPolicy, CodexImportPresence, CodexImportValidation, LiveDocumentSet, LogicalTarget,
    NativeImportCandidate, NativeImportError, NativeImportPolicy, NativeImportStep,
    ObservedDocument,
};
use serde_json::Value;
use std::{fs, path::Path};

use super::{get_codex_auth_path, read_and_validate_codex_config_text};
use crate::error::AppError;

// Match the CLI JSON reader's validation and diagnostics while retaining the
// original bytes. A Value -> JSON -> Value roundtrip can change opaque numbers.
fn read_auth_document(path: &Path) -> Result<Vec<u8>, AppError> {
    if !path.exists() {
        return Err(AppError::Config(format!("文件不存在: {}", path.display())));
    }
    let text = fs::read_to_string(path).map_err(|error| AppError::io(path, error))?;
    serde_json::from_str::<Value>(&text).map_err(|error| AppError::json(path, error))?;
    Ok(text.into_bytes())
}

// Paths, read order, parser grammar, and diagnostics belong to the CLI. The
// registered Core adapter owns import assembly, presence, and classification.
pub(super) fn read() -> Result<NativeImportCandidate, AppError> {
    let auth_path = get_codex_auth_path();
    let mut auth = if auth_path.exists() {
        Some(read_auth_document(&auth_path)?)
    } else {
        None
    };
    let config = read_and_validate_codex_config_text()?;
    let maximum_content_bytes = config.len().max(auth.as_ref().map_or(0, Vec::len));
    let mut config = Some(config.into_bytes());
    let adapter = builtin_app_adapter(&AppType::Codex);
    let documents = adapter.targets().iter().copied().map(|target| {
        let contents = match target {
            LogicalTarget::CodexAuth => auth.take(),
            LogicalTarget::CodexConfig => config.take(),
            _ => return ObservedDocument::unobserved(target),
        };
        contents.map_or_else(
            || ObservedDocument::missing(target),
            |contents| ObservedDocument::present(target, contents),
        )
    });
    let documents = LiveDocumentSet::try_new_with_content_limit(
        AppType::Codex,
        documents,
        maximum_content_bytes,
    )
    .map_err(|error| AppError::Message(format!("Codex native import failed: {error}")))?;
    let policy = NativeImportPolicy::Codex(CodexImportPolicy {
        validation: CodexImportValidation::HostValidated,
        presence: CodexImportPresence::AuthOrNonblankConfig,
        classification: CodexImportClassification::SnapshotPayload(ProviderTableSyntax::TablesOnly),
    });
    match adapter.project_native_import_with_policy(&documents, &policy) {
        Ok(NativeImportStep::Ready { mut candidates }) if candidates.len() == 1 => {
            Ok(candidates.remove(0))
        }
        Ok(_) => Err(AppError::Message(
            "Codex native import did not produce one provider".to_owned(),
        )),
        Err(NativeImportError::Missing { .. }) => Err(AppError::localized(
            "codex.live.missing",
            "Codex 配置文件不存在",
            "Codex configuration is missing",
        )),
        Err(error) => Err(AppError::Message(format!(
            "Codex native import failed: {error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        codex_config::{
            codex_auth_has_login_material, extract_codex_api_key, get_codex_config_path,
        },
        config::read_json_file,
        test_support::TestEnvGuard,
    };
    use cc_switch_core::NativeProviderMode;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    // Previous CLI reader, kept only as an oracle for input/error compatibility.
    fn previous_read() -> Result<Value, AppError> {
        let auth_path = get_codex_auth_path();
        let auth_present = auth_path.exists();
        let auth: Value = if auth_present {
            read_json_file(&auth_path)?
        } else {
            json!({})
        };
        let config = read_and_validate_codex_config_text()?;
        if !auth_present && config.trim().is_empty() {
            return Err(AppError::localized(
                "codex.live.missing",
                "Codex 配置文件不存在",
                "Codex configuration is missing",
            ));
        }
        Ok(json!({"auth":auth,"config":config}))
    }

    #[test]
    fn native_import_matches_previous_snapshots_and_auth_classification() {
        for (auth, config) in [
            (None, None),
            (None, Some("")),
            (None, Some("\u{a0}\n")),
            (Some("null"), Some("\u{a0}\n")),
            (Some("[1,2]"), None),
            (Some("false"), Some("# keep\nmodel = 'x'")),
            (Some(r#"{"last_refresh":"yesterday","future":{"keep":true}}"#), None),
            (Some(r#"{"tokens":{"access_token":"oauth"}}"#), Some("experimental_bearer_token = 'key'")),
            (Some(r#"{"last_refresh":"yesterday"}"#), Some("model_provider = 'v'\nmodel_providers = {v={experimental_bearer_token='inline'}}")),
            (Some(r#"{"last_refresh":"yesterday"}"#), Some("model_provider = 'v'\nexperimental_bearer_token = 'root'\n[model_providers.v]\nexperimental_bearer_token = ' '")),
            (Some(r#"{"last_refresh":"yesterday"}"#), Some("model_provider = 'OPENAI'\n[model_providers.OPENAI]\nexperimental_bearer_token = 'named'")),
            (None, Some("experimental_bearer_token = \"\\e\"")),
            (Some("{"), Some("invalid = [")),
            (Some("{}"), Some("invalid = [")),
        ] {
            let temp = TempDir::new().unwrap();
            let _env = TestEnvGuard::isolated(temp.path());
            fs::create_dir_all(get_codex_auth_path().parent().unwrap()).unwrap();
            if let Some(auth) = auth {
                fs::write(get_codex_auth_path(), auth).unwrap();
            }
            if let Some(config) = config {
                fs::write(get_codex_config_path(), config).unwrap();
            }
            match previous_read() {
                Ok(expected) => {
                    let imported = read().unwrap();
                    assert_eq!(imported.provider.settings, expected);
                    let official = expected.get("auth").is_some_and(codex_auth_has_login_material)
                        && extract_codex_api_key(expected.get("auth"), expected["config"].as_str()).is_none();
                    assert_eq!(imported.classification, Some(if official { NativeProviderMode::Official } else { NativeProviderMode::Custom }));
                }
                Err(expected) => assert_eq!(read().unwrap_err().to_string(), expected.to_string()),
            }
        }
    }

    #[test]
    fn native_import_keeps_large_documents_and_does_not_read_model_catalog() {
        let temp = TempDir::new().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        fs::create_dir_all(get_codex_auth_path().parent().unwrap()).unwrap();
        let config = format!(
            "# {}\nmodel = 'x'\n",
            "x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES)
        );
        let auth = json!({"future":"x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES)});
        fs::write(get_codex_auth_path(), serde_json::to_vec(&auth).unwrap()).unwrap();
        fs::write(get_codex_config_path(), &config).unwrap();
        fs::create_dir(crate::codex_config::get_codex_model_catalog_path()).unwrap();
        let imported = read().unwrap();
        assert_eq!(
            imported.provider.settings,
            json!({"auth":auth,"config":config})
        );
        assert_eq!(imported.provider.settings, previous_read().unwrap());
        assert_eq!(imported.classification, Some(NativeProviderMode::Official));
    }

    #[test]
    fn native_import_preserves_opaque_numeric_values() {
        let temp = TempDir::new().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        fs::create_dir_all(get_codex_auth_path().parent().unwrap()).unwrap();
        let numbers = (1..=300)
            .map(|index| format!("1.{}e-{index}", index * 1_234_567_u64))
            .collect::<Vec<_>>()
            .join(",");
        fs::write(get_codex_auth_path(), format!("{{\"future\":[{numbers}]}}")).unwrap();
        let actual = read().unwrap().provider.settings;
        let expected = previous_read().unwrap();
        for (index, (actual, expected)) in actual["auth"]["future"]
            .as_array()
            .unwrap()
            .iter()
            .zip(expected["auth"]["future"].as_array().unwrap())
            .enumerate()
        {
            assert_eq!(actual, expected, "opaque number at index {index}");
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn native_import_preserves_io_error_order() {
        let temp = TempDir::new().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        fs::create_dir_all(get_codex_auth_path()).unwrap();
        fs::write(get_codex_config_path(), "invalid = [").unwrap();
        assert_eq!(
            read().unwrap_err().to_string(),
            previous_read().unwrap_err().to_string()
        );
    }
}
