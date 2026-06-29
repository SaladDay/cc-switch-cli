use crate::provider::ProviderProxyConfig;
use once_cell::sync::OnceCell;
use reqwest::Client;
use std::env;
use std::net::IpAddr;
use std::sync::RwLock;
use std::time::Duration;

static GLOBAL_CLIENT: OnceCell<RwLock<Client>> = OnceCell::new();
static CURRENT_PROXY_URL: OnceCell<RwLock<Option<String>>> = OnceCell::new();
static CC_SWITCH_PROXY_PORT: OnceCell<RwLock<u16>> = OnceCell::new();

pub fn set_proxy_port(port: u16) {
    if let Some(lock) = CC_SWITCH_PROXY_PORT.get() {
        if let Ok(mut current_port) = lock.write() {
            *current_port = port;
            log::debug!("[GlobalProxy] Updated CC Switch proxy port to {port}");
        }
    } else {
        let _ = CC_SWITCH_PROXY_PORT.set(RwLock::new(port));
        log::debug!("[GlobalProxy] Initialized CC Switch proxy port to {port}");
    }
}

fn get_proxy_port() -> u16 {
    CC_SWITCH_PROXY_PORT
        .get()
        .and_then(|lock| lock.read().ok())
        .map(|port| *port)
        .unwrap_or(15721)
}

pub fn init(proxy_url: Option<&str>) -> Result<(), String> {
    let effective_url = proxy_url.filter(|value| !value.trim().is_empty());
    let client = build_client(effective_url)?;

    if GLOBAL_CLIENT.set(RwLock::new(client.clone())).is_err() {
        log::warn!(
            "[GlobalProxy] [GP-003] Already initialized, updating instead: {}",
            effective_url
                .map(mask_url)
                .unwrap_or_else(|| "direct connection".to_string())
        );
        return apply_proxy(proxy_url);
    }

    let _ = CURRENT_PROXY_URL.set(RwLock::new(effective_url.map(|value| value.to_string())));

    log::info!(
        "[GlobalProxy] Initialized: {}",
        effective_url
            .map(mask_url)
            .unwrap_or_else(|| "direct connection".to_string())
    );

    Ok(())
}

#[allow(dead_code)]
pub fn validate_proxy(proxy_url: Option<&str>) -> Result<(), String> {
    let effective_url = proxy_url.filter(|value| !value.trim().is_empty());
    build_client(effective_url)?;
    Ok(())
}

pub fn apply_proxy(proxy_url: Option<&str>) -> Result<(), String> {
    let effective_url = proxy_url.filter(|value| !value.trim().is_empty());
    let new_client = build_client(effective_url)?;

    if let Some(lock) = GLOBAL_CLIENT.get() {
        let mut client = lock.write().map_err(|error| {
            log::error!("[GlobalProxy] [GP-001] Failed to acquire write lock: {error}");
            "Failed to update proxy: lock poisoned".to_string()
        })?;
        *client = new_client;
    } else {
        return init(proxy_url);
    }

    if let Some(lock) = CURRENT_PROXY_URL.get() {
        let mut url = lock.write().map_err(|error| {
            log::error!("[GlobalProxy] [GP-002] Failed to acquire URL write lock: {error}");
            "Failed to update proxy URL record: lock poisoned".to_string()
        })?;
        *url = effective_url.map(|value| value.to_string());
    }

    log::info!(
        "[GlobalProxy] Applied: {}",
        effective_url
            .map(mask_url)
            .unwrap_or_else(|| "direct connection".to_string())
    );

    Ok(())
}

#[allow(dead_code)]
pub fn update_proxy(proxy_url: Option<&str>) -> Result<(), String> {
    let effective_url = proxy_url.filter(|value| !value.trim().is_empty());
    let new_client = build_client(effective_url)?;

    if let Some(lock) = GLOBAL_CLIENT.get() {
        let mut client = lock.write().map_err(|error| {
            log::error!("[GlobalProxy] [GP-001] Failed to acquire write lock: {error}");
            "Failed to update proxy: lock poisoned".to_string()
        })?;
        *client = new_client;
    } else {
        return init(proxy_url);
    }

    if let Some(lock) = CURRENT_PROXY_URL.get() {
        let mut url = lock.write().map_err(|error| {
            log::error!("[GlobalProxy] [GP-002] Failed to acquire URL write lock: {error}");
            "Failed to update proxy URL record: lock poisoned".to_string()
        })?;
        *url = effective_url.map(|value| value.to_string());
    }

    log::info!(
        "[GlobalProxy] Updated: {}",
        effective_url
            .map(mask_url)
            .unwrap_or_else(|| "direct connection".to_string())
    );

    Ok(())
}

