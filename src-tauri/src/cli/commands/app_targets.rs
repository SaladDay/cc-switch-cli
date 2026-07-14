use std::str::FromStr;

use crate::app_config::AppType;
use crate::error::AppError;

pub(crate) fn supported_app_target_labels() -> &'static str {
    "claude, codex, gemini, opencode, hermes"
}

pub(crate) fn app_targets_or_default(
    raw_targets: &[String],
    fallback: AppType,
    feature: &str,
) -> Result<Vec<AppType>, AppError> {
    if raw_targets.is_empty() {
        return parse_app_targets(&[fallback.as_str().to_string()], feature);
    }

    parse_app_targets(raw_targets, feature)
}

pub(crate) fn parse_app_targets(
    raw_targets: &[String],
    feature: &str,
) -> Result<Vec<AppType>, AppError> {
    let mut targets = Vec::new();

    for raw in raw_targets {
        for value in raw.split(',') {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }

            let app = parse_app_target(value, feature)?;
            if !targets.contains(&app) {
                targets.push(app);
            }
        }
    }

    if targets.is_empty() {
        return Err(AppError::InvalidInput(format!(
            "Please provide at least one app. Supported apps: {}",
            supported_app_target_labels()
        )));
    }

    Ok(targets)
}

fn parse_app_target(value: &str, feature: &str) -> Result<AppType, AppError> {
    let normalized = value.trim().to_lowercase().replace('-', "");
    let app = AppType::from_str(&normalized).map_err(|_| {
        AppError::InvalidInput(format!(
            "Unsupported app id: '{value}'. Supported apps: {}",
            supported_app_target_labels()
        ))
    })?;

    if matches!(app, AppType::OpenClaw) {
        return Err(AppError::InvalidInput(format!(
            "{feature} does not support openclaw yet. Supported apps: {}",
            supported_app_target_labels()
        )));
    }

    Ok(app)
}

pub(crate) fn app_target_names(apps: &[AppType]) -> String {
    apps.iter()
        .map(AppType::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_app_targets_accepts_backend_ids_and_aliases() {
        let apps = parse_app_targets(
            &["claude,codex".to_string(), "open-code".to_string()],
            "MCP",
        )
        .expect("apps should parse");

        assert_eq!(
            apps,
            vec![AppType::Claude, AppType::Codex, AppType::OpenCode]
        );
    }

    #[test]
    fn parse_app_targets_deduplicates_in_order() {
        let apps = parse_app_targets(&["codex".to_string(), "claude,codex".to_string()], "Skills")
            .expect("apps should parse");

        assert_eq!(apps, vec![AppType::Codex, AppType::Claude]);
    }

    #[test]
    fn parse_app_targets_rejects_openclaw() {
        let err = parse_app_targets(&["openclaw".to_string()], "MCP")
            .expect_err("openclaw should be rejected");

        assert!(
            err.to_string().contains("does not support openclaw"),
            "unexpected error: {err}"
        );
    }
}
