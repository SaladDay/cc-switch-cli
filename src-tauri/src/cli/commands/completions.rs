use clap::{Args, Subcommand, ValueEnum};
use clap_complete::Shell;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{home_dir, write_text_file};
use crate::AppError;

const MANAGED_BLOCK_START: &str = "# >>> cc-switch completions >>>";
const MANAGED_BLOCK_END: &str = "# <<< cc-switch completions <<<";
const COMMAND_NAME: &str = "cc-switch-tui";

#[derive(Args, Debug, Clone)]
#[command(arg_required_else_help = true)]
pub struct CompletionsCommand {
    /// The shell to generate completions for
    #[arg(value_enum)]
    pub shell: Option<Shell>,

    #[command(subcommand)]
    pub action: Option<CompletionsAction>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum CompletionsAction {
    /// Install managed shell completions for bash or zsh
    Install(CompletionsInstallCommand),

    /// Show managed shell completions status for bash or zsh
    Status(CompletionLifecycleCommand),

    /// Remove managed shell completions for bash or zsh
    Uninstall(CompletionLifecycleCommand),
}

#[derive(Args, Debug, Clone)]
pub struct CompletionsInstallCommand {
    /// Shell to manage automatically
    #[arg(long, value_enum, default_value_t = ManagedShellSelection::Auto)]
    pub shell: ManagedShellSelection,

