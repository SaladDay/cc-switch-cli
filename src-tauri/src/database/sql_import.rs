//! Bounded execution of untrusted SQL restore input.
//!
//! Imported SQL may create any data/schema inside this disposable database,
//! but it cannot escape the scratch file. Executable schema objects are
//! removed before trusted compatibility migrations run, and the scratch type
//! has no conversion into the publishable `CanonicalStage`.

use super::{Database, MigrationRunContext, SCHEMA_VERSION};
use crate::error::AppError;
use rusqlite::backup::Backup;
use rusqlite::config::DbConfig;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::{Connection, OpenFlags};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Take, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tempfile::NamedTempFile;

pub(crate) const MAX_SQL_IMPORT_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_BINARY_RESTORE_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_SCRATCH_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_SQL_VALUE_BYTES: i32 = 64 * 1024 * 1024;
const MAX_VM_STEPS: u64 = 50_000_000;
const PROGRESS_GRANULARITY: u64 = 1_000;
const MAX_PAGE_COUNT: u64 = 524_288;

#[cfg(test)]
thread_local! {
    static TEST_MAX_VM_STEPS: std::cell::Cell<Option<u64>> =
        const { std::cell::Cell::new(None) };
    static TEST_MAX_PAGE_COUNT: std::cell::Cell<Option<u64>> =
        const { std::cell::Cell::new(None) };
}

fn max_vm_steps() -> u64 {
    #[cfg(test)]
    if let Some(value) = TEST_MAX_VM_STEPS.with(std::cell::Cell::get) {
        return value;
    }
    MAX_VM_STEPS
}

fn max_page_count() -> u64 {
    #[cfg(test)]
    if let Some(value) = TEST_MAX_PAGE_COUNT.with(std::cell::Cell::get) {
        return value;
    }
    MAX_PAGE_COUNT
}

/// PRAGMAs emitted by `Database::dump_sql`. Every other PRAGMA is denied:
/// several can redirect scratch files or weaken schema protection.
const IMPORT_ALLOWED_PRAGMAS: &[&str] = &["foreign_keys", "user_version"];

/// An owned SQL batch with exactly one internal trailing NUL.
pub(super) struct SqlImportBatch {
    sql: String,
    content_start: usize,
}

impl SqlImportBatch {
    pub(super) fn from_borrowed(sql: &str) -> Result<Self, AppError> {
        if sql.len() as u64 > MAX_SQL_IMPORT_BYTES {
            return Err(oversized_import(MAX_SQL_IMPORT_BYTES));
        }
        Self::from_owned(sql.to_owned())
    }

    pub(super) fn from_owned(mut sql: String) -> Result<Self, AppError> {
        if sql.len() as u64 > MAX_SQL_IMPORT_BYTES {
            return Err(oversized_import(MAX_SQL_IMPORT_BYTES));
        }
        if sql.as_bytes().contains(&0) {
            return Err(AppError::localized(
                "backup.sql.contains_nul",
                "SQL 备份包含不受支持的 NUL 字节。",
                "The SQL backup contains an unsupported NUL byte.",
            ));
        }

        let content_start = sql.len() - sql.trim_start_matches('\u{feff}').len();
        sql.try_reserve_exact(1).map_err(|error| {
            AppError::InvalidInput(format!("unable to reserve SQL terminator: {error}"))
        })?;
        sql.push('\0');
        Ok(Self { sql, content_start })
    }

    pub(super) fn read_from_path(path: &Path) -> Result<Self, AppError> {
        Self::read_from_path_with_limit(path, MAX_SQL_IMPORT_BYTES)
    }

    fn read_from_path_with_limit(path: &Path, max_bytes: u64) -> Result<Self, AppError> {
        let bytes = read_restore_file(path, max_bytes)?;
        let sql = String::from_utf8(bytes).map_err(|error| {
            AppError::InvalidInput(format!(
                "SQL restore source is not UTF-8 ({}): {error}",
                path.display()
            ))
        })?;
        Self::from_owned_with_limit(sql, max_bytes)
    }

