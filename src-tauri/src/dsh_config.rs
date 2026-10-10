//! DeepSeek Harness credential file adapter.
//!
//! DSH stores credentials in `~/.dsh/.credentials.yaml`.  Provider switching
//! must update only `refs.DEEPSEEK_API_KEY`; the file may also contain browser
//! grants and fields introduced by newer DSH versions.

use crate::config::get_home_dir;
use crate::error::AppError;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::{Duration, Instant};

pub fn get_dsh_dir() -> PathBuf {
    let configured = std::env::var_os("DSH_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().to_string_lossy().trim().is_empty())
        .unwrap_or_else(|| get_home_dir().join(".dsh"));
    let display = configured.as_os_str().to_string_lossy();
    let expanded = if display == "~" {
        get_home_dir()
    } else if display.starts_with("~/") || display.starts_with("~\\") {
        get_home_dir().join(&display[2..])
    } else {
        configured
    };
    let absolute = std::path::absolute(&expanded).unwrap_or(expanded);
    // Node's path.resolve removes parent segments before following symlinks.
    // Filesystem canonicalization would select a different store and also
    // require the credential directory to exist.
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            _ => resolved.push(component.as_os_str()),
        }
    }
    resolved
}

pub fn get_dsh_credentials_path() -> PathBuf {
    get_dsh_dir().join(".credentials.yaml")
}

pub fn read_dsh_credentials_source() -> Result<Option<String>, AppError> {
    let path = get_dsh_credentials_path();
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(&path)
        .map(Some)
        .map_err(|error| AppError::io(&path, error))
}

pub fn read_dsh_api_key() -> Result<Option<String>, AppError> {
    let Some(source) = read_dsh_credentials_source()? else {
        return Ok(None);
    };
    let value = parse_credentials(&source)?;
    validate_document(&value)?;
    Ok(value
        .get("refs")
        .and_then(|refs| refs.get("DEEPSEEK_API_KEY"))
        .and_then(serde_yaml::Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string))
}

fn parse_credentials(source: &str) -> Result<serde_yaml::Value, AppError> {
    let value: serde_yaml::Value = serde_yaml::from_str(source).map_err(|error| {
        // Parser diagnostics can include credential values. Report only the location.
        let location = error
            .location()
            .map(|location| format!(" at line {}, column {}", location.line(), location.column()))
            .unwrap_or_default();
        AppError::Config(format!("Failed to parse DSH credentials YAML{location}"))
    })?;
    // DSH treats empty and null YAML documents as an empty credential store.
    Ok(if value.is_null() {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    } else {
        value
    })
}

/// Cooperate with DSH's exclusive-create `<filename>.lock` writer protocol.
/// DSH can recover a stale lock using the holder PID after this process exits.
pub(crate) struct CredentialsWriteLock {
    path: PathBuf,
}

/// Serializes CC-Switch provider operations across processes independently of
/// the native credentials lock, so selection, database and publication agree.
pub(crate) struct DshProviderLock {
    _lock: CredentialsWriteLock,
}

pub(crate) fn acquire_provider_lock() -> Result<DshProviderLock, AppError> {
    let directory = get_dsh_dir();
    fs::create_dir_all(&directory).map_err(|error| AppError::io(&directory, error))?;
    Ok(DshProviderLock {
        _lock: CredentialsWriteLock::acquire_at(
            directory.join(".cc-switch-provider.lock"),
            Duration::from_secs(30),
            "Timed out waiting for the DSH provider operation lock",
        )?,
    })
}

