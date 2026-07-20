use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::cli::i18n::texts;
use crate::AppError;

#[derive(Debug, PartialEq, Eq)]
struct PreferredEditorCommand {
    program: String,
    args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetectedEditor {
    pub label: String,
    pub command: String,
}

#[derive(Debug, Clone, Copy)]
struct EditorCandidate {
    label: &'static str,
    program: &'static str,
    args: &'static [&'static str],
    macos_program: Option<&'static str>,
}

const GUI_EDITOR_CANDIDATES: &[EditorCandidate] = &[
    EditorCandidate {
        label: "Visual Studio Code",
        program: "code",
        args: &["--wait"],
        macos_program: Some(
            "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
        ),
    },
    EditorCandidate {
        label: "Visual Studio Code Insiders",
        program: "code-insiders",
        args: &["--wait"],
        macos_program: Some(
            "/Applications/Visual Studio Code - Insiders.app/Contents/Resources/app/bin/code-insiders",
        ),
    },
    EditorCandidate {
        label: "VSCodium",
        program: "codium",
        args: &["--wait"],
        macos_program: Some("/Applications/VSCodium.app/Contents/Resources/app/bin/codium"),
    },
    EditorCandidate {
        label: "Cursor",
        program: "cursor",
        args: &["--wait"],
        macos_program: Some("/Applications/Cursor.app/Contents/Resources/app/bin/cursor"),
    },
    EditorCandidate {
        label: "Windsurf",
        program: "windsurf",
        args: &["--wait"],
        macos_program: Some("/Applications/Windsurf.app/Contents/Resources/app/bin/windsurf"),
    },
    EditorCandidate {
        label: "Zed",
        program: "zed",
        args: &["--wait"],
        macos_program: None,
    },
    EditorCandidate {
        label: "Zed",
        program: "zeditor",
        args: &["--wait"],
        macos_program: None,
    },
    EditorCandidate {
        label: "Sublime Text",
        program: "subl",
        args: &["--wait"],
        macos_program: Some("/Applications/Sublime Text.app/Contents/SharedSupport/bin/subl"),
    },
];

#[cfg(target_os = "linux")]
const PLATFORM_GUI_EDITOR_CANDIDATES: &[EditorCandidate] = &[
    EditorCandidate {
        label: "Kate",
        program: "kate",
        args: &["--block"],
        macos_program: None,
    },
    EditorCandidate {
        label: "GNOME Text Editor",
        program: "gedit",
        args: &["--wait"],
        macos_program: None,
    },
    EditorCandidate {
        label: "GVim",
        program: "gvim",
        args: &["--nofork"],
        macos_program: None,
    },
];

#[cfg(not(target_os = "linux"))]
#[cfg(not(target_os = "windows"))]
const PLATFORM_GUI_EDITOR_CANDIDATES: &[EditorCandidate] = &[];

#[cfg(target_os = "windows")]
const PLATFORM_GUI_EDITOR_CANDIDATES: &[EditorCandidate] = &[EditorCandidate {
    label: "Notepad",
    program: "notepad.exe",
    args: &[],
    macos_program: None,
}];

const TERMINAL_EDITOR_CANDIDATES: &[EditorCandidate] = &[
    EditorCandidate {
        label: "Neovim",
        program: "nvim",
        args: &[],
        macos_program: None,
    },
    EditorCandidate {
        label: "Vim",
        program: "vim",
        args: &[],
        macos_program: None,
    },
    EditorCandidate {
        label: "Helix",
        program: "hx",
        args: &[],
        macos_program: None,
    },
    EditorCandidate {
        label: "Micro",
        program: "micro",
        args: &[],
        macos_program: None,
    },
    EditorCandidate {
        label: "Nano",
        program: "nano",
        args: &[],
        macos_program: None,
    },
    EditorCandidate {
        label: "Emacs",
        program: "emacs",
        args: &[],
        macos_program: None,
    },
    EditorCandidate {
        label: "Vi",
        program: "vi",
        args: &[],
        macos_program: None,
    },
    EditorCandidate {
        label: "Sensible Editor",
        program: "sensible-editor",
        args: &[],
        macos_program: None,
    },
];

