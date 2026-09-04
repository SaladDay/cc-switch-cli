use std::str::FromStr;

use crate::app_config::AppType;
use crate::error::AppError;

pub(crate) fn supported_app_target_labels() -> String {
    supported_app_target_labels_for("Skills")
}

fn supported_app_target_labels_for(feature: &str) -> String {
    AppType::all()
        .filter(|app| app_supports_target_feature(app, feature))
        .map(|app| app.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn app_supports_target_feature(app: &AppType, feature: &str) -> bool {
    if feature.eq_ignore_ascii_case("MCP") {
        app.supports_mcp()
    } else {
        app.supports_skills()
    }
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
            supported_app_target_labels_for(feature)
        )));
    }

    Ok(targets)
}

fn parse_app_target(value: &str, feature: &str) -> Result<AppType, AppError> {
    let normalized = value.trim().to_lowercase().replace('-', "");
    let app = AppType::from_str(&normalized).map_err(|_| {
        AppError::InvalidInput(format!(
            "Unsupported app id: '{value}'. Supported apps: {}",
            supported_app_target_labels_for(feature)
        ))
    })?;

    if !app_supports_target_feature(&app, feature) {
        let qualifier = if matches!(app, AppType::OpenClaw) {
            " yet"
        } else {
            ""
        };
        return Err(AppError::InvalidInput(format!(
            "{feature} does not support {}{qualifier}. Supported apps: {}",
            app.as_str(),
            supported_app_target_labels_for(feature)
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

    #[test]
    fn parse_app_targets_accepts_pi_for_skills_but_not_mcp() {
        assert_eq!(
            parse_app_targets(&["pi".to_string()], "Skills").expect("Pi skills target"),
            vec![AppType::Pi]
        );
        let error = parse_app_targets(&["pi".to_string()], "MCP")
            .expect_err("Pi must not be an MCP target");
        assert!(error.to_string().contains("does not support pi"));
    }

    #[test]
    fn target_labels_follow_cli_owned_capability_subsets() {
        assert_eq!(
            supported_app_target_labels_for("MCP"),
            "claude, codex, gemini, opencode, hermes"
        );
        assert_eq!(
            supported_app_target_labels(),
            "claude, codex, gemini, opencode, hermes, pi"
        );
    }

    #[test]
    fn target_admission_follows_cli_owned_capability_subsets() {
        for feature in ["MCP", "Skills"] {
            for app in AppType::all() {
                let expected = app_supports_target_feature(&app, feature);
                let app_id = app.as_str();
                let accepted = parse_app_targets(&[app_id.to_string()], feature).is_ok();

                assert_eq!(
                    accepted, expected,
                    "{app_id} admission must follow its {feature} capability"
                );
            }
        }
    }
}
