//! Real-consumer acceptance gates, not a replacement Skill implementation.

mod deployment;

use super::*;
use crate::test_support::TestEnvGuard;
use std::{
    cell::RefCell,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

thread_local! {
    static OBSERVATION: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}

pub(super) fn after_observation() {
    let hook = OBSERVATION.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

fn with_observation<T>(hook: impl FnOnce() + 'static, action: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            OBSERVATION.with(|slot| *slot.borrow_mut() = None);
        }
    }
    OBSERVATION.with(|slot| assert!(slot.borrow_mut().replace(Box::new(hook)).is_none()));
    let _reset = Reset;
    action()
}

struct LitePeer(Child);

impl LitePeer {
    fn start(home: &Path, mode: &str) -> Self {
        let binary = std::env::var_os("CC_SWITCH_LITE_TEST_BINARY")
            .expect("set CC_SWITCH_LITE_TEST_BINARY to the built Lite library test binary");
        Self(
            Command::new(binary)
                .args([
                    "--ignored",
                    "--exact",
                    "consumer_coordination::skill::skill_in_cli_fixture",
                    "--test-threads=1",
                ])
                .env("CC_SWITCH_COORDINATION_HOME", home)
                .env("CC_SWITCH_COORDINATION_MODE", mode)
                .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
                .env("CODEX_HOME", home.join(".codex"))
                .env("HERMES_HOME", home.join(".hermes"))
                .env("PI_CODING_AGENT_DIR", home.join(".pi/agent"))
                .stdin(Stdio::piped())
                .spawn()
                .unwrap(),
        )
    }

