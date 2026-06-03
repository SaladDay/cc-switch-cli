//! Hermes-specific CLI commands: memory / user profile management.
//!
//! Mirrors the memory subset of upstream `cc-switch/src-tauri/src/commands/hermes.rs`.
//! The web UI / dashboard launcher lives in the GUI; the CLI only exposes
//! the local-file interactions.

use std::io::{self, Read};
use std::path::Path;
use std::process::Command;

use clap::Subcommand;

use crate::cli::ui::{info, success, warning};
use crate::error::AppError;
use crate::hermes_config::{
    self, get_hermes_dir, read_memory, read_memory_limits, set_memory_enabled, write_memory,
    MemoryKind,
};

#[derive(Subcommand)]
pub enum HermesCommand {
    /// Hermes memory blob (MEMORY.md / USER.md) operations
    #[command(subcommand)]
    Memory(MemoryCommand),
}

#[derive(Subcommand)]
pub enum MemoryCommand {
    /// Print the content of a memory file to stdout
    Show {
        /// Memory kind to read
        #[arg(value_enum, default_value_t = MemoryKindArg::Memory)]
        kind: MemoryKindArg,
    },
    /// Write content into a memory file
    ///
    /// If `--content` is omitted, the new content is read from stdin.
    Set {
        /// Memory kind to write
        #[arg(value_enum)]
        kind: MemoryKindArg,
        /// Inline content (takes precedence over stdin)
        #[arg(long)]
        content: Option<String>,
    },
    /// Clear a memory file (writes empty content)
    Clear {
        /// Memory kind to clear
        #[arg(value_enum)]
        kind: MemoryKindArg,
        /// Confirm the destructive operation
        #[arg(long)]
        yes: bool,
    },
    /// Enable a memory blob (writes `memory_enabled` / `user_profile_enabled = true`)
    Enable {
        /// Memory kind to enable
        #[arg(value_enum)]
        kind: MemoryKindArg,
    },
    /// Disable a memory blob (writes the corresponding `*_enabled = false`)
    Disable {
        /// Memory kind to disable
        #[arg(value_enum)]
        kind: MemoryKindArg,
    },
    /// Show character limits and enable flags for both memory blobs
    Limits,
    /// Open the Hermes memories directory in the system file manager
    #[command(name = "open-dir")]
    OpenDir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum MemoryKindArg {
    Memory,
    User,
}

impl From<MemoryKindArg> for MemoryKind {
    fn from(value: MemoryKindArg) -> Self {
        match value {
            MemoryKindArg::Memory => MemoryKind::Memory,
            MemoryKindArg::User => MemoryKind::User,
        }
    }
}

pub fn execute(cmd: HermesCommand) -> Result<(), AppError> {
    match cmd {
        HermesCommand::Memory(memory_cmd) => execute_memory(memory_cmd),
    }
}

fn execute_memory(cmd: MemoryCommand) -> Result<(), AppError> {
    match cmd {
        MemoryCommand::Show { kind } => show_memory(kind.into()),
        MemoryCommand::Set { kind, content } => set_memory_cmd(kind.into(), content),
        MemoryCommand::Clear { kind, yes } => clear_memory(kind.into(), yes),
        MemoryCommand::Enable { kind } => toggle_memory(kind.into(), true),
        MemoryCommand::Disable { kind } => toggle_memory(kind.into(), false),
        MemoryCommand::Limits => print_limits(),
        MemoryCommand::OpenDir => open_memory_dir(),
    }
}

fn show_memory(kind: MemoryKind) -> Result<(), AppError> {
    let content = read_memory(kind)?;
    if content.is_empty() {
        println!(
            "{}",
            info(&format!(
                "Hermes {} memory is empty (file not created yet)",
                kind.as_str()
            ))
        );
    } else {
        print!("{content}");
        if !content.ends_with('\n') {
            println!();
        }
    }
    Ok(())
}

fn set_memory_cmd(kind: MemoryKind, content: Option<String>) -> Result<(), AppError> {
    let content = match content {
        Some(value) => value,
        None => {
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| AppError::Config(format!("Failed to read stdin: {e}")))?;
            buf
        }
    };

    write_memory(kind, &content)?;
    println!(
        "{}",
        success(&format!(
            "✓ Wrote {} bytes to Hermes {} memory",
            content.len(),
            kind.as_str()
        ))
    );
    Ok(())
}

fn clear_memory(kind: MemoryKind, yes: bool) -> Result<(), AppError> {
    if !yes {
        println!(
            "{}",
            warning(&format!(
                "Refusing to clear Hermes {} memory without --yes",
                kind.as_str()
            ))
        );
        return Ok(());
    }
    write_memory(kind, "")?;
    println!(
        "{}",
        success(&format!("✓ Cleared Hermes {} memory", kind.as_str()))
    );
    Ok(())
}

fn toggle_memory(kind: MemoryKind, enabled: bool) -> Result<(), AppError> {
    set_memory_enabled(kind, enabled)?;
    let verb = if enabled { "Enabled" } else { "Disabled" };
    println!(
        "{}",
        success(&format!("✓ {verb} Hermes {} memory", kind.as_str()))
    );
    Ok(())
}

