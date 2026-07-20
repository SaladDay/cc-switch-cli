//! Low-level S3-compatible HTTP transport.
//!
//! This follows the desktop upstream's merged implementation: AWS endpoints
//! use virtual-hosted addressing, while custom endpoints use path-style
//! addressing. Do not add provider-specific URL heuristics here until upstream
//! settles its pending URL-style work.

use std::time::Duration;

use futures::StreamExt;
use reqwest::StatusCode;
use url::Url;

use crate::error::AppError;
use crate::proxy::http_client;

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const TRANSFER_TIMEOUT_SECS: u64 = 300;

pub(crate) struct S3Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub region: String,
    pub bucket: String,
    /// Empty means the official AWS endpoint.
    pub endpoint: String,
}

fn is_aws_endpoint(endpoint: &str) -> bool {
    endpoint.is_empty() || endpoint.contains("amazonaws.com")
}

fn split_scheme_host(endpoint: &str) -> (&str, &str) {
    if let Some(host) = endpoint.strip_prefix("http://") {
        ("http", host.trim_end_matches('/'))
    } else if let Some(host) = endpoint.strip_prefix("https://") {
        ("https", host.trim_end_matches('/'))
    } else {
        ("https", endpoint.trim_end_matches('/'))
    }
}

fn build_object_url(credentials: &S3Credentials, key: &str) -> String {
    let key = key.trim_start_matches('/');
    if is_aws_endpoint(&credentials.endpoint) {
        format!(
            "https://{}.s3.{}.amazonaws.com/{}",
            credentials.bucket, credentials.region, key
        )
    } else {
        let (scheme, host) = split_scheme_host(&credentials.endpoint);
        format!("{scheme}://{host}/{}/{}", credentials.bucket, key)
    }
}

fn build_bucket_url(credentials: &S3Credentials) -> String {
    if is_aws_endpoint(&credentials.endpoint) {
        format!(
            "https://{}.s3.{}.amazonaws.com/",
            credentials.bucket, credentials.region
        )
    } else {
        let (scheme, host) = split_scheme_host(&credentials.endpoint);
        format!("{scheme}://{host}/{}/", credentials.bucket)
    }
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<sha2::Sha256>;
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(data))
}

fn uri_encode(input: &str, encode_slash: bool) -> String {
    let mut output = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(byte as char);
            }
            b'/' if !encode_slash => output.push('/'),
            _ => {
                use std::fmt::Write;
                let _ = write!(output, "%{byte:02X}");
            }
        }
    }
    output
}

fn sign_request(
    method: &str,
    url: &Url,
    headers: &mut reqwest::header::HeaderMap,
    body_hash: &str,
    credentials: &S3Credentials,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), AppError> {
    let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let host = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    };
    headers.insert("host", header_value(&host, "host")?);
    headers.insert("x-amz-date", header_value(&timestamp, "x-amz-date")?);
    headers.insert(
        "x-amz-content-sha256",
        header_value(body_hash, "x-amz-content-sha256")?,
    );

    let canonical_uri = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    let mut query_pairs = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    query_pairs.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let canonical_query = query_pairs
        .iter()
        .map(|(key, value)| format!("{}={}", uri_encode(key, true), uri_encode(value, true)))
        .collect::<Vec<_>>()
        .join("&");

    let mut header_names = headers
        .keys()
        .map(|name| name.as_str().to_lowercase())
        .collect::<Vec<_>>();
    header_names.sort();
    header_names.dedup();
    let canonical_headers = header_names
        .iter()
        .map(|name| {
            let value = headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .trim();
            format!("{name}:{value}\n")
        })
        .collect::<String>();
    let signed_headers = header_names.join(";");
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{body_hash}"
    );

    let scope = format!("{date}/{}/s3/aws4_request", credentials.region);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let date_key = hmac_sha256(
        format!("AWS4{}", credentials.secret_access_key).as_bytes(),
        date.as_bytes(),
    );
    let region_key = hmac_sha256(&date_key, credentials.region.as_bytes());
    let service_key = hmac_sha256(&region_key, b"s3");
    let signing_key = hmac_sha256(&service_key, b"aws4_request");
    let signature = hmac_sha256(&signing_key, string_to_sign.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        credentials.access_key_id
    );
    headers.insert(
        "authorization",
        header_value(&authorization, "authorization")?,
    );
    Ok(())
}

fn header_value(raw: &str, field: &str) -> Result<reqwest::header::HeaderValue, AppError> {
    reqwest::header::HeaderValue::from_str(raw).map_err(|_| {
        AppError::localized(
            "s3.header.invalid",
            format!("S3 {field} 请求头包含无效字符"),
            format!("S3 {field} header contains invalid characters."),
        )
    })
}

fn redact_url(raw: &str) -> String {
    match Url::parse(raw) {
        Ok(parsed) => {
            let mut output = format!("{}://", parsed.scheme());
            if let Some(host) = parsed.host_str() {
                output.push_str(host);
            }
            if let Some(port) = parsed.port() {
                output.push(':');
                output.push_str(&port.to_string());
            }
            output.push_str(parsed.path());
            output
        }
        Err(_) => raw.split('?').next().unwrap_or(raw).to_string(),
    }
}

