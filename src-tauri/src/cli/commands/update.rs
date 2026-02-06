use clap::Args;
use flate2::read::GzDecoder;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tar::Archive;
use tempfile::TempDir;
use url::Url;

use crate::cli::ui::{highlight, info, success};
use crate::error::AppError;

const REPO_URL: &str = env!("CARGO_PKG_REPOSITORY");
const BINARY_NAME: &str = "cc-switch";
const CHECKSUMS_FILE_NAME: &str = "checksums.txt";
const HTTP_REQUEST_TIMEOUT_SECS: u64 = 30;
const MAX_RELEASE_ASSET_SIZE_BYTES: u64 = 100 * 1024 * 1024;
const USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    "-updater/",
    env!("CARGO_PKG_VERSION")
);

#[derive(Args, Debug, Clone)]
pub struct UpdateCommand {
    /// Target version (example: v4.6.2). Defaults to latest release.
    #[arg(long)]
    pub version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReleaseInfo {
    tag_name: String,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize, Clone)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

struct DownloadedAsset {
    _temp_dir: TempDir,
    archive_path: PathBuf,
}

pub fn execute(cmd: UpdateCommand) -> Result<(), AppError> {
    let runtime = create_runtime()?;
    runtime.block_on(execute_async(cmd))
}

async fn execute_async(cmd: UpdateCommand) -> Result<(), AppError> {
    let current_version = env!("CARGO_PKG_VERSION");
    let explicit_version = cmd.version.as_deref().is_some_and(|v| !v.trim().is_empty());
    let client = create_http_client()?;
    let target_tag = resolve_target_tag(&client, cmd.version.as_deref()).await?;
    let target_version = target_tag.trim_start_matches('v');

    if target_version == current_version {
        println!(
            "{}",
            info(&format!("Already on latest version: v{current_version}"))
        );
        return Ok(());
    }

    if should_skip_implicit_downgrade(current_version, target_version, explicit_version) {
        println!(
            "{}",
            info(&format!(
                "Current version v{current_version} is newer than target {target_tag}; skipping automatic downgrade. Use `cc-switch update --version {target_tag}` to force."
            ))
        );
        return Ok(());
    }

    let expected_asset_name = release_asset_name()?;
    let release = fetch_release_by_tag(&client, &target_tag).await?;
    let release_asset = select_release_asset(&release.assets, &target_tag, &expected_asset_name)
        .ok_or_else(|| {
            AppError::Message(format!(
                "Release {target_tag} does not include expected asset '{expected_asset_name}' (or compatible tagged variant)."
            ))
        })?;
    let download_url = release_asset.browser_download_url.as_str();

    println!(
        "{}",
        highlight(&format!("Current version: v{current_version}"))
    );
    println!("{}", highlight(&format!("Updating to: {target_tag}")));
    println!("{}", info(&format!("Downloading: {download_url}")));
    if release_asset.digest.is_some() {
        println!(
            "{}",
            info("Verifying checksum from release metadata digest.")
        );
    } else {
        let checksum_url =
            format!("{REPO_URL}/releases/download/{target_tag}/{CHECKSUMS_FILE_NAME}");
        println!("{}", info(&format!("Verifying checksum: {checksum_url}")));
    }

    let downloaded_asset =
        download_release_asset(&client, download_url, release_asset.name.as_str(), None).await?;
    verify_asset_checksum(
        &client,
        &downloaded_asset.archive_path,
        &target_tag,
        release_asset,
    )
    .await?;
    let extracted_binary = extract_binary(&downloaded_asset.archive_path)?;
    replace_current_binary(&extracted_binary)?;

    println!(
        "{}",
        success(&format!("Updated successfully to {target_tag}"))
    );
    println!(
        "{}",
        info("Run `cc-switch --version` to verify the installed version.")
    );
    Ok(())
}

fn create_runtime() -> Result<tokio::runtime::Runtime, AppError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("Failed to create runtime: {e}")))
}

