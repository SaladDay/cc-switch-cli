//! Skills service layer
//!
//! v3.10.0+ 统一管理架构（与上游一致）：
//! - SSOT（单一事实源）：`~/.cc-switch/skills/`
//! - 数据库存储安装记录、启用状态与仓库列表（`~/.cc-switch/cc-switch.db`）

mod discovery;

use chrono::{DateTime, Utc};
use futures::future::join_all;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use tokio::time::timeout;

use crate::app_config::AppType;
pub use crate::app_config::{InstalledSkill, SkillApps, UnmanagedSkill};
use crate::config::{create_managed_config_dir_all, get_app_config_dir};
use crate::database::Database;
use crate::error::{format_skill_error, AppError};

const SKILLS_INDEX_VERSION: u32 = 1;
const MAX_SKILL_ARCHIVE_ENTRIES: usize = 10_000;
const MAX_SKILL_ARCHIVE_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SKILL_ARCHIVE_DOWNLOAD_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SKILL_ARCHIVE_PATH_DEPTH: usize = 64;
const SKILL_ARCHIVE_ENTRY_COST: u64 = 4096;

fn default_skills_index_version() -> u32 {
    SKILLS_INDEX_VERSION
}

// ============================================================================
// Legacy (v2) store structures - kept for backward compatibility
// ============================================================================

/// Skill repository configuration (legacy, kept for backward compatibility).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRepo {
    /// GitHub 用户/组织名
    pub owner: String,
    /// 仓库名称
    pub name: String,
    /// 分支 (默认 "main")
    pub branch: String,
    /// 是否启用
    pub enabled: bool,
}

/// Legacy install state: directory -> installed timestamp (Claude-only era).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillState {
    /// 是否已安装
    pub installed: bool,
    /// 安装时间
    #[serde(rename = "installedAt")]
    pub installed_at: DateTime<Utc>,
}

/// Legacy persistent store (was embedded in config.json in older CLI versions).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillStore {
    /// directory -> 安装状态
    pub skills: HashMap<String, SkillState>,
    /// 仓库列表
    pub repos: Vec<SkillRepo>,
}

impl Default for SkillStore {
    fn default() -> Self {
        SkillStore {
            skills: HashMap::new(),
            // Keep aligned with upstream defaults where possible.
            repos: vec![
                SkillRepo {
                    owner: "anthropics".to_string(),
                    name: "skills".to_string(),
                    branch: "main".to_string(),
                    enabled: true,
                },
                SkillRepo {
                    owner: "ComposioHQ".to_string(),
                    name: "awesome-claude-skills".to_string(),
                    branch: "master".to_string(),
                    enabled: true,
                },
                SkillRepo {
                    owner: "cexll".to_string(),
                    name: "myclaude".to_string(),
                    branch: "master".to_string(),
                    enabled: true,
                },
                SkillRepo {
                    owner: "JimLiu".to_string(),
                    name: "baoyu-skills".to_string(),
                    branch: "main".to_string(),
                    enabled: true,
                },
            ],
        }
    }
}

// ============================================================================
// New (Phase 3) SSOT-based model persisted to ~/.cc-switch/skills.json (no DB)
// ============================================================================

/// Skill sync method (upstream-aligned).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum SyncMethod {
    /// Auto choose: prefer symlink, fallback to copy.
    #[default]
    Auto,
    /// Always use symlink.
    Symlink,
    /// Always use directory copy.
    Copy,
}

/// Explicit app matrix submitted when importing unmanaged skills.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSkillSelection {
    pub directory: String,
    #[serde(default)]
    pub apps: SkillApps,
}

/// skills.json (SSOT index; no DB).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsIndex {
    #[serde(default = "default_skills_index_version")]
    pub version: u32,
    #[serde(default)]
    pub sync_method: SyncMethod,
    #[serde(default)]
    pub repos: Vec<SkillRepo>,
    /// directory -> record
    #[serde(default)]
    pub skills: HashMap<String, InstalledSkill>,
    /// One-time SSOT migration flag (scan app dirs -> copy into SSOT -> build records).
    #[serde(default)]
    pub ssot_migration_pending: bool,
}

impl Default for SkillsIndex {
    fn default() -> Self {
        Self {
            version: SKILLS_INDEX_VERSION,
            sync_method: SyncMethod::default(),
            repos: SkillStore::default().repos,
            skills: HashMap::new(),
            ssot_migration_pending: false,
        }
    }
}

// ============================================================================
// Discovery types (repo scanning)
// ============================================================================

/// Discoverable skill (from GitHub repos).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverableSkill {
    /// Unique key: "owner/name:directory"
    pub key: String,
    pub name: String,
    pub description: String,
    /// Directory name (the final path segment)
    pub directory: String,
    #[serde(rename = "readmeUrl")]
    pub readme_url: Option<String>,
    #[serde(rename = "repoOwner")]
    pub repo_owner: String,
    #[serde(rename = "repoName")]
    pub repo_name: String,
    #[serde(rename = "repoBranch")]
    pub repo_branch: String,
}

/// CLI-friendly skill object (discoverable + installed flag).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub key: String,
    pub name: String,
    pub description: String,
    pub directory: String,
    #[serde(rename = "readmeUrl")]
    pub readme_url: Option<String>,
    pub installed: bool,
    #[serde(rename = "repoOwner")]
    pub repo_owner: Option<String>,
    #[serde(rename = "repoName")]
    pub repo_name: Option<String>,
    #[serde(rename = "repoBranch")]
    pub repo_branch: Option<String>,
}

/// One installed Skill whose repository content differs from the local copy.
#[derive(Debug, Clone)]
pub struct SkillUpdateInfo {
    pub id: String,
    pub name: String,
    pub directory: String,
    pub current_hash: Option<String>,
    pub remote_hash: String,
}