fn transport_error(
    key: &'static str,
    operation_zh: &str,
    operation_en: &str,
    error: &reqwest::Error,
) -> AppError {
    let (reason_zh, reason_en) = if error.is_timeout() {
        ("请求超时", "request timed out")
    } else if error.is_connect() {
        ("连接失败", "connection failed")
    } else if error.is_request() {
        ("请求构造失败", "request build failed")
    } else {
        ("网络请求失败", "network request failed")
    };
    AppError::localized(
        key,
        format!("S3 {operation_zh}失败（{reason_zh}）"),
        format!("S3 {operation_en} failed ({reason_en})"),
    )
}

fn status_error(operation: &str, status: StatusCode, url: &str) -> AppError {
    let safe_url = redact_url(url);
    let mut zh = format!("S3 {operation} 失败: {status} ({safe_url})");
    let mut en = format!("S3 {operation} failed: {status} ({safe_url})");
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        zh.push_str("。请检查 Access Key ID 和 Secret Access Key。");
        en.push_str(". Please verify your Access Key ID and Secret Access Key.");
    } else if status == StatusCode::NOT_FOUND && operation == "HEAD bucket" {
        zh.push_str("。请检查存储桶名称和区域是否正确。");
        en.push_str(". Please check the bucket name and region.");
    }
    AppError::localized("s3.http.status", zh, en)
}

fn response_too_large_error(url: &str, max_bytes: usize) -> AppError {
    let max_mb = max_bytes / 1024 / 1024;
    AppError::localized(
        "s3.response_too_large",
        format!("S3 响应体超过上限（{max_mb} MB）: {}", redact_url(url)),
        format!(
            "S3 response body exceeds limit ({max_mb} MB): {}",
            redact_url(url)
        ),
    )
}

fn ensure_content_length_within_limit(
    headers: &reqwest::header::HeaderMap,
    max_bytes: usize,
    url: &str,
) -> Result<(), AppError> {
    let Some(length) = headers.get(reqwest::header::CONTENT_LENGTH) else {
        return Ok(());
    };
    let Ok(length) = length.to_str() else {
        return Ok(());
    };
    let Ok(length) = length.parse::<u64>() else {
        return Ok(());
    };
    if length > max_bytes as u64 {
        return Err(response_too_large_error(url, max_bytes));
    }
    Ok(())
}

pub(crate) async fn test_connection(credentials: &S3Credentials) -> Result<(), AppError> {
    let url_string = build_bucket_url(credentials);
    let url = parse_url(&url_string)?;
    let mut headers = reqwest::header::HeaderMap::new();
    sign_request(
        "HEAD",
        &url,
        &mut headers,
        &sha256_hex(b""),
        credentials,
        chrono::Utc::now(),
    )?;
    let response = http_client::get()
        .head(url.as_str())
        .headers(headers)
        .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|error| transport_error("s3.connection_failed", "连接", "connection", &error))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(status_error("HEAD bucket", response.status(), &url_string))
    }
}

pub(crate) async fn put_object(
    credentials: &S3Credentials,
    key: &str,
    bytes: Vec<u8>,
    content_type: &str,
) -> Result<(), AppError> {
    let url_string = build_object_url(credentials, key);
    let url = parse_url(&url_string)?;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("content-type", header_value(content_type, "content-type")?);
    sign_request(
        "PUT",
        &url,
        &mut headers,
        &sha256_hex(&bytes),
        credentials,
        chrono::Utc::now(),
    )?;
    let response = http_client::get()
        .put(url.as_str())
        .headers(headers)
        .body(bytes)
        .timeout(Duration::from_secs(TRANSFER_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|error| transport_error("s3.put_failed", "PUT 请求", "PUT request", &error))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(status_error("PUT", response.status(), &url_string))
    }
}

pub(crate) async fn get_object(
    credentials: &S3Credentials,
    key: &str,
    max_bytes: usize,
) -> Result<Option<(Vec<u8>, Option<String>)>, AppError> {
    let url_string = build_object_url(credentials, key);
    let url = parse_url(&url_string)?;
    let mut headers = reqwest::header::HeaderMap::new();
    sign_request(
        "GET",
        &url,
        &mut headers,
        &sha256_hex(b""),
        credentials,
        chrono::Utc::now(),
    )?;
    let response = http_client::get()
        .get(url.as_str())
        .headers(headers)
        .timeout(Duration::from_secs(TRANSFER_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|error| transport_error("s3.get_failed", "GET 请求", "GET request", &error))?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(status_error("GET", response.status(), &url_string));
    }
    ensure_content_length_within_limit(response.headers(), max_bytes, &url_string)?;
    let etag = response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            AppError::localized(
                "s3.response_read_failed",
                format!("读取 S3 响应失败: {error}"),
                format!("Failed to read S3 response: {error}"),
            )
        })?;
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(response_too_large_error(&url_string, max_bytes));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Some((bytes, etag)))
}

