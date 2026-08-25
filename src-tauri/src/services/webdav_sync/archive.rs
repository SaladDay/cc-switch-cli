//! Skills ZIP 打包 / 解压 + 备份回滚

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use tempfile::{tempdir, TempDir};
use zip::{write::SimpleFileOptions, DateTime};

use crate::config::create_managed_config_dir_all;
use crate::error::AppError;
use crate::services::skill::{SkillService, SkillStorageLocation};

const MAX_ZIP_ENTRIES: usize = 10_000;
const MAX_ZIP_EXTRACT_BYTES: u64 = 512 * 1024 * 1024; // 512 MB

fn localized(key: &'static str, zh: impl Into<String>, en: impl Into<String>) -> AppError {
    AppError::localized(key, zh, en)
}

fn io_context_localized(
    _key: &'static str,
    zh: impl Into<String>,
    en: impl Into<String>,
    source: std::io::Error,
) -> AppError {
    let zh_msg = zh.into();
    let en_msg = en.into();
    AppError::IoContext {
        context: format!("{zh_msg} ({en_msg})"),
        source,
    }
}

// ---------------------------------------------------------------------------
// Skills 备份 / 回滚
// ---------------------------------------------------------------------------

pub struct SkillsBackup {
    _tmp: TempDir,
    backup_path: PathBuf,
    ssot_dir: PathBuf,
}

impl SkillsBackup {
    pub fn backup_current_skills() -> Result<Self, AppError> {
        let ssot = SkillService::get_ssot_dir()?;
        ensure_unified_ssot_is_fully_managed(&ssot)?;
        let tmp = tempdir().map_err(|e| {
            io_context_localized(
                "webdav.sync.skills_backup_tmpdir_failed",
                "创建 skills 备份临时目录失败",
                "Failed to create temporary directory for skills backup",
                e,
            )
        })?;
        let backup_path = tmp.path().join("skills-backup");
        if ssot.exists() {
            copy_dir_recursive(&ssot, &backup_path)?;
        }
        Ok(Self {
            _tmp: tmp,
            backup_path,
            ssot_dir: ssot,
        })
    }

    pub fn restore(self) -> Result<(), AppError> {
        if self.ssot_dir.exists() {
            fs::remove_dir_all(&self.ssot_dir).map_err(|e| AppError::io(&self.ssot_dir, e))?;
        }
        if self.backup_path.exists() {
            create_managed_config_dir_all(&self.ssot_dir)?;
            copy_dir_recursive(&self.backup_path, &self.ssot_dir)?;
        }
        Ok(())
    }
}

fn unified_storage_enabled() -> bool {
    crate::settings::get_skill_storage_location() == SkillStorageLocation::Unified
}

