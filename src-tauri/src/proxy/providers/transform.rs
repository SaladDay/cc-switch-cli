use crate::proxy::{error::ProxyError, json_canonical::canonical_json_string};
use serde_json::{json, Value};
use std::borrow::Cow;

const ANTHROPIC_BILLING_HEADER_PREFIX: &str = "x-anthropic-billing-header:";

pub fn is_openai_o_series(model: &str) -> bool {
    model.len() > 1
        && model.starts_with('o')
        && model.as_bytes().get(1).is_some_and(|b| b.is_ascii_digit())
}

pub fn supports_reasoning_effort(model: &str) -> bool {
    is_openai_o_series(model)
        || model
            .to_lowercase()
            .strip_prefix("gpt-")
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| c.is_ascii_digit() && c >= '5')
}

pub fn resolve_reasoning_effort(body: &Value) -> Option<&'static str> {
    if let Some(effort) = body
        .pointer("/output_config/effort")
        .and_then(|value| value.as_str())
    {
        return match effort {
            "low" => Some("low"),
            "medium" => Some("medium"),
            "high" => Some("high"),
            "max" => Some("xhigh"),
            _ => None,
        };
    }

    let thinking = body.get("thinking")?;
    match thinking.get("type").and_then(|value| value.as_str()) {
        Some("adaptive") => Some("xhigh"),
        Some("enabled") => {
            let budget = thinking
                .get("budget_tokens")
                .and_then(|value| value.as_u64());
            match budget {
                Some(budget) if budget < 4_000 => Some("low"),
                Some(budget) if budget < 16_000 => Some("medium"),
                Some(_) | None => Some("high"),
            }
        }
        _ => None,
    }
}

pub(crate) fn strip_leading_anthropic_billing_header(text: &str) -> &str {
    if !text.starts_with(ANTHROPIC_BILLING_HEADER_PREFIX) {
        return text;
    }

    let Some(line_end) = text
        .as_bytes()
        .iter()
        .position(|byte| *byte == b'\n' || *byte == b'\r')
    else {
        return "";
    };

    let bytes = text.as_bytes();
    let mut rest_start = line_end + 1;
    if bytes[line_end] == b'\r' && bytes.get(line_end + 1) == Some(&b'\n') {
        rest_start += 1;
    }

    let rest = &text[rest_start..];
    if let Some(stripped) = rest.strip_prefix("\r\n") {
        stripped
    } else if let Some(stripped) = rest.strip_prefix('\n') {
        stripped
    } else if let Some(stripped) = rest.strip_prefix('\r') {
        stripped
    } else {
        rest
    }
}

pub fn sanitize_system_text(text: &str) -> Option<Cow<'_, str>> {
    let text = strip_leading_anthropic_billing_header(text);

    if text.is_empty() {
        return None;
    }

    Some(Cow::Borrowed(text))
}

pub fn anthropic_to_openai(body: Value, cache_key: Option<&str>) -> Result<Value, ProxyError> {
    anthropic_to_openai_with_reasoning_content(body, cache_key, false)
}

