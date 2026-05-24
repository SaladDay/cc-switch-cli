use std::str::FromStr;

use cc_switch_lib::AppType;

#[test]
fn parse_known_apps_case_insensitive_and_trim() {
    assert!(matches!(AppType::from_str("claude"), Ok(AppType::Claude)));
    assert!(matches!(AppType::from_str("codex"), Ok(AppType::Codex)));
    assert!(matches!(AppType::from_str("hermes"), Ok(AppType::Hermes)));
    assert!(matches!(
        AppType::from_str("openclaw"),
        Ok(AppType::OpenClaw)
    ));
    assert!(matches!(
        AppType::from_str(" ClAuDe \n"),
        Ok(AppType::Claude)
    ));
    assert!(matches!(AppType::from_str("\tcoDeX\t"), Ok(AppType::Codex)));
    assert!(matches!(
        AppType::from_str(" HeRmEs\t"),
        Ok(AppType::Hermes)
    ));
    assert!(matches!(
        AppType::from_str("\nOpenClaw\t"),
        Ok(AppType::OpenClaw)
    ));
}

#[test]
fn openclaw_is_listed_and_uses_additive_mode() {
    assert!(AppType::all().any(|app| app == AppType::OpenClaw));
    assert!(AppType::OpenClaw.is_additive_mode());
}

#[test]
fn hermes_is_listed_and_uses_additive_mode() {
    assert!(AppType::all().any(|app| app == AppType::Hermes));
    assert!(AppType::Hermes.is_additive_mode());
}

#[test]
fn parse_unknown_app_returns_localized_error_message() {
    let err = AppType::from_str("unknown").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("可选值") || msg.contains("Allowed"));
    assert!(msg.contains("unknown"));
}