fn editor_candidates() -> Vec<EditorCandidate> {
    GUI_EDITOR_CANDIDATES
        .iter()
        .chain(PLATFORM_GUI_EDITOR_CANDIDATES)
        .chain(TERMINAL_EDITOR_CANDIDATES)
        .copied()
        .collect()
}

/// Detect external editors on demand. This intentionally performs only
/// environment parsing and a bounded set of PATH/direct-path lookups.
pub(crate) fn detect_external_editors() -> Vec<DetectedEditor> {
    let visual = std::env::var("VISUAL").ok();
    let editor = std::env::var("EDITOR").ok();
    let path = std::env::var_os("PATH");
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let candidates = editor_candidates();

    detect_external_editors_from(
        visual.as_deref(),
        editor.as_deref(),
        &candidates,
        cfg!(target_os = "macos"),
        |program| resolve_program_in(program, path.as_deref(), &cwd),
    )
}

fn detect_external_editors_from(
    visual: Option<&str>,
    editor: Option<&str>,
    candidates: &[EditorCandidate],
    include_macos_programs: bool,
    mut resolve_program: impl FnMut(&str) -> Option<String>,
) -> Vec<DetectedEditor> {
    let mut detected = Vec::new();
    let mut seen_commands = HashSet::new();

    for (source, raw) in [("VISUAL", visual), ("EDITOR", editor)] {
        let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
            continue;
        };
        let Ok(parsed) = parse_preferred_editor_command(raw) else {
            continue;
        };
        if !is_blocking_editor_command(&parsed) {
            continue;
        }
        let Some(program) = resolve_program(&parsed.program) else {
            continue;
        };
        let Some(command) = canonical_editor_command(
            std::iter::once(program.as_str()).chain(parsed.args.iter().map(String::as_str)),
        ) else {
            continue;
        };
        push_detected_editor(
            &mut detected,
            &mut seen_commands,
            format!("{source}: {}", editor_program_label(&parsed.program)),
            command,
        );
    }

    for candidate in candidates {
        let program = if let Some(program) = resolve_program(candidate.program) {
            Some(program)
        } else if include_macos_programs {
            candidate.macos_program.and_then(&mut resolve_program)
        } else {
            None
        };
        let Some(program) = program else {
            continue;
        };
        let Some(command) = canonical_editor_command(
            std::iter::once(program.as_str()).chain(candidate.args.iter().copied()),
        ) else {
            continue;
        };
        push_detected_editor(
            &mut detected,
            &mut seen_commands,
            candidate.label.to_string(),
            command,
        );
    }

    detected
}

fn push_detected_editor(
    detected: &mut Vec<DetectedEditor>,
    seen_commands: &mut HashSet<String>,
    label: String,
    command: String,
) {
    if seen_commands.insert(command.clone()) {
        detected.push(DetectedEditor { label, command });
    }
}

fn canonical_editor_command<'a>(parts: impl IntoIterator<Item = &'a str>) -> Option<String> {
    shlex::try_join(parts).ok()
}

fn editor_program_label(program: &str) -> String {
    let name = Path::new(program)
        .file_stem()
        .and_then(OsStr::to_str)
        .filter(|name| !name.is_empty())
        .unwrap_or(program);
    match name.to_ascii_lowercase().as_str() {
        "code" => "Visual Studio Code".to_string(),
        "code-insiders" => "Visual Studio Code Insiders".to_string(),
        "codium" => "VSCodium".to_string(),
        "cursor" => "Cursor".to_string(),
        "windsurf" => "Windsurf".to_string(),
        "zed" | "zeditor" => "Zed".to_string(),
        "subl" => "Sublime Text".to_string(),
        "nvim" => "Neovim".to_string(),
        "vim" => "Vim".to_string(),
        "hx" => "Helix".to_string(),
        "micro" => "Micro".to_string(),
        "nano" => "Nano".to_string(),
        "emacs" => "Emacs".to_string(),
        "vi" => "Vi".to_string(),
        _ => name.to_string(),
    }
}

