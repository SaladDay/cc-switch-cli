//! Deployment baselines and opt-in requirements for sharing one native Skill.

use super::*;
use serde_json::Value;

const METHODS: [SyncMethod; 3] = [SyncMethod::Auto, SyncMethod::Copy, SyncMethod::Symlink];

#[derive(Debug, PartialEq, Eq)]
struct Deployment {
    link: Option<PathBuf>,
    content: Option<(Vec<u8>, Vec<u8>)>,
}

fn destination(home: &Path, app: &AppType) -> PathBuf {
    let path = SkillService::get_app_skills_dir(app)
        .unwrap()
        .join("cli-skill");
    assert!(
        path.starts_with(home),
        "native path escaped the fixture: {path:?}"
    );
    path
}

fn observe(path: &Path) -> Option<Deployment> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => panic!("cannot inspect {path:?}: {error}"),
    };
    let link = metadata
        .file_type()
        .is_symlink()
        .then(|| fs::read_link(path).unwrap());
    assert!(link.is_some() || metadata.is_dir());
    let content = match fs::read(path.join("SKILL.md")) {
        Ok(manifest) => Some((manifest, fs::read(path.join("assets/data.bin")).unwrap())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && link.is_some() => None,
        Err(error) => panic!("cannot read deployed Skill at {path:?}: {error}"),
    };
    Some(Deployment { link, content })
}

fn assert_selected(db: &Database, app: &AppType, enabled: bool) {
    let skill = db.get_installed_skill("cli-skill").unwrap().unwrap();
    assert_eq!(
        skill.apps,
        if enabled {
            SkillApps::only(app)
        } else {
            SkillApps::default()
        }
    );
}

#[test]
fn cli_modes_round_trip_existing_copies_and_links_without_editing_native_controls() {
    for app in [
        AppType::Claude,
        AppType::Gemini,
        AppType::Hermes,
        AppType::Pi,
    ] {
        for method in METHODS {
            for existing_copy in [false, true] {
                let home = tempfile::tempdir().unwrap();
                let _env = TestEnvGuard::isolated(home.path());
                let _pi = crate::pi_config::test_support::TestAgentDir::at(
                    &home.path().join(".pi/agent"),
                );
                let db = fixture(
                    home.path(),
                    if existing_copy {
                        SyncMethod::Copy
                    } else {
                        method
                    },
                );
                let before = rows(&db);
                let path = destination(home.path(), &app);
                let source = SkillService::get_ssot_dir().unwrap().join("cli-skill");
                let gemini = crate::gemini_config::get_gemini_settings_path();
                let gemini_before = fs::read(&gemini).unwrap();
                let hermes = home.path().join(".hermes/config.yaml");
                fs::create_dir_all(hermes.parent().unwrap()).unwrap();
                fs::write(&hermes, "fixture: true\nskills:\n  disabled: [cli-skill]\n").unwrap();
                let hermes_before = fs::read(&hermes).unwrap();
                let assert_controls = || {
                    assert_eq!(fs::read(&gemini).unwrap(), gemini_before);
                    assert_eq!(fs::read(&hermes).unwrap(), hermes_before);
                };

                SkillService::toggle_app("cli-skill", &app, true).unwrap();
                assert_controls();
                let first = observe(&path).unwrap();
                assert_eq!(
                    first.content,
                    Some((b"# Fixture\n".to_vec(), vec![0, 255, 128]))
                );
                let first_copied = existing_copy
                    || method == SyncMethod::Copy
                    || (method == SyncMethod::Auto && first.link.is_none());
                assert_eq!(first.link, (!first_copied).then_some(source.clone()));
                assert_selected(&db, &app, true);
                let expected = if existing_copy && app != AppType::Pi {
                    let updated = (b"# Updated fixture\n".to_vec(), vec![7, 255, 42]);
                    fs::write(source.join("SKILL.md"), &updated.0).unwrap();
                    fs::write(source.join("assets/data.bin"), &updated.1).unwrap();
                    assert_eq!(observe(&path).as_ref(), Some(&first));
                    updated
                } else {
                    (b"# Fixture\n".to_vec(), vec![0, 255, 128])
                };
                let copied = method == SyncMethod::Copy
                    || (method == SyncMethod::Auto && first.link.is_none());
                SkillService::set_sync_method(method).unwrap();
                SkillService::toggle_app("cli-skill", &app, true).unwrap();
                assert_controls();
                let refreshed = observe(&path).unwrap();
                assert_eq!(refreshed.content.as_ref(), Some(&expected));
                assert_eq!(refreshed.link, (!copied).then_some(source.clone()));
                assert_selected(&db, &app, true);
                for _ in 0..2 {
                    SkillService::toggle_app("cli-skill", &app, false).unwrap();
                    assert_controls();
                    assert!(observe(&path).is_none());
                    assert_selected(&db, &app, false);
                }
                assert_eq!(fs::read(source.join("SKILL.md")).unwrap(), expected.0);
                assert_eq!(
                    fs::read(source.join("assets/data.bin")).unwrap(),
                    expected.1
                );
                assert_unowned(&db, &before);
            }
        }
    }
}

