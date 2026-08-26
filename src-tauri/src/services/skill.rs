//! Skills service layer
//!
//! v3.10.0+ 统一管理架构（与上游一致）：
//! - SSOT（单一事实源）：`~/.cc-switch/skills/`
//! - 数据库存储安装记录、启用状态与仓库列表（`~/.cc-switch/cc-switch.db`）

mod discovery;

use chrono::{DateTime, Utc};
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::timeout;

use crate::app_config::AppType;
pub use crate::app_config::{InstalledSkill, SkillApps, UnmanagedSkill};
use crate::config::{create_managed_config_dir_all, get_app_config_dir, write_json_file};
use crate::database::Database;
use crate::error::{format_skill_error, AppError};

const SKILLS_INDEX_VERSION: u32 = 1;
const SKILLS_DISCOVER_CACHE_VERSION: u32 = 2;
const MAX_SKILL_ARCHIVE_ENTRIES: usize = 10_000;
const MAX_SKILL_ARCHIVE_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SKILL_ARCHIVE_DOWNLOAD_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SKILL_ARCHIVE_PATH_DEPTH: usize = 64;
const SKILL_ARCHIVE_ENTRY_COST: u64 = 4096;
const STORAGE_MIGRATION_JOURNAL_FILE: &str = "skill-storage-migration.json";

// Coordinates the Skills database rows with the filesystem SSOT and app projections.
// Lock order: process-wide sync coordination (when present) -> this lock -> database mutex.
fn skill_state_lock() -> &'static RwLock<()> {
    static LOCK: OnceLock<RwLock<()>> = OnceLock::new();
    LOCK.get_or_init(|| RwLock::new(()))
}

pub(crate) fn skill_state_read_guard() -> RwLockReadGuard<'static, ()> {
    skill_state_lock().read().unwrap_or_else(|poisoned| {
        log::warn!("Skills state read lock was poisoned; recovering protected state");
        poisoned.into_inner()
    })
}

pub(crate) fn skill_state_write_guard() -> RwLockWriteGuard<'static, ()> {
    skill_state_lock().write().unwrap_or_else(|poisoned| {
        log::warn!("Skills state write lock was poisoned; recovering protected state");
        poisoned.into_inner()
    })
}

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
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

/// Location of the managed Skills single source of truth (SSOT).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
#[serde(rename_all = "snake_case")]
pub enum SkillStorageLocation {
    /// CC Switch managed directory (`~/.cc-switch/skills/`).
    #[default]
    #[cfg_attr(feature = "cli", value(alias = "cc_switch"))]
    CcSwitch,
    /// Shared Agent Skills directory (`~/.agents/skills/`).
    Unified,
}

