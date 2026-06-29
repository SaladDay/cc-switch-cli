use cc_switch_lib::{AppType, Database, ImportSkillSelection, SkillApps, SkillService};

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
