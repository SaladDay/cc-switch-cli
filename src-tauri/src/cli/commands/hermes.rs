//! Hermes-specific CLI commands: memory / user profile management.
//!
//! Mirrors the memory subset of upstream `cc-switch/src-tauri/src/commands/hermes.rs`.
//! The web UI / dashboard launcher lives in the GUI; the CLI only exposes
//! the local-file interactions.

use std::io::{self, Read};

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

fn ensure_hermes_dir_exists() -> Result<(), AppError> {
    if get_hermes_dir().exists() {
        return Ok(());
    }
    Err(AppError::localized(
        "hermes.dir.missing",
        format!("Hermes 配置目录不存在：{}", get_hermes_dir().display()),
        format!(
            "Hermes config dir not found: {}",
            get_hermes_dir().display()
        ),
    ))
}

pub fn execute(cmd: HermesCommand) -> Result<(), AppError> {
    match cmd {
        HermesCommand::Memory(memory_cmd) => execute_memory(memory_cmd),
    }
}

fn execute_memory(cmd: MemoryCommand) -> Result<(), AppError> {
    ensure_hermes_dir_exists()?;
    match cmd {
        MemoryCommand::Show { kind } => show_memory(kind.into()),
        MemoryCommand::Set { kind, content } => set_memory_cmd(kind.into(), content),
        MemoryCommand::Clear { kind, yes } => clear_memory(kind.into(), yes),
        MemoryCommand::Enable { kind } => toggle_memory(kind.into(), true),
        MemoryCommand::Disable { kind } => toggle_memory(kind.into(), false),
        MemoryCommand::Limits => print_limits(),
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
