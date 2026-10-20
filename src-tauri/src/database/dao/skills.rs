//! Skills 数据访问对象
//!
//! 提供 Skills 和 Skill Repos 的 CRUD 操作。
//!
//! v3.10.0+ 统一管理架构：
//! - Skills 使用统一的 id 主键，支持四应用启用标志
//! - 实际文件存储在 ~/.cc-switch/skills/，同步到各应用目录

use crate::app_config::{InstalledSkill, SkillApps};
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::services::skill::SkillRepo;
use indexmap::IndexMap;
use rusqlite::params;

/// Opaque target-row snapshot for skill publication. Includes desktop-only columns.
pub(crate) struct LocalSkillBaseline {
    pub(crate) directory: String,
    pub(crate) skill: Option<InstalledSkill>,
    rows: Vec<Vec<rusqlite::types::Value>>,
}

impl Database {
    fn local_skill_baseline_on_connection(
        conn: &rusqlite::Connection,
        directory: &str,
    ) -> Result<LocalSkillBaseline, AppError> {
        let mut stmt = conn
            .prepare("SELECT * FROM skills WHERE directory = ?1 COLLATE NOCASE ORDER BY id")
            .map_err(|e| AppError::Database(e.to_string()))?;
        let column_count = stmt.column_count();
        let rows = stmt
            .query_map([directory], |row| {
                let raw = (0..column_count)
                    .map(|i| row.get::<_, rusqlite::types::Value>(i))
                    .collect::<Result<Vec<_>, _>>()?;
                let skill = InstalledSkill {
                    id: row.get("id")?,
                    name: row.get("name")?,
                    description: row.get("description")?,
                    directory: row.get("directory")?,
                    repo_owner: row.get("repo_owner")?,
                    repo_name: row.get("repo_name")?,
                    repo_branch: row.get("repo_branch")?,
                    readme_url: row.get("readme_url")?,
                    apps: SkillApps {
                        claude: row.get("enabled_claude")?,
                        codex: row.get("enabled_codex")?,
                        gemini: row.get("enabled_gemini")?,
                        opencode: row.get("enabled_opencode")?,
                        hermes: row.get("enabled_hermes")?,
                        pi: false,
                    },
                    installed_at: row.get("installed_at")?,
                    content_hash: row.get("content_hash")?,
                    updated_at: row.get("updated_at")?,
                };
                Ok((skill, raw))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut snapshot = LocalSkillBaseline {
            directory: directory.to_string(),
            skill: None,
            rows: Vec::new(),
        };
        for row in rows {
            let (skill, raw) = row.map_err(|e| AppError::Database(e.to_string()))?;
            if snapshot.skill.is_some() {
                return Err(AppError::Message(
                    "Ambiguous same-name skill records".into(),
                ));
            }
            snapshot.skill = Some(skill);
            snapshot.rows.push(raw);
        }
        Ok(snapshot)
    }

    pub(crate) fn capture_local_skill_baseline(
        &self,
        directory: &str,
    ) -> Result<LocalSkillBaseline, AppError> {
        let conn = lock_conn!(self.conn);
        Self::local_skill_baseline_on_connection(&conn, directory)
    }

    /// Capture before preparing any pinned publication files. The snapshot must
    /// describe the same `current` used to choose deployment destinations.
    pub(crate) fn capture_pinned_skill_baseline(
        &self,
        current: &InstalledSkill,
    ) -> Result<LocalSkillBaseline, AppError> {
        let baseline = self.capture_local_skill_baseline(&current.directory)?;
        let Some(stored) = baseline.skill.as_ref() else {
            return Err(AppError::Message(
                "Pinned skill was removed; publication rejected".into(),
            ));
        };
        // Pi enablement is discovered from the filesystem, not stored in SQLite.
        let mut apps = current.apps.clone();
        apps.pi = false;
        if stored.id != current.id
            || stored.name != current.name
            || stored.description != current.description
            || stored.directory != current.directory
            || stored.repo_owner != current.repo_owner
            || stored.repo_name != current.repo_name
            || stored.repo_branch != current.repo_branch
            || stored.readme_url != current.readme_url
            || stored.apps != apps
            || stored.installed_at != current.installed_at
            || stored.content_hash != current.content_hash
            || stored.updated_at != current.updated_at
        {
            return Err(AppError::Message(
                "Pinned skill baseline does not match current; publication rejected".into(),
            ));
        }
        Ok(baseline)
    }

    /// Update source/content only; never recreate a deleted pinned skill.
    pub(crate) fn save_pinned_skill_if_unchanged(
        &self,
        baseline: &LocalSkillBaseline,
        skill: &InstalledSkill,
    ) -> Result<(), AppError> {
        if baseline.skill.is_none() {
            return Err(AppError::InvalidInput(
                "Pinned publication requires an existing skill baseline".into(),
            ));
        }
        self.save_skill_publication_if_unchanged(baseline, skill, true)
    }

    /// Cross-process compare-and-save. A writer cannot change flags/provenance
    /// between the comparison and update because BEGIN IMMEDIATE owns the write lock.
    pub(crate) fn save_local_skill_if_unchanged(
        &self,
        baseline: &LocalSkillBaseline,
        skill: &InstalledSkill,
    ) -> Result<(), AppError> {
        self.save_skill_publication_if_unchanged(baseline, skill, false)
    }

    fn save_skill_publication_if_unchanged(
        &self,
        baseline: &LocalSkillBaseline,
        skill: &InstalledSkill,
        pinned: bool,
    ) -> Result<(), AppError> {
        if baseline.directory != skill.directory {
            return Err(AppError::InvalidInput(
                if pinned {
                    "Pinned baseline directory mismatch"
                } else {
                    "Local baseline directory mismatch"
                }
                .into(),
            ));
        }
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| AppError::Database(e.to_string()))?;
        let current = Self::local_skill_baseline_on_connection(&tx, &baseline.directory)?;
        if current.rows != baseline.rows {
            return Err(AppError::Message(
                if pinned {
                    "Concurrent pinned skill metadata change; publication rejected"
                } else {
                    "Concurrent local skill metadata change; publication rejected"
                }
                .into(),
            ));
        }
        // Also refuse a poisoned/foreign identical primary key under another directory.
        let ids: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM skills WHERE id = ?1 AND directory != ?2",
                params![skill.id, skill.directory],
                |r| r.get(0),
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        if ids != 0 {
            return Err(AppError::Message(
                if pinned {
                    "Foreign pinned skill identity conflict"
                } else {
                    "Foreign local skill identity conflict"
                }
                .into(),
            ));
        }
        if let Some(previous) = &baseline.skill {
            if previous.id != skill.id {
                return Err(AppError::InvalidInput(
                    if pinned {
                        "Pinned baseline identity mismatch"
                    } else {
                        "Local baseline identity mismatch"
                    }
                    .into(),
                ));
            }
            // Keep every enabled_* column (including desktop-only future columns)
            // and installed_at verbatim; this operation updates source/content only.
            if pinned {
                tx.execute(
                    "UPDATE skills SET name = ?1, description = ?2, repo_owner = ?3,
                        repo_name = ?4, repo_branch = ?5, readme_url = ?6,
                        content_hash = ?7, updated_at = ?8 WHERE id = ?9",
                    params![
                        skill.name,
                        skill.description,
                        skill.repo_owner,
                        skill.repo_name,
                        skill.repo_branch,
                        skill.readme_url,
                        skill.content_hash,
                        skill.updated_at,
                        skill.id,
                    ],
                )
                .map_err(|e| AppError::Database(e.to_string()))?;
            } else {
                tx.execute("UPDATE skills SET name = ?1, description = ?2, readme_url = ?3, content_hash = ?4, updated_at = ?5 WHERE id = ?6",
                    params![skill.name, skill.description, skill.readme_url, skill.content_hash, skill.updated_at, skill.id])
                    .map_err(|e| AppError::Database(e.to_string()))?;
            }
        } else {
            Self::save_skill_on_connection(&tx, skill)?;
        }
        tx.commit().map_err(|e| AppError::Database(e.to_string()))
    }
    // ========== InstalledSkill CRUD ==========

    /// 获取所有已安装的 Skills
    pub fn get_all_installed_skills(&self) -> Result<IndexMap<String, InstalledSkill>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, name, description, directory, repo_owner, repo_name, repo_branch,
                        readme_url, enabled_claude, enabled_codex, enabled_gemini, enabled_opencode, enabled_hermes,
                        installed_at, content_hash, updated_at
                 FROM skills ORDER BY name ASC",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let skill_iter = stmt
            .query_map([], |row| {
                Ok(InstalledSkill {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    directory: row.get(3)?,
                    repo_owner: row.get(4)?,
                    repo_name: row.get(5)?,
                    repo_branch: row.get(6)?,
                    readme_url: row.get(7)?,
                    apps: SkillApps {
                        claude: row.get(8)?,
                        codex: row.get(9)?,
                        gemini: row.get(10)?,
                        opencode: row.get(11)?,
                        hermes: row.get(12)?,
                        pi: false,
                    },
                    installed_at: row.get(13)?,
                    content_hash: row.get(14)?,
                    updated_at: row.get(15)?,
                })
            })
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut skills = IndexMap::new();
        for skill_res in skill_iter {
            let skill = skill_res.map_err(|e| AppError::Database(e.to_string()))?;
            skills.insert(skill.id.clone(), skill);
        }
        Ok(skills)
    }

