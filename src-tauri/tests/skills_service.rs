use cc_switch_lib::{
    update_settings, AppSettings, AppType, Database, ImportSkillSelection, SkillApps, SkillService,
    SkillStorageLocation,
};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs};

fn write_skill_md(dir: &std::path::Path, name: &str, description: &str) {
    std::fs::create_dir_all(dir).expect("create skill dir");
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n"),
    )
    .expect("write SKILL.md");
}

fn register_managed_skill(directory: &str, apps: SkillApps) {
    let imported = SkillService::import_from_apps(vec![ImportSkillSelection {
        directory: directory.to_string(),
        apps,
    }])
    .expect("register managed skill through the public import API");
    assert_eq!(imported.len(), 1);
}

fn persisted_settings(home: &std::path::Path) -> AppSettings {
    let raw = std::fs::read_to_string(home.join(".cc-switch").join("settings.json"))
        .expect("read persisted settings");
    serde_json::from_str(&raw).expect("parse persisted settings")
}

fn remove_test_path(path: &std::path::Path) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() || metadata.is_file() {
        std::fs::remove_file(path).expect("remove test file");
    } else {
        std::fs::remove_dir_all(path).expect("remove test directory");
    }
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

#[test]
fn list_installed_triggers_initial_ssot_migration() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let claude_skill_dir = home.join(".claude").join("skills").join("hello-skill");
    write_skill_md(&claude_skill_dir, "Hello Skill", "A test skill");

    let db = Database::init().expect("init db");
    db.set_setting("skills_ssot_migration_pending", "true")
        .expect("set migration pending flag");

    let installed = SkillService::list_installed().expect("list installed");
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].directory, "hello-skill");
    assert!(
        installed[0].apps.claude,
        "skill should be enabled for claude"
    );

    let ssot_skill_dir = home.join(".cc-switch").join("skills").join("hello-skill");
    assert!(
        ssot_skill_dir.exists(),
        "SSOT directory should be created and populated"
    );

    let db = Database::init().expect("init db");
    let pending = db
        .get_setting("skills_ssot_migration_pending")
        .expect("read migration pending flag");
    assert_eq!(
        pending.as_deref(),
        Some("false"),
        "migration flag should be cleared after import"
    );

    let all = db
        .get_all_installed_skills()
        .expect("get all installed skills");
    let migrated = all
        .values()
        .find(|s| s.directory == "hello-skill")
        .expect("hello-skill should exist in db");
    assert!(
        migrated.apps.claude,
        "db record should be enabled for claude"
    );
}

#[test]
fn import_from_apps_imports_agents_skill_with_lock_metadata() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let agents_skill_dir = home.join(".agents").join("skills").join("hello-skill");
    write_skill_md(&agents_skill_dir, "Hello Skill", "From agents");

    let agents_dir = home.join(".agents");
    std::fs::create_dir_all(&agents_dir).expect("create agents dir");
    std::fs::write(
        agents_dir.join(".skill-lock.json"),
        r#"{
  "skills": {
    "hello-skill": {
      "source": "anthropics/skills",
      "sourceType": "github",
      "skillPath": "hello-skill/SKILL.md",
      "branch": "main"
    }
  }
}"#,
    )
    .expect("write agents lock file");

    let imported = SkillService::import_from_app_dirs(vec!["hello-skill".to_string()])
        .expect("import agents skill");

    assert_eq!(imported.len(), 1, "agents skill should be imported");

    let skill = &imported[0];
    assert_eq!(skill.directory, "hello-skill");
    assert_eq!(skill.name, "Hello Skill");
    assert_eq!(skill.id, "anthropics/skills:hello-skill");
    assert_eq!(skill.repo_owner.as_deref(), Some("anthropics"));
    assert_eq!(skill.repo_name.as_deref(), Some("skills"));
    assert_eq!(skill.repo_branch.as_deref(), Some("main"));
    assert_eq!(
        skill.readme_url.as_deref(),
        Some("https://github.com/anthropics/skills/blob/main/hello-skill/SKILL.md")
    );
    assert!(
        skill.apps.is_empty(),
        "agents source should not enable app flags"
    );

    let ssot_skill_dir = home.join(".cc-switch").join("skills").join("hello-skill");
    assert!(ssot_skill_dir.exists(), "skill should be copied into SSOT");
}

