use clap::Args;
#[cfg(not(windows))]
use flate2::read::GzDecoder;
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use tar::Archive;
use tempfile::TempDir;
use url::Url;

use crate::cli::ui::{highlight, info, success};
use crate::error::AppError;

const REPO_URL: &str = env!("CARGO_PKG_REPOSITORY");
const BINARY_NAME: &str = "cc-switch";
const CHECKSUMS_FILE_NAME: &str = "checksums.txt";
const LATEST_MANIFEST_FILE_NAME: &str = "latest.json";
const HTTP_REQUEST_TIMEOUT_SECS: u64 = 30;
const MAX_RELEASE_ASSET_SIZE_BYTES: u64 = 100 * 1024 * 1024;
const GITHUB_API_ACCEPT: &str = "application/vnd.github+json";
const UPDATER_PUBLIC_KEY: &str = include_str!("../../../updater/minisign.pub");
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

struct DownloadedAsset {
    _temp_dir: TempDir,
    archive_path: PathBuf,
}

#[derive(Debug, Deserialize, Clone)]
struct UpdateManifest {
    version: String,
    #[serde(default, rename = "notes")]
    _notes: Option<String>,
    #[serde(default, rename = "pub_date")]
    _pub_date: Option<String>,
    platforms: BTreeMap<String, UpdatePlatformEntry>,
}

#[derive(Debug, Deserialize, Clone)]
struct UpdatePlatformEntry {
    url: String,
    signature: String,
    #[serde(default)]
    variants: BTreeMap<String, UpdatePlatformVariant>,
}