#[derive(Debug, Clone, Default)]
pub struct SkillUpdateCheckResult {
    pub updates: Vec<SkillUpdateInfo>,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SkillUpdateFailure {
    pub id: String,
    pub error: String,
}

#[derive(Debug, Clone, Default)]
pub struct SkillUpdateBatchResult {
    pub updated: Vec<InstalledSkill>,
    pub failures: Vec<SkillUpdateFailure>,
}

struct SkillUpdateOutcome {
    skill: InstalledSkill,
    deployment_failures: Vec<String>,
}

struct DownloadedRepoGuard(PathBuf);

impl DownloadedRepoGuard {
    fn new(path: PathBuf) -> Self {
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for DownloadedRepoGuard {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            log::debug!("Failed to remove temporary Skill repo: {error}");
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct SkillsShApiResponse {
    pub query: String,
    #[serde(rename = "searchType")]
    #[allow(dead_code)]
    pub search_type: String,
    pub skills: Vec<SkillsShApiSkill>,
    pub count: usize,
    #[allow(dead_code)]
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct SkillsShApiSkill {
    #[allow(dead_code)]
    pub id: String,
    #[serde(rename = "skillId")]
    pub skill_id: String,
    pub name: String,
    pub installs: u64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsShSearchResult {
    pub skills: Vec<SkillsShDiscoverableSkill>,
    pub total_count: usize,
    pub query: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsShDiscoverableSkill {
    pub key: String,
    pub name: String,
    pub directory: String,
    pub repo_owner: String,
    pub repo_name: String,
    pub repo_branch: String,
    pub installs: u64,
    pub readme_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SkillsDiscoverCache {
    version: u32,
    repos_fingerprint: String,
    skills: Vec<Skill>,
}

impl From<SkillsShDiscoverableSkill> for DiscoverableSkill {
    fn from(skill: SkillsShDiscoverableSkill) -> Self {
        Self {
            key: skill.key,
            name: skill.name,
            description: String::new(),
            directory: skill.directory,
            readme_url: skill.readme_url,
            repo_owner: skill.repo_owner,
            repo_name: skill.repo_name,
            repo_branch: skill.repo_branch,
        }
    }
}

fn skills_sh_api_skill_to_discoverable(
    skill: SkillsShApiSkill,
) -> Option<SkillsShDiscoverableSkill> {
    let (owner, repo) = skill.source.split_once('/')?;
    if owner.contains('.')
        || repo.contains('.')
        || owner.trim().is_empty()
        || repo.trim().is_empty()
    {
        return None;
    }

    Some(SkillsShDiscoverableSkill {
        key: format!("{owner}/{repo}:{}", skill.skill_id),
        name: skill.name,
        directory: skill.skill_id,
        repo_owner: owner.to_string(),
        repo_name: repo.to_string(),
        repo_branch: "main".to_string(),
        installs: skill.installs,
        readme_url: Some(format!("https://github.com/{owner}/{repo}")),
    })
}

fn discoverable_from_repo_spec(spec: &str) -> Option<DiscoverableSkill> {
    let (repo_spec, directory) = spec.split_once(':')?;
    let (owner, repo) = repo_spec.split_once('/')?;
    let owner = owner.trim();
    let repo = repo.trim();
    let directory = directory.trim();
    if owner.is_empty() || repo.is_empty() || directory.is_empty() {
        return None;
    }

    Some(DiscoverableSkill {
        key: spec.to_string(),
        name: directory.to_string(),
        description: String::new(),
        directory: directory.to_string(),
        readme_url: Some(format!("https://github.com/{owner}/{repo}")),
        repo_owner: owner.to_string(),
        repo_name: repo.to_string(),
        repo_branch: "main".to_string(),
    })
}

/// Skill metadata extracted from SKILL.md YAML front matter.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillMetadata {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Deserialize)]
struct AgentsLockFile {
    skills: HashMap<String, AgentsLockSkill>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentsLockSkill {
    source: Option<String>,
    source_type: Option<String>,
    source_url: Option<String>,
    skill_path: Option<String>,
    branch: Option<String>,
    source_branch: Option<String>,
}

#[derive(Debug, Clone)]
struct LockRepoInfo {
    owner: String,
    repo: String,
    skill_path: Option<String>,
    branch: Option<String>,
}

fn normalize_optional_branch(branch: Option<String>) -> Option<String> {
    branch.and_then(|branch| {
        let trimmed = branch.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn parse_branch_from_source_url(source_url: Option<&str>) -> Option<String> {
    let source_url = source_url?.trim();
    if source_url.is_empty() {
        return None;
    }

    if let Some((_, after_tree)) = source_url.split_once("/tree/") {
        let branch = after_tree.split('/').next()?.trim();
        if !branch.is_empty() {
            return Some(branch.to_string());
        }
    }

    if let Some((_, fragment)) = source_url.split_once('#') {
        let branch = fragment.split('&').next()?.trim();
        if !branch.is_empty() {
            return Some(branch.to_string());
        }
    }

    if let Some((_, query)) = source_url.split_once('?') {
        for pair in query.split('&') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            if matches!(key, "branch" | "ref") {
                let branch = value.trim();
                if !branch.is_empty() {
                    return Some(branch.to_string());
                }
            }
        }
    }

    None
}

fn get_agents_skills_dir() -> Option<PathBuf> {
    dirs::home_dir()
        .map(|home| home.join(".agents").join("skills"))
        .filter(|path| path.exists())
}

fn parse_agents_lock() -> HashMap<String, LockRepoInfo> {
    let path = match dirs::home_dir() {
        Some(home) => home.join(".agents").join(".skill-lock.json"),
        None => return HashMap::new(),
    };

    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(_) => return HashMap::new(),
    };

    let lock: AgentsLockFile = match serde_json::from_str(&content) {
        Ok(lock) => lock,
        Err(_) => return HashMap::new(),
    };

    lock.skills
        .into_iter()
        .filter_map(|(name, skill)| {
            let source = skill.source?;
            if skill.source_type.as_deref() != Some("github") {
                return None;
            }
            let (owner, repo) = source.split_once('/')?;
            let branch = normalize_optional_branch(skill.branch)
                .or_else(|| normalize_optional_branch(skill.source_branch))
                .or_else(|| parse_branch_from_source_url(skill.source_url.as_deref()));
            Some((
                name,
                LockRepoInfo {
                    owner: owner.to_string(),
                    repo: repo.to_string(),
                    skill_path: skill.skill_path,
                    branch,
                },
            ))
        })
        .collect()
}

fn build_repo_info_from_lock(
    lock: &HashMap<String, LockRepoInfo>,
    dir_name: &str,
) -> (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    match lock.get(dir_name) {
        Some(info) => {
            let branch = info.branch.clone();
            let url_branch = branch.clone().unwrap_or_else(|| "HEAD".to_string());
            let fallback = format!("{dir_name}/SKILL.md");
            let doc_path = info.skill_path.as_deref().unwrap_or(&fallback);
            let url = Some(SkillService::build_skill_doc_url(
                &info.owner,
                &info.repo,
                &url_branch,
                doc_path,
            ));
            (
                format!("{}/{}:{dir_name}", info.owner, info.repo),
                Some(info.owner.clone()),
                Some(info.repo.clone()),
                branch,
                url,
            )
        }
        None => (format!("local:{dir_name}"), None, None, None, None),
    }
}

fn merge_repos_from_lock(
    repos: &mut Vec<SkillRepo>,
    lock: &HashMap<String, LockRepoInfo>,
    directories: impl Iterator<Item = impl AsRef<str>>,
) {
    let mut existing: HashSet<(String, String)> = repos
        .iter()
        .map(|repo| (repo.owner.clone(), repo.name.clone()))
        .collect();

    for dir_name in directories {
        if let Some(info) = lock.get(dir_name.as_ref()) {
            let key = (info.owner.clone(), info.repo.clone());
            if existing.insert(key) {
                repos.push(SkillRepo {
                    owner: info.owner.clone(),
                    name: info.repo.clone(),
                    branch: info.branch.clone().unwrap_or_else(|| "HEAD".to_string()),
                    enabled: true,
                });
            }
        }
    }
}

// ============================================================================
// SkillService
// ============================================================================

pub struct SkillService {
    http_client: Client,
}

impl SkillService {
    fn app_supports_skills(app: &AppType) -> bool {
        !matches!(app, AppType::OpenClaw)
    }

    pub fn supported_skill_apps() -> impl Iterator<Item = AppType> {
        [
            AppType::Claude,
            AppType::Codex,
            AppType::Gemini,
            AppType::OpenCode,
            AppType::Hermes,
        ]
        .into_iter()
    }

    fn skill_source_apps() -> impl Iterator<Item = AppType> {
        AppType::all()
    }

    pub fn new() -> Result<Self, AppError> {
        let http_client = Client::builder()
            .user_agent("cc-switch")
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| {
                AppError::localized(
                    "skills.http_client_failed",
                    format!("创建 HTTP 客户端失败: {e}"),
                    format!("Failed to create HTTP client: {e}"),
                )
            })?;

        Ok(Self { http_client })
    }

    // ---------------------------------------------------------------------
    // Paths
    // ---------------------------------------------------------------------

    pub fn get_ssot_dir() -> Result<PathBuf, AppError> {
        let dir = get_app_config_dir().join("skills");
        create_managed_config_dir_all(&dir)?;
        Ok(dir)
    }

    pub fn get_app_skills_dir(app: &AppType) -> Result<PathBuf, AppError> {
        // Override directories follow the same pattern as upstream: <override>/skills
        match app {
            AppType::Claude => {
                if let Some(custom) = crate::settings::get_claude_override_dir() {
                    return Ok(custom.join("skills"));
                }
            }
            AppType::Codex => {
                if let Some(custom) = crate::settings::get_codex_override_dir() {
                    return Ok(custom.join("skills"));
                }
            }
            AppType::Gemini => {
                if let Some(custom) = crate::settings::get_gemini_override_dir() {
                    return Ok(custom.join("skills"));
                }
            }
            AppType::OpenCode => {
                if let Some(custom) = crate::settings::get_opencode_override_dir() {
                    return Ok(custom.join("skills"));
                }
            }
            AppType::Hermes => {
                if let Some(custom) = crate::settings::get_hermes_override_dir() {
                    return Ok(custom.join("skills"));
                }
            }
            AppType::OpenClaw => {
                if let Some(custom) = crate::settings::get_openclaw_override_dir() {
                    return Ok(custom.join("skills"));
                }
            }
        }

        let home = dirs::home_dir().ok_or_else(|| {
            AppError::Message(format_skill_error(
                "GET_HOME_DIR_FAILED",
                &[],
                Some("checkPermission"),
            ))
        })?;

        Ok(match app {
            AppType::Claude => home.join(".claude").join("skills"),
            AppType::Codex => home.join(".codex").join("skills"),
            AppType::Gemini => home.join(".gemini").join("skills"),
            AppType::OpenCode => home.join(".config").join("opencode").join("skills"),
            AppType::Hermes => home.join(".hermes").join("skills"),
            AppType::OpenClaw => home.join(".openclaw").join("skills"),
        })
    }

    // ---------------------------------------------------------------------
    // Storage (SQLite + settings.json)
    // ---------------------------------------------------------------------

    pub fn load_index() -> Result<SkillsIndex, AppError> {
        let db = Database::init()?;

        // Ensure default repos exist (insert-missing only).
        let _ = db.init_default_skill_repos();

        let repos = db.get_skill_repos()?;
        let installed = db.get_all_installed_skills()?;
        let skills: HashMap<String, InstalledSkill> = installed
            .into_values()
            .map(|skill| (skill.directory.clone(), skill))
            .collect();

        let sync_method = crate::settings::get_skill_sync_method();
        let ssot_migration_pending = db
            .get_setting("skills_ssot_migration_pending")?
            .is_some_and(|v| v == "true" || v == "1");

        Ok(SkillsIndex {
            version: SKILLS_INDEX_VERSION,
            sync_method,
            repos,
            skills,
            ssot_migration_pending,
        })
    }

    pub fn save_index(index: &SkillsIndex) -> Result<(), AppError> {
        let db = Database::init()?;

        crate::settings::set_skill_sync_method(index.sync_method)?;

        for repo in &index.repos {
            db.save_skill_repo(repo)?;
        }

        for skill in index.skills.values() {
            db.save_skill(skill)?;
        }

        Ok(())
    }

    // ---------------------------------------------------------------------
    // One-time SSOT migration (scan app dirs -> copy to SSOT -> record in index)
    // ---------------------------------------------------------------------

    pub fn migrate_ssot_if_pending(index: &mut SkillsIndex) -> Result<usize, AppError> {
        if !index.ssot_migration_pending {
            return Ok(0);
        }

        let db = Database::init()?;
        let ssot_dir = Self::get_ssot_dir()?;
        let mut created = 0usize;

        // Safety guard (upstream-aligned):
        // - If we already have managed skills in the index, do NOT auto-import everything
        //   from app dirs (that could unexpectedly "claim" user directories as managed).
        // - Instead, only try to populate SSOT for the already-managed skills (best effort),
        //   then clear the pending flag.
        if !index.skills.is_empty() {
            for (directory, record) in index.skills.iter_mut() {
                let dest = ssot_dir.join(directory);
                if dest.exists() {
                    continue;
                }

                // Prefer looking in apps where this skill is enabled; fallback to all apps.
                let mut candidates: Vec<AppType> = Self::supported_skill_apps()
                    .filter(|app| record.apps.is_enabled_for(app))
                    .collect();
                if candidates.is_empty() {
                    candidates = Self::supported_skill_apps().collect();
                }

                let mut source: Option<PathBuf> = None;
                for app in candidates {
                    let app_dir = match Self::get_app_skills_dir(&app) {
                        Ok(d) => d,
                        Err(_) => continue,
                    };
                    let skill_path = app_dir.join(directory);
                    if skill_path.exists() {
                        source = Some(skill_path);
                        break;
                    }
                }

                match source {
                    Some(source) => {
                        Self::copy_dir_recursive(&source, &dest)?;
                        created += 1;

                        // Backfill metadata if missing.
                        let skill_md = dest.join("SKILL.md");
                        if skill_md.exists() {
                            if let Ok(meta) = Self::parse_skill_metadata_static(&skill_md) {
                                if record.name.trim().is_empty()
                                    || record.name.eq_ignore_ascii_case(&record.directory)
                                {
                                    record.name =
                                        meta.name.unwrap_or_else(|| record.directory.clone());
                                }
                                if record.description.is_none() {
                                    record.description = meta.description;
                                }
                            }
                        }
                    }
                    None => {
                        log::warn!(
                            "SSOT 迁移: 未找到技能目录来源（directory={directory}），已跳过复制"
                        );
                    }
                }
            }

            index.ssot_migration_pending = false;
            let _ = db.set_setting("skills_ssot_migration_pending", "false");
            Self::save_index(index)?;
            return Ok(created);
        }

        let mut discovered: HashMap<String, SkillApps> = HashMap::new();

        for app in Self::supported_skill_apps() {
            let app_dir = match Self::get_app_skills_dir(&app) {
                Ok(d) => d,
                Err(_) => continue,
            };
            if !app_dir.exists() {
                continue;
            }

            for entry in fs::read_dir(&app_dir).map_err(|e| AppError::io(&app_dir, e))? {
                let entry = entry.map_err(|e| AppError::io(&app_dir, e))?;
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let dir_name = entry.file_name().to_string_lossy().to_string();
                if dir_name.starts_with('.') {
                    continue;
                }

                // Copy to SSOT if needed.
                let ssot_path = ssot_dir.join(&dir_name);
                if !ssot_path.exists() {
                    Self::copy_dir_recursive(&path, &ssot_path)?;
                }

                discovered
                    .entry(dir_name)
                    .or_default()
                    .set_enabled_for(&app, true);
            }
        }

        // Upsert index records.
        for (directory, apps) in discovered {
            let ssot_path = ssot_dir.join(&directory);
            let skill_md = ssot_path.join("SKILL.md");
            let (name, description) = if skill_md.exists() {
                match Self::parse_skill_metadata_static(&skill_md) {
                    Ok(meta) => (
                        meta.name.unwrap_or_else(|| directory.clone()),
                        meta.description,
                    ),
                    Err(_) => (directory.clone(), None),
                }
            } else {
                (directory.clone(), None)
            };

            match index.skills.get_mut(&directory) {
                Some(existing) => {
                    existing.apps.merge_enabled(&apps);
                    if existing.name.trim().is_empty() {
                        existing.name = name;
                    }
                    if existing.description.is_none() {
                        existing.description = description;
                    }
                }
                None => {
                    index.skills.insert(
                        directory.clone(),
                        InstalledSkill {
                            id: format!("local:{directory}"),
                            name,
                            description,
                            directory: directory.clone(),
                            readme_url: None,
                            repo_owner: None,
                            repo_name: None,
                            repo_branch: None,
                            apps,
                            installed_at: Utc::now().timestamp(),
                            content_hash: Self::compute_dir_hash(&ssot_path).ok(),
                            updated_at: 0,
                        },
                    );
                    created += 1;
                }
            }
        }

        index.ssot_migration_pending = false;
        let _ = db.set_setting("skills_ssot_migration_pending", "false");
        Self::save_index(index)?;
        Ok(created)
    }

    // ---------------------------------------------------------------------
    // Sync / remove (file operations)
    // ---------------------------------------------------------------------

    #[cfg(unix)]
    fn create_symlink(src: &Path, dest: &Path) -> Result<(), AppError> {
        std::os::unix::fs::symlink(src, dest).map_err(|e| AppError::IoContext {
            context: format!("创建符号链接失败 ({} -> {})", src.display(), dest.display()),
            source: e,
        })
    }

    #[cfg(windows)]
    fn create_symlink(src: &Path, dest: &Path) -> Result<(), AppError> {
        std::os::windows::fs::symlink_dir(src, dest).map_err(|e| AppError::IoContext {
            context: format!("创建符号链接失败 ({} -> {})", src.display(), dest.display()),
            source: e,
        })
    }

    fn is_symlink(path: &Path) -> bool {
        path.symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    }

    fn remove_path(path: &Path) -> Result<(), AppError> {
        if Self::is_symlink(path) {
            #[cfg(unix)]
            fs::remove_file(path).map_err(|e| AppError::io(path, e))?;
            #[cfg(windows)]
            fs::remove_dir(path).map_err(|e| AppError::io(path, e))?;
            return Ok(());
        }

        if path.is_dir() {
            fs::remove_dir_all(path).map_err(|e| AppError::io(path, e))?;
        } else if path.exists() {
            fs::remove_file(path).map_err(|e| AppError::io(path, e))?;
        }
        Ok(())
    }

    pub fn sync_to_app_dir(
        directory: &str,
        app: &AppType,
        method: SyncMethod,
    ) -> Result<(), AppError> {
        if !Self::app_supports_skills(app) {
            return Ok(());
        }

        let ssot_dir = Self::get_ssot_dir()?;
        let source = ssot_dir.join(directory);
        if !source.exists() {
            return Err(AppError::Message(format!(
                "Skill 不存在于 SSOT: {directory}"
            )));
        }

        let app_dir = Self::get_app_skills_dir(app)?;
        // D5: allow creating target app dirs during skills sync.
        fs::create_dir_all(&app_dir).map_err(|e| AppError::io(&app_dir, e))?;

        let dest = app_dir.join(directory);
        if dest.exists() || Self::is_symlink(&dest) {
            Self::remove_path(&dest)?;
        }

        match method {
            SyncMethod::Auto => match Self::create_symlink(&source, &dest) {
                Ok(()) => Ok(()),
                Err(err) => {
                    log::warn!(
                        "Symlink 创建失败，将回退到文件复制: {} -> {}. 错误: {err}",
                        source.display(),
                        dest.display()
                    );
                    Self::copy_dir_recursive(&source, &dest)
                }
            },
            SyncMethod::Symlink => Self::create_symlink(&source, &dest),
            SyncMethod::Copy => Self::copy_dir_recursive(&source, &dest),
        }
    }

    fn sync_updated_skill_to_app(
        directory: &str,
        app: &AppType,
        method: SyncMethod,
    ) -> Result<(), AppError> {
        if !Self::app_supports_skills(app) {
            return Ok(());
        }

        let source = Self::get_ssot_dir()?.join(directory);
        if !source.is_dir() {
            return Err(AppError::Message(format!(
                "Skill does not exist in SSOT: {directory}"
            )));
        }
        let app_dir = Self::get_app_skills_dir(app)?;
        fs::create_dir_all(&app_dir).map_err(|e| AppError::io(&app_dir, e))?;
        let dest = app_dir.join(directory);
        if source == dest {
            return Ok(());
        }

        let staging = tempfile::Builder::new()
            .prefix(".cc-switch-skill-deploy-")
            .tempdir_in(&app_dir)
            .map_err(|e| AppError::io(&app_dir, e))?;
        let next = staging.path().join("next");
        let previous = staging.path().join("previous");
        match method {
            SyncMethod::Auto => {
                if let Err(error) = Self::create_symlink(&source, &next) {
                    log::warn!(
                        "Symlink creation failed during Skill update, falling back to copy: {error}"
                    );
                    Self::copy_dir_recursive(&source, &next)?;
                }
            }
            SyncMethod::Symlink => Self::create_symlink(&source, &next)?,
            SyncMethod::Copy => Self::copy_dir_recursive(&source, &next)?,
        }

        let had_previous = fs::symlink_metadata(&dest).is_ok();
        if had_previous {
            fs::rename(&dest, &previous).map_err(|e| AppError::IoContext {
                context: format!("Failed to stage app Skill: {}", dest.display()),
                source: e,
            })?;
        }
        if let Err(error) = fs::rename(&next, &dest) {
            if had_previous {
                if let Err(rollback) = fs::rename(&previous, &dest) {
                    let preserved = staging.keep();
                    return Err(AppError::Message(format!(
                        "Skill deployment failed ({error}); rollback failed ({rollback}). Previous files remain at {}",
                        preserved.join("previous").display()
                    )));
                }
            }
            return Err(AppError::io(&dest, error));
        }
        drop(staging);
        Ok(())
    }

    pub fn remove_from_app(directory: &str, app: &AppType) -> Result<(), AppError> {
        if !Self::app_supports_skills(app) {
            return Ok(());
        }

        let app_dir = Self::get_app_skills_dir(app)?;
        let path = app_dir.join(directory);
        if path.exists() || Self::is_symlink(&path) {
            Self::remove_path(&path)?;
        }
        Ok(())
    }

    pub fn sync_to_app(index: &SkillsIndex, app: &AppType) -> Result<(), AppError> {
        if !Self::app_supports_skills(app) {
            return Ok(());
        }

        for skill in index.skills.values() {
            if skill.apps.is_enabled_for(app) {
                Self::sync_to_app_dir(&skill.directory, app, index.sync_method)?;
            }
        }
        Ok(())
    }

    /// Best-effort sync for live-flow triggers (provider switch etc).
    pub fn sync_all_enabled_best_effort() -> Result<(), AppError> {
        let mut index = Self::load_index()?;
        let _ = Self::migrate_ssot_if_pending(&mut index);
        for app in Self::supported_skill_apps() {
            if let Err(e) = Self::sync_to_app(&index, &app) {
                log::warn!("同步 Skill 到 {app:?} 失败: {e}");
            }
        }
        Ok(())
    }

    pub fn sync_all_enabled(app: Option<&AppType>) -> Result<(), AppError> {
        let mut index = Self::load_index()?;
        let _ = Self::migrate_ssot_if_pending(&mut index)?;

        match app {
            Some(app) => Self::sync_to_app(&index, app)?,
            None => {
                for app in Self::supported_skill_apps() {
                    Self::sync_to_app(&index, &app)?;
                }
            }
        }

        Ok(())
    }

    pub fn list_installed() -> Result<Vec<InstalledSkill>, AppError> {
        let mut index = Self::load_index()?;
        let _ = Self::migrate_ssot_if_pending(&mut index)?;
        let mut skills: Vec<InstalledSkill> = index.skills.values().cloned().collect();
        skills.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(skills)
    }

    // ---------------------------------------------------------------------
    // Manual update checks and updates
    // ---------------------------------------------------------------------

    /// Hash all non-hidden files in a Skill directory in stable path order.
    pub fn compute_dir_hash(dir: &Path) -> Result<String, AppError> {
        use sha2::{Digest, Sha256};

        fn collect(current: &Path, files: &mut Vec<PathBuf>) -> Result<(), AppError> {
            for entry in fs::read_dir(current).map_err(|e| AppError::io(current, e))? {
                let entry = entry.map_err(|e| AppError::io(current, e))?;
                let name = entry.file_name();
                if name.to_string_lossy().starts_with('.') {
                    continue;
                }
                let file_type = entry
                    .file_type()
                    .map_err(|e| AppError::io(entry.path(), e))?;
                if file_type.is_symlink() {
                    continue;
                }
                if file_type.is_dir() {
                    collect(&entry.path(), files)?;
                } else if file_type.is_file() {
                    files.push(entry.path());
                }
            }
            Ok(())
        }

        if !dir.is_dir() {
            return Err(AppError::Message(format!(
                "Skill directory not found: {}",
                dir.display()
            )));
        }

        let mut files = Vec::new();
        collect(dir, &mut files)?;
        files.sort();

        let mut hasher = Sha256::new();
        for path in files {
            let relative = path.strip_prefix(dir).unwrap_or(&path);
            hasher.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
            hasher.update(b"\0");
            hasher.update(fs::read(&path).map_err(|e| AppError::io(&path, e))?);
            hasher.update(b"\0");
        }
        Ok(format!("{:x}", hasher.finalize()))
    }

    fn validate_update_directory(directory: &str) -> Result<(), AppError> {
        let mut components = Path::new(directory).components();
        if matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none() {
            return Ok(());
        }
        Err(AppError::InvalidInput(format!(
            "Invalid Skill directory: {directory}"
        )))
    }

    fn source_path_from_readme(
        skill: &InstalledSkill,
        downloaded_branch: Option<&str>,
    ) -> Option<PathBuf> {
        let owner = skill.repo_owner.as_deref()?;
        let repo = skill.repo_name.as_deref()?;
        let url = skill.readme_url.as_deref()?;
        let mut branches = Vec::new();
        if let Some(branch) = skill.repo_branch.as_deref() {
            branches.push(branch);
        }
        if let Some(branch) = downloaded_branch {
            if !branches.contains(&branch) {
                branches.push(branch);
            }
        }
        for branch in ["HEAD", "main", "master"] {
            if !branches.contains(&branch) {
                branches.push(branch);
            }
        }

        for kind in ["tree", "blob"] {
            for branch in &branches {
                let prefix = format!("https://github.com/{owner}/{repo}/{kind}/{branch}/");
                let Some(raw) = url.strip_prefix(&prefix) else {
                    continue;
                };
                let mut path = PathBuf::from(raw.trim_end_matches('/'));
                if kind == "blob" && path.file_name().is_some_and(|name| name == "SKILL.md") {
                    path.pop();
                }
                if path
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
                {
                    return Some(path);
                }
            }
        }
        None
    }

    fn resolve_update_source(
        root: &Path,
        skill: &InstalledSkill,
        downloaded_branch: Option<&str>,
    ) -> Result<PathBuf, AppError> {
        if let Some(relative) = Self::source_path_from_readme(skill, downloaded_branch) {
            let exact = root.join(relative);
            if exact.is_dir() && exact.join("SKILL.md").is_file() {
                return Ok(exact);
            }
        }

        let mut matches = Self::scan_skill_dirs(root)?
            .into_iter()
            .filter(|path| {
                path.file_name().is_some_and(|name| {
                    name.to_string_lossy()
                        .eq_ignore_ascii_case(&skill.directory)
                })
            })
            .collect::<Vec<_>>();

        let root_is_skill = root.join("SKILL.md").is_file();
        if matches.is_empty() && root_is_skill {
            return Ok(root.to_path_buf());
        }
        if matches.len() == 1 && !root_is_skill {
            return Ok(matches.remove(0));
        }

        let detail = if matches.is_empty() && !root_is_skill {
            "not found"
        } else {
            "ambiguous"
        };
        Err(AppError::Message(format!(
            "Remote Skill directory '{}' is {detail} in {}/{}",
            skill.directory,
            skill.repo_owner.as_deref().unwrap_or("unknown"),
            skill.repo_name.as_deref().unwrap_or("unknown")
        )))
    }

    /// Check for repository-backed Skill updates. This is only called by
    /// explicit CLI/TUI actions; no background or startup check is scheduled.
    pub async fn check_updates(&self) -> Result<SkillUpdateCheckResult, AppError> {
        let db = Database::init()?;
        let installed = db.get_all_installed_skills()?;
        let ssot_dir = Self::get_ssot_dir()?;
        let mut groups: HashMap<(String, String, String), Vec<InstalledSkill>> = HashMap::new();

        for skill in installed.into_values() {
            let (Some(owner), Some(repo)) = (&skill.repo_owner, &skill.repo_name) else {
                continue;
            };
            groups
                .entry((
                    owner.clone(),
                    repo.clone(),
                    skill
                        .repo_branch
                        .clone()
                        .unwrap_or_else(|| "HEAD".to_string()),
                ))
                .or_default()
                .push(skill);
        }

        let mut groups = groups.into_iter().collect::<Vec<_>>();
        groups.sort_by(|left, right| left.0.cmp(&right.0));
        let mut result = SkillUpdateCheckResult::default();

        for ((owner, name, branch), skills) in groups {
            let repo = SkillRepo {
                owner: owner.clone(),
                name: name.clone(),
                branch,
                enabled: true,
            };
            let (temp_dir, used_branch) = match timeout(
                std::time::Duration::from_secs(60),
                self.download_repo_for_update(&repo),
            )
            .await
            {
                Ok(Ok((path, used_branch))) => (DownloadedRepoGuard::new(path), used_branch),
                Ok(Err(error)) => {
                    result.failures.push(format!("{owner}/{name}: {error}"));
                    continue;
                }
                Err(_) => {
                    result
                        .failures
                        .push(format!("{owner}/{name}: update check timed out"));
                    continue;
                }
            };

            for skill in skills {
                if let Err(error) = Self::validate_update_directory(&skill.directory) {
                    result.failures.push(format!("{}: {error}", skill.id));
                    continue;
                }
                let remote_dir = match Self::resolve_update_source(
                    temp_dir.path(),
                    &skill,
                    Some(&used_branch),
                ) {
                    Ok(path) => path,
                    Err(error) => {
                        result.failures.push(format!("{}: {error}", skill.id));
                        continue;
                    }
                };
                let remote_hash = match Self::compute_dir_hash(&remote_dir) {
                    Ok(hash) => hash,
                    Err(error) => {
                        result.failures.push(format!("{}: {error}", skill.id));
                        continue;
                    }
                };

                let local_dir = ssot_dir.join(&skill.directory);
                let current_hash = if local_dir.is_dir() {
                    match &skill.content_hash {
                        Some(hash) => Some(hash.clone()),
                        None => match Self::compute_dir_hash(&local_dir) {
                            Ok(hash) => {
                                if let Err(error) = db.update_skill_hash(&skill.id, &hash, 0) {
                                    log::warn!(
                                        "Failed to store Skill hash for {}: {error}",
                                        skill.id
                                    );
                                }
                                Some(hash)
                            }
                            Err(error) => {
                                result.failures.push(format!("{}: {error}", skill.id));
                                continue;
                            }
                        },
                    }
                } else {
                    None
                };

                if current_hash.as_deref() != Some(remote_hash.as_str()) {
                    result.updates.push(SkillUpdateInfo {
                        id: skill.id,
                        name: skill.name,
                        directory: skill.directory,
                        current_hash,
                        remote_hash,
                    });
                }
            }
        }

        result
            .updates
            .sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
        Ok(result)
    }

    fn restore_previous_update(dest: &Path, previous: Option<&Path>) -> Result<(), AppError> {
        if fs::symlink_metadata(dest).is_ok() {
            Self::remove_path(dest)?;
        }
        if let Some(previous) = previous {
            fs::rename(previous, dest).map_err(|e| AppError::IoContext {
                context: format!("Failed to restore Skill directory: {}", dest.display()),
                source: e,
            })?;
        }
        Ok(())
    }

    /// Update one repository-backed Skill after an explicit user action.
    async fn update_skill(&self, skill_id: &str) -> Result<SkillUpdateOutcome, AppError> {
        let db = Database::init()?;
        let skill = db
            .get_installed_skill(skill_id)?
            .ok_or_else(|| AppError::Message(format!("Skill not found: {skill_id}")))?;
        Self::validate_update_directory(&skill.directory)?;

        let (owner, name) = match (&skill.repo_owner, &skill.repo_name) {
            (Some(owner), Some(name)) => (owner.clone(), name.clone()),
            _ => {
                return Err(AppError::InvalidInput(format!(
                    "Cannot update local Skill: {skill_id}"
                )))
            }
        };
        let repo = SkillRepo {
            owner: owner.clone(),
            name: name.clone(),
            branch: skill
                .repo_branch
                .clone()
                .unwrap_or_else(|| "HEAD".to_string()),
            enabled: true,
        };

        let (temp_dir, used_branch) = timeout(
            std::time::Duration::from_secs(60),
            self.download_repo_for_update(&repo),
        )
        .await
        .map_err(|_| {
            AppError::Message(format!("Timed out downloading {owner}/{name} for update"))
        })??;
        let temp_dir = DownloadedRepoGuard::new(temp_dir);
        let source = Self::resolve_update_source(temp_dir.path(), &skill, Some(&used_branch))?;
        let source_relative = source
            .strip_prefix(temp_dir.path())
            .map_err(|_| AppError::Message("Remote Skill source escaped its repository".into()))?
            .to_path_buf();

        let ssot_dir = Self::get_ssot_dir()?;
        let sync_method = Self::get_sync_method()?;
        let staging = tempfile::Builder::new()
            .prefix(".cc-switch-skill-update-")
            .tempdir_in(&ssot_dir)
            .map_err(|e| AppError::io(&ssot_dir, e))?;
        let next = staging.path().join("next");
        let previous = staging.path().join("previous");
        Self::copy_dir_recursive(&source, &next)?;
        drop(temp_dir);

        let content_hash = Self::compute_dir_hash(&next)?;
        let (new_name, new_description) =
            Self::read_skill_name_desc(&next.join("SKILL.md"), &skill.directory);
        let current = db.get_installed_skill(skill_id)?.ok_or_else(|| {
            AppError::Message(format!("Skill was removed during update: {skill_id}"))
        })?;
        if current.directory != skill.directory
            || current.repo_owner != skill.repo_owner
            || current.repo_name != skill.repo_name
            || current.repo_branch != skill.repo_branch
            || current.readme_url != skill.readme_url
            || current.content_hash != skill.content_hash
            || current.updated_at != skill.updated_at
        {
            return Err(AppError::Message(format!(
                "Skill changed during update; run the update again: {skill_id}"
            )));
        }
        let dest = ssot_dir.join(&skill.directory);
        let had_previous = fs::symlink_metadata(&dest).is_ok();
        if had_previous {
            fs::rename(&dest, &previous).map_err(|e| AppError::IoContext {
                context: format!("Failed to stage existing Skill: {}", dest.display()),
                source: e,
            })?;
        }
        if let Err(error) = fs::rename(&next, &dest) {
            if had_previous {
                if let Err(rollback) = fs::rename(&previous, &dest) {
                    let preserved = staging.keep();
                    return Err(AppError::Message(format!(
                        "Skill update failed ({error}); rollback failed ({rollback}). Previous files remain at {}",
                        preserved.join("previous").display()
                    )));
                }
            }
            return Err(AppError::io(&dest, error));
        }

        // Preserve app enablement changes made while the repository was downloading.
        let mut updated = current;
        updated.name = new_name;
        updated.description = new_description;
        updated.repo_branch = Some(used_branch.clone());
        let doc_path = if source_relative.as_os_str().is_empty() {
            "SKILL.md".to_string()
        } else {
            format!(
                "{}/SKILL.md",
                source_relative.to_string_lossy().replace('\\', "/")
            )
        };
        updated.readme_url = Some(Self::build_skill_doc_url(
            &owner,
            &name,
            &used_branch,
            &doc_path,
        ));
        updated.content_hash = Some(content_hash);
        updated.updated_at = Utc::now().timestamp();

        if let Err(error) = db.save_skill(&updated) {
            if let Err(rollback) =
                Self::restore_previous_update(&dest, had_previous.then_some(previous.as_path()))
            {
                let preserved = staging.keep();
                return Err(AppError::Message(format!(
                    "Skill metadata update failed ({error}); rollback failed ({rollback}). Previous files remain at {}",
                    preserved.join("previous").display()
                )));
            }
            return Err(error);
        }
        drop(staging);

        let mut deployment_failures = Vec::new();
        for app in Self::supported_skill_apps() {
            if updated.apps.is_enabled_for(&app) {
                if let Err(error) =
                    Self::sync_updated_skill_to_app(&updated.directory, &app, sync_method)
                {
                    log::warn!(
                        "Updated Skill {} but failed to sync it to {app:?}: {error}",
                        updated.id
                    );
                    deployment_failures.push(format!("{app:?}: {error}"));
                }
            }
        }

        Ok(SkillUpdateOutcome {
            skill: updated,
            deployment_failures,
        })
    }

    pub async fn update_skills(&self, ids: &[String]) -> SkillUpdateBatchResult {
        let mut result = SkillUpdateBatchResult::default();
        let mut seen = HashSet::new();
        for id in ids {
            if !seen.insert(id.clone()) {
                continue;
            }
            match self.update_skill(id).await {
                Ok(outcome) => {
                    if !outcome.deployment_failures.is_empty() {
                        result.failures.push(SkillUpdateFailure {
                            id: outcome.skill.id.clone(),
                            error: format!(
                                "content updated, but app deployment failed; retry the update or run `cc-switch skills sync`: {}",
                                outcome.deployment_failures.join("; ")
                            ),
                        });
                    }
                    result.updated.push(outcome.skill);
                }
                Err(error) => result.failures.push(SkillUpdateFailure {
                    id: id.clone(),
                    error: error.to_string(),
                }),
            }
        }
        result
    }

    pub fn list_repos() -> Result<Vec<SkillRepo>, AppError> {
        Ok(Self::load_index()?.repos)
    }

    pub fn get_sync_method() -> Result<SyncMethod, AppError> {
        Ok(crate::settings::get_skill_sync_method())
    }

    pub fn set_sync_method(method: SyncMethod) -> Result<(), AppError> {
        crate::settings::set_skill_sync_method(method)
    }

    pub fn upsert_repo(repo: SkillRepo) -> Result<(), AppError> {
        let mut index = Self::load_index()?;
        if let Some(pos) = index
            .repos
            .iter()
            .position(|r| r.owner == repo.owner && r.name == repo.name)
        {
            index.repos[pos] = repo;
        } else {
            index.repos.push(repo);
        }
        Self::save_index(&index)?;
        Ok(())
    }

    pub fn remove_repo(owner: &str, name: &str) -> Result<(), AppError> {
        let db = Database::init()?;
        db.delete_skill_repo(owner, name)
    }

    fn resolve_directory_from_input(index: &SkillsIndex, input: &str) -> Option<String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return None;
        }

        // Prefer exact directory match.
        if index.skills.contains_key(trimmed) {
            return Some(trimmed.to_string());
        }

        // Case-insensitive directory match.
        let trimmed_lower = trimmed.to_lowercase();
        if let Some((dir, _)) = index
            .skills
            .iter()
            .find(|(dir, _)| dir.to_lowercase() == trimmed_lower)
        {
            return Some(dir.clone());
        }

        // Match by id.
        if let Some((dir, _)) = index
            .skills
            .iter()
            .find(|(_, s)| s.id.eq_ignore_ascii_case(trimmed))
        {
            return Some(dir.clone());
        }

        None
    }

    pub fn toggle_app(directory_or_id: &str, app: &AppType, enabled: bool) -> Result<(), AppError> {
        let mut index = Self::load_index()?;
        let Some(dir) = Self::resolve_directory_from_input(&index, directory_or_id) else {
            return Err(AppError::Message(format!(
                "未找到已安装的 Skill: {directory_or_id}"
            )));
        };

        let Some(record) = index.skills.get_mut(&dir) else {
            return Err(AppError::Message(format!("未找到已安装的 Skill: {dir}")));
        };

        if !Self::app_supports_skills(app) {
            return Ok(());
        }

        record.apps.set_enabled_for(app, enabled);

        if enabled {
            Self::sync_to_app_dir(&record.directory, app, index.sync_method)?;
        } else {
            Self::remove_from_app(&record.directory, app)?;
        }

        Self::save_index(&index)?;
        Ok(())
    }

    pub fn set_apps(directory_or_id: &str, apps: SkillApps) -> Result<bool, AppError> {
        let mut index = Self::load_index()?;
        let Some(dir) = Self::resolve_directory_from_input(&index, directory_or_id) else {
            return Err(AppError::Message(format!(
                "未找到已安装的 Skill: {directory_or_id}"
            )));
        };

        let Some(record) = index.skills.get_mut(&dir) else {
            return Err(AppError::Message(format!("未找到已安装的 Skill: {dir}")));
        };

        let before = record.apps.clone();
        record.apps = apps.clone();
        let directory = record.directory.clone();
        let sync_method = index.sync_method;
        let changes = Self::supported_skill_apps()
            .filter_map(|app| {
                let before_enabled = before.is_enabled_for(&app);
                let after_enabled = apps.is_enabled_for(&app);
                (before_enabled != after_enabled).then_some((app, after_enabled))
            })
            .collect::<Vec<_>>();

        for (app, enabled) in changes {
            if enabled {
                Self::sync_to_app_dir(&directory, &app, sync_method)?;
            } else {
                Self::remove_from_app(&directory, &app)?;
            }
        }

        Self::save_index(&index)?;
        Ok(true)
    }

    pub fn uninstall(directory_or_id: &str) -> Result<(), AppError> {
        let index = Self::load_index()?;
        let Some(dir) = Self::resolve_directory_from_input(&index, directory_or_id) else {
            return Err(AppError::Message(format!(
                "未找到已安装的 Skill: {directory_or_id}"
            )));
        };
        let record = index
            .skills
            .get(&dir)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("未找到已安装的 Skill: {dir}")))?;

        // Remove from app dirs (best effort).
        for app in [
            AppType::Claude,
            AppType::Codex,
            AppType::Gemini,
            AppType::OpenCode,
            AppType::Hermes,
        ] {
            if let Err(e) = Self::remove_from_app(&dir, &app) {
                log::warn!("从 {app:?} 删除 Skill {dir} 失败: {e}");
            }
        }

        // Remove from SSOT.
        let ssot_dir = Self::get_ssot_dir()?;
        let ssot_path = ssot_dir.join(&dir);
        if ssot_path.exists() {
            fs::remove_dir_all(&ssot_path).map_err(|e| AppError::io(&ssot_path, e))?;
        }

        let db = Database::init()?;
        let _ = db.delete_skill(&record.id)?;
        Ok(())
    }

    pub async fn install(&self, spec: &str, app: &AppType) -> Result<InstalledSkill, AppError> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err(AppError::InvalidInput("Skill 不能为空".to_string()));
        }

        let mut index = Self::load_index()?;
        let _ = Self::migrate_ssot_if_pending(&mut index)?;

        // Resolve spec to a discoverable skill.
        let discoverable = self.resolve_install_spec(&index, spec).await?;

        // Directory install name is always the last segment.
        let install_name = Path::new(&discoverable.directory)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| discoverable.directory.clone());

        // Conflict check (directory collisions across repos).
        if let Some(existing) = index.skills.get(&install_name) {
            let same_repo = existing.repo_owner.as_deref()
                == Some(discoverable.repo_owner.as_str())
                && existing.repo_name.as_deref() == Some(discoverable.repo_name.as_str());
            if !same_repo
                && (existing.repo_owner.is_some()
                    || existing.repo_name.is_some()
                    || existing.id.starts_with("local:"))
            {
                let existing_repo = format!(
                    "{}/{}",
                    existing.repo_owner.as_deref().unwrap_or("unknown"),
                    existing.repo_name.as_deref().unwrap_or("unknown")
                );
                let new_repo = format!("{}/{}", discoverable.repo_owner, discoverable.repo_name);

                return Err(AppError::Message(format_skill_error(
                    "SKILL_DIRECTORY_CONFLICT",
                    &[
                        ("directory", install_name.as_str()),
                        ("existing_repo", existing_repo.as_str()),
                        ("new_repo", new_repo.as_str()),
                    ],
                    Some("uninstallFirst"),
                )));
            }

            // Already installed: just enable current app and sync.
            let mut updated = existing.clone();
            updated.apps.set_enabled_for(app, true);
            index.skills.insert(install_name.clone(), updated.clone());
            Self::save_index(&index)?;
            Self::sync_to_app_dir(&install_name, app, index.sync_method)?;
            return Ok(updated);
        }

        // Ensure SSOT dir and install files.
        let ssot_dir = Self::get_ssot_dir()?;
        let dest = ssot_dir.join(&install_name);
        let mut installed_branch = discoverable.repo_branch.clone();
        let mut installed_readme_url = discoverable.readme_url.clone();
        if !dest.exists() {
            let repo = SkillRepo {
                owner: discoverable.repo_owner.clone(),
                name: discoverable.repo_name.clone(),
                branch: discoverable.repo_branch.clone(),
                enabled: true,
            };

            let (temp_dir, used_branch) = timeout(
                std::time::Duration::from_secs(60),
                self.download_repo(&repo),
            )
            .await
            .map_err(|_| {
                AppError::Message(format_skill_error(
                    "DOWNLOAD_TIMEOUT",
                    &[
                        ("owner", repo.owner.as_str()),
                        ("name", repo.name.as_str()),
                        ("timeout", "60"),
                    ],
                    Some("checkNetwork"),
                ))
            })??;

            let source =
                Self::find_skill_dir_in_repo(&temp_dir, &install_name)?.ok_or_else(|| {
                    let _ = fs::remove_dir_all(&temp_dir);
                    AppError::Message(format_skill_error(
                        "SKILL_DIR_NOT_FOUND",
                        &[("directory", install_name.as_str())],
                        Some("checkRepoUrl"),
                    ))
                })?;

            if !source.exists() {
                let _ = fs::remove_dir_all(&temp_dir);
                let source_path_string = source.display().to_string();
                return Err(AppError::Message(format_skill_error(
                    "SKILL_DIR_NOT_FOUND",
                    &[("path", source_path_string.as_str())],
                    Some("checkRepoUrl"),
                )));
            }

            let source_relative = source.strip_prefix(&temp_dir).map_err(|_| {
                AppError::Message("Remote Skill source escaped its repository".into())
            })?;
            let doc_path = format!(
                "{}/SKILL.md",
                source_relative.to_string_lossy().replace('\\', "/")
            );
            installed_readme_url = Some(Self::build_skill_doc_url(
                &repo.owner,
                &repo.name,
                &used_branch,
                &doc_path,
            ));
            installed_branch = used_branch;
            Self::copy_dir_recursive(&source, &dest)?;
            let _ = fs::remove_dir_all(&temp_dir);
        }

        let installed = InstalledSkill {
            id: discoverable.key.clone(),
            name: discoverable.name.clone(),
            description: if discoverable.description.trim().is_empty() {
                None
            } else {
                Some(discoverable.description.clone())
            },
            directory: install_name.clone(),
            readme_url: installed_readme_url,
            repo_owner: Some(discoverable.repo_owner.clone()),
            repo_name: Some(discoverable.repo_name.clone()),
            repo_branch: Some(installed_branch),
            apps: SkillApps::only(app),
            installed_at: Utc::now().timestamp(),
            content_hash: Self::compute_dir_hash(&dest).ok(),
            updated_at: 0,
        };

        index.skills.insert(install_name.clone(), installed.clone());
        Self::save_index(&index)?;
        Self::sync_to_app_dir(&install_name, app, index.sync_method)?;

        Ok(installed)
    }

    async fn resolve_install_spec(
        &self,
        index: &SkillsIndex,
        spec: &str,
    ) -> Result<DiscoverableSkill, AppError> {
        // If the user provides full key (owner/name:dir), match by key.
        let discoverable = self.discover_available(index.repos.clone()).await?;

        if let Some(found) = discoverable.iter().find(|s| s.key == spec) {
            return Ok(found.clone());
        }

        // Otherwise treat as directory name (may be ambiguous).
        let matches: Vec<DiscoverableSkill> = discoverable
            .into_iter()
            .filter(|s| s.directory.eq_ignore_ascii_case(spec))
            .collect();

        match matches.len() {
            0 => self.resolve_skills_sh_install_spec(spec).await,
            1 => Ok(matches[0].clone()),
            _ => Err(AppError::Message(format!(
                "Skill 名称不唯一，请使用完整 key（owner/name:directory）: {spec}"
            ))),
        }
    }

    async fn resolve_skills_sh_install_spec(
        &self,
        spec: &str,
    ) -> Result<DiscoverableSkill, AppError> {
        if let Some(discoverable) = discoverable_from_repo_spec(spec) {
            return Ok(discoverable);
        }

        let result = self.search_skills_sh(spec, 20, 0).await?;

        if let Some(found) = result
            .skills
            .iter()
            .find(|s| s.key == spec || s.directory.eq_ignore_ascii_case(spec))
        {
            return Ok(found.clone().into());
        }

        let matches: Vec<SkillsShDiscoverableSkill> = result
            .skills
            .into_iter()
            .filter(|s| s.name.eq_ignore_ascii_case(spec))
            .collect();

        match matches.len() {
            0 => Err(AppError::Message(format!("未找到可安装的 Skill: {spec}"))),
            1 => Ok(matches[0].clone().into()),
            _ => Err(AppError::Message(format!(
                "Skill 名称不唯一，请使用完整 key: {spec}"
            ))),
        }
    }

    // ---------------------------------------------------------------------
    // Unmanaged scan / import
    // ---------------------------------------------------------------------

    pub fn scan_unmanaged() -> Result<Vec<UnmanagedSkill>, AppError> {
        let index = Self::load_index()?;
        let managed: HashSet<String> = index.skills.keys().cloned().collect();

        let mut scan_sources: Vec<(PathBuf, String)> = Vec::new();
        for app in Self::skill_source_apps() {
            if let Ok(app_dir) = Self::get_app_skills_dir(&app) {
                scan_sources.push((app_dir, app.as_str().to_string()));
            }
        }
        if let Some(agents_dir) = get_agents_skills_dir() {
            scan_sources.push((agents_dir, "agents".to_string()));
        }
        if let Ok(ssot_dir) = Self::get_ssot_dir() {
            scan_sources.push((ssot_dir, "cc-switch".to_string()));
        }

        let mut unmanaged: HashMap<String, UnmanagedSkill> = HashMap::new();

        for (scan_dir, label) in &scan_sources {
            let entries = match fs::read_dir(scan_dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };

            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => continue,
                };
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let dir_name = entry.file_name().to_string_lossy().to_string();
                if dir_name.starts_with('.') || managed.contains(&dir_name) {
                    continue;
                }

                let skill_md = path.join("SKILL.md");
                if !skill_md.exists() {
                    continue;
                }
                let (name, description) = Self::read_skill_name_desc(&skill_md, &dir_name);
                let path_display = path.display().to_string();

                unmanaged
                    .entry(dir_name.clone())
                    .and_modify(|skill| {
                        if !skill.found_in.contains(label) {
                            skill.found_in.push(label.clone());
                        }
                    })
                    .or_insert(UnmanagedSkill {
                        directory: dir_name,
                        name,
                        description,
                        found_in: vec![label.clone()],
                        path: path_display,
                    });
            }
        }