#[test]
fn scan_unmanaged_includes_agents_and_ssot_sources() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    write_skill_md(
        &home.join(".agents").join("skills").join("agents-skill"),
        "Agents Skill",
        "Found in agents",
    );
    write_skill_md(
        &home.join(".cc-switch").join("skills").join("ssot-skill"),
        "SSOT Skill",
        "Found in ssot",
    );

    let unmanaged = SkillService::scan_unmanaged().expect("scan unmanaged skills");

    let agents_skill = unmanaged
        .iter()
        .find(|skill| skill.directory == "agents-skill")
        .expect("agents skill should be visible");
    assert_eq!(agents_skill.name, "Agents Skill");
    assert!(agents_skill
        .found_in
        .iter()
        .any(|source| source == "agents"));

    let ssot_skill = unmanaged
        .iter()
        .find(|skill| skill.directory == "ssot-skill")
        .expect("ssot skill should be visible");
    assert_eq!(ssot_skill.name, "SSOT Skill");
    assert!(ssot_skill
        .found_in
        .iter()
        .any(|source| source == "cc-switch"));
}

#[test]
fn toggle_app_openclaw_skips_live_skill_side_effects() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let claude_skill_dir = home.join(".claude").join("skills").join("hello-skill");
    write_skill_md(&claude_skill_dir, "Hello Skill", "A test skill");

    let imported =
        SkillService::import_from_app_dirs(vec!["hello-skill".to_string()]).expect("import skill");
    assert_eq!(
        imported.len(),
        1,
        "skill should be imported before toggling"
    );

    SkillService::toggle_app("hello-skill", &AppType::OpenClaw, true)
        .expect("openclaw toggle should not fail");

    assert!(
        !home
            .join(".openclaw")
            .join("skills")
            .join("hello-skill")
            .exists(),
        "OpenClaw toggle should not create ~/.openclaw/skills entries"
    );

    let installed = SkillService::list_installed().expect("list installed skills");
    let skill = installed
        .into_iter()
        .find(|skill| skill.directory == "hello-skill")
        .expect("hello-skill should still be installed");
    assert!(
        skill.apps.claude,
        "existing supported app state should be preserved"
    );
}

#[test]
fn scan_unmanaged_includes_openclaw_skill_source() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    write_skill_md(
        &home.join(".openclaw").join("skills").join("openclaw-skill"),
        "OpenClaw Skill",
        "OpenClaw source",
    );

    let unmanaged = SkillService::scan_unmanaged().expect("scan unmanaged skills");
    let skill = unmanaged
        .iter()
        .find(|skill| skill.directory == "openclaw-skill")
        .expect("scan_unmanaged should include ~/.openclaw/skills as an import source");
    assert_eq!(skill.name, "OpenClaw Skill");
    assert!(skill
        .found_in
        .iter()
        .any(|source| source == AppType::OpenClaw.as_str()));
}

#[test]
fn import_from_app_dirs_imports_openclaw_source_without_openclaw_target() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    write_skill_md(
        &home.join(".openclaw").join("skills").join("openclaw-skill"),
        "OpenClaw Skill",
        "OpenClaw source",
    );

    let imported = SkillService::import_from_app_dirs(vec!["openclaw-skill".to_string()])
        .expect("import should not fail");
    assert_eq!(
        imported.len(),
        1,
        "OpenClaw source skill should be imported"
    );
    assert!(
        imported[0].apps.is_empty(),
        "OpenClaw is not a supported skill target app"
    );
    assert!(
        home.join(".cc-switch")
            .join("skills")
            .join("openclaw-skill")
            .exists(),
        "OpenClaw source skill should be copied into SSOT"
    );
}