    /// Explicitly modify the shell rc file with a managed activation block
    #[arg(long)]
    pub activate: bool,
}

#[derive(Args, Debug, Clone)]
pub struct CompletionLifecycleCommand {
    /// Shell to inspect or uninstall
    #[arg(long, value_enum, default_value_t = ManagedShellSelection::Auto)]
    pub shell: ManagedShellSelection,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedShellSelection {
    Auto,
    Bash,
    Zsh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedShell {
    Bash,
    Zsh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompletionPaths {
    completion_file: PathBuf,
    completion_dir: PathBuf,
    rc_file: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivationChange {
    Added,
    Updated,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusSnapshot {
    shell: ManagedShell,
    shell_message: String,
    paths: CompletionPaths,
    completion_file_exists: bool,
    rc_file_exists: bool,
    activation_managed: bool,
    reload_recommended: bool,
    next_step: String,
}

pub fn execute(command: CompletionsCommand) -> Result<(), AppError> {
    match (command.shell, command.action) {
        (Some(shell), None) => {
            crate::cli::generate_completions(shell);
            Ok(())
        }
        (None, Some(CompletionsAction::Install(args))) => execute_install(args),
        (None, Some(CompletionsAction::Status(args))) => execute_status(args),
        (None, Some(CompletionsAction::Uninstall(args))) => execute_uninstall(args),
        (Some(_), Some(_)) => Err(AppError::InvalidInput(
            "shell generator path cannot be combined with lifecycle subcommands".to_string(),
        )),
        (None, None) => Err(AppError::InvalidInput(
            "missing completions shell or lifecycle subcommand".to_string(),
        )),
    }
}

fn execute_install(args: CompletionsInstallCommand) -> Result<(), AppError> {
    let (shell, shell_message) = resolve_managed_shell(args.shell)?;
    let home = require_home_dir()?;
    let paths = completion_paths(shell, &home);
    let script = generated_completion_script(shell)?;

    write_text_file(&paths.completion_file, &script)?;

    println!("{shell_message}");
    println!(
        "Installed completion file: {}",
        paths.completion_file.display()
    );

    if args.activate {
        let activation_change = upsert_activation_block(shell, &paths)?;
        println!(
            "Managed activation block: {}",
            match activation_change {
                ActivationChange::Added => format!("added to {}", paths.rc_file.display()),
                ActivationChange::Updated => format!("updated in {}", paths.rc_file.display()),
                ActivationChange::Unchanged => {
                    format!("already present in {}", paths.rc_file.display())
                }
            }
        );
        println!(
            "Next step: reload your shell or run `{}`",
            reload_command(shell, &paths)
        );
    } else {
        println!(
            "Managed activation block: not changed (use --activate to let cc-switch edit {})",
            paths.rc_file.display()
        );
        println!(
            "Next step: run `{COMMAND_NAME} completions install --shell {} --activate` to activate it automatically, or wire {} manually.",
            shell.as_str(),
            paths.rc_file.display()
        );
    }

    Ok(())
}

fn execute_status(args: CompletionLifecycleCommand) -> Result<(), AppError> {
    let snapshot = completion_status(args.shell)?;

    println!("{}", snapshot.shell_message);
    println!(
        "Completion file: {} ({})",
        snapshot.paths.completion_file.display(),
        if snapshot.completion_file_exists {
            "installed"
        } else {
            "missing"
        }
    );
    println!(
        "RC file: {} ({})",
        snapshot.paths.rc_file.display(),
        if snapshot.rc_file_exists {
            "present"
        } else {
            "missing"
        }
    );
    println!(
        "Managed activation block: {}",
        if snapshot.activation_managed {
            "yes"
        } else {
            "no"
        }
    );
    println!(
        "Current shell likely needs reload: {}",
        if snapshot.reload_recommended {
            "yes"
        } else {
            "no"
        }
    );
    println!("Next step: {}", snapshot.next_step);

    Ok(())
}

fn execute_uninstall(args: CompletionLifecycleCommand) -> Result<(), AppError> {
    let snapshot = completion_status(args.shell)?;

    if snapshot.completion_file_exists {
        fs::remove_file(&snapshot.paths.completion_file)
            .map_err(|err| AppError::io(&snapshot.paths.completion_file, err))?;
    }

    let activation_removed = remove_activation_block(&snapshot.paths)?;

    println!("{}", snapshot.shell_message);
    println!(
        "Removed completion file: {}",
        if snapshot.completion_file_exists {
            snapshot.paths.completion_file.display().to_string()
        } else {
            format!(
                "{} (already absent)",
                snapshot.paths.completion_file.display()
            )
        }
    );
    println!(
        "Managed activation block: {}",
        if activation_removed {
            format!("removed from {}", snapshot.paths.rc_file.display())
        } else {
            format!("not present in {}", snapshot.paths.rc_file.display())
        }
    );
    println!(
        "Next step: reload your shell or restart it to drop any already-loaded completion definitions."
    );

    Ok(())
}

fn completion_status(selection: ManagedShellSelection) -> Result<StatusSnapshot, AppError> {
    let (shell, shell_message) = resolve_managed_shell(selection)?;
    let home = require_home_dir()?;
    let paths = completion_paths(shell, &home);
    let rc_content = read_optional_text_file(&paths.rc_file)?;
    let activation_managed = managed_block_range(&rc_content).is_some();
    let completion_file_exists = paths.completion_file.exists();

    Ok(StatusSnapshot {
        shell,
        shell_message,
        rc_file_exists: paths.rc_file.exists(),
        completion_file_exists,
        activation_managed,
        reload_recommended: completion_file_exists && activation_managed,
        next_step: next_step_for_status(shell, &paths, completion_file_exists, activation_managed),
        paths,
    })
}

fn next_step_for_status(
    shell: ManagedShell,
    paths: &CompletionPaths,
    completion_file_exists: bool,
    activation_managed: bool,
) -> String {
    if !completion_file_exists {
        return format!(
            "Run `{COMMAND_NAME} completions install --shell {}` to install the completion file.",
            shell.as_str()
        );
    }

    if !activation_managed {
        return format!(
            "Run `{COMMAND_NAME} completions install --shell {} --activate` to add a managed activation block to {}.",
            shell.as_str(),
            paths.rc_file.display()
        );
    }

    format!(
        "Reload your shell or run `{}`.",
        reload_command(shell, paths)
    )
}

fn resolve_managed_shell(
    selection: ManagedShellSelection,
) -> Result<(ManagedShell, String), AppError> {
    match selection {
        ManagedShellSelection::Bash => Ok((ManagedShell::Bash, "Selected shell: bash".to_string())),
        ManagedShellSelection::Zsh => Ok((ManagedShell::Zsh, "Selected shell: zsh".to_string())),
        ManagedShellSelection::Auto => detect_shell_from_env(),
    }
}

fn detect_shell_from_env() -> Result<(ManagedShell, String), AppError> {
    let shell_env = env::var("SHELL").ok();
    let shell_name = shell_env
        .as_deref()
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str());

    match shell_name {
        Some("bash") => Ok((
            ManagedShell::Bash,
            format!(
                "Detected shell: bash{}",
                shell_env
                    .as_deref()
                    .map(|value| format!(" (from SHELL={value})"))
                    .unwrap_or_default()
            ),
        )),
        Some("zsh") => Ok((
            ManagedShell::Zsh,
            format!(
                "Detected shell: zsh{}",
                shell_env
                    .as_deref()
                    .map(|value| format!(" (from SHELL={value})"))
                    .unwrap_or_default()
            ),
        )),
        Some(other) => Err(AppError::Message(format!(
            "Automatic completion lifecycle currently supports only bash and zsh. Detected SHELL={other}. Re-run with `--shell bash` or `--shell zsh`, or use raw generation via `{COMMAND_NAME} completions <shell>`."
        ))),
        None => Err(AppError::Message(format!(
            "Could not detect a supported shell from SHELL. Re-run with `--shell bash` or `--shell zsh`, or use raw generation via `{COMMAND_NAME} completions <shell>`."
        ))),
    }
}

fn require_home_dir() -> Result<PathBuf, AppError> {
    home_dir().ok_or_else(|| AppError::Config("无法获取用户主目录".to_string()))
}

fn completion_paths(shell: ManagedShell, home: &Path) -> CompletionPaths {
    match shell {
        ManagedShell::Bash => CompletionPaths {
            completion_file: home
                .join(".local/share/bash-completion/completions")
                .join(COMMAND_NAME),
            completion_dir: home.join(".local/share/bash-completion/completions"),
            rc_file: home.join(".bashrc"),
        },
        ManagedShell::Zsh => CompletionPaths {
            completion_file: home
                .join(".local/share/zsh/site-functions")
                .join(format!("_{COMMAND_NAME}")),
            completion_dir: home.join(".local/share/zsh/site-functions"),
            rc_file: home.join(".zshrc"),
        },
    }
}

fn generated_completion_script(shell: ManagedShell) -> Result<String, AppError> {
    let mut buffer = Vec::new();
    crate::cli::generate_completions_to(shell.generator_shell(), &mut buffer);
    String::from_utf8(buffer)
        .map_err(|err| AppError::Message(format!("Failed to decode generated completions: {err}")))
}

fn upsert_activation_block(
    shell: ManagedShell,
    paths: &CompletionPaths,
) -> Result<ActivationChange, AppError> {
    let existing = read_optional_text_file(&paths.rc_file)?;
    let (stripped, had_block) = remove_managed_block(&existing);
    let insert_before = match shell {
        ManagedShell::Zsh => find_compinit_insert_offset(&stripped),
        ManagedShell::Bash => None,
    };
    let block = activation_block(shell, insert_before.is_none());
    let updated = insert_block(&stripped, &block, insert_before);

    if updated != existing {
        write_text_file(&paths.rc_file, &updated)?;
    }

    Ok(match (had_block, updated == existing) {
        (false, _) => ActivationChange::Added,
        (true, true) => ActivationChange::Unchanged,
        (true, false) => ActivationChange::Updated,
    })
}

fn remove_activation_block(paths: &CompletionPaths) -> Result<bool, AppError> {
    let existing = read_optional_text_file(&paths.rc_file)?;
    let (updated, had_block) = remove_managed_block(&existing);

    if had_block && updated != existing {
        write_text_file(&paths.rc_file, &updated)?;
    }

    Ok(had_block)
}

fn read_optional_text_file(path: &Path) -> Result<String, AppError> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(AppError::io(path, err)),
    }
}

fn managed_block_range(content: &str) -> Option<(usize, usize)> {
    let start = content.find(MANAGED_BLOCK_START)?;
    let end_start = content[start..].find(MANAGED_BLOCK_END)? + start;
    let mut end = end_start + MANAGED_BLOCK_END.len();

    if content[end..].starts_with("\r\n") {
        end += 2;
    } else if content[end..].starts_with('\n') {
        end += 1;
    }

    Some((start, end))
}

fn remove_managed_block(content: &str) -> (String, bool) {
    let Some((start, end)) = managed_block_range(content) else {
        return (content.to_string(), false);
    };

    let mut updated = String::with_capacity(content.len());
    updated.push_str(&content[..start]);
    updated.push_str(&content[end..]);

    (updated, true)
}

fn find_compinit_insert_offset(content: &str) -> Option<usize> {
    let mut offset = 0usize;

    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') && trimmed.contains("compinit") {
            return Some(offset);
        }
        offset += line.len();
    }

