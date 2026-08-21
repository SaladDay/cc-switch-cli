use crate::provider::ProviderProxyConfig;
use once_cell::sync::OnceCell;
use percent_encoding::percent_decode_str;
use reqwest::{Client, ClientBuilder};
use std::collections::HashMap;
use std::env;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

static GLOBAL_CLIENT: OnceCell<RwLock<Client>> = OnceCell::new();
static CURRENT_PROXY_URL: OnceCell<RwLock<Option<String>>> = OnceCell::new();
static CC_SWITCH_PROXY_PORT: OnceCell<RwLock<Option<u16>>> = OnceCell::new();
type ProviderClientSlot = Arc<Mutex<Option<ProviderClientCacheEntry>>>;
static PROVIDER_CLIENTS: OnceCell<Mutex<HashMap<(String, String), ProviderClientSlot>>> =
    OnceCell::new();

struct ProviderClientCacheEntry {
    proxy_url: String,
    client: Client,
}

pub fn set_proxy_port(port: u16) {
    let changed = if let Some(lock) = CC_SWITCH_PROXY_PORT.get() {
        if let Ok(mut current_port) = lock.write() {
            let changed = *current_port != Some(port);
            *current_port = Some(port);
            log::debug!("[GlobalProxy] Updated CC Switch proxy port to {port}");
            changed
        } else {
            false
        }
    } else {
        let _ = CC_SWITCH_PROXY_PORT.set(RwLock::new(Some(port)));
        log::debug!("[GlobalProxy] Initialized CC Switch proxy port to {port}");
        true
    };

    refresh_environment_proxy_routing(changed);
}

pub fn clear_proxy_port(port: u16) {
    let changed = CC_SWITCH_PROXY_PORT
        .get()
        .and_then(|lock| lock.write().ok())
        .is_some_and(|mut current_port| {
            if *current_port == Some(port) {
                *current_port = None;
                log::debug!("[GlobalProxy] Cleared CC Switch proxy port {port}");
                true
            } else {
                false
            }
        });

    refresh_environment_proxy_routing(changed);
}

fn refresh_environment_proxy_routing(changed: bool) {
    if changed && GLOBAL_CLIENT.get().is_some() && get_current_proxy_url().is_none() {
        if let Err(error) = apply_proxy(None) {
            log::error!("[GlobalProxy] Failed to refresh environment proxy routing: {error}");
        }
    }
}

fn get_proxy_port() -> Option<u16> {
    CC_SWITCH_PROXY_PORT
        .get()
        .and_then(|lock| lock.read().ok())
        .and_then(|port| *port)
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
        .get_or_init(|| {
            log::warn!("[GlobalProxy] [GP-004] Client not initialized, initializing fallback");
            let client = build_client(None).unwrap_or_default();
            let _ = CURRENT_PROXY_URL.set(RwLock::new(None));
            RwLock::new(client)
        })
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
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
    configured_builder(Client::builder(), proxy_url)?
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(10)
        .tcp_keepalive(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("Failed to build HTTP client: {error}"))
}

pub fn builder() -> Result<ClientBuilder, String> {
    let proxy_url = get_current_proxy_url();
    configured_builder(Client::builder(), proxy_url.as_deref())
}

fn configured_builder(
    mut builder: ClientBuilder,
    proxy_url: Option<&str>,
) -> Result<ClientBuilder, String> {
    if let Some(url) = proxy_url {
        let proxy = explicit_proxy(url)?;
        builder = builder.proxy(proxy);
        log::debug!("[GlobalProxy] Proxy configured: {}", mask_url(url));
    } else if system_proxy_points_to_loopback() {
        builder = configure_safe_environment_proxies(builder);
        log::warn!("[GlobalProxy] Ignoring environment proxy entries that point back to CC Switch");
    } else {
        log::debug!("[GlobalProxy] Following system proxy (no explicit proxy configured)");
    }

    Ok(builder)
}