fn create_lock_file(path: &std::path::Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn lock_holder_exited(record: &str) -> bool {
    let Some(digits) = record.strip_suffix('\n') else {
        return false;
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let Ok(pid) = digits.parse::<u32>() else {
        return false;
    };
    if pid == 0 || pid > i32::MAX as u32 || pid == std::process::id() {
        return false;
    }
    #[cfg(unix)]
    {
        (unsafe { libc::kill(pid as libc::pid_t, 0) }) != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::Foundation::{
            GetLastError, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return unsafe { GetLastError() } == ERROR_INVALID_PARAMETER;
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        (unsafe { WaitForSingleObject(handle.as_raw_handle().cast(), 0) }) == WAIT_OBJECT_0
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

fn recover_exited_lock(path: &std::path::Path) -> Result<bool, AppError> {
    recover_exited_lock_at_depth(path, 0)
}

fn recover_exited_lock_at_depth(path: &std::path::Path, depth: usize) -> Result<bool, AppError> {
    // Each recovery claim uses the same protocol. Bound recursion for malformed
    // stores while allowing recovery after crashes during claim recovery itself.
    if depth >= 16 {
        return Ok(false);
    }
    let Ok(record) = fs::read_to_string(path) else {
        return Ok(false);
    };
    if !lock_holder_exited(&record) {
        return Ok(false);
    }
    // Use the exact native claim name so simultaneous DSH and CC-Switch
    // contenders serialize their recheck before deleting a dead holder's lock.
    let digest = format!("{:x}", Sha256::digest(record.as_bytes()));
    let mut claim_name = path.as_os_str().to_os_string();
    claim_name.push(format!(".takeover-{}", &digest[..16]));
    let claim_path = PathBuf::from(claim_name);
    let mut file = loop {
        match create_lock_file(&claim_path) {
            Ok(file) => break file,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                if !recover_exited_lock_at_depth(&claim_path, depth + 1)? {
                    return Ok(false);
                }
            }
            Err(error) => return Err(AppError::io(&claim_path, error)),
        }
    };
    let _claim = CredentialsWriteLock { path: claim_path };
    writeln!(file, "{}", std::process::id()).map_err(|error| AppError::io(&_claim.path, error))?;
    drop(file);
    if fs::read_to_string(path).ok().as_deref() != Some(record.as_str())
        || !lock_holder_exited(&record)
    {
        return Ok(false);
    }
    Ok(fs::remove_file(path).is_ok())
}

impl CredentialsWriteLock {
    fn acquire(path: &std::path::Path, wait: Duration) -> Result<Self, AppError> {
        Self::acquire_at(
            path.with_file_name(".credentials.yaml.lock"),
            wait,
            "Timed out waiting for the DSH credentials writer lock",
        )
    }

    pub(crate) fn acquire_at(
        lock_path: PathBuf,
        wait: Duration,
        timeout_message: &str,
    ) -> Result<Self, AppError> {
        let deadline = Instant::now() + wait;
        let mut delay = Duration::from_millis(20);
        loop {
            match create_lock_file(&lock_path) {
                Ok(mut file) => {
                    let guard = Self { path: lock_path };
                    writeln!(file, "{}", std::process::id())
                        .map_err(|error| AppError::io(&guard.path, error))?;
                    return Ok(guard);
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::AlreadyExists
                        || (error.kind() == std::io::ErrorKind::PermissionDenied
                            && fs::symlink_metadata(&lock_path).is_ok()) =>
                {
                    if recover_exited_lock(&lock_path)? {
                        continue;
                    }
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(AppError::Config(timeout_message.to_string()));
                    }
                    std::thread::sleep(delay.min(remaining));
                    delay = (delay * 2).min(Duration::from_millis(200));
                }
                Err(error) => return Err(AppError::io(&lock_path, error)),
            }
        }
    }
}

impl Drop for CredentialsWriteLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn validate_document(value: &serde_yaml::Value) -> Result<(), AppError> {
    let Some(map) = value.as_mapping() else {
        return Err(AppError::Config(
            "DSH credentials YAML root must be a mapping".to_string(),
        ));
    };
    if map.is_empty() {
        return Ok(());
    }
    let version = map
        .get(serde_yaml::Value::String("version".to_string()))
        .and_then(serde_yaml::Value::as_f64)
        .ok_or_else(|| AppError::Config("DSH credentials YAML requires version: 1".to_string()))?;
    // Native DSH parses YAML numbers as JavaScript numbers, including 1.0.
    if version != 1.0 {
        return Err(AppError::Config(format!(
            "Unsupported DSH credentials YAML version: {version}"
        )));
    }
    if let Some(refs) = map.get(serde_yaml::Value::String("refs".to_string())) {
        if !refs.is_null() && !refs.is_mapping() {
            return Err(AppError::Config(
                "DSH credentials YAML refs must be a mapping".to_string(),
            ));
        }
    }
    Ok(())
}

pub fn validate_dsh_credentials_source(source: Option<&str>) -> Result<(), AppError> {
    let Some(source) = source else {
        return Ok(());
    };
    let value = parse_credentials(source)?;
    validate_document(&value)
}

pub fn write_dsh_api_key(api_key: &str) -> Result<(), AppError> {
    write_dsh_credential_ref("DEEPSEEK_API_KEY", api_key)
}

pub(crate) fn read_dsh_credential_ref(reference: &str) -> Result<Option<String>, AppError> {
    let Some(source) = read_dsh_credentials_source()? else {
        return Ok(None);
    };
    let value = parse_credentials(&source)?;
    validate_document(&value)?;
    Ok(value
        .get("refs")
        .and_then(|refs| refs.get(reference))
        .and_then(serde_yaml::Value::as_str)
        .map(str::to_string))
}

pub(crate) fn write_dsh_credential_ref(reference: &str, api_key: &str) -> Result<(), AppError> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(AppError::InvalidInput(
            "DeepSeek Harness API key cannot be empty".to_string(),
        ));
    }

    let path = get_dsh_credentials_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;
    }
    // Hold the native lock across the entire read-modify-write operation.
    let _lock = CredentialsWriteLock::acquire(&path, Duration::from_secs(30))?;
    let source = fs::read_to_string(&path)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok(String::new())
            } else {
                Err(error)
            }
        })
        .map_err(|error| AppError::io(&path, error))?;
    let rendered = update_credentials_ref_source(&source, reference, api_key)?;

    // Set permissions before publication and perform no fallible work after it.
    // Thus an error always leaves the live document intact on every platform.
    let mut temporary = tempfile::Builder::new()
        .prefix(".credentials.")
        .suffix(".tmp")
        .tempfile_in(path.parent().expect("credentials have a parent"))
        .map_err(|error| AppError::io(&path, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| AppError::io(&path, error))?;
    }
    temporary
        .write_all(rendered.as_bytes())
        .map_err(|error| AppError::io(&path, error))?;
    temporary
        .flush()
        .map_err(|error| AppError::io(&path, error))?;
    temporary
        .persist(&path)
        .map_err(|error| AppError::io(&path, error.error))?;
    Ok(())
}

