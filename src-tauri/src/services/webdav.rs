//! WebDAV HTTP 传输层
//!
//! 提供底层 HTTP 操作：PUT / GET / HEAD / PROPFIND / MKCOL，
//! 以及 URL 构建、认证、连接测试等公共工具。

use std::time::Duration;

use reqwest::{Client, Method, StatusCode};
use url::Url;

use crate::error::AppError;

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

/// 普通请求（PROPFIND / MKCOL / HEAD 等）超时
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// 大文件传输（PUT / GET）超时
const TRANSFER_TIMEOUT_SECS: u64 = 300;

// ---------------------------------------------------------------------------
// 认证
// ---------------------------------------------------------------------------

/// `(username, Option<password>)`；`None` 表示无认证。
pub type WebDavAuth = Option<(String, Option<String>)>;

pub fn auth_from_credentials(username: &str, password: &str) -> WebDavAuth {
    let u = username.trim();
    if u.is_empty() {
        return None;
    }
    let p = password.trim();
    Some((u.to_string(), if p.is_empty() { None } else { Some(p.to_string()) }))
}

// ---------------------------------------------------------------------------
// URL 工具
// ---------------------------------------------------------------------------

pub fn parse_base_url(raw: &str) -> Result<Url, AppError> {
    let trimmed = raw.trim().trim_end_matches('/');
    let url = Url::parse(trimmed)
        .map_err(|e| AppError::InvalidInput(format!("WebDAV base_url 不是合法 URL: {e}")))?;
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(AppError::InvalidInput(
            "WebDAV base_url 仅支持 http/https".to_string(),
        ));
    }
    Ok(url)
}

pub fn build_remote_url(base_url: &str, segments: &[String]) -> Result<String, AppError> {
    let mut url = Url::parse(base_url)
        .map_err(|e| AppError::InvalidInput(format!("WebDAV base_url 不是合法 URL: {e}")))?;
    {
        let mut path_builder = url.path_segments_mut().map_err(|_| {
            AppError::InvalidInput("WebDAV base_url 必须是分层目录地址".to_string())
        })?;
        path_builder.pop_if_empty();
        for segment in segments {
            path_builder.push(segment);
        }
    }
    Ok(url.to_string())
}

pub fn path_segments(raw: &str) -> impl Iterator<Item = &str> {
    raw.trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
}

pub fn is_jianguoyun(base_url: &str) -> bool {
    Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(|host| host.to_lowercase()))
        .map(|host| host.contains("jianguoyun.com") || host.contains("nutstore"))
        .unwrap_or(false)
}

fn redact_url(url: &str) -> String {
    match Url::parse(url) {
        Ok(mut parsed) => {
            if parsed.password().is_some() {
                let _ = parsed.set_password(Some("***"));
            }
            parsed.to_string()
        }
        Err(_) => url.to_string(),
    }
}

// ---------------------------------------------------------------------------
// HTTP 客户端
// ---------------------------------------------------------------------------

fn build_client(timeout_secs: u64) -> Result<Client, AppError> {
    Client::builder()
        .timeout(Duration::from_secs(timeout_secs.max(1)))
        .build()
        .map_err(|e| AppError::Message(format!("创建 WebDAV HTTP 客户端失败: {e}")))
}

fn apply_auth(
    builder: reqwest::RequestBuilder,
    auth: &WebDavAuth,
) -> reqwest::RequestBuilder {
    match auth {
        Some((user, pass)) => builder.basic_auth(user, pass.as_deref()),
        None => builder,
    }
}

// ---------------------------------------------------------------------------
// 错误辅助
// ---------------------------------------------------------------------------

