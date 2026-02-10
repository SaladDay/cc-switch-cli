use crate::cli::ui::{create_table, error, highlight, info, success, warning};
use crate::error::AppError;
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Command;

/// CLI tools configuration for version checking
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CliTool {
    id: String,
    label: String,
    npm_package: String,
}

/// Version check result for a CLI tool
#[derive(Debug, Clone, Serialize)]
struct VersionCheckResult {
    id: String,
    label: String,
    current: Option<String>,
    latest: Option<String>,
    status: VersionStatus,
    upgrade_cmd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
enum VersionStatus {
    #[serde(rename = "latest")]
    Latest,
    #[serde(rename = "upgradable")]
    Upgradable,
    #[serde(rename = "not_installed")]
    NotInstalled,
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "fetch_failed")]
    FetchFailed,
}

impl VersionStatus {
    fn display(&self) -> &'static str {
        match self {
            VersionStatus::Latest => "最新",
            VersionStatus::Upgradable => "可升级",
            VersionStatus::NotInstalled => "未安装",
            VersionStatus::Unknown => "未知",
            VersionStatus::FetchFailed => "获取失败",
        }
    }
}

#[derive(Subcommand)]
pub enum CheckCommand {
    /// Check for CLI tool updates (Claude Code, Codex, Gemini, etc.)
    #[command(alias = "update")]
    Updates {
        /// Skip fetching latest versions (offline mode)
        #[arg(long)]
        offline: bool,

        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },

    /// Upgrade CLI tools to latest version
    Upgrade {
        /// Tool ID to upgrade (e.g., claude, codex, gemini). If not specified, upgrades all.
        tool: Option<String>,

        /// Actually execute the upgrade (without this flag, only shows what would be done)
        #[arg(long, short)]
        yes: bool,
    },
}

pub fn execute(cmd: CheckCommand, _app: Option<crate::app_config::AppType>) -> Result<(), AppError> {
    match cmd {
        CheckCommand::Updates { offline, json } => check_updates(offline, json),
        CheckCommand::Upgrade { tool, yes } => upgrade_tools(tool, yes),
    }
}

/// Get the list of CLI tools to check
fn get_cli_tools() -> Vec<CliTool> {
    vec![
        CliTool {
            id: "claude".to_string(),
            label: "Claude Code".to_string(),
            npm_package: "@anthropic-ai/claude-code".to_string(),
        },
        CliTool {
            id: "codex".to_string(),
            label: "Codex".to_string(),
            npm_package: "@openai/codex".to_string(),
        },
        CliTool {
            id: "gemini".to_string(),
            label: "Gemini".to_string(),
            npm_package: "@google/gemini-cli".to_string(),
        },
        CliTool {
            id: "opencode".to_string(),
            label: "OpenCode".to_string(),
            npm_package: "opencode-ai".to_string(),
        },
        CliTool {
            id: "qwen".to_string(),
            label: "Qwen Code".to_string(),
            npm_package: "@qwen-code/qwen-code".to_string(),
        },
    ]
}

/// Get globally installed npm packages and their versions
fn get_npm_globals() -> HashMap<String, String> {
    let mut map = HashMap::new();

    let output = Command::new("npm")
        .args(["ls", "-g", "--depth=0", "--json"])
        .output();

    let Ok(output) = output else {
        return map;
    };

    if !output.status.success() {
        return map;
    }

    let Ok(stdout) = String::from_utf8(output.stdout) else {
        return map;
    };

    #[derive(Deserialize)]
    struct NpmLsOutput {
        dependencies: Option<HashMap<String, NpmPackageInfo>>,
    }

    #[derive(Deserialize)]
    struct NpmPackageInfo {
        version: Option<String>,
    }

    if let Ok(parsed) = serde_json::from_str::<NpmLsOutput>(&stdout) {
        if let Some(deps) = parsed.dependencies {
            for (name, info) in deps {
                if let Some(version) = info.version {
                    map.insert(name, version);
                }
            }
        }
    }

    map
}

/// Get the latest version of an npm package
fn get_npm_latest_version(package: &str) -> Result<String, String> {
    // Try npmmirror first (faster in China), then fallback to official
    let registries = [
        "https://registry.npmmirror.com",
        "https://registry.npmjs.org",
    ];

    for registry in registries {
        let output = Command::new("npm")
            .args(["view", package, "version", "--registry", registry])
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !version.is_empty() {
                    return Ok(version);
                }
            }
        }
    }

    Err("Failed to fetch latest version".to_string())
}

