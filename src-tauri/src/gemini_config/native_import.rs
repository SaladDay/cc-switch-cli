use cc_switch_core::{
    builtin_app_adapter, AppType, LiveDocumentSet, LogicalTarget, NativeImportPolicy,
    NativeImportStep, ObservedDocument,
};
use serde_json::Value;
use std::fs;

use super::{get_gemini_settings_path, read_gemini_env_source};
use crate::error::AppError;

#[cfg(test)]
#[path = "native_import_tests.rs"]
mod tests;

// Callers retain their missing-.env errors. Keep the existing env-before-settings
// read order and path-aware diagnostics; Core owns the native snapshot assembly.
pub(crate) fn read() -> Result<Value, AppError> {
    let env = read_gemini_env_source()?.into_bytes();
    let path = get_gemini_settings_path();
    let settings = if path.exists() {
        // Match read_json_file, retaining the original bytes rather than doing a
        // JSON round trip that can change opaque floating-point values.
        if !path.exists() {
            return Err(AppError::Config(format!("文件不存在: {}", path.display())));
        }
        let text = fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?;
        serde_json::from_str::<Value>(&text).map_err(|error| AppError::json(&path, error))?;
        Some(text.into_bytes())
    } else {
        None
    };
    // The previous trusted-local reader had no document-size limit.
    let limit = env.len().max(settings.as_ref().map_or(0, Vec::len));
    let documents = LiveDocumentSet::try_new_with_content_limit(
        AppType::Gemini,
        [
            ObservedDocument::present(LogicalTarget::GeminiEnv, env),
            settings.map_or_else(
                || ObservedDocument::missing(LogicalTarget::GeminiSettings),
                |bytes| ObservedDocument::present(LogicalTarget::GeminiSettings, bytes),
            ),
        ],
        limit,
    )
    .map_err(|error| AppError::Message(format!("Gemini native import failed: {error}")))?;
    match builtin_app_adapter(&AppType::Gemini)
        .project_native_import_with_policy(&documents, &NativeImportPolicy::GeminiEnvSnapshot)
    {
        Ok(NativeImportStep::Ready { mut candidates }) if candidates.len() == 1 => {
            Ok(candidates.remove(0).provider.settings)
        }
        Ok(_) => Err(AppError::Message(
            "Gemini native import did not produce one provider".to_owned(),
        )),
        Err(error) => Err(AppError::Message(format!(
            "Gemini native import failed: {error}"
        ))),
    }
}