    fn from_owned_with_limit(mut sql: String, max_bytes: u64) -> Result<Self, AppError> {
        if sql.len() as u64 > max_bytes {
            return Err(oversized_import(max_bytes));
        }
        if sql.as_bytes().contains(&0) {
            return Err(AppError::localized(
                "backup.sql.contains_nul",
                "SQL 备份包含不受支持的 NUL 字节。",
                "The SQL backup contains an unsupported NUL byte.",
            ));
        }
        let content_start = sql.len() - sql.trim_start_matches('\u{feff}').len();
        sql.try_reserve_exact(1).map_err(|error| {
            AppError::InvalidInput(format!("unable to reserve SQL terminator: {error}"))
        })?;
        sql.push('\0');
        Ok(Self { sql, content_start })
    }

    /// SQL after leading BOMs, including the internal NUL used by SQLite's
    /// batch parser.
    pub(super) fn sql(&self) -> &str {
        &self.sql[self.content_start..]
    }

    fn content_len(&self) -> u64 {
        self.sql
            .len()
            .saturating_sub(self.content_start)
            .saturating_sub(1) as u64
    }
}

/// An untrusted schema can only exist behind this private type barrier.
pub(super) struct UntrustedScratch {
    connection: Connection,
    _file: NamedTempFile,
    cancellation: Arc<AtomicBool>,
}

impl UntrustedScratch {
    pub(super) fn from_batch(batch: &SqlImportBatch) -> Result<Self, AppError> {
        if batch.content_len() > MAX_SQL_IMPORT_BYTES {
            return Err(oversized_import(MAX_SQL_IMPORT_BYTES));
        }

        let scratch = Self::empty()?;
        scratch.connection.authorizer(Some(import_authorizer));
        let result = scratch.connection.execute_batch(batch.sql());
        scratch
            .connection
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        result.map_err(|error| {
            AppError::Database(format!("execute untrusted SQL restore: {error}"))
        })?;
        scratch.finish_input()
    }

    pub(super) fn from_binary(path: &Path) -> Result<Self, AppError> {
        let owned_source = snapshot_binary_restore_file(path, MAX_BINARY_RESTORE_BYTES)?;
        let source = Connection::open_with_flags(
            owned_source.path(),
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NOFOLLOW
                | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE,
        )
        .map_err(database_error)?;
        let mut scratch = Self::empty()?;
        {
            let backup = Backup::new(&source, &mut scratch.connection).map_err(database_error)?;
            super::backup::run_sqlite_backup_to_completion(&backup)?;
        }
        drop(source);
        drop(owned_source);
        scratch.finish_input()
    }

    pub(super) fn connection(&self) -> &Connection {
        &self.connection
    }

    fn empty() -> Result<Self, AppError> {
        let file = NamedTempFile::new().map_err(|error| AppError::IoContext {
            context: "create untrusted restore scratch".to_string(),
            source: error,
        })?;
        let connection =
            Connection::open(file.path()).map_err(|error| AppError::Database(error.to_string()))?;
        let cancellation = Arc::new(AtomicBool::new(false));
        let scratch = Self {
            connection,
            _file: file,
            cancellation,
        };
        scratch.configure_untrusted_execution()?;
        Ok(scratch)
    }