pub fn anthropic_to_openai_with_reasoning_content(
    body: Value,
    cache_key: Option<&str>,
    preserve_reasoning_content: bool,
) -> Result<Value, ProxyError> {
    let mut result = json!({});

    if let Some(model) = body.get("model").and_then(|m| m.as_str()) {
        result["model"] = json!(model);
    }

    let mut messages = Vec::new();

    if let Some(system) = body.get("system") {
        if let Some(text) = system.as_str() {
            if let Some(text) = sanitize_system_text(text) {
                messages.push(json!({"role": "system", "content": text}));
            }
        } else if let Some(arr) = system.as_array() {
            for msg in arr {
                if let Some(text) = msg.get("text").and_then(|t| t.as_str()) {
                    let Some(text) = sanitize_system_text(text) else {
                        continue;
                    };
                    let system_message = json!({"role": "system", "content": text});
                    messages.push(system_message);
                }
            }
        }
    }

    if let Some(msgs) = body.get("messages").and_then(|m| m.as_array()) {
        for msg in msgs {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = msg.get("content");
            messages.extend(convert_message_to_openai(
                role,
                content,
                preserve_reasoning_content,
            )?);
        }
    }

    normalize_openai_system_messages(&mut messages);
    result["messages"] = json!(messages);

    let model = body.get("model").and_then(|m| m.as_str()).unwrap_or("");
    if let Some(v) = body.get("max_tokens") {
        if is_openai_o_series(model) {
            result["max_completion_tokens"] = v.clone();
        } else {
            result["max_tokens"] = v.clone();
        }
    }
    if let Some(v) = body.get("temperature") {
        result["temperature"] = v.clone();
    }
    if let Some(v) = body.get("top_p") {
        result["top_p"] = v.clone();
    }
    if let Some(v) = body.get("stop_sequences") {
        result["stop"] = v.clone();
    }
    if let Some(v) = body.get("stream") {
        result["stream"] = v.clone();
    }

    if supports_reasoning_effort(model) {
        if let Some(effort) = resolve_reasoning_effort(&body) {
            result["reasoning_effort"] = json!(effort);
        }
    }

    if let Some(tools) = body.get("tools").and_then(|t| t.as_array()) {
        let openai_tools: Vec<Value> = tools
            .iter()
            .filter(|t| t.get("type").and_then(|v| v.as_str()) != Some("BatchTool"))
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                        "description": t.get("description"),
                        "parameters": clean_schema(t.get("input_schema").cloned().unwrap_or(json!({})))
                    }
                })
            })
            .collect();

        if !openai_tools.is_empty() {
            result["tools"] = json!(openai_tools);
        }
    }

    if let Some(v) = body.get("tool_choice") {
        result["tool_choice"] = map_tool_choice_to_chat(v);
    }

    if let Some(key) = cache_key {
        result["prompt_cache_key"] = json!(key);
    }

    Ok(result)
}

/// Inject `stream_options.include_usage` into an OpenAI Chat Completions request.
///
/// OpenAI-compatible upstreams do not emit usage in the SSE stream unless the
/// request explicitly sets `stream_options.include_usage = true`; without it the
/// converted Anthropic `message_delta` reports all-zero tokens (input/output/cache),
/// so streamed requests silently lose their token/cost/cache accounting (see #323).
/// Existing `stream_options` fields the client passed through are preserved; only
/// `include_usage` is added, and non-streaming requests are left untouched.
pub(crate) fn inject_openai_stream_include_usage(result: &mut Value) {
    let is_stream = result
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !is_stream {
        return;
    }
    match result.get_mut("stream_options") {
        Some(Value::Object(opts)) => {
            opts.insert("include_usage".to_string(), json!(true));
        }
        _ => {
            result["stream_options"] = json!({ "include_usage": true });
        }
    }
}

/// Translate Anthropic tool_choice into OpenAI Chat Completions format.
fn map_tool_choice_to_chat(tool_choice: &Value) -> Value {
    match tool_choice {
        Value::String(value) => match value.as_str() {
            "any" => json!("required"),
            _ => json!(value),
        },
        Value::Object(obj) => match obj.get("type").and_then(|value| value.as_str()) {
            Some("any") => json!("required"),
            Some("auto") => json!("auto"),
            Some("none") => json!("none"),
            Some("tool") => {
                let name = obj
                    .get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                json!({
                    "type": "function",
                    "function": { "name": name }
                })
            }
            _ => tool_choice.clone(),
        },
        _ => tool_choice.clone(),
    }
}