#[test]
fn import_from_apps_applies_explicit_target_apps_for_openclaw_source() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    write_skill_md(
        &home.join(".openclaw").join("skills").join("openclaw-skill"),
        "OpenClaw Skill",
        "OpenClaw source",
    );

    let imported = SkillService::import_from_apps(vec![ImportSkillSelection {
        directory: "openclaw-skill".to_string(),
        apps: SkillApps::only(&AppType::Claude),
    }])
    .expect("import should not fail");

    assert_eq!(
        imported.len(),
        1,
        "OpenClaw source skill should be imported"
    );
    assert!(
        imported[0].apps.claude,
        "explicit Claude target should be preserved"
    );
    assert!(
        !imported[0].apps.is_enabled_for(&AppType::OpenClaw),
        "OpenClaw should never be persisted as a target app"
    );
}

#[test]
fn pending_migration_with_existing_managed_list_does_not_claim_unmanaged_skills() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    // Two skills exist in the app dir.
    let claude_dir = home.join(".claude").join("skills");
    write_skill_md(
        &claude_dir.join("managed-skill"),
        "Managed Skill",
        "Managed",
    );
    write_skill_md(
        &claude_dir.join("unmanaged-skill"),
        "Unmanaged Skill",
        "Unmanaged",
    );

    // Seed the DB with a managed list containing only "managed-skill".
    SkillService::import_from_app_dirs(vec!["managed-skill".to_string()])
        .expect("import managed-skill from apps");

    // Remove SSOT copy to ensure pending migration performs a best-effort re-copy.
    let ssot_dir = home.join(".cc-switch").join("skills");
    if ssot_dir.join("managed-skill").exists() {
        std::fs::remove_dir_all(ssot_dir.join("managed-skill"))
            .expect("remove managed-skill ssot dir");
    }

    let db = Database::init().expect("init db");
    db.set_setting("skills_ssot_migration_pending", "true")
        .expect("set migration pending flag");

    // Calling list_installed should perform best-effort SSOT copy for the managed skill,
    // without auto-importing all app dir skills into the managed list.
    let installed = SkillService::list_installed().expect("list installed");
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].directory, "managed-skill");

    assert!(
        ssot_dir.join("managed-skill").exists(),
        "managed skill should be copied into SSOT"
    );
    assert!(
        !ssot_dir.join("unmanaged-skill").exists(),
        "unmanaged skill should NOT be claimed/copied during pending migration when managed list is non-empty"
    );

    let db = Database::init().expect("init db");
    let pending = db
        .get_setting("skills_ssot_migration_pending")
        .expect("read migration pending flag");
    assert_eq!(
        pending.as_deref(),
        Some("false"),
        "migration flag should be cleared after best-effort copy"
    );

    let all = db
        .get_all_installed_skills()
        .expect("get all installed skills");
    assert!(
        all.values().all(|s| s.directory != "unmanaged-skill"),
        "unmanaged skill should remain unmanaged (not added to db)"
    );
}

