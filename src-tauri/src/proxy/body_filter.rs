use serde_json::Value;
use std::collections::HashSet;

#[cfg(test)]
pub fn filter_private_params(body: Value) -> Value {
    filter_private_params_with_whitelist(body, &[])
}

pub fn filter_private_params_with_whitelist(body: Value, whitelist: &[String]) -> Value {
    let whitelist_set: HashSet<&str> = whitelist.iter().map(|item| item.as_str()).collect();
    filter_recursive_with_whitelist(body, &mut Vec::new(), &mut Vec::new(), &whitelist_set)
}

fn filter_recursive_with_whitelist(
    value: Value,
    path: &mut Vec<String>,
    removed_keys: &mut Vec<String>,
    whitelist: &HashSet<&str>,
) -> Value {
    match value {
        Value::Object(map) => {
            let is_schema_name_map = path.last().is_some_and(|key| matches_schema_name_map(key));
            let filtered = map
                .into_iter()
                .filter_map(|(key, value)| {
                    if key.starts_with('_')
                        && !whitelist.contains(key.as_str())
                        && !is_schema_name_map
                    {
                        removed_keys.push(key);
                        None
                    } else {
                        path.push(key.clone());
                        let filtered_value =
                            filter_recursive_with_whitelist(value, path, removed_keys, whitelist);
                        path.pop();
                        Some((key, filtered_value))
                    }
                })
                .collect();

            if !removed_keys.is_empty() {
                log::debug!("[BodyFilter] filtered private params: {removed_keys:?}");
                removed_keys.clear();
            }

            Value::Object(filtered)
        }
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| filter_recursive_with_whitelist(value, path, removed_keys, whitelist))
                .collect(),
        ),
        other => other,
    }
}

fn matches_schema_name_map(key: &str) -> bool {
    matches!(
        key,
        "properties" | "patternProperties" | "definitions" | "$defs"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn filters_private_params_recursively() {
        let input = json!({
            "model": "claude-3-5-sonnet",
            "_top_secret": true,
            "messages": [{
                "role": "user",
                "_message_secret": true,
                "content": [{
                    "type": "text",
                    "text": "hello",
                    "_content_secret": true
                }]
            }],
            "metadata": {
                "keep": "ok",
                "_trace_id": "drop"
            }
        });

        let output = filter_private_params(input);

        assert!(output.get("_top_secret").is_none());
        assert!(output.pointer("/messages/0/_message_secret").is_none());
        assert!(output
            .pointer("/messages/0/content/0/_content_secret")
            .is_none());
        assert!(output.pointer("/metadata/_trace_id").is_none());
        assert_eq!(
            output
                .pointer("/metadata/keep")
                .and_then(|value| value.as_str()),
            Some("ok")
        );
    }

    #[test]
    fn preserves_json_schema_private_looking_property_names() {
        let input = json!({
            "tools": [{
                "name": "lookup",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "_id": {
                            "type": "string",
                            "_private_note": "drop"
                        },
                        "_meta": {"type": "object"}
                    },
                    "$defs": {
                        "_ref": {
                            "type": "string",
                            "_private_note": "drop"
                        }
                    },
                    "_internal_schema_note": "drop"
                }
            }]
        });

        let output = filter_private_params(input);
        let schema = &output["tools"][0]["input_schema"];

        assert!(schema["properties"].get("_id").is_some());
        assert!(schema["properties"].get("_meta").is_some());
        assert!(schema["$defs"].get("_ref").is_some());
        assert!(schema["properties"]["_id"].get("_private_note").is_none());
        assert!(schema["$defs"]["_ref"].get("_private_note").is_none());
        assert!(schema.get("_internal_schema_note").is_none());
    }
}
