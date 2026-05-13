use cc_switch_lib::{AppType, Database, SkillService};

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

struct EnvVarGuard {
    key: &'static str,
    old_value: Option<std::ffi::OsString>,
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.old_value {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
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

    let ssot_skill_dir = SkillService::get_ssot_dir()
        .expect("get ssot dir")
        .join("hello-skill");
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

    let imported = SkillService::import_from_apps(vec!["hello-skill".to_string()])
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

    let ssot_skill_dir = SkillService::get_ssot_dir()
        .expect("get ssot dir")
        .join("hello-skill");
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
    let ssot_dir = SkillService::get_ssot_dir().expect("get ssot dir");
    write_skill_md(&ssot_dir.join("ssot-skill"), "SSOT Skill", "Found in ssot");

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
        .any(|source| source == "cc-switch-tui"));
}

#[test]
fn scan_agent_installed_reads_all_agent_tool_dirs_and_excludes_noop_managed() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    write_skill_md(
        &home.join(".agents").join("skills").join("agent-skill"),
        "Agent Skill",
        "Found in agent",
    );
    write_skill_md(
        &home.join(".claude").join("skills").join("claude-skill"),
        "Claude Skill",
        "Found in Claude",
    );
    write_skill_md(
        &home.join(".hermes").join("skills").join("hermes-skill"),
        "Hermes Skill",
        "Found in Hermes",
    );
    write_skill_md(
        &home.join(".agents").join("skills").join("managed-skill"),
        "Managed Skill",
        "Already managed",
    );

    let imported = SkillService::import_from_agent(vec!["managed-skill".to_string()])
        .expect("seed managed skill from agent");
    assert_eq!(imported.len(), 1);

    let agent_skills = SkillService::scan_agent_installed().expect("scan agent-installed skills");

    assert!(
        agent_skills
            .iter()
            .any(|skill| skill.directory == "agent-skill"),
        "agent skill should be visible"
    );
    assert!(
        agent_skills
            .iter()
            .any(|skill| skill.directory == "claude-skill"
                && skill.found_in.iter().any(|source| source == "claude")),
        "Claude skill directory should be visible in agent import flow"
    );
    assert!(
        agent_skills
            .iter()
            .any(|skill| skill.directory == "hermes-skill"
                && skill.found_in.iter().any(|source| source == "hermes")),
        "Hermes skill directory should be visible in agent import flow"
    );
    assert!(
        agent_skills
            .iter()
            .all(|skill| skill.directory != "managed-skill"),
        "already managed agent skills should not be offered again"
    );
}

#[test]
fn scan_agent_installed_excludes_hermes_bundled_and_category_dirs() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let hermes_skills = home.join(".hermes").join("skills");

    std::fs::create_dir_all(&hermes_skills).expect("create Hermes skills dir");
    std::fs::write(
        hermes_skills.join(".bundled_manifest"),
        "builtin-skill:abc123\nnested-skill:def456\n",
    )
    .expect("write bundled manifest");
    write_skill_md(
        &hermes_skills.join("builtin-skill"),
        "Builtin Skill",
        "Bundled by Hermes",
    );
    write_skill_md(
        &hermes_skills.join("user-skill"),
        "User Skill",
        "Installed by user",
    );
    write_skill_md(
        &hermes_skills.join("category").join("nested-skill"),
        "Nested Skill",
        "Bundled inside category",
    );

    let agent_skills = SkillService::scan_agent_installed().expect("scan agent-installed skills");

    assert!(
        agent_skills
            .iter()
            .any(|skill| skill.directory == "user-skill"
                && skill.found_in.iter().any(|source| source == "hermes")),
        "non-bundled Hermes skills should still be visible"
    );
    assert!(
        agent_skills
            .iter()
            .all(|skill| skill.directory != "builtin-skill"),
        "Hermes bundled skills should not be offered for import"
    );
    assert!(
        agent_skills
            .iter()
            .all(|skill| skill.directory != "category"),
        "category directories without a root SKILL.md should not be offered"
    );
}