#[derive(Debug, Deserialize, Clone)]
struct UpdatePlatformVariant {
    url: String,
    signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestAsset {
    url: String,
    signature: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinuxLibcPreference {
    Auto,
    Musl,
    Glibc,
}

#[derive(Debug, Deserialize, Clone)]
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

#[derive(Debug, Clone)]
enum ResolvedRelease {
    Manifest {
        target_tag: String,
        manifest: UpdateManifest,
    },
    Legacy {
        target_tag: String,
        release: ReleaseInfo,
    },
}

#[derive(Debug)]
enum ManifestFetchError {
    NotFound,
    Invalid(AppError),
}

impl From<AppError> for ManifestFetchError {
    fn from(value: AppError) -> Self {
        Self::Invalid(value)
    }
}

impl ResolvedRelease {
    fn target_tag(&self) -> &str {
        match self {
            Self::Manifest { target_tag, .. } | Self::Legacy { target_tag, .. } => target_tag,
        }
    }
}

pub fn execute(cmd: UpdateCommand) -> Result<(), AppError> {
    let runtime = create_runtime()?;
    runtime.block_on(execute_async(cmd))
}

async fn execute_async(cmd: UpdateCommand) -> Result<(), AppError> {
    let current_version = env!("CARGO_PKG_VERSION");
    let explicit_version = cmd.version.as_deref().is_some_and(|v| !v.trim().is_empty());
    let client = create_http_client()?;
    let release = resolve_target_release(&client, REPO_URL, cmd.version.as_deref()).await?;
    let target_tag = release.target_tag().to_string();
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

    println!(
        "{}",
        highlight(&format!("Current version: v{current_version}"))
    );
    println!("{}", highlight(&format!("Updating to: {target_tag}")));

    let downloaded_asset = match release {
        ResolvedRelease::Manifest { manifest, .. } => {
            let asset = select_current_manifest_asset(&manifest)?;
            println!("{}", info(&format!("Downloading: {}", asset.url)));
            println!("{}", info("Verifying updater signature."));
            let (downloaded_asset, _) =
                download_manifest_release_asset(&client, &manifest, None).await?;
            downloaded_asset
        }
        ResolvedRelease::Legacy {
            target_tag,
            release,
        } => {
            let expected_asset_names = current_release_asset_candidates()?;
            let release_asset = select_release_asset_from_candidates(
                &release.assets,
                &target_tag,
                &expected_asset_names,
            )
            .ok_or_else(|| {
                AppError::Message(format!(
                    "Release {target_tag} does not include any expected assets {:?} (or compatible tagged variants).",
                    expected_asset_names
                ))
            })?;
            let checksum_url = release_checksums_url(REPO_URL, &target_tag)?;
            println!(
                "{}",
                info(&format!(
                    "Downloading: {}",
                    release_asset.browser_download_url.as_str()
                ))
            );
            if release_asset.digest.is_some() {
                println!(
                    "{}",
                    info("Verifying checksum from release metadata digest.")
                );
            } else {
                println!("{}", info(&format!("Verifying checksum: {checksum_url}")));
            }
            let (downloaded_asset, _) =
                download_legacy_release_asset(&client, &target_tag, Some(&release), None).await?;
            downloaded_asset
        }
    };

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
    println!(
        "{}",
        info(
            "If you want to install or refresh managed bash/zsh completions, run: `cc-switch completions install`."
        )
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

fn update_manifest_url(repo_url: &str, tag: Option<&str>) -> Result<Url, AppError> {
    match tag {
        Some(tag) => release_page_url(
            repo_url,
            &format!("download/{tag}/{LATEST_MANIFEST_FILE_NAME}"),
        ),
        None => release_page_url(
            repo_url,
            &format!("latest/download/{LATEST_MANIFEST_FILE_NAME}"),
        ),
    }
}

async fn fetch_update_manifest(
    client: &reqwest::Client,
    repo_url: &str,
    tag: Option<&str>,
) -> Result<UpdateManifest, ManifestFetchError> {
    let url = update_manifest_url(repo_url, tag)?;
    let response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(|e| {
            ManifestFetchError::Invalid(AppError::Message(format!(
                "Failed to query update manifest: {e}"
            )))
        })?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(ManifestFetchError::NotFound);
    }

    response
        .error_for_status()
        .map_err(|e| {
            ManifestFetchError::Invalid(AppError::Message(format!(
                "Update manifest returned error: {e}"
            )))
        })?
        .json::<UpdateManifest>()
        .await
        .map_err(|e| {
            ManifestFetchError::Invalid(AppError::Message(format!(
                "Failed to parse update manifest: {e}"
            )))
        })
}

fn manifest_target_tag(manifest: &UpdateManifest) -> Result<String, AppError> {
    let tag = normalize_tag(manifest.version.trim());
    validate_target_tag(&tag)?;
    Ok(tag)
}

fn validate_requested_manifest_tag(
    manifest: &UpdateManifest,
    requested_tag: &str,
) -> Result<(), AppError> {
    let manifest_tag = manifest_target_tag(manifest)?;
    if manifest_tag != requested_tag {
        return Err(AppError::Message(format!(
            "Update manifest version {manifest_tag} does not match requested version {requested_tag}."
        )));
    }
    Ok(())
}

fn current_platform_key() -> Result<&'static str, AppError> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    match (os, arch) {
        ("macos", "x86_64") => Ok("darwin-x86_64"),
        ("macos", "aarch64") => Ok("darwin-aarch64"),
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("linux", "aarch64") => Ok("linux-aarch64"),
        ("windows", "x86_64") => Ok("windows-x86_64"),
        _ => Err(AppError::Message(format!(
            "Self-update is not supported for platform {os}/{arch}."
        ))),
    }
}

fn linux_libc_preference() -> Result<LinuxLibcPreference, AppError> {
    let raw = std::env::var("CC_SWITCH_LINUX_LIBC").unwrap_or_else(|_| "auto".to_string());
    match raw.trim() {
        "" | "auto" | "AUTO" => Ok(LinuxLibcPreference::Auto),
        "musl" | "MUSL" => Ok(LinuxLibcPreference::Musl),
        "glibc" | "GLIBC" | "gnu" | "GNU" => Ok(LinuxLibcPreference::Glibc),
        other => Err(AppError::Message(format!(
            "Unsupported CC_SWITCH_LINUX_LIBC='{other}'. Expected auto, musl, or glibc."
        ))),
    }
}

fn push_manifest_asset(candidates: &mut Vec<ManifestAsset>, asset: ManifestAsset) {
    if !candidates.contains(&asset) {
        candidates.push(asset);
    }
}

fn asset_looks_like_musl(url: &str) -> bool {
    url.contains("-musl")
}

fn select_manifest_asset(
    manifest: &UpdateManifest,
    platform_key: &str,
    preference: LinuxLibcPreference,
) -> Result<ManifestAsset, AppError> {
    manifest_asset_candidates(manifest, platform_key, preference)?
        .into_iter()
        .next()
        .ok_or_else(|| {
            AppError::Message("Update manifest does not contain a usable asset.".to_string())
        })
}

fn manifest_asset_candidates(
    manifest: &UpdateManifest,
    platform_key: &str,
    preference: LinuxLibcPreference,
) -> Result<Vec<ManifestAsset>, AppError> {
    let entry = manifest.platforms.get(platform_key).ok_or_else(|| {
        AppError::Message(format!(
            "Update manifest does not provide platform entry '{platform_key}'."
        ))
    })?;

    let primary = ManifestAsset {
        url: entry.url.clone(),
        signature: entry.signature.clone(),
    };

    if !platform_key.starts_with("linux-") {
        return Ok(vec![primary]);
    }

    let musl_variant = entry.variants.get("musl").map(|variant| ManifestAsset {
        url: variant.url.clone(),
        signature: variant.signature.clone(),
    });
    let glibc_variant = entry.variants.get("glibc").map(|variant| ManifestAsset {
        url: variant.url.clone(),
        signature: variant.signature.clone(),
    });

    let mut candidates = Vec::new();
    match preference {
        LinuxLibcPreference::Auto => {
            push_manifest_asset(&mut candidates, primary);
            if let Some(asset) = glibc_variant {
                push_manifest_asset(&mut candidates, asset);
            }
            if let Some(asset) = musl_variant {
                push_manifest_asset(&mut candidates, asset);
            }
        }
        LinuxLibcPreference::Musl => {
            if let Some(asset) = musl_variant {
                push_manifest_asset(&mut candidates, asset);
            } else if asset_looks_like_musl(&primary.url) {
                push_manifest_asset(&mut candidates, primary.clone());
            } else {
                return Err(AppError::Message(format!(
                    "Update manifest does not provide a musl variant for platform '{platform_key}'."
                )));
            }
        }
        LinuxLibcPreference::Glibc => {
            if let Some(asset) = glibc_variant {
                push_manifest_asset(&mut candidates, asset);
            } else if !asset_looks_like_musl(&primary.url) {
                push_manifest_asset(&mut candidates, primary.clone());
            } else {
                return Err(AppError::Message(format!(
                    "Update manifest does not provide a glibc variant for platform '{platform_key}'."
                )));
            }
            push_manifest_asset(&mut candidates, primary);
            if let Some(asset) = musl_variant {
                push_manifest_asset(&mut candidates, asset);
            }
        }
    }

    Ok(candidates)
}

fn select_current_manifest_asset(manifest: &UpdateManifest) -> Result<ManifestAsset, AppError> {
    select_manifest_asset(manifest, current_platform_key()?, linux_libc_preference()?)
}

fn release_asset_candidates_for_platform(
    os: &str,
    arch: &str,
    preference: LinuxLibcPreference,
) -> Result<Vec<String>, AppError> {
    let names = match (os, arch) {
        ("macos", "x86_64") => vec![
            "cc-switch-cli-darwin-universal.tar.gz".to_string(),
            "cc-switch-cli-darwin-x64.tar.gz".to_string(),
        ],
        ("macos", "aarch64") => vec![
            "cc-switch-cli-darwin-universal.tar.gz".to_string(),
            "cc-switch-cli-darwin-arm64.tar.gz".to_string(),
        ],
        ("linux", "x86_64") => match preference {
            LinuxLibcPreference::Auto => vec![
                "cc-switch-cli-linux-x64-musl.tar.gz".to_string(),
                "cc-switch-cli-linux-x64.tar.gz".to_string(),
            ],
            LinuxLibcPreference::Musl => vec!["cc-switch-cli-linux-x64-musl.tar.gz".to_string()],
            LinuxLibcPreference::Glibc => vec![
                "cc-switch-cli-linux-x64.tar.gz".to_string(),
                "cc-switch-cli-linux-x64-musl.tar.gz".to_string(),
            ],
        },
        ("linux", "aarch64") => match preference {
            LinuxLibcPreference::Auto => vec![
                "cc-switch-cli-linux-arm64-musl.tar.gz".to_string(),
                "cc-switch-cli-linux-arm64.tar.gz".to_string(),
            ],
            LinuxLibcPreference::Musl => {
                vec!["cc-switch-cli-linux-arm64-musl.tar.gz".to_string()]
            }
            LinuxLibcPreference::Glibc => vec![
                "cc-switch-cli-linux-arm64.tar.gz".to_string(),
                "cc-switch-cli-linux-arm64-musl.tar.gz".to_string(),
            ],
        },
        ("windows", "x86_64") => vec!["cc-switch-cli-windows-x64.zip".to_string()],
        _ => {
            return Err(AppError::Message(format!(
                "Self-update is not supported for platform {os}/{arch}."
            )))
        }
    };

    Ok(names)
}

fn current_release_asset_candidates() -> Result<Vec<String>, AppError> {
    release_asset_candidates_for_platform(
        std::env::consts::OS,
        std::env::consts::ARCH,
        linux_libc_preference()?,
    )
}

fn select_release_asset_from_candidates<'a>(
    assets: &'a [ReleaseAsset],
    target_tag: &str,
    expected_asset_names: &[String],
) -> Option<&'a ReleaseAsset> {
    expected_asset_names.iter().find_map(|expected_asset_name| {
        select_release_asset(assets, target_tag, expected_asset_name)
    })
}