fn is_blocking_editor_command(command: &PreferredEditorCommand) -> bool {
    let program = Path::new(&command.program)
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or(command.program.as_str())
        .to_ascii_lowercase();

    match program.as_str() {
        "code" | "code-insiders" | "codium" | "cursor" | "windsurf" | "zed" | "zeditor"
        | "subl" => command
            .args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--wait" | "-w")),
        "kate" => command
            .args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--block" | "-b")),
        "gedit" => command.args.iter().any(|arg| arg == "--wait"),
        "gvim" => command
            .args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--nofork" | "-f")),
        // macOS `open` is safe only when explicitly asked to wait for the
        // opened application. Plain `open` returns before editing completes.
        "open" => command
            .args
            .iter()
            .any(|arg| matches!(arg.as_str(), "-W" | "--wait-apps")),
        "xdg-open" | "gnome-open" | "kde-open" | "wslview" | "cygstart" | "explorer" | "start" => {
            false
        }
        "gio" => command.args.first().is_none_or(|arg| arg != "open"),
        "cmd" => !command
            .args
            .iter()
            .any(|arg| arg.eq_ignore_ascii_case("start")),
        _ => true,
    }
}

fn resolve_program_in(program: &str, path: Option<&OsStr>, cwd: &Path) -> Option<String> {
    which::which_in(program, path, cwd)
        .ok()?
        .into_os_string()
        .into_string()
        .ok()
}

pub fn open_external_editor(initial_content: &str) -> Result<String, AppError> {
    let preferred_editor = crate::settings::get_preferred_editor();
    open_external_editor_with_preference(initial_content, preferred_editor.as_deref())
}

pub(crate) fn validate_preferred_editor_command(command: &str) -> Result<(), AppError> {
    parse_blocking_editor_command(command).map(|_| ())
}

fn open_external_editor_with_preference(
    initial_content: &str,
    preferred_editor: Option<&str>,
) -> Result<String, AppError> {
    let command = preferred_editor.ok_or_else(|| {
        editor_failure(crate::t!(
            "no default external editor is configured; choose one in Settings",
            "尚未配置默认外部编辑器；请先在设置中选择"
        ))
    })?;
    edit_with_preferred_editor(initial_content, command)
}

fn edit_with_preferred_editor(initial_content: &str, command: &str) -> Result<String, AppError> {
    let parsed = parse_blocking_editor_command(command)?;
    let mut file = tempfile::NamedTempFile::new().map_err(|error| {
        editor_failure(format!(
            "{}: {error}",
            crate::t!("failed to create a temporary file", "创建临时文件失败")
        ))
    })?;
    file.write_all(initial_content.as_bytes())
        .and_then(|_| file.flush())
        .map_err(|error| {
            editor_failure(format!(
                "{}: {error}",
                crate::t!("failed to prepare the temporary file", "准备临时文件失败")
            ))
        })?;

    // Close the creating handle before the editor opens the file. This is
    // required by some Windows editors and still keeps automatic cleanup.
    let path = file.into_temp_path();
    let status = Command::new(&parsed.program)
        .args(&parsed.args)
        .arg(&path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| {
            editor_failure(format!(
                "{} `{}`: {error}",
                crate::t!(
                    "failed to launch configured editor",
                    "启动已配置的编辑器失败"
                ),
                command.trim()
            ))
        })?;

    if !status.success() {
        return Err(editor_failure(format!(
            "{} `{}`: {status}",
            crate::t!(
                "configured editor exited unsuccessfully",
                "已配置的编辑器异常退出"
            ),
            command.trim()
        )));
    }

    let edited = fs::read_to_string(&path).map_err(|error| {
        editor_failure(format!(
            "{}: {error}",
            crate::t!(
                "failed to read the edited temporary file",
                "读取编辑后的临时文件失败"
            )
        ))
    })?;
    path.close().map_err(|error| {
        editor_failure(format!(
            "{}: {error}",
            crate::t!("failed to remove the temporary file", "删除临时文件失败")
        ))
    })?;

    Ok(edited)
}

