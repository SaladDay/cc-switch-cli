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
fn cli_catalog_preserves_cli_order_with_core_ids_and_modes() {
    let cli_apps = AppType::all().collect::<Vec<_>>();
    let cli_ids = cli_apps.iter().map(AppType::as_str).collect::<Vec<_>>();
    assert_eq!(
        cli_ids,
        vec!["claude", "codex", "gemini", "opencode", "hermes", "openclaw", "pi"]
    );
    assert_eq!(
        cli_apps
            .iter()
            .map(AppType::is_additive_mode)
            .collect::<Vec<_>>(),
        vec![false, false, false, true, true, true, true]
    );
    for app in cli_apps {
        let core = CoreAppType::from_str(app.as_str()).expect("CLI app must exist in Core");
        assert_eq!(app.as_str(), builtin_app_registry().for_app(&core).id());
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

    assert!(error.starts_with(
        "unknown variant `unknown`, expected one of `claude`, `codex`, `gemini`, `opencode`, `hermes`, `openclaw`, `pi`"
    ));
    assert!(serde_json::from_str::<AppType>("\" ClAuDe \"").is_err());
}

#[cfg(feature = "cli")]
#[test]
fn clap_app_values_preserve_cli_names_and_order() {
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
    assert_eq!(
        clap_ids,
        [
            "claude",
            "codex",
            "gemini",
            "open-code",
            "hermes",
            "open-claw",
            "pi",
        ]
    );
}

#[cfg(feature = "cli")]
#[test]
fn clap_preserves_compound_app_acceptance_contract() {
    use cc_switch_lib::cli::Cli;
    use clap::Parser;

    for (value, expected) in [
        ("open-code", AppType::OpenCode),
        ("open-claw", AppType::OpenClaw),
    ] {
        let cli = Cli::try_parse_from(["cc-switch", "--app", value])
            .expect("legacy Clap app value must parse");
        assert_eq!(cli.app, Some(expected));
    }

    for value in ["opencode", "openclaw"] {
        assert!(
            Cli::try_parse_from(["cc-switch", "--app", value]).is_err(),
            "Core storage ID must not widen the Clap input contract"
        );
    }
}