pub(crate) fn explicit_proxy(url: &str) -> Result<reqwest::Proxy, String> {
    let mut parsed = url::Url::parse(url)
        .map_err(|error| format!("Invalid proxy URL '{}': {}", mask_url(url), error))?;

    let scheme = parsed.scheme().to_string();
    if !["http", "https", "socks5", "socks5h"].contains(&scheme.as_str()) {
        return Err(format!(
            "Invalid proxy scheme '{}' in URL '{}'. Supported: http, https, socks5, socks5h",
            scheme,
            mask_url(url)
        ));
    }

    let username = percent_decode_str(parsed.username())
        .decode_utf8_lossy()
        .into_owned();
    let password = parsed
        .password()
        .map(|value| percent_decode_str(value).decode_utf8_lossy().into_owned())
        .unwrap_or_default();
    parsed
        .set_username("")
        .map_err(|_| "Invalid proxy username".to_string())?;
    parsed
        .set_password(None)
        .map_err(|_| "Invalid proxy password".to_string())?;

    let mut proxy = reqwest::Proxy::all(parsed.as_str())
        .map_err(|error| format!("Invalid proxy URL '{}': {}", mask_url(url), error))?;
    if !username.is_empty() {
        if matches!(scheme.as_str(), "socks5" | "socks5h") {
            if password.is_empty() {
                return Err("SOCKS proxy authentication requires a non-empty password".to_string());
            }
            proxy = proxy.basic_auth(&username, &password);
        } else {
            use base64::Engine as _;
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
            let mut header = reqwest::header::HeaderValue::from_str(&format!("Basic {encoded}"))
                .map_err(|_| "Invalid proxy credentials".to_string())?;
            header.set_sensitive(true);
            proxy = proxy.custom_http_auth(header);
        }
    }
    Ok(proxy)
}

fn system_proxy_points_to_loopback() -> bool {
    EnvironmentProxies::from_env().points_to_cc_switch()
}

#[derive(Clone)]
struct EnvironmentProxy {
    key: &'static str,
    value: String,
}

#[derive(Default)]
struct EnvironmentProxies {
    http: Option<EnvironmentProxy>,
    https: Option<EnvironmentProxy>,
    all: Option<EnvironmentProxy>,
}

impl EnvironmentProxies {
    fn from_env() -> Self {
        Self {
            http: first_environment_proxy(&["HTTP_PROXY", "http_proxy"]),
            https: first_environment_proxy(&["HTTPS_PROXY", "https_proxy"]),
            all: first_environment_proxy(&["ALL_PROXY", "all_proxy"]),
        }
    }

    fn points_to_cc_switch(&self) -> bool {
        [&self.http, &self.https, &self.all]
            .into_iter()
            .flatten()
            .any(|proxy| proxy_points_to_loopback(&proxy.value))
    }

    fn effective_variable_names(&self) -> Vec<String> {
        if env::var_os("REQUEST_METHOD").is_some() {
            return Vec::new();
        }

        [
            ("http", self.http.as_ref()),
            ("https", self.https.as_ref()),
            ("all", self.all.as_ref()),
        ]
        .into_iter()
        .filter_map(|(scope, proxy)| {
            let proxy = proxy?;
            if proxy_points_to_loopback(&proxy.value)
                || environment_proxy(scope, &proxy.value).is_err()
            {
                return None;
            }
            Some(proxy.key.to_string())
        })
        .collect()
    }
}