    fn configure_untrusted_execution(&self) -> Result<(), AppError> {
        self.connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)
            .map_err(database_error)?;
        self.connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)
            .map_err(database_error)?;
        self.connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
            .map_err(database_error)?;
        self.connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DDL, false)
            .map_err(database_error)?;
        self.connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML, false)
            .map_err(database_error)?;

        self.connection.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0);
        self.connection
            .set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_SQL_VALUE_BYTES);
        self.connection
            .set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, MAX_SQL_IMPORT_BYTES as i32);
        self.connection
            .set_limit(Limit::SQLITE_LIMIT_VDBE_OP, 1_000_000);
        self.connection
            .set_limit(Limit::SQLITE_LIMIT_TRIGGER_DEPTH, 0);
        self.connection
            .execute_batch(&format!(
                "PRAGMA trusted_schema = OFF;
                 PRAGMA foreign_keys = OFF;
                 PRAGMA max_page_count = {};",
                max_page_count()
            ))
            .map_err(database_error)?;

        let steps = Arc::new(AtomicU64::new(0));
        let cancellation = Arc::clone(&self.cancellation);
        let max_steps = max_vm_steps();
        self.connection.progress_handler(
            PROGRESS_GRANULARITY as i32,
            Some(move || {
                cancellation.load(Ordering::Relaxed)
                    || steps.fetch_add(PROGRESS_GRANULARITY, Ordering::Relaxed) >= max_steps
            }),
        );
        Ok(())
    }

    fn finish_input(self) -> Result<Self, AppError> {
        self.enforce_scratch_size()?;
        self.constrain_scratch_growth()?;

        let version = Database::get_user_version(&self.connection)?;
        if version > SCHEMA_VERSION {
            return Err(AppError::InvalidInput(format!(
                "restore schema version {version} is newer than supported {SCHEMA_VERSION}"
            )));
        }

        // These objects must disappear while triggers are still disabled.
        // Trusted migrations may UPDATE/DELETE candidate rows and therefore
        // must never execute a trigger supplied by the restore input.
        self.drop_untrusted_auxiliary_schema()?;
        // Source recognition is deliberately first. Never create current
        // tables here: doing so would turn a missing source table into a valid
        // empty one and destroy fail-closed version authentication.
        Database::validate_untrusted_migration_source(&self.connection)?;
        let migration_sensitive =
            super::restore_policy::capture_migration_sensitive_user_data(&self.connection)?;
        self.connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, true)
            .map_err(database_error)?;

        Database::apply_schema_migrations_on_conn(
            &self.connection,
            MigrationRunContext::UntrustedRestore,
        )?;
        super::restore_policy::restore_migration_sensitive_user_data(
            &self.connection,
            migration_sensitive,
        )?;
        self.connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(database_error)?;
        self.enforce_scratch_size()?;
        Ok(self)
    }

    fn drop_untrusted_auxiliary_schema(&self) -> Result<(), AppError> {
        for schema in ["sqlite_schema", "sqlite_temp_schema"] {
            let mut statement = self
                .connection
                .prepare(&format!(
                    "SELECT type, name FROM {schema}
                     WHERE type IN ('trigger', 'view', 'index')
                       AND sql IS NOT NULL
                       AND name NOT LIKE 'sqlite_%'
                     ORDER BY CASE type
                         WHEN 'trigger' THEN 0
                         WHEN 'view' THEN 1
                         ELSE 2
                     END, name"
                ))
                .map_err(database_error)?;
            let objects = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?;
            drop(statement);

            for (kind, name) in objects {
                let keyword = match kind.as_str() {
                    "trigger" => "TRIGGER",
                    "view" => "VIEW",
                    "index" => "INDEX",
                    _ => {
                        return Err(AppError::Database(format!(
                            "unsupported scratch object type {kind:?}"
                        )));
                    }
                };
                self.connection
                    .execute(
                        &format!("DROP {keyword} IF EXISTS {}", quote_identifier(&name)),
                        [],
                    )
                    .map_err(database_error)?;
            }
        }
        Ok(())
    }

    fn constrain_scratch_growth(&self) -> Result<(), AppError> {
        let page_size: u64 = self
            .connection
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(database_error)?;
        if page_size == 0 {
            return Err(AppError::InvalidInput(
                "restore scratch reported a zero page size".to_string(),
            ));
        }
        let byte_bounded_pages = MAX_SCRATCH_BYTES / page_size;
        let requested = max_page_count().min(byte_bounded_pages);
        if requested == 0 {
            return Err(AppError::InvalidInput(
                "restore scratch page size exceeds the byte budget".to_string(),
            ));
        }
        let applied: u64 = self
            .connection
            .query_row(&format!("PRAGMA max_page_count = {requested}"), [], |row| {
                row.get(0)
            })
            .map_err(database_error)?;
        if applied > requested {
            return Err(AppError::InvalidInput(format!(
                "restore scratch already exceeds its {requested}-page growth budget"
            )));
        }
        Ok(())
    }

    fn enforce_scratch_size(&self) -> Result<(), AppError> {
        let page_count: u64 = self
            .connection
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .map_err(database_error)?;
        let page_size: u64 = self
            .connection
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(database_error)?;
        let logical_size = page_count.saturating_mul(page_size);
        let file_size = self
            ._file
            .as_file()
            .metadata()
            .map_err(|error| AppError::io(self._file.path(), error))?
            .len();
        if logical_size > MAX_SCRATCH_BYTES || file_size > MAX_SCRATCH_BYTES {
            return Err(AppError::InvalidInput(format!(
                "restore scratch exceeds {MAX_SCRATCH_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

fn import_authorizer(context: AuthContext<'_>) -> Authorization {
    let escapes_scratch = context
        .database_name
        .is_some_and(|name| name.eq_ignore_ascii_case("temp"))
        || match context.action {
            AuthAction::Attach { .. } | AuthAction::Detach { .. } => true,
            AuthAction::CreateVtable { .. } | AuthAction::DropVtable { .. } => true,
            AuthAction::CreateTempIndex { .. }
            | AuthAction::CreateTempTable { .. }
            | AuthAction::CreateTempTrigger { .. }
            | AuthAction::CreateTempView { .. }
            | AuthAction::DropTempIndex { .. }
            | AuthAction::DropTempTable { .. }
            | AuthAction::DropTempTrigger { .. }
            | AuthAction::DropTempView { .. } => true,
            AuthAction::Function { function_name } => {
                function_name.eq_ignore_ascii_case("load_extension")
            }
            AuthAction::Unknown { .. } => true,
            AuthAction::Pragma { pragma_name, .. } => !IMPORT_ALLOWED_PRAGMAS
                .iter()
                .any(|allowed| pragma_name.eq_ignore_ascii_case(allowed)),
            _ => false,
        };

    if escapes_scratch {
        log::warn!(
            "SQL import rejected an out-of-bounds action: {:?}",
            context.action
        );
        Authorization::Deny
    } else {
        Authorization::Allow
    }
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn oversized_import(max_bytes: u64) -> AppError {
    AppError::localized(
        "backup.sql.too_large",
        format!("SQL 备份超过大小上限（{max_bytes} 字节）。"),
        format!("The SQL backup exceeds the {max_bytes}-byte limit."),
    )
}

#[cfg(unix)]
fn open_nofollow(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(windows)]
fn open_nofollow(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(all(not(unix), not(windows)))]
fn open_nofollow(path: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!(
            "nofollow restore-source opens are unsupported on this platform: {}",
            path.display()
        ),
    ))
}

#[cfg(unix)]
fn same_file_identity(left: &Metadata, right: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(unix)]
fn same_open_file_identity(left: &File, right: &File) -> std::io::Result<bool> {
    Ok(same_file_identity(&left.metadata()?, &right.metadata()?))
}

#[cfg(windows)]
fn windows_file_identity(file: &File) -> std::io::Result<(u64, [u8; 16])> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
    };

    let mut information = FILE_ID_INFO::default();
    // SAFETY: the file handle stays live and `information` is a correctly
    // sized writable output buffer.
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            std::ptr::addr_of_mut!(information).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } != 0;
    if succeeded {
        Ok((
            information.VolumeSerialNumber,
            information.FileId.Identifier,
        ))
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn same_open_file_identity(left: &File, right: &File) -> std::io::Result<bool> {
    Ok(windows_file_identity(left)? == windows_file_identity(right)?)
}

#[cfg(all(not(unix), not(windows)))]
fn same_open_file_identity(_left: &File, _right: &File) -> std::io::Result<bool> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "stable restore-source identities are unsupported on this platform",
    ))
}

