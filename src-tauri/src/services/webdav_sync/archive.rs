//! Skills ZIP packing and bounded extraction into caller-owned staging.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};

use zip::{write::SimpleFileOptions, DateTime};

use crate::error::AppError;
use crate::services::skill::SkillService;
use crate::skill_directory::SkillDirectory;

const MAX_ZIP_ENTRIES: usize = 10_000;
const MAX_ZIP_EXTRACT_BYTES: u64 = 512 * 1024 * 1024; // 512 MB

fn localized(key: &'static str, zh: impl Into<String>, en: impl Into<String>) -> AppError {
    AppError::localized(key, zh, en)
}

// ---------------------------------------------------------------------------
// ZIP 打包
// ---------------------------------------------------------------------------

pub(crate) fn zip_skills_ssot(dest_path: &Path) -> Result<BTreeSet<String>, AppError> {
    let source = SkillService::get_ssot_dir()?;
    zip_skills_directory(&source, dest_path)
}

fn zip_skills_directory(source: &Path, dest_path: &Path) -> Result<BTreeSet<String>, AppError> {
    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let file = fs::File::create(dest_path).map_err(|e| AppError::io(dest_path, e))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip_file_options();
    let result = pack_skills_root(source, &mut writer, options);
    let directories = match result {
        Ok(directories) => directories,
        Err(error) => {
            drop(writer);
            let _ = fs::remove_file(dest_path);
            return Err(error);
        }
    };

    writer.finish().map_err(|e| {
        localized(
            "webdav.sync.skills_zip_write_failed",
            format!("写入 skills.zip 失败: {e}"),
            format!("Failed to write skills.zip: {e}"),
        )
    })?;
    Ok(directories)
}

pub(crate) fn zip_file_options() -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(DateTime::default())
}

#[derive(Default)]
struct SkillsZipPackingState {
    entries: usize,
    uncompressed_bytes: u64,
    collision_paths: HashSet<String>,
}

fn pack_skills_root(
    source: &Path,
    writer: &mut zip::ZipWriter<fs::File>,
    options: SimpleFileOptions,
) -> Result<BTreeSet<String>, AppError> {
    let source_metadata =
        fs::symlink_metadata(source).map_err(|error| AppError::io(source, error))?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "Skills source must be a plain directory: {}",
            source.display()
        )));
    }

    let mut entries = sorted_directory_entries(source)?;
    let mut directories = BTreeSet::new();
    let mut collision_directories = BTreeMap::<String, String>::new();
    let mut state = SkillsZipPackingState::default();
    for entry in entries.drain(..) {
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            AppError::InvalidInput(format!(
                "Skills source contains a non-UTF-8 directory: {}",
                entry.path().display()
            ))
        })?;
        if name.starts_with('.') {
            continue;
        }
        let directory = SkillDirectory::parse(name).map_err(|error| {
            AppError::InvalidInput(format!(
                "invalid portable Skill directory {name:?}: {error}"
            ))
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| AppError::io(&path, error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(AppError::InvalidInput(format!(
                "Skills root entries must be plain directories: {}",
                path.display()
            )));
        }

        let collision_key = directory.collision_key();
        if let Some(existing) =
            collision_directories.insert(collision_key, directory.as_str().to_string())
        {
            return Err(AppError::InvalidInput(format!(
                "Skills source contains normalization-colliding directories {existing:?} and {:?}",
                directory.as_str()
            )));
        }

        let relative = PathBuf::from(directory.as_str());
        let relative_zip = record_portable_zip_entry(&relative, &mut state)?;
        writer
            .add_directory(format!("{relative_zip}/"), options)
            .map_err(|error| zip_write_error("directory", error))?;
        let files = zip_dir_recursive(&path, &relative, writer, options, &mut state)?;
        if files == 0 {
            return Err(AppError::InvalidInput(format!(
                "Skill directory {:?} contains no file payload",
                directory.as_str()
            )));
        }
        directories.insert(directory.as_str().to_string());
    }
    Ok(directories)
}

