use percent_encoding::percent_decode_str;
use serde::Serialize;
use std::time::{Duration, Instant};

use crate::database::Database;
use crate::error::AppError;
use crate::proxy::http_client;
use crate::store::AppState;

const PROXY_ENV_KEYS: [&str; 6] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
];

#[derive(Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalOutboundProxyConfig {
    pub url: String,
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for GlobalOutboundProxyConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GlobalOutboundProxyConfig")
            .field("url", &http_client::mask_url(&self.url))
            .field(
                "username",
                &if self.username.is_empty() { "" } else { "***" },
            )
            .field(
                "password",
                &if self.password.is_empty() { "" } else { "***" },
            )
            .finish()
    }
}

impl GlobalOutboundProxyConfig {
    pub fn from_full_url(full_url: &str) -> Result<Self, AppError> {
        let full_url = full_url.trim();
        if full_url.is_empty() {
            return Ok(Self::default());
        }

        let mut parsed = parse_proxy_url(full_url)?;
        let username = decode_url_component(parsed.username());
        let password = parsed
            .password()
            .map(decode_url_component)
            .unwrap_or_default();
        parsed
            .set_username("")
            .map_err(|_| invalid_proxy_username())?;
        parsed
            .set_password(None)
            .map_err(|_| invalid_proxy_password())?;

        Ok(Self {
            url: parsed.to_string(),
            username,
            password,
        })
    }