fn create_http_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(HTTP_REQUEST_TIMEOUT_SECS))
        .build()
        .map_err(|e| AppError::Message(format!("Failed to initialize HTTP client: {e}")))
}

async fn resolve_target_tag(
    client: &reqwest::Client,
    version: Option<&str>,
) -> Result<String, AppError> {
    let tag = match version.map(str::trim).filter(|v| !v.is_empty()) {
        Some(version) => normalize_tag(version),
        None => fetch_latest_release_tag(client).await?,
    };
    validate_target_tag(&tag)?;
    Ok(tag)
}

fn validate_target_tag(tag: &str) -> Result<(), AppError> {
    if !tag.starts_with('v') {
        return Err(AppError::Message(format!(
            "Invalid version tag '{tag}': must start with 'v'."
        )));
    }
    if tag.len() > 64 {
        return Err(AppError::Message(format!(
            "Invalid version tag '{tag}': too long."
        )));
    }
    if tag.contains('/') || tag.contains('\\') || tag.contains("..") {
        return Err(AppError::Message(format!(
            "Invalid version tag '{tag}': contains forbidden path characters."
        )));
    }
    if !tag
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_')
    {
        return Err(AppError::Message(format!(
            "Invalid version tag '{tag}': only [A-Za-z0-9._-] allowed."
        )));
    }
    Ok(())
}

fn normalize_tag(version: &str) -> String {
    if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{version}")
    }
}