#[test]
fn storage_migration_moves_only_managed_skills_and_refreshes_apps() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_root = SkillService::get_ssot_dir().expect("create managed SSOT root");
    let new_root = home.join(".agents").join("skills");
    write_skill_md(&old_root.join("managed"), "Managed", "Managed by CC Switch");
    write_skill_md(&old_root.join("unmanaged"), "Unmanaged", "Leave in place");
    register_managed_skill("managed", SkillApps::only(&AppType::Claude));

    let result =
        SkillService::migrate_storage(SkillStorageLocation::Unified).expect("migrate storage");

    assert_eq!(result.migrated_count, 1);
    assert!(result.errors.is_empty());
    assert!(!old_root.join("managed").exists());
    assert!(new_root.join("managed").join("SKILL.md").is_file());
    assert!(old_root.join("unmanaged").join("SKILL.md").is_file());
    assert!(!new_root.join("unmanaged").exists());
    assert!(
        home.join(".claude")
            .join("skills")
            .join("managed")
            .join("SKILL.md")
            .is_file(),
        "enabled app deployment should be refreshed"
    );
    assert_eq!(
        persisted_settings(home).skill_storage_location,
        SkillStorageLocation::Unified
    );

    let reversed =
        SkillService::migrate_storage(SkillStorageLocation::CcSwitch).expect("migrate back");
    assert_eq!(reversed.migrated_count, 1);
    assert!(reversed.errors.is_empty());
    assert!(old_root.join("managed").join("SKILL.md").is_file());
    assert!(!new_root.join("managed").exists());
    assert!(old_root.join("unmanaged").join("SKILL.md").is_file());
    assert_eq!(
        persisted_settings(home).skill_storage_location,
        SkillStorageLocation::CcSwitch
    );
}

#[test]
fn storage_migration_same_target_preserves_an_opposite_identical_copy() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    let new_skill = home.join(".agents").join("skills").join("managed");
    write_skill_md(&old_skill, "Managed", "Managed source");
    register_managed_skill("managed", SkillApps::default());
    SkillService::migrate_storage(SkillStorageLocation::Unified).expect("initial migration");
    write_skill_md(&old_skill, "Managed", "Managed source");

    let result =
        SkillService::migrate_storage(SkillStorageLocation::Unified).expect("same-target no-op");

    assert_eq!(result.migrated_count, 0);
    assert_eq!(result.skipped_count, 0);
    assert!(result.errors.is_empty());
    assert!(old_skill.join("SKILL.md").is_file());
    assert!(new_skill.join("SKILL.md").is_file());
}

#[test]
fn storage_migration_same_target_does_not_adopt_an_opposite_copy() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    write_skill_md(&old_skill, "Managed", "Opposite copy");
    register_managed_skill("managed", SkillApps::default());
    let mut settings = AppSettings::default();
    settings.skill_storage_location = SkillStorageLocation::Unified;
    update_settings(settings).expect("point settings at Unified without a journal");

    let result =
        SkillService::migrate_storage(SkillStorageLocation::Unified).expect("same-target no-op");

    assert_eq!(result.migrated_count, 0);
    assert!(result.errors.is_empty());
    assert!(old_skill.join("SKILL.md").is_file());
    assert!(!home.join(".agents").join("skills").join("managed").exists());
}

#[test]
fn storage_migration_rejects_different_target_content_before_moving() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    let new_skill = home.join(".agents").join("skills").join("managed");
    write_skill_md(&old_skill, "Managed", "Original content");
    write_skill_md(&new_skill, "Managed", "Different content");
    register_managed_skill("managed", SkillApps::default());

    let error = SkillService::migrate_storage(SkillStorageLocation::Unified)
        .expect_err("different target content must stop migration");

    assert!(error.to_string().contains("pre-existing"));
    assert!(old_skill.join("SKILL.md").is_file());
    assert!(new_skill.join("SKILL.md").is_file());
    assert_eq!(
        persisted_settings(home).skill_storage_location,
        SkillStorageLocation::CcSwitch
    );
}

#[test]
fn storage_migration_does_not_claim_an_identical_unknown_target() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    let new_skill = home.join(".agents").join("skills").join("managed");
    write_skill_md(&old_skill, "Managed", "Same content");
    write_skill_md(&new_skill, "Managed", "Same content");
    register_managed_skill("managed", SkillApps::default());

    SkillService::migrate_storage(SkillStorageLocation::Unified)
        .expect_err("content equality must not imply ownership");

    assert!(old_skill.join("SKILL.md").is_file());
    assert!(new_skill.join("SKILL.md").is_file());
    assert_eq!(
        persisted_settings(home).skill_storage_location,
        SkillStorageLocation::CcSwitch
    );
}