#[test]
fn import_from_agent_ignores_hermes_bundled_skill() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let hermes_skills = home.join(".hermes").join("skills");

    std::fs::create_dir_all(&hermes_skills).expect("create Hermes skills dir");
    std::fs::write(
        hermes_skills.join(".bundled_manifest"),
        "builtin-skill:abc123\n",
    )
    .expect("write bundled manifest");
    write_skill_md(
        &hermes_skills.join("builtin-skill"),
        "Builtin Skill",
        "Bundled by Hermes",
    );

    let imported = SkillService::import_from_agent(vec!["builtin-skill".to_string()])
        .expect("import should ignore bundled skill without failing");

    assert!(
        imported.is_empty(),
        "direct import should not claim Hermes bundled skills"
    );
    assert!(
        !SkillService::get_ssot_dir()
            .expect("get ssot dir")
            .join("builtin-skill")
            .exists(),
        "bundled skill should not be copied into SSOT"
    );
}

#[test]
fn import_from_agent_prefers_agents_dir_when_same_directory_exists_elsewhere() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    write_skill_md(
        &home.join(".claude").join("skills").join("same-skill"),
        "Claude Skill",
        "From claude",
    );
    write_skill_md(
        &home.join(".agents").join("skills").join("same-skill"),
        "Agent Skill",
        "From agent",
    );

    let imported = SkillService::import_from_agent(vec!["same-skill".to_string()])
        .expect("import should prefer agents source");

    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0].name, "Agent Skill");
    assert_eq!(imported[0].description.as_deref(), Some("From agent"));
    assert!(
        imported[0].apps.claude,
        "agent import should preserve that the skill is already installed for Claude"
    );

    let ssot_skill_md = SkillService::get_ssot_dir()
        .expect("get ssot dir")
        .join("same-skill")
        .join("SKILL.md");
    let content = std::fs::read_to_string(ssot_skill_md).expect("read imported skill");
    assert!(
        content.contains("name: Agent Skill"),
        "SSOT content should come from ~/.agents/skills"
    );
}

#[test]
fn import_from_agent_reads_codex_home_skills_and_enables_codex() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_codex_home = std::env::var_os("CODEX_HOME");
    let _codex_home_guard = EnvVarGuard {
        key: "CODEX_HOME",
        old_value: old_codex_home,
    };
    let codex_home = home.join(".codex-agent-home");
    unsafe {
        std::env::set_var("CODEX_HOME", &codex_home);
    }

    write_skill_md(
        &codex_home.join("skills").join("codex-agent-skill"),
        "Codex Agent Skill",
        "From Codex agent",
    );

    let scan_result = SkillService::scan_agent_installed().expect("scan agent-installed skills");
    assert!(
        scan_result
            .iter()
            .any(|skill| skill.directory == "codex-agent-skill"
                && skill.found_in == vec!["codex".to_string()]),
        "Codex agent home skills should be offered by the agent import flow"
    );

    let imported = SkillService::import_from_agent(vec!["codex-agent-skill".to_string()])
        .expect("import Codex agent skill");

    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0].name, "Codex Agent Skill");
    assert!(
        imported[0].apps.codex,
        "agent import should preserve that the skill is already installed for Codex"
    );
    assert!(
        SkillService::get_ssot_dir()
            .expect("get ssot dir")
            .join("codex-agent-skill")
            .exists(),
        "Codex agent skill should be copied into SSOT"
    );
}

#[test]
fn import_from_agent_deduplicates_requested_directories() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    write_skill_md(
        &home.join(".agents").join("skills").join("agent-skill"),
        "Agent Skill",
        "Found in agent",
    );

    let imported =
        SkillService::import_from_agent(vec!["agent-skill".to_string(), "agent-skill".to_string()])
            .expect("import should deduplicate requested directories");

    assert_eq!(
        imported.len(),
        1,
        "duplicate import requests should create one result"
    );
    assert_eq!(imported[0].directory, "agent-skill");
}