fn print_limits() -> Result<(), AppError> {
    let limits = read_memory_limits()?;
    println!("Hermes memory limits:");
    println!(
        "  memory:  {} chars  (enabled: {})",
        limits.memory, limits.memory_enabled
    );
    println!(
        "  user:    {} chars  (enabled: {})",
        limits.user, limits.user_enabled
    );

    let memory_len = read_memory(MemoryKind::Memory)
        .map(|s| s.len())
        .unwrap_or(0);
    let user_len = read_memory(MemoryKind::User).map(|s| s.len()).unwrap_or(0);

    println!();
    println!("Current usage (file size in bytes; Hermes truncates at character budget on load):");
    println!("  memory:  {memory_len} bytes");
    println!("  user:    {user_len} bytes");

    // Avoid `unused import` if the helper isn't used elsewhere in this file.
    let _ = hermes_config::get_hermes_config_path();
    Ok(())
}

fn open_memory_dir() -> Result<(), AppError> {
    let target_dir = get_hermes_dir().join("memories");
    std::fs::create_dir_all(&target_dir).map_err(|e| AppError::io(&target_dir, e))?;
    open_directory(&target_dir).map_err(AppError::Message)?;
    println!("{}", success("Opened Hermes memories directory."));
    Ok(())
}

fn open_directory(path: &Path) -> Result<bool, String> {
    if std::env::var_os("CC_SWITCH_TEST_DISABLE_OPEN").is_some() {
        return Ok(true);
    }

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(path);
        command
    };

    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(path);
        command
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("explorer");
        command.arg(path);
        command
    };

    let status = command
        .status()
        .map_err(|error| format!("Failed to open directory {}: {error}", path.display()))?;

    if status.success() {
        Ok(true)
    } else {
        Err(format!(
            "Failed to open directory {}: opener exited with status {status}",
            path.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        lock_test_home_and_settings, set_test_home_override, TestHomeSettingsLock,
    };
    use std::ffi::OsString;
    use std::path::Path;
    use tempfile::{tempdir, TempDir};

    struct EnvGuard {
        _lock: TestHomeSettingsLock,
        old_home: Option<OsString>,
        old_userprofile: Option<OsString>,
        old_config_dir: Option<OsString>,
        old_disable_open: Option<OsString>,
        home: TempDir,
    }

    impl EnvGuard {
        fn new() -> Self {
            let lock = lock_test_home_and_settings();
            let home = tempdir().expect("create temp home");
            let old_home = std::env::var_os("HOME");
            let old_userprofile = std::env::var_os("USERPROFILE");
            let old_config_dir = std::env::var_os("CC_SWITCH_CONFIG_DIR");
            let old_disable_open = std::env::var_os("CC_SWITCH_TEST_DISABLE_OPEN");
            std::env::set_var("HOME", home.path());
            std::env::set_var("USERPROFILE", home.path());
            std::env::set_var("CC_SWITCH_CONFIG_DIR", home.path().join(".cc-switch"));
            std::env::set_var("CC_SWITCH_TEST_DISABLE_OPEN", "1");
            set_test_home_override(Some(home.path()));
            crate::settings::reload_test_settings();
            Self {
                _lock: lock,
                old_home,
                old_userprofile,
                old_config_dir,
                old_disable_open,
                home,
            }
        }

        fn hermes_memories_dir(&self) -> std::path::PathBuf {
            self.home.path().join(".hermes").join("memories")
        }

        fn hermes_config_path(&self) -> std::path::PathBuf {
            self.home.path().join(".hermes").join("config.yaml")
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.old_userprofile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
            match &self.old_config_dir {
                Some(value) => std::env::set_var("CC_SWITCH_CONFIG_DIR", value),
                None => std::env::remove_var("CC_SWITCH_CONFIG_DIR"),
            }
            match &self.old_disable_open {
                Some(value) => std::env::set_var("CC_SWITCH_TEST_DISABLE_OPEN", value),
                None => std::env::remove_var("CC_SWITCH_TEST_DISABLE_OPEN"),
            }
            set_test_home_override(self.old_home.as_deref().map(Path::new));
            crate::settings::reload_test_settings();
        }
    }

    #[test]
    fn hermes_memory_open_dir_creates_memories_directory() {
        let env = EnvGuard::new();

        execute_memory(MemoryCommand::OpenDir).expect("open Hermes memories dir");

        assert!(env.hermes_memories_dir().is_dir());
    }

    #[test]
    fn hermes_memory_show_works_without_existing_hermes_dir() {
        let env = EnvGuard::new();

        execute_memory(MemoryCommand::Show {
            kind: MemoryKindArg::Memory,
        })
        .expect("show Hermes memory");

        assert!(!env.hermes_memories_dir().exists());
    }

    #[test]
    fn hermes_memory_set_creates_memories_directory_without_existing_hermes_dir() {
        let env = EnvGuard::new();

        execute_memory(MemoryCommand::Set {
            kind: MemoryKindArg::User,
            content: Some("user profile".to_string()),
        })
        .expect("set Hermes user memory");

        assert_eq!(
            read_memory(MemoryKind::User).expect("read user memory"),
            "user profile"
        );
        assert!(env.hermes_memories_dir().is_dir());
    }

    #[test]
    fn hermes_memory_toggle_creates_config_without_existing_hermes_dir() {
        let env = EnvGuard::new();

        execute_memory(MemoryCommand::Disable {
            kind: MemoryKindArg::Memory,
        })
        .expect("disable Hermes memory");

        let limits = read_memory_limits().expect("read memory limits");
        assert!(!limits.memory_enabled);
        assert!(env.hermes_config_path().is_file());
    }

    #[test]
    fn hermes_memory_limits_works_without_existing_hermes_dir() {
        let env = EnvGuard::new();

        execute_memory(MemoryCommand::Limits).expect("print Hermes limits");

        assert!(!env.hermes_config_path().exists());
    }
}