fn zip_dir_recursive(
    current: &Path,
    relative_current: &Path,
    writer: &mut zip::ZipWriter<fs::File>,
    options: SimpleFileOptions,
    state: &mut SkillsZipPackingState,
) -> Result<u64, AppError> {
    let mut entries = sorted_directory_entries(current)?;
    let mut files = 0u64;

    for entry in entries.drain(..) {
        let path = entry.path();
        let name = entry.file_name();
        let name_text = name.to_str().ok_or_else(|| {
            AppError::InvalidInput(format!(
                "Skills source contains a non-UTF-8 path: {}",
                path.display()
            ))
        })?;
        if name_text.starts_with('.') {
            continue;
        }
        SkillDirectory::parse(name_text).map_err(|error| {
            AppError::InvalidInput(format!(
                "invalid portable Skills path component {name_text:?}: {error}"
            ))
        })?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| AppError::io(&path, error))?;
        if metadata.file_type().is_symlink() {
            return Err(AppError::InvalidInput(format!(
                "Skills source contains a symbolic link: {}",
                path.display()
            )));
        }

        let relative = relative_current.join(&name);
        let relative_zip = record_portable_zip_entry(&relative, state)?;
        if metadata.is_dir() {
            writer
                .add_directory(format!("{relative_zip}/"), options)
                .map_err(|error| zip_write_error("directory", error))?;
            files =
                files.saturating_add(zip_dir_recursive(&path, &relative, writer, options, state)?);
            continue;
        }
        if !metadata.is_file() {
            return Err(AppError::InvalidInput(format!(
                "Skills source contains a non-regular file: {}",
                path.display()
            )));
        }

        let mut source_file = open_regular_file_no_follow(&path)?;
        let source_size = source_file
            .metadata()
            .map_err(|error| AppError::io(&path, error))?
            .len();
        if state.uncompressed_bytes.saturating_add(source_size) > MAX_ZIP_EXTRACT_BYTES {
            return Err(localized(
                "webdav.sync.skills_zip_too_large",
                "Skills 文件总大小超过同步上限",
                "Skills files exceed the sync size limit",
            ));
        }
        writer
            .start_file(&relative_zip, options)
            .map_err(|error| zip_write_error("file", error))?;
        let mut bounded = (&mut source_file).take(source_size.saturating_add(1));
        let copied = io::copy(&mut bounded, writer).map_err(|error| AppError::io(&path, error))?;
        if copied != source_size {
            return Err(AppError::InvalidInput(format!(
                "Skill file changed while the snapshot was being created: {}",
                path.display()
            )));
        }
        state.uncompressed_bytes = state.uncompressed_bytes.saturating_add(copied);
        files = files.saturating_add(1);
    }
    Ok(files)
}

fn sorted_directory_entries(path: &Path) -> Result<Vec<fs::DirEntry>, AppError> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| AppError::io(path, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AppError::io(path, error))?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn record_portable_zip_entry(
    relative: &Path,
    state: &mut SkillsZipPackingState,
) -> Result<String, AppError> {
    let mut components = Vec::new();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(AppError::InvalidInput(format!(
                "Skills source contains an unsafe relative path: {}",
                relative.display()
            )));
        };
        let component = component.to_str().ok_or_else(|| {
            AppError::InvalidInput(format!(
                "Skills source contains a non-UTF-8 path: {}",
                relative.display()
            ))
        })?;
        let component = SkillDirectory::parse(component).map_err(|error| {
            AppError::InvalidInput(format!(
                "invalid portable Skills path component {component:?}: {error}"
            ))
        })?;
        components.push(component);
    }
    let collision_path = components
        .iter()
        .map(SkillDirectory::collision_key)
        .collect::<Vec<_>>()
        .join("/");
    if !state.collision_paths.insert(collision_path) {
        return Err(AppError::InvalidInput(format!(
            "Skills source contains a duplicate or normalization-colliding path: {}",
            relative.display()
        )));
    }
    state.entries = state.entries.saturating_add(1);
    if state.entries > MAX_ZIP_ENTRIES {
        return Err(localized(
            "webdav.sync.skills_zip_too_many_entries",
            format!("Skills 文件条目数超过上限 {MAX_ZIP_ENTRIES}"),
            format!("Skills file entry count exceeds the limit {MAX_ZIP_ENTRIES}"),
        ));
    }
    Ok(components
        .iter()
        .map(SkillDirectory::as_str)
        .collect::<Vec<_>>()
        .join("/"))
}