pub fn get() -> Client {
    GLOBAL_CLIENT
        .get()
        .and_then(|lock| lock.read().ok())
        .map(|client| client.clone())
        .unwrap_or_else(|| {
            log::warn!("[GlobalProxy] [GP-004] Client not initialized, using fallback");
            build_client(None).unwrap_or_default()
        })
}

pub fn get_current_proxy_url() -> Option<String> {
    CURRENT_PROXY_URL
        .get()
        .and_then(|lock| lock.read().ok())
        .and_then(|url| url.clone())
}

#[allow(dead_code)]
pub fn is_proxy_enabled() -> bool {
    get_current_proxy_url().is_some()
}

fn build_client(proxy_url: Option<&str>) -> Result<Client, String> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(10)
        .tcp_keepalive(Duration::from_secs(60));

    if let Some(url) = proxy_url {
        let parsed = url::Url::parse(url)
            .map_err(|error| format!("Invalid proxy URL '{}': {}", mask_url(url), error))?;

        let scheme = parsed.scheme();
        if !["http", "https", "socks5", "socks5h"].contains(&scheme) {
            return Err(format!(
                "Invalid proxy scheme '{}' in URL '{}'. Supported: http, https, socks5, socks5h",
                scheme,
                mask_url(url)
            ));
        }

        let proxy = reqwest::Proxy::all(url)
            .map_err(|error| format!("Invalid proxy URL '{}': {}", mask_url(url), error))?;
        builder = builder.proxy(proxy);
        log::debug!("[GlobalProxy] Proxy configured: {}", mask_url(url));
    } else if system_proxy_points_to_loopback() {
        builder = builder.no_proxy();
        log::warn!("[GlobalProxy] System proxy points to localhost, bypassing to avoid recursion");
    } else {
        log::debug!("[GlobalProxy] Following system proxy (no explicit proxy configured)");
    }

    builder
        .build()
        .map_err(|error| format!("Failed to build HTTP client: {error}"))
}

fn system_proxy_points_to_loopback() -> bool {
    const KEYS: [&str; 6] = [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];

    KEYS.iter()
        .filter_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .any(|value| proxy_points_to_loopback(&value))
}

fn proxy_points_to_loopback(value: &str) -> bool {
    fn host_is_loopback(host: &str) -> bool {
        if host.eq_ignore_ascii_case("localhost") {
            return true;
        }

        host.parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
    }

    fn is_cc_switch_proxy_port(port: Option<u16>) -> bool {
        port == Some(get_proxy_port())
    }

    if let Ok(parsed) = url::Url::parse(value) {
        if let Some(host) = parsed.host_str() {
            return host_is_loopback(host) && is_cc_switch_proxy_port(parsed.port());
        }
        return false;
    }

    let with_scheme = format!("http://{value}");
    if let Ok(parsed) = url::Url::parse(&with_scheme) {
        if let Some(host) = parsed.host_str() {
            return host_is_loopback(host) && is_cc_switch_proxy_port(parsed.port());
        }
    }

    false
}

pub fn mask_url(url: &str) -> String {
    if let Ok(parsed) = url::Url::parse(url) {
        let host = parsed.host_str().unwrap_or("?");
        return match parsed.port() {
            Some(port) => format!("{}://{}:{}", parsed.scheme(), host, port),
            None => format!("{}://{}", parsed.scheme(), host),
        };
    }

    if url.len() > 20 {
        format!("{}...", &url[..20])
    } else {
        url.to_string()
    }
}