#[test]
fn scan_agent_installed_prefers_claude_config_dir_env_over_default() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_claude_config_dir = std::env::var_os("CLAUDE_CONFIG_DIR");
    let _claude_config_guard = EnvVarGuard {
        key: "CLAUDE_CONFIG_DIR",
        old_value: old_claude_config_dir,
    };
    let claude_home = home.join(".claude-env-home");
    unsafe {
        std::env::set_var("CLAUDE_CONFIG_DIR", &claude_home);
    }

    write_skill_md(
        &claude_home.join("skills").join("env-claude-skill"),
        "Env Claude Skill",
        "From CLAUDE_CONFIG_DIR",
    );
    write_skill_md(
        &home
            .join(".claude")
            .join("skills")
            .join("fallback-claude-skill"),
        "Fallback Claude Skill",
        "From default Claude dir",
    );

    let scan_result = SkillService::scan_agent_installed().expect("scan agent-installed skills");
    assert!(
        scan_result
            .iter()
            .any(|skill| skill.directory == "env-claude-skill"
                && skill.found_in == vec!["claude".to_string()]),
        "CLAUDE_CONFIG_DIR skills should be offered first"
    );
    assert!(
        scan_result
            .iter()
            .all(|skill| skill.directory != "fallback-claude-skill"),
        "default Claude skills should not be scanned when CLAUDE_CONFIG_DIR skills exist"
    );
}

#[test]
fn scan_agent_installed_falls_back_when_claude_config_dir_env_has_no_skills() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_claude_config_dir = std::env::var_os("CLAUDE_CONFIG_DIR");
    let _claude_config_guard = EnvVarGuard {
        key: "CLAUDE_CONFIG_DIR",
        old_value: old_claude_config_dir,
    };
    unsafe {
        std::env::set_var("CLAUDE_CONFIG_DIR", home.join(".claude-env-home"));
    }

    write_skill_md(
        &home
            .join(".claude")
            .join("skills")
            .join("fallback-claude-skill"),
        "Fallback Claude Skill",
        "From default Claude dir",
    );

    let scan_result = SkillService::scan_agent_installed().expect("scan agent-installed skills");
    assert!(
        scan_result
            .iter()
            .any(|skill| skill.directory == "fallback-claude-skill"
                && skill.found_in == vec!["claude".to_string()]),
        "default Claude skills should be scanned when CLAUDE_CONFIG_DIR has no skills directory"
    );
}

#[test]
fn scan_agent_installed_falls_back_when_env_skills_path_is_not_directory() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_hermes_home = std::env::var_os("HERMES_HOME");
    let _hermes_home_guard = EnvVarGuard {
        key: "HERMES_HOME",
        old_value: old_hermes_home,
    };
    let hermes_home = home.join(".hermes-env-home");
    unsafe {
        std::env::set_var("HERMES_HOME", &hermes_home);
    }
    std::fs::create_dir_all(&hermes_home).expect("create Hermes env home");
    std::fs::write(hermes_home.join("skills"), "not a directory").expect("write skills file");

    write_skill_md(
        &home
            .join(".hermes")
            .join("skills")
            .join("fallback-hermes-skill"),
        "Fallback Hermes Skill",
        "From default Hermes dir",
    );

    let scan_result = SkillService::scan_agent_installed().expect("scan agent-installed skills");
    assert!(
        scan_result
            .iter()
            .any(|skill| skill.directory == "fallback-hermes-skill"
                && skill.found_in == vec!["hermes".to_string()]),
        "default Hermes skills should be scanned when HERMES_HOME/skills is not a directory"
    );
}