pub fn webdav_status_error(
    base_url: &str,
    operation: &str,
    status: StatusCode,
    url: &str,
) -> AppError {
    let display_url = redact_url(url);
    let mut message = format!("WebDAV {operation} 失败: {status} ({display_url})");
    let jgy = is_jianguoyun(base_url);

    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        if jgy {
            message.push_str(
                "。坚果云通常需要「第三方应用密码」，并且 base_url 应指向 /dav/ 下的目录。",
            );
        } else {
            message.push_str("。请检查 WebDAV 用户名、密码，以及该目录的读写权限。");
        }
    } else if jgy && (status == StatusCode::NOT_FOUND || status.is_redirection()) {
        message.push_str(
            "。坚果云常见原因是 base_url 不在 /dav/ 可写目录下；请改为 https://dav.jianguoyun.com/dav/....",
        );
    } else if operation == "MKCOL" && status == StatusCode::CONFLICT {
        if jgy {
            message
                .push_str("。坚果云对分层自动建目录较敏感，请先在服务端手动创建上级目录后再重试。");
        } else {
            message.push_str("。请确认上级目录存在，或将 remote_root/profile 调整到可写路径。");
        }
    } else if operation == "MKCOL" && status == StatusCode::METHOD_NOT_ALLOWED {
        message.push_str("。目录可能已存在，可忽略此状态。");
    }
    AppError::Message(message)
}

fn with_service_hint(base_url: &str, message: impl Into<String>) -> String {
    let mut msg = message.into();
    if is_jianguoyun(base_url) {
        msg.push_str(
            "。坚果云请优先使用「第三方应用密码」，并确认 base_url 指向 /dav/ 下的可写目录。",
        );
    }
    msg
}

// ---------------------------------------------------------------------------
// 连接测试
// ---------------------------------------------------------------------------

pub async fn test_connection(base_url: &str, auth: &WebDavAuth) -> Result<(), AppError> {
    let client = build_client(DEFAULT_TIMEOUT_SECS)?;
    let method =
        Method::from_bytes(b"PROPFIND").map_err(|e| AppError::Message(e.to_string()))?;
    let mut req = client.request(method, base_url).header("Depth", "0");
    req = apply_auth(req, auth);
    let resp = req.send().await.map_err(|e| {
        AppError::Message(with_service_hint(
            base_url,
            format!("WebDAV 连接测试失败: {e}"),
        ))
    })?;
    match resp.status() {
        StatusCode::OK | StatusCode::MULTI_STATUS | StatusCode::NO_CONTENT => Ok(()),
        status => Err(webdav_status_error(base_url, "PROPFIND", status, base_url)),
    }
}

// ---------------------------------------------------------------------------
// PUT
// ---------------------------------------------------------------------------

pub async fn put_bytes(
    url: &str,
    auth: &WebDavAuth,
    bytes: Vec<u8>,
    content_type: &str,
) -> Result<(), AppError> {
    let base_url = url;
    let client = build_client(TRANSFER_TIMEOUT_SECS)?;
    let mut req = client
        .put(url)
        .header("Content-Type", content_type)
        .body(bytes);
    req = apply_auth(req, auth);
    let resp = req.send().await.map_err(|e| {
        AppError::Message(with_service_hint(
            base_url,
            format!("WebDAV PUT 请求失败: {e}"),
        ))
    })?;
    if !resp.status().is_success() {
        return Err(webdav_status_error(base_url, "PUT", resp.status(), url));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// GET
// ---------------------------------------------------------------------------

pub async fn get_bytes(
    url: &str,
    auth: &WebDavAuth,
    max_bytes: Option<u64>,
) -> Result<Option<(Vec<u8>, Option<String>)>, AppError> {
    let base_url = url;
    let client = build_client(TRANSFER_TIMEOUT_SECS)?;
    let mut req = client.get(url);
    req = apply_auth(req, auth);
    let resp = req.send().await.map_err(|e| {
        AppError::Message(with_service_hint(
            base_url,
            format!("WebDAV GET 请求失败: {e}"),
        ))
    })?;
    if resp.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(webdav_status_error(base_url, "GET", resp.status(), url));
    }
    let etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if let Some(limit) = max_bytes {
        if let Some(len) = resp.content_length() {
            if len > limit {
                return Err(AppError::Message(format!(
                    "WebDAV 响应超过大小限制 ({limit} bytes)"
                )));
            }
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AppError::Message(format!("读取 WebDAV 响应失败: {e}")))?;
        if bytes.len() as u64 > limit {
            return Err(AppError::Message(format!(
                "WebDAV 响应超过大小限制 ({limit} bytes)"
            )));
        }
        Ok(Some((bytes.to_vec(), etag)))
    } else {
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AppError::Message(format!("读取 WebDAV 响应失败: {e}")))?;
        Ok(Some((bytes.to_vec(), etag)))
    }
}

