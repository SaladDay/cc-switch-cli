use rquickjs::{Context, Function, Runtime};
use serde_json::Value;
use std::collections::HashMap;
use url::{Host, Url};

use crate::error::AppError;

/// 执行用量查询脚本
pub async fn execute_usage_script(
    script_code: &str,
    api_key: &str,
    base_url: &str,
    timeout_secs: u64,
    access_token: Option<&str>,
    user_id: Option<&str>,
    template_type: Option<&str>,
) -> Result<Value, AppError> {
    let is_custom_template = template_type.map(|t| t == "custom").unwrap_or(false);
    let replaced = build_script_with_vars(script_code, api_key, base_url, access_token, user_id);

    if !base_url.is_empty() {
        validate_base_url(base_url)?;
    }

    // 2. 在独立作用域中提取 request 配置（确保 Runtime/Context 在 await 前释放）
    let request_config = {
        let runtime = Runtime::new().map_err(|e| {
            AppError::localized(
                "usage_script.runtime_create_failed",
                format!("创建 JS 运行时失败: {e}"),
                format!("Failed to create JS runtime: {e}"),
            )
        })?;
        let context = Context::full(&runtime).map_err(|e| {
            AppError::localized(
                "usage_script.context_create_failed",
                format!("创建 JS 上下文失败: {e}"),
                format!("Failed to create JS context: {e}"),
            )
        })?;

        context.with(|ctx| {
            // 执行用户代码，获取配置对象
            let config: rquickjs::Object = ctx.eval(replaced.clone()).map_err(|e| {
                AppError::localized(
                    "usage_script.config_parse_failed",
                    format!("解析配置失败: {e}"),
                    format!("Failed to parse config: {e}"),
                )
            })?;

            // 提取 request 配置
            let request: rquickjs::Object = config.get("request").map_err(|e| {
                AppError::localized(
                    "usage_script.request_missing",
                    format!("缺少 request 配置: {e}"),
                    format!("Missing request config: {e}"),
                )
            })?;

            // 将 request 转换为 JSON 字符串
            let request_json: String = ctx
                .json_stringify(request)
                .map_err(|e| {
                    AppError::localized(
                        "usage_script.request_serialize_failed",
                        format!("序列化 request 失败: {e}"),
                        format!("Failed to serialize request: {e}"),
                    )
                })?
                .ok_or_else(|| {
                    AppError::localized(
                        "usage_script.serialize_none",
                        "序列化返回 None",
                        "Serialization returned None",
                    )
                })?
                .get()
                .map_err(|e| {
                    AppError::localized(
                        "usage_script.get_string_failed",
                        format!("获取字符串失败: {e}"),
                        format!("Failed to get string: {e}"),
                    )
                })?;

            Ok::<_, AppError>(request_json)
        })?
    }; // Runtime 和 Context 在这里被 drop

    // 3. 解析 request 配置
    let request: RequestConfig = serde_json::from_str(&request_config).map_err(|e| {
        AppError::localized(
            "usage_script.request_format_invalid",
            format!("request 配置格式错误: {e}"),
            format!("Invalid request config format: {e}"),
        )
    })?;

    validate_request_url(&request.url, base_url, is_custom_template)?;

    // 4. 发送 HTTP 请求
    let response_data = send_http_request(&request, timeout_secs).await?;

    // 5. 在独立作用域中执行 extractor（确保 Runtime/Context 在函数结束前释放）
    let result: Value = {
        let runtime = Runtime::new().map_err(|e| {
            AppError::localized(
                "usage_script.runtime_create_failed",
                format!("创建 JS 运行时失败: {e}"),
                format!("Failed to create JS runtime: {e}"),
            )
        })?;
        let context = Context::full(&runtime).map_err(|e| {
            AppError::localized(
                "usage_script.context_create_failed",
                format!("创建 JS 上下文失败: {e}"),
                format!("Failed to create JS context: {e}"),
            )
        })?;

        context.with(|ctx| {
            // 重新 eval 获取配置对象
            let config: rquickjs::Object = ctx.eval(replaced.clone()).map_err(|e| {
                AppError::localized(
                    "usage_script.config_reparse_failed",
                    format!("重新解析配置失败: {e}"),
                    format!("Failed to re-parse config: {e}"),
                )
            })?;

            // 提取 extractor 函数
            let extractor: Function = config.get("extractor").map_err(|e| {
                AppError::localized(
                    "usage_script.extractor_missing",
                    format!("缺少 extractor 函数: {e}"),
                    format!("Missing extractor function: {e}"),
                )
            })?;

            // 将响应数据转换为 JS 值
            let response_js: rquickjs::Value =
                ctx.json_parse(response_data.as_str()).map_err(|e| {
                    AppError::localized(
                        "usage_script.response_parse_failed",
                        format!("解析响应 JSON 失败: {e}"),
                        format!("Failed to parse response JSON: {e}"),
                    )
                })?;

            // 调用 extractor(response)
            let result_js: rquickjs::Value = extractor.call((response_js,)).map_err(|e| {
                AppError::localized(
                    "usage_script.extractor_exec_failed",
                    format!("执行 extractor 失败: {e}"),
                    format!("Failed to execute extractor: {e}"),
                )
            })?;

            // 转换为 JSON 字符串
            let result_json: String = ctx
                .json_stringify(result_js)
                .map_err(|e| {
                    AppError::localized(
                        "usage_script.result_serialize_failed",
                        format!("序列化结果失败: {e}"),
                        format!("Failed to serialize result: {e}"),
                    )
                })?
                .ok_or_else(|| {
                    AppError::localized(
                        "usage_script.serialize_none",
                        "序列化返回 None",
                        "Serialization returned None",
                    )
                })?
                .get()
                .map_err(|e| {
                    AppError::localized(
                        "usage_script.get_string_failed",
                        format!("获取字符串失败: {e}"),
                        format!("Failed to get string: {e}"),
                    )
                })?;

            // 解析为 serde_json::Value
            serde_json::from_str(&result_json).map_err(|e| {
                AppError::localized(
                    "usage_script.json_parse_failed",
                    format!("JSON 解析失败: {e}"),
                    format!("JSON parse failed: {e}"),
                )
            })
        })?
    }; // Runtime 和 Context 在这里被 drop

    // 6. 验证返回值格式
    validate_result(&result)?;

    Ok(result)
}

