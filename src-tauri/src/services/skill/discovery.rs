use super::*;
use reqwest::header::{HeaderValue, ACCEPT};

impl SkillService {
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
                if skill.directory.eq_ignore_ascii_case(&directory) {
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
        let temp_dir = timeout(std::time::Duration::from_secs(60), self.download_repo(repo))
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

            let directory = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if directory.is_empty() {
                continue;
            }

            let relative = path.strip_prefix(&temp_dir).unwrap_or(&path);
            let relative_path = relative.to_string_lossy().replace('\\', "/");
            let readme_path = if relative_path.trim().is_empty() {
                directory.clone()
            } else {
                relative_path
            };

            skills.push(DiscoverableSkill {
                key: format!("{}/{}:{}", repo.owner, repo.name, directory),
                name: meta.name.unwrap_or_else(|| directory.clone()),
                description: meta.description.unwrap_or_default(),
                directory,
                readme_url: Some(format!(
                    "https://github.com/{}/{}/tree/{}/{}",
                    repo.owner, repo.name, repo.branch, readme_path
                )),
                repo_owner: repo.owner.clone(),
                repo_name: repo.name.clone(),
                repo_branch: repo.branch.clone(),
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
        let content = content.trim_start_matches('\u{feff}');
        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() < 3 {
            return Ok(SkillMetadata {
                name: None,
                description: None,
            });
        }
        let front_matter = parts[1].trim();
        let meta: SkillMetadata = serde_yaml::from_str(front_matter).unwrap_or(SkillMetadata {
            name: None,
            description: None,
        });
        Ok(meta)
    }

    pub(super) async fn download_repo(&self, repo: &SkillRepo) -> Result<PathBuf, AppError> {
        let temp_dir = tempfile::tempdir().map_err(|e| {
            AppError::localized(
                "skills.tempdir_failed",
                format!("创建临时目录失败: {e}"),
                format!("Failed to create temp dir: {e}"),
            )
        })?;
        let temp_path = temp_dir.path().to_path_buf();
        let _ = temp_dir.keep();

        let branches = if repo.branch.trim().is_empty() {
            vec!["main", "master"]
        } else {
            vec![repo.branch.as_str(), "main", "master"]
        };

        let mut last_error: Option<AppError> = None;
        for branch in branches {
            match self.download_and_extract(repo, branch, &temp_path).await {
                Ok(()) => return Ok(temp_path),
                Err(e) => {
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
        repo: &SkillRepo,
        branch: &str,
        dest: &Path,
    ) -> Result<(), AppError> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/zipball/{}",
            repo.owner, repo.name, branch
        );

        let mut request = self
            .http_client
            .get(&url)
            .header(
                ACCEPT,
                HeaderValue::from_static("application/vnd.github+json"),
            )
            .header(
                "X-GitHub-Api-Version",
                HeaderValue::from_static("2022-11-28"),
            );

        if let Some(token) = Self::resolve_repo_token(repo)? {
            request = request.bearer_auth(token);
        }

        let response = request.send().await.map_err(|e| {
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

        let bytes = response.bytes().await.map_err(|e| {
            AppError::localized(
                "skills.download_failed",
                format!("读取下载内容失败: {e}"),
                format!("Failed to read download bytes: {e}"),
            )
        })?;

        let cursor = std::io::Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor).map_err(|e| {
            AppError::localized(
                "skills.zip_invalid",
                format!("ZIP 文件损坏: {e}"),
                format!("Invalid ZIP: {e}"),
            )
        })?;

        let root_name = if !archive.is_empty() {
            let first_file = archive.by_index(0).map_err(|e| {
                AppError::localized(
                    "skills.zip_invalid",
                    format!("读取 ZIP 失败: {e}"),
                    format!("Failed to read ZIP: {e}"),
                )
            })?;
            let name = first_file.name();
            name.split('/').next().unwrap_or("").to_string()
        } else {
            return Err(AppError::Message(format_skill_error(
                "EMPTY_ARCHIVE",
                &[],
                Some("checkRepoUrl"),
            )));
        };

        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .map_err(|e| AppError::Message(e.to_string()))?;
            let file_path = file.name();

            let relative_path =
                if let Some(stripped) = file_path.strip_prefix(&format!("{root_name}/")) {
                    stripped
                } else {
                    continue;
                };
            if relative_path.is_empty() {
                continue;
            }

            let outpath = dest.join(relative_path);
            if file.is_dir() {
                fs::create_dir_all(&outpath).map_err(|e| AppError::io(&outpath, e))?;
            } else {
                if let Some(parent) = outpath.parent() {
                    fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
                }
                let mut outfile =
                    fs::File::create(&outpath).map_err(|e| AppError::io(&outpath, e))?;
                std::io::copy(&mut file, &mut outfile).map_err(|e| AppError::IoContext {
                    context: format!("写入文件失败: {}", outpath.display()),
                    source: e,
                })?;
            }
        }

        Ok(())
    }

    fn resolve_repo_token(repo: &SkillRepo) -> Result<Option<String>, AppError> {
        let Some(token_env) = repo
            .token_env
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };

        let token = std::env::var(token_env).map_err(|_| {
            AppError::InvalidInput(format!(
                "仓库 {}/{} 需要环境变量 {}，但当前未设置",
                repo.owner, repo.name, token_env
            ))
        })?;
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(AppError::InvalidInput(format!(
                "环境变量 {} 已设置，但内容为空",
                token_env
            )));
        }

        Ok(Some(token))
    }

    pub(super) fn scan_skill_dirs(root: &Path) -> Result<Vec<PathBuf>, AppError> {
        let mut results = Vec::new();
        let mut stack = vec![root.to_path_buf()];

        while let Some(dir) = stack.pop() {
            // Treat directories that contain SKILL.md as a skill root.
            // Do not treat the repo root itself as a skill to avoid random temp dir names.
            if dir != root && dir.join("SKILL.md").exists() {
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