/// Result of moving managed Skills between SSOT locations.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MigrationResult {
    pub migrated_count: usize,
    pub skipped_count: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StorageMigrationJournal {
    source: SkillStorageLocation,
    target: SkillStorageLocation,
    token: String,
    hashes: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MigrationDeploymentAction {
    Refresh,
    AlreadyCurrent,
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
    /// Repository-relative source directory, or the repository name for a root Skill.
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

pub struct SkillService;

#[derive(Debug, Clone)]
enum PiSkillDeployment {
    Symlink { expected_target: PathBuf },
    Copy { expected_hash: String },
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
            AppType::Pi,
        ]
        .into_iter()
    }

    fn skill_source_apps() -> impl Iterator<Item = AppType> {
        AppType::all()
    }

    pub fn new() -> Result<Self, AppError> {
        Ok(Self)
    }

    // ---------------------------------------------------------------------
    // Paths
    // ---------------------------------------------------------------------

    fn ssot_dir_for(location: SkillStorageLocation) -> Result<PathBuf, AppError> {
        match location {
            SkillStorageLocation::CcSwitch => Ok(get_app_config_dir().join("skills")),
            SkillStorageLocation::Unified => crate::config::home_dir()
                .map(|home| home.join(".agents").join("skills"))
                .ok_or_else(|| {
                    AppError::Message(format_skill_error(
                        "GET_HOME_DIR_FAILED",
                        &[],
                        Some("checkPermission"),
                    ))
                }),
        }
    }

    fn validate_storage_root(location: SkillStorageLocation, root: &Path) -> Result<(), AppError> {
        if location != SkillStorageLocation::Unified {
            return Ok(());
        }

        let agents_dir = root.parent().ok_or_else(|| {
            AppError::InvalidInput(format!(
                "Invalid Unified Skill storage path: {}",
                root.display()
            ))
        })?;
        for path in [agents_dir, root] {
            match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(AppError::InvalidInput(format!(
                        "Unified Skill storage cannot use a symbolic link: {}",
                        path.display()
                    )));
                }
                Ok(metadata) if !metadata.is_dir() => {
                    return Err(AppError::InvalidInput(format!(
                        "Unified Skill storage requires a directory: {}",
                        path.display()
                    )));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(AppError::io(path, error)),
            }
        }
        Ok(())
    }

    pub fn get_ssot_dir() -> Result<PathBuf, AppError> {
        let location = crate::settings::get_skill_storage_location();
        let dir = Self::ssot_dir_for(location)?;
        Self::validate_storage_root(location, &dir)?;
        create_managed_config_dir_all(&dir)?;
        Self::validate_storage_root(location, &dir)?;
        Ok(dir)
    }

    pub fn get_app_skills_dir(app: &AppType) -> Result<PathBuf, AppError> {
        // Reuse each app's authoritative config-dir resolver so settings and
        // environment overrides (notably CLAUDE_CONFIG_DIR/CODEX_HOME) agree.
        Ok(match app {
            AppType::Claude => crate::config::get_claude_config_dir().join("skills"),
            AppType::Codex => crate::codex_config::get_codex_config_dir().join("skills"),
            AppType::Gemini => crate::gemini_config::get_gemini_dir().join("skills"),
            AppType::OpenCode => crate::opencode_config::get_opencode_dir().join("skills"),
            AppType::Hermes => crate::hermes_config::get_hermes_dir().join("skills"),
            AppType::OpenClaw => crate::openclaw_config::get_openclaw_dir().join("skills"),
            AppType::Pi => crate::pi_config::get_pi_agent_dir()?.join("skills"),
        })
    }

    fn comparable_path(path: &Path) -> PathBuf {
        fn normalize(path: &Path) -> PathBuf {
            let mut normalized = PathBuf::new();
            for component in path.components() {
                match component {
                    Component::CurDir => {}
                    Component::ParentDir => {
                        let _ = normalized.pop();
                    }
                    Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                        normalized.push(component.as_os_str());
                    }
                }
            }
            normalized
        }

        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        };

        // Resolve the longest existing prefix before applying a remaining
        // suffix. In particular, `link/..` must follow `link` first instead of
        // being collapsed lexically, matching filesystem path resolution.
        for ancestor in path.ancestors() {
            if let Ok(mut resolved) = ancestor.canonicalize() {
                let suffix = path
                    .strip_prefix(ancestor)
                    .unwrap_or_else(|_| Path::new(""));
                for component in suffix.components() {
                    match component {
                        Component::CurDir => {}
                        Component::ParentDir => {
                            let _ = resolved.pop();
                        }
                        Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                            resolved.push(component.as_os_str());
                        }
                    }
                }
                return normalize(&resolved);
            }
        }

        normalize(&path)
    }

    fn paths_equal(left: &Path, right: &Path) -> bool {
        Self::comparable_path(left) == Self::comparable_path(right)
    }

    fn paths_overlap(left: &Path, right: &Path) -> bool {
        let left = Self::comparable_path(left);
        let right = Self::comparable_path(right);
        left.starts_with(&right) || right.starts_with(&left)
    }

    fn get_distinct_app_skills_dir(ssot_dir: &Path, app: &AppType) -> Result<PathBuf, AppError> {
        let app_dir = Self::get_app_skills_dir(app)?;
        if Self::paths_overlap(ssot_dir, &app_dir) {
            return Err(AppError::InvalidInput(format!(
                "Skill storage directory cannot overlap the {} Skills directory: {} and {}",
                app.as_str(),
                ssot_dir.display(),
                app_dir.display()
            )));
        }
        Ok(app_dir)
    }

    fn validate_skill_storage_destination(ssot_dir: &Path) -> Result<(), AppError> {
        for app in Self::supported_skill_apps() {
            Self::get_distinct_app_skills_dir(ssot_dir, &app)?;
        }
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Storage (SQLite + settings.json)
    // ---------------------------------------------------------------------

    pub fn load_index() -> Result<SkillsIndex, AppError> {
        // Default repository initialization is a write, so keep it and the
        // following multi-query snapshot in one exclusive state window.
        let _state_guard = skill_state_write_guard();
        let db = Database::init()?;
        let _ = db.init_default_skill_repos();
        Self::load_index_from_db(&db)
    }

    /// Caller must hold either the Skills state read or write guard.
    pub(crate) fn load_index_unlocked() -> Result<SkillsIndex, AppError> {
        let db = Database::init()?;
        Self::load_index_from_db(&db)
    }

    pub(crate) fn load_index_after_migration() -> Result<SkillsIndex, AppError> {
        let mut index = Self::load_index()?;
        Self::migrate_ssot_if_pending(&mut index)?;
        let _state_guard = skill_state_read_guard();
        Self::load_index_unlocked()
    }

    pub(crate) fn load_index_from_database(db: &Database) -> Result<SkillsIndex, AppError> {
        let _state_guard = skill_state_read_guard();
        Self::load_index_from_db(db)
    }

    fn load_index_from_db(db: &Database) -> Result<SkillsIndex, AppError> {
        let repos = db.get_skill_repos()?;
        let mut installed = db.get_all_installed_skills()?;
        for skill in installed.values_mut() {
            skill.apps.pi = Self::skill_exists_in_app(&skill.directory, &AppType::Pi);
        }
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
        let _state_guard = skill_state_write_guard();
        Self::save_index_unlocked(index)
    }

    /// Caller must hold the Skills state write guard.
    fn save_index_unlocked(index: &SkillsIndex) -> Result<(), AppError> {
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
        let _state_guard = skill_state_write_guard();
        *index = Self::load_index_unlocked()?;
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
            for record in index.skills.values_mut() {
                let directory = match Self::require_valid_directory(&record.directory) {
                    Ok(directory) => directory,
                    Err(error) => {
                        log::warn!(
                            "SSOT 迁移: 跳过非法 Skill directory {:?}: {error}",
                            record.directory
                        );
                        continue;
                    }
                };
                let dest = ssot_dir.join(&directory);
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
                    let skill_path = app_dir.join(&directory);
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
            Self::save_index_unlocked(index)?;
            return Ok(created);
        }

        let mut discovered: HashMap<String, SkillApps> = HashMap::new();

        // Pi support did not exist before the SSOT migration. Never claim an
        // independently installed Pi skill as a legacy CC Switch deployment.
        for app in Self::supported_skill_apps().filter(|app| !matches!(app, AppType::Pi)) {
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
                if Self::require_valid_directory(&dir_name).is_err() {
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
        Self::save_index_unlocked(index)?;
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

    fn skill_exists_in_app(directory: &str, app: &AppType) -> bool {
        let Ok(directory) = Self::require_valid_directory(directory) else {
            return false;
        };
        let Ok(app_dir) = Self::get_app_skills_dir(app) else {
            return false;
        };
        app_dir.join(directory).is_dir()
    }

    fn compute_pi_deployment_hash(dir: &Path) -> Result<String, AppError> {
        use sha2::{Digest, Sha256};

        fn collect(current: &Path, entries: &mut Vec<PathBuf>) -> Result<(), AppError> {
            for entry in fs::read_dir(current).map_err(|error| AppError::io(current, error))? {
                let entry = entry.map_err(|error| AppError::io(current, error))?;
                let path = entry.path();
                let file_type = entry
                    .file_type()
                    .map_err(|error| AppError::io(&path, error))?;
                entries.push(path.clone());
                if file_type.is_dir() {
                    collect(&path, entries)?;
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
        let mut entries = Vec::new();
        collect(dir, &mut entries)?;
        entries.sort();

        let mut hasher = Sha256::new();
        for path in entries {
            let relative = path.strip_prefix(dir).unwrap_or(&path);
            hasher.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
            hasher.update(b"\0");
            let metadata =
                fs::symlink_metadata(&path).map_err(|error| AppError::io(&path, error))?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                hasher.update(b"link\0");
                hasher.update(
                    fs::read_link(&path)
                        .map_err(|error| AppError::io(&path, error))?
                        .to_string_lossy()
                        .as_bytes(),
                );
            } else if file_type.is_dir() {
                hasher.update(b"dir\0");
            } else if file_type.is_file() {
                hasher.update(b"file\0");
                hasher.update(fs::read(&path).map_err(|error| AppError::io(&path, error))?);
            } else {
                hasher.update(b"other\0");
            }
            hasher.update(b"\0");
        }
        Ok(format!("{:x}", hasher.finalize()))
    }

    fn inspect_pi_skill_destination(
        source: &Path,
        destination: &Path,
    ) -> Result<Option<PiSkillDeployment>, AppError> {
        if !destination.exists() && !Self::is_symlink(destination) {
            return Ok(None);
        }
        if Self::is_symlink(destination) {
            let target =
                fs::read_link(destination).map_err(|error| AppError::io(destination, error))?;
            let resolved = if target.is_absolute() {
                target
            } else {
                destination
                    .parent()
                    .map(|parent| parent.join(&target))
                    .unwrap_or(target)
            };
            if matches!(
                (resolved.canonicalize(), source.canonicalize()),
                (Ok(resolved), Ok(source)) if resolved == source
            ) {
                return Ok(Some(PiSkillDeployment::Symlink {
                    expected_target: resolved,
                }));
            }
        } else if destination.is_dir() {
            if let (Ok(destination_hash), Ok(source_hash)) = (
                Self::compute_pi_deployment_hash(destination),
                Self::compute_pi_deployment_hash(source),
            ) {
                if destination_hash == source_hash {
                    return Ok(Some(PiSkillDeployment::Copy {
                        expected_hash: destination_hash,
                    }));
                }
            }
        }
        Err(AppError::InvalidInput(format!(
            "Pi skill destination already exists and is not managed by CC Switch: {}",
            destination.display()
        )))
    }

    fn preflight_pi_skill_destination(
        source: &Path,
        directory: &str,
        app: &AppType,
    ) -> Result<(), AppError> {
        if !matches!(app, AppType::Pi) {
            return Ok(());
        }
        let ssot_dir = Self::get_ssot_dir()?;
        let app_dir = Self::get_distinct_app_skills_dir(&ssot_dir, app)?;
        let destination = app_dir.join(directory);
        if destination.exists() || Self::is_symlink(&destination) {
            Self::inspect_pi_skill_destination(source, &destination)?;
        }
        Ok(())
    }

    fn pi_skill_destination_is_managed(source: &Path, destination: &Path) -> bool {
        Self::inspect_pi_skill_destination(source, destination)
            .ok()
            .flatten()
            .is_some()
    }

    fn copy_skill_to_app(source: &Path, destination: &Path) -> Result<(), AppError> {
        Self::copy_dir_recursive(source, destination)
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

        let directory = Self::require_valid_directory(directory)?;
        let ssot_dir = Self::get_ssot_dir()?;
        let source = ssot_dir.join(&directory);
        Self::validate_sync_source_dir(&source, &directory)?;

        let app_dir = Self::get_distinct_app_skills_dir(&ssot_dir, app)?;
        // D5: allow creating target app dirs during skills sync.
        fs::create_dir_all(&app_dir).map_err(|e| AppError::io(&app_dir, e))?;

        let dest = app_dir.join(&directory);

        // Pi's native Skills directory may contain user-managed entries. Match
        // upstream by replacing only a destination that still mirrors the
        // CC Switch source; preserve every conflicting entry.
        if matches!(app, AppType::Pi) && (dest.exists() || Self::is_symlink(&dest)) {
            Self::inspect_pi_skill_destination(&source, &dest)?;
        }

        match method {
            SyncMethod::Auto => {
                if dest.exists() && !Self::is_symlink(&dest) {
                    return Self::replace_dest_with_copy(&source, &dest, &directory);
                }

                if Self::is_symlink(&dest) {
                    Self::remove_path(&dest)?;
                }

                match Self::create_symlink(&source, &dest) {
                    Ok(()) => Ok(()),
                    Err(err) => {
                        log::warn!(
                            "Symlink 创建失败，将回退到文件复制: {} -> {}. 错误: {err}",
                            source.display(),
                            dest.display()
                        );
                        Self::replace_dest_with_copy(&source, &dest, &directory)
                    }
                }
            }
            SyncMethod::Symlink => {
                if dest.exists() || Self::is_symlink(&dest) {
                    Self::remove_path(&dest)?;
                }
                Self::create_symlink(&source, &dest)
            }
            SyncMethod::Copy => Self::replace_dest_with_copy(&source, &dest, &directory),
        }
    }

    fn validate_sync_source_dir(source: &Path, directory: &str) -> Result<(), AppError> {
        if !source.is_dir() {
            return Err(AppError::Message(format!(
                "Skill 不存在于 SSOT: {directory}"
            )));
        }

        let manifest = source.join("SKILL.md");
        if !manifest.is_file() {
            return Err(AppError::Message(format!(
                "Skill 源目录缺少 SKILL.md，拒绝同步以避免覆盖目标目录: {}",
                source.display()
            )));
        }

        Ok(())
    }

    fn replace_dest_with_copy(source: &Path, dest: &Path, directory: &str) -> Result<(), AppError> {
        Self::validate_sync_source_dir(source, directory)?;

        let parent = dest.parent().ok_or_else(|| {
            AppError::InvalidInput(format!("Invalid Skill destination: {}", dest.display()))
        })?;
        fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp_name = Self::sanitize_backup_segment(directory);
        let tmp = parent.join(format!(".{tmp_name}.tmp-{}-{nonce}", std::process::id()));

        if tmp.exists() || Self::is_symlink(&tmp) {
            Self::remove_path(&tmp)?;
        }

        if let Err(error) = Self::copy_dir_recursive(source, &tmp) {
            let _ = Self::remove_path(&tmp);
            return Err(error);
        }

        if dest.exists() || Self::is_symlink(dest) {
            Self::remove_path(dest)?;
        }

        if let Err(error) = fs::rename(&tmp, dest) {
            let _ = Self::remove_path(&tmp);
            return Err(AppError::IoContext {
                context: format!(
                    "替换 Skill 目录失败: {} -> {}",
                    tmp.display(),
                    dest.display()
                ),
                source: error,
            });
        }

        Ok(())
    }

    fn sanitize_backup_segment(segment: &str) -> String {
        let sanitized = segment
            .chars()
            .map(|character| match character {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => character,
                _ => '-',
            })
            .collect::<String>()
            .trim_matches('-')
            .to_string();

        if sanitized.is_empty() {
            "skill".to_string()
        } else {
            sanitized
        }
    }

    /// Return whether a path is a symlink whose target lives under the SSOT.
    fn is_symlink_to_ssot(path: &Path, ssot_dir: &Path) -> bool {
        if !Self::is_symlink(path) {
            return false;
        }

        let Ok(target) = fs::read_link(path) else {
            return false;
        };

        if target.is_absolute() && target.starts_with(ssot_dir) {
            return true;
        }

        let resolved = path
            .parent()
            .map(|parent| parent.join(&target))
            .unwrap_or(target.clone());
        let canonical_ssot = ssot_dir
            .canonicalize()
            .unwrap_or_else(|_| ssot_dir.to_path_buf());
        let canonical_target = resolved.canonicalize().unwrap_or(resolved);

        canonical_target.starts_with(&canonical_ssot)
    }

    fn sync_updated_skill_to_app(
        directory: &str,
        app: &AppType,
        method: SyncMethod,
    ) -> Result<(), AppError> {
        if !Self::app_supports_skills(app) {
            return Ok(());
        }

        let directory = Self::require_valid_directory(directory)?;
        let ssot_dir = Self::get_ssot_dir()?;
        let source = ssot_dir.join(&directory);
        Self::validate_sync_source_dir(&source, &directory)?;
        let app_dir = Self::get_distinct_app_skills_dir(&ssot_dir, app)?;
        fs::create_dir_all(&app_dir).map_err(|e| AppError::io(&app_dir, e))?;
        let dest = app_dir.join(&directory);
        if matches!(app, AppType::Pi)
            && (dest.exists() || Self::is_symlink(&dest))
            && !Self::pi_skill_destination_is_managed(&source, &dest)
        {
            return Err(AppError::InvalidInput(format!(
                "Pi skill destination changed outside CC Switch and was preserved: {}",
                dest.display()
            )));
        }
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
                if dest.exists() && !Self::is_symlink(&dest) {
                    Self::copy_dir_recursive(&source, &next)?;
                } else if let Err(error) = Self::create_symlink(&source, &next) {
                    log::warn!(
                        "Symlink creation failed during Skill update, falling back to copy: {error}"
                    );
                    Self::copy_skill_to_app(&source, &next)?;
                }
            }
            SyncMethod::Symlink => Self::create_symlink(&source, &next)?,
            SyncMethod::Copy => Self::copy_skill_to_app(&source, &next)?,
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

    fn refresh_pi_skill_destination(
        source: &Path,
        destination: &Path,
        deployment: &PiSkillDeployment,
    ) -> Result<(), AppError> {
        match deployment {
            PiSkillDeployment::Symlink { expected_target } => {
                if !Self::is_symlink(destination) {
                    return Err(AppError::InvalidInput(format!(
                        "Pi skill destination changed during update: {}",
                        destination.display()
                    )));
                }
                let target =
                    fs::read_link(destination).map_err(|error| AppError::io(destination, error))?;
                let resolved = if target.is_absolute() {
                    target
                } else {
                    destination
                        .parent()
                        .map(|parent| parent.join(&target))
                        .unwrap_or(target)
                };
                if &resolved != expected_target {
                    return Err(AppError::InvalidInput(format!(
                        "Pi skill destination changed during update: {}",
                        destination.display()
                    )));
                }
                Ok(())
            }
            PiSkillDeployment::Copy { expected_hash } => {
                if Self::is_symlink(destination)
                    || !destination.is_dir()
                    || !matches!(
                        Self::compute_pi_deployment_hash(destination),
                        Ok(current_hash) if &current_hash == expected_hash
                    )
                {
                    return Err(AppError::InvalidInput(format!(
                        "Pi skill destination changed during update: {}",
                        destination.display()
                    )));
                }

                let directory =
                    destination
                        .file_name()
                        .and_then(OsStr::to_str)
                        .ok_or_else(|| {
                            AppError::InvalidInput(format!(
                                "Invalid Pi Skill destination: {}",
                                destination.display()
                            ))
                        })?;
                Self::replace_dest_with_copy(source, destination, directory)
            }
        }
    }

    pub fn remove_from_app(directory: &str, app: &AppType) -> Result<(), AppError> {
        if !Self::app_supports_skills(app) {
            return Ok(());
        }

        let directory = Self::require_valid_directory(directory)?;
        let ssot_dir = Self::get_ssot_dir()?;
        let app_dir = Self::get_distinct_app_skills_dir(&ssot_dir, app)?;
        let path = app_dir.join(&directory);
        if path.exists() || Self::is_symlink(&path) {
            if matches!(app, AppType::Pi) {
                let source = Self::get_ssot_dir()?.join(directory);
                if !Self::pi_skill_destination_is_managed(&source, &path) {
                    return Err(AppError::InvalidInput(format!(
                        "Pi skill destination changed outside CC Switch and was preserved: {}",
                        path.display()
                    )));
                }
            }
            Self::remove_path(&path)?;
        }
        Ok(())
    }

    pub fn sync_to_app(index: &SkillsIndex, app: &AppType) -> Result<(), AppError> {
        let _state_guard = skill_state_read_guard();
        Self::sync_to_app_unlocked(index, app)
    }

    /// Caller must hold either the Skills state read or write guard.
    fn sync_to_app_unlocked(index: &SkillsIndex, app: &AppType) -> Result<(), AppError> {
        if !Self::app_supports_skills(app) {
            return Ok(());
        }

        let ssot_dir = Self::get_ssot_dir()?;
        let app_dir = Self::get_distinct_app_skills_dir(&ssot_dir, app)?;
        let indexed_skills: HashMap<String, &InstalledSkill> = index
            .skills
            .values()
            .map(|skill| (skill.directory.to_lowercase(), skill))
            .collect();

        if app_dir.exists() {
            for entry in fs::read_dir(&app_dir).map_err(|error| AppError::io(&app_dir, error))? {
                let entry = entry.map_err(|error| AppError::io(&app_dir, error))?;
                let path = entry.path();
                let dir_name = entry.file_name().to_string_lossy().to_string();

                if dir_name.starts_with('.') {
                    continue;
                }

                if let Some(skill) = indexed_skills.get(&dir_name.to_lowercase()) {
                    if !skill.apps.is_enabled_for(app) {
                        Self::remove_path(&path)?;
                    }
                    continue;
                }

                if Self::is_symlink_to_ssot(&path, &ssot_dir) {
                    Self::remove_path(&path)?;
                }
            }
        }

        for skill in index.skills.values() {
            if skill.apps.is_enabled_for(app) {
                if let Err(error) = Self::sync_to_app_dir(&skill.directory, app, index.sync_method)
                {
                    log::warn!(
                        "同步 Skill {} 到 {app:?} 失败，已跳过该条: {error}",
                        skill.directory
                    );
                }
            }
        }
        Ok(())
    }

    /// Best-effort sync for live-flow triggers (provider switch etc).
    pub fn sync_all_enabled_best_effort() -> Result<(), AppError> {
        let mut index = Self::load_index()?;
        let _ = Self::migrate_ssot_if_pending(&mut index);
        let _state_guard = skill_state_read_guard();
        let index = Self::load_index_unlocked()?;
        for app in Self::supported_skill_apps() {
            if let Err(e) = Self::sync_to_app_unlocked(&index, &app) {
                log::warn!("同步 Skill 到 {app:?} 失败: {e}");
            }
        }
        Ok(())
    }

    pub fn sync_all_enabled(app: Option<&AppType>) -> Result<(), AppError> {
        let mut index = Self::load_index()?;
        let _ = Self::migrate_ssot_if_pending(&mut index)?;
        let _state_guard = skill_state_read_guard();
        let index = Self::load_index_unlocked()?;

        match app {
            Some(app) => Self::sync_to_app_unlocked(&index, app)?,
            None => {
                for app in Self::supported_skill_apps() {
                    Self::sync_to_app_unlocked(&index, &app)?;
                }
            }
        }

        Ok(())
    }

    fn migration_tree_hash_with_ignored_root_file(
        dir: &Path,
        ignored_root_file: Option<&OsStr>,
    ) -> Result<String, AppError> {
        use sha2::{Digest, Sha256};

        fn update_framed(hasher: &mut Sha256, bytes: &[u8]) {
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }

        fn visit(
            root: &Path,
            current: &Path,
            ignored_root_file: Option<&OsStr>,
            hasher: &mut Sha256,
        ) -> Result<(), AppError> {
            let mut entries = fs::read_dir(current)
                .map_err(|error| AppError::io(current, error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::io(current, error))?;
            entries.sort_by_key(|entry| entry.file_name());

            for entry in entries {
                let path = entry.path();
                if current == root
                    && ignored_root_file.is_some_and(|ignored| entry.file_name() == ignored)
                {
                    continue;
                }
                let relative = path.strip_prefix(root).unwrap_or(&path);
                let file_type = entry
                    .file_type()
                    .map_err(|error| AppError::io(&path, error))?;
                if file_type.is_symlink() {
                    return Err(AppError::InvalidInput(format!(
                        "Skill storage migration does not follow symbolic links: {}",
                        path.display()
                    )));
                }
                if file_type.is_dir() {
                    hasher.update(b"D");
                    update_framed(hasher, relative.as_os_str().as_encoded_bytes());
                    visit(root, &path, ignored_root_file, hasher)?;
                } else if file_type.is_file() {
                    hasher.update(b"F");
                    update_framed(hasher, relative.as_os_str().as_encoded_bytes());
                    let mut file =
                        fs::File::open(&path).map_err(|error| AppError::io(&path, error))?;
                    let mut content_hasher = Sha256::new();
                    let mut content_len = 0u64;
                    let mut buffer = [0u8; 16 * 1024];
                    loop {
                        let read = file
                            .read(&mut buffer)
                            .map_err(|error| AppError::io(&path, error))?;
                        if read == 0 {
                            break;
                        }
                        content_len = content_len.saturating_add(read as u64);
                        content_hasher.update(&buffer[..read]);
                    }
                    hasher.update(content_len.to_le_bytes());
                    hasher.update(content_hasher.finalize());
                } else {
                    return Err(AppError::InvalidInput(format!(
                        "Unsupported file type in Skill storage migration: {}",
                        path.display()
                    )));
                }
            }
            Ok(())
        }

        let metadata = fs::symlink_metadata(dir).map_err(|error| AppError::io(dir, error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(AppError::InvalidInput(format!(
                "Skill storage migration requires a real directory: {}",
                dir.display()
            )));
        }
        let mut hasher = Sha256::new();
        hasher.update(b"cc-switch-skill-tree-v2\0");
        visit(dir, dir, ignored_root_file, &mut hasher)?;
        Ok(format!("{:x}", hasher.finalize()))
    }

    fn migration_tree_hash(dir: &Path) -> Result<String, AppError> {
        Self::migration_tree_hash_with_ignored_root_file(dir, None)
    }

    pub(crate) fn validate_managed_skill_tree(dir: &Path) -> Result<(), AppError> {
        Self::migration_tree_hash(dir).map(|_| ())
    }

    fn migration_tree_hash_if_present(path: &Path) -> Result<Option<String>, AppError> {
        match fs::symlink_metadata(path) {
            Ok(_) => Self::migration_tree_hash(path).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(AppError::io(path, error)),
        }
    }

    fn copy_migration_tree(src: &Path, dest: &Path) -> Result<(), AppError> {
        let metadata = fs::symlink_metadata(src).map_err(|error| AppError::io(src, error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(AppError::InvalidInput(format!(
                "Skill storage migration requires a real directory: {}",
                src.display()
            )));
        }

        fs::create_dir(dest).map_err(|error| AppError::io(dest, error))?;
        let mut entries = fs::read_dir(src)
            .map_err(|error| AppError::io(src, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| AppError::io(src, error))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let source = entry.path();
            let target = dest.join(entry.file_name());
            let file_type = entry
                .file_type()
                .map_err(|error| AppError::io(&source, error))?;
            if file_type.is_symlink() {
                return Err(AppError::InvalidInput(format!(
                    "Skill storage migration does not follow symbolic links: {}",
                    source.display()
                )));
            }
            if file_type.is_dir() {
                Self::copy_migration_tree(&source, &target)?;
            } else if file_type.is_file() {
                fs::copy(&source, &target).map_err(|error| AppError::io(&target, error))?;
            } else {
                return Err(AppError::InvalidInput(format!(
                    "Unsupported file type in Skill storage migration: {}",
                    source.display()
                )));
            }
        }
        Ok(())
    }

    fn migration_marker_name(journal: &StorageMigrationJournal) -> String {
        format!(".cc-switch-migration-{}", journal.token)
    }

    fn migration_marker_matches(
        target: &Path,
        journal: &StorageMigrationJournal,
    ) -> Result<bool, AppError> {
        let marker = target.join(Self::migration_marker_name(journal));
        match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                let token =
                    fs::read_to_string(&marker).map_err(|error| AppError::io(&marker, error))?;
                if token == journal.token {
                    Ok(true)
                } else {
                    Err(AppError::InvalidInput(format!(
                        "Skill migration marker does not match the active migration: {}",
                        marker.display()
                    )))
                }
            }
            Ok(_) => Err(AppError::InvalidInput(format!(
                "Invalid Skill migration marker: {}",
                marker.display()
            ))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(AppError::io(&marker, error)),
        }
    }

    fn migration_target_hash(
        target: &Path,
        journal: &StorageMigrationJournal,
    ) -> Result<String, AppError> {
        if Self::migration_marker_matches(target, journal)? {
            let marker_name = Self::migration_marker_name(journal);
            Self::migration_tree_hash_with_ignored_root_file(target, Some(OsStr::new(&marker_name)))
        } else {
            Self::migration_tree_hash(target)
        }
    }

    fn stage_migration_copy(
        src: &Path,
        dest: &Path,
        target_root: &Path,
        journal: &StorageMigrationJournal,
    ) -> Result<(), AppError> {
        let staging = tempfile::Builder::new()
            .prefix(".cc-switch-skill-migration-")
            .tempdir_in(target_root)
            .map_err(|error| AppError::io(target_root, error))?;
        let next = staging.path().join("next");
        Self::copy_migration_tree(src, &next)?;
        if Self::migration_tree_hash(src)? != Self::migration_tree_hash(&next)? {
            return Err(AppError::Message(format!(
                "Skill changed while it was being copied: {}",
                src.display()
            )));
        }
        let marker = next.join(Self::migration_marker_name(journal));
        let mut marker_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)
            .map_err(|error| AppError::io(&marker, error))?;
        marker_file
            .write_all(journal.token.as_bytes())
            .map_err(|error| AppError::io(&marker, error))?;
        marker_file
            .flush()
            .map_err(|error| AppError::io(&marker, error))?;
        fs::rename(&next, dest).map_err(|error| AppError::io(dest, error))?;
        Ok(())
    }

    fn migration_deployment_action(
        destination: &Path,
        old_source: &Path,
        new_source: &Path,
        expected_hash: &str,
    ) -> Result<MigrationDeploymentAction, AppError> {
        let metadata = match fs::symlink_metadata(destination) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(MigrationDeploymentAction::Refresh);
            }
            Err(error) => return Err(AppError::io(destination, error)),
        };
        if metadata.file_type().is_symlink() {
            let target =
                fs::read_link(destination).map_err(|error| AppError::io(destination, error))?;
            let target = if target.is_absolute() {
                target
            } else {
                destination
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(target)
            };
            if Self::paths_equal(&target, old_source) || Self::paths_equal(&target, new_source) {
                return Ok(MigrationDeploymentAction::Refresh);
            }
            return Err(AppError::InvalidInput(format!(
                "Refusing to replace an unmanaged Skill deployment: {}",
                destination.display()
            )));
        }
        if metadata.is_dir() && Self::migration_tree_hash(destination)? == expected_hash {
            // An ordinary directory has no ownership proof. If it already has
            // the current content, leave it byte-for-byte untouched.
            return Ok(MigrationDeploymentAction::AlreadyCurrent);
        }
        Err(AppError::InvalidInput(format!(
            "Refusing to replace an unmanaged Skill deployment: {}",
            destination.display()
        )))
    }

    fn sync_migrated_skill_to_app(
        directory: &str,
        app: &AppType,
        method: SyncMethod,
        old_root: &Path,
        new_root: &Path,
    ) -> Result<(), AppError> {
        let directory = Self::require_valid_directory(directory)?;
        let source = new_root.join(&directory);
        Self::validate_sync_source_dir(&source, &directory)?;
        let expected_hash = Self::migration_tree_hash(&source)?;
        let app_dir = Self::get_distinct_app_skills_dir(new_root, app)?;
        let destination = app_dir.join(&directory);
        let action = Self::migration_deployment_action(
            &destination,
            &old_root.join(&directory),
            &source,
            &expected_hash,
        )?;
        if action == MigrationDeploymentAction::AlreadyCurrent {
            return Ok(());
        }
        Self::sync_updated_skill_to_app(&directory, app, method)
    }

    fn storage_migration_journal_path() -> PathBuf {
        get_app_config_dir().join(STORAGE_MIGRATION_JOURNAL_FILE)
    }

    fn load_storage_migration_journal() -> Result<Option<StorageMigrationJournal>, AppError> {
        let path = Self::storage_migration_journal_path();
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(AppError::io(&path, error)),
        };
        serde_json::from_str(&raw).map(Some).map_err(|error| {
            AppError::InvalidInput(format!(
                "Invalid Skill storage migration journal {}: {error}",
                path.display()
            ))
        })
    }

    fn save_storage_migration_journal(journal: &StorageMigrationJournal) -> Result<(), AppError> {
        write_json_file(&Self::storage_migration_journal_path(), journal)
    }

    fn clear_storage_migration_journal() -> Result<(), AppError> {
        let path = Self::storage_migration_journal_path();
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AppError::io(&path, error)),
        }
    }

    fn ensure_journal_matches_index(
        journal: &StorageMigrationJournal,
        index: &SkillsIndex,
    ) -> Result<(), AppError> {
        if journal.hashes.len() == index.skills.len()
            && index
                .skills
                .keys()
                .all(|directory| journal.hashes.contains_key(directory))
        {
            return Ok(());
        }
        Err(AppError::InvalidInput(
            "Managed Skills changed during an unfinished storage migration; finish or recover that migration before installing or removing Skills"
                .to_string(),
        ))
    }

    fn remove_storage_migration_markers(
        root: &Path,
        index: &SkillsIndex,
        journal: &StorageMigrationJournal,
    ) -> Result<(), AppError> {
        let marker_name = Self::migration_marker_name(journal);
        for skill in index.skills.values() {
            let directory = Self::require_valid_directory(&skill.directory)?;
            let target = root.join(&directory);
            if Self::migration_marker_matches(&target, journal)? {
                let marker = target.join(&marker_name);
                fs::remove_file(&marker).map_err(|error| AppError::io(&marker, error))?;
            }
        }
        Ok(())
    }

    /// Move managed Skills between the CC Switch and shared Agent Skills SSOTs.
    /// Target copies are verified before the setting changes. Old copies remain
    /// available until every enabled app deployment has been refreshed.
    pub fn migrate_storage(target: SkillStorageLocation) -> Result<MigrationResult, AppError> {
        let _state_guard = skill_state_write_guard();
        let current = crate::settings::get_skill_storage_location();
        let mut journal = Self::load_storage_migration_journal()?;
        if current == target && journal.is_none() {
            return Ok(MigrationResult::default());
        }

        let new_dir = Self::ssot_dir_for(target)?;
        let old_location = match target {
            SkillStorageLocation::CcSwitch => SkillStorageLocation::Unified,
            SkillStorageLocation::Unified => SkillStorageLocation::CcSwitch,
        };
        let old_dir = Self::ssot_dir_for(old_location)?;
        Self::validate_storage_root(old_location, &old_dir)?;
        Self::validate_storage_root(target, &new_dir)?;
        if Self::paths_overlap(&old_dir, &new_dir) {
            return Err(AppError::InvalidInput(format!(
                "Skill storage directories cannot be equal or overlap: {} and {}",
                old_dir.display(),
                new_dir.display()
            )));
        }
        Self::validate_skill_storage_destination(&old_dir)?;
        Self::validate_skill_storage_destination(&new_dir)?;

        let index = Self::load_index_unlocked()?;
        // The migration journal and every filesystem sink below use the installed
        // directory as a path segment. Reject a poisoned row before creating the
        // destination or performing any per-Skill write.
        for skill in index.skills.values() {
            Self::require_valid_directory(&skill.directory)?;
        }
        create_managed_config_dir_all(&new_dir)?;
        Self::validate_storage_root(target, &new_dir)?;
        if let Some(active) = &journal {
            if active.source != old_location
                || active.target != target
                || (current != active.source && current != active.target)
            {
                return Err(AppError::InvalidInput(format!(
                    "An unfinished Skill storage migration from {:?} to {:?} must be reconciled before starting another migration",
                    active.source, active.target
                )));
            }
            if current == active.source {
                Self::ensure_journal_matches_index(active, &index)?;
            }
        }

        let mut copies = Vec::new();
        let mut result = MigrationResult::default();

        // A first switch never infers ownership from content equality. Persist
        // an intent before copying; an embedded token then proves which target
        // directories were created by an interrupted attempt.
        if journal.is_none() && current != target {
            let mut hashes = HashMap::new();
            for skill in index.skills.values() {
                let directory = Self::require_valid_directory(&skill.directory)?;
                let src = old_dir.join(&directory);
                let dst = new_dir.join(&directory);
                let source_hash = Self::migration_tree_hash_if_present(&src)?.ok_or_else(|| {
                    AppError::InvalidInput(format!(
                        "Managed Skill is missing from the current storage: {}",
                        skill.directory
                    ))
                })?;
                match fs::symlink_metadata(&dst) {
                    Ok(_) => {
                        return Err(AppError::InvalidInput(format!(
                            "Refusing to claim a pre-existing Skill migration target: {}",
                            dst.display()
                        )));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(AppError::io(&dst, error)),
                }
                hashes.insert(directory, source_hash);
            }
            let next = StorageMigrationJournal {
                source: old_location,
                target,
                token: uuid::Uuid::new_v4().to_string(),
                hashes,
            };
            Self::save_storage_migration_journal(&next)?;
            journal = Some(next);
        }

        // Preflight the complete managed set before writing a target directory.
        for skill in index.skills.values() {
            let directory = Self::require_valid_directory(&skill.directory)?;
            let src = old_dir.join(&directory);
            let dst = new_dir.join(&directory);
            match Self::migration_tree_hash_if_present(&dst)? {
                Some(_) if current == target => {
                    if let Some(active) = &journal {
                        if Self::migration_marker_matches(&dst, active)? {
                            let expected = active.hashes.get(&directory).ok_or_else(|| {
                                AppError::InvalidInput(format!(
                                    "Skill is missing from the active migration journal: {}",
                                    skill.directory
                                ))
                            })?;
                            if &Self::migration_target_hash(&dst, active)? != expected {
                                return Err(AppError::InvalidInput(format!(
                                    "Interrupted Skill migration target changed: {}",
                                    dst.display()
                                )));
                            }
                        }
                    }
                }
                Some(_) => {
                    let active = journal.as_ref().ok_or_else(|| {
                        AppError::InvalidInput(format!(
                            "Refusing to claim a pre-existing Skill migration target: {}",
                            dst.display()
                        ))
                    })?;
                    if !Self::migration_marker_matches(&dst, active)? {
                        return Err(AppError::InvalidInput(format!(
                            "Refusing to claim a pre-existing Skill migration target: {}",
                            dst.display()
                        )));
                    }
                    let expected = active.hashes.get(&directory).ok_or_else(|| {
                        AppError::InvalidInput(format!(
                            "Skill is missing from the active migration journal: {}",
                            skill.directory
                        ))
                    })?;
                    if &Self::migration_target_hash(&dst, active)? != expected {
                        return Err(AppError::InvalidInput(format!(
                            "Interrupted Skill migration target changed: {}",
                            dst.display()
                        )));
                    }
                    if Self::migration_tree_hash(&src)? != *expected {
                        return Err(AppError::InvalidInput(format!(
                            "Skill changed during an unfinished storage migration: {}",
                            skill.directory
                        )));
                    }
                    result.skipped_count += 1;
                }
                None => {
                    let active = journal.as_ref().ok_or_else(|| {
                        AppError::InvalidInput(format!(
                            "Managed Skill is missing from both storage locations: {}",
                            skill.directory
                        ))
                    })?;
                    let expected = active.hashes.get(&directory).ok_or_else(|| {
                        AppError::InvalidInput(format!(
                            "Skill is missing from the active migration journal: {}",
                            skill.directory
                        ))
                    })?;
                    if Self::migration_tree_hash(&src)? != *expected {
                        return Err(AppError::InvalidInput(format!(
                            "Skill changed during an unfinished storage migration: {}",
                            skill.directory
                        )));
                    }
                    copies.push((src, dst));
                }
            }
        }

        for (src, dst) in copies {
            let active = journal
                .as_ref()
                .expect("copies require a migration journal");
            Self::stage_migration_copy(&src, &dst, &new_dir, active)?;
            result.migrated_count += 1;
        }

        if current != target {
            crate::settings::set_skill_storage_location(target)?;
        }
        if let Some(active) = &journal {
            Self::remove_storage_migration_markers(&new_dir, &index, active)?;
        }

        // Reconcile per Skill/app through the existing staged deployment path.
        // Unknown destinations are preserved and reported instead of overwritten.
        for skill in index.skills.values() {
            for app in Self::supported_skill_apps() {
                if skill.apps.is_enabled_for(&app) {
                    if let Err(error) = Self::sync_migrated_skill_to_app(
                        &skill.directory,
                        &app,
                        index.sync_method,
                        &old_dir,
                        &new_dir,
                    ) {
                        result.errors.push(format!(
                            "{}/{}: {error}",
                            app.as_str(),
                            skill.directory
                        ));
                    }
                }
            }
        }

        // Keep the old SSOT as a working fallback whenever deployment is partial.
        // A same-target retry re-enters this path, repairs deployment, then cleans it.
        if result.errors.is_empty() {
            if let Some(active) = journal.as_ref() {
                for skill in index.skills.values() {
                    let directory = Self::require_valid_directory(&skill.directory)?;
                    let Some(expected_source_hash) = active.hashes.get(&directory) else {
                        // This Skill was added after settings moved to the target;
                        // it has no old migration-owned copy to clean up.
                        continue;
                    };
                    let source = old_dir.join(&directory);
                    let target_path = new_dir.join(&directory);
                    let source_hash = match Self::migration_tree_hash_if_present(&source) {
                        Ok(Some(hash)) => hash,
                        Ok(None) => continue,
                        Err(error) => {
                            result.errors.push(format!(
                                "{}: could not verify the old copy; it was preserved: {error}",
                                skill.directory
                            ));
                            continue;
                        }
                    };
                    let target_hash = match Self::migration_tree_hash(&target_path) {
                        Ok(hash) => hash,
                        Err(error) => {
                            result.errors.push(format!(
                            "{}: could not verify the new copy; the old copy was preserved: {error}",
                            skill.directory
                        ));
                            continue;
                        }
                    };
                    let source_is_original = source_hash == *expected_source_hash;
                    if (current == active.target && !source_is_original)
                        || (current != active.target && source_hash != target_hash)
                    {
                        result.errors.push(format!(
                            "{}: old and new copies differ; the old copy was preserved",
                            skill.directory
                        ));
                        continue;
                    }
                    if let Err(error) = Self::remove_path(&source) {
                        result.errors.push(format!(
                            "{}: failed to remove the old copy: {error}",
                            skill.directory
                        ));
                    }
                }
            }
        }

        if result.errors.is_empty() && journal.is_some() {
            if let Err(error) = Self::clear_storage_migration_journal() {
                result.errors.push(format!(
                    "failed to clear the completed Skill storage migration journal: {error}"
                ));
            }
        }

        Ok(result)
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

    /// Validate and normalize a repository-relative Skill source path.
    fn sanitize_skill_source_path(raw: &str) -> Option<PathBuf> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }

        let mut normalized = PathBuf::new();
        let mut has_component = false;
        for component in Path::new(trimmed).components() {
            match component {
                Component::Normal(name) => {
                    let segment = name.to_string_lossy().trim().to_string();
                    if segment.is_empty() || segment == "." || segment == ".." {
                        return None;
                    }
                    normalized.push(segment);
                    has_component = true;
                }
                Component::CurDir
                | Component::ParentDir
                | Component::RootDir
                | Component::Prefix(_) => return None,
            }
        }

        has_component.then_some(normalized)
    }

    /// Validate and normalize the single path segment used for an installed Skill.
    fn sanitize_install_name(raw: &str) -> Option<String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.contains('/') || trimmed.contains('\\') {
            return None;
        }

        let path = Path::new(trimmed);
        let mut components = path.components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(name)), None) => {
                let normalized = name.to_string_lossy().trim().to_string();
                if normalized.is_empty()
                    || normalized == "."
                    || normalized == ".."
                    || normalized.starts_with('.')
                {
                    None
                } else {
                    Some(normalized)
                }
            }
            _ => None,
        }
    }

    fn source_install_name(directory: &str) -> String {
        Path::new(directory)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| directory.to_string())
    }

    /// Validate an externally sourced installed-directory value without rewriting it.
    fn require_valid_directory(directory: &str) -> Result<String, AppError> {
        match Self::sanitize_install_name(directory) {
            Some(normalized) if normalized == directory => Ok(normalized),
            _ => Err(AppError::InvalidInput(format!(
                "Invalid Skill directory (possible path traversal): {directory:?}"
            ))),
        }
    }

    fn reject_unmanaged_install_collision(
        destination: &Path,
        directory: &str,
        new_repo: &str,
    ) -> Result<(), AppError> {
        if crate::settings::get_skill_storage_location() == SkillStorageLocation::Unified
            && fs::symlink_metadata(destination).is_ok()
        {
            return Err(AppError::Message(format_skill_error(
                "SKILL_DIRECTORY_CONFLICT",
                &[
                    ("directory", directory),
                    ("existing_repo", "unmanaged local directory"),
                    ("new_repo", new_repo),
                ],
                Some("importOrUninstallFirst"),
            )));
        }
        Ok(())
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

            // Remote I/O is complete; keep the local DB and SSOT stable while
            // hashes are read and missing hash metadata is backfilled.
            let _state_guard = skill_state_read_guard();

            for skill in skills {
                let directory = match Self::require_valid_directory(&skill.directory) {
                    Ok(directory) => directory,
                    Err(error) => {
                        result.failures.push(format!("{}: {error}", skill.id));
                        continue;
                    }
                };
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

                let local_dir = ssot_dir.join(&directory);
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
        let mut skill = db
            .get_installed_skill(skill_id)?
            .ok_or_else(|| AppError::Message(format!("Skill not found: {skill_id}")))?;
        let directory = Self::require_valid_directory(&skill.directory)?;
        skill.apps.pi = Self::skill_exists_in_app(&skill.directory, &AppType::Pi);

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

        let content_hash = Self::compute_dir_hash(&source)?;
        let (new_name, new_description) =
            Self::read_skill_name_desc(&source.join("SKILL.md"), &skill.directory);
        // Remote I/O and inspection are complete. Serialize SSOT staging, the
        // final replacement, metadata write, and app refresh as one mutation.
        let _state_guard = skill_state_write_guard();
        let mut current = db.get_installed_skill(skill_id)?.ok_or_else(|| {
            AppError::Message(format!("Skill was removed during update: {skill_id}"))
        })?;
        current.apps.pi = Self::skill_exists_in_app(&current.directory, &AppType::Pi);
        if current.directory != skill.directory
            || current.repo_owner != skill.repo_owner
            || current.repo_name != skill.repo_name
            || current.repo_branch != skill.repo_branch
            || current.readme_url != skill.readme_url
            || current.content_hash != skill.content_hash
            || current.updated_at != skill.updated_at
            || current.installed_at != skill.installed_at
        {
            return Err(AppError::Message(format!(
                "Skill changed during update; run the update again: {skill_id}"
            )));
        }
        let ssot_dir = Self::get_ssot_dir()?;
        let staging = tempfile::Builder::new()
            .prefix(".cc-switch-skill-update-")
            .tempdir_in(&ssot_dir)
            .map_err(|e| AppError::io(&ssot_dir, e))?;
        let next = staging.path().join("next");
        let previous = staging.path().join("previous");
        Self::copy_dir_recursive(&source, &next)?;
        drop(temp_dir);
        let sync_method = Self::get_sync_method()?;
        let dest = ssot_dir.join(&directory);
        let pi_deployment = if current.apps.pi {
            let pi_dir = Self::get_distinct_app_skills_dir(&ssot_dir, &AppType::Pi)?;
            Self::inspect_pi_skill_destination(&dest, &pi_dir.join(&directory))?
        } else {
            None
        };
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
        if let Some(deployment) = pi_deployment.as_ref() {
            let pi_destination = Self::get_app_skills_dir(&AppType::Pi)?.join(&updated.directory);
            if let Err(error) =
                Self::refresh_pi_skill_destination(&dest, &pi_destination, deployment)
            {
                log::warn!(
                    "Updated Skill {} but failed to sync it to Pi: {error}",
                    updated.id
                );
                deployment_failures.push(format!("Pi: {error}"));
            }
        }
        for app in Self::supported_skill_apps() {
            if matches!(app, AppType::Pi) {
                continue;
            }
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
        let _state_guard = skill_state_write_guard();
        crate::settings::set_skill_sync_method(method)
    }

    pub fn upsert_repo(repo: SkillRepo) -> Result<(), AppError> {
        let _state_guard = skill_state_write_guard();
        Database::init()?.save_skill_repo(&repo)
    }

    pub fn remove_repo(owner: &str, name: &str) -> Result<(), AppError> {
        let _state_guard = skill_state_write_guard();
        let db = Database::init()?;
        db.delete_skill_repo(owner, name)
    }

    pub fn set_repo_enabled(owner: &str, name: &str, enabled: bool) -> Result<bool, AppError> {
        let _state_guard = skill_state_write_guard();
        Database::init()?.set_skill_repo_enabled(owner, name, enabled)
    }

    fn resolve_directory_from_input(index: &SkillsIndex, input: &str) -> Option<String> {
        // Keep poisoned legacy rows removable from the TUI, which submits the
        // stored directory verbatim. Normalized matching remains available for
        // ordinary CLI input below.
        if index.skills.contains_key(input) {
            return Some(input.to_string());
        }

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

    /// Caller must hold the Skills state write guard.
    fn reuse_existing_install(
        discoverable: &DiscoverableSkill,
        install_name: &str,
        app: &AppType,
    ) -> Result<Option<InstalledSkill>, AppError> {
        let db = Database::init()?;
        for existing in db.get_all_installed_skills()?.values() {
            if !existing.directory.eq_ignore_ascii_case(install_name) {
                continue;
            }

            let same_repo = existing.repo_owner.as_deref()
                == Some(discoverable.repo_owner.as_str())
                && existing.repo_name.as_deref() == Some(discoverable.repo_name.as_str());
            if !same_repo {
                let existing_repo = format!(
                    "{}/{}",
                    existing.repo_owner.as_deref().unwrap_or("unknown"),
                    existing.repo_name.as_deref().unwrap_or("unknown")
                );
                let new_repo = format!("{}/{}", discoverable.repo_owner, discoverable.repo_name);
                return Err(AppError::Message(format_skill_error(
                    "SKILL_DIRECTORY_CONFLICT",
                    &[
                        ("directory", install_name),
                        ("existing_repo", existing_repo.as_str()),
                        ("new_repo", new_repo.as_str()),
                    ],
                    Some("uninstallFirst"),
                )));
            }

            let mut updated = existing.clone();
            updated.apps.set_enabled_for(app, true);
            db.save_skill(&updated)?;
            Self::sync_to_app_dir(&updated.directory, app, Self::get_sync_method()?)?;
            return Ok(Some(updated));
        }

        Ok(None)
    }

    /// Caller must hold the Skills state write guard.
    fn persist_and_sync_new_skill(
        db: &Database,
        installed: &InstalledSkill,
        app: &AppType,
        sync_method: SyncMethod,
    ) -> Result<(), AppError> {
        let source = Self::get_ssot_dir()?.join(&installed.directory);
        Self::preflight_pi_skill_destination(&source, &installed.directory, app)?;
        db.save_skill(installed)?;
        if let Err(error) = Self::sync_to_app_dir(&installed.directory, app, sync_method) {
            if let Err(rollback_error) = db.delete_skill(&installed.id) {
                log::error!(
                    "Failed to roll back Skill {} after sync error: {rollback_error}",
                    installed.id
                );
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn toggle_app(directory_or_id: &str, app: &AppType, enabled: bool) -> Result<(), AppError> {
        let _state_guard = skill_state_write_guard();
        let mut index = Self::load_index_unlocked()?;
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

        Database::init()?.save_skill(record)?;
        Ok(())
    }

    pub fn set_apps(directory_or_id: &str, apps: SkillApps) -> Result<bool, AppError> {
        let _state_guard = skill_state_write_guard();
        let mut index = Self::load_index_unlocked()?;
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

        Database::init()?.save_skill(record)?;
        Ok(true)
    }

    pub fn uninstall(directory_or_id: &str) -> Result<(), AppError> {
        let _state_guard = skill_state_write_guard();
        let index = Self::load_index_unlocked()?;
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

        match Self::require_valid_directory(&record.directory) {
            Ok(directory) => {
                // Remove from app dirs (best effort).
                for app in Self::supported_skill_apps() {
                    if let Err(error) = Self::remove_from_app(&directory, &app) {
                        log::warn!("从 {app:?} 删除 Skill {directory} 失败: {error}");
                    }
                }

                // Remove from SSOT.
                let ssot_dir = Self::get_ssot_dir()?;
                let ssot_path = ssot_dir.join(&directory);
                if ssot_path.exists() {
                    fs::remove_dir_all(&ssot_path)
                        .map_err(|error| AppError::io(&ssot_path, error))?;
                }
            }
            Err(error) => {
                // A poisoned row must remain removable without touching paths outside
                // the managed roots.
                log::warn!(
                    "Skill {} 的 directory 非法（{:?}），跳过文件清理，仅删除数据库记录: {error}",
                    record.id,
                    record.directory
                );
            }
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

        // Repository sources may be nested, but the installed directory is one safe segment.
        let source_rel =
            Self::sanitize_skill_source_path(&discoverable.directory).ok_or_else(|| {
                AppError::InvalidInput(format!(
                    "Invalid Skill directory: {:?}",
                    discoverable.directory
                ))
            })?;
        let install_name = source_rel
            .file_name()
            .and_then(|name| Self::sanitize_install_name(&name.to_string_lossy()))
            .ok_or_else(|| {
                AppError::InvalidInput(format!(
                    "Invalid Skill directory: {:?}",
                    discoverable.directory
                ))
            })?;

        let new_repo = format!("{}/{}", discoverable.repo_owner, discoverable.repo_name);
        let needs_download = {
            let _state_guard = skill_state_write_guard();
            if let Some(existing) = Self::reuse_existing_install(&discoverable, &install_name, app)?
            {
                return Ok(existing);
            }
            let dest = Self::get_ssot_dir()?.join(&install_name);
            Self::reject_unmanaged_install_collision(&dest, &install_name, &new_repo)?;
            !dest.exists()
        };

        let mut installed_branch = discoverable.repo_branch.clone();
        let mut installed_readme_url = discoverable.readme_url.clone();
        let mut downloaded_source: Option<(DownloadedRepoGuard, PathBuf)> = None;
        if needs_download {
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

            let temp_dir = DownloadedRepoGuard::new(temp_dir);
            let source = Self::resolve_skill_source_dir(temp_dir.path(), &discoverable.directory)?
                .ok_or_else(|| {
                    AppError::Message(format_skill_error(
                        "SKILL_DIR_NOT_FOUND",
                        &[("directory", install_name.as_str())],
                        Some("checkRepoUrl"),
                    ))
                })?;

            if !source.exists() {
                let source_path_string = source.display().to_string();
                return Err(AppError::Message(format_skill_error(
                    "SKILL_DIR_NOT_FOUND",
                    &[("path", source_path_string.as_str())],
                    Some("checkRepoUrl"),
                )));
            }

            let source_relative = source.strip_prefix(temp_dir.path()).map_err(|_| {
                AppError::Message("Remote Skill source escaped its repository".into())
            })?;
            let relative_path = source_relative.to_string_lossy().replace('\\', "/");
            let doc_path = if relative_path.is_empty() {
                "SKILL.md".to_string()
            } else {
                format!("{relative_path}/SKILL.md")
            };
            installed_readme_url = Some(Self::build_skill_doc_url(
                &repo.owner,
                &repo.name,
                &used_branch,
                &doc_path,
            ));
            installed_branch = used_branch;
            downloaded_source = Some((temp_dir, source));
        }

        // Network I/O is complete. Re-check under the write guard before any
        // SSOT or database mutation, then keep the projection consistent.
        let _state_guard = skill_state_write_guard();
        if let Some(existing) = Self::reuse_existing_install(&discoverable, &install_name, app)? {
            return Ok(existing);
        }
        let ssot_dir = Self::get_ssot_dir()?;
        let dest = ssot_dir.join(&install_name);
        Self::reject_unmanaged_install_collision(&dest, &install_name, &new_repo)?;
        if !dest.exists() {
            let source = downloaded_source
                .as_ref()
                .map(|(_, source)| source)
                .ok_or_else(|| {
                    AppError::Message("Skill directory changed during install; retry".into())
                })?;
            Self::copy_dir_recursive(source, &dest)?;
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

        let db = Database::init()?;
        Self::persist_and_sync_new_skill(&db, &installed, app, Self::get_sync_method()?)?;

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
            .filter(|skill| {
                skill.directory.eq_ignore_ascii_case(spec)
                    || Path::new(&skill.directory)
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(spec))
            })
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
        let _state_guard = skill_state_read_guard();
        let index = Self::load_index_unlocked()?;
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
        let _state_guard = skill_state_write_guard();
        let mut index = Self::load_index_unlocked()?;
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
            let dir_name = match Self::require_valid_directory(&selection.directory) {
                Ok(directory) => directory,
                Err(error) => {
                    log::warn!(
                        "跳过非法的 Skill 导入目录 {:?}: {error}",
                        selection.directory
                    );
                    continue;
                }
            };
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
            let mut apps = selection.apps;
            apps.pi = Self::skill_exists_in_app(&dir_name, &AppType::Pi);
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

        let db = Database::init()?;
        for repo in &index.repos {
            db.save_skill_repo(repo)?;
        }
        for skill in &imported {
            db.save_skill(skill)?;
        }
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

        let response = crate::proxy::http_client::get()
            .get(url)
            .header(reqwest::header::USER_AGENT, "cc-switch")
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
                let install_name = Self::source_install_name(&d.directory).to_lowercase();
                let installed = installed_dirs.contains(&install_name);
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
        if cache.version == SKILLS_DISCOVER_CACHE_VERSION && cache.repos_fingerprint == fingerprint
        {
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
            let install_name = Self::source_install_name(&skill.directory).to_lowercase();
            let repo_key = (
                install_name.clone(),
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
            skill.installed =
                installed_keys.contains(&repo_key) || installed_dirs.contains(&install_name);
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
            version: SKILLS_DISCOVER_CACHE_VERSION,
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
    use serial_test::serial;

    #[test]
    fn skill_state_lock_allows_snapshots_but_excludes_writers() {
        let first_reader = skill_state_read_guard();
        let second_reader = skill_state_read_guard();
        assert!(
            skill_state_lock().try_write().is_err(),
            "a Skill mutation must wait for every snapshot reader"
        );

        drop(second_reader);
        drop(first_reader);
    }

    #[test]
    fn new_install_rolls_back_database_row_when_projection_fails() {
        let temp = tempfile::tempdir().expect("create test home");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let _state_guard = skill_state_write_guard();
        let directory = "rollback-skill";
        let source = SkillService::get_ssot_dir()
            .expect("create SSOT")
            .join(directory);
        fs::create_dir_all(&source).expect("create source Skill");
        fs::write(source.join("SKILL.md"), "# managed\n").expect("write source Skill");

        let app_dir =
            SkillService::get_app_skills_dir(&AppType::Claude).expect("resolve Claude Skills");
        fs::create_dir_all(app_dir.parent().expect("Claude config directory"))
            .expect("create Claude config directory");
        fs::write(&app_dir, "not a directory\n").expect("block app Skills directory creation");

        let installed = InstalledSkill {
            id: "example/skills:rollback-skill".to_string(),
            name: "Rollback Skill".to_string(),
            description: None,
            directory: directory.to_string(),
            repo_owner: Some("example".to_string()),
            repo_name: Some("skills".to_string()),
            repo_branch: Some("main".to_string()),
            readme_url: None,
            apps: SkillApps::only(&AppType::Claude),
            installed_at: Utc::now().timestamp(),
            content_hash: None,
            updated_at: 0,
        };
        let db = Database::init().expect("init database");

        SkillService::persist_and_sync_new_skill(
            &db,
            &installed,
            &AppType::Claude,
            SyncMethod::Symlink,
        )
        .expect_err("unmanaged destination must reject projection");

        assert!(db
            .get_installed_skill(&installed.id)
            .expect("query rolled-back Skill")
            .is_none());
        assert_eq!(
            fs::read_to_string(app_dir).expect("blocking file remains"),
            "not a directory\n"
        );
    }

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

    fn poisoned_skill(id: &str, directory: &str) -> InstalledSkill {
        InstalledSkill {
            id: id.to_string(),
            name: "Poisoned".to_string(),
            description: None,
            directory: directory.to_string(),
            repo_owner: None,
            repo_name: None,
            repo_branch: None,
            readme_url: None,
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
    fn pi_deployment_hash_includes_hidden_native_changes() {
        let temp = tempfile::tempdir().expect("create Pi hash fixture");
        fs::write(temp.path().join("SKILL.md"), "managed").expect("write manifest");
        let initial =
            SkillService::compute_pi_deployment_hash(temp.path()).expect("compute Pi hash");
        fs::write(temp.path().join(".env"), "native edit").expect("write hidden native file");
        assert_ne!(
            SkillService::compute_pi_deployment_hash(temp.path()).expect("hash hidden edit"),
            initial
        );
    }

    #[test]
    #[serial]
    fn pi_remove_preserves_a_same_name_external_directory() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _environment = crate::test_support::TestEnvGuard::isolated(home.path());
        let _pi = crate::pi_config::test_support::TestAgentDir::new();
        let source = SkillService::get_ssot_dir()
            .expect("resolve SSOT")
            .join("demo");
        fs::create_dir_all(&source).expect("create managed source");
        fs::write(source.join("SKILL.md"), "managed").expect("write managed source");
        let destination = SkillService::get_app_skills_dir(&AppType::Pi)
            .expect("resolve Pi skills")
            .join("demo");
        fs::create_dir_all(&destination).expect("create external Pi skill");
        fs::write(destination.join("SKILL.md"), "external").expect("write external Pi skill");

        SkillService::remove_from_app("demo", &AppType::Pi)
            .expect_err("external Pi skill must be preserved");
        assert_eq!(
            fs::read_to_string(destination.join("SKILL.md")).expect("read preserved skill"),
            "external"
        );
    }

    #[test]
    #[serial]
    fn pi_remove_accepts_an_identical_directory() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _environment = crate::test_support::TestEnvGuard::isolated(home.path());
        let _pi = crate::pi_config::test_support::TestAgentDir::new();
        let source = SkillService::get_ssot_dir()
            .expect("resolve SSOT")
            .join("demo");
        fs::create_dir_all(&source).expect("create managed source");
        fs::write(source.join("SKILL.md"), "identical").expect("write managed source");
        let destination = SkillService::get_app_skills_dir(&AppType::Pi)
            .expect("resolve Pi skills")
            .join("demo");
        fs::create_dir_all(&destination).expect("create external Pi skill");
        fs::write(destination.join("SKILL.md"), "identical").expect("write external Pi skill");

        SkillService::remove_from_app("demo", &AppType::Pi)
            .expect("an identical Pi deployment is safe to remove");
        assert!(!destination.exists());
    }

    #[test]
    #[serial]
    fn pi_native_directory_is_reported_as_enabled() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _environment = crate::test_support::TestEnvGuard::isolated(home.path());
        let _pi = crate::pi_config::test_support::TestAgentDir::new();
        let source = SkillService::get_ssot_dir()
            .expect("resolve SSOT")
            .join("demo");
        fs::create_dir_all(&source).expect("create managed source");
        fs::write(source.join("SKILL.md"), "identical").expect("write managed source");
        let destination = SkillService::get_app_skills_dir(&AppType::Pi)
            .expect("resolve Pi skills")
            .join("demo");
        fs::create_dir_all(&destination).expect("create external Pi skill");
        fs::write(destination.join("SKILL.md"), "identical").expect("write external Pi skill");

        assert!(SkillService::skill_exists_in_app("demo", &AppType::Pi));
    }

    #[test]
    #[serial]
    fn pi_matching_copy_allows_safe_removal() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _environment = crate::test_support::TestEnvGuard::isolated(home.path());
        let _pi = crate::pi_config::test_support::TestAgentDir::new();
        let source = SkillService::get_ssot_dir()
            .expect("resolve SSOT")
            .join("demo");
        fs::create_dir_all(&source).expect("create managed source");
        fs::write(source.join("SKILL.md"), "managed").expect("write managed source");

        SkillService::sync_to_app_dir("demo", &AppType::Pi, SyncMethod::Copy)
            .expect("deploy managed copy");
        let destination = SkillService::get_app_skills_dir(&AppType::Pi)
            .expect("resolve Pi skills")
            .join("demo");
        assert_eq!(
            fs::read_to_string(destination.join("SKILL.md")).expect("read managed copy"),
            "managed"
        );

        SkillService::remove_from_app("demo", &AppType::Pi).expect("remove managed copy");
        assert!(!destination.exists());
    }

    #[test]
    #[serial]
    fn importing_a_native_pi_skill_keeps_the_matching_directory() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _environment = crate::test_support::TestEnvGuard::isolated(home.path());
        let _pi = crate::pi_config::test_support::TestAgentDir::new();
        let destination = SkillService::get_app_skills_dir(&AppType::Pi)
            .expect("resolve Pi skills")
            .join("demo");
        fs::create_dir_all(&destination).expect("create native Pi skill");
        fs::write(destination.join("SKILL.md"), "native").expect("write native Pi skill");

        let imported = SkillService::import_from_apps(vec![ImportSkillSelection {
            directory: "demo".to_string(),
            apps: SkillApps::only(&AppType::Pi),
        }])
        .expect("import native Pi skill");

        assert_eq!(imported.len(), 1);
        assert_eq!(
            fs::read_to_string(destination.join("SKILL.md")).expect("read native Pi skill"),
            "native"
        );
        let index = SkillService::load_index().expect("reload imported skill");
        assert!(index.skills["demo"].apps.pi);
    }

    #[test]
    #[serial]
    fn uninstall_preserves_a_modified_pi_copy_and_removes_managed_state() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _environment = crate::test_support::TestEnvGuard::isolated(home.path());
        let _pi = crate::pi_config::test_support::TestAgentDir::new();
        let destination = SkillService::get_app_skills_dir(&AppType::Pi)
            .expect("resolve Pi skills")
            .join("demo");
        fs::create_dir_all(&destination).expect("create native Pi skill");
        fs::write(destination.join("SKILL.md"), "native").expect("write native Pi skill");
        SkillService::import_from_apps(vec![ImportSkillSelection {
            directory: "demo".to_string(),
            apps: SkillApps::only(&AppType::Pi),
        }])
        .expect("import native Pi skill");
        fs::write(destination.join("SKILL.md"), "modified outside CC Switch")
            .expect("modify deployed Pi skill");

        SkillService::uninstall("demo").expect("uninstall managed state");

        assert!(destination.exists());
        assert!(!SkillService::get_ssot_dir()
            .expect("resolve SSOT")
            .join("demo")
            .exists());
        assert!(!SkillService::load_index()
            .expect("reload preserved index")
            .skills
            .contains_key("demo"));
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn pi_explicit_sync_method_replaces_the_managed_deployment_type() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _environment = crate::test_support::TestEnvGuard::isolated(home.path());
        let _pi = crate::pi_config::test_support::TestAgentDir::new();
        let source = SkillService::get_ssot_dir()
            .expect("resolve SSOT")
            .join("demo");
        fs::create_dir_all(&source).expect("create managed source");
        fs::write(source.join("SKILL.md"), "managed").expect("write managed source");
        let destination = SkillService::get_app_skills_dir(&AppType::Pi)
            .expect("resolve Pi skills")
            .join("demo");

        SkillService::sync_to_app_dir("demo", &AppType::Pi, SyncMethod::Copy)
            .expect("deploy managed copy");
        assert!(!SkillService::is_symlink(&destination));
        assert!(destination.join("SKILL.md").exists());

        SkillService::sync_to_app_dir("demo", &AppType::Pi, SyncMethod::Symlink)
            .expect("replace copy with symlink");
        assert!(SkillService::is_symlink(&destination));

        SkillService::sync_to_app_dir("demo", &AppType::Pi, SyncMethod::Copy)
            .expect("replace symlink with copy");
        assert!(!SkillService::is_symlink(&destination));
        assert!(destination.join("SKILL.md").exists());
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
        SkillService::sync_updated_skill_to_app("demo", &AppType::Codex, SyncMethod::Copy)
            .expect_err("an incomplete staged source should fail");
        assert_eq!(
            fs::read_to_string(deployed.join("SKILL.md")).expect("read preserved deployment"),
            "old"
        );

        fs::write(source.join("SKILL.md"), "new").expect("write replacement source");
        SkillService::sync_updated_skill_to_app("demo", &AppType::Codex, SyncMethod::Copy)
            .expect("deploy replacement");
        assert_eq!(
            fs::read_to_string(deployed.join("SKILL.md")).expect("read replaced deployment"),
            "new"
        );
    }

    #[test]
    fn copy_sync_rejects_an_incomplete_source_without_touching_the_destination() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let source = SkillService::get_ssot_dir()
            .expect("resolve isolated SSOT")
            .join("demo");
        fs::create_dir_all(&source).expect("create incomplete source");
        let app_dir = SkillService::get_app_skills_dir(&AppType::Codex)
            .expect("resolve isolated Codex Skills directory");
        let destination = app_dir.join("demo");
        fs::create_dir_all(&destination).expect("create existing destination");
        fs::write(destination.join("SKILL.md"), "old").expect("write existing destination");

        let error = SkillService::sync_to_app_dir("demo", &AppType::Codex, SyncMethod::Copy)
            .expect_err("a source without SKILL.md must not be deployed");

        assert!(error.to_string().contains("SKILL.md"), "{error}");
        assert_eq!(
            fs::read_to_string(destination.join("SKILL.md")).expect("read preserved destination"),
            "old"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_sync_keeps_the_destination_when_staging_fails() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let source = SkillService::get_ssot_dir()
            .expect("resolve isolated SSOT")
            .join("demo");
        fs::create_dir_all(&source).expect("create source");
        fs::write(source.join("SKILL.md"), "new").expect("write source manifest");
        std::os::unix::fs::symlink(source.join("missing"), source.join("broken"))
            .expect("create dangling source symlink");
        let app_dir = SkillService::get_app_skills_dir(&AppType::Codex)
            .expect("resolve isolated Codex Skills directory");
        let destination = app_dir.join("demo");
        fs::create_dir_all(&destination).expect("create existing destination");
        fs::write(destination.join("SKILL.md"), "old").expect("write existing destination");

        SkillService::sync_to_app_dir("demo", &AppType::Codex, SyncMethod::Copy)
            .expect_err("a staging copy failure must be reported");

        assert_eq!(
            fs::read_to_string(destination.join("SKILL.md")).expect("read preserved destination"),
            "old"
        );
    }

    #[test]
    fn auto_sync_refreshes_an_existing_copy_without_converting_it_to_a_symlink() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let source = SkillService::get_ssot_dir()
            .expect("resolve isolated SSOT")
            .join("demo");
        fs::create_dir_all(&source).expect("create source");
        fs::write(source.join("SKILL.md"), "new").expect("write source manifest");
        let app_dir = SkillService::get_app_skills_dir(&AppType::Codex)
            .expect("resolve isolated Codex Skills directory");
        let destination = app_dir.join("demo");
        fs::create_dir_all(&destination).expect("create existing copy");
        fs::write(destination.join("SKILL.md"), "old").expect("write existing copy");

        SkillService::sync_to_app_dir("demo", &AppType::Codex, SyncMethod::Auto)
            .expect("refresh existing copy");

        assert!(!SkillService::is_symlink(&destination));
        assert_eq!(
            fs::read_to_string(destination.join("SKILL.md")).expect("read refreshed copy"),
            "new"
        );

        fs::write(source.join("SKILL.md"), "updated").expect("update source manifest");
        SkillService::sync_updated_skill_to_app("demo", &AppType::Codex, SyncMethod::Auto)
            .expect("refresh existing copy after a Skill update");
        assert!(!SkillService::is_symlink(&destination));
        assert_eq!(
            fs::read_to_string(destination.join("SKILL.md")).expect("read updated copy"),
            "updated"
        );
    }

    #[test]
    fn copy_staging_sanitizes_nonportable_skill_names() {
        assert_eq!(
            SkillService::sanitize_backup_segment(&"💡".repeat(60)),
            "skill"
        );
        assert_eq!(
            SkillService::sanitize_backup_segment("demo skill"),
            "demo-skill"
        );
    }

    #[test]
    fn app_sync_removes_disabled_deployments_but_preserves_unmanaged_directories() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let app_dir = SkillService::get_app_skills_dir(&AppType::Codex)
            .expect("resolve isolated Codex Skills directory");
        for directory in ["disabled", "unmanaged"] {
            let path = app_dir.join(directory);
            fs::create_dir_all(&path).expect("create app Skill directory");
            fs::write(path.join("SKILL.md"), directory).expect("write app Skill manifest");
        }
        let mut index = SkillsIndex::default();
        let disabled = poisoned_skill("local:disabled", "disabled");
        index.skills.insert(disabled.directory.clone(), disabled);

        SkillService::sync_to_app(&index, &AppType::Codex).expect("reconcile app Skills");

        assert!(!app_dir.join("disabled").exists());
        assert!(app_dir.join("unmanaged").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn app_sync_removes_an_orphaned_ssot_symlink_without_touching_its_source() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let source = SkillService::get_ssot_dir()
            .expect("resolve isolated SSOT")
            .join("orphaned");
        fs::create_dir_all(&source).expect("create orphaned SSOT source");
        fs::write(source.join("SKILL.md"), "orphaned").expect("write orphaned source");
        let app_dir = SkillService::get_app_skills_dir(&AppType::Codex)
            .expect("resolve isolated Codex Skills directory");
        fs::create_dir_all(&app_dir).expect("create Codex Skills directory");
        let deployment = app_dir.join("orphaned");
        std::os::unix::fs::symlink(&source, &deployment).expect("create orphaned deployment");

        SkillService::sync_to_app(&SkillsIndex::default(), &AppType::Codex)
            .expect("reconcile orphaned deployment");

        assert!(fs::symlink_metadata(&deployment).is_err());
        assert!(source.join("SKILL.md").is_file());
    }

    #[test]
    fn migration_refresh_rejects_matching_incomplete_source_and_destination() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let old_root = home.path().join("old-skills");
        let new_root = home.path().join("new-skills");
        let source = new_root.join("demo");
        fs::create_dir_all(&source).expect("create incomplete migrated source");
        fs::write(source.join("data.txt"), "same").expect("write migrated source content");
        let app_dir = SkillService::get_app_skills_dir(&AppType::Codex)
            .expect("resolve isolated Codex Skills directory");
        let destination = app_dir.join("demo");
        fs::create_dir_all(&destination).expect("create matching deployment");
        fs::write(destination.join("data.txt"), "same").expect("write matching deployment");

        let error = SkillService::sync_migrated_skill_to_app(
            "demo",
            &AppType::Codex,
            SyncMethod::Copy,
            &old_root,
            &new_root,
        )
        .expect_err("a migrated source without SKILL.md must be rejected");

        assert!(error.to_string().contains("SKILL.md"), "{error}");
        assert_eq!(
            fs::read_to_string(destination.join("data.txt")).expect("read preserved destination"),
            "same"
        );
        assert!(source.join("data.txt").is_file());
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
    fn repository_scan_includes_root_level_skill() {
        let temp = tempfile::tempdir().expect("create repository fixture");
        fs::write(temp.path().join("SKILL.md"), "root").expect("write root manifest");
        let nested = temp.path().join("nested/other");
        fs::create_dir_all(&nested).expect("create nested skill");
        fs::write(nested.join("SKILL.md"), "nested").expect("write nested manifest");

        let discovered = SkillService::scan_skill_dirs(temp.path()).expect("scan repository");

        assert_eq!(discovered, vec![temp.path().to_path_buf()]);
    }

    #[test]
    fn install_source_prefers_exact_nested_path_for_duplicate_names() {
        let temp = tempfile::tempdir().expect("create repository fixture");
        let first = temp.path().join("first/shared");
        let expected = temp.path().join("second/shared");
        for directory in [&first, &expected] {
            fs::create_dir_all(directory).expect("create nested skill");
            fs::write(directory.join("SKILL.md"), "skill").expect("write manifest");
        }

        let resolved = SkillService::resolve_skill_source_dir(temp.path(), "second/shared")
            .expect("resolve source")
            .expect("find exact nested source");

        assert_eq!(resolved, expected);
    }

    #[test]
    fn install_source_accepts_root_level_skill() {
        let temp = tempfile::tempdir().expect("create repository fixture");
        fs::write(temp.path().join("SKILL.md"), "root").expect("write root manifest");

        let resolved = SkillService::resolve_skill_source_dir(temp.path(), "repository-name")
            .expect("resolve source")
            .expect("find root source");

        assert_eq!(resolved, temp.path());
    }

    #[test]
    fn install_source_rejects_same_name_wrapper_without_manifest() {
        let temp = tempfile::tempdir().expect("create repository fixture");
        let wrapper = temp.path().join("shared");
        fs::create_dir_all(wrapper.join("plugin")).expect("create wrapper");
        let expected = wrapper.join("skills/shared");
        fs::create_dir_all(&expected).expect("create nested skill");
        fs::write(expected.join("SKILL.md"), "skill").expect("write manifest");

        let resolved = SkillService::resolve_skill_source_dir(temp.path(), "shared")
            .expect("resolve source")
            .expect("find manifest-anchored source");

        assert_eq!(resolved, expected);
    }

    #[test]
    fn installed_directory_must_be_one_safe_path_segment() {
        assert_eq!(
            SkillService::require_valid_directory("safe-skill").expect("valid directory"),
            "safe-skill"
        );
        for invalid in [
            "..",
            "../outside",
            "nested/skill",
            "nested\\skill",
            "",
            ".hidden",
            "C:\\outside",
            "/outside",
            " safe-skill ",
        ] {
            assert!(
                SkillService::require_valid_directory(invalid).is_err(),
                "must reject {invalid:?}"
            );
        }
    }

    #[test]
    fn remove_from_app_rejects_a_traversal_directory() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let victim = home.path().join("victim-remove");
        fs::create_dir_all(&victim).expect("create victim directory");
        let app_dir = SkillService::get_app_skills_dir(&AppType::Claude)
            .expect("resolve isolated Claude Skills directory");
        fs::create_dir_all(&app_dir).expect("create Claude Skills directory");

        SkillService::remove_from_app("../../victim-remove", &AppType::Claude)
            .expect_err("traversal directory must be rejected");

        assert!(
            victim.exists(),
            "directory outside the app root must survive"
        );
    }

    #[test]
    fn uninstall_removes_a_poisoned_row_without_touching_its_path() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let victim = home.path().join("victim-uninstall");
        fs::create_dir_all(&victim).expect("create victim directory");
        let db = Database::init().expect("initialize isolated database");
        let skill = poisoned_skill("local:poisoned", "../../victim-uninstall");
        db.save_skill(&skill).expect("save poisoned Skill row");

        SkillService::uninstall(&skill.directory)
            .expect("poisoned row must remain removable through the TUI directory input");

        assert!(victim.exists(), "directory outside the SSOT must survive");
        assert!(
            db.get_installed_skill(&skill.id)
                .expect("query removed Skill")
                .is_none(),
            "poisoned database row must be deleted"
        );
    }

    #[test]
    fn uninstall_accepts_exact_empty_and_whitespace_directory_inputs() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let db = Database::init().expect("initialize isolated database");

        for (id, directory) in [("local:whitespace", " bad "), ("local:empty", "")] {
            let skill = poisoned_skill(id, directory);
            db.save_skill(&skill).expect("save poisoned Skill row");

            SkillService::uninstall(directory)
                .expect("the exact stored directory must remain removable");

            assert!(
                db.get_installed_skill(id)
                    .expect("query removed Skill")
                    .is_none(),
                "poisoned database row {id} must be deleted"
            );
        }
    }

    #[test]
    fn sync_to_app_skips_a_poisoned_row_and_syncs_a_healthy_skill() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let ssot_dir = SkillService::get_ssot_dir().expect("resolve isolated SSOT");
        let healthy_dir = ssot_dir.join("healthy");
        fs::create_dir_all(&healthy_dir).expect("create healthy Skill");
        fs::write(healthy_dir.join("SKILL.md"), "# Healthy").expect("write healthy Skill");

        let mut index = SkillsIndex::default();
        let mut poisoned = poisoned_skill("local:poisoned", "../../escape");
        poisoned.apps = SkillApps::only(&AppType::Claude);
        let mut healthy = poisoned_skill("local:healthy", "healthy");
        healthy.apps = SkillApps::only(&AppType::Claude);
        index.skills.insert(poisoned.directory.clone(), poisoned);
        index.skills.insert(healthy.directory.clone(), healthy);

        SkillService::sync_to_app(&index, &AppType::Claude)
            .expect("one poisoned row must not abort app sync");

        let app_dir = SkillService::get_app_skills_dir(&AppType::Claude)
            .expect("resolve isolated Claude Skills directory");
        assert!(
            app_dir.join("healthy").exists(),
            "the healthy Skill must still be deployed"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn install_rejects_a_traversal_directory_before_filesystem_changes() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let db = Database::init().expect("initialize isolated database");
        for mut repo in SkillStore::default().repos {
            repo.enabled = false;
            db.save_skill_repo(&repo)
                .expect("disable default repository");
        }
        let codex_root = crate::codex_config::get_codex_config_dir();
        fs::create_dir_all(&codex_root).expect("create isolated Codex root");
        let sentinel = codex_root.join("sentinel");
        fs::write(&sentinel, "preserve").expect("write Codex sentinel");

        SkillService::new()
            .expect("create Skill service")
            .install("owner/repo:..", &AppType::Codex)
            .await
            .expect_err("traversal install must be rejected");

        assert!(
            sentinel.is_file(),
            "Codex configuration must not be replaced"
        );
        assert!(
            db.get_all_installed_skills()
                .expect("list installed Skills")
                .is_empty(),
            "invalid install must not persist a Skill"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn install_rejects_case_insensitive_directory_collisions() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let db = Database::init().expect("initialize isolated database");
        for mut repo in SkillStore::default().repos {
            repo.enabled = false;
            db.save_skill_repo(&repo)
                .expect("disable default repository");
        }
        let mut existing = poisoned_skill("legacy-record", "Demo");
        existing.name = "Legacy".to_string();
        existing.directory = "Demo".to_string();
        db.save_skill(&existing).expect("save existing Skill");

        SkillService::new()
            .expect("create Skill service")
            .install("second/repo:demo", &AppType::Codex)
            .await
            .expect_err("case-insensitive directory collision must be rejected");

        assert_eq!(
            db.get_all_installed_skills()
                .expect("list installed Skills")
                .len(),
            1,
            "conflicting install must not add another record"
        );
    }

    #[test]
    fn storage_migration_rejects_a_poisoned_directory_before_moving_it() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let victim = home.path().join("victim-migrate");
        fs::create_dir_all(&victim).expect("create victim directory");
        let db = Database::init().expect("initialize isolated database");
        let skill = poisoned_skill("local:poisoned", "../../victim-migrate");
        db.save_skill(&skill).expect("save poisoned Skill row");

        SkillService::migrate_storage(SkillStorageLocation::Unified)
            .expect_err("storage migration must reject a poisoned directory");

        assert!(
            victim.exists(),
            "directory outside the SSOT must not be moved"
        );
        assert!(
            !home.path().join(".agents").join("skills").exists(),
            "migration target must not be created before directory validation"
        );
        assert!(
            !SkillService::storage_migration_journal_path().exists(),
            "migration intent must not be persisted for an invalid row"
        );
    }

    #[test]
    fn import_skips_a_traversal_selection_without_claiming_its_source() {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let victim = home.path().join("victim-import");
        fs::create_dir_all(&victim).expect("create victim directory");
        fs::write(victim.join("SKILL.md"), "# Victim").expect("write victim Skill file");

        let imported = SkillService::import_from_apps(vec![ImportSkillSelection {
            directory: "../../victim-import".to_string(),
            apps: SkillApps::only(&AppType::Claude),
        }])
        .expect("invalid import selections should be skipped");

        assert!(
            imported.is_empty(),
            "invalid selection must not be imported"
        );
        assert!(
            victim.exists(),
            "source outside the SSOT must remain untouched"
        );
        assert!(
            Database::init()
                .expect("open isolated database")
                .get_all_installed_skills()
                .expect("list installed Skills")
                .is_empty(),
            "invalid import must not create a database row"
        );
    }

    #[test]
    fn migration_tree_hash_has_unambiguous_entry_framing() {
        let temp = tempfile::tempdir().expect("create hash fixtures");
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir_all(&first).expect("create first tree");
        fs::create_dir_all(&second).expect("create second tree");
        fs::write(first.join("a"), b"X").expect("write first a");
        fs::write(first.join("b"), b"Y").expect("write first b");
        fs::write(second.join("a"), b"X\0b\0file\0Y").expect("write framed collision fixture");

        assert_ne!(
            SkillService::migration_tree_hash(&first).expect("hash first tree"),
            SkillService::migration_tree_hash(&second).expect("hash second tree")
        );
    }

    #[cfg(unix)]
    #[test]
    fn migration_tree_hash_preserves_non_utf8_names() {
        use std::os::unix::ffi::OsStringExt;

        let temp = tempfile::tempdir().expect("create hash fixtures");
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir_all(&first).expect("create first tree");
        fs::create_dir_all(&second).expect("create second tree");
        fs::write(
            first.join(std::ffi::OsString::from_vec(vec![b'a', 0x80])),
            b"same",
        )
        .expect("write first non-UTF8 file");
        fs::write(
            second.join(std::ffi::OsString::from_vec(vec![b'a', 0x81])),
            b"same",
        )
        .expect("write second non-UTF8 file");

        assert_ne!(
            SkillService::migration_tree_hash(&first).expect("hash first tree"),
            SkillService::migration_tree_hash(&second).expect("hash second tree")
        );
    }

    #[test]
    #[serial_test::serial(home_settings)]
    fn storage_migration_rejects_physically_identical_roots() {
        let temp = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        std::env::set_var("CC_SWITCH_CONFIG_DIR", temp.path().join(".agents"));
        crate::settings::reload_test_settings();

        SkillService::migrate_storage(SkillStorageLocation::Unified)
            .expect_err("the two storage choices resolve to the same root");

        assert!(!temp.path().join(".agents").join("skills").exists());
    }

    #[test]
    #[serial_test::serial(home_settings)]
    fn interrupted_storage_copy_resumes_only_with_its_persisted_marker() {
        let temp = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let old_root = SkillService::get_ssot_dir().expect("resolve old SSOT");
        let old_skill = old_root.join("managed");
        fs::create_dir_all(&old_skill).expect("create old Skill");
        fs::write(old_skill.join("SKILL.md"), "managed").expect("write old Skill");

        let db = Database::init().expect("initialize database");
        let mut skill = repository_skill(None);
        skill.id = "local:managed".to_string();
        skill.name = "Managed".to_string();
        skill.directory = "managed".to_string();
        skill.repo_owner = None;
        skill.repo_name = None;
        skill.repo_branch = None;
        db.save_skill(&skill).expect("save managed Skill");

        let new_root =
            SkillService::ssot_dir_for(SkillStorageLocation::Unified).expect("resolve target SSOT");
        fs::create_dir_all(&new_root).expect("create target root");
        let new_skill = new_root.join("managed");
        SkillService::copy_migration_tree(&old_skill, &new_skill)
            .expect("simulate completed copy before interruption");
        let journal = StorageMigrationJournal {
            source: SkillStorageLocation::CcSwitch,
            target: SkillStorageLocation::Unified,
            token: "interrupted-test-token".to_string(),
            hashes: HashMap::from([(
                "managed".to_string(),
                SkillService::migration_tree_hash(&old_skill).expect("hash source"),
            )]),
        };
        fs::write(
            new_skill.join(SkillService::migration_marker_name(&journal)),
            &journal.token,
        )
        .expect("write migration marker");
        SkillService::save_storage_migration_journal(&journal).expect("save migration journal");

        let result = SkillService::migrate_storage(SkillStorageLocation::Unified)
            .expect("resume interrupted migration");

        assert!(result.errors.is_empty());
        assert!(!old_skill.exists());
        assert!(new_skill.join("SKILL.md").is_file());
        assert!(!new_skill
            .join(SkillService::migration_marker_name(&journal))
            .exists());
        assert!(SkillService::load_storage_migration_journal()
            .expect("read cleared journal")
            .is_none());
    }

    #[test]
    #[serial_test::serial(home_settings)]
    fn unified_install_rejects_an_existing_unmanaged_directory() {
        let temp = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let mut settings = crate::settings::AppSettings::default();
        settings.skill_storage_location = SkillStorageLocation::Unified;
        crate::settings::update_settings(settings).expect("enable unified storage");
        let destination = SkillService::get_ssot_dir()
            .expect("resolve unified storage")
            .join("personal");
        fs::create_dir_all(&destination).expect("create unmanaged directory");

        SkillService::reject_unmanaged_install_collision(&destination, "personal", "owner/repo")
            .expect_err("install must not silently claim an unmanaged directory");
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
    #[serial_test::serial(home_settings)]
    fn nested_source_path_maps_to_the_installed_leaf_directory() {
        let temp = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let mut index = SkillsIndex::default();
        index.skills.clear();
        index
            .skills
            .insert("shared".to_string(), repository_skill(None));
        let remote = Skill {
            key: "owner/repo:first/shared".to_string(),
            name: "Shared".to_string(),
            description: String::new(),
            directory: "first/shared".to_string(),
            readme_url: None,
            installed: false,
            repo_owner: Some("owner".to_string()),
            repo_name: Some("repo".to_string()),
            repo_branch: Some("main".to_string()),
        };

        let mut skills = SkillService::apply_installed_state(vec![remote], &index);
        assert!(skills[0].installed);

        let ssot_skill = SkillService::get_ssot_dir()
            .expect("resolve SSOT")
            .join("shared");
        fs::create_dir_all(&ssot_skill).expect("create installed Skill");
        fs::write(ssot_skill.join("SKILL.md"), "shared").expect("write manifest");
        SkillService::merge_local_ssot_skills(&index, &mut skills).expect("merge local Skills");

        assert_eq!(skills.len(), 1);
        assert!(skills[0].installed);
        assert_eq!(skills[0].directory, "first/shared");
    }

    #[test]
    #[serial_test::serial(home_settings)]
    fn legacy_discovery_cache_is_invalidated_after_source_identity_changes() {
        let temp = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let path = SkillService::discover_cache_path();
        fs::create_dir_all(path.parent().expect("cache parent")).expect("create cache directory");
        let cache = SkillsDiscoverCache {
            version: SKILLS_DISCOVER_CACHE_VERSION - 1,
            repos_fingerprint: "owner/repo@main".to_string(),
            skills: Vec::new(),
        };
        fs::write(
            &path,
            serde_json::to_vec(&cache).expect("encode legacy cache"),
        )
        .expect("write legacy cache");

        assert!(SkillService::load_discover_cache("owner/repo@main")
            .expect("load discovery cache")
            .is_none());
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