#[cfg(unix)]
fn open_backup_directory(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(windows)]
fn open_backup_directory(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(all(not(unix), not(windows)))]
fn open_backup_directory(path: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!(
            "anchored backup-directory opens are unsupported on this platform: {}",
            path.display()
        ),
    ))
}

#[cfg(unix)]
fn open_backup_child(
    directory: &File,
    _directory_path: &Path,
    filename: &std::ffi::OsStr,
) -> std::io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let filename = std::ffi::CString::new(filename.as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            filename.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        // SAFETY: openat returned a new owned descriptor; ownership moves to File.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

#[cfg(windows)]
fn open_backup_child(
    directory: &File,
    _directory_path: &Path,
    filename: &std::ffi::OsStr,
) -> std::io::Result<File> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        NtCreateFile, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
        FILE_SYNCHRONOUS_IO_NONALERT,
    };
    use windows_sys::Win32::Foundation::{
        CloseHandle, RtlNtStatusToDosError, INVALID_HANDLE_VALUE, OBJ_CASE_INSENSITIVE,
        UNICODE_STRING,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_NORMAL, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_READ,
        FILE_SHARE_WRITE, SYNCHRONIZE,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let mut wide = filename.encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    let byte_length = wide
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let object_name = UNICODE_STRING {
        Length: byte_length,
        MaximumLength: byte_length,
        Buffer: wide.as_mut_ptr(),
    };
    let object_attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: directory.as_raw_handle(),
        ObjectName: std::ptr::addr_of!(object_name),
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut io_status = IO_STATUS_BLOCK::default();
    let mut handle = INVALID_HANDLE_VALUE;
    // SAFETY: the live directory handle anchors the child lookup and all
    // output pointers reference correctly sized writable storage.
    let status = unsafe {
        NtCreateFile(
            std::ptr::addr_of_mut!(handle),
            FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            std::ptr::addr_of!(object_attributes),
            std::ptr::addr_of_mut!(io_status),
            std::ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            std::ptr::null(),
            0,
        )
    };
    if status < 0 {
        if handle != INVALID_HANDLE_VALUE && !handle.is_null() {
            // SAFETY: the failed call still returned an owned handle.
            unsafe {
                CloseHandle(handle);
            }
        }
        return Err(std::io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ));
    }
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(std::io::Error::other(
            "NtCreateFile succeeded without returning a file handle",
        ));
    }
    // SAFETY: NtCreateFile returned a new owned handle.
    Ok(unsafe { File::from_raw_handle(handle) })
}