/// 请求配置结构
#[derive(Debug, serde::Deserialize)]
struct RequestConfig {
    url: String,
    method: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
}

/// 发送 HTTP 请求
async fn send_http_request(config: &RequestConfig, timeout_secs: u64) -> Result<String, AppError> {
    let client = crate::proxy::http_client::get();
    let request_timeout = std::time::Duration::from_secs(timeout_secs.clamp(2, 30));

    // 严格校验 HTTP 方法，非法值不回退为 GET
    let method: reqwest::Method = config.method.parse().map_err(|_| {
        AppError::localized(
            "usage_script.invalid_http_method",
            format!("不支持的 HTTP 方法: {}", config.method),
            format!("Unsupported HTTP method: {}", config.method),
        )
    })?;

    let mut req = client
        .request(method.clone(), &config.url)
        .timeout(request_timeout);

    // 添加请求头
    for (k, v) in &config.headers {
        req = req.header(k, v);
    }

    // 添加请求体
    if let Some(body) = &config.body {
        req = req.body(body.clone());
    }

    // 发送请求
    let resp = req.send().await.map_err(|e| {
        AppError::localized(
            "usage_script.request_failed",
            format!("请求失败: {e}"),
            format!("Request failed: {e}"),
        )
    })?;

    let status = resp.status();
    let text = resp.text().await.map_err(|e| {
        AppError::localized(
            "usage_script.read_response_failed",
            format!("读取响应失败: {e}"),
            format!("Failed to read response: {e}"),
        )
    })?;

    if !status.is_success() {
        let preview = if text.len() > 200 {
            let mut safe_cut = 200usize;
            while !text.is_char_boundary(safe_cut) {
                safe_cut = safe_cut.saturating_sub(1);
            }
            format!("{}...", &text[..safe_cut])
        } else {
            text.clone()
        };
        return Err(AppError::localized(
            "usage_script.http_error",
            format!("HTTP {status} : {preview}"),
            format!("HTTP {status} : {preview}"),
        ));
    }

    Ok(text)
}