fn parse_public_key(public_key_text: &str) -> Result<PublicKey, AppError> {
    PublicKey::decode(public_key_text.trim())
        .or_else(|_| PublicKey::from_base64(public_key_text.trim()))
        .map_err(|e| AppError::Message(format!("Invalid updater public key: {e}")))
}

fn verify_minisign_signature(
    payload: &[u8],
    signature_text: &str,
    public_key_text: &str,
) -> Result<(), AppError> {
    let public_key = parse_public_key(public_key_text)?;
    let signature = Signature::decode(signature_text.trim())
        .map_err(|e| AppError::Message(format!("Invalid updater signature: {e}")))?;
    public_key
        .verify(payload, &signature, false)
        .map_err(|e| AppError::Message(format!("Updater signature verification failed: {e}")))
}

fn verify_downloaded_asset_signature(
    archive_path: &Path,
    signature_text: &str,
) -> Result<(), AppError> {
    let payload = fs::read(archive_path).map_err(|e| AppError::io(archive_path, e))?;
    verify_minisign_signature(&payload, signature_text, UPDATER_PUBLIC_KEY)
}

fn asset_name_from_url(url: &str) -> Result<String, AppError> {
    let parsed = Url::parse(url)
        .map_err(|e| AppError::Message(format!("Invalid asset URL '{url}': {e}")))?;
    let asset_name = parsed
        .path_segments()
        .and_then(|segments| segments.last())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Message(format!("Asset URL has no file name: {url}")))?;

    sanitized_asset_file_name(asset_name).map(str::to_string)
}

