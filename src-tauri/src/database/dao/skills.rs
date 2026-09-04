//! Skills 数据访问对象
//!
//! 提供 Skills 和 Skill Repos 的 CRUD 操作。
//!
//! v3.10.0+ 统一管理架构：
//! - Skills 使用统一的 id 主键，支持四应用启用标志
//! - 实际文件存储在 ~/.cc-switch/skills/，同步到各应用目录

use crate::app_config::{InstalledSkill, SkillApps};
use crate::database::{
    lock_conn, shared_store_error, sqlite_write_error, Database, LEGACY_PI_SKILL_BACKFILL_SETTING,
};
use crate::error::AppError;
use crate::services::skill::SkillRepo;
use cc_switch_core::{builtin_app_registry, AppType as CoreAppType, SkillCatalogColumn};
use cc_switch_store::{
    update_skill_host_fields_if_unchanged, SkillCatalogRow, SkillCatalogValues,
    SkillCatalogWriteOutcome,
};
use indexmap::IndexMap;
use rusqlite::{params, types::Value, Connection, OptionalExtension};

fn require_skill_applied(outcome: SkillCatalogWriteOutcome, action: &str) -> Result<(), AppError> {
    match outcome {
        SkillCatalogWriteOutcome::Applied => Ok(()),
        SkillCatalogWriteOutcome::NotApplied => Err(AppError::Conflict(format!(
            "Skill catalog {action} was not applied"
        ))),
    }
}

fn requested_skill_selection(apps: &SkillApps, app: &CoreAppType) -> Option<bool> {
    match app {
        CoreAppType::Claude => Some(apps.claude),
        CoreAppType::Codex => Some(apps.codex),
        CoreAppType::Gemini => Some(apps.gemini),
        CoreAppType::OpenCode => Some(apps.opencode),
        CoreAppType::Hermes => Some(apps.hermes),
        CoreAppType::Pi => Some(apps.pi),
        CoreAppType::ClaudeDesktop | CoreAppType::GrokBuild | CoreAppType::OpenClaw => None,
    }
}

fn complete_skill_selections(
    apps: &SkillApps,
    current: Option<&SkillCatalogRow>,
) -> Vec<(SkillCatalogColumn, bool)> {
    builtin_app_registry()
        .descriptors()
        .filter_map(|descriptor| {
            descriptor.skill_contract().map(|contract| {
                let selected = requested_skill_selection(apps, descriptor.app())
                    .or_else(|| current.and_then(|row| row.selected_for(descriptor.app())))
                    .unwrap_or(false);
                (contract.catalog_column(), selected)
            })
        })
        .collect()
}

fn skill_apps_from_catalog(values: &SkillCatalogValues) -> SkillApps {
    SkillApps {
        claude: values.selected_for(&CoreAppType::Claude).unwrap_or(false),
        codex: values.selected_for(&CoreAppType::Codex).unwrap_or(false),
        gemini: values.selected_for(&CoreAppType::Gemini).unwrap_or(false),
        opencode: values.selected_for(&CoreAppType::OpenCode).unwrap_or(false),
        hermes: values.selected_for(&CoreAppType::Hermes).unwrap_or(false),
        pi: values.selected_for(&CoreAppType::Pi).unwrap_or(false),
    }
}

struct SkillHostMetadata {
    repo_owner: Option<String>,
    repo_name: Option<String>,
    repo_branch: Option<String>,
    readme_url: Option<String>,
    installed_at: i64,
    content_hash: Option<String>,
    updated_at: i64,
}

fn optional_text_value(value: &Option<String>) -> Value {
    value
        .as_ref()
        .map_or(Value::Null, |value| Value::Text(value.clone()))
}

fn skill_host_values(skill: &InstalledSkill) -> Vec<(String, Value)> {
    vec![
        (
            "repo_owner".to_owned(),
            optional_text_value(&skill.repo_owner),
        ),
        (
            "repo_name".to_owned(),
            optional_text_value(&skill.repo_name),
        ),
        (
            "repo_branch".to_owned(),
            optional_text_value(&skill.repo_branch),
        ),
        (
            "readme_url".to_owned(),
            optional_text_value(&skill.readme_url),
        ),
        (
            "installed_at".to_owned(),
            Value::Integer(skill.installed_at),
        ),
        (
            "content_hash".to_owned(),
            optional_text_value(&skill.content_hash),
        ),
        ("updated_at".to_owned(), Value::Integer(skill.updated_at)),
    ]
}