    /// 获取单个已安装的 Skill
    pub fn get_installed_skill(&self, id: &str) -> Result<Option<InstalledSkill>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, name, description, directory, repo_owner, repo_name, repo_branch,
                        readme_url, enabled_claude, enabled_codex, enabled_gemini, enabled_opencode, enabled_hermes,
                        installed_at, content_hash, updated_at
                 FROM skills WHERE id = ?1",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let result = stmt.query_row([id], |row| {
            Ok(InstalledSkill {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                directory: row.get(3)?,
                repo_owner: row.get(4)?,
                repo_name: row.get(5)?,
                repo_branch: row.get(6)?,
                readme_url: row.get(7)?,
                apps: SkillApps {
                    claude: row.get(8)?,
                    codex: row.get(9)?,
                    gemini: row.get(10)?,
                    opencode: row.get(11)?,
                    hermes: row.get(12)?,
                    pi: false,
                },
                installed_at: row.get(13)?,
                content_hash: row.get(14)?,
                updated_at: row.get(15)?,
            })
        });

        match result {
            Ok(skill) => Ok(Some(skill)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    /// 保存 Skill（添加或更新）
    pub fn save_skill(&self, skill: &InstalledSkill) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        Self::save_skill_on_connection(&conn, skill)
    }

    fn save_skill_on_connection(
        conn: &rusqlite::Connection,
        skill: &InstalledSkill,
    ) -> Result<(), AppError> {
        conn.execute(
            "INSERT INTO skills
             (id, name, description, directory, repo_owner, repo_name, repo_branch,
              readme_url, enabled_claude, enabled_codex, enabled_gemini, enabled_opencode, enabled_hermes,
              installed_at, content_hash, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                directory = excluded.directory,
                repo_owner = excluded.repo_owner,
                repo_name = excluded.repo_name,
                repo_branch = excluded.repo_branch,
                readme_url = excluded.readme_url,
                enabled_claude = excluded.enabled_claude,
                enabled_codex = excluded.enabled_codex,
                enabled_gemini = excluded.enabled_gemini,
                enabled_opencode = excluded.enabled_opencode,
                enabled_hermes = excluded.enabled_hermes,
                installed_at = excluded.installed_at,
                content_hash = excluded.content_hash,
                updated_at = excluded.updated_at",
            params![
                skill.id,
                skill.name,
                skill.description,
                skill.directory,
                skill.repo_owner,
                skill.repo_name,
                skill.repo_branch,
                skill.readme_url,
                skill.apps.claude,
                skill.apps.codex,
                skill.apps.gemini,
                skill.apps.opencode,
                skill.apps.hermes,
                skill.installed_at,
                skill.content_hash,
                skill.updated_at,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 删除 Skill
    pub fn delete_skill(&self, id: &str) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let affected = conn
            .execute("DELETE FROM skills WHERE id = ?1", params![id])
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(affected > 0)
    }

    /// 清空所有 Skills（用于迁移）
    pub fn clear_skills(&self) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute("DELETE FROM skills", [])
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 更新 Skill 的应用启用状态
    pub fn update_skill_apps(&self, id: &str, apps: &SkillApps) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let affected = conn
            .execute(
                "UPDATE skills SET enabled_claude = ?1, enabled_codex = ?2, enabled_gemini = ?3, enabled_opencode = ?4, enabled_hermes = ?5 WHERE id = ?6",
                params![apps.claude, apps.codex, apps.gemini, apps.opencode, apps.hermes, id],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(affected > 0)
    }

    /// 更新 Skill 的内容哈希和更新时间。
    pub fn update_skill_hash(
        &self,
        id: &str,
        content_hash: &str,
        updated_at: i64,
    ) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let affected = conn
            .execute(
                "UPDATE skills SET content_hash = ?1, updated_at = ?2 WHERE id = ?3",
                params![content_hash, updated_at, id],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(affected > 0)
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

    #[test]
    fn skill_hash_fields_round_trip_without_schema_changes() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let db = Database::init().expect("initialize database");
        let skill = InstalledSkill {
            id: "owner/repo:demo".to_string(),
            name: "Demo".to_string(),
            description: None,
            directory: "demo".to_string(),
            repo_owner: Some("owner".to_string()),
            repo_name: Some("repo".to_string()),
            repo_branch: Some("main".to_string()),
            readme_url: None,
            apps: SkillApps::default(),
            installed_at: 1,
            content_hash: Some("before".to_string()),
            updated_at: 2,
        };

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
    }
}

#[cfg(test)]
mod local_compare_tests {
    use super::*;
    fn record() -> InstalledSkill {
        InstalledSkill {
            id: "local:demo".into(),
            name: "demo".into(),
            description: Some("demo".into()),
            directory: "demo".into(),
            repo_owner: None,
            repo_name: None,
            repo_branch: None,
            readme_url: Some("original".into()),
            apps: SkillApps {
                codex: true,
                ..Default::default()
            },
            installed_at: 10,
            content_hash: Some("old".into()),
            updated_at: 10,
        }
    }
    #[test]
    fn local_dao_preserves_unmodelled_flags_and_install_time() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let db = Database::init().unwrap();
        let mut row = record();
        db.save_skill(&row).unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "UPDATE skills SET enabled_mcode=1, enabled_grokbuild=1 WHERE id='local:demo'",
                [],
            )
            .unwrap();
        }
        let baseline = db.capture_local_skill_baseline("demo").unwrap();
        row.content_hash = Some("new".into());
        row.installed_at = 999;
        row.apps.codex = false;
        db.save_local_skill_if_unchanged(&baseline, &row).unwrap();
        let conn = db.conn.lock().unwrap();
        let flags: (i64,i64,i64,i64) = conn.query_row("SELECT enabled_mcode,enabled_grokbuild,enabled_codex,installed_at FROM skills WHERE id='local:demo'", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        assert_eq!(flags, (1, 1, 1, 10));
    }
    #[test]
    fn local_dao_rejects_concurrent_unmodelled_flag() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let db = Database::init().unwrap();
        let row = record();
        db.save_skill(&row).unwrap();
        let baseline = db.capture_local_skill_baseline("demo").unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "UPDATE skills SET enabled_mcode=1 WHERE id='local:demo'",
                [],
            )
            .unwrap();
        }
        assert!(db
            .save_local_skill_if_unchanged(&baseline, &row)
            .unwrap_err()
            .to_string()
            .contains("Concurrent"));
        let conn = db.conn.lock().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT enabled_mcode FROM skills WHERE id='local:demo'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
    #[test]
    fn local_dao_rejects_concurrent_foreign_first_install() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let db = Database::init().unwrap();
        let row = record();
        let baseline = db.capture_local_skill_baseline("demo").unwrap();
        let mut foreign = row.clone();
        foreign.id = "foreign/repository:demo".into();
        db.save_skill(&foreign).unwrap();
        assert!(db.save_local_skill_if_unchanged(&baseline, &row).is_err());
        assert!(db.get_installed_skill("local:demo").unwrap().is_none());
        assert!(db.get_installed_skill(&foreign.id).unwrap().is_some());
    }
}