async fn fetch_latest_release_tag(client: &reqwest::Client) -> Result<String, AppError> {
    let api_url = release_api_url(REPO_URL, "latest")?;
    let release = client
        .get(api_url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(|e| AppError::Message(format!("Failed to query latest release: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Message(format!("Release API returned error: {e}")))?
        .json::<ReleaseInfo>()
        .await
        .map_err(|e| AppError::Message(format!("Failed to parse latest release response: {e}")))?;
    Ok(release.tag_name)
}

async fn fetch_release_by_tag(
    client: &reqwest::Client,
    tag: &str,
) -> Result<ReleaseInfo, AppError> {
    let api_url = release_api_url(REPO_URL, &format!("tags/{tag}"))?;
    client
        .get(api_url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(|e| AppError::Message(format!("Failed to query release {tag}: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Message(format!("Release API returned error for {tag}: {e}")))?
        .json::<ReleaseInfo>()
        .await
        .map_err(|e| AppError::Message(format!("Failed to parse release response for {tag}: {e}")))
}

fn release_api_url(repo_url: &str, suffix: &str) -> Result<Url, AppError> {
    let repo_url = Url::parse(repo_url)
        .map_err(|e| AppError::Message(format!("Invalid repository URL '{repo_url}': {e}")))?;
    let host = repo_url
        .host_str()
        .ok_or_else(|| AppError::Message(format!("Repository URL is missing host: {repo_url}")))?;

    let path = repo_url.path().trim_matches('/');
    let mut parts = path.split('/');
    let owner = parts.next().filter(|s| !s.is_empty()).ok_or_else(|| {
        AppError::Message(format!(
            "Repository URL must include owner and repo: {repo_url}"
        ))
    })?;
    let repo = parts.next().filter(|s| !s.is_empty()).ok_or_else(|| {
        AppError::Message(format!(
            "Repository URL must include owner and repo: {repo_url}"
        ))
    })?;
    if parts.next().is_some() {
        return Err(AppError::Message(format!(
            "Repository URL must be in '<host>/<owner>/<repo>' format: {repo_url}"
        )));
    }
    let repo = repo.strip_suffix(".git").unwrap_or(repo);

    let api_path = if host == "github.com" {
        format!("/repos/{owner}/{repo}/releases/{suffix}")
    } else {
        format!("/api/v3/repos/{owner}/{repo}/releases/{suffix}")
    };

    let mut api_url = repo_url.clone();
    if host == "github.com" {
        api_url
            .set_host(Some("api.github.com"))
            .map_err(|_| AppError::Message("Failed to set GitHub API host.".to_string()))?;
    }
    api_url.set_path(&api_path);
    api_url.set_query(None);
    api_url.set_fragment(None);

    Ok(api_url)
}

fn select_release_asset<'a>(
    assets: &'a [ReleaseAsset],
    target_tag: &str,
    expected_asset_name: &str,
) -> Option<&'a ReleaseAsset> {
    let tagged_variant = tagged_asset_name(target_tag, expected_asset_name);

    assets
        .iter()
        .find(|asset| asset.name == expected_asset_name)
        .or_else(|| assets.iter().find(|asset| asset.name == tagged_variant))
}

fn tagged_asset_name(tag: &str, asset_name: &str) -> String {
    if let Some(suffix) = asset_name.strip_prefix("cc-switch-cli-") {
        return format!("cc-switch-cli-{tag}-{suffix}");
    }
    asset_name.to_string()
}

fn release_asset_name() -> Result<String, AppError> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let name = match (os, arch) {
        ("macos", "x86_64") | ("macos", "aarch64") => "cc-switch-cli-darwin-universal.tar.gz",
        ("linux", "x86_64") => "cc-switch-cli-linux-x64-musl.tar.gz",
        ("linux", "aarch64") => "cc-switch-cli-linux-arm64-musl.tar.gz",
        ("windows", "x86_64") => "cc-switch-cli-windows-x64.zip",
        _ => {
            return Err(AppError::Message(format!(
                "Self-update is not supported for platform {os}/{arch}."
            )));
        }
    };

    Ok(name.to_string())
}

async fn download_release_asset(
    client: &reqwest::Client,
    url: &str,
    asset_name: &str,
    on_progress: Option<&dyn Fn(u64, Option<u64>)>,
) -> Result<DownloadedAsset, AppError> {
    let mut response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(|e| AppError::Message(format!("Failed to download release asset: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Message(format!("Release asset request failed: {e}")))?;
    let content_length = response.content_length();
    if let Some(cl) = content_length {
        validate_download_size_limit(cl, asset_name)?;
    }

    let temp_dir = tempfile::tempdir()
        .map_err(|e| AppError::Message(format!("Failed to create temp directory: {e}")))?;
    let file_name = sanitized_asset_file_name(asset_name)?;
    let archive_path = temp_dir.path().join(file_name);
    let mut output = fs::File::create(&archive_path).map_err(|e| AppError::io(&archive_path, e))?;
    let mut downloaded_bytes = 0_u64;
    let mut last_reported = 0_u64;

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| AppError::Message(format!("Failed to read release asset chunk: {e}")))?
    {
        downloaded_bytes = downloaded_bytes.saturating_add(chunk.len() as u64);
        validate_download_size_limit(downloaded_bytes, asset_name)?;
        output
            .write_all(&chunk)
            .map_err(|e| AppError::io(&archive_path, e))?;

        if let Some(cb) = on_progress {
            if downloaded_bytes - last_reported >= 64 * 1024 {
                cb(downloaded_bytes, content_length);
                last_reported = downloaded_bytes;
            }
        }
    }

    if let Some(cb) = on_progress {
        cb(downloaded_bytes, content_length);
    }

    Ok(DownloadedAsset {
        _temp_dir: temp_dir,
        archive_path,
    })
}

fn sanitized_asset_file_name(asset_name: &str) -> Result<&str, AppError> {
    Path::new(asset_name)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::Message(format!("Invalid asset name: {asset_name}")))
}

fn validate_download_size_limit(size_bytes: u64, asset_name: &str) -> Result<(), AppError> {
    if size_bytes <= MAX_RELEASE_ASSET_SIZE_BYTES {
        return Ok(());
    }
    let max_mb = MAX_RELEASE_ASSET_SIZE_BYTES / (1024 * 1024);
    let size_mb = size_bytes / (1024 * 1024);
    Err(AppError::Message(format!(
        "Release asset '{asset_name}' is too large ({size_mb} MB). Maximum allowed size is {max_mb} MB."
    )))
}

async fn verify_asset_checksum(
    client: &reqwest::Client,
    archive_path: &Path,
    target_tag: &str,
    release_asset: &ReleaseAsset,
) -> Result<(), AppError> {
    let actual = compute_sha256_hex(archive_path)?;

    let expected = if let Some(expected) = release_asset
        .digest
        .as_deref()
        .and_then(parse_sha256_digest)
    {
        expected
    } else {
        let checksum_url =
            format!("{REPO_URL}/releases/download/{target_tag}/{CHECKSUMS_FILE_NAME}");
        let checksum_content = download_text(client, &checksum_url).await?;
        parse_checksum_for_asset(&checksum_content, release_asset.name.as_str())?
    };

    if actual != expected {
        return Err(AppError::Message(format!(
            "Checksum mismatch for {}: expected {expected}, got {actual}.",
            release_asset.name
        )));
    }

    Ok(())
}

fn compute_sha256_hex(path: &Path) -> Result<String, AppError> {
    let mut file = fs::File::open(path).map_err(|e| AppError::io(path, e))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| AppError::io(path, e))?;

    Ok(format!("{:x}", hasher.finalize()))
}

fn should_skip_implicit_downgrade(
    current_version: &str,
    target_version: &str,
    explicit_version: bool,
) -> bool {
    if explicit_version {
        return false;
    }
    let Ok(current) = Version::parse(current_version) else {
        return false;
    };
    let Ok(target) = Version::parse(target_version) else {
        return false;
    };
    target < current
}

async fn download_text(client: &reqwest::Client, url: &str) -> Result<String, AppError> {
    client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(|e| AppError::Message(format!("Failed to download checksum file: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Message(format!("Checksum file request failed: {e}")))?
        .text()
        .await
        .map_err(|e| AppError::Message(format!("Failed to read checksum file body: {e}")))
}

fn parse_checksum_for_asset(checksum_content: &str, asset_name: &str) -> Result<String, AppError> {
    let expected = checksum_content
        .lines()
        .filter_map(|line| {
            let line = line.trim_end();
            if line.is_empty() {
                return None;
            }

            let (hash, file) = parse_sha256sum_line(line)?;

            if file == asset_name {
                Some(hash.to_ascii_lowercase())
            } else {
                None
            }
        })
        .next();

    expected.ok_or_else(|| {
        AppError::Message(format!(
            "Unable to find SHA256 for {asset_name} in {CHECKSUMS_FILE_NAME}."
        ))
    })
}

fn parse_sha256sum_line(line: &str) -> Option<(&str, &str)> {
    // sha256sum output format:
    // - text mode:   "<64-hex>  <filename>"
    // - binary mode: "<64-hex> *<filename>"
    if line.len() < 66 {
        return None;
    }

    let (hash, remainder) = line.split_at(64);
    if !hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }

    if let Some(file) = remainder
        .strip_prefix("  ")
        .or_else(|| remainder.strip_prefix(" *"))
    {
        return Some((hash, file));
    }

    None
}

fn parse_sha256_digest(digest: &str) -> Option<String> {
    let digest = digest.strip_prefix("sha256:")?;
    if digest.len() != 64 || !digest.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    Some(digest.to_ascii_lowercase())
}

fn extract_binary(archive_path: &Path) -> Result<PathBuf, AppError> {
    let extract_dir = archive_path
        .parent()
        .ok_or_else(|| AppError::Message("Invalid archive path".to_string()))?
        .join("extracted");
    fs::create_dir_all(&extract_dir).map_err(|e| AppError::io(&extract_dir, e))?;

    if cfg!(windows) {
        extract_zip_binary(archive_path, &extract_dir)
    } else {
        extract_tar_binary(archive_path, &extract_dir)
    }
}

#[cfg(not(windows))]
fn extract_tar_binary(archive_path: &Path, extract_dir: &Path) -> Result<PathBuf, AppError> {
    let file = fs::File::open(archive_path).map_err(|e| AppError::io(archive_path, e))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|e| AppError::Message(format!("Failed to read archive entries: {e}")))?;

    for entry in entries {
        let mut entry =
            entry.map_err(|e| AppError::Message(format!("Failed to read archive entry: {e}")))?;

        if !entry.header().entry_type().is_file() {
            continue;
        }

        let entry_path = entry
            .path()
            .map_err(|e| AppError::Message(format!("Failed to inspect archive entry path: {e}")))?;
        if entry_path.file_name().and_then(|name| name.to_str()) != Some(BINARY_NAME) {
            continue;
        }

        let binary_path = extract_dir.join(BINARY_NAME);
        let mut output =
            fs::File::create(&binary_path).map_err(|e| AppError::io(&binary_path, e))?;
        std::io::copy(&mut entry, &mut output)
            .map_err(|e| AppError::Message(format!("Failed to unpack binary from TAR: {e}")))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o755);
            fs::set_permissions(&binary_path, perms).map_err(|e| AppError::io(&binary_path, e))?;
        }

        return Ok(binary_path);
    }

    Err(AppError::Message(format!(
        "Extracted archive does not contain expected binary: {BINARY_NAME}"
    )))
}

#[cfg(not(windows))]
fn extract_zip_binary(_archive_path: &Path, _extract_dir: &Path) -> Result<PathBuf, AppError> {
    Err(AppError::Message(
        "ZIP extraction is only supported on Windows.".to_string(),
    ))
}

#[cfg(windows)]
fn extract_zip_binary(archive_path: &Path, extract_dir: &Path) -> Result<PathBuf, AppError> {
    let file = fs::File::open(archive_path).map_err(|e| AppError::io(archive_path, e))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| AppError::Message(format!("Failed to open ZIP archive: {e}")))?;
    let binary_filename = format!("{BINARY_NAME}.exe");

    let mut entry = zip.by_name(&binary_filename).map_err(|_| {
        AppError::Message(format!("ZIP archive does not contain {binary_filename}"))
    })?;

    let binary_path = extract_dir.join(binary_filename);
    let mut output = fs::File::create(&binary_path).map_err(|e| AppError::io(&binary_path, e))?;
    std::io::copy(&mut entry, &mut output)
        .map_err(|e| AppError::Message(format!("Failed to extract binary from ZIP: {e}")))?;

    Ok(binary_path)
}

#[cfg(windows)]
fn extract_tar_binary(_archive_path: &Path, _extract_dir: &Path) -> Result<PathBuf, AppError> {
    Err(AppError::Message(
        "TAR extraction is not supported on Windows.".to_string(),
    ))
}

fn replace_current_binary(new_binary_path: &Path) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        return self_replace::self_replace(new_binary_path).map_err(|e| {
            AppError::Message(format!(
                "Failed to replace running executable on Windows: {e}"
            ))
        });
    }

    #[cfg(not(windows))]
    {
        let current_binary = std::env::current_exe().map_err(|e| {
            AppError::Message(format!("Failed to resolve current executable path: {e}"))
        })?;
        let parent = current_binary.parent().ok_or_else(|| {
            AppError::Message("Current executable path has no parent directory.".to_string())
        })?;

        let staged_binary = parent.join(format!("{BINARY_NAME}.new"));
        remove_file_if_present(&staged_binary)?;

        fs::copy(new_binary_path, &staged_binary)
            .map_err(|e| map_update_permission_error(&staged_binary, e))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o755);
            fs::set_permissions(&staged_binary, perms)
                .map_err(|e| map_update_permission_error(&staged_binary, e))?;
        }

        fs::rename(&staged_binary, &current_binary)
            .map_err(|e| map_update_permission_error(&current_binary, e))?;
        Ok(())
    }
}

