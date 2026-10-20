use super::*;

// Resolve trusted system temp aliases while the new download directory is
// still empty. Links within downloaded skill trees remain publication errors.
pub(super) fn download_temp_root(temp: &tempfile::TempDir) -> Result<PathBuf, AppError> {
    fs::canonicalize(temp.path()).map_err(|e| AppError::io(temp.path(), e))
}

impl SkillService {
    fn validate_github_repo_ref(owner: &str, repo: &str, branch: &str) -> Result<(), AppError> {
        let owner_valid = !owner.is_empty()
            && owner.len() <= 100
            && owner
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && !owner.starts_with('-')
            && !owner.ends_with('-');
        let repo_valid = !repo.is_empty()
            && repo.len() <= 100
            && repo != "."
            && repo != ".."
            && repo
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
        let branch_valid = !branch.is_empty()
            && branch.len() <= 1024
            && branch != "@"
            && !branch.starts_with('/')
            && !branch.ends_with('/')
            && !branch.ends_with('.')
            && !branch.contains("//")
            && !branch.contains("..")
            && !branch.contains("@{")
            && branch.split('/').all(|component| {
                !component.is_empty()
                    && component != "."
                    && component != ".."
                    && !component.ends_with(".lock")
                    && component.chars().all(|ch| {
                        !ch.is_control()
                            && !ch.is_whitespace()
                            && !matches!(ch, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
                    })
            });
        if owner_valid && repo_valid && branch_valid {
            Ok(())
        } else {
            Err(AppError::InvalidInput(format!(
                "Invalid GitHub Skill repository reference: {owner}/{repo}@{branch}"
            )))
        }
    }

    pub(super) fn github_archive_url(
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<String, AppError> {
        Self::validate_github_repo_ref(owner, repo, branch)?;
        let mut url = url::Url::parse("https://github.com/")
            .map_err(|error| AppError::Message(error.to_string()))?;
        let components = branch.split('/').collect::<Vec<_>>();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| AppError::Message("Invalid GitHub archive base URL".into()))?;
            segments.extend([owner, repo, "archive", "refs", "heads"]);
            for component in &components[..components.len() - 1] {
                segments.push(component);
            }
            segments.push(&format!("{}.zip", components[components.len() - 1]));
        }
        Ok(url.to_string())
    }

