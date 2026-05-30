use super::*;
use axum::{response::Redirect, routing::get, Router};
use minisign::KeyPair;
use serial_test::serial;
use std::collections::BTreeMap;
use std::io::Cursor;
use tokio::net::TcpListener;

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

#[cfg(not(windows))]
fn force_homebrew_install_for_test() -> EnvVarGuard {
    let exe = std::env::current_exe().expect("current test executable should resolve");
    let prefix = exe
        .parent()
        .expect("executable must have a parent directory");
    EnvVarGuard::set("HOMEBREW_PREFIX", prefix.as_os_str())
}

#[cfg(not(windows))]
#[tokio::test]
#[serial(homebrew_update)]
async fn cli_explicit_update_exits_early_for_homebrew_install() {
    let _homebrew = force_homebrew_install_for_test();

    execute_async(UpdateCommand {
        version: Some("v999.0.0".to_string()),
    })
    .await
    .expect("homebrew-managed explicit CLI update should exit without querying releases");
}

#[cfg(not(windows))]
#[test]
fn cli_default_homebrew_update_is_not_blocked_before_checking_latest() {
    assert!(!should_block_homebrew_before_update_check(true, false));
}

#[cfg(not(windows))]
#[test]
fn cli_explicit_homebrew_update_is_blocked_before_release_lookup() {
    assert!(should_block_homebrew_before_update_check(true, true));
}

#[cfg(not(windows))]
#[test]
#[serial(homebrew_update)]
fn tui_update_check_marks_homebrew_package_manager_update() {
    let _homebrew = force_homebrew_install_for_test();

    let info = build_update_check_info(
        env!("CARGO_PKG_VERSION"),
        "v999.0.0".to_string(),
        is_homebrew_install(),
    );

    assert_eq!(info.target_tag, "v999.0.0");
    assert!(!info.is_already_latest);
    assert!(info.is_homebrew_managed);
}

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
fn release_page_url_for_github_com() {
    let url = release_page_url("https://github.com/saladday/cc-switch-cli", "latest")
        .expect("release page url should be built");
    assert_eq!(
        url.as_str(),
        "https://github.com/saladday/cc-switch-cli/releases/latest"
    );
}

#[test]
fn release_page_url_for_github_enterprise() {
    let url = release_page_url(
        "https://github.enterprise.local/team/cc-switch-cli.git",
        "tag/v4.6.2",
    )
    .expect("release page url should be built");
    assert_eq!(
        url.as_str(),
        "https://github.enterprise.local/team/cc-switch-cli/releases/tag/v4.6.2"
    );
}

#[test]
fn release_asset_names_prefer_plain_then_tagged_variant() {
    let names = release_asset_names("v4.6.2", "cc-switch-cli-linux-x64-musl.tar.gz");
    assert_eq!(
        names,
        vec![
            "cc-switch-cli-linux-x64-musl.tar.gz".to_string(),
            "cc-switch-cli-v4.6.2-linux-x64-musl.tar.gz".to_string(),
        ]
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
fn extract_release_tag_from_url_reads_release_tag_page() {
    let url = Url::parse("https://github.com/saladday/cc-switch-cli/releases/tag/v4.6.2")
        .expect("url should parse");
    let tag = extract_release_tag_from_url(&url).expect("tag should be extracted");
    assert_eq!(tag, "v4.6.2");
}

#[tokio::test]
async fn fetch_latest_release_tag_prefers_release_api_when_available() {
    let app = Router::new()
        .route(
            "/api/v3/repos/team/cc-switch-cli/releases/latest",
            get(|| async { axum::Json(serde_json::json!({ "tag_name": "v4.6.3" })) }),
        )
        .route(
            "/team/cc-switch-cli/releases/latest",
            get(|| async { Redirect::temporary("/team/cc-switch-cli/releases/tag/v4.6.2") }),
        )
        .route(
            "/team/cc-switch-cli/releases/tag/v4.6.2",
            get(|| async { "ok" }),
        );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let addr = listener.local_addr().expect("local addr should resolve");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server should run");
    });

    let client = create_http_client().expect("http client should initialize");
    let repo_url = format!("http://{addr}/team/cc-switch-cli");
    let tag = fetch_latest_release_tag(&client, &repo_url)
        .await
        .expect("latest tag should resolve from release api");
    assert_eq!(tag, "v4.6.3");

    server.abort();
}