fn open_regular_file_no_follow(path: &Path) -> Result<fs::File, AppError> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| AppError::io(path, error))?;
    let metadata = file.metadata().map_err(|error| AppError::io(path, error))?;
    if !metadata.is_file() {
        return Err(AppError::InvalidInput(format!(
            "Skills source entry is not a regular file: {}",
            path.display()
        )));
    }
    Ok(file)
}

fn zip_write_error(kind: &str, error: zip::result::ZipError) -> AppError {
    localized(
        "webdav.sync.skills_zip_write_failed",
        format!("写入 Skills ZIP {kind} 失败: {error}"),
        format!("Failed to write Skills ZIP {kind}: {error}"),
    )
}

// ---------------------------------------------------------------------------
// ZIP extraction into same-volume restore staging
// ---------------------------------------------------------------------------

pub(crate) fn extract_skills_zip_into(
    raw: &[u8],
    destination: &Path,
) -> Result<BTreeSet<String>, AppError> {
    if destination.exists() {
        return Err(AppError::InvalidInput(format!(
            "Skills restore staging already exists: {}",
            destination.display()
        )));
    }
    create_private_staging_directory(destination)?;
    let result = extract_skills_zip_into_inner(raw, destination);
    if result.is_err() {
        if let Err(cleanup_error) = fs::remove_dir_all(destination) {
            log::warn!(
                "failed to remove rejected Skills staging {}: {}",
                destination.display(),
                cleanup_error
            );
        }
    }
    result
}

fn create_private_staging_directory(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(path)
            .map_err(|error| AppError::io(path, error))
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path).map_err(|error| AppError::io(path, error))
    }
}