    None
}

fn insert_block(content: &str, block: &str, before: Option<usize>) -> String {
    let mut updated = String::new();

    match before {
        Some(index) => {
            updated.push_str(&content[..index]);
            if !updated.is_empty() && !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push_str(block);
            if !updated.ends_with('\n') {
                updated.push('\n');
            }
            if !content[index..].is_empty() && !content[index..].starts_with('\n') {
                updated.push('\n');
            }
            updated.push_str(&content[index..]);
        }
        None => {
            updated.push_str(content);
            if !updated.is_empty() && !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push_str(block);
            if !updated.ends_with('\n') {
                updated.push('\n');
            }
        }
    }

    updated
}

fn activation_block(shell: ManagedShell, bootstrap_compinit: bool) -> String {
    match shell {
        ManagedShell::Bash => format!(
            "{MANAGED_BLOCK_START}\nif [ -f \"$HOME/.local/share/bash-completion/completions/{COMMAND_NAME}\" ]; then\n  . \"$HOME/.local/share/bash-completion/completions/{COMMAND_NAME}\"\nfi\n{MANAGED_BLOCK_END}\n"
        ),
        ManagedShell::Zsh if bootstrap_compinit => format!(
            "{MANAGED_BLOCK_START}\nif [ -d \"$HOME/.local/share/zsh/site-functions\" ]; then\n  fpath=(\"$HOME/.local/share/zsh/site-functions\" $fpath)\nfi\nautoload -Uz compinit\ncompinit\n{MANAGED_BLOCK_END}\n"
        ),
        ManagedShell::Zsh => format!(
            "{MANAGED_BLOCK_START}\nif [ -d \"$HOME/.local/share/zsh/site-functions\" ]; then\n  fpath=(\"$HOME/.local/share/zsh/site-functions\" $fpath)\nfi\n{MANAGED_BLOCK_END}\n"
        ),
    }
}

fn reload_command(shell: ManagedShell, paths: &CompletionPaths) -> String {
    match shell {
        ManagedShell::Bash | ManagedShell::Zsh => format!("source {}", paths.rc_file.display()),
    }
}

impl ManagedShell {
    fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
        }
    }

    fn generator_shell(self) -> Shell {
        match self {
            Self::Bash => Shell::Bash,
            Self::Zsh => Shell::Zsh,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{lock_test_home_and_settings, set_test_home_override};
    use serial_test::serial;
    use std::ffi::OsString;

    struct ShellEnvGuard {
        original: Option<OsString>,
    }

    impl ShellEnvGuard {
        fn set(value: Option<&str>) -> Self {
            let original = env::var_os("SHELL");
            match value {
                Some(value) => unsafe { env::set_var("SHELL", value) },
                None => unsafe { env::remove_var("SHELL") },
            }
            Self { original }
        }
    }

    impl Drop for ShellEnvGuard {
        fn drop(&mut self) {
            match self.original.as_ref() {
                Some(value) => unsafe { env::set_var("SHELL", value) },
                None => unsafe { env::remove_var("SHELL") },
            }
        }
    }

    struct TestHomeGuard {
        _lock: crate::test_support::TestHomeSettingsLock,
        old_shell: ShellEnvGuard,
    }

    impl TestHomeGuard {
        fn new(home: &Path, shell: Option<&str>) -> Self {
            let lock = lock_test_home_and_settings();
            set_test_home_override(Some(home));
            let old_shell = ShellEnvGuard::set(shell);
            Self {
                _lock: lock,
                old_shell,
            }
        }
    }

    impl Drop for TestHomeGuard {
        fn drop(&mut self) {
            let _ = &self.old_shell;
            set_test_home_override(None);
        }
    }

    fn marker_count(content: &str) -> usize {
        content.matches(MANAGED_BLOCK_START).count()
    }

    #[test]
    #[serial(home_settings)]
    fn detects_supported_shell_from_env() {
        let _guard = ShellEnvGuard::set(Some("/bin/bash"));
        let (shell, message) =
            resolve_managed_shell(ManagedShellSelection::Auto).expect("bash should resolve");
        assert_eq!(shell, ManagedShell::Bash);
        assert!(message.contains("Detected shell: bash"));
    }

    #[test]
    #[serial(home_settings)]
    fn auto_shell_detection_rejects_unsupported_shell() {
        let _guard = ShellEnvGuard::set(Some("/usr/bin/fish"));
        let err = resolve_managed_shell(ManagedShellSelection::Auto)
            .expect_err("fish should not be supported for managed lifecycle");
        assert!(err.to_string().contains("only bash and zsh"));
    }

    #[test]
    #[serial(home_settings)]
    fn auto_shell_detection_requires_shell_env() {
        let _guard = ShellEnvGuard::set(None);
        let err = resolve_managed_shell(ManagedShellSelection::Auto)
            .expect_err("missing SHELL should fail");
        assert!(err.to_string().contains("Could not detect"));
    }

    #[test]
    fn calculates_bash_and_zsh_completion_paths_under_home_override() {
        let home = PathBuf::from("/tmp/cc-switch-home");

        let bash = completion_paths(ManagedShell::Bash, &home);
        assert_eq!(
            bash.completion_file,
            home.join(".local/share/bash-completion/completions/cc-switch-tui")
        );
        assert_eq!(bash.rc_file, home.join(".bashrc"));

        let zsh = completion_paths(ManagedShell::Zsh, &home);
        assert_eq!(
            zsh.completion_file,
            home.join(".local/share/zsh/site-functions/_cc-switch-tui")
        );
        assert_eq!(zsh.rc_file, home.join(".zshrc"));
    }

    #[test]
    #[serial(home_settings)]
    fn install_with_activate_creates_completion_file_and_managed_bash_block() {
        let temp = tempfile::tempdir().expect("create temp home");
        let _guard = TestHomeGuard::new(temp.path(), Some("/bin/bash"));

        execute_install(CompletionsInstallCommand {
            shell: ManagedShellSelection::Auto,
            activate: true,
        })
        .expect("install should succeed");

        let paths = completion_paths(ManagedShell::Bash, temp.path());
        let script = fs::read_to_string(&paths.completion_file).expect("read bash completion");
        let rc = fs::read_to_string(&paths.rc_file).expect("read bash rc");

        assert!(script.contains("_cc-switch-tui"));
        assert_eq!(marker_count(&rc), 1);
        assert!(rc.contains(". \"$HOME/.local/share/bash-completion/completions/cc-switch-tui\""));
    }

    #[test]
    #[serial(home_settings)]
    fn activate_is_idempotent_for_bash() {
        let temp = tempfile::tempdir().expect("create temp home");
        let _guard = TestHomeGuard::new(temp.path(), Some("/bin/bash"));

        execute_install(CompletionsInstallCommand {
            shell: ManagedShellSelection::Auto,
            activate: true,
        })
        .expect("first install should succeed");
        execute_install(CompletionsInstallCommand {
            shell: ManagedShellSelection::Auto,
            activate: true,
        })
        .expect("second install should succeed");

        let paths = completion_paths(ManagedShell::Bash, temp.path());
        let rc = fs::read_to_string(&paths.rc_file).expect("read bash rc");
        assert_eq!(marker_count(&rc), 1);
    }

    #[test]
    #[serial(home_settings)]
    fn uninstall_removes_only_managed_block_and_keeps_surrounding_bash_content() {
        let temp = tempfile::tempdir().expect("create temp home");
        let _guard = TestHomeGuard::new(temp.path(), Some("/bin/bash"));
        let paths = completion_paths(ManagedShell::Bash, temp.path());
        write_text_file(
            &paths.rc_file,
            "export PATH=\"$HOME/.local/bin:$PATH\"\n\n# >>> cc-switch completions >>>\nlegacy\n# <<< cc-switch completions <<<\n\nalias ll='ls -al'\n",
        )
        .expect("write bash rc");

        execute_uninstall(CompletionLifecycleCommand {
            shell: ManagedShellSelection::Auto,
        })
        .expect("uninstall should succeed");

        let rc = fs::read_to_string(&paths.rc_file).expect("read bash rc");
        assert!(!rc.contains(MANAGED_BLOCK_START));
        assert!(rc.contains("export PATH=\"$HOME/.local/bin:$PATH\""));
        assert!(rc.contains("alias ll='ls -al'"));
    }

    #[test]
    #[serial(home_settings)]
    fn zsh_activate_inserts_managed_block_before_existing_compinit() {
        let temp = tempfile::tempdir().expect("create temp home");
        let _guard = TestHomeGuard::new(temp.path(), Some("/bin/zsh"));
        let paths = completion_paths(ManagedShell::Zsh, temp.path());
        write_text_file(
            &paths.rc_file,
            "export PATH=\"$HOME/.local/bin:$PATH\"\nautoload -Uz compinit\ncompinit\n",
        )
        .expect("seed zshrc");

        execute_install(CompletionsInstallCommand {
            shell: ManagedShellSelection::Auto,
            activate: true,
        })
        .expect("install should succeed");

        let rc = fs::read_to_string(&paths.rc_file).expect("read zshrc");
        let block_pos = rc.find(MANAGED_BLOCK_START).expect("managed block exists");
        let compinit_pos = rc.find("autoload -Uz compinit").expect("compinit exists");
        assert!(block_pos < compinit_pos);
        assert!(!rc[block_pos..compinit_pos].contains("compinit\ncompinit"));
    }

    #[test]
    #[serial(home_settings)]
    fn zsh_activate_bootstraps_compinit_when_missing() {
        let temp = tempfile::tempdir().expect("create temp home");
        let _guard = TestHomeGuard::new(temp.path(), Some("/bin/zsh"));

        execute_install(CompletionsInstallCommand {
            shell: ManagedShellSelection::Auto,
            activate: true,
        })
        .expect("install should succeed");

        let paths = completion_paths(ManagedShell::Zsh, temp.path());
        let rc = fs::read_to_string(&paths.rc_file).expect("read zshrc");
        assert!(rc.contains("autoload -Uz compinit"));
        assert!(rc.contains("\ncompinit\n"));
    }

    #[test]
    #[serial(home_settings)]
    fn status_reports_missing_activation_when_install_did_not_edit_rc() {
        let temp = tempfile::tempdir().expect("create temp home");
        let _guard = TestHomeGuard::new(temp.path(), Some("/bin/bash"));

        execute_install(CompletionsInstallCommand {
            shell: ManagedShellSelection::Auto,
            activate: false,
        })
        .expect("install should succeed");

        let snapshot = completion_status(ManagedShellSelection::Auto).expect("status");
        assert!(snapshot.completion_file_exists);
        assert!(!snapshot.activation_managed);
        assert!(snapshot.next_step.contains("--activate"));
    }
}