fn parse_preferred_editor_command(command: &str) -> Result<PreferredEditorCommand, AppError> {
    let command = command.trim();
    if command.is_empty() {
        return Err(invalid_editor_command(crate::t!(
            "the command is empty",
            "命令为空"
        )));
    }
    if command.as_bytes().contains(&0) {
        return Err(invalid_editor_command(crate::t!(
            "NUL bytes are not allowed",
            "不允许包含 NUL 字节"
        )));
    }

    let mut parts = shlex::split(command).ok_or_else(|| {
        invalid_editor_command(crate::t!(
            "quotes or escapes are not balanced",
            "引号或转义符不完整"
        ))
    })?;
    if parts.is_empty() || parts[0].trim().is_empty() {
        return Err(invalid_editor_command(crate::t!(
            "the executable is empty",
            "可执行文件为空"
        )));
    }

    let program = parts.remove(0);
    Ok(PreferredEditorCommand {
        program,
        args: parts,
    })
}

fn parse_blocking_editor_command(command: &str) -> Result<PreferredEditorCommand, AppError> {
    let parsed = parse_preferred_editor_command(command)?;
    if !is_blocking_editor_command(&parsed) {
        return Err(invalid_editor_command(crate::t!(
            "the editor command must wait until editing is finished (for example, use --wait)",
            "编辑器命令必须等待编辑完成（例如使用 --wait）"
        )));
    }
    Ok(parsed)
}

fn invalid_editor_command(detail: &str) -> AppError {
    editor_failure(format!(
        "{}: {detail}",
        crate::t!("invalid preferred editor command", "无效的首选编辑器命令")
    ))
}

fn editor_failure(error: impl std::fmt::Display) -> AppError {
    AppError::Message(format!("{}: {error}", texts::editor_failed()))
}

#[cfg(test)]
mod tests {
    use super::{
        detect_external_editors_from, editor_candidates, open_external_editor_with_preference,
        parse_preferred_editor_command, resolve_program_in, validate_preferred_editor_command,
        DetectedEditor, EditorCandidate, PreferredEditorCommand,
    };

    #[test]
    fn preferred_editor_command_parses_program_and_arguments() {
        assert_eq!(
            parse_preferred_editor_command("code --wait --reuse-window")
                .expect("parse editor command"),
            PreferredEditorCommand {
                program: "code".to_string(),
                args: vec!["--wait".to_string(), "--reuse-window".to_string()],
            }
        );
        assert_eq!(
            parse_preferred_editor_command(
                "\"/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code\" --wait"
            )
            .expect("parse quoted editor path"),
            PreferredEditorCommand {
                program: "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"
                    .to_string(),
                args: vec!["--wait".to_string()],
            }
        );
    }

    #[test]
    fn preferred_editor_command_rejects_empty_invalid_and_nul_input() {
        assert!(validate_preferred_editor_command("").is_err());
        assert!(validate_preferred_editor_command("   ").is_err());
        assert!(validate_preferred_editor_command("\"unterminated").is_err());
        assert!(validate_preferred_editor_command("\"\" --wait").is_err());
        assert!(validate_preferred_editor_command("code\0--wait").is_err());
        assert!(validate_preferred_editor_command("code").is_err());
        assert!(validate_preferred_editor_command("code -w").is_ok());
        assert!(validate_preferred_editor_command("xdg-open").is_err());
        assert!(validate_preferred_editor_command("open -W -a TextEdit").is_ok());
    }

    #[test]
    fn editor_detection_prioritizes_environment_and_deduplicates_commands() {
        let candidates = [
            EditorCandidate {
                label: "Visual Studio Code",
                program: "code",
                args: &["--wait"],
                macos_program: None,
            },
            EditorCandidate {
                label: "Neovim",
                program: "nvim",
                args: &[],
                macos_program: None,
            },
            EditorCandidate {
                label: "Vim",
                program: "vim",
                args: &[],
                macos_program: None,
            },
        ];

        let detected = detect_external_editors_from(
            Some("  code --wait  "),
            Some("nvim"),
            &candidates,
            false,
            |program| matches!(program, "code" | "nvim" | "vim").then(|| program.to_string()),
        );

        assert_eq!(
            detected,
            vec![
                DetectedEditor {
                    label: "VISUAL: Visual Studio Code".to_string(),
                    command: "code --wait".to_string(),
                },
                DetectedEditor {
                    label: "EDITOR: Neovim".to_string(),
                    command: "nvim".to_string(),
                },
                DetectedEditor {
                    label: "Vim".to_string(),
                    command: "vim".to_string(),
                },
            ]
        );
    }