fn normalize_openai_system_messages(messages: &mut Vec<Value>) {
    let system_count = messages
        .iter()
        .filter(|message| message.get("role").and_then(|value| value.as_str()) == Some("system"))
        .count();

    if system_count == 0 {
        return;
    }

    if system_count == 1 {
        if let Some(index) = messages.iter().position(|message| {
            message.get("role").and_then(|value| value.as_str()) == Some("system")
        }) {
            if index > 0 {
                let message = messages.remove(index);
                messages.insert(0, message);
            }
        }
        return;
    }

    let mut parts = Vec::new();
    messages.retain(|message| {
        if message.get("role").and_then(|value| value.as_str()) != Some("system") {
            return true;
        }

        match message.get("content") {
            Some(Value::String(text)) if !text.is_empty() => parts.push(text.clone()),
            Some(Value::Array(content_parts)) => {
                let text = content_parts
                    .iter()
                    .filter_map(|part| part.get("text").and_then(|value| value.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    parts.push(text);
                }
            }
            _ => {}
        }

        false
    });

    if !parts.is_empty() {
        let merged = json!({"role": "system", "content": parts.join("\n")});
        messages.insert(0, merged);
    }
}

fn convert_message_to_openai(
    role: &str,
    content: Option<&Value>,
    preserve_reasoning_content: bool,
) -> Result<Vec<Value>, ProxyError> {
    let mut result = Vec::new();

    let content = match content {
        Some(c) => c,
        None => {
            result.push(json!({"role": role, "content": null}));
            return Ok(result);
        }
    };

    if let Some(text) = content.as_str() {
        result.push(json!({"role": role, "content": text}));
        return Ok(result);
    }

    if let Some(blocks) = content.as_array() {
        let mut content_parts = Vec::new();
        let mut tool_calls = Vec::new();
        let mut reasoning_parts = Vec::new();

        for block in blocks {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        content_parts.push(json!({"type": "text", "text": text}));
                    }
                }
                "image" => {
                    if let Some(source) = block.get("source") {
                        let media_type = source
                            .get("media_type")
                            .and_then(|m| m.as_str())
                            .unwrap_or("image/png");
                        let data = source.get("data").and_then(|d| d.as_str()).unwrap_or("");
                        content_parts.push(json!({
                            "type": "image_url",
                            "image_url": {"url": format!("data:{};base64,{}", media_type, data)}
                        }));
                    }
                }
                "tool_use" => {
                    let id = block.get("id").and_then(|i| i.as_str()).unwrap_or("");
                    let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let input = block.get("input").cloned().unwrap_or(json!({}));
                    tool_calls.push(json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": canonical_json_string(&input)
                        }
                    }));
                }
                "tool_result" => {
                    let tool_use_id = block
                        .get("tool_use_id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("");
                    let content_val = block.get("content");
                    let content_str = match content_val {
                        Some(Value::String(s)) => s.clone(),
                        Some(v) => canonical_json_string(v),
                        None => String::new(),
                    };
                    result.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": content_str
                    }));
                }
                "thinking" => {
                    if let Some(thinking) = block.get("thinking").and_then(|t| t.as_str()) {
                        if !thinking.is_empty() {
                            reasoning_parts.push(thinking.to_string());
                        }
                    }
                }
                "redacted_thinking" if preserve_reasoning_content => {
                    reasoning_parts.push("[redacted thinking]".to_string());
                }
                _ => {}
            }
        }

        if !content_parts.is_empty() || !tool_calls.is_empty() {
            let mut msg = json!({"role": role});
            if content_parts.is_empty() {
                msg["content"] = Value::Null;
            } else if content_parts.len() == 1 {
                if let Some(text) = content_parts[0].get("text") {
                    msg["content"] = text.clone();
                } else {
                    msg["content"] = json!(content_parts);
                }
            } else {
                msg["content"] = json!(content_parts);
            }

            if !tool_calls.is_empty() {
                msg["tool_calls"] = json!(tool_calls);
            }

            if preserve_reasoning_content && role == "assistant" && !tool_calls.is_empty() {
                let reasoning_content = if reasoning_parts.is_empty() {
                    "tool call".to_string()
                } else {
                    reasoning_parts.join("\n")
                };
                msg["reasoning_content"] = json!(reasoning_content);
            }

            result.push(msg);
        }

        return Ok(result);
    }

    result.push(json!({"role": role, "content": content}));
    Ok(result)
}

pub(crate) fn clean_schema(mut schema: Value) -> Value {
    if let Some(obj) = schema.as_object_mut() {
        if obj.get("format").and_then(|v| v.as_str()) == Some("uri") {
            obj.remove("format");
        }

        if let Some(properties) = obj.get_mut("properties").and_then(|v| v.as_object_mut()) {
            for (_, value) in properties.iter_mut() {
                *value = clean_schema(value.clone());
            }
        }

        if let Some(items) = obj.get_mut("items") {
            *items = clean_schema(items.clone());
        }
    }
    schema
}