        Ok(unmanaged.into_values().collect())
    }

    pub fn import_from_app_dirs(directories: Vec<String>) -> Result<Vec<InstalledSkill>, AppError> {
        let scan = Self::scan_unmanaged()?;
        let imports = directories
            .into_iter()
            .map(|directory| {
                let apps = scan
                    .iter()
                    .find(|skill| skill.directory == directory)
                    .map(|skill| SkillApps::from_labels(&skill.found_in))
                    .unwrap_or_default();
                ImportSkillSelection { directory, apps }
            })
            .collect();

        Self::import_from_apps(imports)
    }

    pub fn import_from_apps(
        imports: Vec<ImportSkillSelection>,
    ) -> Result<Vec<InstalledSkill>, AppError> {
        let mut index = Self::load_index()?;
        let ssot_dir = Self::get_ssot_dir()?;
        let agents_lock = parse_agents_lock();
        let mut imported = Vec::new();

        merge_repos_from_lock(
            &mut index.repos,
            &agents_lock,
            imports.iter().map(|selection| selection.directory.as_str()),
        );

        let mut search_sources: Vec<(PathBuf, String)> = Vec::new();
        for app in Self::skill_source_apps() {
            if let Ok(app_dir) = Self::get_app_skills_dir(&app) {
                search_sources.push((app_dir, app.as_str().to_string()));
            }
        }
        if let Some(agents_dir) = get_agents_skills_dir() {
            search_sources.push((agents_dir, "agents".to_string()));
        }
        search_sources.push((ssot_dir.clone(), "cc-switch".to_string()));

        for selection in imports {
            let dir_name = selection.directory;
            let mut source_path: Option<PathBuf> = None;

            for (base, label) in &search_sources {
                let skill_path = base.join(&dir_name);
                if skill_path.exists() {
                    if source_path.is_none() {
                        source_path = Some(skill_path);
                    }
                    log::debug!("Skill '{dir_name}' found in source '{label}'");
                }
            }

            let Some(source) = source_path else { continue };
            if !source.join("SKILL.md").exists() {
                continue;
            }

            let dest = ssot_dir.join(&dir_name);
            if !dest.exists() {
                Self::copy_dir_recursive(&source, &dest)?;
            }

            let skill_md = dest.join("SKILL.md");
            let (name, description) = Self::read_skill_name_desc(&skill_md, &dir_name);
            let apps = selection.apps;
            let (id, repo_owner, repo_name, repo_branch, readme_url) =
                build_repo_info_from_lock(&agents_lock, &dir_name);

            let skill = InstalledSkill {
                id,
                name,
                description,
                directory: dir_name.clone(),
                repo_owner,
                repo_name,
                repo_branch,
                readme_url,
                apps,
                installed_at: Utc::now().timestamp(),
                content_hash: Self::compute_dir_hash(&dest).ok(),
                updated_at: 0,
            };

            index.skills.insert(dir_name.clone(), skill.clone());
            imported.push(skill);
        }

        Self::save_index(&index)?;
        Ok(imported)
    }