#[cfg(test)]
fn update_credentials_source(source: &str, api_key: &str) -> Result<String, AppError> {
    update_credentials_ref_source(source, "DEEPSEEK_API_KEY", api_key)
}

fn update_credentials_ref_source(
    source: &str,
    reference: &str,
    api_key: &str,
) -> Result<String, AppError> {
    let mut root = parse_credentials(source)?;

    validate_document(&root)?;
    let root_map = root.as_mapping_mut().ok_or_else(|| {
        AppError::Config("DSH credentials YAML root must be a mapping".to_string())
    })?;
    root_map
        .entry(serde_yaml::Value::String("version".to_string()))
        .or_insert_with(|| serde_yaml::Value::Number(serde_yaml::Number::from(1)));
    let refs_key = serde_yaml::Value::String("refs".to_string());
    let refs = root_map
        .entry(refs_key)
        .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    if refs.is_null() {
        *refs = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }
    let refs_map = refs.as_mapping_mut().ok_or_else(|| {
        AppError::Config("DSH credentials YAML refs must be a mapping".to_string())
    })?;
    refs_map.insert(
        serde_yaml::Value::String(reference.to_string()),
        serde_yaml::Value::String(api_key.to_string()),
    );

    // Editing the syntax tree keeps untouched scalars in their original form.
    // Re-serializing serde_yaml values can change YAML 1.2 types used by DSH
    // (for example, a grant payload's `012345` number becomes a string).
    let empty = parse_credentials(source)?
        .as_mapping()
        .is_some_and(|map| map.is_empty());
    let input = if empty {
        "version: 1\nrefs: {}\n"
    } else {
        source
    };
    let file = yaml_edit::YamlFile::from_str(input)
        .map_err(|_| AppError::Config("Cannot safely edit DSH credentials YAML".into()))?;
    let document = file
        .document()
        .ok_or_else(|| AppError::Config("DSH credentials YAML requires a document".into()))?;
    let mapping = document
        .as_mapping()
        .ok_or_else(|| AppError::Config("DSH credentials YAML root must be a mapping".into()))?;
    if mapping.get_mapping("refs").is_none() {
        mapping.set("refs", yaml_edit::YamlValue::mapping());
    }
    let refs = mapping
        .get_mapping("refs")
        .ok_or_else(|| AppError::Config("Cannot safely edit DSH credentials refs".into()))?;
    let rendered = if let Some(existing) = refs.get(reference) {
        // Preserve the terminating line break of a block scalar. Replacing its
        // syntax node through the editor can otherwise join the following key
        // onto the replacement value.
        let range = yaml_edit::AsYaml::as_node(&existing)
            .ok_or_else(|| AppError::Config("Cannot locate DSH API key YAML node".into()))?
            .text_range();
        let start = u32::from(range.start()) as usize;
        let end = u32::from(range.end()) as usize;
        let mut rendered = file.to_string();
        let old = rendered
            .get(start..end)
            .ok_or_else(|| AppError::Config("Cannot locate DSH API key YAML node".into()))?;
        let mut quoted = serde_json::to_string(api_key)
            .map_err(|_| AppError::Config("Cannot encode DSH API key".into()))?;
        if old.ends_with("\r\n") {
            quoted.push_str("\r\n");
        } else if old.ends_with('\n') {
            quoted.push('\n');
        }
        rendered.replace_range(start..end, &quoted);
        rendered
    } else {
        refs.set(reference, yaml_edit::ScalarValue::double_quoted(api_key));
        file.to_string()
    };
    // Fail before publication if an unsupported syntax or alias changes another
    // value, or if insertion fails to encode the requested string exactly.
    if parse_credentials(&rendered)? != root {
        return Err(AppError::Config(
            "DSH credentials edit would change other values".into(),
        ));
    }
    Ok(rendered)
}

