use std::str::FromStr;

use cc_switch_core::{builtin_app_registry, AppType as CoreAppType};
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

#[test]
fn cli_catalog_follows_core_order_ids_and_modes() {
    let cli_apps = AppType::all().collect::<Vec<_>>();
    let cli_ids = cli_apps.iter().map(AppType::as_str).collect::<Vec<_>>();
    let expected_ids = builtin_app_registry()
        .descriptors()
        .filter_map(|descriptor| match descriptor.app() {
            CoreAppType::ClaudeDesktop | CoreAppType::GrokBuild => None,
            _ => Some(descriptor.id()),
        })
        .collect::<Vec<_>>();

    assert_eq!(cli_ids, expected_ids);
    for app in cli_apps {
        let core = CoreAppType::from_str(app.as_str()).expect("CLI app must exist in Core");
        assert_eq!(app.is_additive_mode(), core.is_additive_mode());
        let encoded = serde_json::to_string(&app).expect("serialize CLI app");
        assert_eq!(
            encoded,
            serde_json::to_string(&core).expect("serialize Core app")
        );
        assert_eq!(
            serde_json::from_str::<AppType>(&encoded).expect("deserialize CLI app"),
            app,
        );
    }
}

#[test]
fn core_apps_without_cli_support_stay_unavailable() {
    assert!(AppType::from_str("claude-desktop").is_err());
    assert!(AppType::from_str("grokbuild").is_err());
    assert!(serde_json::from_str::<AppType>("\"claude-desktop\"").is_err());
    assert!(serde_json::from_str::<AppType>("\"grokbuild\"").is_err());
}

#[test]
fn serde_errors_only_list_cli_supported_apps() {
    let error = serde_json::from_str::<AppType>("\"unknown\"")
        .expect_err("unknown app must be rejected")
        .to_string();

    assert!(error.contains("unknown"));
    assert!(error.contains("claude"));
    assert!(!error.contains("claude-desktop"));
    assert!(!error.contains("grokbuild"));
    assert!(serde_json::from_str::<AppType>("\" ClAuDe \"").is_err());
}

#[cfg(feature = "cli")]
#[test]
fn clap_app_values_follow_core_order() {
    use clap::ValueEnum;

    let clap_ids = AppType::value_variants()
        .iter()
        .map(|app| {
            app.to_possible_value()
                .expect("CLI app must have a Clap value")
                .get_name()
                .to_string()
        })
        .collect::<Vec<_>>();
    let registry_ids = AppType::all()
        .map(|app| app.as_str().to_string())
        .collect::<Vec<_>>();

    assert_eq!(clap_ids, registry_ids);
}

#[cfg(feature = "cli")]
#[test]
fn clap_accepts_core_ids_and_legacy_compound_aliases() {
    use cc_switch_lib::cli::Cli;
    use clap::Parser;

    for (value, expected) in [
        ("opencode", AppType::OpenCode),
        ("open-code", AppType::OpenCode),
        ("openclaw", AppType::OpenClaw),
        ("open-claw", AppType::OpenClaw),
    ] {
        let cli = Cli::try_parse_from(["cc-switch", "--app", value])
            .expect("canonical IDs and legacy Clap names must parse");
        assert_eq!(cli.app, Some(expected));
    }
}