#[cfg(unix)]
#[test]
fn storage_migration_leaves_an_identical_plain_app_directory_untouched() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    let deployed = home.join(".claude").join("skills").join("managed");
    write_skill_md(&old_skill, "Managed", "Same deployment");
    write_skill_md(&deployed, "Managed", "Same deployment");
    register_managed_skill("managed", SkillApps::only(&AppType::Claude));

    let before = std::fs::symlink_metadata(&deployed).expect("read deployment metadata");
    assert!(before.is_dir());
    SkillService::migrate_storage(SkillStorageLocation::Unified)
        .expect("migrate without claiming the app directory");
    let after = std::fs::symlink_metadata(&deployed).expect("read deployment metadata again");

    use std::os::unix::fs::MetadataExt;
    assert!(after.is_dir());
    assert_eq!(
        before.ino(),
        after.ino(),
        "plain deployment must not be replaced"
    );
}

#[test]
fn storage_migration_compares_hidden_content() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    let new_skill = home.join(".agents").join("skills").join("managed");
    write_skill_md(&old_skill, "Managed", "Same visible content");
    write_skill_md(&new_skill, "Managed", "Same visible content");
    std::fs::write(old_skill.join(".env"), "TOKEN=old").expect("write old hidden content");
    std::fs::write(new_skill.join(".env"), "TOKEN=new").expect("write new hidden content");
    register_managed_skill("managed", SkillApps::default());

    SkillService::migrate_storage(SkillStorageLocation::Unified)
        .expect_err("different hidden content must stop migration");

    assert!(old_skill.join("SKILL.md").is_file());
    assert_eq!(
        persisted_settings(home).skill_storage_location,
        SkillStorageLocation::CcSwitch
    );
}

#[test]
fn storage_migration_does_not_adopt_a_target_when_the_current_source_is_missing() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    write_skill_md(&old_skill, "Managed", "Original");
    register_managed_skill("managed", SkillApps::default());
    remove_test_path(&old_skill);
    let new_skill = home.join(".agents").join("skills").join("managed");
    write_skill_md(&new_skill, "Managed", "Unknown target");

    SkillService::migrate_storage(SkillStorageLocation::Unified)
        .expect_err("a target without the current source cannot be identified safely");

    assert!(new_skill.join("SKILL.md").is_file());
    assert_eq!(
        persisted_settings(home).skill_storage_location,
        SkillStorageLocation::CcSwitch
    );
}

#[cfg(unix)]
#[test]
fn storage_migration_rejects_symbolic_links_without_following_them() {
    use std::os::unix::fs::symlink;

    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    write_skill_md(&skill, "Managed", "Contains an external link");
    let external = home.join("private");
    std::fs::create_dir_all(&external).expect("create external directory");
    std::fs::write(external.join("secret"), "do not copy").expect("write external file");
    symlink(&external, skill.join("linked-private")).expect("create external symlink");
    register_managed_skill("managed", SkillApps::default());

    SkillService::migrate_storage(SkillStorageLocation::Unified)
        .expect_err("migration must reject a nested symlink");

    assert!(!home.join(".agents").join("skills").join("managed").exists());
    assert_eq!(
        persisted_settings(home).skill_storage_location,
        SkillStorageLocation::CcSwitch
    );
}