/// Map an OpenAI-compatible `usage` object onto Anthropic usage fields.
///
/// OpenAI `prompt_tokens` is inclusive of cache hits, but Anthropic `input_tokens`
/// is fresh input only. This path is billed as app_type="claude" (the cost calculator
/// does NOT subtract cache again), so `cache_read` + `cache_creation` must be subtracted
/// here — otherwise cached tokens would be counted both as input and in the cache buckets.
/// The three buckets are mutually exclusive: input + cache_read + cache_creation ==
/// prompt_tokens. Symmetric with the streaming path's `build_stream_usage_json`.
fn openai_usage_to_anthropic(body: &Value) -> Value {
    let usage = body.get("usage").cloned().unwrap_or(json!({}));
    // Direct `cache_read_input_tokens` field takes priority over nested cached_tokens.
    let cached = usage
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            usage
                .pointer("/prompt_tokens_details/cached_tokens")
                .and_then(|v| v.as_u64())
        })
        .unwrap_or(0);
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            usage
                .pointer("/prompt_tokens_details/cache_write_tokens")
                .or_else(|| usage.pointer("/input_tokens_details/cache_write_tokens"))
                .and_then(|v| v.as_u64())
        })
        .unwrap_or(0);
    let input_tokens = usage
        .get("prompt_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        .saturating_sub(cached)
        .saturating_sub(cache_creation) as u32;
    let output_tokens = usage
        .get("completion_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let mut usage_json = json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens
    });

    if cached > 0 {
        usage_json["cache_read_input_tokens"] = json!(cached);
    }
    if cache_creation > 0 {
        usage_json["cache_creation_input_tokens"] = json!(cache_creation);
    }

    usage_json
}