#[test]
fn cli_missing_source_refuses_enable_and_preserves_pi_removal_policy() {
    for app in [AppType::Claude, AppType::Pi] {
        for method in METHODS {
            let home = tempfile::tempdir().unwrap();
            let _env = TestEnvGuard::isolated(home.path());
            let _pi =
                crate::pi_config::test_support::TestAgentDir::at(&home.path().join(".pi/agent"));
            let db = fixture(home.path(), method);
            SkillService::toggle_app("cli-skill", &app, true).unwrap();
            let path = destination(home.path(), &app);
            let source = SkillService::get_ssot_dir().unwrap().join("cli-skill");
            let retained = home.path().join("retained-source");
            fs::rename(&source, &retained).unwrap();
            let before = observe(&path);
            let catalog = rows(&db);
            let error = SkillService::toggle_app("cli-skill", &app, true).unwrap_err();
            assert!(
                matches!(&error, AppError::Message(message) if message.contains("SSOT")),
                "expected missing source: {error}"
            );
            assert_eq!(observe(&path), before);
            assert_eq!(rows(&db), catalog);
            let removal = SkillService::toggle_app("cli-skill", &app, false);
            if app == AppType::Pi {
                assert!(matches!(removal, Err(AppError::InvalidInput(_))));
                assert_eq!(observe(&path), before);
                assert_eq!(rows(&db), catalog);
            } else {
                removal.unwrap();
                assert!(observe(&path).is_none());
                assert_selected(&db, &app, false);
            }
            assert_eq!(
                fs::read(retained.join("assets/data.bin")).unwrap(),
                [0, 255, 128]
            );
            assert_unowned(&db, &catalog);
        }
    }
}

#[test]
fn cli_pi_toggle_preserves_an_externally_modified_copy() {
    for method in METHODS {
        for enabled in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let _env = TestEnvGuard::isolated(home.path());
            let _pi =
                crate::pi_config::test_support::TestAgentDir::at(&home.path().join(".pi/agent"));
            let db = fixture(home.path(), SyncMethod::Copy);
            SkillService::toggle_app("cli-skill", &AppType::Pi, true).unwrap();
            SkillService::set_sync_method(method).unwrap();
            let path = destination(home.path(), &AppType::Pi);
            fs::write(path.join("assets/data.bin"), [7, 255, 42]).unwrap();
            let before = observe(&path);
            let catalog = rows(&db);
            assert!(matches!(
                SkillService::toggle_app("cli-skill", &AppType::Pi, enabled),
                Err(AppError::InvalidInput(_))
            ));
            assert_eq!(observe(&path), before);
            assert_eq!(rows(&db), catalog);
            assert_eq!(
                fs::read(
                    SkillService::get_ssot_dir()
                        .unwrap()
                        .join("cli-skill/assets/data.bin")
                )
                .unwrap(),
                [0, 255, 128]
            );
        }
    }
}

fn peer(home: &Path, action: &str) -> Value {
    LitePeer::start(home, &format!("deployment:claude:{action}")).finish();
    let result: Value =
        serde_json::from_slice(&fs::read(home.join("lite-skill-deployment.json")).unwrap())
            .unwrap();
    assert!(result
        .as_object()
        .is_some_and(|value| value.contains_key("error")));
    assert_eq!(result["state"]["app"], "claude");
    assert_eq!(
        fs::read(home.join(".cc-switch/skills/cli-skill/assets/data.bin")).unwrap(),
        [0, 255, 128]
    );
    if result["error"].is_null() {
        let enabled = match action {
            "enable" => true,
            "disable" => false,
            _ => panic!("unexpected peer action: {action}"),
        };
        assert_eq!(result["state"]["selected"], enabled);
        assert_eq!(result["state"]["enabled"], enabled);
    }
    result
}

