#![cfg(not(windows))]

use serial_test::serial;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root should exist")
        .to_path_buf()
}

fn install_script_path() -> PathBuf {
    repo_root().join("install.sh")
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("script should be written");
    let mut perms = fs::metadata(path)
        .expect("metadata should exist")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("permissions should be updated");
}

struct Harness {
    _temp: TempDir,
    home: PathBuf,
    fakebin: PathBuf,
    install_dir: PathBuf,
    logs_dir: PathBuf,
    archive_path: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp dir should exist");
        let home = temp.path().join("home");
        let fakebin = temp.path().join("fakebin");
        let install_dir = temp.path().join("install");
        let logs_dir = temp.path().join("logs");
        let payload_dir = temp.path().join("payload");
        let archive_path = temp.path().join("cc-switch.tar.gz");

        fs::create_dir_all(&home).expect("home should exist");
        fs::create_dir_all(&fakebin).expect("fakebin should exist");
        fs::create_dir_all(&install_dir).expect("install dir should exist");
        fs::create_dir_all(&logs_dir).expect("logs dir should exist");
        fs::create_dir_all(&payload_dir).expect("payload dir should exist");

        write_executable(
            &payload_dir.join("cc-switch-tui"),
            "#!/usr/bin/env bash\necho new build\n",
        );

        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive_path)
            .arg("-C")
            .arg(&payload_dir)
            .arg("cc-switch-tui")
            .status()
            .expect("tar should run");
        assert!(status.success(), "tar should create archive");

        write_executable(
            &fakebin.join("uname"),
            r#"#!/usr/bin/env bash
set -eu
case "${1:-}" in
  -s) printf 'Linux\n' ;;
  -m) printf 'x86_64\n' ;;
  *) /usr/bin/uname "$@" ;;
esac
"#,
        );

        write_executable(
            &fakebin.join("curl"),
            r#"#!/usr/bin/env bash
set -eu
output=''
url=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output)
      output="$2"
      shift 2
      ;;
    --fail|--location|--silent|--show-error)
      shift
      ;;
    *)
      url="$1"
      shift
      ;;
  esac
done

printf '%s' "$url" > "${CC_SWITCH_TEST_LOG_DIR}/last-url"
if [ "${url##*/}" = "latest.json" ]; then
  printf '{\n  "version": "v0.0.0"\n}\n' > "$output"
  exit 0
fi
if [ "${CC_SWITCH_TEST_FAIL_MUSL:-0}" = "1" ] && [[ "${url##*/}" == *linux-x64-musl.tar.gz ]]; then
  exit 22
fi
cp "${CC_SWITCH_TEST_ARCHIVE_PATH}" "$output"
"#,
        );

        Self {
            _temp: temp,
            home,
            fakebin,
            install_dir,
            logs_dir,
            archive_path,
        }
    }

    fn run(&self, extra_envs: &[(&str, &str)], extra_path: Option<&Path>) -> Output {
        let mut path_parts = vec![self.fakebin.display().to_string()];
        if let Some(extra) = extra_path {
            path_parts.push(extra.display().to_string());
        }
        path_parts.push(std::env::var("PATH").unwrap_or_default());

        let mut command = Command::new("bash");
        command
            .arg(install_script_path())
            .env("HOME", &self.home)
            .env("CC_SWITCH_INSTALL_DIR", &self.install_dir)
            .env("CC_SWITCH_TEST_ARCHIVE_PATH", &self.archive_path)
            .env("CC_SWITCH_TEST_LOG_DIR", &self.logs_dir)
            .env("PATH", path_parts.join(":"));

        for (key, value) in extra_envs {
            command.env(key, value);
        }

        command.output().expect("install script should run")
    }
}

#[test]
#[serial]
fn install_script_requires_force_for_non_tty_overwrite() {
    let harness = Harness::new();
    write_executable(
        &harness.install_dir.join("cc-switch-tui"),
        "#!/usr/bin/env bash\necho old build\n",
    );

    let output = harness.run(&[], None);
    assert!(
        !output.status.success(),
        "overwrite should fail without force"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("CC_SWITCH_FORCE=1"), "stderr was: {stderr}");
}

#[test]
#[serial]
fn install_script_force_overwrites_and_warns_about_shadowed_path() {
    let harness = Harness::new();
    let shadow_dir = harness.home.join("shadow-bin");
    fs::create_dir_all(&shadow_dir).expect("shadow dir should exist");
    write_executable(
        &shadow_dir.join("cc-switch-tui"),
        "#!/usr/bin/env bash\necho shadow build\n",
    );
    write_executable(
        &harness.install_dir.join("cc-switch-tui"),
        "#!/usr/bin/env bash\necho old build\n",
    );

    let output = harness.run(&[("CC_SWITCH_FORCE", "1")], Some(&shadow_dir));
    assert!(
        output.status.success(),
        "force overwrite should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("shadow"), "stderr was: {stderr}");

    let installed = fs::read_to_string(harness.install_dir.join("cc-switch-tui"))
        .expect("installed file should exist");
    assert!(installed.contains("new build"));
}

#[test]
#[serial]
fn install_script_supports_linux_glibc_override() {
    let harness = Harness::new();

    let output = harness.run(&[("CC_SWITCH_LINUX_LIBC", "glibc")], None);
    assert!(
        output.status.success(),
        "glibc override install should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let requested_url = fs::read_to_string(harness.logs_dir.join("last-url"))
        .expect("download url should be logged");
    assert!(
        requested_url.ends_with("/cc-switch-tui-v0.0.0-linux-x64.tar.gz"),
        "expected glibc asset request, got {requested_url}"
    );
}

#[test]
#[serial]
fn install_script_falls_back_to_glibc_when_musl_download_fails() {
    let harness = Harness::new();

    let output = harness.run(&[("CC_SWITCH_TEST_FAIL_MUSL", "1")], None);
    assert!(
        output.status.success(),
        "glibc fallback install should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let requested_url = fs::read_to_string(harness.logs_dir.join("last-url"))
        .expect("download url should be logged");
    assert!(
        requested_url.ends_with("/cc-switch-tui-v0.0.0-linux-x64.tar.gz"),
        "expected fallback glibc asset request, got {requested_url}"
    );
}