pub fn openai_to_anthropic(body: Value) -> Result<Value, ProxyError> {
    let choices = body
        .get("choices")
        .and_then(|c| c.as_array())
        .ok_or_else(|| ProxyError::TransformError("No choices in response".to_string()))?;

    // Some upstreams (e.g. NVIDIA NIM's z-ai/glm-5.2) intermittently return a valid
    // JSON body whose `choices` is an empty array alongside a usage-only payload:
    //   {"id":"...","choices":[],"usage":{...}}
    // Treat this as an empty assistant turn instead of failing the whole request with
    // a 502, mirroring how the streaming path skips usage-only tail chunks. See #325
    // and the desktop fix for farion1231/cc-switch#3951.
    //
    // Guard on the absence of a top-level `error`: an error body that also carries an
    // empty `choices` array must not be masked as a successful empty turn. It falls
    // through and is surfaced as a transform failure instead.
    if choices.is_empty() && body.get("error").is_none() {
        return Ok(json!({
            "id": body.get("id").and_then(|i| i.as_str()).unwrap_or(""),
            "type": "message",
            "role": "assistant",
            "content": [],
            "model": body.get("model").and_then(|m| m.as_str()).unwrap_or(""),
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": openai_usage_to_anthropic(&body)
        }));
    }

    let choice = choices
        .first()
        .ok_or_else(|| ProxyError::TransformError("Empty choices array".to_string()))?;
    let message = choice
        .get("message")
        .ok_or_else(|| ProxyError::TransformError("No message in choice".to_string()))?;

    let mut content = Vec::new();
    let mut has_tool_use = false;

    if let Some(reasoning_content) = message.get("reasoning_content").and_then(|r| r.as_str()) {
        if !reasoning_content.is_empty() {
            content.push(json!({"type": "thinking", "thinking": reasoning_content}));
        }
    }

    if let Some(msg_content) = message.get("content") {
        if let Some(text) = msg_content.as_str() {
            if !text.is_empty() {
                content.push(json!({"type": "text", "text": text}));
            }
        } else if let Some(parts) = msg_content.as_array() {
            for part in parts {
                let part_type = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match part_type {
                    "text" | "output_text" => {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                content.push(json!({"type": "text", "text": text}));
                            }
                        }
                    }
                    "refusal" => {
                        if let Some(refusal) = part.get("refusal").and_then(|r| r.as_str()) {
                            if !refusal.is_empty() {
                                content.push(json!({"type": "text", "text": refusal}));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if let Some(refusal) = message.get("refusal").and_then(|r| r.as_str()) {
        if !refusal.is_empty() {
            content.push(json!({"type": "text", "text": refusal}));
        }
    }

    if let Some(tool_calls) = message.get("tool_calls").and_then(|t| t.as_array()) {
        if !tool_calls.is_empty() {
            has_tool_use = true;
        }
        for tc in tool_calls {
            let id = tc.get("id").and_then(|i| i.as_str()).unwrap_or("");
            let empty_obj = json!({});
            let func = tc.get("function").unwrap_or(&empty_obj);
            let name = func.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args_str = func
                .get("arguments")
                .and_then(|a| a.as_str())
                .unwrap_or("{}");
            let input: Value = serde_json::from_str(args_str).unwrap_or(json!({}));
            content.push(json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input
            }));
        }
    }

    if !has_tool_use {
        if let Some(function_call) = message.get("function_call") {
            let id = function_call
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("");
            let name = function_call
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            let has_arguments = function_call.get("arguments").is_some();

            let input = match function_call.get("arguments") {
                Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(json!({})),
                Some(v @ Value::Object(_)) | Some(v @ Value::Array(_)) => v.clone(),
                _ => json!({}),
            };

            if !name.is_empty() || has_arguments {
                content.push(json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input
                }));
                has_tool_use = true;
            }
        }
    }

    let stop_reason = choice
        .get("finish_reason")
        .and_then(|r| r.as_str())
        .map(|r| match r {
            "stop" => "end_turn",
            "length" => "max_tokens",
            "tool_calls" | "function_call" => "tool_use",
            "content_filter" => "end_turn",
            other => {
                log::warn!(
                    "[Claude/OpenAI] Unknown finish_reason in non-streaming response: {other}"
                );
                "end_turn"
            }
        })
        .or(if has_tool_use { Some("tool_use") } else { None });

    let usage_json = openai_usage_to_anthropic(&body);

    Ok(json!({
        "id": body.get("id").and_then(|i| i.as_str()).unwrap_or(""),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": body.get("model").and_then(|m| m.as_str()).unwrap_or(""),
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": usage_json
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_include_usage_preserves_existing_stream_options() {
        let mut body = json!({
            "stream": true,
            "stream_options": { "continuous_usage_stats": true }
        });
        inject_openai_stream_include_usage(&mut body);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["stream_options"]["continuous_usage_stats"], true);
    }

    #[test]
    fn inject_include_usage_noop_for_non_stream() {
        let mut body = json!({ "stream": false });
        inject_openai_stream_include_usage(&mut body);
        assert!(body.get("stream_options").is_none());

        let mut body = json!({ "messages": [] });
        inject_openai_stream_include_usage(&mut body);
        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn anthropic_to_openai_removes_billing_header_from_system_string() {
        let input = json!({
            "model": "gpt-5",
            "system": "x-anthropic-billing-header: cc_version=2.1.120.cf9; cc_entrypoint=cli; cch=543cf;\nYou are helpful.",
            "messages": [{"role": "user", "content": "Hello"}]
        });

        let result = anthropic_to_openai(input, None).unwrap();

        assert_eq!(result["messages"][0]["content"], "You are helpful.");
    }

    #[test]
    fn anthropic_to_openai_removes_billing_header_from_system_array() {
        let input = json!({
            "model": "gpt-5",
            "system": [{
                "type": "text",
                "text": "x-anthropic-billing-header: cc_version=2.1.120.cf9; cc_entrypoint=cli; cch=543cf;\nProject instructions",
                "cache_control": {"type": "ephemeral"}
            }],
            "messages": [{"role": "user", "content": "Hello"}]
        });

        let result = anthropic_to_openai(input, None).unwrap();

        assert_eq!(result["messages"][0]["content"], "Project instructions");
        assert!(result["messages"][0].get("cache_control").is_none());
    }

    #[test]
    fn anthropic_to_openai_omits_empty_billing_header_system_block() {
        let input = json!({
            "model": "gpt-5",
            "system": [{
                "type": "text",
                "text": "x-anthropic-billing-header: cc_version=2.1.120.cf9; cc_entrypoint=cli; cch=543cf;"
            }],
            "messages": [{"role": "user", "content": "Hello"}]
        });

        let result = anthropic_to_openai(input, None).unwrap();

        assert_eq!(result["messages"][0]["role"], "user");
    }

    #[test]
    fn sanitize_system_text_keeps_non_leading_billing_header_text() {
        let text = "First line\n  x-anthropic-billing-header: cc_version=2.1.120.cf9; cc_entrypoint=cli; cch=543cf;\n\nLast line\n";

        let result = sanitize_system_text(text).unwrap();

        assert_eq!(result, text);
    }

    #[test]
    fn sanitize_system_text_strips_leading_billing_header_with_crlf() {
        let text = "x-anthropic-billing-header: cc_version=2.1.120.cf9; cc_entrypoint=cli; cch=543cf;\r\n\r\nYou are helpful.";

        let result = sanitize_system_text(text).unwrap();

        assert_eq!(result, "You are helpful.");
    }

    #[test]
    fn anthropic_to_openai_injects_prompt_cache_key() {
        let input = json!({
            "model": "claude-3-opus",
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": "Hello"}]
        });

        let result = anthropic_to_openai(input, Some("provider-123")).unwrap();

        assert_eq!(result["prompt_cache_key"], "provider-123");
    }

    #[test]
    fn anthropic_to_openai_preserves_system_cache_control() {
        let input = json!({
            "model": "claude-3-sonnet",
            "max_tokens": 1024,
            "system": [{
                "type": "text",
                "text": "System prompt",
                "cache_control": {"type": "ephemeral"}
            }],
            "messages": [{"role": "user", "content": "Hello"}]
        });

        let result = anthropic_to_openai(input, None).unwrap();

        assert_eq!(result["messages"][0]["role"], "system");
        assert!(result["messages"][0].get("cache_control").is_none());
    }

    #[test]
    fn anthropic_to_openai_preserves_matching_system_cache_control_when_merging() {
        let input = json!({
            "model": "claude-3-sonnet",
            "max_tokens": 1024,
            "system": [
                {"type": "text", "text": "You are Claude Code.", "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": "Be concise.", "cache_control": {"type": "ephemeral"}}
            ],
            "messages": [{"role": "user", "content": "Hello"}]
        });

        let result = anthropic_to_openai(input, None).unwrap();

        assert_eq!(result["messages"].as_array().unwrap().len(), 2);
        assert_eq!(result["messages"][0]["role"], "system");
        assert_eq!(
            result["messages"][0]["content"],
            "You are Claude Code.\nBe concise."
        );
        assert!(result["messages"][0].get("cache_control").is_none());
        assert_eq!(result["messages"][1]["role"], "user");
    }

    #[test]
    fn anthropic_to_openai_drops_mixed_present_absent_system_cache_control_when_merging() {
        let input = json!({
            "model": "claude-3-sonnet",
            "max_tokens": 1024,
            "system": [
                {"type": "text", "text": "You are Claude Code.", "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": "Be concise."}
            ],
            "messages": [{"role": "user", "content": "Hello"}]
        });

        let result = anthropic_to_openai(input, None).unwrap();

        assert_eq!(result["messages"][0]["role"], "system");
        assert_eq!(
            result["messages"][0]["content"],
            "You are Claude Code.\nBe concise."
        );
        assert!(result["messages"][0].get("cache_control").is_none());
    }

    #[test]
    fn anthropic_to_openai_drops_conflicting_system_cache_control_when_merging() {
        let input = json!({
            "model": "claude-3-sonnet",
            "max_tokens": 1024,
            "system": [
                {"type": "text", "text": "You are Claude Code.", "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": "Be concise.", "cache_control": {"type": "ephemeral", "ttl": "5m"}}
            ],
            "messages": [{"role": "user", "content": "Hello"}]
        });

        let result = anthropic_to_openai(input, None).unwrap();

        assert_eq!(result["messages"][0]["role"], "system");
        assert_eq!(
            result["messages"][0]["content"],
            "You are Claude Code.\nBe concise."
        );
        assert!(result["messages"][0].get("cache_control").is_none());
    }

    #[test]
    fn anthropic_to_openai_strips_text_block_cache_control_and_simplifies_single_block() {
        let input = json!({
            "model": "claude-3-opus",
            "max_tokens": 1024,
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "Hello",
                    "cache_control": {"type": "ephemeral", "ttl": "5m"}
                }]
            }]
        });

        let result = anthropic_to_openai(input, None).unwrap();

        assert_eq!(result["messages"][0]["content"], "Hello");
    }

    #[test]
    fn anthropic_to_openai_preserves_tool_cache_control() {
        let input = json!({
            "model": "claude-3-opus",
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": "Hello"}],
            "tools": [{
                "name": "get_weather",
                "description": "Get weather",
                "input_schema": {"type": "object"},
                "cache_control": {"type": "ephemeral"}
            }]
        });

        let result = anthropic_to_openai(input, None).unwrap();

        assert!(result["tools"][0].get("cache_control").is_none());
    }

    fn run_tool_choice(value: Value) -> Value {
        let input = json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hello"}],
            "tools": [{
                "name": "search",
                "description": "Search",
                "input_schema": {"type": "object", "properties": {}}
            }],
            "tool_choice": value
        });

        anthropic_to_openai(input, None).unwrap()["tool_choice"].clone()
    }

    #[test]
    fn tool_choice_string_any_maps_to_required() {
        assert_eq!(run_tool_choice(json!("any")), json!("required"));
    }

    #[test]
    fn tool_choice_string_auto_and_none_pass_through() {
        assert_eq!(run_tool_choice(json!("auto")), json!("auto"));
        assert_eq!(run_tool_choice(json!("none")), json!("none"));
    }

    #[test]
    fn tool_choice_object_any_maps_to_required() {
        assert_eq!(run_tool_choice(json!({"type": "any"})), json!("required"));
    }

    #[test]
    fn tool_choice_object_auto_and_none_collapse_to_string() {
        assert_eq!(run_tool_choice(json!({"type": "auto"})), json!("auto"));
        assert_eq!(run_tool_choice(json!({"type": "none"})), json!("none"));
    }

    #[test]
    fn tool_choice_forced_tool_maps_to_nested_function_selector() {
        assert_eq!(
            run_tool_choice(json!({"type": "tool", "name": "search"})),
            json!({"type": "function", "function": {"name": "search"}})
        );
    }

    #[test]
    fn anthropic_to_openai_maps_reasoning_effort_for_gpt5() {
        let input = json!({
            "model": "gpt-5.4",
            "max_tokens": 1024,
            "thinking": {"type": "adaptive"},
            "messages": [{"role": "user", "content": "Hello"}]
        });

        let result = anthropic_to_openai(input, None).unwrap();

        assert_eq!(result["reasoning_effort"], "xhigh");
        assert_eq!(result["max_tokens"], 1024);
    }

    #[test]
    fn anthropic_to_openai_uses_max_completion_tokens_for_o_series() {
        let input = json!({
            "model": "o3-mini",
            "max_tokens": 2048,
            "messages": [{"role": "user", "content": "Hello"}]
        });

        let result = anthropic_to_openai(input, None).unwrap();

        assert_eq!(result["max_completion_tokens"], 2048);
        assert!(result.get("max_tokens").is_none());
    }

    #[test]
    fn anthropic_to_openai_does_not_emit_reasoning_content_by_default() {
        let input = json!({
            "model": "gpt-4o",
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "I should call the tool."},
                    {"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {"city": "Tokyo"}}
                ]
            }]
        });

        let result = anthropic_to_openai(input, None).unwrap();

        assert!(result["messages"][0].get("reasoning_content").is_none());
    }

    #[test]
    fn anthropic_to_openai_tool_use_preserves_reasoning_content_when_enabled() {
        let input = json!({
            "model": "deepseek-v4-pro",
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "I should call the tool."},
                    {"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {"city": "Tokyo"}}
                ]
            }]
        });

        let result = anthropic_to_openai_with_reasoning_content(input, None, true).unwrap();

        assert_eq!(
            result["messages"][0]["reasoning_content"],
            "I should call the tool."
        );
        assert_eq!(result["messages"][0]["tool_calls"][0]["id"], "call_1");
    }

    #[test]
    fn anthropic_to_openai_tool_use_injects_placeholder_reasoning_content_when_missing() {
        let input = json!({
            "model": "deepseek-v4-pro",
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {"city": "Tokyo"}}
                ]
            }]
        });

        let result = anthropic_to_openai_with_reasoning_content(input, None, true).unwrap();

        assert_eq!(result["messages"][0]["reasoning_content"], "tool call");
    }

    #[test]
    fn anthropic_to_openai_tool_use_uses_redacted_thinking_placeholder() {
        let input = json!({
            "model": "mimo-v2.5-pro",
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "redacted_thinking", "data": "opaque"},
                    {"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {"city": "Tokyo"}}
                ]
            }]
        });

        let result = anthropic_to_openai_with_reasoning_content(input, None, true).unwrap();

        assert_eq!(
            result["messages"][0]["reasoning_content"],
            "[redacted thinking]"
        );
        assert_eq!(result["messages"][0]["tool_calls"][0]["id"], "call_1");
    }

    #[test]
    fn anthropic_to_openai_tool_use_arguments_are_canonical_json() {
        let input = json!({
            "model": "claude-3-opus",
            "messages": [{
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "call_1",
                        "name": "lookup",
                        "input": {
                            "z": 1,
                            "a": {
                                "b": 2,
                                "a": 1
                            }
                        }
                    }
                ]
            }]
        });

        let result = anthropic_to_openai(input, None).unwrap();

        assert_eq!(
            result["messages"][0]["tool_calls"][0]["function"]["arguments"],
            r#"{"a":{"a":1,"b":2},"z":1}"#
        );
    }

    #[test]
    fn anthropic_to_openai_tool_result_content_is_canonical_json() {
        let input = json!({
            "model": "claude-3-opus",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "call_1",
                    "content": {
                        "z": 1,
                        "a": {
                            "b": 2,
                            "a": 1
                        }
                    }
                }]
            }]
        });

        let result = anthropic_to_openai(input, None).unwrap();

        assert_eq!(result["messages"][0]["role"], "tool");
        assert_eq!(result["messages"][0]["tool_call_id"], "call_1");
        assert_eq!(
            result["messages"][0]["content"],
            r#"{"a":{"a":1,"b":2},"z":1}"#
        );
    }

    #[test]
    fn openai_to_anthropic_maps_reasoning_content_to_thinking_block() {
        let input = json!({
            "id": "chatcmpl-deepseek",
            "model": "deepseek-v4-flash",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "reasoning_content": "Need the current date before calling weather.",
                    "content": "Let me check.",
                    "tool_calls": [{
                        "id": "call_date",
                        "type": "function",
                        "function": {"name": "get_date", "arguments": "{}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });

        let result = openai_to_anthropic(input).unwrap();

        assert_eq!(result["content"][0]["type"], "thinking");
        assert_eq!(
            result["content"][0]["thinking"],
            "Need the current date before calling weather."
        );
        assert_eq!(result["content"][1]["type"], "text");
        assert_eq!(result["content"][2]["type"], "tool_use");
    }

    #[test]
    fn openai_to_anthropic_treats_empty_choices_as_empty_turn() {
        // NVIDIA NIM's z-ai/glm-5.2 intermittently returns a usage-only body with an
        // empty `choices` array. It must map to a valid empty message, not a 502. (#325)
        let input = json!({
            "id": "chatcmpl-nvidia",
            "model": "z-ai/glm-5.2",
            "choices": [],
            "usage": {
                "prompt_tokens": 13312,
                "completion_tokens": 79,
                "prompt_tokens_details": {
                    "cached_tokens": 100,
                    "cache_write_tokens": 200
                }
            }
        });

        let result = openai_to_anthropic(input).unwrap();

        assert_eq!(result["type"], "message");
        assert_eq!(result["role"], "assistant");
        assert_eq!(result["id"], "chatcmpl-nvidia");
        assert_eq!(result["model"], "z-ai/glm-5.2");
        assert_eq!(result["stop_reason"], "end_turn");
        assert_eq!(result["content"], json!([]));
        // Fresh input = prompt_tokens - cache read - cache write.
        assert_eq!(result["usage"]["input_tokens"], 13012);
        assert_eq!(result["usage"]["output_tokens"], 79);
        assert_eq!(result["usage"]["cache_read_input_tokens"], 100);
        assert_eq!(result["usage"]["cache_creation_input_tokens"], 200);
    }

    #[test]
    fn openai_to_anthropic_still_errors_when_choices_field_missing() {
        // A body with no `choices` field at all is genuinely malformed and should
        // still be rejected, unlike an explicitly empty array.
        let input = json!({"id": "chatcmpl-bad", "usage": {}});
        let err = openai_to_anthropic(input).unwrap_err();
        assert!(matches!(err, ProxyError::TransformError(_)));
    }

    #[test]
    fn openai_to_anthropic_does_not_mask_error_body_with_empty_choices() {
        // An error payload that also carries an empty `choices` array must not be
        // silently converted into a successful empty turn.
        let input = json!({
            "choices": [],
            "error": {"message": "upstream exploded", "type": "server_error"}
        });
        let err = openai_to_anthropic(input).unwrap_err();
        assert!(matches!(err, ProxyError::TransformError(_)));
    }

    #[test]
    fn deepseek_reasoning_content_round_trips_for_tool_calls() {
        let upstream_response = json!({
            "id": "chatcmpl-deepseek",
            "model": "deepseek-v4-flash",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "reasoning_content": "Need the current date before calling weather.",
                    "content": "Let me check.",
                    "tool_calls": [{
                        "id": "call_date",
                        "type": "function",
                        "function": {"name": "get_date", "arguments": "{}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });

        let anthropic_response = openai_to_anthropic(upstream_response).unwrap();
        let follow_up_request = json!({
            "model": "deepseek-v4-flash",
            "messages": [{
                "role": "assistant",
                "content": anthropic_response["content"].clone()
            }]
        });
        let replayed =
            anthropic_to_openai_with_reasoning_content(follow_up_request, None, true).unwrap();
        let msg = &replayed["messages"][0];

        assert_eq!(
            msg["reasoning_content"],
            "Need the current date before calling weather."
        );
        assert_eq!(msg["tool_calls"][0]["id"], "call_date");
        assert_eq!(msg["tool_calls"][0]["function"]["name"], "get_date");
    }
}