    // ---------------------------------------------------------------------
    // Repo discovery / list
    // ---------------------------------------------------------------------

    pub async fn discover_available(
        &self,
        repos: Vec<SkillRepo>,
    ) -> Result<Vec<DiscoverableSkill>, AppError> {
        let enabled_repos: Vec<SkillRepo> = repos.into_iter().filter(|r| r.enabled).collect();
        let tasks = enabled_repos
            .iter()
            .map(|repo| self.fetch_repo_skills(repo));
        let results: Vec<Result<Vec<DiscoverableSkill>, AppError>> = join_all(tasks).await;

        let mut skills = Vec::new();
        for (repo, result) in enabled_repos.into_iter().zip(results.into_iter()) {
            match result {
                Ok(repo_skills) => skills.extend(repo_skills),
                Err(e) => log::warn!("获取仓库 {}/{} 技能失败: {}", repo.owner, repo.name, e),
            }
        }

        Self::deduplicate_discoverable(&mut skills);
        skills.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(skills)
    }

    pub async fn search_skills_sh(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<SkillsShSearchResult, AppError> {
        let limit = limit.clamp(1, 100);
        let url = url::Url::parse_with_params(
            "https://skills.sh/api/search",
            &[
                ("q", query),
                ("limit", &limit.to_string()),
                ("offset", &offset.to_string()),
            ],
        )
        .map_err(|e| AppError::Message(format!("Invalid skills.sh search URL: {e}")))?;

        let response = self
            .http_client
            .get(url)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| AppError::Message(format!("skills.sh search request failed: {e}")))?
            .error_for_status()
            .map_err(|e| AppError::Message(format!("skills.sh search failed: {e}")))?
            .json::<SkillsShApiResponse>()
            .await
            .map_err(|e| AppError::Message(format!("Failed to parse skills.sh response: {e}")))?;

        let skills = response
            .skills
            .into_iter()
            .filter_map(|skill| skills_sh_api_skill_to_discoverable(skill))
            .collect();

        Ok(SkillsShSearchResult {
            skills,
            total_count: response.count,
            query: response.query,
        })
    }