    #[test]
    fn editor_detection_skips_empty_invalid_and_unavailable_environment_values() {
        let candidates = [EditorCandidate {
            label: "Nano",
            program: "nano",
            args: &[],
            macos_program: None,
        }];

        let invalid = detect_external_editors_from(
            Some("  "),
            Some("\"unterminated"),
            &candidates,
            false,
            |program| (program == "nano").then(|| program.to_string()),
        );
        assert_eq!(
            invalid,
            vec![DetectedEditor {
                label: "Nano".to_string(),
                command: "nano".to_string(),
            }]
        );

        let unavailable =
            detect_external_editors_from(None, Some("custom-editor"), &[], false, |_| None);
        assert!(unavailable.is_empty());
    }

    #[test]
    fn editor_detection_preserves_the_resolved_executable_path() {
        let candidates = [EditorCandidate {
            label: "Visual Studio Code",
            program: "code",
            args: &["--wait"],
            macos_program: None,
        }];
        let resolved = r"C:\Users\demo\AppData\Local\Programs\Microsoft VS Code\bin\code.cmd";

        let detected = detect_external_editors_from(None, None, &candidates, false, |program| {
            (program == "code").then(|| resolved.to_string())
        });

        assert_eq!(detected.len(), 1);
        assert_eq!(
            shlex::split(&detected[0].command),
            Some(vec![resolved.to_string(), "--wait".to_string()])
        );
    }

    #[test]
    fn editor_detection_rejects_nonblocking_generic_openers() {
        let candidates = [EditorCandidate {
            label: "Nano",
            program: "nano",
            args: &[],
            macos_program: None,
        }];
        let detected = detect_external_editors_from(
            Some("xdg-open"),
            Some("open"),
            &candidates,
            false,
            |program| Some(program.to_string()),
        );

        assert_eq!(
            detected,
            vec![DetectedEditor {
                label: "Nano".to_string(),
                command: "nano".to_string(),
            }]
        );

        let blocking_open = detect_external_editors_from(
            Some("open -W -a TextEdit"),
            None,
            &[],
            false,
            |program| Some(program.to_string()),
        );
        assert_eq!(blocking_open.len(), 1);
        assert_eq!(blocking_open[0].label, "VISUAL: open");
        assert_eq!(blocking_open[0].command, "open -W -a TextEdit");

        let code_without_wait = detect_external_editors_from(
            Some("code"),
            None,
            &[EditorCandidate {
                label: "Visual Studio Code",
                program: "code",
                args: &["--wait"],
                macos_program: None,
            }],
            false,
            |program| Some(program.to_string()),
        );
        assert_eq!(code_without_wait.len(), 1);
        assert_eq!(code_without_wait[0].label, "Visual Studio Code");
        assert_eq!(code_without_wait[0].command, "code --wait");
    }