fn first_environment_proxy(keys: &[&'static str]) -> Option<EnvironmentProxy> {
    keys.iter()
        .find_map(|key| env::var(key).ok().map(|value| (*key, value)))
        .map(|(key, value)| EnvironmentProxy {
            key,
            value: value.trim().to_string(),
        })
        .filter(|proxy| !proxy.value.is_empty())
}

pub fn effective_environment_proxy_variables() -> Vec<String> {
    EnvironmentProxies::from_env().effective_variable_names()
}

fn environment_proxy(scope: &str, value: &str) -> Result<reqwest::Proxy, reqwest::Error> {
    match scope {
        "http" => reqwest::Proxy::http(value),
        "https" => reqwest::Proxy::https(value),
        _ => reqwest::Proxy::all(value),
    }
}

fn configure_safe_environment_proxies(mut builder: ClientBuilder) -> ClientBuilder {
    builder = builder.no_proxy();
    if env::var_os("REQUEST_METHOD").is_some() {
        return builder;
    }

    let proxies = EnvironmentProxies::from_env();
    let no_proxy = reqwest::NoProxy::from_env();
    for (scope, value) in [
        ("http", proxies.http),
        ("https", proxies.https),
        ("all", proxies.all),
    ] {
        let Some(proxy) = value.filter(|proxy| !proxy_points_to_loopback(&proxy.value)) else {
            continue;
        };
        match environment_proxy(scope, &proxy.value) {
            Ok(reqwest_proxy) => {
                builder = builder.proxy(reqwest_proxy.no_proxy(no_proxy.clone()));
            }
            Err(error) => {
                log::debug!(
                    "[GlobalProxy] Ignoring invalid {scope} environment proxy '{}': {error}",
                    mask_url(&proxy.value)
                );
            }
        }
    }
    builder
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
        get_proxy_port().is_some_and(|active_port| port == Some(active_port))
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

    "<invalid proxy URL>".to_string()
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
    if is_proxy_enabled() {
        return Some(get());
    }
    let config = proxy_config.filter(|config| config.enabled)?;
    let proxy_url = build_proxy_url_from_config(config)?;
    build_provider_proxy_client(&proxy_url)
}

fn build_provider_proxy_client(proxy_url: &str) -> Option<Client> {
    log::debug!(
        "[ProviderProxy] Building client with proxy: {}",
        mask_url(proxy_url)
    );

    let proxy = match reqwest::Proxy::all(proxy_url) {
        Ok(proxy) => proxy,
        Err(error) => {
            log::error!(
                "[ProviderProxy] Failed to create proxy from '{}': {}",
                mask_url(proxy_url),
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
                mask_url(proxy_url)
            );
            Some(client)
        }
        Err(error) => {
            log::error!("[ProviderProxy] Failed to build client: {error}");
            None
        }
    }
}

pub fn get_for_provider(
    app_type: &str,
    provider_id: &str,
    proxy_config: Option<&ProviderProxyConfig>,
) -> Client {
    if is_proxy_enabled() {
        return get();
    }
    let Some(config) = proxy_config.filter(|config| config.enabled) else {
        return get();
    };
    let Some(proxy_url) = build_proxy_url_from_config(config) else {
        return get();
    };

    let key = (app_type.to_string(), provider_id.to_string());
    let slot = {
        let clients = PROVIDER_CLIENTS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut clients = clients
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clients
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone()
    };
    let mut entry = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    if let Some(entry) = entry.as_ref() {
        if entry.proxy_url == proxy_url {
            return entry.client.clone();
        }
    }

    let Some(client) = build_provider_proxy_client(&proxy_url) else {
        return get();
    };
    *entry = Some(ProviderClientCacheEntry {
        proxy_url,
        client: client.clone(),
    });
    client
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
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
        for malformed in [
            "http://user:secret@[",
            "http://alice:supersecret",
            "1alice:secret@[@",
        ] {
            let masked = mask_url(malformed);
            assert_eq!(masked, "<invalid proxy URL>");
            assert!(!masked.contains("alice"));
            assert!(!masked.contains("secret"));
        }
    }

    #[test]
    fn test_build_client_direct() {
        assert!(build_client(None).is_ok());
    }

    #[test]
    fn reqwest_enables_webpki_and_native_root_stores() {
        Client::builder()
            .tls_built_in_webpki_certs(true)
            .tls_built_in_native_certs(true)
            .build()
            .expect("build reqwest client with WebPKI and native root stores");
    }

    #[test]
    fn provider_proxy_cache_replaces_changed_config() {
        let mut config = ProviderProxyConfig {
            enabled: true,
            proxy_type: Some("http".to_string()),
            proxy_host: Some("127.0.0.1".to_string()),
            proxy_port: Some(7890),
            ..ProviderProxyConfig::default()
        };
        let _ = get_for_provider("cache-test", "replace-config", Some(&config));

        config.proxy_port = Some(7891);
        let _ = get_for_provider("cache-test", "replace-config", Some(&config));

        let clients = PROVIDER_CLIENTS
            .get()
            .expect("provider cache initialized")
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let slot = clients
            .get(&("cache-test".to_string(), "replace-config".to_string()))
            .expect("provider cache entry");
        let entry = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = entry.as_ref().expect("initialized provider client");
        assert_eq!(entry.proxy_url, "http://127.0.0.1:7891");
    }

    #[test]
    fn test_build_client_with_http_proxy() {
        assert!(build_client(Some("http://127.0.0.1:7890")).is_ok());
    }

    #[test]
    fn http_proxy_username_with_empty_password_is_applied() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake proxy");
        listener
            .set_nonblocking(true)
            .expect("configure fake proxy");
        let address = listener.local_addr().expect("fake proxy address");
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .expect("configure proxy connection");
                        let mut buffer = [0_u8; 4096];
                        let count = stream.read(&mut buffer).expect("read proxy request");
                        stream
                            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                            .expect("write proxy response");
                        return Some(String::from_utf8_lossy(&buffer[..count]).into_owned());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept proxy request: {error}"),
                }
            }
            None
        });

        let proxy =
            explicit_proxy(&format!("http://alice@{address}")).expect("build authenticated proxy");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        let response = runtime.block_on(async move {
            let client = Client::builder()
                .proxy(proxy)
                .timeout(Duration::from_secs(2))
                .build()
                .expect("build proxy client");
            client.get("http://127.0.0.1:1/probe").send().await
        });
        let request = server
            .join()
            .expect("join fake proxy")
            .expect("request should reach the proxy");

        assert_eq!(response.expect("proxy response").status(), 204);
        assert!(request
            .lines()
            .any(|line| { line.eq_ignore_ascii_case("proxy-authorization: Basic YWxpY2U6") }));
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
        let _guard = env_lock().lock().unwrap();
        clear_proxy_port(15721);

        assert!(!proxy_points_to_loopback("http://127.0.0.1:15721"));
        set_proxy_port(15721);

        assert!(proxy_points_to_loopback("http://127.0.0.1:15721"));
        assert!(proxy_points_to_loopback("socks5://localhost:15721"));
        assert!(proxy_points_to_loopback("127.0.0.1:15721"));

        assert!(!proxy_points_to_loopback("http://127.0.0.1:7890"));
        assert!(!proxy_points_to_loopback("socks5://localhost:1080"));
        assert!(!proxy_points_to_loopback("http://192.168.1.10:7890"));
        assert!(!proxy_points_to_loopback("http://192.168.1.10:15721"));

        set_proxy_port(16000);
        assert!(proxy_points_to_loopback("http://127.0.0.1:16000"));
        assert!(!proxy_points_to_loopback("http://127.0.0.1:15721"));
        clear_proxy_port(16000);
        assert!(!proxy_points_to_loopback("http://127.0.0.1:16000"));
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

        std::env::set_var("HTTP_PROXY", "http://127.0.0.1:15721");
        std::env::set_var("HTTPS_PROXY", "http://10.0.0.3:8443");
        let proxies = EnvironmentProxies::from_env();
        assert!(proxies
            .http
            .as_ref()
            .is_some_and(|proxy| proxy_points_to_loopback(&proxy.value)));
        assert_eq!(
            proxies.https.as_ref().map(|proxy| proxy.value.as_str()),
            Some("http://10.0.0.3:8443")
        );
        assert_eq!(proxies.effective_variable_names(), vec!["HTTPS_PROXY"]);
        assert!(configure_safe_environment_proxies(Client::builder())
            .build()
            .is_ok());

        std::env::set_var("HTTP_PROXY", "http://[");
        std::env::remove_var("HTTPS_PROXY");
        assert!(EnvironmentProxies::from_env()
            .effective_variable_names()
            .is_empty());

        std::env::set_var("HTTP_PROXY", "http://10.0.0.2:7890");
        std::env::set_var("REQUEST_METHOD", "GET");
        assert!(EnvironmentProxies::from_env()
            .effective_variable_names()
            .is_empty());
        std::env::remove_var("REQUEST_METHOD");

        for key in &keys {
            std::env::remove_var(key);
        }
        clear_proxy_port(15721);
    }
}