async fn download_manifest_release_asset(
    client: &reqwest::Client,
    manifest: &UpdateManifest,
    on_progress: Option<&dyn Fn(u64, Option<u64>)>,
) -> Result<(DownloadedAsset, ManifestAsset), AppError> {
    let assets =
        manifest_asset_candidates(manifest, current_platform_key()?, linux_libc_preference()?)?;
    let mut last_error = None;

    for asset in assets {
        let asset_name = asset_name_from_url(&asset.url)?;
        match download_release_asset(client, &asset.url, &asset_name, on_progress).await {
            Ok(downloaded_asset) => {
                verify_downloaded_asset_signature(
                    &downloaded_asset.archive_path,
                    &asset.signature,
                )?;
                return Ok((downloaded_asset, asset));
            }
            Err(err) => last_error = Some(err),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        AppError::Message("Update manifest did not produce a downloadable asset.".to_string())
    }))
}

async fn download_legacy_release_asset(
    client: &reqwest::Client,
    target_tag: &str,
    release: Option<&ReleaseInfo>,
    on_progress: Option<&dyn Fn(u64, Option<u64>)>,
) -> Result<(DownloadedAsset, ReleaseAsset), AppError> {
    let release = match release {
        Some(release) => release.clone(),
        None => fetch_release_by_tag(client, REPO_URL, target_tag).await?,
    };
    let expected_asset_names = current_release_asset_candidates()?;
    let release_asset = select_release_asset_from_candidates(
        &release.assets,
        target_tag,
        &expected_asset_names,
    )
        .ok_or_else(|| {
            AppError::Message(format!(
                "Release {target_tag} does not include any expected assets {:?} (or compatible tagged variants).",
                expected_asset_names
            ))
        })?
        .clone();
    let downloaded_asset = download_release_asset(
        client,
        release_asset.browser_download_url.as_str(),
        release_asset.name.as_str(),
        on_progress,
    )
    .await?;
    verify_asset_checksum(
        client,
        &downloaded_asset.archive_path,
        target_tag,
        &release_asset,
    )
    .await?;
    Ok((downloaded_asset, release_asset))
}