    #[test]
    fn common_gui_editor_candidates_always_include_wait_arguments() {
        let candidates = editor_candidates();
        let detected = detect_external_editors_from(None, None, &candidates, false, |program| {
            Some(program.to_string())
        });

        for program in [
            "code",
            "code-insiders",
            "codium",
            "cursor",
            "windsurf",
            "zed",
            "zeditor",
            "subl",
        ] {
            let editor = detected
                .iter()
                .find(|editor| editor.command.starts_with(program))
                .unwrap_or_else(|| panic!("missing GUI editor candidate: {program}"));
            assert_eq!(editor.command, format!("{program} --wait"));
        }
        for program in [
            "nvim",
            "vim",
            "hx",
            "micro",
            "nano",
            "emacs",
            "vi",
            "sensible-editor",
        ] {
            assert!(detected.iter().any(|editor| editor.command == program));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_gui_editor_candidates_use_blocking_arguments() {
        let candidates = editor_candidates();
        let detected = detect_external_editors_from(None, None, &candidates, false, |program| {
            Some(program.to_string())
        });

        for command in ["kate --block", "gedit --wait", "gvim --nofork"] {
            assert!(detected.iter().any(|editor| editor.command == command));
        }
    }

    #[test]
    fn editor_detection_can_use_a_verified_macos_bundle_fallback() {
        let candidates = [EditorCandidate {
            label: "Demo",
            program: "demo",
            args: &["--wait"],
            macos_program: Some("/Applications/Demo Editor.app/Contents/MacOS/demo"),
        }];
        let detected = detect_external_editors_from(None, None, &candidates, true, |program| {
            (program == "/Applications/Demo Editor.app/Contents/MacOS/demo")
                .then(|| program.to_string())
        });

        assert_eq!(detected.len(), 1);
        assert_eq!(detected[0].label, "Demo");
        assert_eq!(
            shlex::split(&detected[0].command),
            Some(vec![
                "/Applications/Demo Editor.app/Contents/MacOS/demo".to_string(),
                "--wait".to_string(),
            ])
        );
    }

    #[test]
    fn opening_without_a_configured_editor_reports_settings_action() {
        let error = open_external_editor_with_preference("before", None)
            .expect_err("an unset editor must not be chosen automatically");
        let message = error.to_string();
        assert!(message.contains("Settings"), "{message}");
    }

    #[cfg(unix)]
    fn create_editor_script(directory: &std::path::Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;

        let path = directory.join(name);
        std::fs::write(&path, body).expect("write fake editor");
        let mut permissions = std::fs::metadata(&path)
            .expect("read fake editor metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).expect("make fake editor executable");
        path.to_string_lossy().into_owned()
    }

    #[cfg(unix)]
    #[test]
    fn editor_resolution_preserves_isolated_and_direct_executable_paths() {
        let directory = tempfile::tempdir().expect("create isolated editor directory");
        let editor =
            create_editor_script(directory.path(), "detected-editor", "#!/bin/sh\nexit 0\n");
        let path = std::env::join_paths([directory.path()]).expect("build isolated PATH");

        assert_eq!(
            resolve_program_in("detected-editor", Some(path.as_os_str()), directory.path(),)
                .as_deref(),
            Some(editor.as_str())
        );
        assert_eq!(
            resolve_program_in(&editor, Some(path.as_os_str()), directory.path()).as_deref(),
            Some(editor.as_str())
        );
        assert_eq!(
            resolve_program_in("missing-editor", Some(path.as_os_str()), directory.path(),),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn configured_editor_receives_arguments_and_rewrites_last_file_argument() {
        let directory = tempfile::tempdir().expect("create isolated editor directory");
        let editor = create_editor_script(
            directory.path(),
            "fake editor",
            "#!/bin/sh\ninitial=$(cat \"$2\")\nprintf '%s|%s' \"$initial\" \"$1\" > \"$2\"\n",
        );
        let command =
            shlex::try_join([editor.as_str(), "after"]).expect("quote fake editor command");

        let edited = open_external_editor_with_preference("before", Some(&command))
            .expect("run configured editor");

        assert_eq!(edited, "before|after");
    }

    #[cfg(unix)]
    #[test]
    fn configured_editor_nonzero_exit_is_reported_without_fallback() {
        let directory = tempfile::tempdir().expect("create isolated editor directory");
        let editor =
            create_editor_script(directory.path(), "failing-editor", "#!/bin/sh\nexit 23\n");
        let command = shlex::try_join([editor.as_str()]).expect("quote fake editor command");

        let error = open_external_editor_with_preference("before", Some(&command))
            .expect_err("nonzero editor exit should fail");
        let message = error.to_string();
        assert!(message.contains(&command), "{message}");
        assert!(message.contains("23"), "{message}");
    }

    #[test]
    fn missing_configured_editor_is_reported_without_fallback() {
        let command = "cc-switch-editor-that-does-not-exist-7f2bd3";
        let error = open_external_editor_with_preference("before", Some(command))
            .expect_err("missing configured editor should fail");
        assert!(error.to_string().contains(command));
    }
}