    async fn github_default_branch(&self, owner: &str, repo: &str) -> Result<String, AppError> {
        Self::validate_github_repo_ref(owner, repo, "HEAD")?;
        let mut url = url::Url::parse("https://api.github.com/")
            .map_err(|error| AppError::Message(error.to_string()))?;
        url.path_segments_mut()
            .map_err(|_| AppError::Message("Invalid GitHub API base URL".into()))?
            .extend(["repos", owner, repo]);
        let response = crate::proxy::http_client::get()
            .get(url)
            .header(reqwest::header::USER_AGENT, "cc-switch")
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|error| {
                AppError::Message(format!("Failed to resolve default branch: {error}"))
            })?;
        if !response.status().is_success() {
            return Err(AppError::Message(format!(
                "Failed to resolve default branch for {owner}/{repo}: HTTP {}",
                response.status()
            )));
        }
        #[derive(Deserialize)]
        struct RepositoryInfo {
            default_branch: String,
        }
        let info = response.json::<RepositoryInfo>().await.map_err(|error| {
            AppError::Message(format!("Invalid GitHub repository metadata: {error}"))
        })?;
        Self::validate_github_repo_ref(owner, repo, &info.default_branch)?;
        Ok(info.default_branch)
    }

    pub(super) fn merge_local_ssot_skills(
        index: &SkillsIndex,
        skills: &mut Vec<Skill>,
    ) -> Result<(), AppError> {
        let ssot = Self::get_ssot_dir()?;
        if !ssot.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(&ssot).map_err(|e| AppError::io(&ssot, e))? {
            let entry = entry.map_err(|e| AppError::io(&ssot, e))?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let directory = entry.file_name().to_string_lossy().to_string();
            if directory.starts_with('.') {
                continue;
            }

            let mut found = false;
            for skill in skills.iter_mut() {
                if Self::source_install_name(&skill.directory).eq_ignore_ascii_case(&directory) {
                    skill.installed = true;
                    found = true;
                    break;
                }
            }
            if found {
                continue;
            }

            let record = index.skills.get(&directory);
            let skill_md = path.join("SKILL.md");
            let (name, description) = if let Some(r) = record {
                (r.name.clone(), r.description.clone().unwrap_or_default())
            } else if skill_md.exists() {
                match Self::parse_skill_metadata_static(&skill_md) {
                    Ok(meta) => (
                        meta.name.unwrap_or_else(|| directory.clone()),
                        meta.description.unwrap_or_default(),
                    ),
                    Err(_) => (directory.clone(), String::new()),
                }
            } else {
                (directory.clone(), String::new())
            };

            skills.push(Skill {
                key: format!("local:{directory}"),
                name,
                description,
                directory,
                readme_url: None,
                installed: true,
                repo_owner: None,
                repo_name: None,
                repo_branch: None,
            });
        }

        Ok(())
    }

    pub(super) async fn fetch_repo_skills(
        &self,
        repo: &SkillRepo,
    ) -> Result<Vec<DiscoverableSkill>, AppError> {
        let (temp_dir, used_branch) = timeout(
            std::time::Duration::from_secs(SKILL_REPO_DOWNLOAD_TIMEOUT_SECS),
            self.download_repo(repo),
        )
        .await
        .map_err(|_| {
            AppError::Message(format_skill_error(
                "DOWNLOAD_TIMEOUT",
                &[
                    ("owner", repo.owner.as_str()),
                    ("name", repo.name.as_str()),
                    ("timeout", "660"),
                ],
                Some("checkNetwork"),
            ))
        })??;

        let mut skills = Vec::new();
        let skill_dirs = Self::scan_skill_dirs(&temp_dir)?;
        for path in skill_dirs {
            let skill_md = path.join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }

            let meta = match Self::parse_skill_metadata_static(&skill_md) {
                Ok(m) => m,
                Err(_) => SkillMetadata {
                    name: None,
                    description: None,
                },
            };

            let relative = path.strip_prefix(&temp_dir).unwrap_or(&path);
            let relative_path = relative.to_string_lossy().replace('\\', "/");
            let directory = if relative_path.is_empty() {
                repo.name.clone()
            } else {
                relative_path.clone()
            };
            if directory.is_empty() {
                continue;
            }

            let doc_path = if relative_path.is_empty() {
                "SKILL.md".to_string()
            } else {
                format!("{relative_path}/SKILL.md")
            };

            skills.push(DiscoverableSkill {
                key: format!("{}/{}:{}", repo.owner, repo.name, directory),
                name: meta.name.unwrap_or_else(|| directory.clone()),
                description: meta.description.unwrap_or_default(),
                directory,
                readme_url: Some(Self::build_skill_doc_url(
                    &repo.owner,
                    &repo.name,
                    &used_branch,
                    &doc_path,
                )),
                repo_owner: repo.owner.clone(),
                repo_name: repo.name.clone(),
                repo_branch: used_branch.clone(),
            });
        }

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(skills)
    }

    pub(super) fn deduplicate_discoverable(skills: &mut Vec<DiscoverableSkill>) {
        let mut seen: HashSet<String> = HashSet::new();
        skills.retain(|s| {
            let key = format!("{}|{}", s.repo_owner.to_lowercase(), s.key.to_lowercase());
            if seen.contains(&key) {
                false
            } else {
                seen.insert(key);
                true
            }
        });
    }

    pub(super) fn deduplicate_skills(skills: &mut Vec<Skill>) {
        let mut seen = HashSet::new();
        skills.retain(|skill| {
            let key = skill.directory.to_lowercase();
            if seen.contains(&key) {
                false
            } else {
                seen.insert(key);
                true
            }
        });
    }

    pub(super) fn build_skill_doc_url(
        owner: &str,
        repo: &str,
        branch: &str,
        doc_path: &str,
    ) -> String {
        format!("https://github.com/{owner}/{repo}/blob/{branch}/{doc_path}")
    }

    pub(super) fn read_skill_name_desc(
        skill_md: &Path,
        fallback_name: &str,
    ) -> (String, Option<String>) {
        if skill_md.exists() {
            match Self::parse_skill_metadata_static(skill_md) {
                Ok(meta) => (
                    meta.name.unwrap_or_else(|| fallback_name.to_string()),
                    meta.description,
                ),
                Err(_) => (fallback_name.to_string(), None),
            }
        } else {
            (fallback_name.to_string(), None)
        }
    }

    pub(super) fn parse_skill_metadata_static(path: &Path) -> Result<SkillMetadata, AppError> {
        let content = fs::read_to_string(path).map_err(|e| AppError::io(path, e))?;
        Ok(Self::parse_skill_metadata_content(&content))
    }

    pub(super) fn parse_skill_metadata_content(content: &str) -> SkillMetadata {
        let content = content.trim_start_matches('\u{feff}');
        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() < 3 {
            return SkillMetadata {
                name: None,
                description: None,
            };
        }
        let front_matter = parts[1].trim();
        serde_yaml::from_str(front_matter).unwrap_or(SkillMetadata {
            name: None,
            description: None,
        })
    }

    pub(super) async fn download_repo(
        &self,
        repo: &SkillRepo,
    ) -> Result<(PathBuf, String), AppError> {
        self.download_repo_with_fallback(repo, true).await
    }

    pub(super) async fn download_repo_for_update(
        &self,
        repo: &SkillRepo,
    ) -> Result<(PathBuf, String), AppError> {
        self.download_repo_with_fallback(repo, false).await
    }

    pub(super) fn branch_candidates(
        requested_branch: &str,
        resolved_default: Option<String>,
        allow_explicit_fallback: bool,
    ) -> Vec<String> {
        let branchless = requested_branch.eq_ignore_ascii_case("HEAD");
        let mut branches = Vec::new();
        if branchless {
            branches.extend(resolved_default);
        } else {
            branches.push(requested_branch.to_string());
        }
        if branchless || allow_explicit_fallback {
            for fallback in ["main", "master"] {
                if !branches.iter().any(|branch| branch == fallback) {
                    branches.push(fallback.to_string());
                }
            }
        }
        branches
    }

    async fn download_repo_with_fallback(
        &self,
        repo: &SkillRepo,
        allow_explicit_fallback: bool,
    ) -> Result<(PathBuf, String), AppError> {
        let requested_branch = if repo.branch.trim().is_empty() {
            "HEAD"
        } else {
            repo.branch.as_str()
        };
        Self::validate_github_repo_ref(&repo.owner, &repo.name, requested_branch)?;
        let temp_dir = tempfile::tempdir().map_err(|e| {
            AppError::localized(
                "skills.tempdir_failed",
                format!("创建临时目录失败: {e}"),
                format!("Failed to create temp dir: {e}"),
            )
        })?;
        let temp_path = download_temp_root(&temp_dir)?;

        let mut default_branch_error = None;
        let resolved_default = if requested_branch.eq_ignore_ascii_case("HEAD") {
            match self.github_default_branch(&repo.owner, &repo.name).await {
                Ok(branch) => Some(branch),
                Err(error) => {
                    default_branch_error = Some(error);
                    None
                }
            }
        } else {
            None
        };
        let branches =
            Self::branch_candidates(requested_branch, resolved_default, allow_explicit_fallback);

        let mut last_error = default_branch_error;
        for branch in branches {
            let url = Self::github_archive_url(&repo.owner, &repo.name, &branch)?;
            // Test-only transport seam: never compiled into the production CLI.
            #[cfg(test)]
            let url = if repo.owner == "ccswitch-slow-budget-fixture" {
                format!("http://127.0.0.1:{}/archive.zip", repo.name)
            } else {
                url
            };

            match self.download_and_extract(&url, &temp_path).await {
                Ok(()) => {
                    let _ = temp_dir.keep();
                    return Ok((temp_path, branch));
                }
                Err(e) => {
                    let _ = fs::remove_dir_all(&temp_path);
                    let _ = fs::create_dir_all(&temp_path);
                    last_error = Some(e);
                    continue;
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            AppError::Message(format_skill_error(
                "DOWNLOAD_FAILED",
                &[],
                Some("checkNetwork"),
            ))
        }))
    }

    pub(super) async fn download_and_extract(
        &self,
        url: &str,
        dest: &Path,
    ) -> Result<(), AppError> {
        let response = crate::proxy::http_client::get()
            .get(url)
            .header(reqwest::header::USER_AGENT, "cc-switch")
            .timeout(std::time::Duration::from_secs(600))
            .send()
            .await
            .map_err(|e| {
                AppError::localized(
                    "skills.download_failed",
                    format!("下载失败: {e}"),
                    format!("Download failed: {e}"),
                )
            })?;

        if !response.status().is_success() {
            let status = response.status().as_u16().to_string();
            return Err(AppError::Message(format_skill_error(
                "DOWNLOAD_FAILED",
                &[("status", status.as_str())],
                match status.as_str() {
                    "403" => Some("http403"),
                    "404" => Some("http404"),
                    "429" => Some("http429"),
                    _ => Some("checkNetwork"),
                },
            )));
        }

        if let Some(size) = response.content_length() {
            Self::check_archive_download_size(size)?;
        }

        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| {
            AppError::localized(
                "skills.download_failed",
                format!("读取下载内容失败: {e}"),
                format!("Failed to read download bytes: {e}"),
            )
        })? {
            Self::check_archive_download_size(
                (bytes.len() as u64).saturating_add(chunk.len() as u64),
            )?;
            bytes.extend_from_slice(&chunk);
        }

        let cursor = std::io::Cursor::new(bytes);
        let archive = zip::ZipArchive::new(cursor).map_err(|e| {
            AppError::localized(
                "skills.zip_invalid",
                format!("ZIP 文件损坏: {e}"),
                format!("Invalid ZIP: {e}"),
            )
        })?;

        Self::extract_repo_archive(archive, dest)
    }

    fn check_archive_download_size(size: u64) -> Result<(), AppError> {
        if size > MAX_SKILL_ARCHIVE_DOWNLOAD_BYTES {
            return Err(AppError::Message(format!(
                "Skill repository archive exceeds the {} MiB download limit",
                MAX_SKILL_ARCHIVE_DOWNLOAD_BYTES / 1024 / 1024
            )));
        }
        Ok(())
    }

    fn extract_repo_archive<R: std::io::Read + std::io::Seek>(
        archive: zip::ZipArchive<R>,
        dest: &Path,
    ) -> Result<(), AppError> {
        Self::extract_repo_archive_with_limits(
            archive,
            dest,
            MAX_SKILL_ARCHIVE_ENTRIES,
            MAX_SKILL_ARCHIVE_TOTAL_BYTES,
        )
    }

    pub(super) fn validate_archive_relative_path(path: &Path) -> Result<(), AppError> {
        let components = path.components().collect::<Vec<_>>();
        if components.is_empty()
            || components.len() > MAX_SKILL_ARCHIVE_PATH_DEPTH
            || components
                .iter()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(AppError::Message(format_skill_error(
                "INVALID_ARCHIVE_PATH",
                &[],
                Some("checkRepoUrl"),
            )));
        }
        Ok(())
    }

    fn validate_archive_entry_path(path: &Path) -> Result<(), AppError> {
        let value = path.to_string_lossy();
        let components = path.components().collect::<Vec<_>>();
        if value.contains('\\')
            || components.is_empty()
            || components
                .iter()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(AppError::Message(format_skill_error(
                "INVALID_ARCHIVE_PATH",
                &[],
                Some("checkRepoUrl"),
            )));
        }
        Ok(())
    }

    pub(super) fn charge_archive_budget(
        extracted_bytes: &mut u64,
        amount: u64,
        max_bytes: u64,
    ) -> Result<(), AppError> {
        if extracted_bytes.saturating_add(amount) > max_bytes {
            return Err(AppError::Message(format!(
                "Skill repository archive exceeds the {} MiB extraction limit",
                max_bytes / 1024 / 1024
            )));
        }
        *extracted_bytes += amount;
        Ok(())
    }

    fn create_archive_dirs(
        dest: &Path,
        relative: &Path,
        extracted_bytes: &mut u64,
        max_bytes: u64,
    ) -> Result<(), AppError> {
        if relative.as_os_str().is_empty() {
            return Ok(());
        }
        Self::validate_archive_relative_path(relative)?;
        let mut current = dest.to_path_buf();
        for component in relative.components() {
            let Component::Normal(component) = component else {
                unreachable!("validated archive path only has normal components")
            };
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => {
                    return Err(AppError::InvalidInput(format!(
                        "Archive path is not a directory: {}",
                        current.display()
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Self::charge_archive_budget(
                        extracted_bytes,
                        SKILL_ARCHIVE_ENTRY_COST,
                        max_bytes,
                    )?;
                    fs::create_dir(&current).map_err(|e| AppError::io(&current, e))?;
                }
                Err(error) => return Err(AppError::io(&current, error)),
            }
        }
        Ok(())
    }

    pub(super) fn extract_repo_archive_with_limits<R: std::io::Read + std::io::Seek>(
        mut archive: zip::ZipArchive<R>,
        dest: &Path,
        max_entries: usize,
        max_bytes: u64,
    ) -> Result<(), AppError> {
        if archive.len() > max_entries {
            return Err(AppError::Message(format!(
                "Skill repository archive has too many entries ({} > {max_entries})",
                archive.len()
            )));
        }

        let root_name = if !archive.is_empty() {
            let first_file = archive.by_index(0).map_err(|e| {
                AppError::localized(
                    "skills.zip_invalid",
                    format!("读取 ZIP 失败: {e}"),
                    format!("Failed to read ZIP: {e}"),
                )
            })?;
            let path = Path::new(first_file.name());
            Self::validate_archive_entry_path(path)?;
            path.components()
                .next()
                .and_then(|component| match component {
                    Component::Normal(name) => Some(name.to_string_lossy().to_string()),
                    _ => None,
                })
                .unwrap_or_default()
        } else {
            return Err(AppError::Message(format_skill_error(
                "EMPTY_ARCHIVE",
                &[],
                Some("checkRepoUrl"),
            )));
        };
        Self::validate_archive_relative_path(Path::new(&root_name))?;

        let mut extracted_bytes = 0u64;
        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .map_err(|e| AppError::Message(e.to_string()))?;
            let entry_path = Path::new(file.name());
            Self::validate_archive_entry_path(entry_path)?;
            let Ok(relative_path) = entry_path.strip_prefix(Path::new(&root_name)) else {
                continue;
            };
            if relative_path.as_os_str().is_empty() {
                continue;
            }
            Self::validate_archive_relative_path(relative_path)?;

            let outpath = dest.join(relative_path);
            if file.is_dir() {
                Self::create_archive_dirs(dest, relative_path, &mut extracted_bytes, max_bytes)?;
            } else {
                if let Some(parent) = relative_path.parent() {
                    Self::create_archive_dirs(dest, parent, &mut extracted_bytes, max_bytes)?;
                }
                Self::charge_archive_budget(
                    &mut extracted_bytes,
                    SKILL_ARCHIVE_ENTRY_COST,
                    max_bytes,
                )?;
                let mut outfile =
                    fs::File::create(&outpath).map_err(|e| AppError::io(&outpath, e))?;
                let mut buffer = [0u8; 16 * 1024];
                loop {
                    let read = std::io::Read::read(&mut file, &mut buffer).map_err(|e| {
                        AppError::IoContext {
                            context: format!("读取压缩文件失败: {}", outpath.display()),
                            source: e,
                        }
                    })?;
                    if read == 0 {
                        break;
                    }
                    Self::charge_archive_budget(&mut extracted_bytes, read as u64, max_bytes)?;
                    std::io::Write::write_all(&mut outfile, &buffer[..read]).map_err(|e| {
                        AppError::IoContext {
                            context: format!("写入文件失败: {}", outpath.display()),
                            source: e,
                        }
                    })?;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    // Git stores only the executable bit. Match targeted blob
                    // downloads regardless of the process umask.
                    let mode = if file.unix_mode().unwrap_or(0o644) & 0o111 != 0 {
                        0o755
                    } else {
                        0o644
                    };
                    outfile
                        .set_permissions(fs::Permissions::from_mode(mode))
                        .map_err(|e| AppError::io(&outpath, e))?;
                }
            }
        }

        Ok(())
    }

    pub(super) fn scan_skill_dirs(root: &Path) -> Result<Vec<PathBuf>, AppError> {
        let mut results = Vec::new();
        let mut stack = vec![root.to_path_buf()];

        while let Some(dir) = stack.pop() {
            // Treat directories that contain SKILL.md as a skill root, including
            // repositories whose root directory is the Skill itself.
            if dir.join("SKILL.md").is_file() {
                results.push(dir);
                continue;
            }

            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(e) => return Err(AppError::io(&dir, e)),
            };

            for entry in entries {
                let entry = entry.map_err(|e| AppError::io(&dir, e))?;
                let file_type = entry.file_type().map_err(|e| AppError::io(&dir, e))?;
                if !file_type.is_dir() {
                    continue;
                }

                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') || name == "node_modules" || name == "target" {
                    continue;
                }

                stack.push(entry.path());
            }
        }

        Ok(results)
    }

    pub(super) fn find_skill_dir_in_repo(
        root: &Path,
        directory: &str,
    ) -> Result<Option<PathBuf>, AppError> {
        let target = directory.trim();
        if target.is_empty() {
            return Ok(None);
        }

        let mut matches = Vec::new();
        for dir in Self::scan_skill_dirs(root)? {
            let name = dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.eq_ignore_ascii_case(target) {
                matches.push(dir);
            }
        }

        if matches.len() > 1 {
            log::warn!(
                "发现多个同名 skill 目录 '{target}'，将使用第一个匹配项（共 {} 个）",
                matches.len()
            );
        }

        Ok(matches.into_iter().next())
    }

    /// Resolve a repository-relative source path to a directory anchored by SKILL.md.
    pub(super) fn resolve_skill_source_dir(
        root: &Path,
        raw_directory: &str,
    ) -> Result<Option<PathBuf>, AppError> {
        let Some(source_rel) = Self::sanitize_skill_source_path(raw_directory) else {
            return Ok(None);
        };
        let Some(install_name) = source_rel
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
        else {
            return Ok(None);
        };

        let direct = root.join(&source_rel);
        if direct.is_dir() && direct.join("SKILL.md").is_file() {
            return Ok(Some(direct));
        }

        if let Some(found) = Self::find_skill_dir_in_repo(root, &install_name)? {
            log::info!(
                "Skill directory '{}' not found at direct path, using fallback: {}",
                install_name,
                found.display()
            );
            return Ok(Some(found));
        }

        if root.join("SKILL.md").is_file() {
            log::info!(
                "Skill directory '{}' not found, but SKILL.md exists at root, using repo root",
                install_name
            );
            return Ok(Some(root.to_path_buf()));
        }

        Ok(None)
    }

    pub(super) fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), AppError> {
        fs::create_dir_all(dest).map_err(|e| AppError::io(dest, e))?;
        for entry in fs::read_dir(src).map_err(|e| AppError::io(src, e))? {
            let entry = entry.map_err(|e| AppError::io(src, e))?;
            let path = entry.path();
            let dest_path = dest.join(entry.file_name());

            if path.is_dir() {
                Self::copy_dir_recursive(&path, &dest_path)?;
            } else {
                fs::copy(&path, &dest_path).map_err(|e| AppError::io(&dest_path, e))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod archive_capacity_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    #[serial_test::serial(home_settings)]
    fn download_temp_root_is_safe_for_publication_and_keeps_owner_cleanup() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let temp = tempfile::tempdir().unwrap();
        let root = download_temp_root(&temp).unwrap();
        assert_eq!(root, fs::canonicalize(temp.path()).unwrap());
        fs::write(root.join("SKILL.md"), b"# Downloaded skill").unwrap();
        assert!(local::local_tree_hash(&root).is_ok());
        drop(temp);
        assert!(!root.exists());
    }

    fn read_request(stream: &mut std::net::TcpStream) {
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 2048];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0 && request.len() < 16384);
            request.extend_from_slice(&buffer[..count]);
        }
    }

    fn directory_archive(entries: usize) -> zip::ZipArchive<std::io::Cursor<Vec<u8>>> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.add_directory("repo-main/", options).unwrap();
        for index in 1..entries {
            writer
                .add_directory(format!("repo-main/d{index}/"), options)
                .unwrap();
        }
        zip::ZipArchive::new(std::io::Cursor::new(writer.finish().unwrap().into_inner())).unwrap()
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial(home_settings)]
    async fn archive_capacity_update_check_respects_git_executable_modes() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let owner = "ccswitch-slow-budget-fixture";
        let source = SkillService::get_ssot_dir().unwrap().join("shared");
        fs::create_dir_all(&source).unwrap();
        for (name, bytes, mode) in [
            ("SKILL.md", "# shared\n", 0o644),
            ("run.sh", "#!/bin/sh\nexit 0\n", 0o755),
        ] {
            fs::write(source.join(name), bytes).unwrap();
            fs::set_permissions(source.join(name), fs::Permissions::from_mode(mode)).unwrap();
        }
        let db = Database::init().unwrap();
        db.save_skill(&InstalledSkill {
            id: format!("{owner}/{port}:shared"),
            name: "Shared".to_string(),
            description: None,
            directory: "shared".to_string(),
            repo_owner: Some(owner.to_string()),
            repo_name: Some(port.to_string()),
            repo_branch: Some("main".to_string()),
            readme_url: Some(format!(
                "https://github.com/{owner}/{port}/blob/main/shared/SKILL.md"
            )),
            apps: SkillApps::default(),
            installed_at: 0,
            content_hash: Some(local::local_tree_hash(&source).unwrap()),
            updated_at: 0,
        })
        .unwrap();
        let server = std::thread::spawn(move || {
            for (content, mode) in [
                ("# shared\n", 0o755),
                ("# changed\n", 0o755),
                ("# shared\n", 0o644),
            ] {
                let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
                for (name, bytes, permissions) in [
                    ("SKILL.md", content, 0o644),
                    ("run.sh", "#!/bin/sh\nexit 0\n", mode),
                ] {
                    writer
                        .start_file(
                            format!("repo-main/shared/{name}"),
                            zip::write::SimpleFileOptions::default().unix_permissions(permissions),
                        )
                        .unwrap();
                    writer.write_all(bytes.as_bytes()).unwrap();
                }
                let payload = writer.finish().unwrap().into_inner();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(std::time::Instant::now() < deadline);
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Err(e) => panic!("accept update check: {e}"),
                    }
                };
                read_request(&mut stream);
                stream
                    .set_write_timeout(Some(std::time::Duration::from_secs(10)))
                    .unwrap();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                )
                .unwrap();
                stream.write_all(&payload).unwrap();
            }
        });
        let service = SkillService::new().unwrap();
        for expected_updates in [0, 1, 1] {
            let result = service.check_updates().await.unwrap();
            assert!(result.failures.is_empty(), "{:?}", result.failures);
            assert_eq!(result.updates.len(), expected_updates);
        }
        server.join().unwrap();
    }

    #[test]
    fn archive_capacity_download_size_boundaries() {
        assert_eq!(MAX_SKILL_ARCHIVE_DOWNLOAD_BYTES, 256 * 1024 * 1024);
        for size in [
            128 * 1024 * 1024,
            129 * 1024 * 1024,
            MAX_SKILL_ARCHIVE_DOWNLOAD_BYTES,
        ] {
            SkillService::check_archive_download_size(size).unwrap();
        }
        for size in [MAX_SKILL_ARCHIVE_DOWNLOAD_BYTES + 1, u64::MAX] {
            assert!(SkillService::check_archive_download_size(size)
                .unwrap_err()
                .to_string()
                .contains("256 MiB download limit"));
        }
    }

    #[test]
    fn archive_capacity_accepts_more_than_ten_thousand_entries() {
        assert_eq!(MAX_SKILL_ARCHIVE_ENTRIES, 20_000);
        let dest = tempfile::tempdir().unwrap();
        SkillService::extract_repo_archive(directory_archive(10_001), dest.path()).unwrap();
        assert!(dest.path().join("d10000").is_dir());
    }

    #[test]
    fn archive_capacity_rejects_more_than_twenty_thousand_entries_before_writes() {
        let dest = tempfile::tempdir().unwrap();
        let error =
            SkillService::extract_repo_archive(directory_archive(20_001), dest.path()).unwrap_err();
        assert!(error.to_string().contains("20001 > 20000"));
        assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn archive_capacity_downloads_above_old_limit() {
        let fixture = tempfile::tempdir().unwrap();
        let archive_path = fixture.path().join("large.zip");
        let file = fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.add_directory("repo-main/", options).unwrap();
        writer.start_file("repo-main/payload.bin", options).unwrap();
        let chunk = vec![0x5a_u8; 1024 * 1024];
        for _ in 0..129 {
            writer.write_all(&chunk).unwrap();
        }
        drop(writer.finish().unwrap());
        let length = fs::metadata(&archive_path).unwrap().len();
        assert!(length > 128 * 1024 * 1024 && length < MAX_SKILL_ARCHIVE_DOWNLOAD_BYTES);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            stream
                .set_write_timeout(Some(std::time::Duration::from_secs(30)))
                .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            std::io::copy(&mut fs::File::open(archive_path).unwrap(), &mut stream).ok();
        });
        let dest = tempfile::tempdir().unwrap();
        let result = SkillService::new()
            .unwrap()
            .download_and_extract(&format!("http://{address}/archive.zip"), dest.path())
            .await;
        server.join().unwrap();
        result.unwrap();
        assert_eq!(
            fs::metadata(dest.path().join("payload.bin")).unwrap().len(),
            129 * 1024 * 1024
        );
    }

    #[tokio::test]
    async fn archive_capacity_rejects_oversize_content_length() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            let length = MAX_SKILL_ARCHIVE_DOWNLOAD_BYTES + 1;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        });
        let dest = tempfile::tempdir().unwrap();
        let error = SkillService::new()
            .unwrap()
            .download_and_extract(&format!("http://{address}/archive.zip"), dest.path())
            .await
            .unwrap_err();
        server.join().unwrap();
        assert!(error.to_string().contains("256 MiB download limit"));
        assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn archive_capacity_rejects_oversize_stream_without_content_length() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            stream
                .set_write_timeout(Some(std::time::Duration::from_secs(30)))
                .unwrap();
            write!(stream, "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n").unwrap();
            let chunk = vec![0_u8; 1024 * 1024];
            for _ in 0..257 {
                if stream.write_all(&chunk).is_err() {
                    break;
                }
            }
        });
        let dest = tempfile::tempdir().unwrap();
        let error = SkillService::new()
            .unwrap()
            .download_and_extract(&format!("http://{address}/archive.zip"), dest.path())
            .await
            .unwrap_err();
        server.join().unwrap();
        assert!(error.to_string().contains("256 MiB download limit"));
        assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 0);
    }
}