#[cfg(all(not(unix), not(windows)))]
fn open_backup_child(
    _directory: &File,
    _directory_path: &Path,
    _filename: &std::ffi::OsStr,
) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "anchored backup-child opens are unsupported on this platform",
    ))
}

fn open_validated_backup_directory(path: &Path) -> Result<File, AppError> {
    let directory = open_backup_directory(path).map_err(|error| AppError::io(path, error))?;
    let metadata = directory
        .metadata()
        .map_err(|error| AppError::io(path, error))?;
    if !metadata.file_type().is_dir() || metadata_is_reparse_point(&metadata) {
        return Err(AppError::InvalidInput(format!(
            "backup directory must be a non-symlink directory: {}",
            path.display()
        )));
    }
    Ok(directory)
}

#[cfg(windows)]
fn metadata_is_reparse_point(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse_point(_metadata: &Metadata) -> bool {
    false
}

fn validate_regular_file(path: &Path, max_bytes: u64) -> Result<Metadata, AppError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppError::InvalidInput(format!(
                "SQL restore source does not exist: {}",
                path.display()
            )));
        }
        Err(error) => return Err(AppError::io(path, error)),
    };
    if !metadata.file_type().is_file() || metadata_is_reparse_point(&metadata) {
        return Err(AppError::localized(
            "backup.sql.not_regular_file",
            format!("SQL 备份必须是普通且非符号链接文件: {}", path.display()),
            format!(
                "The SQL backup must be a regular non-symlink file: {}",
                path.display()
            ),
        ));
    }
    if metadata.len() > max_bytes {
        return Err(oversized_import(max_bytes));
    }
    Ok(metadata)
}