fn claude_deployment(db: &Database, home: &Path, copied: bool) -> Deployment {
    let path = destination(home, &AppType::Claude);
    let current = observe(&path).unwrap();
    assert_eq!(
        current.content,
        Some((b"# Fixture\n".to_vec(), vec![0, 255, 128]))
    );
    assert_eq!(current.link.is_none(), copied);
    if !copied {
        assert_eq!(
            fs::canonicalize(&path).unwrap(),
            fs::canonicalize(home.join(".cc-switch/skills/cli-skill")).unwrap()
        );
    }
    assert_selected(db, &AppType::Claude, true);
    current
}

fn require_peer(result: &Value, context: &str, failures: &mut Vec<String>) -> bool {
    if result["error"].is_null() {
        true
    } else {
        failures.push(format!("{context}: {result}"));
        false
    }
}

#[test]
#[ignore = "requires the independently built Lite library test binary; shared deployment adoption gate"]
fn cli_created_skill_can_be_disabled_and_reenabled_by_lite_without_changing_copy_mode() {
    let mut failures = Vec::new();
    for method in METHODS {
        let home = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(home.path());
        let db = fixture(home.path(), method);
        let catalog = rows(&db);
        SkillService::toggle_app("cli-skill", &AppType::Claude, true).unwrap();
        let path = destination(home.path(), &AppType::Claude);
        let copied = method == SyncMethod::Copy
            || (method == SyncMethod::Auto && observe(&path).unwrap().link.is_none());
        let initial = claude_deployment(&db, home.path(), copied);
        let selected = rows(&db);
        let result = peer(home.path(), "disable");
        assert_unowned(&db, &catalog);
        if !require_peer(
            &result,
            &format!("CLI {method:?} -> Lite disable"),
            &mut failures,
        ) {
            assert_eq!(rows(&db), selected);
            assert_eq!(observe(&path), Some(initial));
            continue;
        }
        assert_selected(&db, &AppType::Claude, false);
        assert!(!path.join("SKILL.md").exists());
        for _ in 0..2 {
            let result = peer(home.path(), "enable");
            assert!(require_peer(&result, "Lite reenable", &mut failures));
            claude_deployment(&db, home.path(), copied);
        }
        assert_unowned(&db, &catalog);
    }
    assert!(
        failures.is_empty(),
        "shared deployment failures: {failures:#?}"
    );
}

#[test]
#[ignore = "requires the independently built Lite library test binary; shared deployment adoption gate"]
fn lite_created_skill_remains_operable_after_cli_touches_enabled_or_disabled_reference() {
    let mut failures = Vec::new();
    for method in METHODS {
        for initially_enabled in [true, false] {
            let home = tempfile::tempdir().unwrap();
            let _env = TestEnvGuard::isolated(home.path());
            let db = fixture(home.path(), method);
            let catalog = rows(&db);
            assert_eq!(peer(home.path(), "enable")["error"], Value::Null);
            let initial = claude_deployment(&db, home.path(), false);
            let path = destination(home.path(), &AppType::Claude);
            if !initially_enabled {
                assert_eq!(peer(home.path(), "disable")["error"], Value::Null);
                assert_eq!(
                    observe(&path),
                    Some(Deployment {
                        link: initial.link,
                        content: None,
                    })
                );
                assert_selected(&db, &AppType::Claude, false);
            }
            SkillService::toggle_app("cli-skill", &AppType::Claude, true).unwrap();
            let copied = method == SyncMethod::Copy;
            let deployed = claude_deployment(&db, home.path(), copied);
            let selected = rows(&db);
            let result = peer(home.path(), "disable");
            assert_unowned(&db, &catalog);
            if !require_peer(
                &result,
                &format!("Lite enabled={initially_enabled} -> CLI {method:?} -> Lite disable"),
                &mut failures,
            ) {
                assert_eq!(rows(&db), selected);
                assert_eq!(observe(&path), Some(deployed));
                continue;
            }
            assert_selected(&db, &AppType::Claude, false);
            assert!(!path.join("SKILL.md").exists());
            assert_eq!(peer(home.path(), "enable")["error"], Value::Null);
            claude_deployment(&db, home.path(), copied);
            SkillService::toggle_app("cli-skill", &AppType::Claude, false).unwrap();
            assert!(!path.join("SKILL.md").exists());
            assert_selected(&db, &AppType::Claude, false);
            assert_eq!(peer(home.path(), "enable")["error"], Value::Null);
            claude_deployment(&db, home.path(), copied);
            assert_unowned(&db, &catalog);
        }
    }
    assert!(
        failures.is_empty(),
        "shared deployment failures: {failures:#?}"
    );
}