    pub async fn list_skills(&self) -> Result<Vec<Skill>, AppError> {
        let mut index = Self::load_index()?;
        let _ = Self::migrate_ssot_if_pending(&mut index)?;
        self.list_skills_for_index(&index).await
    }

    pub async fn list_skills_cached(&self, force_refresh: bool) -> Result<Vec<Skill>, AppError> {
        let mut index = Self::load_index()?;
        let _ = Self::migrate_ssot_if_pending(&mut index)?;
        let fingerprint = Self::repos_fingerprint(&index.repos);

        if !force_refresh {
            if let Some(skills) = Self::load_discover_cache(&fingerprint)? {
                return Ok(Self::apply_installed_state(skills, &index));
            }
        }

        let skills = self.list_skills_for_index(&index).await?;
        Self::save_discover_cache(&fingerprint, &skills)?;
        Ok(skills)
    }

    async fn list_skills_for_index(&self, index: &SkillsIndex) -> Result<Vec<Skill>, AppError> {
        let discoverable = self.discover_available(index.repos.clone()).await?;
        let installed_dirs: HashSet<String> =
            index.skills.keys().map(|s| s.to_lowercase()).collect();

        let mut out: Vec<Skill> = discoverable
            .into_iter()
            .map(|d| {
                let installed = installed_dirs.contains(&d.directory.to_lowercase());
                Skill {
                    key: d.key,
                    name: d.name,
                    description: d.description,
                    directory: d.directory,
                    readme_url: d.readme_url,
                    installed,
                    repo_owner: Some(d.repo_owner),
                    repo_name: Some(d.repo_name),
                    repo_branch: Some(d.repo_branch),
                }
            })
            .collect();

        // Add local SSOT-only skills not in repos.
        Self::merge_local_ssot_skills(&index, &mut out)?;

        // De-dup + sort.
        Self::deduplicate_skills(&mut out);
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(out)
    }