/// Copy a validated binary source through an anchored directory handle into a
/// process-owned file. SQLite never reopens the user-controlled path.
fn snapshot_binary_restore_file(path: &Path, max_bytes: u64) -> Result<NamedTempFile, AppError> {
    let initial = validate_regular_file(path, max_bytes)?;
    let directory_path = path.parent().ok_or_else(|| {
        AppError::InvalidInput(format!(
            "binary restore source has no parent directory: {}",
            path.display()
        ))
    })?;
    let filename = path.file_name().ok_or_else(|| {
        AppError::InvalidInput(format!(
            "binary restore source has no filename: {}",
            path.display()
        ))
    })?;
    let directory = open_validated_backup_directory(directory_path)?;
    let mut source = open_backup_child(&directory, directory_path, filename)
        .map_err(|error| AppError::io(path, error))?;
    let opened = source
        .metadata()
        .map_err(|error| AppError::io(path, error))?;
    #[cfg(unix)]
    let changed_before_open = !same_file_identity(&initial, &opened);
    #[cfg(not(unix))]
    let changed_before_open = {
        let _shape_only = initial;
        false
    };
    if !opened.file_type().is_file()
        || metadata_is_reparse_point(&opened)
        || opened.len() > max_bytes
        || changed_before_open
    {
        return Err(AppError::InvalidInput(format!(
            "binary restore source must be a bounded regular file: {}",
            path.display()
        )));
    }

    let mut owned = NamedTempFile::new().map_err(|error| AppError::IoContext {
        context: "create owned binary restore snapshot".to_string(),
        source: error,
    })?;
    let copied = std::io::copy(
        &mut Read::by_ref(&mut source).take(max_bytes + 1),
        owned.as_file_mut(),
    )
    .map_err(|error| AppError::io(path, error))?;
    if copied > max_bytes {
        return Err(oversized_import(max_bytes));
    }
    owned
        .as_file_mut()
        .flush()
        .map_err(|error| AppError::io(owned.path(), error))?;

    let completed = source
        .metadata()
        .map_err(|error| AppError::io(path, error))?;
    let current = open_backup_child(&directory, directory_path, filename)
        .map_err(|error| AppError::io(path, error))?;
    let current_metadata = current
        .metadata()
        .map_err(|error| AppError::io(path, error))?;
    let current_directory = open_validated_backup_directory(directory_path).map_err(|error| {
        AppError::InvalidInput(format!(
            "backup directory changed while source was read: {error}"
        ))
    })?;
    let same_source =
        same_open_file_identity(&source, &current).map_err(|error| AppError::io(path, error))?;
    let same_directory = same_open_file_identity(&directory, &current_directory)
        .map_err(|error| AppError::io(directory_path, error))?;
    if !same_source
        || !same_directory
        || metadata_is_reparse_point(&completed)
        || metadata_is_reparse_point(&current_metadata)
        || opened.len() != copied
        || completed.len() != copied
        || current_metadata.len() != copied
        || opened.modified().ok() != completed.modified().ok()
    {
        return Err(AppError::InvalidInput(format!(
            "binary restore source changed while it was read: {}",
            path.display()
        )));
    }
    Ok(owned)
}