#[cfg(test)]
mod pinned_compare_tests {
    use super::*;
    use rusqlite::{types::Value, Connection};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    // Separate connections to a real disk file: the peer bypasses Database's
    // mutex just as another CC Switch process does. No host config is touched.
    fn fixture() -> (tempfile::TempDir, Database, Connection, InstalledSkill) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skills.db");
        let db = Database {
            conn: Mutex::new(Connection::open(&path).unwrap()),
            runtime_key: format!("file:{}", path.display()),
            db_path: Some(path.clone()),
        };
        db.create_tables().unwrap();
        let peer = Connection::open(&path).unwrap();
        peer.execute_batch(
            "ALTER TABLE skills ADD COLUMN opaque_metadata BLOB;
             ALTER TABLE skills ADD COLUMN enabled_future INTEGER NOT NULL DEFAULT 0;",
        )
        .unwrap();
        let current = InstalledSkill {
            id: "owner/repo:demo".into(),
            name: "demo".into(),
            description: Some("old description".into()),
            directory: "demo".into(),
            repo_owner: Some("owner".into()),
            repo_name: Some("repo".into()),
            repo_branch: Some("main".into()),
            readme_url: Some("original".into()),
            apps: SkillApps {
                codex: true,
                ..Default::default()
            },
            installed_at: 10,
            content_hash: Some("old".into()),
            updated_at: 20,
        };
        db.save_skill(&current).unwrap();
        peer.execute(
            "UPDATE skills SET enabled_mcode = 1, enabled_grokbuild = 1,
                enabled_future = 7, opaque_metadata = X'00FF8041' WHERE id = ?1",
            [&current.id],
        )
        .unwrap();
        (dir, db, peer, current)
    }

    fn updated(current: &InstalledSkill) -> InstalledSkill {
        let mut skill = current.clone();
        skill.name = "new name".into();
        skill.description = Some("new description".into());
        skill.repo_branch = Some("release".into());
        skill.readme_url = Some("pinned source".into());
        skill.content_hash = Some("new".into());
        skill.updated_at = 30;
        // Publication must ignore even deliberately stale/incorrect input values.
        skill.apps = SkillApps {
            claude: true,
            ..Default::default()
        };
        skill.installed_at = 999;
        skill
    }

    fn raw_row(conn: &Connection, id: &str) -> Option<BTreeMap<String, Value>> {
        let mut stmt = conn.prepare("SELECT * FROM skills WHERE id = ?1").unwrap();
        let names: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();
        let mut rows = stmt.query([id]).unwrap();
        rows.next().unwrap().map(|row| {
            names
                .into_iter()
                .enumerate()
                .map(|(i, name)| (name, row.get::<_, Value>(i).unwrap()))
                .collect()
        })
    }

    #[test]
    fn pinned_dao_preserves_flags_opaque_columns_and_install_time() {
        let (_dir, db, peer, mut current) = fixture();
        // Pi is derived from files; it must not invalidate the SQLite baseline.
        current.apps.pi = true;
        let baseline = db.capture_pinned_skill_baseline(&current).unwrap();
        let mut next = updated(&current);
        next.repo_owner = Some("new owner".into());
        next.repo_name = Some("new repo".into());
        let mut expected = raw_row(&peer, &current.id).unwrap();
        for (column, value) in [
            ("name", Value::Text(next.name.clone())),
            (
                "description",
                Value::Text(next.description.clone().unwrap()),
            ),
            ("repo_owner", Value::Text(next.repo_owner.clone().unwrap())),
            ("repo_name", Value::Text(next.repo_name.clone().unwrap())),
            (
                "repo_branch",
                Value::Text(next.repo_branch.clone().unwrap()),
            ),
            ("readme_url", Value::Text(next.readme_url.clone().unwrap())),
            (
                "content_hash",
                Value::Text(next.content_hash.clone().unwrap()),
            ),
            ("updated_at", Value::Integer(next.updated_at)),
        ] {
            expected.insert(column.into(), value);
        }
        db.save_pinned_skill_if_unchanged(&baseline, &next).unwrap();
        // Compare the entire raw row, including every field not in our model.
        assert_eq!(raw_row(&peer, &current.id).unwrap(), expected);
    }

    #[test]
    fn pinned_dao_rejects_external_enablement_changes() {
        let (_dir, db, peer, current) = fixture();
        for column in [
            "enabled_claude",
            "enabled_codex",
            "enabled_gemini",
            "enabled_opencode",
            "enabled_hermes",
            "enabled_mcode",
            "enabled_grokbuild",
            "enabled_future",
        ] {
            let current = db.get_installed_skill(&current.id).unwrap().unwrap();
            let baseline = db.capture_pinned_skill_baseline(&current).unwrap();
            peer.execute(
                &format!("UPDATE skills SET {column} = NOT {column} WHERE id = ?1"),
                [&current.id],
            )
            .unwrap();
            let external = raw_row(&peer, &current.id).unwrap();
            let error = db
                .save_pinned_skill_if_unchanged(&baseline, &updated(&current))
                .unwrap_err();
            assert!(
                error.to_string().contains("Concurrent"),
                "{column}: {error}"
            );
            assert_eq!(raw_row(&peer, &current.id).unwrap(), external, "{column}");
            assert!(db.conn.lock().unwrap().is_autocommit());
        }
    }

    #[test]
    fn pinned_dao_rejects_external_delete_without_recreating_row() {
        let (_dir, db, peer, current) = fixture();
        let baseline = db.capture_pinned_skill_baseline(&current).unwrap();
        peer.execute("DELETE FROM skills WHERE id = ?1", [&current.id])
            .unwrap();
        assert!(db
            .save_pinned_skill_if_unchanged(&baseline, &updated(&current))
            .unwrap_err()
            .to_string()
            .contains("Concurrent"));
        assert!(raw_row(&peer, &current.id).is_none());
    }

    #[test]
    fn pinned_dao_rejects_external_opaque_metadata_change() {
        let (_dir, db, peer, current) = fixture();
        let baseline = db.capture_pinned_skill_baseline(&current).unwrap();
        peer.execute(
            "UPDATE skills SET opaque_metadata = X'FE0081FF' WHERE id = ?1",
            [&current.id],
        )
        .unwrap();
        let external = raw_row(&peer, &current.id).unwrap();
        assert!(db
            .save_pinned_skill_if_unchanged(&baseline, &updated(&current))
            .unwrap_err()
            .to_string()
            .contains("Concurrent"));
        assert_eq!(raw_row(&peer, &current.id).unwrap(), external);
    }

    #[test]
    fn pinned_dao_requires_baseline_to_match_current_before_file_preparation() {
        let (_dir, db, peer, current) = fixture();
        // Changes before capture must not become a silently accepted baseline
        // for deployment decisions made from an older current record.
        for (column, value) in [
            ("enabled_codex", "0"),
            ("repo_branch", "'release'"),
            ("name", "'changed'"),
            ("installed_at", "999"),
        ] {
            peer.execute(
                &format!("UPDATE skills SET {column} = {value} WHERE id = ?1"),
                [&current.id],
            )
            .unwrap();
            assert!(db.capture_pinned_skill_baseline(&current).is_err());
            db.save_skill(&current).unwrap();
        }
        peer.execute("DELETE FROM skills WHERE id = ?1", [&current.id])
            .unwrap();
        assert!(db.capture_pinned_skill_baseline(&current).is_err());
        let empty = db.capture_local_skill_baseline(&current.directory).unwrap();
        assert!(db
            .save_pinned_skill_if_unchanged(&empty, &updated(&current))
            .is_err());
        assert!(raw_row(&peer, &current.id).is_none());
    }

    #[test]
    fn pinned_dao_rejects_identity_or_directory_changes() {
        let (_dir, db, peer, current) = fixture();
        let baseline = db.capture_pinned_skill_baseline(&current).unwrap();
        let before = raw_row(&peer, &current.id).unwrap();
        let mut next = updated(&current);
        next.id = "foreign/repo:demo".into();
        assert!(db.save_pinned_skill_if_unchanged(&baseline, &next).is_err());
        next.id = current.id.clone();
        next.directory = "other".into();
        assert!(db.save_pinned_skill_if_unchanged(&baseline, &next).is_err());
        assert_eq!(raw_row(&peer, &current.id).unwrap(), before);
        assert!(raw_row(&peer, "foreign/repo:demo").is_none());
    }
}