    fn finish(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "Lite Skill peer failed: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "Lite Skill peer did not exit");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_held(&mut self, home: &Path) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !home.join("lite-skill-held").is_file() {
            assert!(
                self.0.try_wait().unwrap().is_none(),
                "Lite holder exited early"
            );
            assert!(Instant::now() < deadline, "Lite holder did not signal");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for LitePeer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn fixture(home: &Path, method: SyncMethod) -> Database {
    let db = Database::init().unwrap();
    db.set_setting(LEGACY_PI_SKILL_BACKFILL_SETTING, "false")
        .unwrap();
    SkillService::set_sync_method(method).unwrap();
    for id in ["cli-skill", "peer-skill"] {
        db.save_skill(&InstalledSkill {
            id: id.into(),
            name: id.into(),
            description: Some("keep description".into()),
            directory: id.into(),
            repo_owner: Some("fixture-owner".into()),
            repo_name: Some("fixture-repo".into()),
            repo_branch: Some("fixture-branch".into()),
            readme_url: None,
            apps: SkillApps::default(),
            installed_at: 123,
            content_hash: Some("opaque-host-hash".into()),
            updated_at: 456,
        })
        .unwrap();
    }
    db.conn
        .lock()
        .unwrap()
        .execute_batch(
            "ALTER TABLE skills ADD COLUMN fixture_opaque BLOB;
         UPDATE skills SET fixture_opaque = X'00FF81', enabled_grokbuild = 1;",
        )
        .unwrap();
    let source = SkillService::get_ssot_dir().unwrap().join("cli-skill");
    fs::create_dir_all(source.join("assets")).unwrap();
    fs::write(source.join("SKILL.md"), "# Fixture\n").unwrap();
    fs::write(source.join("assets/data.bin"), [0, 255, 128]).unwrap();
    let gemini = crate::gemini_config::get_gemini_settings_path();
    fs::create_dir_all(gemini.parent().unwrap()).unwrap();
    // Gemini's native disabled list must agree with the initial catalog flags.
    fs::write(
        gemini,
        "{\"fixture\":true,\"skills\":{\"disabled\":[\"cli-skill\",\"peer-skill\"]}}\n",
    )
    .unwrap();
    fs::write(home.join("coordination-fixture"), "cli-lite-v1").unwrap();
    db
}

fn rows(db: &Database) -> Vec<cc_switch_store::SkillCatalogRow> {
    cc_switch_store::read_skill_catalog_rows(&db.conn.lock().unwrap()).unwrap()
}

fn assert_unowned(db: &Database, before: &[cc_switch_store::SkillCatalogRow]) {
    let after = rows(db);
    assert_eq!(
        after.iter().find(|r| r.id() == Some("peer-skill")),
        before.iter().find(|r| r.id() == Some("peer-skill"))
    );
    let opaque: (Vec<u8>, bool) = db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT fixture_opaque, enabled_grokbuild FROM skills WHERE id = 'cli-skill'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(opaque, (vec![0, 255, 129], true));
    let skill = db.get_installed_skill("cli-skill").unwrap().unwrap();
    assert_eq!(skill.description.as_deref(), Some("keep description"));
    assert_eq!(skill.repo_branch.as_deref(), Some("fixture-branch"));
    assert_eq!((skill.installed_at, skill.updated_at), (123, 456));
    assert_eq!(skill.content_hash.as_deref(), Some("opaque-host-hash"));
}

#[derive(Debug, PartialEq, Eq)]
struct NativeState {
    link: Option<PathBuf>,
    manifest: Vec<u8>,
    asset: Vec<u8>,
}

fn native(app: &AppType) -> Option<NativeState> {
    let path = SkillService::get_app_skills_dir(app)
        .unwrap()
        .join("cli-skill");
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        metadata => {
            let metadata = metadata.unwrap();
            Some(NativeState {
                link: metadata
                    .file_type()
                    .is_symlink()
                    .then(|| fs::read_link(&path).unwrap()),
                manifest: fs::read(path.join("SKILL.md")).unwrap(),
                asset: fs::read(path.join("assets/data.bin")).unwrap(),
            })
        }
    }
}

#[test]
fn skill_toggle_round_trips_without_rewriting_unowned_data() {
    for method in [SyncMethod::Auto, SyncMethod::Copy] {
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::isolated(temp.path());
        let db = fixture(temp.path(), method);
        let before = rows(&db);
        for enabled in [true, true, false, false] {
            SkillService::toggle_app("cli-skill", &AppType::Claude, enabled).unwrap();
            assert_eq!(native(&AppType::Claude).is_some(), enabled);
            assert_eq!(
                db.get_installed_skill("cli-skill")
                    .unwrap()
                    .unwrap()
                    .apps
                    .claude,
                enabled
            );
            assert_unowned(&db, &before);
        }
    }
}

#[test]
#[ignore = "acceptance gate: native compensation awaits Skill writer adoption"]
fn skill_toggle_restores_native_state_after_database_rejection() {
    let mut changed = Vec::new();
    for method in [SyncMethod::Auto, SyncMethod::Copy] {
        for enabled in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let _env = TestEnvGuard::isolated(temp.path());
            let db = fixture(temp.path(), method);
            if !enabled {
                SkillService::toggle_app("cli-skill", &AppType::Claude, true).unwrap();
            }
            let before = native(&AppType::Claude);
            let catalog = rows(&db);
            db.conn
                .lock()
                .unwrap()
                .execute_batch(
                    "CREATE TRIGGER fixture_reject_skill BEFORE UPDATE OF enabled_claude ON skills
                 WHEN OLD.id = 'cli-skill' AND NEW.enabled_claude != OLD.enabled_claude
                 BEGIN SELECT RAISE(ABORT, 'fixture rejects Skill selection'); END;",
                )
                .unwrap();
            let error =
                SkillService::toggle_app("cli-skill", &AppType::Claude, enabled).unwrap_err();
            assert!(
                matches!(error, AppError::Conflict(_)),
                "expected database rejection: {error}"
            );
            assert_eq!(rows(&db), catalog);
            if native(&AppType::Claude) != before {
                changed.push(format!("{method:?}/enabled={enabled}"));
            }
            db.conn
                .lock()
                .unwrap()
                .execute_batch("DROP TRIGGER fixture_reject_skill")
                .unwrap();
            SkillService::toggle_app("cli-skill", &AppType::Claude, enabled).unwrap();
            assert_eq!(native(&AppType::Claude).is_some(), enabled);
        }
    }
    assert!(
        changed.is_empty(),
        "failed database writes left native changes: {changed:?}"
    );
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn skill_toggle_preserves_another_app_changed_by_lite() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let db = fixture(temp.path(), SyncMethod::Auto);
    let before = rows(&db);
    let home = temp.path().to_owned();
    with_observation(
        move || LitePeer::start(&home, "interleave").finish(),
        || {
            SkillService::toggle_app("cli-skill", &AppType::Claude, true).unwrap();
        },
    );
    let outcome = fs::read_to_string(temp.path().join("lite-skill-outcome")).unwrap();
    if outcome == "busy" {
        // A coordinated writer may exclude the peer instead of merging a completed write.
        LitePeer::start(temp.path(), "toggle").finish();
    } else {
        assert_eq!(outcome, "committed");
    }
    let skill = db.get_installed_skill("cli-skill").unwrap().unwrap();
    assert!(skill.apps.claude);
    assert!(native(&AppType::Claude).is_some());
    assert!(native(&AppType::Gemini).is_some());
    assert_unowned(&db, &before);
    assert!(
        skill.apps.gemini,
        "CLI must retain the peer's Gemini selection on the same Skill"
    );
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn skill_toggle_respects_a_native_receipt_held_by_lite() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let db = fixture(temp.path(), SyncMethod::Auto);
    let before = rows(&db);
    let mut peer = LitePeer::start(temp.path(), "hold");
    peer.wait_held(temp.path());
    let held = native(&AppType::Gemini);
    assert!(
        held.is_some(),
        "Lite must hold an actual native publication"
    );
    let result = SkillService::toggle_app("cli-skill", &AppType::Claude, true);
    let after = rows(&db);
    let claude = native(&AppType::Claude);
    assert_eq!(native(&AppType::Gemini), held);
    peer.0.stdin.as_mut().unwrap().write_all(b"r").unwrap();
    peer.finish();
    // Core retains its protected public link when disabled, with no reachable Skill.
    assert!(!SkillService::get_app_skills_dir(&AppType::Gemini)
        .unwrap()
        .join("cli-skill/SKILL.md")
        .exists());
    // Also exercise release and retry before reporting a baseline refusal failure.
    SkillService::toggle_app("cli-skill", &AppType::Claude, true).unwrap();
    LitePeer::start(temp.path(), "toggle").finish();
    assert!(
        db.get_installed_skill("cli-skill")
            .unwrap()
            .unwrap()
            .apps
            .gemini
    );
    assert!(
        matches!(result, Err(AppError::Conflict(_))),
        "CLI wrote while Lite held the native lock: {result:?}"
    );
    assert_eq!(after, before);
    assert!(claude.is_none());
}