#[tokio::test]
async fn fetch_latest_release_tag_falls_back_to_release_page_after_rate_limit() {
    let app = Router::new()
        .route(
            "/team/cc-switch-cli/releases/latest",
            get(|| async { Redirect::temporary("/team/cc-switch-cli/releases/tag/v4.6.2") }),
        )
        .route(
            "/team/cc-switch-cli/releases/tag/v4.6.2",
            get(|| async { "ok" }),
        )
        .route(
            "/api/v3/repos/team/cc-switch-cli/releases/latest",
            get(|| async { axum::http::StatusCode::FORBIDDEN }),
        );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let addr = listener.local_addr().expect("local addr should resolve");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server should run");
    });

    let client = create_http_client().expect("http client should initialize");
    let repo_url = format!("http://{addr}/team/cc-switch-cli");
    let tag = fetch_latest_release_tag(&client, &repo_url)
        .await
        .expect("latest tag should resolve from redirect");
    assert_eq!(tag, "v4.6.2");

    server.abort();
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
    let selected = select_release_asset(&assets, "v4.6.2", "cc-switch-cli-linux-x64-musl.tar.gz")
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
    let selected = select_release_asset(&assets, "v4.6.2", "cc-switch-cli-linux-x64-musl.tar.gz")
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

#[test]
fn select_manifest_asset_prefers_linux_glibc_variant_when_overridden() {
    let manifest = UpdateManifest {
        version: "v4.6.3".to_string(),
        _notes: None,
        _pub_date: None,
        platforms: BTreeMap::from([(
            "linux-x86_64".to_string(),
            UpdatePlatformEntry {
                url: "https://example.com/cc-switch-cli-linux-x64-musl.tar.gz".to_string(),
                signature: "musl-signature".to_string(),
                variants: BTreeMap::from([(
                    "glibc".to_string(),
                    UpdatePlatformVariant {
                        url: "https://example.com/cc-switch-cli-linux-x64.tar.gz".to_string(),
                        signature: "glibc-signature".to_string(),
                    },
                )]),
            },
        )]),
    };

    let asset = select_manifest_asset(&manifest, "linux-x86_64", LinuxLibcPreference::Glibc)
        .expect("glibc variant should be selected");

    assert_eq!(
        asset.url,
        "https://example.com/cc-switch-cli-linux-x64.tar.gz"
    );
    assert_eq!(asset.signature, "glibc-signature");
}

#[test]
fn select_manifest_asset_accepts_glibc_primary_entry_without_variant() {
    let manifest = UpdateManifest {
        version: "v4.6.3".to_string(),
        _notes: None,
        _pub_date: None,
        platforms: BTreeMap::from([(
            "linux-x86_64".to_string(),
            UpdatePlatformEntry {
                url: "https://example.com/glibc.tar.gz".to_string(),
                signature: "glibc-signature".to_string(),
                variants: BTreeMap::new(),
            },
        )]),
    };

    let asset = select_manifest_asset(&manifest, "linux-x86_64", LinuxLibcPreference::Glibc)
        .expect("glibc primary entry should be accepted");

    assert_eq!(asset.url, "https://example.com/glibc.tar.gz");
}

#[test]
fn manifest_linux_asset_candidates_keep_musl_strict_when_forced() {
    let manifest = UpdateManifest {
        version: "v4.6.3".to_string(),
        _notes: None,
        _pub_date: None,
        platforms: BTreeMap::from([(
            "linux-x86_64".to_string(),
            UpdatePlatformEntry {
                url: "https://example.com/cc-switch-cli-linux-x64-musl.tar.gz".to_string(),
                signature: "musl-signature".to_string(),
                variants: BTreeMap::from([(
                    "glibc".to_string(),
                    UpdatePlatformVariant {
                        url: "https://example.com/cc-switch-cli-linux-x64.tar.gz".to_string(),
                        signature: "glibc-signature".to_string(),
                    },
                )]),
            },
        )]),
    };

    let candidates =
        manifest_asset_candidates(&manifest, "linux-x86_64", LinuxLibcPreference::Musl)
            .expect("musl candidates should resolve");

    assert_eq!(
        candidates,
        vec![ManifestAsset {
            url: "https://example.com/cc-switch-cli-linux-x64-musl.tar.gz".to_string(),
            signature: "musl-signature".to_string(),
        }]
    );
}

#[test]
fn legacy_linux_asset_candidates_follow_glibc_override() {
    let candidates =
        release_asset_candidates_for_platform("linux", "x86_64", LinuxLibcPreference::Glibc)
            .expect("glibc candidates should resolve");

    assert_eq!(
        candidates,
        vec![
            "cc-switch-cli-linux-x64.tar.gz".to_string(),
            "cc-switch-cli-linux-x64-musl.tar.gz".to_string(),
        ]
    );
}

#[test]
fn legacy_linux_asset_candidates_keep_musl_strict_when_forced() {
    let candidates =
        release_asset_candidates_for_platform("linux", "x86_64", LinuxLibcPreference::Musl)
            .expect("musl candidates should resolve");

    assert_eq!(
        candidates,
        vec!["cc-switch-cli-linux-x64-musl.tar.gz".to_string(),]
    );
}