fn ensure_unified_ssot_is_fully_managed(ssot: &Path) -> Result<(), AppError> {
    if !unified_storage_enabled() || !ssot.exists() {
        return Ok(());
    }
    let managed = SkillService::load_index_unlocked()?.skills;
    for entry in fs::read_dir(ssot).map_err(|error| AppError::io(ssot, error))? {
        let entry = entry.map_err(|error| AppError::io(ssot, error))?;
        let file_name = entry.file_name();
        let name = file_name.to_str().ok_or_else(|| {
            AppError::localized(
                "sync.unified_skills_non_utf8_name",
                format!(
                    "Unified 技能目录包含无法安全识别的名称：{}",
                    entry.path().display()
                ),
                format!(
                    "Unified Skill storage contains a name that cannot be identified safely: {}",
                    entry.path().display()
                ),
            )
        })?;
        if !managed.contains_key(name) {
            return Err(AppError::localized(
                "sync.unified_skills_contains_unmanaged",
                format!(
                    "Unified 技能目录包含未管理目录 {name}，云同步不会替换它。请先显式导入，或切回 CC Switch 存储。"
                ),
                format!(
                    "Unified Skill storage contains the unmanaged directory {name}; cloud sync will not replace it. Import it explicitly or use CC Switch storage."
                ),
            ));
        }
        SkillService::validate_managed_skill_tree(&entry.path())?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ZIP 打包
// ---------------------------------------------------------------------------

pub fn zip_skills_ssot(dest_path: &Path) -> Result<(), AppError> {
    let source = SkillService::get_ssot_dir()?;
    ensure_unified_ssot_is_fully_managed(&source)?;
    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let file = fs::File::create(dest_path).map_err(|e| AppError::io(dest_path, e))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip_file_options();

    if source.exists() {
        let canonical_root = fs::canonicalize(&source).unwrap_or_else(|_| source.clone());
        let mut visited = HashSet::new();
        mark_visited_dir(&canonical_root, &mut visited)?;
        zip_dir_recursive(
            &canonical_root,
            &canonical_root,
            &mut writer,
            options,
            &mut visited,
        )?;
    }

    writer.finish().map_err(|e| {
        localized(
            "webdav.sync.skills_zip_write_failed",
            format!("写入 skills.zip 失败: {e}"),
            format!("Failed to write skills.zip: {e}"),
        )
    })?;
    Ok(())
}

pub fn zip_file_options() -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(DateTime::default())
}

/// 记录已访问目录的 canonical path，返回 true 表示首次访问。
fn mark_visited_dir(path: &Path, visited: &mut HashSet<PathBuf>) -> Result<bool, AppError> {
    let canonical = fs::canonicalize(path).map_err(|e| AppError::io(path, e))?;
    Ok(visited.insert(canonical))
}

pub fn zip_dir_recursive(
    root: &Path,
    current: &Path,
    writer: &mut zip::ZipWriter<fs::File>,
    options: SimpleFileOptions,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), AppError> {
    let mut entries = fs::read_dir(current)
        .map_err(|e| AppError::io(current, e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::io(current, e))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // 跳过 dotfiles
        if name_str.starts_with('.') {
            continue;
        }

        // 解析符号链接，确保目标在 root 内
        let real_path = match fs::canonicalize(&path) {
            Ok(p) if p.starts_with(root) => p,
            Ok(_) => {
                log::warn!(
                    "[WebDAV] Skipping symlink outside skills root: {}",
                    path.display()
                );
                continue;
            }
            Err(_) => path.clone(),
        };

        let rel = real_path
            .strip_prefix(root)
            .or_else(|_| path.strip_prefix(root))
            .map_err(|e| {
                localized(
                    "webdav.sync.zip_relative_path_failed",
                    format!("生成 ZIP 相对路径失败: {e}"),
                    format!("Failed to build relative ZIP path: {e}"),
                )
            })?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");

        if real_path.is_dir() {
            // 跳过已访问的目录（符号链接循环检测）
            if !mark_visited_dir(&real_path, visited)? {
                log::warn!(
                    "[WebDAV] Skipping already visited directory: {}",
                    real_path.display()
                );
                continue;
            }
            writer
                .add_directory(format!("{rel_str}/"), options)
                .map_err(|e| {
                    localized(
                        "webdav.sync.zip_add_directory_failed",
                        format!("写入 ZIP 目录失败: {e}"),
                        format!("Failed to write ZIP directory entry: {e}"),
                    )
                })?;
            zip_dir_recursive(root, &real_path, writer, options, visited)?;
        } else {
            writer.start_file(&rel_str, options).map_err(|e| {
                localized(
                    "webdav.sync.zip_start_file_failed",
                    format!("写入 ZIP 文件头失败: {e}"),
                    format!("Failed to start ZIP file entry: {e}"),
                )
            })?;
            let mut f = fs::File::open(&real_path).map_err(|e| AppError::io(&real_path, e))?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)
                .map_err(|e| AppError::io(&real_path, e))?;
            writer.write_all(&buf).map_err(|e| {
                localized(
                    "webdav.sync.zip_write_file_failed",
                    format!("写入 ZIP 文件内容失败: {e}"),
                    format!("Failed to write ZIP file content: {e}"),
                )
            })?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ZIP 解压 + 恢复
// ---------------------------------------------------------------------------

pub fn restore_skills_zip(raw: &[u8]) -> Result<(), AppError> {
    let tmp = tempdir().map_err(|e| {
        io_context_localized(
            "webdav.sync.skills_extract_tmpdir_failed",
            "创建 skills 解压临时目录失败",
            "Failed to create temporary directory for skills extraction",
            e,
        )
    })?;
    let zip_path = tmp.path().join("skills.zip");
    crate::config::atomic_write(&zip_path, raw)?;

    let file = fs::File::open(&zip_path).map_err(|e| AppError::io(&zip_path, e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        localized(
            "webdav.sync.skills_zip_parse_failed",
            format!("解析 skills.zip 失败: {e}"),
            format!("Failed to parse skills.zip: {e}"),
        )
    })?;

    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(localized(
            "webdav.sync.skills_zip_too_many_entries",
            format!(
                "skills.zip 条目数过多（{}），上限 {MAX_ZIP_ENTRIES}",
                archive.len()
            ),
            format!(
                "skills.zip has too many entries ({}), limit is {MAX_ZIP_ENTRIES}",
                archive.len()
            ),
        ));
    }

    let extracted = tmp.path().join("skills-extracted");
    fs::create_dir_all(&extracted).map_err(|e| AppError::io(&extracted, e))?;

    let mut total_bytes: u64 = 0;
    for idx in 0..archive.len() {
        let mut entry = archive.by_index(idx).map_err(|e| {
            localized(
                "webdav.sync.skills_zip_entry_read_failed",
                format!("读取 ZIP 项失败: {e}"),
                format!("Failed to read ZIP entry: {e}"),
            )
        })?;
        let Some(safe_name) = entry.enclosed_name() else {
            continue;
        };
        let out_path = extracted.join(safe_name);
        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| AppError::io(&out_path, e))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }
        let mut out = fs::File::create(&out_path).map_err(|e| AppError::io(&out_path, e))?;
        let _written = copy_entry_with_total_limit(
            &mut entry,
            &mut out,
            &mut total_bytes,
            MAX_ZIP_EXTRACT_BYTES,
            &out_path,
        )?;
    }

    let ssot = SkillService::get_ssot_dir()?;
    ensure_unified_ssot_is_fully_managed(&ssot)?;
    let bak = ssot.with_extension("bak");

    if unified_storage_enabled() && bak.exists() {
        return Err(AppError::localized(
            "sync.unified_skills_backup_collision",
            format!("Unified 技能恢复不会覆盖已有备份目录：{}", bak.display()),
            format!(
                "Unified Skill restore will not replace the existing backup directory: {}",
                bak.display()
            ),
        ));
    }

    // 原子替换：先 rename 到 .bak，再 copy，失败则回滚
    if ssot.exists() {
        if bak.exists() {
            let _ = fs::remove_dir_all(&bak);
        }
        fs::rename(&ssot, &bak).map_err(|e| AppError::io(&ssot, e))?;
    }

    if let Err(e) =
        create_managed_config_dir_all(&ssot).and_then(|_| copy_dir_recursive(&extracted, &ssot))
    {
        if bak.exists() {
            let _ = fs::remove_dir_all(&ssot);
            let _ = fs::rename(&bak, &ssot);
        }
        return Err(e);
    }

    let _ = fs::remove_dir_all(&bak);
    Ok(())
}

