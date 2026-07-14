use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::cli::i18n::texts;

pub(crate) const USER_AGENT_PRESETS: [&str; 5] = [
    "claude-cli/2.1.161 (external, cli)",
    "claude-cli/2.1.161",
    "claude-code/1.0.0",
    "claude-code/0.1.0",
    "Kilo-Code/1.0",
];

pub(crate) const USER_AGENT_PICKER_CUSTOM_INDEX: usize = 0;
pub(crate) const USER_AGENT_PICKER_PRESET_OFFSET: usize = 1;
pub(crate) const USER_AGENT_PICKER_NO_OVERRIDE_INDEX: usize =
    USER_AGENT_PICKER_PRESET_OFFSET + USER_AGENT_PRESETS.len();

pub(crate) const fn user_agent_picker_option_count() -> usize {
    USER_AGENT_PICKER_NO_OVERRIDE_INDEX + 1
}

pub(crate) fn user_agent_picker_selection(value: &str) -> usize {
    let value = value.trim();
    if value.is_empty() {
        return USER_AGENT_PICKER_NO_OVERRIDE_INDEX;
    }

    USER_AGENT_PRESETS
        .iter()
        .position(|preset| *preset == value)
        .map_or(USER_AGENT_PICKER_CUSTOM_INDEX, |index| {
            USER_AGENT_PICKER_PRESET_OFFSET + index
        })
}

const PROTECTED_LOCAL_PROXY_HEADER_NAMES: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "proxy-authorization",
    "proxy-authenticate",
    "te",
    "trailer",
    "upgrade",
    "accept-encoding",
    "content-type",
    "authorization",
    "x-api-key",
    "x-goog-api-key",
    "chatgpt-account-id",
    "session_id",
    "x-client-request-id",
    "x-codex-window-id",
    "x-forwarded-host",
    "x-forwarded-port",
    "x-forwarded-proto",
    "forwarded",
    "cf-connecting-ip",
    "cf-ipcountry",
    "cf-ray",
    "cf-visitor",
    "true-client-ip",
    "fastly-client-ip",
    "x-azure-clientip",
    "x-azure-fdid",
    "x-azure-ref",
    "akamai-origin-hop",
    "x-akamai-config-log-detail",
    "x-request-id",
    "x-correlation-id",
    "x-trace-id",
    "x-amzn-trace-id",
    "x-b3-traceid",
    "x-b3-spanid",
    "x-b3-parentspanid",
    "x-b3-sampled",
    "traceparent",
    "tracestate",
];

pub(crate) fn is_valid_custom_user_agent(value: &str) -> bool {
    value
        .trim()
        .bytes()
        .all(|byte| byte == b'\t' || (byte >= 0x20 && byte != 0x7f))
}

pub(crate) fn parse_local_proxy_header_overrides(
    raw: &str,
) -> Result<BTreeMap<String, String>, String> {
    let Some(object) = parse_override_object(raw)? else {
        return Ok(BTreeMap::new());
    };

    let mut entries = Vec::with_capacity(object.len());
    for (name, value) in object {
        let Value::String(value) = value else {
            return Err(texts::tui_override_header_non_string(&name));
        };
        entries.push((name, value));
    }

    normalize_local_proxy_header_overrides(entries)
}

pub(crate) fn normalize_local_proxy_header_overrides(
    entries: impl IntoIterator<Item = (String, String)>,
) -> Result<BTreeMap<String, String>, String> {
    let mut headers = BTreeMap::new();
    for (name, value) in entries {
        let trimmed_name = name.trim();
        if trimmed_name.is_empty() {
            return Err(texts::tui_override_header_empty_name().to_string());
        }
        if !is_valid_http_header_name(trimmed_name) {
            return Err(texts::tui_override_header_invalid_name(&name));
        }
        if !is_valid_http_header_value(&value) {
            return Err(texts::tui_override_header_control_chars(&name));
        }

        let normalized_name = trimmed_name.to_ascii_lowercase();
        if headers.contains_key(&normalized_name) {
            return Err(texts::tui_override_header_duplicate(&name));
        }
        if PROTECTED_LOCAL_PROXY_HEADER_NAMES.contains(&normalized_name.as_str()) {
            return Err(texts::tui_override_header_protected(&name));
        }
        headers.insert(normalized_name, value);
    }

    Ok(headers)
}

pub(crate) fn parse_local_proxy_body_override(raw: &str) -> Result<Option<Value>, String> {
    let Some(object) = parse_override_object(raw)? else {
        return Ok(None);
    };
    if object.contains_key("stream") {
        return Err(texts::tui_override_body_stream_protected().to_string());
    }
    if object.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Value::Object(object)))
    }
}

pub(crate) fn format_local_proxy_header_overrides(headers: &BTreeMap<String, String>) -> String {
    let object = headers
        .iter()
        .map(|(name, value)| (name.clone(), Value::String(value.clone())))
        .collect::<Map<_, _>>();
    serde_json::to_string_pretty(&Value::Object(object)).unwrap_or_else(|_| "{}".to_string())
}