async fn resolve_target_release(
    client: &reqwest::Client,
    repo_url: &str,
    version: Option<&str>,
) -> Result<ResolvedRelease, AppError> {
    if let Some(version) = version.map(str::trim).filter(|value| !value.is_empty()) {
        let target_tag = normalize_tag(version);
        validate_target_tag(&target_tag)?;

        match fetch_update_manifest(client, repo_url, Some(&target_tag)).await {
            Ok(manifest) => {
                validate_requested_manifest_tag(&manifest, &target_tag)?;
                return Ok(ResolvedRelease::Manifest {
                    target_tag,
                    manifest,
                });
            }
            Err(ManifestFetchError::NotFound) => {}
            Err(ManifestFetchError::Invalid(err)) => return Err(err),
        }

        return Ok(ResolvedRelease::Legacy {
            target_tag: target_tag.clone(),
            release: fetch_release_by_tag(client, repo_url, &target_tag).await?,
        });
    }

    match fetch_update_manifest(client, repo_url, None).await {
        Ok(manifest) => {
            return Ok(ResolvedRelease::Manifest {
                target_tag: manifest_target_tag(&manifest)?,
                manifest,
            });
        }
        Err(ManifestFetchError::NotFound) => {}
        Err(ManifestFetchError::Invalid(err)) => return Err(err),
    }

    let target_tag = fetch_latest_release_tag(client, repo_url).await?;
    Ok(ResolvedRelease::Legacy {
        target_tag: target_tag.clone(),
        release: fetch_release_by_tag(client, repo_url, &target_tag).await?,
    })
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

async fn fetch_latest_release_tag(
    client: &reqwest::Client,
    repo_url: &str,
) -> Result<String, AppError> {
    let api_url = release_api_url(repo_url, "latest")?;
    let response = client
        .get(api_url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::ACCEPT, GITHUB_API_ACCEPT)
        .send()
        .await
        .map_err(|e| AppError::Message(format!("Failed to query latest release: {e}")))?;

    if matches!(
        response.status(),
        reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::TOO_MANY_REQUESTS
    ) {
        return fetch_latest_release_tag_from_release_page(client, repo_url).await;
    }

    let release = response
        .error_for_status()
        .map_err(|e| AppError::Message(format!("Release API returned error: {e}")))?
        .json::<ReleaseInfo>()
        .await
        .map_err(|e| AppError::Message(format!("Failed to parse latest release response: {e}")))?;

    Ok(release.tag_name)
}

async fn fetch_release_by_tag(
    client: &reqwest::Client,
    repo_url: &str,
    tag: &str,
) -> Result<ReleaseInfo, AppError> {
    let api_url = release_api_url(repo_url, &format!("tags/{tag}"))?;
    client
        .get(api_url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::ACCEPT, GITHUB_API_ACCEPT)
        .send()
        .await
        .map_err(|e| AppError::Message(format!("Failed to query release {tag}: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Message(format!("Release API returned error for {tag}: {e}")))?
        .json::<ReleaseInfo>()
        .await
        .map_err(|e| AppError::Message(format!("Failed to parse release response for {tag}: {e}")))
}

async fn fetch_latest_release_tag_from_release_page(
    client: &reqwest::Client,
    repo_url: &str,
) -> Result<String, AppError> {
    let latest_url = release_page_url(repo_url, "latest")?;
    let response = client
        .get(latest_url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(|e| AppError::Message(format!("Failed to query latest release page: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Message(format!("Latest release page returned error: {e}")))?;

    extract_release_tag_from_url(response.url()).ok_or_else(|| {
        AppError::Message(format!(
            "Failed to resolve latest release tag from {}.",
            response.url()
        ))
    })
}

fn release_page_url(repo_url: &str, suffix: &str) -> Result<Url, AppError> {
    let repo_url = Url::parse(repo_url)
        .map_err(|e| AppError::Message(format!("Invalid repository URL '{repo_url}': {e}")))?;

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

    let mut release_url = repo_url.clone();
    release_url.set_path(&format!("/{owner}/{repo}/releases/{suffix}"));
    release_url.set_query(None);
    release_url.set_fragment(None);

    Ok(release_url)
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

fn release_checksums_url(repo_url: &str, tag: &str) -> Result<Url, AppError> {
    release_page_url(repo_url, &format!("download/{tag}/{CHECKSUMS_FILE_NAME}"))
}

fn extract_release_tag_from_url(url: &Url) -> Option<String> {
    let segments = url.path_segments()?.collect::<Vec<_>>();
    segments
        .windows(3)
        .find(|window| window[0] == "releases" && window[1] == "tag")
        .map(|window| window[2].to_string())
}

fn tagged_asset_name(tag: &str, asset_name: &str) -> String {
    if let Some(suffix) = asset_name.strip_prefix("cc-switch-cli-") {
        return format!("cc-switch-cli-{tag}-{suffix}");
    }
    asset_name.to_string()
}

fn release_asset_names(tag: &str, asset_name: &str) -> Vec<String> {
    let tagged = tagged_asset_name(tag, asset_name);
    if tagged == asset_name {
        vec![asset_name.to_string()]
    } else {
        vec![asset_name.to_string(), tagged]
    }
}

fn select_release_asset<'a>(
    assets: &'a [ReleaseAsset],
    target_tag: &str,
    expected_asset_name: &str,
) -> Option<&'a ReleaseAsset> {
    let expected_names = release_asset_names(target_tag, expected_asset_name);
    expected_names
        .iter()
        .find_map(|expected_name| assets.iter().find(|asset| asset.name == *expected_name))
}

async fn download_release_asset(
    client: &reqwest::Client,
    url: &str,
    asset_name: &str,
    on_progress: Option<&dyn Fn(u64, Option<u64>)>,
) -> Result<DownloadedAsset, AppError> {
    let response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(|e| AppError::Message(format!("Failed to download release asset: {e}")))?;

    let mut response = response
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
        let checksum_url = release_checksums_url(REPO_URL, target_tag)?;
        let checksum_content = download_text(client, checksum_url.as_str()).await?;
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
    let response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(|e| AppError::Message(format!("Failed to download checksum file: {e}")))?;

    response
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
    let target_tag = resolve_target_release(&client, REPO_URL, None)
        .await?
        .target_tag()
        .to_string();
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
    let release = resolve_target_release(&client, REPO_URL, Some(target_tag)).await?;
    let downloaded_asset = match release {
        ResolvedRelease::Manifest { manifest, .. } => {
            let (downloaded_asset, _) =
                download_manifest_release_asset(&client, &manifest, Some(&on_progress)).await?;
            downloaded_asset
        }
        ResolvedRelease::Legacy {
            target_tag,
            release,
        } => {
            let (downloaded_asset, _) = download_legacy_release_asset(
                &client,
                &target_tag,
                Some(&release),
                Some(&on_progress),
            )
            .await?;
            downloaded_asset
        }
    };
    let extracted_binary = extract_binary(&downloaded_asset.archive_path)?;
    replace_current_binary(&extracted_binary)?;

    Ok(())
}

#[cfg(test)]
mod tests;