/// Compare two semver versions
/// Returns: -1 if a < b, 0 if a == b, 1 if a > b
fn compare_versions(a: &str, b: &str) -> i32 {
    let parse = |v: &str| -> Vec<u32> {
        v.trim_start_matches('v')
            .split('.')
            .filter_map(|s| s.parse::<u32>().ok())
            .collect()
    };

    let a_parts = parse(a);
    let b_parts = parse(b);

    for i in 0..3 {
        let a_val = a_parts.get(i).copied().unwrap_or(0);
        let b_val = b_parts.get(i).copied().unwrap_or(0);
        if a_val < b_val {
            return -1;
        }
        if a_val > b_val {
            return 1;
        }
    }
    0
}

fn check_updates(offline: bool, json_output: bool) -> Result<(), AppError> {
    let tools = get_cli_tools();
    let npm_globals = get_npm_globals();

    let mut results: Vec<VersionCheckResult> = Vec::new();

    if !json_output {
        println!("\n{}", highlight("AI CLI Tools Version Check"));
        println!("{}", "═".repeat(60));
        println!();
    }

    // Use indicatif for progress if not JSON output
    let pb = if !json_output && !offline {
        let pb = indicatif::ProgressBar::new(tools.len() as u64);
        pb.set_style(
            indicatif::ProgressStyle::default_bar()
                .template("{spinner:.cyan} [{pos}/{len}] Checking {msg}...")
                .unwrap(),
        );
        Some(pb)
    } else {
        None
    };

    for tool in &tools {
        if let Some(ref pb) = pb {
            pb.set_message(tool.label.clone());
        }

        let current = npm_globals.get(&tool.npm_package).cloned();

        let (latest, fetch_error) = if offline {
            (None, None)
        } else {
            match get_npm_latest_version(&tool.npm_package) {
                Ok(v) => (Some(v), None),
                Err(e) => (None, Some(e)),
            }
        };

        let status = if current.is_none() {
            VersionStatus::NotInstalled
        } else if offline {
            VersionStatus::Unknown
        } else if fetch_error.is_some() {
            VersionStatus::FetchFailed
        } else if let (Some(ref curr), Some(ref lat)) = (&current, &latest) {
            if compare_versions(curr, lat) < 0 {
                VersionStatus::Upgradable
            } else {
                VersionStatus::Latest
            }
        } else {
            VersionStatus::Unknown
        };

        let upgrade_cmd = if current.is_some() || status == VersionStatus::NotInstalled {
            Some(format!("npm i -g {}@latest", tool.npm_package))
        } else {
            None
        };

        results.push(VersionCheckResult {
            id: tool.id.clone(),
            label: tool.label.clone(),
            current,
            latest,
            status,
            upgrade_cmd,
            error: fetch_error,
        });

        if let Some(ref pb) = pb {
            pb.inc(1);
        }
    }

    if let Some(pb) = pb {
        pb.finish_and_clear();
    }

    // Output results
    if json_output {
        let output = serde_json::json!({
            "title": "AI CLI Tools",
            "results": results
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
        return Ok(());
    }

    // Display table
    let mut table = create_table();
    table.set_header(vec!["CLI Tool", "Current", "Latest", "Status"]);

    for result in &results {
        let current_display = result.current.clone().unwrap_or("-".to_string());
        let latest_display = result.latest.clone().unwrap_or("-".to_string());
        let status_display = result.status.display();

        table.add_row(vec![
            result.label.as_str(),
            &current_display,
            &latest_display,
            status_display,
        ]);
    }

    println!("{}", table);

    // Summary
    let upgradable_count = results
        .iter()
        .filter(|r| r.status == VersionStatus::Upgradable)
        .count();
    let not_installed_count = results
        .iter()
        .filter(|r| r.status == VersionStatus::NotInstalled)
        .count();
    let fetch_failed_count = results
        .iter()
        .filter(|r| r.status == VersionStatus::FetchFailed)
        .count();

    println!();

    if upgradable_count > 0 {
        println!(
            "{}",
            warning(&format!("⬆ {} tool(s) can be upgraded", upgradable_count))
        );
        println!(
            "{}",
            info("  Run `cc-switch check upgrade --yes` to upgrade all")
        );
    }

    if not_installed_count > 0 {
        println!(
            "{}",
            info(&format!("📦 {} tool(s) not installed", not_installed_count))
        );
    }

    if fetch_failed_count > 0 {
        println!(
            "{}",
            error(&format!(
                "⚠ {} tool(s) failed to fetch latest version",
                fetch_failed_count
            ))
        );
        println!("{}", info("  Check your network or npm registry settings"));
    }

    if upgradable_count == 0 && fetch_failed_count == 0 && !offline {
        let installed_count = results
            .iter()
            .filter(|r| r.status == VersionStatus::Latest)
            .count();
        if installed_count > 0 {
            println!("{}", success("✓ All installed tools are up to date"));
        }
    }

    if offline {
        println!();
        println!(
            "{}",
            info("ℹ Offline mode: latest versions not checked. Remove --offline to check for updates.")
        );
    }

    Ok(())
}

fn upgrade_tools(tool_id: Option<String>, yes: bool) -> Result<(), AppError> {
    let tools = get_cli_tools();
    let npm_globals = get_npm_globals();

    // Filter tools to upgrade
    let tools_to_check: Vec<&CliTool> = if let Some(ref id) = tool_id {
        tools
            .iter()
            .filter(|t| t.id == *id || t.label.to_lowercase() == id.to_lowercase())
            .collect()
    } else {
        tools.iter().collect()
    };

    if tools_to_check.is_empty() {
        println!(
            "{}",
            error(&format!(
                "Tool '{}' not found. Available: claude, codex, gemini, opencode, qwen",
                tool_id.unwrap_or_default()
            ))
        );
        return Ok(());
    }

    // Check which tools need upgrade
    let mut upgradable: Vec<(&CliTool, String, String)> = Vec::new();

    println!("\n{}", highlight("Checking for upgrades..."));
    println!();

    for tool in tools_to_check {
        let current = npm_globals.get(&tool.npm_package);

        if current.is_none() {
            println!(
                "{}",
                info(&format!("  {} - not installed, will install", tool.label))
            );
            upgradable.push((tool, "-".to_string(), "latest".to_string()));
            continue;
        }

        match get_npm_latest_version(&tool.npm_package) {
            Ok(latest) => {
                let curr = current.unwrap();
                if compare_versions(curr, &latest) < 0 {
                    println!(
                        "{}",
                        warning(&format!(
                            "  {} - {} → {} (upgradable)",
                            tool.label, curr, latest
                        ))
                    );
                    upgradable.push((tool, curr.clone(), latest));
                } else {
                    println!(
                        "{}",
                        success(&format!("  {} - {} (up to date)", tool.label, curr))
                    );
                }
            }
            Err(_) => {
                println!(
                    "{}",
                    error(&format!(
                        "  {} - failed to check latest version",
                        tool.label
                    ))
                );
            }
        }
    }

    println!();

    if upgradable.is_empty() {
        println!("{}", success("✓ Nothing to upgrade"));
        return Ok(());
    }

    println!(
        "{}",
        highlight(&format!("{} tool(s) to upgrade:", upgradable.len()))
    );
    for (tool, _, _) in &upgradable {
        println!("  - {} (npm i -g {}@latest)", tool.label, tool.npm_package);
    }
    println!();

    if !yes {
        println!(
            "{}",
            warning("Add --yes flag to actually execute the upgrades")
        );
        return Ok(());
    }

    // Execute upgrades with registry fallback (npmmirror first, then official)
    let registries = [
        ("npmmirror", "https://registry.npmmirror.com"),
        ("npmjs.org", "https://registry.npmjs.org"),
    ];

    for (tool, _, _) in &upgradable {
        println!("{}", info(&format!("Upgrading {}...", tool.label)));

        let pkg = format!("{}@latest", tool.npm_package);
        let mut upgraded = false;

        for (reg_name, reg_url) in &registries {
            let status = Command::new("npm")
                .args(["i", "-g", &pkg, "--registry", reg_url])
                .status();

            match status {
                Ok(s) if s.success() => {
                    println!("{}", success(&format!("  ✓ {} upgraded successfully", tool.label)));
                    upgraded = true;
                    break;
                }
                _ => {
                    println!(
                        "{}",
                        warning(&format!("  ⚠ {} registry failed, trying next...", reg_name))
                    );
                }
            }
        }

        if !upgraded {
            println!("{}", error(&format!("  ✗ Failed to upgrade {} (all registries failed)", tool.label)));
        }
    }

    println!();
    println!("{}", success("Done!"));

    Ok(())
}