fn extract_skills_zip_into_inner(
    raw: &[u8],
    destination: &Path,
) -> Result<BTreeSet<String>, AppError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(raw)).map_err(|e| {
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

    let mut total_bytes: u64 = 0;
    let mut relative_paths = HashSet::new();
    let mut top_level: BTreeMap<String, (String, u64)> = BTreeMap::new();
    for idx in 0..archive.len() {
        let mut entry = archive.by_index(idx).map_err(|e| {
            localized(
                "webdav.sync.skills_zip_entry_read_failed",
                format!("读取 ZIP 项失败: {e}"),
                format!("Failed to read ZIP entry: {e}"),
            )
        })?;
        let Some(safe_name) = entry.enclosed_name() else {
            return Err(localized(
                "webdav.sync.skills_zip_unsafe_entry",
                format!("skills.zip 包含不安全路径项: {}", entry.name()),
                format!("skills.zip contains an unsafe path entry: {}", entry.name()),
            ));
        };
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170_000 == 0o120_000)
        {
            return Err(localized(
                "webdav.sync.skills_zip_symlink_entry",
                format!("skills.zip 包含符号链接项: {}", entry.name()),
                format!(
                    "skills.zip contains a symbolic-link entry: {}",
                    entry.name()
                ),
            ));
        }

        let mut components = Vec::new();
        for component in safe_name.components() {
            let std::path::Component::Normal(component) = component else {
                return Err(localized(
                    "webdav.sync.skills_zip_unsafe_entry",
                    format!("skills.zip 包含不安全路径项: {}", entry.name()),
                    format!("skills.zip contains an unsafe path entry: {}", entry.name()),
                ));
            };
            let component = component.to_str().ok_or_else(|| {
                localized(
                    "webdav.sync.skills_zip_non_utf8_entry",
                    "skills.zip 包含非 UTF-8 路径项",
                    "skills.zip contains a non-UTF-8 path entry",
                )
            })?;
            let component = SkillDirectory::parse(component).map_err(|error| {
                AppError::InvalidInput(format!(
                    "invalid portable Skills ZIP component {component:?}: {error}"
                ))
            })?;
            components.push(component);
        }
        let Some(skill_directory) = components.first() else {
            return Err(AppError::InvalidInput(
                "skills.zip contains an empty path".to_string(),
            ));
        };

        let collision_path = components
            .iter()
            .map(SkillDirectory::collision_key)
            .collect::<Vec<_>>()
            .join("/");
        if !relative_paths.insert(collision_path) {
            return Err(AppError::InvalidInput(format!(
                "skills.zip contains a duplicate or normalization-colliding path: {}",
                entry.name()
            )));
        }

        let skill_key = skill_directory.collision_key();
        let skill_name = skill_directory.as_str().to_string();
        match top_level.get(&skill_key) {
            Some((existing, _)) if existing != &skill_name => {
                return Err(AppError::InvalidInput(format!(
                    "skills.zip contains normalization-colliding directories {existing:?} and {skill_name:?}"
                )));
            }
            Some(_) => {}
            None => {
                top_level.insert(skill_key.clone(), (skill_name, 0));
            }
        }

        let relative = components.iter().fold(PathBuf::new(), |path, component| {
            path.join(component.as_str())
        });
        let out_path = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| AppError::io(&out_path, e))?;
            continue;
        }
        if components.len() < 2 {
            return Err(AppError::InvalidInput(format!(
                "skills.zip file is not nested under a Skill directory: {}",
                entry.name()
            )));
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
        out.sync_all()
            .map_err(|error| AppError::io(&out_path, error))?;
        top_level
            .get_mut(&skill_key)
            .expect("top-level Skill inserted before extraction")
            .1 += 1;
    }

    for (name, files) in top_level.values() {
        if *files == 0 {
            return Err(AppError::InvalidInput(format!(
                "skills.zip contains no file payload for Skill directory {name:?}"
            )));
        }
    }
    sync_directory(destination)?;
    Ok(top_level.into_values().map(|(name, _files)| name).collect())
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

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), AppError> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| AppError::io(path, error))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), AppError> {
    Ok(())
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn zip_output_is_stable_for_same_content() {
        let tmp = tempdir().expect("create temp dir");
        let source = tmp.path().join("skills");
        fs::create_dir_all(source.join("alpha").join("nested")).expect("create source dirs");
        fs::write(source.join("alpha").join("b.txt"), b"bbb").expect("write b");
        fs::write(source.join("alpha").join("nested").join("a.txt"), b"aaa").expect("write a");

        let zip1 = tmp.path().join("first.zip");
        let zip2 = tmp.path().join("second.zip");
        let directories1 = zip_skills_directory(&source, &zip1).expect("zip source directory #1");

        std::thread::sleep(std::time::Duration::from_secs(1));

        let directories2 = zip_skills_directory(&source, &zip2).expect("zip source directory #2");

        let bytes1 = fs::read(&zip1).expect("read zip1");
        let bytes2 = fs::read(&zip2).expect("read zip2");
        assert_eq!(directories1, BTreeSet::from(["alpha".to_string()]));
        assert_eq!(directories1, directories2);
        assert_eq!(bytes1, bytes2, "zip output should be deterministic");
    }

    #[cfg(unix)]
    #[test]
    fn zip_rejects_symbolic_links_instead_of_following_or_skipping_them() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("skills");
        let skill = source.join("alpha");
        fs::create_dir_all(&skill).expect("create Skill directory");
        fs::write(skill.join("SKILL.md"), b"valid").expect("write Skill payload");
        let outside = temp.path().join("outside.txt");
        fs::write(&outside, b"outside").expect("write symlink target");
        symlink(&outside, skill.join("linked.txt")).expect("create symlink");

        let error = zip_skills_directory(&source, &temp.path().join("skills.zip"))
            .expect_err("symbolic links must reject the whole snapshot");
        assert!(error.to_string().contains("symbolic link"));
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
    fn restore_rejects_entire_zip_when_any_entry_has_no_enclosed_name() {
        let home = tempdir().expect("create isolated restore home");
        let _environment = crate::test_support::TestEnvGuard::isolated(home.path());
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        writer
            .start_file("valid/SKILL.md", zip_file_options())
            .expect("start valid entry");
        writer.write_all(b"valid").expect("write valid entry");
        writer
            .start_file("../escape.txt", zip_file_options())
            .expect("start unsafe entry");
        writer.write_all(b"escape").expect("write unsafe entry");
        let bytes = writer.finish().expect("finish hostile zip").into_inner();

        assert!(
            extract_skills_zip_into(&bytes, &home.path().join("staged-skills")).is_err(),
            "an unsafe ZIP entry must reject the whole backup"
        );
        assert!(
            !home.path().join("escape.txt").exists(),
            "hostile ZIP entry escaped the extraction root"
        );
    }
}