fn read_restore_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, AppError> {
    let initial = validate_regular_file(path, max_bytes)?;
    let mut file = open_nofollow(path).map_err(|error| AppError::io(path, error))?;
    let opened = file.metadata().map_err(|error| AppError::io(path, error))?;

    #[cfg(unix)]
    let changed_before_open = !same_file_identity(&initial, &opened);
    #[cfg(not(unix))]
    let changed_before_open = {
        let _shape_only = initial;
        false
    };
    if !opened.file_type().is_file()
        || metadata_is_reparse_point(&opened)
        || opened.len() > max_bytes
        || changed_before_open
    {
        return Err(AppError::InvalidInput(format!(
            "restore source changed before open: {}",
            path.display()
        )));
    }

    let capacity = usize::try_from(opened.len())
        .ok()
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| {
            AppError::InvalidInput(format!(
                "SQL restore source is too large to address: {}",
                path.display()
            ))
        })?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(capacity).map_err(|error| {
        AppError::InvalidInput(format!(
            "unable to reserve memory for SQL restore source {}: {error}",
            path.display()
        ))
    })?;
    let mut limited: Take<&mut File> = Read::by_ref(&mut file).take(max_bytes + 1);
    limited
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::io(path, error))?;
    if bytes.len() as u64 > max_bytes {
        return Err(oversized_import(max_bytes));
    }

    let completed = file.metadata().map_err(|error| AppError::io(path, error))?;
    let current_path = fs::symlink_metadata(path).map_err(|error| AppError::io(path, error))?;
    let current_file = open_nofollow(path).map_err(|error| AppError::io(path, error))?;
    let current = current_file
        .metadata()
        .map_err(|error| AppError::io(path, error))?;
    let same_identity =
        same_open_file_identity(&file, &current_file).map_err(|error| AppError::io(path, error))?;
    let consumed = bytes.len() as u64;
    if !current_path.file_type().is_file()
        || !current.file_type().is_file()
        || metadata_is_reparse_point(&current_path)
        || metadata_is_reparse_point(&current)
        || !same_identity
        || opened.len() != consumed
        || completed.len() != consumed
        || current_path.len() != consumed
        || current.len() != consumed
        || opened.modified().ok() != completed.modified().ok()
    {
        return Err(AppError::InvalidInput(format!(
            "restore source changed while it was read: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

fn database_error(error: rusqlite::Error) -> AppError {
    AppError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        validate_regular_file, SqlImportBatch, UntrustedScratch, MAX_BINARY_RESTORE_BYTES,
        MAX_SQL_IMPORT_BYTES, TEST_MAX_PAGE_COUNT, TEST_MAX_VM_STEPS,
    };
    use crate::error::AppError;
    use std::fs::{self, File};

    struct RestoreLimitGuard {
        previous_vm_steps: Option<u64>,
        previous_page_count: Option<u64>,
    }

    impl RestoreLimitGuard {
        fn set(vm_steps: Option<u64>, page_count: Option<u64>) -> Self {
            let previous_vm_steps = TEST_MAX_VM_STEPS.with(|current| current.replace(vm_steps));
            let previous_page_count =
                TEST_MAX_PAGE_COUNT.with(|current| current.replace(page_count));
            Self {
                previous_vm_steps,
                previous_page_count,
            }
        }
    }

    impl Drop for RestoreLimitGuard {
        fn drop(&mut self) {
            TEST_MAX_VM_STEPS.with(|current| current.set(self.previous_vm_steps));
            TEST_MAX_PAGE_COUNT.with(|current| current.set(self.previous_page_count));
        }
    }

    #[test]
    fn batch_has_exactly_one_internal_terminator_and_skips_leading_boms() {
        let batch = SqlImportBatch::from_owned("\u{feff}\u{feff}SELECT 1;".to_string())
            .expect("prepare SQL batch");
        assert_eq!(batch.sql(), "SELECT 1;\0");
        assert_eq!(
            batch
                .sql()
                .as_bytes()
                .iter()
                .filter(|byte| **byte == 0)
                .count(),
            1
        );
    }

    #[test]
    fn batch_rejects_any_existing_nul() {
        let error = SqlImportBatch::from_owned("SELECT 1;\0SELECT 2;".to_string())
            .err()
            .expect("existing NUL must be rejected");
        assert!(error.to_string().to_ascii_lowercase().contains("nul"));
    }

    #[test]
    fn file_batch_keeps_export_file_as_plain_text() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let path = temp.path().join("export.sql");
        fs::write(&path, "\u{feff}SELECT 1;").expect("write SQL export");
        let batch = SqlImportBatch::read_from_path(&path).expect("prepare file batch");
        assert_eq!(batch.sql(), "SELECT 1;\0");
        assert_eq!(
            fs::read(&path).expect("read original SQL export"),
            "\u{feff}SELECT 1;".as_bytes()
        );
    }

    #[test]
    fn file_batch_rejects_content_larger_than_the_configured_limit() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let path = temp.path().join("oversized.sql");
        fs::write(&path, "12345").expect("write oversized fixture");
        let error = SqlImportBatch::read_from_path_with_limit(&path, 4)
            .err()
            .expect("files larger than the configured limit must be rejected");
        assert!(error.to_string().contains('4'));
    }

    #[test]
    fn regular_file_limit_accepts_n_and_rejects_n_plus_one() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let at_limit = temp.path().join("at-limit.sql");
        File::create(&at_limit)
            .and_then(|file| file.set_len(MAX_SQL_IMPORT_BYTES))
            .expect("create sparse at-limit file");
        assert_eq!(
            validate_regular_file(&at_limit, MAX_SQL_IMPORT_BYTES)
                .expect("the exact public limit must be accepted")
                .len(),
            MAX_SQL_IMPORT_BYTES
        );

        let over_limit = temp.path().join("over-limit.sql");
        File::create(&over_limit)
            .and_then(|file| file.set_len(MAX_SQL_IMPORT_BYTES + 1))
            .expect("create sparse over-limit file");
        assert!(
            SqlImportBatch::read_from_path(&over_limit).is_err(),
            "the public file entry must reject N+1 before allocating it"
        );
    }

    #[test]
    fn untrusted_execution_enforces_vm_and_page_budgets() {
        {
            let _limits = RestoreLimitGuard::set(Some(1_000), None);
            let batch = SqlImportBatch::from_owned(
                "WITH RECURSIVE counter(value) AS (
                     VALUES(0)
                     UNION ALL
                     SELECT value + 1 FROM counter WHERE value < 100000
                 )
                 SELECT SUM(value) FROM counter;"
                    .to_string(),
            )
            .expect("prepare recursive SQL");
            let error = UntrustedScratch::from_batch(&batch)
                .err()
                .expect("the VM budget must interrupt untrusted SQL");
            assert!(
                error.to_string().to_ascii_lowercase().contains("interrupt"),
                "unexpected VM budget error: {error}"
            );
        }

        {
            let _limits = RestoreLimitGuard::set(None, Some(8));
            let batch = SqlImportBatch::from_owned(
                "CREATE TABLE filler (payload BLOB);
                 INSERT INTO filler(payload) VALUES (zeroblob(1048576));"
                    .to_string(),
            )
            .expect("prepare page-heavy SQL");
            assert!(
                UntrustedScratch::from_batch(&batch).is_err(),
                "the page budget must reject oversized scratch growth"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn file_batch_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("create temp dir");
        let target = temp.path().join("target.sql");
        let link = temp.path().join("linked.sql");
        fs::write(&target, "SELECT 1;").expect("write target");
        symlink(&target, &link).expect("create symlink");
        assert!(SqlImportBatch::read_from_path(&link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn binary_restore_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("create temp dir");
        let target = temp.path().join("target.db");
        let link = temp.path().join("linked.db");
        fs::write(&target, b"not sqlite").expect("write target");
        symlink(&target, &link).expect("create symlink");
        assert!(UntrustedScratch::from_binary(&link).is_err());
    }

    #[test]
    fn binary_restore_rejects_n_plus_one_before_copying() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let over_limit = temp.path().join("over-limit.db");
        File::create(&over_limit)
            .and_then(|file| file.set_len(MAX_BINARY_RESTORE_BYTES + 1))
            .expect("create sparse over-limit binary fixture");
        assert!(UntrustedScratch::from_binary(&over_limit).is_err());
    }

    #[test]
    fn missing_file_is_reported_as_invalid_restore_input() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("missing.sql");

        assert!(matches!(
            SqlImportBatch::read_from_path(&missing),
            Err(AppError::InvalidInput(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn file_batch_rejects_fifos_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let temp = tempfile::tempdir().expect("create temp dir");
        let fifo = temp.path().join("restore.fifo");
        let path =
            CString::new(fifo.as_os_str().as_bytes()).expect("temporary FIFO path has no NUL");
        // SAFETY: `path` is a live NUL-terminated pathname and mkfifo does not
        // retain the pointer after returning.
        let result = unsafe { libc::mkfifo(path.as_ptr(), 0o600) };
        assert_eq!(
            result,
            0,
            "create FIFO fixture: {}",
            std::io::Error::last_os_error()
        );

        assert!(
            SqlImportBatch::read_from_path(&fifo).is_err(),
            "restore files must be ordinary files, never streams"
        );
    }
}