/// 带总量限制的流式复制，在写入前检查大小是否超限。
fn copy_entry_with_total_limit(
    reader: &mut impl Read,
    writer: &mut impl Write,
    total_bytes: &mut u64,
    max_total_bytes: u64,
    out_path: &Path,
) -> Result<u64, AppError> {
    let mut buf = [0u8; 16 * 1024];
    let mut written = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| AppError::io(out_path, e))?;
        if n == 0 {
            break;
        }

        if total_bytes.saturating_add(n as u64) > max_total_bytes {
            let max_mb = max_total_bytes / 1024 / 1024;
            return Err(localized(
                "webdav.sync.skills_zip_too_large",
                format!("skills.zip 解压后体积超过上限（{max_mb} MB）"),
                format!("skills.zip extracted size exceeds limit ({max_mb} MB)"),
            ));
        }

        writer
            .write_all(&buf[..n])
            .map_err(|e| AppError::io(out_path, e))?;
        *total_bytes += n as u64;
        written += n as u64;
    }
    Ok(written)
}

// ---------------------------------------------------------------------------
// 目录递归复制
// ---------------------------------------------------------------------------

pub fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), AppError> {
    let mut visited = HashSet::new();
    copy_dir_recursive_inner(src, dest, &mut visited)
}

fn copy_dir_recursive_inner(
    src: &Path,
    dest: &Path,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), AppError> {
    if !src.exists() {
        return Ok(());
    }
    if !mark_visited_dir(src, visited)? {
        log::warn!(
            "[WebDAV] Skipping already visited copy path: {}",
            src.display()
        );
        return Ok(());
    }
    fs::create_dir_all(dest).map_err(|e| AppError::io(dest, e))?;
    for entry in fs::read_dir(src).map_err(|e| AppError::io(src, e))? {
        let entry = entry.map_err(|e| AppError::io(src, e))?;
        let path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive_inner(&path, &dest_path, visited)?;
        } else {
            fs::copy(&path, &dest_path).map_err(|e| AppError::io(&dest_path, e))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{update_settings, AppSettings};
    use tempfile::tempdir;

    #[test]
    fn zip_output_is_stable_for_same_content() {
        let tmp = tempdir().expect("create temp dir");
        let source = tmp.path().join("skills");
        fs::create_dir_all(source.join("nested")).expect("create source dirs");
        fs::write(source.join("b.txt"), b"bbb").expect("write b");
        fs::write(source.join("nested").join("a.txt"), b"aaa").expect("write a");

        let zip1 = tmp.path().join("first.zip");
        let zip2 = tmp.path().join("second.zip");

        let file1 = fs::File::create(&zip1).expect("create zip1");
        let mut writer1 = zip::ZipWriter::new(file1);
        let mut visited1 = HashSet::new();
        mark_visited_dir(&source, &mut visited1).expect("mark root");
        zip_dir_recursive(
            &source,
            &source,
            &mut writer1,
            zip_file_options(),
            &mut visited1,
        )
        .expect("zip source #1");
        writer1.finish().expect("finish zip1");

        std::thread::sleep(std::time::Duration::from_secs(1));

        let file2 = fs::File::create(&zip2).expect("create zip2");
        let mut writer2 = zip::ZipWriter::new(file2);
        let mut visited2 = HashSet::new();
        mark_visited_dir(&source, &mut visited2).expect("mark root");
        zip_dir_recursive(
            &source,
            &source,
            &mut writer2,
            zip_file_options(),
            &mut visited2,
        )
        .expect("zip source #2");
        writer2.finish().expect("finish zip2");

        let bytes1 = fs::read(&zip1).expect("read zip1");
        let bytes2 = fs::read(&zip2).expect("read zip2");
        assert_eq!(bytes1, bytes2, "zip output should be deterministic");
    }

    #[test]
    fn mark_visited_dir_tracks_canonical_duplicates() {
        let temp = tempdir().expect("tempdir");
        let dir = temp.path().join("skills");
        fs::create_dir_all(&dir).expect("create dir");

        let mut visited = HashSet::new();
        assert!(mark_visited_dir(&dir, &mut visited).expect("first visit"));
        assert!(!mark_visited_dir(&dir, &mut visited).expect("second visit"));
    }

    #[test]
    fn copy_entry_with_total_limit_rejects_oversized_stream_before_write() {
        use std::io::Cursor;
        let mut reader = Cursor::new(vec![1u8; 16]);
        let mut writer = Vec::new();
        let mut total_bytes = 0u64;

        let err = copy_entry_with_total_limit(
            &mut reader,
            &mut writer,
            &mut total_bytes,
            8,
            Path::new("skills-extracted/file.bin"),
        )
        .expect_err("stream larger than limit should be rejected");
        assert!(err.to_string().contains("超过"), "unexpected error: {err}");
        assert_eq!(
            writer.len(),
            0,
            "should not write when the first chunk exceeds limit"
        );
    }

    #[test]
    #[serial_test::serial(home_settings)]
    fn unified_archive_refuses_to_touch_unmanaged_directories() {
        let temp = tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let mut settings = AppSettings::default();
        settings.skill_storage_location = SkillStorageLocation::Unified;
        update_settings(settings).expect("enable unified Skill storage");

        let ssot = SkillService::get_ssot_dir().expect("resolve unified SSOT");
        fs::create_dir_all(ssot.join("personal")).expect("create unmanaged Skill");
        fs::write(ssot.join("personal/SKILL.md"), "personal").expect("write unmanaged Skill");

        let snapshot = temp.path().join("skills.zip");
        let error = zip_skills_ssot(&snapshot)
            .expect_err("cloud archive must not claim an unmanaged unified directory");

        assert!(error.to_string().contains("personal"));
        assert_eq!(
            fs::read_to_string(ssot.join("personal/SKILL.md"))
                .expect("read preserved unmanaged Skill"),
            "personal"
        );

        let replacement = temp.path().join("replacement.zip");
        let file = fs::File::create(&replacement).expect("create replacement archive");
        zip::ZipWriter::new(file)
            .finish()
            .expect("finish replacement archive");
        restore_skills_zip(&fs::read(&replacement).expect("read replacement archive"))
            .expect_err("cloud restore must not replace an unmanaged unified directory");
        assert_eq!(
            fs::read_to_string(ssot.join("personal/SKILL.md"))
                .expect("read preserved unmanaged Skill after restore refusal"),
            "personal"
        );

        fs::remove_dir_all(ssot.join("personal")).expect("remove unmanaged fixture");
        let sibling_backup = ssot.with_extension("bak");
        fs::create_dir_all(&sibling_backup).expect("create external sibling backup");
        fs::write(sibling_backup.join("keep.txt"), "external").expect("write external backup");
        restore_skills_zip(&fs::read(&replacement).expect("read replacement archive"))
            .expect_err("Unified restore must not remove a pre-existing sibling backup");
        assert_eq!(
            fs::read_to_string(sibling_backup.join("keep.txt"))
                .expect("read preserved sibling backup"),
            "external"
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(home_settings)]
    fn unified_archive_rejects_links_and_special_files_inside_managed_skills() {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;

        let temp = tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let mut settings = AppSettings::default();
        settings.skill_storage_location = SkillStorageLocation::Unified;
        update_settings(settings).expect("enable unified Skill storage");

        let ssot = SkillService::get_ssot_dir().expect("resolve unified SSOT");
        let managed = ssot.join("managed");
        fs::create_dir_all(&managed).expect("create managed Skill");
        fs::write(managed.join("SKILL.md"), "managed").expect("write managed Skill");
        let db = crate::database::Database::init().expect("initialize database");
        db.save_skill(&crate::app_config::InstalledSkill {
            id: "local:managed".to_string(),
            name: "Managed".to_string(),
            description: None,
            directory: "managed".to_string(),
            readme_url: None,
            repo_owner: None,
            repo_name: None,
            repo_branch: None,
            apps: crate::app_config::SkillApps::default(),
            installed_at: 0,
            content_hash: None,
            updated_at: 0,
        })
        .expect("register managed Skill");

        let external = temp.path().join("external.txt");
        fs::write(&external, "external").expect("write external fixture");
        let nested_link = managed.join("external-link");
        symlink(&external, &nested_link).expect("create nested symlink");
        let snapshot = temp.path().join("linked.zip");
        zip_skills_ssot(&snapshot).expect_err("archive must reject nested symlinks");
        assert_eq!(
            fs::read_to_string(&external).expect("read external file"),
            "external"
        );

        fs::remove_file(&nested_link).expect("remove symlink fixture");
        let socket_path = managed.join("special.sock");
        let _listener = UnixListener::bind(&socket_path).expect("create special file fixture");
        zip_skills_ssot(&snapshot).expect_err("archive must reject special files");
        assert!(socket_path.exists());
    }
}