pub(crate) async fn head_object(
    credentials: &S3Credentials,
    key: &str,
) -> Result<Option<String>, AppError> {
    let url_string = build_object_url(credentials, key);
    let url = parse_url(&url_string)?;
    let mut headers = reqwest::header::HeaderMap::new();
    sign_request(
        "HEAD",
        &url,
        &mut headers,
        &sha256_hex(b""),
        credentials,
        chrono::Utc::now(),
    )?;
    let response = http_client::get()
        .head(url.as_str())
        .headers(headers)
        .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|error| transport_error("s3.head_failed", "HEAD 请求", "HEAD request", &error))?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(status_error("HEAD", response.status(), &url_string));
    }
    Ok(response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string))
}

fn parse_url(url: &str) -> Result<Url, AppError> {
    Url::parse(url).map_err(|error| {
        AppError::localized(
            "s3.url.invalid",
            format!("S3 URL 无效: {error}"),
            format!("Invalid S3 URL: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn credentials(endpoint: &str, region: &str, bucket: &str) -> S3Credentials {
        S3Credentials {
            access_key_id: "AKIDEXAMPLE".to_string(),
            secret_access_key: "SECRET".to_string(),
            region: region.to_string(),
            bucket: bucket.to_string(),
            endpoint: endpoint.to_string(),
        }
    }

    #[test]
    fn aws_uses_virtual_hosted_style() {
        let credentials = credentials("", "us-east-1", "my-bucket");
        assert_eq!(
            build_object_url(&credentials, "path/file.json"),
            "https://my-bucket.s3.us-east-1.amazonaws.com/path/file.json"
        );
    }

    #[test]
    fn custom_endpoint_uses_upstream_path_style() {
        let credentials = credentials("minio.example.com:9000", "us-east-1", "my-bucket");
        assert_eq!(
            build_object_url(&credentials, "path/file.json"),
            "https://minio.example.com:9000/my-bucket/path/file.json"
        );
    }

    #[test]
    fn custom_endpoint_preserves_http_scheme() {
        let credentials = credentials("http://minio:9000/", "us-east-1", "data");
        assert_eq!(build_bucket_url(&credentials), "http://minio:9000/data/");
    }

    #[test]
    fn bare_custom_endpoint_defaults_to_https_and_trims_slash() {
        let credentials = credentials("minio:9000/", "us-east-1", "data");
        assert_eq!(
            build_object_url(&credentials, "/snapshot/db.sql"),
            "https://minio:9000/data/snapshot/db.sql"
        );
    }

    #[test]
    fn sig_v4_uri_encoding_matches_rfc3986_rules() {
        assert_eq!(uri_encode("hello world", true), "hello%20world");
        assert_eq!(uri_encode("a+b=c&d", true), "a%2Bb%3Dc%26d");
        assert_eq!(uri_encode("path/to/file", false), "path/to/file");
        assert_eq!(uri_encode("path/to/file", true), "path%2Fto%2Ffile");
    }

    #[test]
    fn hmac_sha256_matches_known_vector() {
        let digest = hmac_sha256(b"key", b"The quick brown fox jumps over the lazy dog")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            digest,
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn signature_matches_upstream_known_vector() {
        let credentials = S3Credentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            region: "us-east-1".to_string(),
            bucket: "examplebucket".to_string(),
            endpoint: String::new(),
        };
        let url =
            Url::parse("https://examplebucket.s3.amazonaws.com/?lifecycle").expect("valid URL");
        let mut headers = reqwest::header::HeaderMap::new();
        sign_request(
            "GET",
            &url,
            &mut headers,
            &sha256_hex(b""),
            &credentials,
            chrono::Utc
                .with_ymd_and_hms(2013, 5, 24, 0, 0, 0)
                .single()
                .expect("valid timestamp"),
        )
        .expect("sign request");
        let authorization = headers
            .get("authorization")
            .expect("authorization header")
            .to_str()
            .expect("header text");
        assert!(authorization.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request"
        ));
        assert!(authorization.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date"));
        assert!(authorization.contains(
            "Signature=fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"
        ));
    }

    #[test]
    fn invalid_access_key_returns_error_instead_of_panicking() {
        let credentials = S3Credentials {
            access_key_id: "AKID\nINJECTED".to_string(),
            secret_access_key: "SECRET".to_string(),
            region: "us-east-1".to_string(),
            bucket: "examplebucket".to_string(),
            endpoint: String::new(),
        };
        let url = Url::parse("https://examplebucket.s3.amazonaws.com/").expect("valid URL");
        let result = sign_request(
            "GET",
            &url,
            &mut reqwest::header::HeaderMap::new(),
            &sha256_hex(b""),
            &credentials,
            chrono::Utc
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid timestamp"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn response_limit_rejects_large_content_length() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::CONTENT_LENGTH, "101".parse().unwrap());
        assert!(
            ensure_content_length_within_limit(&headers, 100, "https://example.com/x").is_err()
        );
    }

    #[test]
    fn errors_never_expose_query_credentials() {
        let redacted = redact_url(
            "https://bucket.example.com/file?X-Amz-Credential=AKID&X-Amz-Signature=secret",
        );
        assert_eq!(redacted, "https://bucket.example.com/file");
    }
}