pub fn dsh_api_key_settings(api_key: &str) -> Value {
    serde_json::json!({ "apiKey": api_key })
}

pub fn validate_dsh_provider_settings(settings: &Value) -> Result<(), AppError> {
    let settings = settings.as_object().ok_or_else(|| {
        AppError::InvalidInput("DSH provider configuration must be an object".into())
    })?;
    let api_key = settings
        .get("apiKey")
        .and_then(Value::as_str)
        .map(str::trim);
    if api_key.is_none_or(str::is_empty) {
        return Err(AppError::InvalidInput(
            "DSH provider requires a non-empty apiKey".into(),
        ));
    }
    crate::dsh_provider_config::validate_settings(&Value::Object(settings.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn recovers_exited_holder_using_native_claim_protocol() {
        let home = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(home.path());
        let path = get_dsh_credentials_path();
        write_dsh_api_key("old").unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        let record = format!("{pid}\n");
        assert!(lock_holder_exited(&record));
        let lock_path = path.with_file_name(".credentials.yaml.lock");
        fs::write(&lock_path, &record).unwrap();
        let digest = format!("{:x}", Sha256::digest(record.as_bytes()));
        let claim_path =
            path.with_file_name(format!(".credentials.yaml.lock.takeover-{}", &digest[..16]));
        // A native contender's claim prevents us from removing the stale lock.
        fs::write(&claim_path, format!("{}\n", std::process::id())).unwrap();
        assert!(!recover_exited_lock(&lock_path).unwrap());
        assert_eq!(fs::read_to_string(&lock_path).unwrap(), record);
        fs::remove_file(&claim_path).unwrap();
        write_dsh_api_key("new").unwrap();
        assert_eq!(read_dsh_api_key().unwrap().as_deref(), Some("new"));
        assert!(!lock_path.exists());
        assert!(!claim_path.exists());
    }

    #[test]
    fn malformed_or_live_holder_records_are_never_reclaimed() {
        for record in [
            "",
            "0\n",
            "-1\n",
            "2147483648\n",
            "1",
            "1\n\n",
            "\n",
            "key-sentinel\n",
        ] {
            assert!(!lock_holder_exited(record));
        }
        assert!(!lock_holder_exited(&format!("{}\n", std::process::id())));
    }

    #[test]
    fn recovers_abandoned_claims_for_native_and_provider_locks() {
        let home = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(home.path());
        fs::create_dir_all(get_dsh_dir()).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let record = format!("{}\n", child.id());
        child.wait().unwrap();
        assert!(lock_holder_exited(&record));
        let digest = format!("{:x}", Sha256::digest(record.as_bytes()));
        for filename in [".credentials.yaml.lock", ".cc-switch-provider.lock"] {
            let lock_path = get_dsh_dir().join(filename);
            let claim_path = get_dsh_dir().join(format!("{filename}.takeover-{}", &digest[..16]));
            let nested_claim = PathBuf::from(format!(
                "{}.takeover-{}",
                claim_path.display(),
                &digest[..16]
            ));
            for path in [&lock_path, &claim_path, &nested_claim] {
                fs::write(path, &record).unwrap();
            }
            let lock = CredentialsWriteLock::acquire_at(
                lock_path.clone(),
                Duration::ZERO,
                "test lock timed out",
            )
            .expect("abandoned claims must not block acquisition");
            assert_eq!(
                fs::read_to_string(&lock_path).unwrap(),
                format!("{}\n", std::process::id())
            );
            assert!(!claim_path.exists());
            assert!(!nested_claim.exists());
            drop(lock);
            assert!(!lock_path.exists());
        }
    }

    #[test]
    fn resolves_native_home_overrides() {
        let home = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(home.path());
        for (configured, expected) in [
            ("", home.path().join(".dsh")),
            (" \t\n ", home.path().join(".dsh")),
            ("~", home.path().to_path_buf()),
            ("~/custom", home.path().join("custom")),
            ("~/missing/../custom", home.path().join("custom")),
            ("~\\custom", home.path().join("custom")),
            (
                "relative-dsh",
                std::env::current_dir().unwrap().join("relative-dsh"),
            ),
        ] {
            std::env::set_var("DSH_HOME", configured);
            assert_eq!(get_dsh_dir(), expected);
        }
    }

    #[cfg(unix)]
    #[test]
    fn resolves_parent_segments_before_following_symlinks() {
        let home = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(home.path());
        let lexical_parent = home.path().join("a");
        let target = home.path().join("other/target");
        fs::create_dir_all(&lexical_parent).unwrap();
        fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, lexical_parent.join("link")).unwrap();
        std::env::set_var("DSH_HOME", lexical_parent.join("link/../dsh"));
        assert_eq!(get_dsh_dir(), lexical_parent.join("dsh"));
        write_dsh_api_key("synthetic-key").unwrap();
        assert!(lexical_parent.join("dsh/.credentials.yaml").exists());
        assert!(!home.path().join("other/dsh").exists());
    }

    #[test]
    fn preserves_untouched_yaml_syntax_and_native_scalar_types() {
        let records = "records:\n  client-connection/test:\n    kind: grant\n    payload:\n      padded: 012345\n      exponent: 1e3\n      yes: yes\n      text: '012345'\n      nested: { value: 012345 } # keep comment\n";
        let source = format!("# credentials\n---\nversion: 1.0\nrefs:\n  DEEPSEEK_API_KEY: old\n  OTHER: 'keep'\n{records}...\n");
        let rendered = update_credentials_source(&source, "new").unwrap();
        assert!(rendered.contains(records));
        assert!(rendered.starts_with("# credentials\n---\nversion: 1.0\n"));
        assert!(rendered.ends_with("...\n"));
        assert!(rendered.contains("OTHER: 'keep'"));
        assert_eq!(
            parse_credentials(&rendered).unwrap()["refs"]["DEEPSEEK_API_KEY"].as_str(),
            Some("new")
        );
    }

    #[test]
    fn edits_flow_and_block_credentials_and_quotes_api_key_strings() {
        for source in [
            "{version: 1, refs: {DEEPSEEK_API_KEY: old, OTHER: keep}, records: {}}\n",
            "version: 1\nrefs: null\nrecords: {}\n",
            "version: 1\nrecords: {}\n",
            "version: 1\nrefs:\n  'DEEPSEEK_API_KEY': |\n    old\nrecords: {}\n",
        ] {
            for key in ["012345", "null", "yes", "colon: #quote\"\\\n键"] {
                let rendered = update_credentials_source(source, key).unwrap();
                assert_eq!(
                    parse_credentials(&rendered).unwrap()["refs"]["DEEPSEEK_API_KEY"].as_str(),
                    Some(key)
                );
            }
        }
    }

    #[test]
    fn initializes_native_empty_documents() {
        let home = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(home.path());
        let path = get_dsh_credentials_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for source in ["", " \n", "# empty credentials\n", "null\n", "{}\n"] {
            fs::write(&path, source).unwrap();
            assert_eq!(read_dsh_api_key().unwrap(), None);
            write_dsh_api_key("new").unwrap();
            assert_eq!(read_dsh_api_key().unwrap().as_deref(), Some("new"));
            assert!(!path.with_file_name(".credentials.yaml.lock").exists());
        }
    }

    #[test]
    fn writer_waits_for_native_lock_then_preserves_latest_records() {
        let home = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(home.path());
        let path = get_dsh_credentials_path();
        write_dsh_api_key("old").unwrap();
        let lock_path = path.with_file_name(".credentials.yaml.lock");
        // Simulate DSH's exclusive-create lock and an in-flight grant refresh.
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .unwrap();
        drop(file);
        fs::write(&lock_path, format!("{}\n", std::process::id())).unwrap();
        let (sent, received) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            sent.send(write_dsh_api_key("new")).unwrap();
        });
        assert!(matches!(
            received.recv_timeout(Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        let refreshed = "version: 1\nrefs:\n  DEEPSEEK_API_KEY: old\nrecords:\n  browser-session:\n    kind: grant\n    payload:\n      secret: refreshed\n";
        fs::write(&path, refreshed).unwrap();
        fs::remove_file(&lock_path).unwrap();
        received
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        writer.join().unwrap();
        let value: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["refs"]["DEEPSEEK_API_KEY"].as_str(), Some("new"));
        assert_eq!(
            value["records"]["browser-session"]["payload"]["secret"].as_str(),
            Some("refreshed")
        );
        assert!(!lock_path.exists());
    }

    #[test]
    fn contended_lock_times_out_without_removing_holder_or_changing_credentials() {
        let home = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(home.path());
        let path = get_dsh_credentials_path();
        write_dsh_api_key("old").unwrap();
        let source = fs::read(&path).unwrap();
        let lock = CredentialsWriteLock::acquire(&path, Duration::ZERO).unwrap();
        let lock_source = fs::read(&lock.path).unwrap();
        assert!(CredentialsWriteLock::acquire(&path, Duration::from_millis(10)).is_err());
        assert_eq!(fs::read(&path).unwrap(), source);
        assert_eq!(fs::read(&lock.path).unwrap(), lock_source);
        drop(lock);
        assert!(!path.with_file_name(".credentials.yaml.lock").exists());
    }

    #[test]
    fn updates_key_without_dropping_grant_or_unknown_fields() {
        let home = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(home.path());
        let path = home.path().join(".dsh/.credentials.yaml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "version: 1\nrefs:\n  DEEPSEEK_API_KEY: old\nfuture: keep\nrecords:\n  client-connection/browser-session:\n    kind: grant\n    payload:\n      secret: preserve\n",
        )
        .unwrap();

        write_dsh_api_key("new").unwrap();
        let source = fs::read_to_string(&path).unwrap();
        let value: serde_yaml::Value = serde_yaml::from_str(&source).unwrap();
        assert_eq!(read_dsh_api_key().unwrap().as_deref(), Some("new"));
        assert_eq!(value["future"].as_str(), Some("keep"));
        assert_eq!(
            value["records"]["client-connection/browser-session"]["payload"]["secret"].as_str(),
            Some("preserve")
        );
    }

    #[test]
    fn creates_versioned_document_when_missing() {
        let home = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(home.path());
        write_dsh_api_key("key").unwrap();
        let source = fs::read_to_string(get_dsh_credentials_path()).unwrap();
        assert!(source.contains("version: 1"));
        assert_eq!(read_dsh_api_key().unwrap().as_deref(), Some("key"));
    }

    #[test]
    fn accepts_native_numeric_version_representations() {
        let home = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(home.path());
        let path = get_dsh_credentials_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for version in ["1", "1.0", "1.00", "1e0"] {
            fs::write(
                &path,
                format!("version: {version}\nrefs:\n  DEEPSEEK_API_KEY: old\nrecords: {{}}\n"),
            )
            .unwrap();
            assert_eq!(read_dsh_api_key().unwrap().as_deref(), Some("old"));
            write_dsh_api_key("new").unwrap();
            assert_eq!(read_dsh_api_key().unwrap().as_deref(), Some("new"));
        }
    }

    #[test]
    fn rejects_unversioned_nonempty_document_without_writing() {
        let home = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(home.path());
        let path = get_dsh_credentials_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "DEEPSEEK_API_KEY: old\n").unwrap();
        assert!(write_dsh_api_key("new").is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "DEEPSEEK_API_KEY: old\n");
    }

    #[test]
    fn rejects_invalid_documents_without_writing_or_disclosing_credentials() {
        let home = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(home.path());
        let path = get_dsh_credentials_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for source in [
            "version: 2\nrefs: {}\n",
            "version: 1.5\nrefs: {}\n",
            "version: '1.0'\nrefs: {}\n",
            "version: true\nrefs: {}\n",
            "version: .nan\nrefs: {}\n",
            "version: .inf\nrefs: {}\n",
            "version: 1\nrefs: credential-sentinel\n",
            "version: 1\nrefs: [credential-sentinel\n",
            "- credential-sentinel\n",
        ] {
            fs::write(&path, source).unwrap();
            for result in [read_dsh_api_key().map(|_| ()), write_dsh_api_key("new")] {
                let error = result.expect_err("invalid credentials must be rejected");
                assert!(!error.to_string().contains("credential-sentinel"));
            }
            assert_eq!(fs::read_to_string(&path).unwrap(), source);
            assert!(!path.with_file_name(".credentials.yaml.lock").exists());
        }
        assert!(write_dsh_api_key(" \n ").is_err());
    }

    #[test]
    fn uses_dsh_home_and_initializes_null_refs() {
        let home = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(home.path());
        let dsh_home = home.path().join("custom-dsh");
        std::env::set_var("DSH_HOME", &dsh_home);
        let path = dsh_home.join(".credentials.yaml");
        fs::create_dir_all(&dsh_home).unwrap();
        fs::write(&path, "version: 1\nrefs: null\nrecords: {}\n").unwrap();
        assert_eq!(read_dsh_api_key().unwrap(), None);
        write_dsh_api_key(" key ").unwrap();
        assert_eq!(read_dsh_api_key().unwrap().as_deref(), Some("key"));
        assert!(!home.path().join(".dsh/.credentials.yaml").exists());
    }

    #[cfg(unix)]
    #[test]
    fn creates_and_replaces_credentials_with_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let home = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(home.path());
        let path = get_dsh_credentials_path();
        write_dsh_api_key("initial").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write_dsh_api_key("replacement").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