fn build_script_with_vars(
    script_code: &str,
    api_key: &str,
    base_url: &str,
    access_token: Option<&str>,
    user_id: Option<&str>,
) -> String {
    let mut replaced = script_code
        .replace("{{apiKey}}", api_key)
        .replace("{{baseUrl}}", base_url);

    if let Some(token) = access_token {
        replaced = replaced.replace("{{accessToken}}", token);
    }
    if let Some(uid) = user_id {
        replaced = replaced.replace("{{userId}}", uid);
    }

    replaced
}

fn validate_base_url(base_url: &str) -> Result<(), AppError> {
    if base_url.is_empty() {
        return Err(AppError::localized(
            "usage_script.base_url_empty",
            "base_url 不能为空",
            "base_url cannot be empty",
        ));
    }

    let parsed_url = Url::parse(base_url).map_err(|e| {
        AppError::localized(
            "usage_script.base_url_invalid",
            format!("无效的 base_url: {e}"),
            format!("Invalid base_url: {e}"),
        )
    })?;

    if parsed_url.scheme() != "https" && !is_loopback_host(&parsed_url) {
        return Err(AppError::localized(
            "usage_script.base_url_https_required",
            "base_url 必须使用 HTTPS 协议（localhost 除外）",
            "base_url must use HTTPS (localhost allowed)",
        ));
    }

    let hostname = parsed_url.host_str().ok_or_else(|| {
        AppError::localized(
            "usage_script.base_url_hostname_missing",
            "base_url 必须包含有效的主机名",
            "base_url must include a valid hostname",
        )
    })?;

    if hostname.is_empty() {
        return Err(AppError::localized(
            "usage_script.base_url_hostname_empty",
            "base_url 主机名不能为空",
            "base_url hostname cannot be empty",
        ));
    }

    Ok(())
}

fn validate_request_url(
    request_url: &str,
    base_url: &str,
    is_custom_template: bool,
) -> Result<(), AppError> {
    let parsed_request = Url::parse(request_url).map_err(|e| {
        AppError::localized(
            "usage_script.request_url_invalid",
            format!("无效的请求 URL: {e}"),
            format!("Invalid request URL: {e}"),
        )
    })?;

    let is_request_loopback = is_loopback_host(&parsed_request);

    if !is_custom_template && parsed_request.scheme() != "https" && !is_request_loopback {
        return Err(AppError::localized(
            "usage_script.request_https_required",
            "请求 URL 必须使用 HTTPS 协议（localhost 除外）",
            "Request URL must use HTTPS (localhost allowed)",
        ));
    }

    if !base_url.is_empty() && !is_custom_template {
        let parsed_base = Url::parse(base_url).map_err(|e| {
            AppError::localized(
                "usage_script.base_url_invalid",
                format!("无效的 base_url: {e}"),
                format!("Invalid base_url: {e}"),
            )
        })?;

        if parsed_request.host_str() != parsed_base.host_str() {
            return Err(AppError::localized(
                "usage_script.request_host_mismatch",
                format!(
                    "请求域名 {} 与 base_url 域名 {} 不匹配（必须是同源请求）",
                    parsed_request.host_str().unwrap_or("unknown"),
                    parsed_base.host_str().unwrap_or("unknown")
                ),
                format!(
                    "Request host {} must match base_url host {} (same-origin required)",
                    parsed_request.host_str().unwrap_or("unknown"),
                    parsed_base.host_str().unwrap_or("unknown")
                ),
            ));
        }

        match (
            parsed_request.port_or_known_default(),
            parsed_base.port_or_known_default(),
        ) {
            (Some(request_port), Some(base_port)) if request_port == base_port => Ok(()),
            (Some(request_port), Some(base_port)) => Err(AppError::localized(
                "usage_script.request_port_mismatch",
                format!("请求端口 {request_port} 必须与 base_url 端口 {base_port} 匹配"),
                format!("Request port {request_port} must match base_url port {base_port}"),
            )),
            _ => Err(AppError::localized(
                "usage_script.request_port_unknown",
                "无法确定端口号",
                "Unable to determine port number",
            )),
        }?;
    }

    Ok(())
}

fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    }
}

/// 验证脚本返回值（支持单对象或数组）
fn validate_result(result: &Value) -> Result<(), AppError> {
    // 如果是数组，验证每个元素
    if let Some(arr) = result.as_array() {
        if arr.is_empty() {
            return Err(AppError::localized(
                "usage_script.empty_array",
                "脚本返回的数组不能为空",
                "Script returned empty array",
            ));
        }
        for (idx, item) in arr.iter().enumerate() {
            validate_single_usage(item).map_err(|e| {
                AppError::localized(
                    "usage_script.array_validation_failed",
                    format!("数组索引[{idx}]验证失败: {e}"),
                    format!("Validation failed at index [{idx}]: {e}"),
                )
            })?;
        }
        return Ok(());
    }

    // 如果是单对象，直接验证（向后兼容）
    validate_single_usage(result)
}

/// 验证单个用量数据对象
fn validate_single_usage(result: &Value) -> Result<(), AppError> {
    let obj = result.as_object().ok_or_else(|| {
        AppError::localized(
            "usage_script.must_return_object",
            "脚本必须返回对象或对象数组",
            "Script must return object or array of objects",
        )
    })?;

    // 所有字段均为可选，只进行类型检查
    if obj.contains_key("isValid")
        && !result["isValid"].is_null()
        && !result["isValid"].is_boolean()
    {
        return Err(AppError::localized(
            "usage_script.isvalid_type_error",
            "isValid 必须是布尔值或 null",
            "isValid must be boolean or null",
        ));
    }
    if obj.contains_key("invalidMessage")
        && !result["invalidMessage"].is_null()
        && !result["invalidMessage"].is_string()
    {
        return Err(AppError::localized(
            "usage_script.invalidmessage_type_error",
            "invalidMessage 必须是字符串或 null",
            "invalidMessage must be string or null",
        ));
    }
    if obj.contains_key("remaining")
        && !result["remaining"].is_null()
        && !result["remaining"].is_number()
    {
        return Err(AppError::localized(
            "usage_script.remaining_type_error",
            "remaining 必须是数字或 null",
            "remaining must be number or null",
        ));
    }
    if obj.contains_key("unit") && !result["unit"].is_null() && !result["unit"].is_string() {
        return Err(AppError::localized(
            "usage_script.unit_type_error",
            "unit 必须是字符串或 null",
            "unit must be string or null",
        ));
    }
    if obj.contains_key("total") && !result["total"].is_null() && !result["total"].is_number() {
        return Err(AppError::localized(
            "usage_script.total_type_error",
            "total 必须是数字或 null",
            "total must be number or null",
        ));
    }
    if obj.contains_key("used") && !result["used"].is_null() && !result["used"].is_number() {
        return Err(AppError::localized(
            "usage_script.used_type_error",
            "used 必须是数字或 null",
            "used must be number or null",
        ));
    }
    if obj.contains_key("planName")
        && !result["planName"].is_null()
        && !result["planName"].is_string()
    {
        return Err(AppError::localized(
            "usage_script.planname_type_error",
            "planName 必须是字符串或 null",
            "planName must be string or null",
        ));
    }
    if obj.contains_key("extra") && !result["extra"].is_null() && !result["extra"].is_string() {
        return Err(AppError::localized(
            "usage_script.extra_type_error",
            "extra 必须是字符串或 null",
            "extra must be string or null",
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{execute_usage_script, send_http_request, validate_request_url, RequestConfig};
    use axum::{http::StatusCode, routing::get, Router};
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn proxy_test_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[tokio::test]
    #[expect(
        clippy::await_holding_lock,
        reason = "test serializes global proxy client state"
    )]
    async fn send_http_request_http_error_preview_handles_multibyte_truncation() {
        let _guard = proxy_test_lock();
        crate::proxy::http_client::apply_proxy(None).expect("reset proxy to direct");

        let body = "你".repeat(70);
        let app = Router::new().route(
            "/",
            get({
                let body = body.clone();
                move || async move { (StatusCode::INTERNAL_SERVER_ERROR, body.clone()) }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let address = listener.local_addr().expect("listener local addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("server should run");
        });

        let request = RequestConfig {
            url: format!("http://{address}/"),
            method: "GET".to_string(),
            headers: HashMap::new(),
            body: None,
        };

        let err = send_http_request(&request, 2)
            .await
            .expect_err("http error should return AppError");
        assert!(err.to_string().contains("HTTP 500"));

        server.abort();
    }

    #[tokio::test]
    #[expect(
        clippy::await_holding_lock,
        reason = "test serializes global proxy client state"
    )]
    async fn send_http_request_uses_shared_proxy_aware_client() {
        let _guard = proxy_test_lock();

        let app = Router::new().route("/", get(|| async { "ok" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let address = listener.local_addr().expect("listener local addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("server should run");
        });

        crate::proxy::http_client::apply_proxy(Some("http://127.0.0.1:9"))
            .expect("apply unreachable proxy");

        let request = RequestConfig {
            url: format!("http://{address}/"),
            method: "GET".to_string(),
            headers: HashMap::new(),
            body: None,
        };

        let result = send_http_request(&request, 2).await;

        crate::proxy::http_client::apply_proxy(None).expect("reset proxy to direct");
        server.abort();

        let err = result.expect_err("proxy-aware client should fail through unreachable proxy");
        let message = err.to_string();
        assert!(
            message.contains("请求失败") || message.contains("Request failed"),
            "unexpected error message: {message}"
        );
    }

    #[tokio::test]
    #[expect(
        clippy::await_holding_lock,
        reason = "test serializes global proxy client state"
    )]
    async fn execute_usage_script_custom_template_skips_same_origin_check() {
        let _guard = proxy_test_lock();
        crate::proxy::http_client::apply_proxy(None).expect("reset proxy to direct");

        let app = Router::new().route("/", get(|| async { axum::Json(json!({ "total": 1 })) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let address = listener.local_addr().expect("listener local addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("server should run");
        });

        let script = format!(
            r#"({{
                request: {{
                    url: "http://{address}/",
                    method: "GET"
                }},
                extractor: function(response) {{
                    return {{ total: response.total }};
                }}
            }})"#
        );

        let non_custom =
            execute_usage_script(&script, "", "https://api.example.com", 2, None, None, None).await;
        let err = non_custom.expect_err("non-custom template should enforce same-origin");
        let message = err.to_string();
        assert!(
            message.contains("同源请求") || message.contains("same-origin"),
            "unexpected error message: {message}"
        );

        let custom = execute_usage_script(
            &script,
            "",
            "https://api.example.com",
            2,
            None,
            None,
            Some("custom"),
        )
        .await
        .expect("custom template should skip same-origin restriction");
        assert_eq!(
            custom.get("total").and_then(|value| value.as_i64()),
            Some(1)
        );

        server.abort();
    }

    #[test]
    fn validate_request_url_matches_upstream_custom_and_same_origin_rules() {
        validate_request_url("https://10.0.0.1/api", "", true)
            .expect("upstream allows custom HTTPS targets without private-IP filtering");
        validate_request_url("http://127.0.0.1/api", "", true)
            .expect("loopback target should remain allowed");

        let err = validate_request_url(
            "https://other.example/api",
            "https://api.example.com",
            false,
        )
        .expect_err("non-custom templates should enforce same-origin");
        let message = err.to_string();
        assert!(
            message.contains("同源请求") || message.contains("same-origin"),
            "unexpected error message: {message}"
        );
    }
}