fn remove_file_if_present(path: &Path) -> Result<(), AppError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(AppError::io(path, err)),
    }
}

fn map_update_permission_error(target: &Path, err: std::io::Error) -> AppError {
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        return AppError::Message(format!(
            "Permission denied while updating {}. Re-run with elevated privileges (for example: sudo cc-switch update), or use your package manager update command.",
            target.display()
        ));
    }
    AppError::io(target, err)
}

pub(crate) struct UpdateCheckInfo {
    pub current_version: String,
    pub target_tag: String,
    pub is_already_latest: bool,
    pub is_downgrade: bool,
}

pub(crate) async fn check_for_update() -> Result<UpdateCheckInfo, AppError> {
    let current_version = env!("CARGO_PKG_VERSION");
    let client = create_http_client()?;
    let target_tag = resolve_target_tag(&client, None).await?;
    let target_version = target_tag.trim_start_matches('v');

    let is_already_latest = target_version == current_version;
    let is_downgrade = should_skip_implicit_downgrade(current_version, target_version, false);

    Ok(UpdateCheckInfo {
        current_version: current_version.to_string(),
        target_tag,
        is_already_latest,
        is_downgrade,
    })
}

pub(crate) async fn download_and_apply(
    target_tag: &str,
    on_progress: impl Fn(u64, Option<u64>),
) -> Result<(), AppError> {
    let client = create_http_client()?;
    let expected_asset_name = release_asset_name()?;
    let release = fetch_release_by_tag(&client, target_tag).await?;
    let release_asset = select_release_asset(&release.assets, target_tag, &expected_asset_name)
        .ok_or_else(|| {
            AppError::Message(format!(
                "Release {target_tag} does not include expected asset '{expected_asset_name}' (or compatible tagged variant)."
            ))
        })?;
    let download_url = release_asset.browser_download_url.as_str();

    let downloaded_asset =
        download_release_asset(&client, download_url, &release_asset.name, Some(&on_progress))
            .await?;
    verify_asset_checksum(&client, &downloaded_asset.archive_path, target_tag, release_asset)
        .await?;
    let extracted_binary = extract_binary(&downloaded_asset.archive_path)?;
    replace_current_binary(&extracted_binary)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_tag_adds_prefix_when_missing() {
        assert_eq!(normalize_tag("4.6.2"), "v4.6.2");
    }

    #[test]
    fn normalize_tag_keeps_existing_prefix() {
        assert_eq!(normalize_tag("v4.6.2"), "v4.6.2");
    }

    #[test]
    fn parse_checksum_for_asset_finds_plain_filename() {
        let checksums =
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  cc-switch-cli-linux-x64-musl.tar.gz\n";
        let got = parse_checksum_for_asset(checksums, "cc-switch-cli-linux-x64-musl.tar.gz")
            .expect("checksum should exist");
        assert_eq!(
            got,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn parse_checksum_for_asset_supports_star_prefix() {
        let checksums =
            "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB *cc-switch-cli-linux-x64-musl.tar.gz\n";
        let got = parse_checksum_for_asset(checksums, "cc-switch-cli-linux-x64-musl.tar.gz")
            .expect("checksum should exist");
        assert_eq!(
            got,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
    }

    #[test]
    fn parse_checksum_for_asset_supports_spaces_in_filename() {
        let checksums =
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc  file with spaces.tar.gz\n";
        let got = parse_checksum_for_asset(checksums, "file with spaces.tar.gz")
            .expect("checksum should exist");
        assert_eq!(
            got,
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        );
    }

    #[test]
    fn release_api_url_for_github_com() {
        let url = release_api_url("https://github.com/saladday/cc-switch-cli", "latest")
            .expect("api url should be built");
        assert_eq!(
            url.as_str(),
            "https://api.github.com/repos/saladday/cc-switch-cli/releases/latest"
        );
    }

    #[test]
    fn release_api_url_for_github_enterprise() {
        let url = release_api_url(
            "https://github.enterprise.local/team/cc-switch-cli.git",
            "tags/v4.6.2",
        )
        .expect("api url should be built");
        assert_eq!(
            url.as_str(),
            "https://github.enterprise.local/api/v3/repos/team/cc-switch-cli/releases/tags/v4.6.2"
        );
    }

    #[test]
    fn select_release_asset_prefers_unprefixed_name() {
        let assets = vec![
            ReleaseAsset {
                name: "cc-switch-cli-v4.6.2-linux-x64-musl.tar.gz".to_string(),
                browser_download_url: "https://example.com/tagged".to_string(),
                digest: None,
            },
            ReleaseAsset {
                name: "cc-switch-cli-linux-x64-musl.tar.gz".to_string(),
                browser_download_url: "https://example.com/plain".to_string(),
                digest: None,
            },
        ];
        let selected =
            select_release_asset(&assets, "v4.6.2", "cc-switch-cli-linux-x64-musl.tar.gz")
                .expect("asset should be selected");
        assert_eq!(selected.browser_download_url, "https://example.com/plain");
    }

    #[test]
    fn select_release_asset_falls_back_to_tagged_variant() {
        let assets = vec![ReleaseAsset {
            name: "cc-switch-cli-v4.6.2-linux-x64-musl.tar.gz".to_string(),
            browser_download_url: "https://example.com/tagged".to_string(),
            digest: None,
        }];
        let selected =
            select_release_asset(&assets, "v4.6.2", "cc-switch-cli-linux-x64-musl.tar.gz")
                .expect("asset should be selected");
        assert_eq!(selected.browser_download_url, "https://example.com/tagged");
    }

    #[test]
    fn parse_sha256_digest_accepts_valid_value() {
        let digest = parse_sha256_digest(
            "sha256:ABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCD",
        )
        .expect("digest should parse");
        assert_eq!(
            digest,
            "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd"
        );
    }

    #[test]
    fn should_skip_implicit_downgrade_for_prerelease_current() {
        assert!(should_skip_implicit_downgrade(
            "4.7.0-alpha.1",
            "4.6.2",
            false
        ));
    }

    #[test]
    fn should_not_skip_when_version_explicitly_requested() {
        assert!(!should_skip_implicit_downgrade(
            "4.7.0-alpha.1",
            "4.6.2",
            true
        ));
    }

    #[test]
    fn sanitized_asset_file_name_strips_path_segments() {
        let name = sanitized_asset_file_name("nested/path/cc-switch-cli-linux-x64-musl.tar.gz")
            .expect("file name should be extracted");
        assert_eq!(name, "cc-switch-cli-linux-x64-musl.tar.gz");
    }

    #[test]
    fn sanitized_asset_file_name_rejects_invalid_value() {
        let err = sanitized_asset_file_name("").expect_err("empty name should fail");
        assert!(err.to_string().contains("Invalid asset name"));
    }

    #[test]
    fn validate_target_tag_accepts_normal_value() {
        validate_target_tag("v4.6.3-rc1").expect("valid tag should pass");
    }

    #[test]
    fn validate_target_tag_rejects_path_content() {
        let err = validate_target_tag("v4.6.3/../../evil").expect_err("must reject traversal");
        assert!(err.to_string().contains("forbidden"));
    }

    #[test]
    fn validate_download_size_limit_accepts_limit_boundary() {
        validate_download_size_limit(
            MAX_RELEASE_ASSET_SIZE_BYTES,
            "cc-switch-cli-linux-x64-musl.tar.gz",
        )
        .expect("size at limit should pass");
    }

    #[test]
    fn validate_download_size_limit_rejects_oversized_asset() {
        let err = validate_download_size_limit(
            MAX_RELEASE_ASSET_SIZE_BYTES + 1,
            "cc-switch-cli-linux-x64-musl.tar.gz",
        )
        .expect_err("size over limit should fail");
        assert!(err.to_string().contains("too large"));
    }
}
