use serde_json::{json, Value};

use super::types::OptimizerConfig;

pub fn inject(body: &mut Value, config: &OptimizerConfig) {
    if !config.enabled || !config.cache_injection {
        return;
    }

    let existing = count_existing(body);

    let mut budget = 4usize.saturating_sub(existing);
    if budget == 0 {
        return;
    }

    if budget > 0 {
        if let Some(tools) = body.get_mut("tools").and_then(|value| value.as_array_mut()) {
            if let Some(last) = tools.last_mut() {
                if last.get("cache_control").is_none() {
                    if let Some(obj) = last.as_object_mut() {
                        obj.insert(
                            "cache_control".to_string(),
                            make_cache_control(&config.cache_ttl),
                        );
                        budget -= 1;
                    }
                }
            }
        }
    }

    if budget > 0 {
        if let Some(system_text) = body.get("system").and_then(|value| value.as_str()) {
            body["system"] = json!([{"type": "text", "text": system_text}]);
        }
        if let Some(system) = body
            .get_mut("system")
            .and_then(|value| value.as_array_mut())
        {
            if let Some(last) = system.last_mut() {
                if last.get("cache_control").is_none() {
                    if let Some(obj) = last.as_object_mut() {
                        obj.insert(
                            "cache_control".to_string(),
                            make_cache_control(&config.cache_ttl),
                        );
                        budget -= 1;
                    }
                }
            }
        }
    }

    if budget > 0 {
        if let Some(messages) = body
            .get_mut("messages")
            .and_then(|value| value.as_array_mut())
        {
            for message in messages.iter_mut().rev() {
                if inject_message_breakpoint(message, &config.cache_ttl) {
                    budget -= 1;
                    break;
                }
            }

            if budget > 0 && messages.len() >= 4 {
                let mut user_count = 0;
                for message in messages.iter_mut().rev() {
                    if message.get("role").and_then(Value::as_str) != Some("user") {
                        continue;
                    }
                    user_count += 1;
                    if user_count == 2 {
                        inject_message_breakpoint(message, &config.cache_ttl);
                        break;
                    }
                }
            }
        }
    }
}

fn inject_message_breakpoint(message: &mut Value, ttl: &str) -> bool {
    let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
        return false;
    };
    let Some(block) = content.iter_mut().rev().find(|block| {
        !matches!(
            block.get("type").and_then(Value::as_str),
            Some("thinking" | "redacted_thinking")
        )
    }) else {
        return false;
    };
    if block.get("cache_control").is_some() {
        return false;
    }
    let Some(object) = block.as_object_mut() else {
        return false;
    };
    object.insert("cache_control".to_string(), make_cache_control(ttl));
    true
}

fn make_cache_control(ttl: &str) -> Value {
    if ttl == "5m" {
        json!({"type": "ephemeral"})
    } else {
        json!({"type": "ephemeral", "ttl": ttl})
    }
}

fn count_existing(body: &Value) -> usize {
    let mut count = 0;

    if let Some(tools) = body.get("tools").and_then(|value| value.as_array()) {
        count += tools
            .iter()
            .filter(|tool| tool.get("cache_control").is_some())
            .count();
    }

    if let Some(system) = body.get("system").and_then(|value| value.as_array()) {
        count += system
            .iter()
            .filter(|block| block.get("cache_control").is_some())
            .count();
    }

    if let Some(messages) = body.get("messages").and_then(|value| value.as_array()) {
        for message in messages {
            if let Some(content) = message.get("content").and_then(|value| value.as_array()) {
                count += content
                    .iter()
                    .filter(|block| block.get("cache_control").is_some())
                    .count();
            }
        }
    }

    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_config() -> OptimizerConfig {
        OptimizerConfig {
            enabled: true,
            thinking_optimizer: true,
            cache_injection: true,
            cache_ttl: "1h".to_string(),
        }
    }

    #[test]
    fn injects_breakpoints_into_tools_system_and_last_assistant_block() {
        let mut body = json!({
            "tools": [{"name": "tool_a"}],
            "system": [{"type": "text", "text": "sys"}],
            "messages": [{
                "role": "assistant",
                "content": [{"type": "text", "text": "hello"}]
            }]
        });

        inject(&mut body, &enabled_config());

        assert!(body["tools"][0].get("cache_control").is_some());
        assert!(body["system"][0].get("cache_control").is_some());
        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_some());
    }

    #[test]
    fn ttl_5m_omits_ttl_field() {
        let config = OptimizerConfig {
            cache_ttl: "5m".to_string(),
            ..enabled_config()
        };
        let mut body = json!({"tools": [{"name": "tool_a"}]});

        inject(&mut body, &config);

        let cache_control = &body["tools"][0]["cache_control"];
        assert_eq!(cache_control["type"], "ephemeral");
        assert!(cache_control.get("ttl").is_none() || cache_control["ttl"].is_null());
    }

    #[test]
    fn injects_latest_tool_result_instead_of_older_assistant() {
        let mut body = json!({
            "messages": [
                {"role": "assistant", "content": [{"type": "tool_use", "id": "call_1", "name": "Read", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "done"}]}
            ]
        });

        inject(&mut body, &enabled_config());

        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_none());
        assert!(body["messages"][1]["content"][0]
            .get("cache_control")
            .is_some());
    }

    #[test]
    fn preserves_existing_caller_ttl() {
        let mut body = json!({
            "tools": [{
                "name": "tool_a",
                "cache_control": {"type": "ephemeral", "ttl": "5m"}
            }]
        });

        inject(&mut body, &enabled_config());

        assert_eq!(body["tools"][0]["cache_control"]["ttl"], "5m");
    }

    #[test]
    fn long_history_adds_an_older_user_anchor() {
        let mut body = json!({
            "tools": [{"name": "tool_a"}],
            "system": [{"type": "text", "text": "sys"}],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "first"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "answer"}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "result"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "latest"}]}
            ]
        });

        inject(&mut body, &enabled_config());

        assert_eq!(count_existing(&body), 4);
        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_some());
        assert!(body["messages"][3]["content"][0]
            .get("cache_control")
            .is_some());
    }
}