fn build_proxy_url_from_config(config: &ProviderProxyConfig) -> Option<String> {
    let proxy_type = config.proxy_type.as_deref().unwrap_or("http");
    let host = config.proxy_host.as_deref()?;
    let port = config.proxy_port?;

    if let (Some(username), Some(password)) = (&config.proxy_username, &config.proxy_password) {
        if !username.is_empty() && !password.is_empty() {
            return Some(format!(
                "{proxy_type}://{username}:{password}@{host}:{port}"
            ));
        }
    }

    Some(format!("{proxy_type}://{host}:{port}"))
}

pub fn build_client_for_provider(proxy_config: Option<&ProviderProxyConfig>) -> Option<Client> {
    let config = proxy_config.filter(|config| config.enabled)?;
    let proxy_url = build_proxy_url_from_config(config)?;

    log::debug!(
        "[ProviderProxy] Building client with proxy: {}",
        mask_url(&proxy_url)
    );

    let proxy = match reqwest::Proxy::all(&proxy_url) {
        Ok(proxy) => proxy,
        Err(error) => {
            log::error!(
                "[ProviderProxy] Failed to create proxy from '{}': {}",
                mask_url(&proxy_url),
                error
            );
            return None;
        }
    };

    match Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(10)
        .tcp_keepalive(Duration::from_secs(60))
        .proxy(proxy)
        .build()
    {
        Ok(client) => {
            log::info!(
                "[ProviderProxy] Client built with proxy: {}",
                mask_url(&proxy_url)
            );
            Some(client)
        }
        Err(error) => {
            log::error!("[ProviderProxy] Failed to build client: {error}");
            None
        }
    }
}

pub fn get_for_provider(proxy_config: Option<&ProviderProxyConfig>) -> Client {
    if let Some(client) = build_client_for_provider(proxy_config) {
        return client;
    }

    get()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn test_mask_url() {
        assert_eq!(mask_url("http://127.0.0.1:7890"), "http://127.0.0.1:7890");
        assert_eq!(
            mask_url("http://user:pass@127.0.0.1:7890"),
            "http://127.0.0.1:7890"
        );
        assert_eq!(
            mask_url("socks5://admin:secret@proxy.example.com:1080"),
            "socks5://proxy.example.com:1080"
        );
        assert_eq!(
            mask_url("http://proxy.example.com"),
            "http://proxy.example.com"
        );
        assert_eq!(
            mask_url("https://user:pass@proxy.example.com"),
            "https://proxy.example.com"
        );
    }

    #[test]
    fn test_build_client_direct() {
        assert!(build_client(None).is_ok());
    }

    #[test]
    fn test_build_client_with_http_proxy() {
        assert!(build_client(Some("http://127.0.0.1:7890")).is_ok());
    }

    #[test]
    fn test_build_client_with_socks5_proxy() {
        assert!(build_client(Some("socks5://127.0.0.1:1080")).is_ok());
    }

    #[test]
    fn test_build_client_invalid_url() {
        let result = build_client(Some("invalid-scheme://127.0.0.1:7890"));
        assert!(result.is_err(), "Should reject invalid proxy scheme");
    }

    #[test]
    fn test_proxy_points_to_loopback() {
        set_proxy_port(15721);

        assert!(proxy_points_to_loopback("http://127.0.0.1:15721"));
        assert!(proxy_points_to_loopback("socks5://localhost:15721"));
        assert!(proxy_points_to_loopback("127.0.0.1:15721"));

        assert!(!proxy_points_to_loopback("http://127.0.0.1:7890"));
        assert!(!proxy_points_to_loopback("socks5://localhost:1080"));
        assert!(!proxy_points_to_loopback("http://192.168.1.10:7890"));
        assert!(!proxy_points_to_loopback("http://192.168.1.10:15721"));
    }

    #[test]
    fn test_system_proxy_points_to_loopback() {
        let _guard = env_lock().lock().unwrap();
        set_proxy_port(15721);

        let keys = [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ];

        for key in &keys {
            std::env::remove_var(key);
        }

        std::env::set_var("HTTP_PROXY", "http://127.0.0.1:15721");
        assert!(system_proxy_points_to_loopback());

        std::env::set_var("HTTP_PROXY", "http://127.0.0.1:7890");
        assert!(!system_proxy_points_to_loopback());

        std::env::set_var("HTTP_PROXY", "http://10.0.0.2:7890");
        assert!(!system_proxy_points_to_loopback());

        for key in &keys {
            std::env::remove_var(key);
        }
    }
}