#[tokio::test]
async fn fetch_update_manifest_reads_latest_json_without_release_api() {
    let platform_key = current_platform_key().expect("platform key should resolve");
    let manifest = serde_json::json!({
        "version": "v4.6.3",
        "notes": "manifest path",
        "pub_date": "2026-03-14T00:00:00Z",
        "platforms": {
            platform_key: {
                "url": "https://example.com/cc-switch.tar.gz",
                "signature": "fake-signature"
            }
        }
    });

    let app = Router::new().route(
        "/team/cc-switch-cli/releases/latest/download/latest.json",
        get(move || {
            let manifest = manifest.clone();
            async move { axum::Json(manifest) }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let addr = listener.local_addr().expect("local addr should resolve");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server should run");
    });

    let client = create_http_client().expect("http client should initialize");
    let repo_url = format!("http://{addr}/team/cc-switch-cli");
    let manifest = fetch_update_manifest(&client, &repo_url, None)
        .await
        .expect("latest manifest should resolve");
    assert_eq!(manifest.version, "v4.6.3");

    server.abort();
}

#[tokio::test]
async fn resolve_target_release_rejects_manifest_version_mismatch_for_explicit_version() {
    let platform_key = current_platform_key().expect("platform key should resolve");
    let manifest = serde_json::json!({
        "version": "v4.6.4",
        "platforms": {
            platform_key: {
                "url": "https://example.com/cc-switch.tar.gz",
                "signature": "fake-signature"
            }
        }
    });

    let app = Router::new().route(
        "/team/cc-switch-cli/releases/download/v4.6.3/latest.json",
        get(move || {
            let manifest = manifest.clone();
            async move { axum::Json(manifest) }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let addr = listener.local_addr().expect("local addr should resolve");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server should run");
    });

    let client = create_http_client().expect("http client should initialize");
    let repo_url = format!("http://{addr}/team/cc-switch-cli");
    let err = resolve_target_release(&client, &repo_url, Some("v4.6.3"))
        .await
        .expect_err("mismatched manifest version must fail");
    assert!(err.to_string().contains("does not match requested version"));

    server.abort();
}

#[tokio::test]
async fn resolve_target_release_falls_back_only_when_manifest_is_missing() {
    let app = Router::new()
        .route(
            "/team/cc-switch-cli/releases/latest/download/latest.json",
            get(|| async { axum::http::StatusCode::NOT_FOUND }),
        )
        .route(
            "/api/v3/repos/team/cc-switch-cli/releases/latest",
            get(|| async {
                axum::Json(serde_json::json!({
                    "tag_name": "v4.6.3",
                    "assets": []
                }))
            }),
        )
        .route(
            "/api/v3/repos/team/cc-switch-cli/releases/tags/v4.6.3",
            get(|| async {
                axum::Json(serde_json::json!({
                    "tag_name": "v4.6.3",
                    "assets": []
                }))
            }),
        );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let addr = listener.local_addr().expect("local addr should resolve");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server should run");
    });

    let client = create_http_client().expect("http client should initialize");
    let repo_url = format!("http://{addr}/team/cc-switch-cli");
    let release = resolve_target_release(&client, &repo_url, None)
        .await
        .expect("404 manifest should fall back to legacy release");
    assert!(matches!(
        release,
        ResolvedRelease::Legacy { ref target_tag, .. } if target_tag == "v4.6.3"
    ));

    server.abort();
}

#[tokio::test]
async fn resolve_target_release_does_not_fallback_when_manifest_is_invalid() {
    let app = Router::new()
        .route(
            "/team/cc-switch-cli/releases/latest/download/latest.json",
            get(|| async { "not-json" }),
        )
        .route(
            "/api/v3/repos/team/cc-switch-cli/releases/latest",
            get(|| async {
                axum::Json(serde_json::json!({
                    "tag_name": "v4.6.3",
                    "assets": []
                }))
            }),
        )
        .route(
            "/api/v3/repos/team/cc-switch-cli/releases/tags/v4.6.3",
            get(|| async {
                axum::Json(serde_json::json!({
                    "tag_name": "v4.6.3",
                    "assets": []
                }))
            }),
        );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let addr = listener.local_addr().expect("local addr should resolve");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server should run");
    });

    let client = create_http_client().expect("http client should initialize");
    let repo_url = format!("http://{addr}/team/cc-switch-cli");
    let err = resolve_target_release(&client, &repo_url, None)
        .await
        .expect_err("invalid manifest should not fall back to legacy release");
    assert!(err.to_string().contains("Failed to parse update manifest"));

    server.abort();
}

#[test]
fn verify_minisign_signature_accepts_valid_signature() {
    let payload = br#"{"version":"v4.6.3"}"#;
    let KeyPair { pk, sk } =
        KeyPair::generate_unencrypted_keypair().expect("key pair should generate");
    let signature = minisign::sign(None, &sk, Cursor::new(payload), None, None)
        .expect("payload should sign")
        .to_string();
    let public_key = pk.to_box().expect("public key box").to_string();

    verify_minisign_signature(payload, &signature, &public_key).expect("signature should verify");
}
