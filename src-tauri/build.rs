use chrono::Local;
use std::process::Command;

fn set_build_time() {
    let build_time = Local::now().format("%Y-%m-%d %H:%M:%S %:z").to_string();
    println!("cargo:rustc-env=CC_SWITCH_TUI_BUILD_TIME={build_time}");
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn set_git_revision_hash() {
    if let Some(rev) = git_output(&["rev-parse", "--short=7", "HEAD"]) {
        println!("cargo:rustc-env=CC_SWITCH_TUI_BUILD_GIT_HASH={rev}");
    }
}

fn set_git_tag_version() {
    if let Some(tag) = git_output(&["describe", "--tags", "--abbrev=0"]) {
        let version = tag.strip_prefix('v').unwrap_or(&tag);
        if !version.is_empty() {
            println!("cargo:rustc-env=CC_SWITCH_TUI_GIT_TAG_VERSION={version}");
        }
    }
}

fn set_git_is_clean_commit() {
    let Ok(output) = Command::new("git").args(["status", "--porcelain"]).output() else {
        return;
    };

    if output.status.success() && output.stdout.is_empty() {
        println!("cargo:rustc-env=CC_SWITCH_TUI_GIT_IS_CLEAN_COMMIT=1");
    }
}

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/index");

    set_build_time();
    set_git_revision_hash();
    set_git_tag_version();
    set_git_is_clean_commit();
}