#[test]
fn storage_migration_preserves_an_unknown_app_destination_and_retries() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    write_skill_md(&old_skill, "Managed", "Managed source");
    register_managed_skill("managed", SkillApps::only(&AppType::Claude));
    SkillService::sync_all_enabled(Some(&AppType::Claude)).expect("create app deployment");

    let deployed = home.join(".claude").join("skills").join("managed");
    remove_test_path(&deployed);
    write_skill_md(&deployed, "Personal", "Do not replace");

    let partial =
        SkillService::migrate_storage(SkillStorageLocation::Unified).expect("move SSOT safely");
    assert_eq!(partial.errors.len(), 1);
    assert_eq!(
        std::fs::read_to_string(deployed.join("SKILL.md")).expect("read preserved deployment"),
        "---\nname: Personal\ndescription: Do not replace\n---\n\n# Personal\n"
    );
    assert!(old_skill.exists(), "old source is the deployment fallback");
    assert_eq!(
        persisted_settings(home).skill_storage_location,
        SkillStorageLocation::Unified
    );

    remove_test_path(&deployed);
    let retried =
        SkillService::migrate_storage(SkillStorageLocation::Unified).expect("retry deployment");
    assert!(retried.errors.is_empty());
    assert!(deployed.join("SKILL.md").is_file());
    assert!(
        !old_skill.exists(),
        "successful retry cleans the old source"
    );
}

#[test]
fn storage_migration_retry_accepts_changes_in_the_current_target() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    write_skill_md(&old_skill, "Managed", "Original source");
    register_managed_skill("managed", SkillApps::only(&AppType::Claude));
    let deployed = home.join(".claude").join("skills").join("managed");
    write_skill_md(&deployed, "Personal", "Block initial deployment");

    let partial =
        SkillService::migrate_storage(SkillStorageLocation::Unified).expect("partial migration");
    assert_eq!(partial.errors.len(), 1);
    let new_skill = home.join(".agents").join("skills").join("managed");
    write_skill_md(&new_skill, "Managed", "Updated in current storage");

    let imported = home.join(".gemini").join("skills").join("added-later");
    write_skill_md(&imported, "Added Later", "New managed Skill");
    register_managed_skill("added-later", SkillApps::only(&AppType::Gemini));
    remove_test_path(&deployed);

    let retried =
        SkillService::migrate_storage(SkillStorageLocation::Unified).expect("retry migration");

    assert!(retried.errors.is_empty(), "{:?}", retried.errors);
    assert!(!old_skill.exists());
    assert!(new_skill.join("SKILL.md").is_file());
    assert!(home
        .join(".agents")
        .join("skills")
        .join("added-later")
        .join("SKILL.md")
        .is_file());
}

#[test]
fn storage_migration_rejects_an_app_directory_alias() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    write_skill_md(&old_skill, "Managed", "Keep safe");
    register_managed_skill("managed", SkillApps::only(&AppType::Codex));
    let mut settings = AppSettings::default();
    settings.codex_config_dir = Some(home.join(".agents").display().to_string());
    update_settings(settings).expect("set Codex override");

    SkillService::migrate_storage(SkillStorageLocation::Unified)
        .expect_err("aliased destination must be rejected");

    assert!(old_skill.join("SKILL.md").is_file());
    assert_eq!(
        persisted_settings(home).skill_storage_location,
        SkillStorageLocation::CcSwitch
    );
}

#[test]
fn storage_migration_rejects_an_app_directory_nested_under_the_ssot() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    write_skill_md(&old_skill, "Managed", "Keep safe");
    register_managed_skill("managed", SkillApps::only(&AppType::Codex));
    let mut settings = AppSettings::default();
    settings.codex_config_dir = Some(
        home.join(".agents")
            .join("skills")
            .join("nested")
            .display()
            .to_string(),
    );
    update_settings(settings).expect("set nested Codex override");

    SkillService::migrate_storage(SkillStorageLocation::Unified)
        .expect_err("nested storage roots must be rejected");

    assert!(old_skill.join("SKILL.md").is_file());
    assert_eq!(
        persisted_settings(home).skill_storage_location,
        SkillStorageLocation::CcSwitch
    );
}