fn malformed_skill_host_metadata(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::InvalidColumnType(..)
            | rusqlite::Error::IntegralValueOutOfRange(..)
            | rusqlite::Error::FromSqlConversionFailure(..)
    )
}

fn read_skill_host_metadata(
    connection: &Connection,
    id: &str,
) -> rusqlite::Result<Option<SkillHostMetadata>> {
    connection
        .query_row(
            "SELECT repo_owner, repo_name, repo_branch, readme_url,
                    installed_at, content_hash, updated_at
             FROM skills WHERE id COLLATE BINARY = ?1",
            [id],
            |row| {
                Ok(SkillHostMetadata {
                    repo_owner: row.get(0)?,
                    repo_name: row.get(1)?,
                    repo_branch: row.get(2)?,
                    readme_url: row.get(3)?,
                    installed_at: row.get(4)?,
                    content_hash: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
        .optional()
}

fn installed_skill_from_catalog(
    connection: &Connection,
    values: &SkillCatalogValues,
) -> rusqlite::Result<InstalledSkill> {
    let metadata = read_skill_host_metadata(connection, &values.id)?
        .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    Ok(InstalledSkill {
        id: values.id.clone(),
        name: values.name.clone(),
        description: values.description.clone(),
        directory: values.directory.clone(),
        repo_owner: metadata.repo_owner,
        repo_name: metadata.repo_name,
        repo_branch: metadata.repo_branch,
        readme_url: metadata.readme_url,
        apps: skill_apps_from_catalog(values),
        installed_at: metadata.installed_at,
        content_hash: metadata.content_hash,
        updated_at: metadata.updated_at,
    })
}

impl Database {
    // ========== InstalledSkill CRUD ==========

    /// 获取所有已安装的 Skills
    pub fn get_all_installed_skills(&self) -> Result<IndexMap<String, InstalledSkill>, AppError> {
        let mut conn = lock_conn!(self.conn);
        let transaction = conn.transaction().map_err(sqlite_write_error)?;
        let mut skills = IndexMap::new();
        for row in
            cc_switch_store::read_skill_catalog_rows(&transaction).map_err(shared_store_error)?
        {
            let Some(values) = row.values() else {
                continue;
            };
            match installed_skill_from_catalog(&transaction, values) {
                Ok(skill) => {
                    skills.insert(skill.id.clone(), skill);
                }
                Err(error) if malformed_skill_host_metadata(&error) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        transaction.commit().map_err(sqlite_write_error)?;
        Ok(skills)
    }

    /// 获取单个已安装的 Skill
    pub fn get_installed_skill(&self, id: &str) -> Result<Option<InstalledSkill>, AppError> {
        let mut conn = lock_conn!(self.conn);
        let transaction = conn.transaction().map_err(sqlite_write_error)?;
        let Some(row) = cc_switch_store::read_skill_catalog_row(&transaction, id)
            .map_err(shared_store_error)?
        else {
            transaction.commit().map_err(sqlite_write_error)?;
            return Ok(None);
        };
        let values = row.values().ok_or_else(|| {
            AppError::Database("Skill row does not match the shared storage contract".to_owned())
        })?;
        let skill = installed_skill_from_catalog(&transaction, values).map_err(AppError::from)?;
        transaction.commit().map_err(sqlite_write_error)?;
        Ok(Some(skill))
    }

    pub(crate) fn delete_malformed_skill_by_id(&self, id: &str) -> Result<bool, AppError> {
        let mut conn = lock_conn!(self.conn);
        let mut transaction =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        let Some(current) = cc_switch_store::read_skill_catalog_row(&transaction, id)
            .map_err(shared_store_error)?
        else {
            transaction.commit().map_err(sqlite_write_error)?;
            return Ok(false);
        };
        let shared_values = current.values();
        let host_values_are_valid = match shared_values {
            Some(values) => match read_skill_host_metadata(&transaction, &values.id) {
                Ok(Some(_)) => true,
                Ok(None) => false,
                Err(error) if malformed_skill_host_metadata(&error) => false,
                Err(error) => return Err(error.into()),
            },
            None => false,
        };
        if shared_values.is_some() && host_values_are_valid {
            transaction.commit().map_err(sqlite_write_error)?;
            return Ok(false);
        }

        let outcome =
            cc_switch_store::delete_skill_catalog_if_unchanged(&mut transaction, &current)
                .map_err(shared_store_error)?;
        require_skill_applied(outcome, "malformed-row delete")?;
        transaction.commit().map_err(sqlite_write_error)?;
        Ok(true)
    }

    pub(crate) fn backfill_legacy_pi_skill_selections(
        &self,
        mut is_deployed: impl FnMut(&str) -> bool,
    ) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let mut transaction =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        let marker = transaction
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                [LEGACY_PI_SKILL_BACKFILL_SETTING],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(sqlite_write_error)?;
        if marker.as_deref() != Some("true") {
            transaction.commit().map_err(sqlite_write_error)?;
            return Ok(());
        }

        let rows =
            cc_switch_store::read_skill_catalog_rows(&transaction).map_err(shared_store_error)?;
        for current in rows {
            let Some(values) = current.values() else {
                continue;
            };
            if current.selected_for(&CoreAppType::Pi) != Some(false)
                || !is_deployed(&values.directory)
            {
                continue;
            }
            let selections = builtin_app_registry()
                .descriptors()
                .filter_map(|descriptor| {
                    descriptor.skill_contract().map(|contract| {
                        let selected = if descriptor.app() == &CoreAppType::Pi {
                            true
                        } else {
                            current.selected_for(descriptor.app()).unwrap_or(false)
                        };
                        (contract.catalog_column(), selected)
                    })
                })
                .collect::<Vec<_>>();
            let outcome = cc_switch_store::update_skill_catalog_if_unchanged(
                &mut transaction,
                &current,
                &values.id,
                &values.name,
                values.description.as_deref(),
                &values.directory,
                selections,
            )
            .map_err(shared_store_error)?;
            require_skill_applied(outcome, "legacy Pi selection backfill")?;
        }

        let expected_catalog =
            cc_switch_store::read_skill_catalog_rows(&transaction).map_err(shared_store_error)?;
        let affected = transaction
            .execute(
                "UPDATE settings SET value = 'false' WHERE key = ?1 AND value = 'true'",
                [LEGACY_PI_SKILL_BACKFILL_SETTING],
            )
            .map_err(sqlite_write_error)?;
        let catalog_unchanged = cc_switch_store::read_skill_catalog_rows(&transaction)
            .map_err(shared_store_error)?
            == expected_catalog;
        let marker_is_false = transaction
            .query_row(
                "SELECT value = 'false' FROM settings WHERE key = ?1",
                [LEGACY_PI_SKILL_BACKFILL_SETTING],
                |row| row.get::<_, bool>(0),
            )
            .map_err(sqlite_write_error)?;
        if affected != 1 || !marker_is_false || !catalog_unchanged {
            return Err(AppError::Conflict(
                "Skill legacy Pi selection backfill was not completed".to_owned(),
            ));
        }
        transaction.commit().map_err(sqlite_write_error)?;
        Ok(())
    }

    /// 保存 Skill（添加或更新）
    pub fn save_skill(&self, skill: &InstalledSkill) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let mut transaction =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        let before =
            cc_switch_store::read_skill_catalog_rows(&transaction).map_err(shared_store_error)?;
        let current = before
            .iter()
            .find(|row| row.id() == Some(skill.id.as_str()));
        let selections = complete_skill_selections(&skill.apps, current);
        let outcome = match current {
            Some(current) => cc_switch_store::update_skill_catalog_if_unchanged(
                &mut transaction,
                current,
                &skill.id,
                &skill.name,
                skill.description.as_deref(),
                &skill.directory,
                selections.iter().copied(),
            ),
            None => cc_switch_store::insert_skill_catalog_if_absent(
                &mut transaction,
                &skill.id,
                &skill.name,
                skill.description.as_deref(),
                &skill.directory,
                selections.iter().copied(),
            ),
        }
        .map_err(shared_store_error)?;
        require_skill_applied(outcome, "save")?;
        let current = cc_switch_store::read_skill_catalog_row(&transaction, &skill.id)
            .map_err(shared_store_error)?
            .ok_or_else(|| {
                AppError::Conflict("Skill catalog row disappeared before metadata save".to_owned())
            })?;
        let outcome = update_skill_host_fields_if_unchanged(
            &mut transaction,
            &current,
            skill_host_values(skill),
        )
        .map_err(shared_store_error)?;
        require_skill_applied(outcome, "metadata save")?;
        transaction.commit().map_err(sqlite_write_error)?;
        Ok(())
    }

    /// 删除 Skill
    pub fn delete_skill(&self, id: &str) -> Result<bool, AppError> {
        let mut conn = lock_conn!(self.conn);
        let mut transaction =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        let Some(current) = cc_switch_store::read_skill_catalog_row(&transaction, id)
            .map_err(shared_store_error)?
        else {
            transaction.commit().map_err(sqlite_write_error)?;
            return Ok(false);
        };
        let outcome =
            cc_switch_store::delete_skill_catalog_if_unchanged(&mut transaction, &current)
                .map_err(shared_store_error)?;
        require_skill_applied(outcome, "delete")?;
        transaction.commit().map_err(sqlite_write_error)?;
        Ok(true)
    }

    /// 更新 Skill 的内容哈希和更新时间。
    pub fn update_skill_hash(
        &self,
        id: &str,
        content_hash: &str,
        updated_at: i64,
    ) -> Result<bool, AppError> {
        let mut conn = lock_conn!(self.conn);
        let mut transaction =
            cc_switch_store::begin_immediate_transaction(&mut conn).map_err(shared_store_error)?;
        let Some(current) = cc_switch_store::read_skill_catalog_row(&transaction, id)
            .map_err(shared_store_error)?
        else {
            transaction.commit().map_err(sqlite_write_error)?;
            return Ok(false);
        };
        let outcome = update_skill_host_fields_if_unchanged(
            &mut transaction,
            &current,
            [
                (
                    "content_hash".to_owned(),
                    Value::Text(content_hash.to_owned()),
                ),
                ("updated_at".to_owned(), Value::Integer(updated_at)),
            ],
        )
        .map_err(shared_store_error)?;
        require_skill_applied(outcome, "hash update")?;
        transaction.commit().map_err(sqlite_write_error)?;
        Ok(true)
    }

    // ========== SkillRepo CRUD（保持原有） ==========

    /// 获取所有 Skill 仓库
    pub fn get_skill_repos(&self) -> Result<Vec<SkillRepo>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT owner, name, branch, enabled FROM skill_repos ORDER BY owner ASC, name ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let repo_iter = stmt
            .query_map([], |row| {
                Ok(SkillRepo {
                    owner: row.get(0)?,
                    name: row.get(1)?,
                    branch: row.get(2)?,
                    enabled: row.get(3)?,
                })
            })
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut repos = Vec::new();
        for repo_res in repo_iter {
            repos.push(repo_res.map_err(|e| AppError::Database(e.to_string()))?);
        }
        Ok(repos)
    }

    /// 保存 Skill 仓库
    pub fn save_skill_repo(&self, repo: &SkillRepo) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT OR REPLACE INTO skill_repos (owner, name, branch, enabled) VALUES (?1, ?2, ?3, ?4)",
            params![repo.owner, repo.name, repo.branch, repo.enabled],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 删除 Skill 仓库
    pub fn delete_skill_repo(&self, owner: &str, name: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM skill_repos WHERE owner = ?1 AND name = ?2",
            params![owner, name],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 精确更新 Skill 仓库启用状态，避免覆盖并发更新的分支等字段。
    pub fn set_skill_repo_enabled(
        &self,
        owner: &str,
        name: &str,
        enabled: bool,
    ) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let changed = conn
            .execute(
                "UPDATE skill_repos SET enabled = ?3 WHERE owner = ?1 AND name = ?2",
                params![owner, name, enabled],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(changed > 0)
    }

    /// 初始化默认的 Skill 仓库（启动时调用，每个数据库仅执行一次）
    pub fn init_default_skill_repos(&self) -> Result<usize, AppError> {
        const INITIALIZED_KEY: &str = "default_skill_repos_initialized";

        if self.get_bool_flag(INITIALIZED_KEY)? {
            return Ok(0);
        }

        // 兼容升级前已经存在的用户选择，并记录初始化状态，避免以后删空后恢复默认值。
        if !self.get_skill_repos()?.is_empty() {
            self.set_setting(INITIALIZED_KEY, "true")?;
            return Ok(0);
        }

        let default_store = crate::services::skill::SkillStore::default();
        let mut count = 0;

        for repo in &default_store.repos {
            self.save_skill_repo(repo)?;
            count += 1;
            log::info!("初始化默认 Skill 仓库: {}/{}", repo.owner, repo.name);
        }

        self.set_setting(INITIALIZED_KEY, "true")?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed_skill(id: &str) -> InstalledSkill {
        InstalledSkill {
            id: id.to_owned(),
            name: "Demo".to_owned(),
            description: Some("Description".to_owned()),
            directory: format!("{id}-directory"),
            repo_owner: Some("owner".to_owned()),
            repo_name: Some("repo".to_owned()),
            repo_branch: Some("main".to_owned()),
            readme_url: Some("https://example.invalid/readme".to_owned()),
            apps: SkillApps::default(),
            installed_at: 1,
            content_hash: Some("before".to_owned()),
            updated_at: 2,
        }
    }

    #[test]
    fn skill_hash_fields_round_trip_without_schema_changes() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let db = Database::init().expect("initialize database");
        let skill = installed_skill("owner/repo:demo");

        db.save_skill(&skill).expect("save skill hash fields");
        let stored = db
            .get_installed_skill(&skill.id)
            .expect("read skill")
            .expect("stored skill");
        assert_eq!(stored.content_hash.as_deref(), Some("before"));
        assert_eq!(stored.updated_at, 2);

        assert!(db
            .update_skill_hash(&skill.id, "after", 3)
            .expect("update hash fields"));
        let updated = db
            .get_installed_skill(&skill.id)
            .expect("read updated skill")
            .expect("updated skill");
        assert_eq!(updated.content_hash.as_deref(), Some("after"));
        assert_eq!(updated.updated_at, 3);
        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "CREATE TRIGGER suppress_skill_hash
                 BEFORE UPDATE OF content_hash ON skills
                 WHEN OLD.id = 'owner/repo:demo'
                 BEGIN SELECT RAISE(IGNORE); END;",
            )?;
        }
        assert!(matches!(
            db.update_skill_hash(&skill.id, "suppressed", 4),
            Err(AppError::Conflict(_))
        ));
        let preserved = db.get_installed_skill(&skill.id)?.expect("preserved Skill");
        assert_eq!(preserved.content_hash.as_deref(), Some("after"));
        assert_eq!(preserved.updated_at, 3);
        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "DROP TRIGGER suppress_skill_hash;
                 CREATE TRIGGER reject_skill_hash
                 BEFORE UPDATE OF content_hash ON skills
                 WHEN OLD.id = 'owner/repo:demo'
                 BEGIN SELECT RAISE(ABORT, 'sensitive trigger detail'); END;",
            )?;
        }
        let error = db
            .update_skill_hash(&skill.id, "rejected", 5)
            .expect_err("trigger must reject hash update");
        assert!(matches!(error, AppError::Conflict(_)));
        assert!(!error.to_string().contains("sensitive trigger detail"));
        Ok(())
    }

    #[test]
    fn legacy_pi_backfill_rolls_back_selection_and_marker_on_conflict() -> Result<(), AppError> {
        let db = Database::memory()?;
        let skill = installed_skill("legacy");
        db.save_skill(&skill)?;
        db.set_setting(LEGACY_PI_SKILL_BACKFILL_SETTING, "true")?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "CREATE TRIGGER suppress_legacy_pi_backfill
                 BEFORE UPDATE OF enabled_pi ON skills
                 WHEN OLD.id = 'legacy'
                 BEGIN SELECT RAISE(IGNORE); END;",
            )?;
        }

        assert!(matches!(
            db.backfill_legacy_pi_skill_selections(|_| true),
            Err(AppError::Conflict(_))
        ));
        assert!(
            !db.get_installed_skill("legacy")?
                .expect("legacy Skill remains")
                .apps
                .pi
        );
        assert_eq!(
            db.get_setting(LEGACY_PI_SKILL_BACKFILL_SETTING)?.as_deref(),
            Some("true")
        );
        Ok(())
    }

    #[test]
    fn skill_round_trip_uses_registry_and_preserves_host_extensions() -> Result<(), AppError> {
        let db = Database::memory().expect("create memory database");
        {
            let conn = lock_conn!(db.conn);
            conn.execute("ALTER TABLE skills ADD COLUMN host_extension TEXT", [])
                .expect("add host extension");
            conn.execute(
                "INSERT INTO skills (
                    id, name, directory, enabled_grokbuild, host_extension
                 ) VALUES ('demo', 'Demo', 'demo-directory', 1, 'keep')",
                [],
            )
            .expect("seed hidden registry selection");
        }

        let mut skill = db
            .get_installed_skill("demo")
            .expect("read seeded Skill")
            .expect("seeded Skill exists");
        skill.name = "Updated".to_owned();
        skill.apps.pi = true;
        skill.repo_owner = Some("updated-owner".to_owned());
        db.save_skill(&skill)
            .expect("save through Core catalog API");

        let loaded = db
            .get_installed_skill("demo")
            .expect("read updated Skill")
            .expect("updated Skill exists");
        assert_eq!(loaded.name, "Updated");
        assert!(loaded.apps.pi);
        assert_eq!(loaded.repo_owner.as_deref(), Some("updated-owner"));
        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "CREATE TRIGGER rewrite_skill_host_extension
                 AFTER UPDATE OF repo_owner ON skills
                 WHEN NEW.id = 'demo'
                 BEGIN
                    UPDATE skills SET host_extension = 'rewritten' WHERE id = NEW.id;
                 END;",
            )?;
        }
        skill.repo_owner = Some("rejected-owner".to_owned());
        assert!(matches!(db.save_skill(&skill), Err(AppError::Conflict(_))));
        let conn = lock_conn!(db.conn);
        assert_eq!(
            conn.query_row(
                "SELECT enabled_grokbuild, enabled_pi, host_extension, repo_owner
                 FROM skills WHERE id = 'demo'",
                [],
                |row| Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?
                )),
            )
            .expect("read preserved shared row"),
            (1, 1, "keep".to_owned(), "updated-owner".to_owned())
        );
        Ok(())
    }

    #[test]
    fn malformed_skill_does_not_block_other_crud_and_remains_deletable() -> Result<(), AppError> {
        let db = Database::memory().expect("create memory database");
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO skills (id, name, description, directory)
                 VALUES ('malformed', 'Malformed', x'80', 'malformed')",
                [],
            )
            .expect("seed malformed Skill");
            conn.execute(
                "INSERT INTO skills (id, name, directory, installed_at)
                 VALUES ('bad-host', 'Bad Host', 'bad-host', 'invalid')",
                [],
            )
            .expect("seed malformed host metadata");
            conn.execute(
                "INSERT INTO skills (
                    id, name, directory, enabled_claude, enabled_grokbuild
                 ) VALUES ('bad-selection', 'Bad Selection', 'bad-selection', 2, 1)",
                [],
            )
            .expect("seed partially malformed selection");
        }

        let skill = installed_skill("valid");
        db.save_skill(&skill)
            .expect("save valid Skill beside malformed row");
        let installed = db
            .get_all_installed_skills()
            .expect("read valid Skills beside malformed row");
        assert!(installed.contains_key("valid"));
        assert!(!installed.contains_key("malformed"));
        assert!(!installed.contains_key("bad-host"));
        let repair = installed_skill("bad-selection");
        db.save_skill(&repair)
            .expect("repair malformed selection through raw snapshot");
        {
            let conn = lock_conn!(db.conn);
            assert_eq!(
                conn.query_row(
                    "SELECT enabled_claude, enabled_grokbuild FROM skills
                     WHERE id = 'bad-selection'",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )?,
                (0, 1)
            );
        }
        assert!(db
            .delete_skill("malformed")
            .expect("delete malformed row through raw snapshot"));
        assert!(db.delete_skill("bad-host")?);
        assert!(!db
            .delete_skill("missing")
            .expect("missing delete is a no-op"));
        Ok(())
    }

    #[test]
    fn skill_save_rolls_back_host_trigger_rewrite_of_another_row() -> Result<(), AppError> {
        let db = Database::memory().expect("create memory database");
        let mut target = installed_skill("target");
        let other = installed_skill("other");
        db.save_skill(&target).expect("save target Skill");
        db.save_skill(&other).expect("save other Skill");
        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "CREATE TRIGGER rewrite_other_skill_after_host_update
                 AFTER UPDATE OF repo_owner ON skills
                 WHEN NEW.id = 'target'
                 BEGIN
                    UPDATE skills SET name = 'rewritten' WHERE id = 'other';
                 END;",
            )
            .expect("create hostile host trigger");
        }

        target.repo_owner = Some("changed-owner".to_owned());
        assert!(matches!(db.save_skill(&target), Err(AppError::Conflict(_))));
        assert_eq!(
            db.get_installed_skill("target")
                .expect("read rolled-back target")
                .expect("target exists")
                .repo_owner
                .as_deref(),
            Some("owner")
        );
        assert_eq!(
            db.get_installed_skill("other")
                .expect("read preserved other")
                .expect("other exists")
                .name,
            "Demo"
        );
        Ok(())
    }
}