    fn discover_cache_path() -> PathBuf {
        get_app_config_dir()
            .join("cache")
            .join("skills-discover.json")
    }

    fn repos_fingerprint(repos: &[SkillRepo]) -> String {
        let mut enabled = repos
            .iter()
            .filter(|repo| repo.enabled)
            .map(|repo| format!("{}/{}@{}", repo.owner, repo.name, repo.branch))
            .collect::<Vec<_>>();
        enabled.sort();
        enabled.join("|")
    }

    fn load_discover_cache(fingerprint: &str) -> Result<Option<Vec<Skill>>, AppError> {
        let path = Self::discover_cache_path();
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path)
            .map_err(|e| AppError::Message(format!("Failed to read skills discover cache: {e}")))?;
        let cache: SkillsDiscoverCache = serde_json::from_str(&content).map_err(|e| {
            AppError::Message(format!("Failed to parse skills discover cache: {e}"))
        })?;
        if cache.version == SKILLS_INDEX_VERSION && cache.repos_fingerprint == fingerprint {
            Ok(Some(cache.skills))
        } else {
            Ok(None)
        }
    }

    fn apply_installed_state(mut skills: Vec<Skill>, index: &SkillsIndex) -> Vec<Skill> {
        let installed_keys = index
            .skills
            .values()
            .map(|skill| {
                (
                    skill.directory.to_lowercase(),
                    skill
                        .repo_owner
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase(),
                    skill
                        .repo_name
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase(),
                )
            })
            .collect::<HashSet<_>>();
        let installed_dirs = index
            .skills
            .keys()
            .map(|directory| directory.to_lowercase())
            .collect::<HashSet<_>>();

        for skill in &mut skills {
            let repo_key = (
                skill.directory.to_lowercase(),
                skill
                    .repo_owner
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase(),
                skill
                    .repo_name
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase(),
            );
            skill.installed = installed_keys.contains(&repo_key)
                || installed_dirs.contains(&skill.directory.to_lowercase());
        }
        skills
    }

    fn save_discover_cache(fingerprint: &str, skills: &[Skill]) -> Result<(), AppError> {
        let path = Self::discover_cache_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                AppError::Message(format!("Failed to create skills cache dir: {e}"))
            })?;
        }
        let cache = SkillsDiscoverCache {
            version: SKILLS_INDEX_VERSION,
            repos_fingerprint: fingerprint.to_string(),
            skills: skills.to_vec(),
        };
        let content = serde_json::to_string_pretty(&cache).map_err(|e| {
            AppError::Message(format!("Failed to encode skills discover cache: {e}"))
        })?;
        fs::write(path, content)
            .map_err(|e| AppError::Message(format!("Failed to write skills discover cache: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_repo_archive_at(
        path: &str,
        content: &[u8],
    ) -> zip::ZipArchive<std::io::Cursor<Vec<u8>>> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default();
        writer
            .add_directory("repo-main/", options)
            .expect("add archive root");
        writer.start_file(path, options).expect("add archive file");
        std::io::Write::write_all(&mut writer, content).expect("write archive content");
        let mut cursor = writer.finish().expect("finish archive");
        cursor.set_position(0);
        zip::ZipArchive::new(cursor).expect("open archive")
    }

    fn test_repo_archive(content: &[u8]) -> zip::ZipArchive<std::io::Cursor<Vec<u8>>> {
        test_repo_archive_at("repo-main/SKILL.md", content)
    }

    fn repository_skill(readme_url: Option<&str>) -> InstalledSkill {
        InstalledSkill {
            id: "owner/repo:shared".to_string(),
            name: "Shared".to_string(),
            description: None,
            directory: "shared".to_string(),
            repo_owner: Some("owner".to_string()),
            repo_name: Some("repo".to_string()),
            repo_branch: Some("main".to_string()),
            readme_url: readme_url.map(str::to_string),
            apps: SkillApps::default(),
            installed_at: 0,
            content_hash: None,
            updated_at: 0,
        }
    }

    #[test]
    fn skill_directory_hash_is_stable_and_ignores_hidden_files() {
        let temp = tempfile::tempdir().expect("create hash fixture");
        fs::create_dir_all(temp.path().join("nested")).expect("create nested directory");
        fs::write(temp.path().join("SKILL.md"), "first").expect("write manifest");
        fs::write(temp.path().join("nested/data.txt"), "data").expect("write nested file");
        let initial = SkillService::compute_dir_hash(temp.path()).expect("compute initial hash");

        fs::write(temp.path().join(".cache"), "ignored").expect("write hidden file");
        assert_eq!(
            SkillService::compute_dir_hash(temp.path()).expect("hash with hidden file"),
            initial
        );

        fs::write(temp.path().join("nested/data.txt"), "changed").expect("change visible file");
        assert_ne!(
            SkillService::compute_dir_hash(temp.path()).expect("hash changed content"),
            initial
        );
    }

    #[test]
    fn repository_archive_enforces_entry_and_extracted_size_limits() {
        let entry_dest = tempfile::tempdir().expect("create entry-limit destination");
        let entry_error = SkillService::extract_repo_archive_with_limits(
            test_repo_archive(b"data"),
            entry_dest.path(),
            1,
            100,
        )
        .expect_err("archive should exceed the one-entry limit");
        assert!(entry_error.to_string().contains("too many entries"));

        let size_dest = tempfile::tempdir().expect("create size-limit destination");
        let size_error = SkillService::extract_repo_archive_with_limits(
            test_repo_archive(b"data"),
            size_dest.path(),
            10,
            3,
        )
        .expect_err("archive should exceed the extracted-byte limit");
        assert!(size_error.to_string().contains("extraction limit"));
    }

    #[test]
    fn repository_archive_rejects_paths_that_escape_after_root_stripping() {
        let parent = tempfile::tempdir().expect("create traversal destination parent");
        let dest = parent.path().join("extract");
        fs::create_dir(&dest).expect("create traversal destination");
        let error = SkillService::extract_repo_archive_with_limits(
            test_repo_archive_at("repo-main/../escaped.txt", b"escape"),
            &dest,
            10,
            1024 * 1024,
        )
        .expect_err("root-relative parent traversal must be rejected");

        assert!(error.to_string().contains("INVALID_ARCHIVE_PATH"));
        assert!(!parent.path().join("escaped.txt").exists());
    }

    #[test]
    fn repository_archive_charges_implicit_directories_to_the_budget() {
        let dest = tempfile::tempdir().expect("create directory-budget destination");
        let error = SkillService::extract_repo_archive_with_limits(
            test_repo_archive_at("repo-main/a/b/c/SKILL.md", b""),
            dest.path(),
            10,
            SKILL_ARCHIVE_ENTRY_COST - 1,
        )
        .expect_err("implicit directories must consume extraction budget");

        assert!(error.to_string().contains("extraction limit"));
    }

    #[test]
    fn github_archive_coordinates_cannot_change_the_download_endpoint() {
        assert!(SkillService::github_archive_url("owner", "repo", "feature/nested").is_ok());
        for invalid in [
            ("owner/escape", "repo", "main"),
            ("owner", "../releases", "main"),
            ("owner", "repo", "../../../releases/download/payload"),
        ] {
            assert!(SkillService::github_archive_url(invalid.0, invalid.1, invalid.2).is_err());
        }
    }

    #[test]
    fn manual_update_keeps_an_explicit_repository_branch_pinned() {
        assert_eq!(
            SkillService::branch_candidates("release", None, false),
            vec!["release"]
        );
        assert_eq!(
            SkillService::branch_candidates("release", None, true),
            vec!["release", "main", "master"]
        );
        assert_eq!(
            SkillService::branch_candidates("HEAD", Some("trunk".to_string()), false),
            vec!["trunk", "main", "master"]
        );
    }

    #[test]
    fn update_deployment_keeps_existing_app_copy_until_replacement_is_ready() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let app_dir = SkillService::get_app_skills_dir(&AppType::Codex)
            .expect("resolve isolated Codex skills directory");
        let deployed = app_dir.join("demo");
        fs::create_dir_all(&deployed).expect("create existing app deployment");
        fs::write(deployed.join("SKILL.md"), "old").expect("write existing app deployment");

        SkillService::sync_updated_skill_to_app("demo", &AppType::Codex, SyncMethod::Copy)
            .expect_err("missing staged source should fail");
        assert_eq!(
            fs::read_to_string(deployed.join("SKILL.md")).expect("read preserved deployment"),
            "old"
        );

        let source = SkillService::get_ssot_dir()
            .expect("resolve isolated SSOT")
            .join("demo");
        fs::create_dir_all(&source).expect("create replacement source");
        fs::write(source.join("SKILL.md"), "new").expect("write replacement source");
        SkillService::sync_updated_skill_to_app("demo", &AppType::Codex, SyncMethod::Copy)
            .expect("deploy replacement");
        assert_eq!(
            fs::read_to_string(deployed.join("SKILL.md")).expect("read replaced deployment"),
            "new"
        );
    }

    #[test]
    fn update_source_uses_nested_path_from_readme_url() {
        let temp = tempfile::tempdir().expect("create repository fixture");
        let first = temp.path().join("first/shared");
        let expected = temp.path().join("second/shared");
        fs::create_dir_all(&first).expect("create first duplicate");
        fs::create_dir_all(&expected).expect("create expected duplicate");
        fs::write(first.join("SKILL.md"), "first").expect("write first manifest");
        fs::write(expected.join("SKILL.md"), "second").expect("write expected manifest");

        let skill = repository_skill(Some(
            "https://github.com/owner/repo/tree/main/second/shared",
        ));
        let resolved = SkillService::resolve_update_source(temp.path(), &skill, Some("main"))
            .expect("readme path should disambiguate duplicate names");

        assert_eq!(resolved, expected);
    }

    #[test]
    fn update_source_rejects_ambiguous_name_without_source_path() {
        let temp = tempfile::tempdir().expect("create repository fixture");
        for parent in ["first", "second"] {
            let directory = temp.path().join(parent).join("shared");
            fs::create_dir_all(&directory).expect("create duplicate skill");
            fs::write(directory.join("SKILL.md"), parent).expect("write manifest");
        }

        let error =
            SkillService::resolve_update_source(temp.path(), &repository_skill(None), Some("main"))
                .expect_err("ambiguous source must not update an arbitrary directory");
        assert!(error.to_string().contains("ambiguous"), "{error}");
    }

    #[test]
    fn update_source_accepts_head_metadata_for_branchless_nested_skill() {
        let temp = tempfile::tempdir().expect("create repository fixture");
        let expected = temp.path().join("second/shared");
        for parent in ["first", "second"] {
            let directory = temp.path().join(parent).join("shared");
            fs::create_dir_all(&directory).expect("create duplicate skill");
            fs::write(directory.join("SKILL.md"), parent).expect("write manifest");
        }

        let mut skill = repository_skill(Some(
            "https://github.com/owner/repo/blob/HEAD/second/shared/SKILL.md",
        ));
        skill.repo_branch = None;
        let resolved = SkillService::resolve_update_source(temp.path(), &skill, Some("main"))
            .expect("HEAD metadata should preserve the nested source path");

        assert_eq!(resolved, expected);
    }

    #[test]
    fn update_source_prioritizes_exact_root_metadata() {
        let temp = tempfile::tempdir().expect("create repository fixture");
        fs::write(temp.path().join("SKILL.md"), "root").expect("write root manifest");
        let nested = temp.path().join("nested/shared");
        fs::create_dir_all(&nested).expect("create nested skill");
        fs::write(nested.join("SKILL.md"), "nested").expect("write nested manifest");

        let skill = repository_skill(Some("https://github.com/owner/repo/blob/main/SKILL.md"));
        let resolved = SkillService::resolve_update_source(temp.path(), &skill, Some("main"))
            .expect("exact root metadata should win over a same-named nested skill");

        assert_eq!(resolved, temp.path());
    }

    #[test]
    fn update_directory_must_be_one_safe_path_segment() {
        assert!(SkillService::validate_update_directory("safe-skill").is_ok());
        assert!(SkillService::validate_update_directory("../outside").is_err());
        assert!(SkillService::validate_update_directory("nested/skill").is_err());
        assert!(SkillService::validate_update_directory("").is_err());
    }

    #[test]
    fn skills_sh_api_skill_maps_github_source() {
        let skill = skills_sh_api_skill_to_discoverable(SkillsShApiSkill {
            id: "skill-key".to_string(),
            skill_id: "hello-skill".to_string(),
            name: "Hello Skill".to_string(),
            installs: 42,
            source: "owner/repo".to_string(),
        })
        .expect("github source should map");

        assert_eq!(skill.key, "owner/repo:hello-skill");
        assert_eq!(skill.directory, "hello-skill");
        assert_eq!(skill.repo_owner, "owner");
        assert_eq!(skill.repo_name, "repo");
        assert_eq!(skill.repo_branch, "main");
        assert_eq!(skill.installs, 42);
        assert_eq!(
            skill.readme_url.as_deref(),
            Some("https://github.com/owner/repo")
        );
    }

    #[test]
    fn skills_sh_api_skill_filters_non_github_source() {
        let skill = skills_sh_api_skill_to_discoverable(SkillsShApiSkill {
            id: "skill-key".to_string(),
            skill_id: "hello-skill".to_string(),
            name: "Hello Skill".to_string(),
            installs: 42,
            source: "skills.example.com/repo".to_string(),
        });

        assert!(skill.is_none());
    }

    #[test]
    fn discoverable_from_repo_spec_builds_installable_skill() {
        let skill =
            discoverable_from_repo_spec("owner/repo:hello-skill").expect("repo spec should map");

        assert_eq!(skill.key, "owner/repo:hello-skill");
        assert_eq!(skill.directory, "hello-skill");
        assert_eq!(skill.repo_owner, "owner");
        assert_eq!(skill.repo_name, "repo");
        assert_eq!(skill.repo_branch, "main");
        assert_eq!(
            skill.readme_url.as_deref(),
            Some("https://github.com/owner/repo")
        );
    }

    #[test]
    fn repos_fingerprint_is_order_stable_for_enabled_repos() {
        let repos = vec![
            SkillRepo {
                owner: "b".to_string(),
                name: "repo".to_string(),
                branch: "main".to_string(),
                enabled: true,
            },
            SkillRepo {
                owner: "a".to_string(),
                name: "repo".to_string(),
                branch: "dev".to_string(),
                enabled: true,
            },
            SkillRepo {
                owner: "ignored".to_string(),
                name: "repo".to_string(),
                branch: "main".to_string(),
                enabled: false,
            },
        ];

        assert_eq!(
            SkillService::repos_fingerprint(&repos),
            "a/repo@dev|b/repo@main"
        );
    }
}