#[test]
fn storage_migration_alias_check_honors_codex_home() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    write_skill_md(&old_skill, "Managed", "Keep safe");
    register_managed_skill("managed", SkillApps::only(&AppType::Codex));
    let codex_home = home.join(".agents");
    std::fs::create_dir_all(&codex_home).expect("create CODEX_HOME");
    let _env = EnvVarGuard::set("CODEX_HOME", &codex_home);

    SkillService::migrate_storage(SkillStorageLocation::Unified)
        .expect_err("CODEX_HOME alias must be rejected");

    assert!(old_skill.join("SKILL.md").is_file());
    assert_eq!(
        persisted_settings(home).skill_storage_location,
        SkillStorageLocation::CcSwitch
    );
}

#[cfg(unix)]
#[test]
fn app_sync_resolves_a_symlink_before_parent_components() {
    use std::os::unix::fs::symlink;

    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    write_skill_md(&old_skill, "Managed", "Keep safe");
    register_managed_skill("managed", SkillApps::only(&AppType::Claude));
    SkillService::migrate_storage(SkillStorageLocation::Unified)
        .expect("move to Unified before configuring the alias");

    let nested = home.join(".agents").join("nested");
    std::fs::create_dir_all(&nested).expect("create symlink target");
    let alias = home.join("alias");
    symlink(&nested, &alias).expect("create config alias");
    let claude_override = alias.join("..");
    let _env = EnvVarGuard::set("CLAUDE_CONFIG_DIR", &claude_override);

    SkillService::sync_all_enabled(Some(&AppType::Claude))
        .expect_err("resolved app and SSOT roots overlap");

    let skill = home.join(".agents").join("skills").join("managed");
    assert!(skill.join("SKILL.md").is_file());
    assert!(std::fs::symlink_metadata(&skill)
        .expect("read preserved Skill")
        .is_dir());
}

#[test]
fn storage_migration_rejects_overlap_with_the_current_ssot() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    write_skill_md(&old_skill, "Managed", "Keep safe");
    register_managed_skill("managed", SkillApps::only(&AppType::Claude));
    let _env = EnvVarGuard::set("CLAUDE_CONFIG_DIR", &home.join(".cc-switch"));

    SkillService::migrate_storage(SkillStorageLocation::Unified)
        .expect_err("the current SSOT must not double as an app deployment root");

    assert!(old_skill.join("SKILL.md").is_file());
    assert_eq!(
        persisted_settings(home).skill_storage_location,
        SkillStorageLocation::CcSwitch
    );
}

#[cfg(unix)]
#[test]
fn storage_migration_rejects_a_symlinked_unified_root() {
    use std::os::unix::fs::symlink;

    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    write_skill_md(&old_skill, "Managed", "Keep safe");
    register_managed_skill("managed", SkillApps::default());
    let external = home.join("external-agents");
    std::fs::create_dir_all(&external).expect("create external directory");
    symlink(&external, home.join(".agents")).expect("link Unified root elsewhere");

    SkillService::migrate_storage(SkillStorageLocation::Unified)
        .expect_err("Unified root symlinks must be rejected");

    assert!(old_skill.join("SKILL.md").is_file());
    assert!(!external.join("skills").exists());
}

#[test]
fn app_sync_rejects_an_ssot_alias_without_deleting_the_skill() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_skill = SkillService::get_ssot_dir()
        .expect("create managed SSOT root")
        .join("managed");
    let skill = home.join(".agents").join("skills").join("managed");
    write_skill_md(&old_skill, "Managed", "Keep safe");
    register_managed_skill("managed", SkillApps::only(&AppType::Codex));
    std::fs::create_dir_all(skill.parent().expect("unified skill root"))
        .expect("create unified skill root");
    std::fs::rename(&old_skill, &skill).expect("move skill to unified SSOT");
    let mut settings = AppSettings::default();
    settings.skill_storage_location = SkillStorageLocation::Unified;
    settings.codex_config_dir = Some(home.join(".agents").display().to_string());
    update_settings(settings).expect("set aliased paths");

    SkillService::sync_all_enabled(Some(&AppType::Codex))
        .expect_err("sync must reject aliased roots");

    assert!(skill.join("SKILL.md").is_file());
}