    pub fn to_full_url(&self) -> Result<String, AppError> {
        let raw_url = self.url.trim();
        if raw_url.is_empty() {
            if self.username.trim().is_empty() && self.password.is_empty() {
                return Ok(String::new());
            }
            return Err(AppError::InvalidInput(
                crate::t!(
                    "Enter a proxy URL before setting credentials.",
                    "设置用户名或密码前，请先填写代理 URL。"
                )
                .to_string(),
            ));
        }

        let mut parsed = parse_proxy_url(raw_url)?;
        if self.username.trim().is_empty() {
            if !self.password.is_empty() {
                return Err(AppError::InvalidInput(
                    crate::t!(
                        "Enter a proxy username before setting a password.",
                        "设置密码前，请先填写代理用户名。"
                    )
                    .to_string(),
                ));
            }
            parsed
                .set_username("")
                .map_err(|_| invalid_proxy_username())?;
            parsed
                .set_password(None)
                .map_err(|_| invalid_proxy_password())?;
        } else {
            parsed
                .set_username(self.username.trim())
                .map_err(|_| invalid_proxy_username())?;
            parsed
                .set_password(Some(self.password.as_str()))
                .map_err(|_| invalid_proxy_password())?;
        }

        let full_url = parsed.to_string();
        http_client::validate_proxy(Some(&full_url)).map_err(AppError::InvalidInput)?;
        Ok(full_url)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalProxyUpdateOutcome {
    pub daemon_warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GlobalProxyTestResult {
    pub success: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
}

pub fn load(db: &Database) -> Result<Option<GlobalOutboundProxyConfig>, AppError> {
    db.get_global_proxy_url()?
        .map(|url| GlobalOutboundProxyConfig::from_full_url(&url))
        .transpose()
}

pub fn set(
    state: &AppState,
    config: &GlobalOutboundProxyConfig,
) -> Result<GlobalProxyUpdateOutcome, AppError> {
    let full_url = config.to_full_url()?;
    update(state, Some(full_url.as_str()))
}

pub fn clear(state: &AppState) -> Result<GlobalProxyUpdateOutcome, AppError> {
    update(state, None)
}

fn update(state: &AppState, full_url: Option<&str>) -> Result<GlobalProxyUpdateOutcome, AppError> {
    http_client::validate_proxy(full_url).map_err(AppError::InvalidInput)?;
    state.db.set_global_proxy_url(full_url)?;
    http_client::apply_proxy(full_url).map_err(AppError::Message)?;

    #[cfg(unix)]
    let daemon_warning = crate::daemon::notify_outbound_proxy_reload().err();
    #[cfg(not(unix))]
    let daemon_warning = None;

    Ok(GlobalProxyUpdateOutcome { daemon_warning })
}

pub fn configured_environment_variables() -> Vec<String> {
    PROXY_ENV_KEYS
        .into_iter()
        .filter(|key| {
            std::env::var(key)
                .ok()
                .is_some_and(|value| !value.trim().is_empty())
        })
        .map(str::to_string)
        .collect()
}

pub fn effective_environment_variables() -> Vec<String> {
    http_client::effective_environment_proxy_variables()
}

pub fn initialize_http_client(db: &Database) {
    let saved_url = match db.get_global_proxy_url() {
        Ok(url) => url,
        Err(error) => {
            log::error!("[GlobalProxy] Failed to read saved configuration: {error}");
            None
        }
    };

    if let Err(error) = http_client::init(saved_url.as_deref()) {
        log::error!("[GlobalProxy] Failed to initialize saved configuration: {error}");
        if let Err(fallback_error) = http_client::init(None) {
            log::error!(
                "[GlobalProxy] Failed to initialize environment fallback: {fallback_error}"
            );
        }
    }
}

pub fn initialize_http_client_from_disk_best_effort() {
    match Database::read_global_proxy_url_from_disk_compatible() {
        Ok(saved_url) => {
            if let Err(error) = http_client::init(saved_url.as_deref()) {
                log::error!("[GlobalProxy] Failed to initialize saved configuration: {error}");
                if let Err(fallback_error) = http_client::init(None) {
                    log::error!(
                        "[GlobalProxy] Failed to initialize environment fallback: {fallback_error}"
                    );
                }
            }
        }
        Err(error) => {
            log::debug!("[GlobalProxy] Saved configuration unavailable: {error}");
            if let Err(init_error) = http_client::init(None) {
                log::error!(
                    "[GlobalProxy] Failed to initialize environment fallback: {init_error}"
                );
            }
        }
    }
}

pub async fn test(config: &GlobalOutboundProxyConfig) -> Result<GlobalProxyTestResult, AppError> {
    let full_url = config.to_full_url()?;
    if full_url.is_empty() {
        return Err(AppError::InvalidInput("Proxy URL is empty".to_string()));
    }

    let proxy = http_client::explicit_proxy(&full_url).map_err(AppError::InvalidInput)?;
    let client = reqwest::Client::builder()
        .proxy(proxy)
        .timeout(Duration::from_secs(5))
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| {
            AppError::Message(format!("Failed to build proxy test client: {error}"))
        })?;

    let started = Instant::now();
    let targets = [
        "https://httpbin.org/get",
        "https://www.google.com",
        "https://api.anthropic.com",
    ];
    let mut last_error = None;

    let attempts = async {
        for target in targets {
            match client.head(target).send().await {
                Ok(response)
                    if response.status().is_success()
                        || response.status().is_redirection()
                        || response.status().is_client_error()
                            && response.status()
                                != reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED =>
                {
                    return Some(GlobalProxyTestResult {
                        success: true,
                        latency_ms: started.elapsed().as_millis() as u64,
                        error: None,
                    });
                }
                Ok(response)
                    if response.status() == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED =>
                {
                    last_error = Some(format!(
                        "Proxy authentication failed: HTTP {}",
                        response.status()
                    ));
                }
                Ok(response) => {
                    last_error = Some(format!(
                        "Proxy test target returned HTTP {}",
                        response.status()
                    ));
                }
                Err(error) => {
                    last_error = Some(redact_error(&error.to_string(), &full_url));
                }
            }
        }
        None
    };

    match tokio::time::timeout(Duration::from_secs(5), attempts).await {
        Ok(Some(result)) => return Ok(result),
        Ok(None) => {}
        Err(_) => last_error = Some("Proxy test timed out".to_string()),
    }

    Ok(GlobalProxyTestResult {
        success: false,
        latency_ms: started.elapsed().as_millis() as u64,
        error: Some(last_error.unwrap_or_else(|| "All proxy test targets failed".to_string())),
    })
}

fn parse_proxy_url(raw: &str) -> Result<url::Url, AppError> {
    let parsed = url::Url::parse(raw).map_err(|_| {
        AppError::InvalidInput(
            crate::t!(
                "Invalid proxy URL. Enter a full URL, for example http://127.0.0.1:7890 or socks5://127.0.0.1:1080.",
                "代理 URL 无效。请输入完整地址，例如 http://127.0.0.1:7890 或 socks5://127.0.0.1:1080。"
            )
            .to_string(),
        )
    })?;
    if !matches!(parsed.scheme(), "http" | "https" | "socks5" | "socks5h") {
        return Err(AppError::InvalidInput(if crate::cli::i18n::is_chinese() {
            format!(
                "不支持代理协议“{}”。请使用 http、https、socks5 或 socks5h。",
                parsed.scheme()
            )
        } else {
            format!(
                "Unsupported proxy scheme '{}'. Use http, https, socks5, or socks5h.",
                parsed.scheme()
            )
        }));
    }
    if parsed.host_str().is_none() {
        return Err(AppError::InvalidInput(
            crate::t!(
                "The proxy URL must include a host, for example http://127.0.0.1:7890.",
                "代理 URL 必须包含主机，例如 http://127.0.0.1:7890。"
            )
            .to_string(),
        ));
    }
    Ok(parsed)
}

fn invalid_proxy_username() -> AppError {
    AppError::InvalidInput(
        crate::t!("The proxy username is invalid.", "代理用户名无效。").to_string(),
    )
}

fn invalid_proxy_password() -> AppError {
    AppError::InvalidInput(
        crate::t!("The proxy password is invalid.", "代理密码无效。").to_string(),
    )
}

fn decode_url_component(value: &str) -> String {
    percent_decode_str(value).decode_utf8_lossy().into_owned()
}

fn redact_error(message: &str, full_url: &str) -> String {
    let mut redacted = message.replace(full_url, &http_client::mask_url(full_url));
    if let Ok(config) = GlobalOutboundProxyConfig::from_full_url(full_url) {
        for secret in [&config.username, &config.password] {
            if !secret.is_empty() {
                redacted = redacted.replace(secret, "***");
            }
        }
    }
    redacted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_config_roundtrips_plaintext_credentials() {
        let parsed =
            GlobalOutboundProxyConfig::from_full_url("http://user%40name:p%3Ass@127.0.0.1:7890")
                .expect("parse proxy URL");

        assert_eq!(parsed.url, "http://127.0.0.1:7890/");
        assert_eq!(parsed.username, "user@name");
        assert_eq!(parsed.password, "p:ss");
        assert_eq!(
            parsed.to_full_url().expect("merge proxy URL"),
            "http://user%40name:p%3Ass@127.0.0.1:7890/"
        );
    }

    #[test]
    fn password_requires_username() {
        let config = GlobalOutboundProxyConfig {
            url: "socks5://127.0.0.1:1080".to_string(),
            username: String::new(),
            password: "secret".to_string(),
        };
        assert!(config.to_full_url().is_err());
    }

    #[test]
    fn username_with_empty_password_keeps_explicit_basic_auth() {
        let config = GlobalOutboundProxyConfig {
            url: "http://127.0.0.1:7890".to_string(),
            username: "alice".to_string(),
            password: String::new(),
        };

        assert_eq!(
            config.to_full_url().expect("build proxy URL"),
            "http://alice@127.0.0.1:7890/"
        );
        http_client::explicit_proxy(&config.to_full_url().expect("build proxy URL"))
            .expect("build authenticated proxy");
    }

    #[test]
    fn invalid_error_never_contains_credentials() {
        let full_url = "http://alice:secret@127.0.0.1:7890";
        let redacted = redact_error(
            &format!("could not connect to {full_url}; user alice password secret"),
            full_url,
        );
        assert!(!redacted.contains("alice"));
        assert!(!redacted.contains("secret"));
        assert!(redacted.contains("http://127.0.0.1:7890"));
    }

    #[test]
    fn malformed_url_error_never_contains_credentials() {
        for malformed in ["http://alice:supersecret", "1alice:secret@[@"] {
            let error = GlobalOutboundProxyConfig::from_full_url(malformed)
                .expect_err("malformed URL should fail")
                .to_string();
            assert!(!error.contains("alice"), "{error}");
            assert!(!error.contains("secret"), "{error}");
            assert!(error.contains("127.0.0.1:7890"), "{error}");
            assert!(!error.contains("relative URL"), "{error}");
        }
    }

    #[test]
    fn debug_output_redacts_credentials() {
        let config = GlobalOutboundProxyConfig {
            url: "http://alice:secret@127.0.0.1:7890".to_string(),
            username: "alice".to_string(),
            password: "secret".to_string(),
        };
        let debug = format!("{config:?}");
        assert!(!debug.contains("alice"));
        assert!(!debug.contains("secret"));
        assert!(debug.contains("http://127.0.0.1:7890"));
    }

    #[test]
    fn load_splits_persisted_proxy_credentials() {
        let db = Database::memory().expect("create database");
        db.set_global_proxy_url(Some("socks5://alice:secret@127.0.0.1:1080"))
            .expect("save proxy");

        let loaded = load(&db).expect("load proxy").expect("saved proxy");

        assert_eq!(loaded.url, "socks5://127.0.0.1:1080");
        assert_eq!(loaded.username, "alice");
        assert_eq!(loaded.password, "secret");
    }
}