#[test]
fn import_from_agent_prefers_hermes_home_env_and_enables_hermes() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let old_hermes_home = std::env::var_os("HERMES_HOME");
    let _hermes_home_guard = EnvVarGuard {
        key: "HERMES_HOME",
        old_value: old_hermes_home,
    };
    let hermes_home = home.join(".hermes-env-home");
    unsafe {
        std::env::set_var("HERMES_HOME", &hermes_home);
    }

    write_skill_md(
        &hermes_home.join("skills").join("env-hermes-skill"),
        "Env Hermes Skill",
        "From HERMES_HOME",
    );
    write_skill_md(
        &home
            .join(".hermes")
            .join("skills")
            .join("fallback-hermes-skill"),
        "Fallback Hermes Skill",
        "From default Hermes dir",
    );

    let scan_result = SkillService::scan_agent_installed().expect("scan agent-installed skills");
    assert!(
        scan_result
            .iter()
            .any(|skill| skill.directory == "env-hermes-skill"
                && skill.found_in == vec!["hermes".to_string()]),
        "HERMES_HOME skills should be offered first"
    );
    assert!(
        scan_result
            .iter()
            .all(|skill| skill.directory != "fallback-hermes-skill"),
        "default Hermes skills should not be scanned when HERMES_HOME skills exist"
    );

    let imported = SkillService::import_from_agent(vec!["env-hermes-skill".to_string()])
        .expect("import Hermes env skill");
    assert_eq!(imported.len(), 1);
    assert!(imported[0].apps.hermes);
}

#[test]
fn import_from_agent_imports_hermes_skill_and_enables_hermes() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    write_skill_md(
        &home.join(".hermes").join("skills").join("hermes-skill"),
        "Hermes Skill",
        "From Hermes",
    );

    let imported = SkillService::import_from_agent(vec!["hermes-skill".to_string()])
        .expect("import Hermes skill");

    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0].directory, "hermes-skill");
    assert!(
        imported[0].apps.hermes,
        "import_from_agent should enable Hermes when importing from ~/.hermes/skills"
    );
    assert!(
        SkillService::get_ssot_dir()
            .expect("get ssot dir")
            .join("hermes-skill")
            .exists(),
        "Hermes skill should be copied into SSOT"
    );
}

#[test]
fn import_from_agent_skips_existing_managed_skill() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    write_skill_md(
        &home.join(".agents").join("skills").join("shared-skill"),
        "Shared Skill",
        "Initially generic",
    );
    let imported = SkillService::import_from_agent(vec!["shared-skill".to_string()])
        .expect("seed generic managed skill");
    assert_eq!(imported.len(), 1);
    assert!(
        imported[0].apps.is_empty(),
        "generic .agents skill should not enable an app by itself"
    );

    write_skill_md(
        &home.join(".hermes").join("skills").join("shared-skill"),
        "Shared Skill",
        "Now installed for Hermes",
    );

    let scan_result = SkillService::scan_agent_installed().expect("scan agent-installed skills");
    assert!(
        scan_result
            .iter()
            .all(|skill| skill.directory != "shared-skill"),
        "managed skills should not be offered by agent import"
    );

    let skipped = SkillService::import_from_agent(vec!["shared-skill".to_string()])
        .expect("skip existing managed skill");

    assert!(
        skipped.is_empty(),
        "existing managed skill should not be re-imported"
    );

    let installed = SkillService::list_installed().expect("list installed skills");
    let skill = installed
        .iter()
        .find(|skill| skill.directory == "shared-skill")
        .expect("shared skill should remain installed");
    assert!(
        skill.apps.hermes,
        "list_installed should reflect managed skills already present in an app skills dir"
    );

    let db = Database::init().expect("init db");
    let all = db
        .get_all_installed_skills()
        .expect("get all installed skills");
    let persisted = all
        .values()
        .find(|skill| skill.directory == "shared-skill")
        .expect("shared skill should remain in db");
    assert!(
        persisted.apps.hermes,
        "reconciled app enablement should persist"
    );
}

#[test]
fn list_installed_reconciles_managed_codex_skill_present_on_disk() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    write_skill_md(
        &home.join(".agents").join("skills").join("codex-live-skill"),
        "Codex Live Skill",
        "Initially generic",
    );
    let imported = SkillService::import_from_agent(vec!["codex-live-skill".to_string()])
        .expect("seed managed skill from generic agent dir");
    assert_eq!(imported.len(), 1);
    assert!(
        !imported[0].apps.codex,
        "generic agent source should not enable Codex"
    );

    write_skill_md(
        &home.join(".codex").join("skills").join("codex-live-skill"),
        "Codex Live Skill",
        "Already installed for Codex",
    );

    let installed = SkillService::list_installed().expect("list installed skills");
    let skill = installed
        .iter()
        .find(|skill| skill.directory == "codex-live-skill")
        .expect("codex-live-skill should remain installed");
    assert!(
        skill.apps.codex,
        "managed skill present in ~/.codex/skills should show Codex enabled"
    );
}

