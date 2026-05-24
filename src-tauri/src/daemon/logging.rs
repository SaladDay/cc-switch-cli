//! Daemon log file appender.
//!
//! Adds a file destination to the existing `log` facade. The file is opened in
//! append mode and rotated when it grows past `MAX_LOG_BYTES`, keeping a single
//! `.1` backup. Concurrent writers serialize through a `Mutex<File>`.
//!
//! Initialization is idempotent: calling `install()` more than once installs
//! the logger only once.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use chrono::Utc;
use log::{Level, LevelFilter, Log, Metadata, Record};

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

static INSTALL_RESULT: OnceLock<Result<PathBuf, String>> = OnceLock::new();

struct DaemonLogger {
    path: PathBuf,
    file: Mutex<File>,
    level: LevelFilter,
}

impl DaemonLogger {
    fn open(path: &Path, level: LevelFilter) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            file: Mutex::new(file),
            level,
        })
    }
}

impl Log for DaemonLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let now = Utc::now().to_rfc3339();
        let line = format!(
            "{} {:5} {} {}\n",
            now,
            level_label(record.level()),
            record.target(),
            record.args()
        );

        let mut file = match self.file.lock() {
            Ok(f) => f,
            Err(poisoned) => poisoned.into_inner(),
        };

        // Rotate before writing if the file is already over the cap; this way
        // the new line lands in the fresh log file rather than the rolled-over
        // backup.
        if let Ok(metadata) = file.metadata() {
            if metadata.len() > MAX_LOG_BYTES {
                drop(file);
                let _ = self.rotate();
                file = match self.file.lock() {
                    Ok(f) => f,
                    Err(poisoned) => poisoned.into_inner(),
                };
            }
        }

        let _ = file.write_all(line.as_bytes());
    }

    fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.flush();
        }
    }
}

impl DaemonLogger {
    fn rotate(&self) -> std::io::Result<()> {
        let backup = self.path.with_extension(format!(
            "{}.1",
            self.path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("log")
        ));
        let _ = std::fs::remove_file(&backup);
        std::fs::rename(&self.path, &backup)?;
        let new_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let mut guard = match self.file.lock() {
            Ok(f) => f,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = new_file;
        Ok(())
    }
}

fn level_label(level: Level) -> &'static str {
    match level {
        Level::Error => "ERROR",
        Level::Warn => "WARN",
        Level::Info => "INFO",
        Level::Debug => "DEBUG",
        Level::Trace => "TRACE",
    }
}

/// Install the daemon logger as the global `log` sink. Returns the resolved
/// log path. Idempotent — subsequent calls return the originally chosen path
/// without re-installing.
pub fn install(path: &Path, level: LevelFilter) -> Result<PathBuf, String> {
    INSTALL_RESULT
        .get_or_init(|| {
            let logger = DaemonLogger::open(path, level)
                .map_err(|err| format!("open daemon logger at {} failed: {err}", path.display()))?;
            let resolved = logger.path.clone();
            let boxed: Box<dyn Log> = Box::new(logger);
            log::set_boxed_logger(boxed).map_err(|err| {
                format!("install daemon logger at {} failed: {err}", path.display())
            })?;
            log::set_max_level(level);
            Ok(resolved)
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_directly(path: &Path, msg: &str) {
        let logger = DaemonLogger::open(path, LevelFilter::Info).expect("open logger");
        logger.log(
            &Record::builder()
                .level(Level::Info)
                .target("test")
                .args(format_args!("{}", msg))
                .build(),
        );
        logger.flush();
    }

    #[test]
    fn writing_appends_to_log_file() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("test.log");
        write_directly(&path, "hello world");
        let contents = std::fs::read_to_string(&path).expect("read log");
        assert!(contents.contains("INFO"));
        assert!(contents.contains("hello world"));
        assert!(contents.ends_with('\n'));
    }

    #[test]
    fn rotation_renames_existing_log_when_too_large() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("test.log");
        // Pre-populate with > MAX_LOG_BYTES so the next write triggers rotation.
        std::fs::write(&path, vec![b'x'; (MAX_LOG_BYTES + 1) as usize]).expect("seed");
        write_directly(&path, "trigger rotate");

        let backup = path.with_extension("log.1");
        assert!(backup.exists(), "rotation should produce a .1 backup");
        let new_contents = std::fs::read_to_string(&path).expect("read new log");
        assert!(new_contents.contains("trigger rotate"));
    }

    #[test]
    fn level_filter_is_respected() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("test.log");
        let logger = DaemonLogger::open(&path, LevelFilter::Warn).expect("open");
        logger.log(
            &Record::builder()
                .level(Level::Info)
                .target("test")
                .args(format_args!("info-line"))
                .build(),
        );
        logger.log(
            &Record::builder()
                .level(Level::Warn)
                .target("test")
                .args(format_args!("warn-line"))
                .build(),
        );
        logger.flush();
        let contents = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(!contents.contains("info-line"));
        assert!(contents.contains("warn-line"));
    }
}