// ---------------------------------------------------------------------------
// HEAD
// ---------------------------------------------------------------------------

pub async fn head_etag(url: &str, auth: &WebDavAuth) -> Result<Option<String>, AppError> {
    let base_url = url;
    let client = build_client(DEFAULT_TIMEOUT_SECS)?;
    let mut req = client.head(url);
    req = apply_auth(req, auth);
    let resp = req.send().await.map_err(|e| {
        AppError::Message(with_service_hint(
            base_url,
            format!("WebDAV HEAD 请求失败: {e}"),
        ))
    })?;
    if resp.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(webdav_status_error(base_url, "HEAD", resp.status(), url));
    }
    Ok(resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string()))
}

// ---------------------------------------------------------------------------
// 目录操作
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteDirProbe {
    Exists,
    Missing,
    Unsupported,
}

async fn propfind_remote_dir(
    url: &str,
    auth: &WebDavAuth,
    base_url: &str,
) -> Result<RemoteDirProbe, AppError> {
    let client = build_client(DEFAULT_TIMEOUT_SECS)?;
    let method =
        Method::from_bytes(b"PROPFIND").map_err(|e| AppError::Message(e.to_string()))?;
    let mut req = client.request(method, url).header("Depth", "0");
    req = apply_auth(req, auth);
    let resp = req.send().await.map_err(|e| {
        AppError::Message(with_service_hint(
            base_url,
            format!("WebDAV PROPFIND 请求失败: {e}"),
        ))
    })?;
    match resp.status() {
        StatusCode::OK | StatusCode::MULTI_STATUS | StatusCode::NO_CONTENT => {
            Ok(RemoteDirProbe::Exists)
        }
        StatusCode::NOT_FOUND => Ok(RemoteDirProbe::Missing),
        StatusCode::METHOD_NOT_ALLOWED => Ok(RemoteDirProbe::Unsupported),
        status => Err(webdav_status_error(base_url, "PROPFIND", status, url)),
    }
}

async fn mkcol_remote_dir(
    url: &str,
    auth: &WebDavAuth,
    base_url: &str,
) -> Result<StatusCode, AppError> {
    let client = build_client(DEFAULT_TIMEOUT_SECS)?;
    let method = Method::from_bytes(b"MKCOL").map_err(|e| AppError::Message(e.to_string()))?;
    let mut req = client.request(method, url);
    req = apply_auth(req, auth);
    let resp = req.send().await.map_err(|e| {
        AppError::Message(with_service_hint(
            base_url,
            format!("WebDAV MKCOL 请求失败: {e}"),
        ))
    })?;
    Ok(resp.status())
}

fn should_verify_after_mkcol(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
            | StatusCode::CONFLICT
    )
}

/// DELETE a remote collection (directory). Returns Ok(true) if deleted,
/// Ok(false) if 404/410 (already gone), Err on other failures.
pub async fn delete_collection(url: &str, auth: &WebDavAuth) -> Result<bool, AppError> {
    let client = build_client(30)?;
    let req = apply_auth(client.request(Method::DELETE, url), auth);
    let resp = req.send().await.map_err(|e| {
        AppError::Message(format!(
            "WebDAV DELETE {} failed: {e}",
            redact_url(url)
        ))
    })?;
    let status = resp.status();
    match status {
        s if s.is_success() => Ok(true),
        StatusCode::NOT_FOUND | StatusCode::GONE => Ok(false),
        _ => Err(webdav_status_error(url, "DELETE", status, url)),
    }
}

pub async fn ensure_remote_directories(
    base_url: &str,
    segments: &[String],
    auth: &WebDavAuth,
) -> Result<(), AppError> {
    let mut current = Vec::<String>::new();
    for segment in segments {
        current.push(segment.clone());
        let url = build_remote_url(base_url, &current)?;
        ensure_single_dir(&url, auth, base_url).await?;
    }
    Ok(())
}