#[cfg(test)]
mod large_archive_timeout_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn slow_repository_budget_regression() {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.add_directory("repo-main/", options).unwrap();
        writer.start_file("repo-main/SKILL.md", options).unwrap();
        writer.write_all(b"---\nname: slow-fixture\ndescription: Local archive timeout fixture\n---\n# Fixture\n").unwrap();
        let payload = writer.finish().unwrap().into_inner();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2048];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0 && request.len() < 16384);
                request.extend_from_slice(&buffer[..count]);
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            )
            .unwrap();
            let split = payload.len() / 2;
            stream.write_all(&payload[..split]).unwrap();
            stream.flush().unwrap();
            std::thread::sleep(Duration::from_secs(65));
            let _ = stream.write_all(&payload[split..]);
        });
        let repo = SkillRepo {
            owner: "ccswitch-slow-budget-fixture".to_string(),
            name: port.to_string(),
            branch: "main".to_string(),
            enabled: true,
        };
        let started = Instant::now();
        let service = SkillService::new().unwrap();
        let result = timeout(
            std::time::Duration::from_secs(SKILL_REPO_DOWNLOAD_TIMEOUT_SECS),
            service.download_repo_for_update(&repo),
        )
        .await;
        let elapsed = started.elapsed().as_secs();
        println!("[slow-fixture] elapsed_seconds={elapsed}");
        server.join().unwrap();
        assert!(
            matches!(&result, Ok(Ok(_))),
            "repository request failed after {elapsed}s: {result:?}"
        );
        let (path, branch) = result.unwrap().unwrap();
        let downloaded = DownloadedRepoGuard::new(path);
        assert_eq!(branch, "main");
        assert!(fs::read_to_string(downloaded.path().join("SKILL.md"))
            .unwrap()
            .contains("name: slow-fixture"));
    }
}