pub(crate) fn format_local_proxy_body_override(body: Option<&Value>) -> String {
    serde_json::to_string_pretty(body.unwrap_or(&Value::Object(Map::new())))
        .unwrap_or_else(|_| "{}".to_string())
}

fn parse_override_object(raw: &str) -> Result<Option<Map<String, Value>>, String> {
    if raw.trim().is_empty() {
        return Ok(None);
    }

    let value: Value = serde_json::from_str(raw)
        .map_err(|error| texts::tui_toast_invalid_json(&error.to_string()))?;
    value
        .as_object()
        .cloned()
        .map(Some)
        .ok_or_else(|| texts::tui_override_json_not_object().to_string())
}

fn is_valid_http_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn is_valid_http_header_value(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte == b'\t' || (byte >= 0x20 && byte != 0x7f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_validation_matches_http_header_value_control_character_rules() {
        assert!(is_valid_custom_user_agent(""));
        assert!(is_valid_custom_user_agent(" claude-cli/2.1.161 "));
        assert!(is_valid_custom_user_agent("agent\tvariant"));
        assert!(is_valid_custom_user_agent("客户端/1.0"));
        assert!(!is_valid_custom_user_agent("agent\nvariant"));
        assert!(!is_valid_custom_user_agent("agent\u{7f}"));
    }

    #[test]
    fn user_agent_picker_selection_tracks_custom_presets_and_no_override() {
        assert_eq!(
            user_agent_picker_selection("custom-agent/1.0"),
            USER_AGENT_PICKER_CUSTOM_INDEX
        );
        for (index, preset) in USER_AGENT_PRESETS.iter().enumerate() {
            assert_eq!(
                user_agent_picker_selection(preset),
                USER_AGENT_PICKER_PRESET_OFFSET + index
            );
        }
        assert_eq!(
            user_agent_picker_selection("  "),
            USER_AGENT_PICKER_NO_OVERRIDE_INDEX
        );
        assert_eq!(
            user_agent_picker_option_count(),
            USER_AGENT_PRESETS.len() + 2
        );
    }

    #[test]
    fn header_overrides_are_trimmed_lowercased_and_sorted() {
        let parsed = parse_local_proxy_header_overrides(
            r#"{" X-Zeta ":"two","X-Alpha":"one","User-Agent":"custom"}"#,
        )
        .expect("valid overrides");

        assert_eq!(
            parsed.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["user-agent", "x-alpha", "x-zeta"]
        );
        assert_eq!(parsed["x-alpha"], "one");
    }

    #[test]
    fn header_override_normalizer_matches_json_parser() {
        let normalized = normalize_local_proxy_header_overrides([
            (" X-Zeta ".to_string(), "two".to_string()),
            ("X-Alpha".to_string(), "one".to_string()),
        ])
        .expect("valid imported overrides");

        assert_eq!(
            normalized.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["x-alpha", "x-zeta"]
        );
        assert!(normalize_local_proxy_header_overrides([
            ("X-Test".to_string(), "one".to_string()),
            ("x-test".to_string(), "two".to_string()),
        ])
        .is_err());
    }

    #[test]
    fn header_overrides_reject_invalid_values_duplicates_and_protected_headers() {
        assert!(parse_local_proxy_header_overrides(r#"{"bad name":"value"}"#).is_err());
        assert!(parse_local_proxy_header_overrides(r#"{"x-test":1}"#).is_err());
        assert!(parse_local_proxy_header_overrides(r#"{"X-Test":"one","x-test":"two"}"#).is_err());
        assert!(parse_local_proxy_header_overrides(r#"{"Authorization":"secret"}"#).is_err());
        assert!(parse_local_proxy_header_overrides(r#"{"x-test":"line\nbreak"}"#).is_err());
    }

    #[test]
    fn body_override_requires_an_object_and_rejects_top_level_stream() {
        assert!(parse_local_proxy_body_override("").unwrap().is_none());
        assert!(parse_local_proxy_body_override("{}").unwrap().is_none());
        assert!(parse_local_proxy_body_override("[]").is_err());
        assert!(parse_local_proxy_body_override(r#"{"stream":true}"#).is_err());

        let body = parse_local_proxy_body_override(
            r#"{"store":false,"nested":{"stream":true},"items":[1,2]}"#,
        )
        .expect("valid body")
        .expect("non-empty body");
        assert_eq!(body["store"], false);
        assert_eq!(body["nested"]["stream"], true);
    }

    #[test]
    fn formatting_empty_overrides_produces_an_editable_object() {
        assert_eq!(format_local_proxy_header_overrides(&BTreeMap::new()), "{}");
        assert_eq!(format_local_proxy_body_override(None), "{}");
    }
}
