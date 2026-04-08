use crate::proxy::error::ProxyError;
use serde_json::{json, Value};

pub fn anthropic_to_openai(
    body: Value,
    cache_key: Option<&str>,
    stream_include_usage: bool,
) -> Result<Value, ProxyError> {
    let mut result = json!({});

    if let Some(model) = body.get("model").and_then(|m| m.as_str()) {
        result["model"] = json!(model);
    }

    let mut messages = Vec::new();

    if let Some(system) = body.get("system") {
        if let Some(text) = system.as_str() {
            messages.push(json!({"role": "system", "content": text}));
        } else if let Some(arr) = system.as_array() {
            for msg in arr {
                if let Some(text) = msg.get("text").and_then(|t| t.as_str()) {
                    let mut system_message = json!({"role": "system", "content": text});
                    if let Some(cache_control) = msg.get("cache_control") {
                        system_message["cache_control"] = cache_control.clone();
                    }
                    messages.push(system_message);
                }
            }
        }
    }

    if let Some(msgs) = body.get("messages").and_then(|m| m.as_array()) {
        for msg in msgs {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = msg.get("content");
            messages.extend(convert_message_to_openai(role, content)?);
        }
    }

    result["messages"] = json!(messages);

    if let Some(v) = body.get("max_tokens") {
        result["max_tokens"] = v.clone();
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
        if stream_include_usage && v.as_bool() == Some(true) {
            result["stream_options"] = json!({"include_usage": true});
        }
    }

    if let Some(tools) = body.get("tools").and_then(|t| t.as_array()) {
        let openai_tools: Vec<Value> = tools
            .iter()
            .filter(|t| t.get("type").and_then(|v| v.as_str()) != Some("BatchTool"))
            .map(|t| {
                let mut tool = json!({
                    "type": "function",
                    "function": {
                        "name": t.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                        "description": t.get("description"),
                        "parameters": clean_schema(t.get("input_schema").cloned().unwrap_or(json!({})))
                    }
                });
                if let Some(cache_control) = t.get("cache_control") {
                    tool["cache_control"] = cache_control.clone();
                }
                tool
            })
            .collect();

        if !openai_tools.is_empty() {
            result["tools"] = json!(openai_tools);
        }
    }

    if let Some(v) = body.get("tool_choice") {
        result["tool_choice"] = v.clone();
    }

    if let Some(key) = cache_key {
        result["prompt_cache_key"] = json!(key);
    }

    Ok(result)
}

fn convert_message_to_openai(
    role: &str,
    content: Option<&Value>,
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

        for block in blocks {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        let mut part = json!({"type": "text", "text": text});
                        if let Some(cache_control) = block.get("cache_control") {
                            part["cache_control"] = cache_control.clone();
                        }
                        content_parts.push(part);
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
                            "arguments": serde_json::to_string(&input).unwrap_or_default()
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
                        Some(v) => serde_json::to_string(v).unwrap_or_default(),
                        None => String::new(),
                    };
                    result.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": content_str
                    }));
                }
                "thinking" => {}
                _ => {}
            }
        }

        if !content_parts.is_empty() || !tool_calls.is_empty() {
            let mut msg = json!({"role": role});
            if content_parts.is_empty() {
                msg["content"] = Value::Null;
            } else if content_parts.len() == 1 {
                if content_parts[0].get("cache_control").is_some() {
                    msg["content"] = json!(content_parts);
                } else if let Some(text) = content_parts[0].get("text") {
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

pub fn openai_to_anthropic(body: Value) -> Result<Value, ProxyError> {
    let choices = body
        .get("choices")
        .and_then(|c| c.as_array())
        .ok_or_else(|| ProxyError::TransformError("No choices in response".to_string()))?;
    let choice = choices
        .first()
        .ok_or_else(|| ProxyError::TransformError("Empty choices array".to_string()))?;
    let message = choice
        .get("message")
        .ok_or_else(|| ProxyError::TransformError("No message in choice".to_string()))?;

    let mut content = Vec::new();
    let mut has_tool_use = false;

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

    let usage = body.get("usage").cloned().unwrap_or(json!({}));
    let input_tokens = usage
        .get("prompt_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let output_tokens = usage
        .get("completion_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let mut usage_json = json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens
    });

    if let Some(cached) = usage
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(|v| v.as_u64())
    {
        usage_json["cache_read_input_tokens"] = json!(cached);
    }
    if let Some(v) = usage.get("cache_read_input_tokens") {
        usage_json["cache_read_input_tokens"] = v.clone();
    }
    if let Some(v) = usage.get("cache_creation_input_tokens") {
        usage_json["cache_creation_input_tokens"] = v.clone();
    }

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
    fn anthropic_to_openai_injects_prompt_cache_key() {
        let input = json!({
            "model": "claude-3-opus",
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": "Hello"}]
        });

        let result = anthropic_to_openai(input, Some("provider-123"), false).unwrap();

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

        let result = anthropic_to_openai(input, None, false).unwrap();

        assert_eq!(result["messages"][0]["role"], "system");
        assert_eq!(result["messages"][0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn anthropic_to_openai_preserves_text_block_cache_control_and_array_shape() {
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

        let result = anthropic_to_openai(input, None, false).unwrap();

        assert!(result["messages"][0]["content"].is_array());
        assert_eq!(
            result["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert_eq!(
            result["messages"][0]["content"][0]["cache_control"]["ttl"],
            "5m"
        );
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

        let result = anthropic_to_openai(input, None, false).unwrap();

        assert_eq!(result["tools"][0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn anthropic_to_openai_injects_stream_options_when_enabled() {
        let input = json!({
            "model": "gpt-4",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "Hello"}],
            "stream": true
        });

        let result = anthropic_to_openai(input, None, true).unwrap();

        assert_eq!(result["stream"], true);
        assert_eq!(result["stream_options"]["include_usage"], true);
    }

    #[test]
    fn anthropic_to_openai_does_not_inject_stream_options_when_disabled() {
        let input = json!({
            "model": "gpt-4",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "Hello"}],
            "stream": true
        });

        let result = anthropic_to_openai(input, None, false).unwrap();

        assert_eq!(result["stream"], true);
        assert!(result.get("stream_options").is_none());
    }

    #[test]
    fn anthropic_to_openai_does_not_inject_stream_options_when_stream_is_false() {
        let input = json!({
            "model": "gpt-4",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "Hello"}],
            "stream": false
        });

        let result = anthropic_to_openai(input, None, true).unwrap();

        assert_eq!(result["stream"], false);
        assert!(result.get("stream_options").is_none());
    }
}
