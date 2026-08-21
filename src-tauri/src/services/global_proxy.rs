use percent_encoding::percent_decode_str;

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

#[derive(Clone, Default, PartialEq, Eq)]
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
            .field("username", &(!self.username.is_empty()).then_some("***"))
            .field("password", &(!self.password.is_empty()).then_some("***"))
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
            return Ok(String::new());
        }

        let mut parsed = parse_proxy_url(raw_url)?;
        if !self.username.trim().is_empty() {
            parsed
                .set_username(self.username.trim())
                .map_err(|_| invalid_proxy_username())?;
        }
        if !self.password.is_empty() {
            parsed
                .set_password(Some(self.password.as_str()))
                .map_err(|_| invalid_proxy_password())?;
        } else if !self.username.trim().is_empty() {
            parsed
                .set_password(None)
                .map_err(|_| invalid_proxy_password())?;
        }

        let full_url = parsed.to_string();
        http_client::validate_proxy(Some(&full_url)).map_err(AppError::InvalidInput)?;
        Ok(full_url)
    }
}

pub fn load(db: &Database) -> Result<Option<GlobalOutboundProxyConfig>, AppError> {
    db.get_global_proxy_url()?
        .map(|url| GlobalOutboundProxyConfig::from_full_url(&url))
        .transpose()
}

pub fn set(
    state: &AppState,
    config: &GlobalOutboundProxyConfig,
) -> Result<Option<String>, AppError> {
    let full_url = config.to_full_url()?;
    update(state, Some(full_url.as_str()))
}

pub fn clear(state: &AppState) -> Result<Option<String>, AppError> {
    update(state, None)
}

fn update(state: &AppState, full_url: Option<&str>) -> Result<Option<String>, AppError> {
    http_client::validate_proxy(full_url).map_err(AppError::InvalidInput)?;
    state.db.set_global_proxy_url(full_url)?;
    http_client::apply_proxy(full_url).map_err(AppError::Message)?;

    #[cfg(unix)]
    let reload_warning = [
        crate::daemon::notify_outbound_proxy_reload().err(),
        state
            .proxy_service
            .notify_foreground_outbound_proxy_reload()
            .err(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    #[cfg(not(unix))]
    let reload_warning: Vec<String> = Vec::new();

    Ok((!reload_warning.is_empty()).then(|| reload_warning.join("; ")))
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

pub fn initialize_http_client(db: &Database) {
    let saved_url = match db.get_global_proxy_url() {
        Ok(url) => url,
        Err(error) => {
            log::error!("[GlobalProxy] Failed to read saved configuration: {error}");
            None
        }
    };
    if initialize(saved_url.as_deref()).is_err() && saved_url.is_some() {
        if let Err(error) = db.set_global_proxy_url(None) {
            log::error!("[GlobalProxy] Failed to clear invalid configuration: {error}");
        }
    }
}

pub fn initialize_http_client_from_disk_best_effort() {
    let saved_url = match Database::read_global_proxy_url_from_disk_compatible() {
        Ok(url) => url,
        Err(error) => {
            log::debug!("[GlobalProxy] Saved configuration unavailable: {error}");
            None
        }
    };
    let _ = initialize(saved_url.as_deref());
}

fn initialize(saved_url: Option<&str>) -> Result<(), ()> {
    if let Err(error) = http_client::init(saved_url) {
        log::error!("[GlobalProxy] Failed to initialize saved configuration: {error}");
        if let Err(error) = http_client::init(None) {
            log::error!("[GlobalProxy] Failed to initialize environment fallback: {error}");
        }
        return Err(());
    }
    Ok(())
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
        return Err(AppError::InvalidInput(if crate::i18n::is_chinese() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_config_roundtrips_credentials() {
        let config =
            GlobalOutboundProxyConfig::from_full_url("http://user%40name:p%3Ass@127.0.0.1:7890")
                .expect("parse proxy URL");

        assert_eq!(config.url, "http://127.0.0.1:7890/");
        assert_eq!(config.username, "user@name");
        assert_eq!(config.password, "p:ss");
        assert_eq!(
            config.to_full_url().expect("build proxy URL"),
            "http://user%40name:p%3Ass@127.0.0.1:7890/"
        );
    }

    #[test]
    fn full_url_credentials_are_preserved_when_fields_are_empty() {
        let config = GlobalOutboundProxyConfig {
            url: "http://alice:secret@127.0.0.1:7890".to_string(),
            ..Default::default()
        };

        assert_eq!(
            config.to_full_url().expect("build proxy URL"),
            "http://alice:secret@127.0.0.1:7890/"
        );
    }

    #[test]
    fn separate_password_merges_with_embedded_or_empty_username() {
        let embedded_username = GlobalOutboundProxyConfig {
            url: "http://alice@127.0.0.1:7890".to_string(),
            password: "secret".to_string(),
            ..Default::default()
        };
        assert_eq!(
            embedded_username.to_full_url().expect("build proxy URL"),
            "http://alice:secret@127.0.0.1:7890/"
        );

        let empty_username = GlobalOutboundProxyConfig {
            url: "http://127.0.0.1:7890".to_string(),
            password: "secret".to_string(),
            ..Default::default()
        };
        assert_eq!(
            empty_username.to_full_url().expect("build proxy URL"),
            "http://:secret@127.0.0.1:7890/"
        );
    }

    #[test]
    fn invalid_url_has_readable_error() {
        let error = GlobalOutboundProxyConfig::from_full_url("Extu^")
            .expect_err("invalid URL should fail")
            .to_string();

        assert!(error.contains("Invalid proxy URL"), "{error}");
        assert!(!error.contains("relative URL"), "{error}");
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
    }

    #[test]
    fn load_splits_persisted_credentials() {
        let db = Database::memory().expect("create database");
        db.set_global_proxy_url(Some("socks5://alice:secret@127.0.0.1:1080"))
            .expect("save proxy");

        let loaded = load(&db).expect("load proxy").expect("saved proxy");

        assert_eq!(loaded.url, "socks5://127.0.0.1:1080");
        assert_eq!(loaded.username, "alice");
        assert_eq!(loaded.password, "secret");
    }
}