#[test]
fn toggle_app_openclaw_syncs_live_skill_directory() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let claude_skill_dir = home.join(".claude").join("skills").join("hello-skill");
    write_skill_md(&claude_skill_dir, "Hello Skill", "A test skill");

    let imported =
        SkillService::import_from_apps(vec!["hello-skill".to_string()]).expect("import skill");
    assert_eq!(
        imported.len(),
        1,
        "skill should be imported before toggling"
    );

    SkillService::toggle_app("hello-skill", &AppType::OpenClaw, true)
        .expect("openclaw toggle should not fail");

    assert!(
        home.join(".openclaw")
            .join("skills")
            .join("hello-skill")
            .exists(),
        "OpenClaw toggle should create ~/.openclaw/skills entries"
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
    assert!(
        skill.apps.openclaw,
        "OpenClaw enablement should be persisted"
    );
}

#[test]
fn toggle_app_hermes_preserves_existing_native_skill_directory() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    write_skill_md(
        &home.join(".claude").join("skills").join("devops"),
        "DevOps",
        "Managed from Claude",
    );
    let imported = SkillService::import_from_apps(vec!["devops".to_string()])
        .expect("import managed devops skill");
    assert_eq!(imported.len(), 1);

    let hermes_devops = home.join(".hermes").join("skills").join("devops");
    std::fs::create_dir_all(hermes_devops.join("kanban-worker.bak"))
        .expect("create Hermes native category");
    std::fs::write(hermes_devops.join("native.txt"), "owned by Hermes")
        .expect("write Hermes native marker");

    SkillService::toggle_app("devops", &AppType::Hermes, true)
        .expect("Hermes native directory conflict should be preserved");

    assert!(
        hermes_devops.join("native.txt").exists(),
        "Hermes native directory should not be deleted or overwritten"
    );
    assert!(
        !hermes_devops.join("SKILL.md").exists(),
        "cc-switch should not copy over an unmanaged Hermes directory"
    );

    SkillService::toggle_app("devops", &AppType::Hermes, false)
        .expect("disabling should also preserve unmanaged Hermes directory");
    assert!(
        hermes_devops.join("native.txt").exists(),
        "disabling a managed skill must not delete Hermes native directories"
    );
}

#[test]
fn scan_unmanaged_includes_openclaw_skill_directory() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    write_skill_md(
        &home.join(".openclaw").join("skills").join("openclaw-skill"),
        "OpenClaw Skill",
        "Should be ignored",
    );

    let unmanaged = SkillService::scan_unmanaged().expect("scan unmanaged skills");
    let skill = unmanaged
        .iter()
        .find(|skill| skill.directory == "openclaw-skill")
        .expect("scan_unmanaged should include ~/.openclaw/skills");
    assert!(skill.found_in.iter().any(|source| source == "openclaw"));
}

#[test]
fn import_from_apps_imports_openclaw_skill_directory() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    write_skill_md(
        &home.join(".openclaw").join("skills").join("openclaw-skill"),
        "OpenClaw Skill",
        "Should be ignored",
    );

    let imported = SkillService::import_from_apps(vec!["openclaw-skill".to_string()])
        .expect("import should not fail");
    assert_eq!(imported.len(), 1);
    assert!(
        imported[0].apps.openclaw,
        "import_from_apps should enable OpenClaw when importing from ~/.openclaw/skills"
    );
    assert!(
        SkillService::get_ssot_dir()
            .expect("get ssot dir")
            .join("openclaw-skill")
            .exists(),
        "OpenClaw-only skills should be copied into SSOT"
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
    SkillService::import_from_apps(vec!["managed-skill".to_string()])
        .expect("import managed-skill from apps");

    // Remove SSOT copy to ensure pending migration performs a best-effort re-copy.
    let ssot_dir = SkillService::get_ssot_dir().expect("get ssot dir");
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