async fn ensure_single_dir(
    url: &str,
    auth: &WebDavAuth,
    base_url: &str,
) -> Result<(), AppError> {
    match propfind_remote_dir(url, auth, base_url).await? {
        RemoteDirProbe::Exists => return Ok(()),
        RemoteDirProbe::Missing | RemoteDirProbe::Unsupported => {}
    }

    let status = mkcol_remote_dir(url, auth, base_url).await?;
    match status {
        StatusCode::CREATED => Ok(()),
        status if should_verify_after_mkcol(status) => {
            match propfind_remote_dir(url, auth, base_url).await? {
                RemoteDirProbe::Exists => Ok(()),
                RemoteDirProbe::Missing | RemoteDirProbe::Unsupported => {
                    Err(webdav_status_error(base_url, "MKCOL", status, url))
                }
            }
        }
        _ => Err(webdav_status_error(base_url, "MKCOL", status, url)),
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_remote_url_encodes_path_segments() {
        let base = "https://dav.example.com/remote.php/dav/files/demo";
        let segments = vec![
            "cc switch-sync".to_string(),
            "team a".to_string(),
            "v2".to_string(),
            "default profile".to_string(),
            "manifest.json".to_string(),
        ];
        let url = build_remote_url(base, &segments).expect("build remote url");
        assert_eq!(
            url,
            "https://dav.example.com/remote.php/dav/files/demo/cc%20switch-sync/team%20a/v2/default%20profile/manifest.json"
        );
    }

    #[test]
    fn path_segments_splits_correctly() {
        let segs: Vec<&str> = path_segments("/a/b/c/").collect();
        assert_eq!(segs, vec!["a", "b", "c"]);

        let segs: Vec<&str> = path_segments("single").collect();
        assert_eq!(segs, vec!["single"]);

        let segs: Vec<&str> = path_segments("").collect();
        assert!(segs.is_empty());
    }

    #[test]
    fn is_jianguoyun_detects_known_hosts() {
        assert!(is_jianguoyun("https://dav.jianguoyun.com/dav"));
        assert!(is_jianguoyun("https://dav.nutstore.net/dav"));
        assert!(!is_jianguoyun("https://dav.example.com/dav"));
    }

    #[test]
    fn auth_from_credentials_empty_username_returns_none() {
        assert!(auth_from_credentials("", "pass").is_none());
        assert!(auth_from_credentials("  ", "pass").is_none());
    }

    #[test]
    fn auth_from_credentials_valid() {
        let auth = auth_from_credentials("user", "pass");
        assert_eq!(
            auth,
            Some(("user".to_string(), Some("pass".to_string())))
        );
    }

    #[test]
    fn auth_from_credentials_empty_password() {
        let auth = auth_from_credentials("user", "");
        assert_eq!(auth, Some(("user".to_string(), None)));
    }

    #[test]
    fn redact_url_hides_password() {
        let url = "https://user:secret@example.com/dav";
        let redacted = redact_url(url);
        assert!(!redacted.contains("secret"));
        assert!(redacted.contains("***"));
    }

    #[test]
    fn redact_url_no_password_unchanged() {
        let url = "https://example.com/dav";
        assert_eq!(redact_url(url), url);
    }

    #[test]
    fn parse_base_url_rejects_ftp() {
        assert!(parse_base_url("ftp://example.com").is_err());
    }

    #[test]
    fn parse_base_url_accepts_https() {
        let url = parse_base_url("https://example.com/dav/").unwrap();
        assert_eq!(url.scheme(), "https");
    }

    #[test]
    fn mkcol_405_and_409_require_post_verification() {
        assert!(should_verify_after_mkcol(StatusCode::METHOD_NOT_ALLOWED));
        assert!(should_verify_after_mkcol(StatusCode::CONFLICT));
        assert!(should_verify_after_mkcol(StatusCode::TEMPORARY_REDIRECT));
        assert!(should_verify_after_mkcol(StatusCode::PERMANENT_REDIRECT));
        assert!(!should_verify_after_mkcol(StatusCode::CREATED));
    }
}
