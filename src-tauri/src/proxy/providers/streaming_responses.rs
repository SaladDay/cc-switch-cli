use bytes::Bytes;
use futures::stream::{Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

use crate::proxy::response::StreamCompletion;
use crate::proxy::sse::{append_utf8_safe, strip_sse_field, take_sse_block};

use super::reasoning_bridge::{encode_openai_reasoning_item, reasoning_summary_text};
use super::transform_responses::{
    build_anthropic_usage_from_responses, map_responses_stop_reason,
    sanitize_anthropic_tool_use_input_json,
};

#[inline]
fn response_object_from_event(data: &Value) -> &Value {
    data.get("response").unwrap_or(data)
}

fn anthropic_sse(event_name: &str, payload: &Value) -> Bytes {
    Bytes::from(format!(
        "event: {event_name}\ndata: {}\n\n",
        serde_json::to_string(payload).unwrap_or_default()
    ))
}

fn message_start_sse(message_id: &Option<String>, model: &Option<String>, usage: Value) -> Bytes {
    anthropic_sse(
        "message_start",
        &json!({
            "type": "message_start",
            "message": {
                "id": message_id.clone().unwrap_or_default(),
                "type": "message",
                "role": "assistant",
                "content": [],
                "stop_reason": Value::Null,
                "stop_sequence": Value::Null,
                "model": model.clone().unwrap_or_default(),
                "usage": usage
            }
        }),
    )
}

fn responses_error_details(data: &Value, fallback: &str) -> (String, String) {
    let response = response_object_from_event(data);
    let error = response.get("error").unwrap_or(response);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .filter(|message| !message.trim().is_empty())
        .unwrap_or(fallback)
        .to_string();
    let error_type = error
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| error.get("code").and_then(Value::as_str))
        .unwrap_or("upstream_error")
        .to_string();
    (message, error_type)
}

fn error_sse(message: &str, error_type: &str) -> Bytes {
    anthropic_sse(
        "error",
        &json!({
            "type": "error",
            "error": { "type": error_type, "message": message }
        }),
    )
}

#[inline]
fn content_part_key(data: &Value) -> Option<String> {
    if let (Some(item_id), Some(content_index)) = (
        data.get("item_id").and_then(|v| v.as_str()),
        data.get("content_index").and_then(|v| v.as_u64()),
    ) {
        return Some(format!("part:{item_id}:{content_index}"));
    }
    if let (Some(output_index), Some(content_index)) = (
        data.get("output_index").and_then(|v| v.as_u64()),
        data.get("content_index").and_then(|v| v.as_u64()),
    ) {
        return Some(format!("part:out:{output_index}:{content_index}"));
    }
    None
}

#[inline]
fn tool_item_key_from_added(data: &Value, item: &Value) -> Option<String> {
    if let Some(item_id) = item.get("id").and_then(|v| v.as_str()) {
        return Some(format!("tool:{item_id}"));
    }
    if let Some(item_id) = data.get("item_id").and_then(|v| v.as_str()) {
        return Some(format!("tool:{item_id}"));
    }
    if let Some(output_index) = data.get("output_index").and_then(|v| v.as_u64()) {
        return Some(format!("tool:out:{output_index}"));
    }
    None
}

#[inline]
fn tool_item_key_from_event(data: &Value) -> Option<String> {
    if let Some(item_id) = data.get("item_id").and_then(|v| v.as_str()) {
        return Some(format!("tool:{item_id}"));
    }
    if let Some(output_index) = data.get("output_index").and_then(|v| v.as_u64()) {
        return Some(format!("tool:out:{output_index}"));
    }
    None
}

fn buffered_tool_arguments(chunks: &[String]) -> String {
    let joined = chunks.concat();
    if joined.is_empty() || matches!(serde_json::from_str::<Value>(&joined), Ok(Value::Object(_))) {
        return joined;
    }

    chunks
        .iter()
        .rev()
        .find(|chunk| matches!(serde_json::from_str::<Value>(chunk), Ok(Value::Object(_))))
        .cloned()
        .unwrap_or(joined)
}

#[inline]
fn reasoning_item_key(data: &Value, item: Option<&Value>) -> Option<String> {
    if let Some(item_id) = item
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .or_else(|| data.get("item_id").and_then(Value::as_str))
    {
        return Some(format!("reasoning:{item_id}"));
    }
    data.get("output_index")
        .and_then(Value::as_u64)
        .map(|index| format!("reasoning:out:{index}"))
}

#[inline]
fn resolve_content_index(
    data: &Value,
    next_content_index: &mut u32,
    index_by_key: &mut HashMap<String, u32>,
    fallback_open_index: &mut Option<u32>,
) -> u32 {
    if let Some(k) = content_part_key(data) {
        if let Some(existing) = index_by_key.get(&k).copied() {
            existing
        } else {
            let assigned = *next_content_index;
            *next_content_index += 1;
            index_by_key.insert(k, assigned);
            assigned
        }
    } else if let Some(existing) = *fallback_open_index {
        existing
    } else {
        let assigned = *next_content_index;
        *next_content_index += 1;
        *fallback_open_index = Some(assigned);
        assigned
    }
}

pub fn create_anthropic_sse_stream_from_responses(
    stream: impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
    stream_completion: StreamCompletion,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    async_stream::stream! {
        let mut buffer = String::new();
        let mut utf8_remainder: Vec<u8> = Vec::new();
        let mut message_id: Option<String> = None;
        let mut current_model: Option<String> = None;
        let mut has_sent_message_start = false;
        let mut has_tool_use = false;
        let mut next_content_index: u32 = 0;
        let mut index_by_key: HashMap<String, u32> = HashMap::new();
        let mut open_indices: HashSet<u32> = HashSet::new();
        let mut fallback_open_index: Option<u32> = None;
        let mut current_text_index: Option<u32> = None;
        let mut tool_index_by_item_id: HashMap<String, u32> = HashMap::new();
        let mut tool_name_by_index: HashMap<u32, String> = HashMap::new();
        let mut tool_args_by_index: HashMap<u32, Vec<String>> = HashMap::new();
        let mut last_tool_index: Option<u32> = None;
        let mut reasoning_index_by_item_id: HashMap<String, u32> = HashMap::new();
        let mut reasoning_item_by_index: HashMap<u32, Value> = HashMap::new();
        let mut reasoning_text_by_index: HashMap<u32, String> = HashMap::new();
        let mut legacy_reasoning_index: Option<u32> = None;

        let stream = stream
            .map(|result| (result, false))
            .chain(futures::stream::once(async {
                (Ok::<Bytes, std::io::Error>(Bytes::new()), true)
            }));
        tokio::pin!(stream);

        while let Some((chunk, is_eof)) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    append_utf8_safe(&mut buffer, &mut utf8_remainder, &bytes);
                    if is_eof && !buffer.trim().is_empty() {
                        buffer.push_str("\n\n");
                    }

                    while let Some(block) = take_sse_block(&mut buffer) {
                        if block.trim().is_empty() {
                            continue;
                        }

                        let mut event_type: Option<String> = None;
                        let mut data_parts: Vec<String> = Vec::new();

                        for line in block.lines() {
                            if let Some(evt) = strip_sse_field(line, "event") {
                                event_type = Some(evt.trim().to_string());
                            } else if let Some(d) = strip_sse_field(line, "data") {
                                data_parts.push(d.to_string());
                            }
                        }

                        if data_parts.is_empty() {
                            continue;
                        }

                        let data_str = data_parts.join("\n");
                        let data: Value = match serde_json::from_str(&data_str) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let event_name = event_type
                            .as_deref()
                            .filter(|name| !name.is_empty())
                            .or_else(|| data.get("type").and_then(Value::as_str))
                            .unwrap_or("");

                        if matches!(
                            event_name,
                            "response.output_text.delta"
                                | "response.refusal.delta"
                                | "response.function_call_arguments.delta"
                                | "response.reasoning_summary_text.delta"
                                | "response.reasoning_text.delta"
                                | "response.reasoning.delta"
                        ) && !has_sent_message_start
                        {
                            yield Ok(message_start_sse(
                                &message_id,
                                &current_model,
                                json!({ "input_tokens": 0, "output_tokens": 0 }),
                            ));
                            has_sent_message_start = true;
                        }

                        match event_name {
                            "response.created" => {
                                let response_obj = response_object_from_event(&data);
                                if let Some(id) = response_obj.get("id").and_then(|i| i.as_str()) {
                                    message_id = Some(id.to_string());
                                }
                                if let Some(model) = response_obj.get("model").and_then(|m| m.as_str()) {
                                    current_model = Some(model.to_string());
                                }

                                has_sent_message_start = true;
                                yield Ok(message_start_sse(
                                    &message_id,
                                    &current_model,
                                    build_anthropic_usage_from_responses(response_obj.get("usage")),
                                ));
                            }
                            "response.content_part.added" => {
                                if !has_sent_message_start {
                                    yield Ok(message_start_sse(
                                        &message_id,
                                        &current_model,
                                        json!({ "input_tokens": 0, "output_tokens": 0 }),
                                    ));
                                    has_sent_message_start = true;
                                }

                                if let Some(part) = data.get("part") {
                                    let part_type = part.get("type").and_then(|t| t.as_str());
                                    if matches!(part_type, Some("output_text") | Some("refusal")) {
                                        let index = if let Some(index) = current_text_index {
                                            index
                                        } else {
                                            let index = resolve_content_index(
                                                &data,
                                                &mut next_content_index,
                                                &mut index_by_key,
                                                &mut fallback_open_index,
                                            );
                                            current_text_index = Some(index);
                                            index
                                        };
                                        if open_indices.contains(&index) {
                                            continue;
                                        }

                                        let event = json!({
                                            "type": "content_block_start",
                                            "index": index,
                                            "content_block": { "type": "text", "text": "" }
                                        });
                                        let sse = format!("event: content_block_start\ndata: {}\n\n", serde_json::to_string(&event).unwrap_or_default());
                                        yield Ok(Bytes::from(sse));
                                        open_indices.insert(index);
                                    }
                                }
                            }
                            "response.output_text.delta" | "response.refusal.delta" => {
                                if let Some(delta) = data.get("delta").and_then(|d| d.as_str()) {
                                    let index = if let Some(index) = current_text_index {
                                        index
                                    } else {
                                        let index = resolve_content_index(
                                            &data,
                                            &mut next_content_index,
                                            &mut index_by_key,
                                            &mut fallback_open_index,
                                        );
                                        current_text_index = Some(index);
                                        index
                                    };

                                    if !open_indices.contains(&index) {
                                        let start_event = json!({
                                            "type": "content_block_start",
                                            "index": index,
                                            "content_block": { "type": "text", "text": "" }
                                        });
                                        let start_sse = format!("event: content_block_start\ndata: {}\n\n", serde_json::to_string(&start_event).unwrap_or_default());
                                        yield Ok(Bytes::from(start_sse));
                                        open_indices.insert(index);
                                    }

                                    let event = json!({
                                        "type": "content_block_delta",
                                        "index": index,
                                        "delta": { "type": "text_delta", "text": delta }
                                    });
                                    let sse = format!("event: content_block_delta\ndata: {}\n\n", serde_json::to_string(&event).unwrap_or_default());
                                    yield Ok(Bytes::from(sse));
                                }
                            }
                            "response.content_part.done" | "response.refusal.done" => {
                                let index = current_text_index.take().or_else(|| {
                                    let key = content_part_key(&data);
                                    if let Some(k) = key {
                                        index_by_key.get(&k).copied()
                                    } else {
                                        fallback_open_index
                                    }
                                });
                                if let Some(index) = index {
                                    if !open_indices.remove(&index) {
                                        continue;
                                    }
                                    let event = json!({ "type": "content_block_stop", "index": index });
                                    let sse = format!("event: content_block_stop\ndata: {}\n\n", serde_json::to_string(&event).unwrap_or_default());
                                    yield Ok(Bytes::from(sse));
                                    if fallback_open_index == Some(index) {
                                        fallback_open_index = None;
                                    }
                                }
                            }
                            "response.output_item.added" => {
                                if let Some(item) = data.get("item") {
                                    if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                                        has_tool_use = true;
                                        if let Some(index) = current_text_index.take() {
                                            if open_indices.remove(&index) {
                                                let stop_event = json!({ "type": "content_block_stop", "index": index });
                                                let stop_sse = format!("event: content_block_stop\ndata: {}\n\n", serde_json::to_string(&stop_event).unwrap_or_default());
                                                yield Ok(Bytes::from(stop_sse));
                                            }
                                            if fallback_open_index == Some(index) {
                                                fallback_open_index = None;
                                            }
                                        }
                                        if !has_sent_message_start {
                                            yield Ok(message_start_sse(
                                                &message_id,
                                                &current_model,
                                                json!({ "input_tokens": 0, "output_tokens": 0 }),
                                            ));
                                            has_sent_message_start = true;
                                        }

                                        let call_id = item.get("call_id").and_then(|i| i.as_str()).unwrap_or("");
                                        let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
                                        let index = if let Some(k) = tool_item_key_from_added(&data, item) {
                                            if let Some(existing) = index_by_key.get(&k).copied() {
                                                existing
                                            } else {
                                                let assigned = next_content_index;
                                                next_content_index += 1;
                                                index_by_key.insert(k, assigned);
                                                assigned
                                            }
                                        } else {
                                            let assigned = next_content_index;
                                            next_content_index += 1;
                                            assigned
                                        };
                                        if let Some(item_id) = item
                                            .get("id")
                                            .and_then(|v| v.as_str())
                                            .or_else(|| data.get("item_id").and_then(|v| v.as_str()))
                                        {
                                            tool_index_by_item_id.insert(item_id.to_string(), index);
                                        }
                                        tool_name_by_index.insert(index, name.to_string());
                                        last_tool_index = Some(index);

                                        if open_indices.contains(&index) {
                                            continue;
                                        }

                                        tool_args_by_index.insert(index, Vec::new());

                                        let event = json!({
                                            "type": "content_block_start",
                                            "index": index,
                                            "content_block": {
                                                "type": "tool_use",
                                                "id": call_id,
                                                "name": name
                                            }
                                        });
                                        let sse = format!("event: content_block_start\ndata: {}\n\n", serde_json::to_string(&event).unwrap_or_default());
                                        yield Ok(Bytes::from(sse));
                                        open_indices.insert(index);
                                    } else if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                                        if !has_sent_message_start {
                                            yield Ok(message_start_sse(&message_id, &current_model, json!({"input_tokens": 0, "output_tokens": 0})));
                                            has_sent_message_start = true;
                                        }

                                        let index = if let Some(key) = reasoning_item_key(&data, Some(item)) {
                                            if let Some(existing) = index_by_key.get(&key).copied() {
                                                existing
                                            } else {
                                                let assigned = next_content_index;
                                                next_content_index += 1;
                                                index_by_key.insert(key, assigned);
                                                assigned
                                            }
                                        } else {
                                            let assigned = next_content_index;
                                            next_content_index += 1;
                                            assigned
                                        };
                                        if let Some(item_id) = item
                                            .get("id")
                                            .and_then(Value::as_str)
                                            .or_else(|| data.get("item_id").and_then(Value::as_str))
                                        {
                                            reasoning_index_by_item_id.insert(item_id.to_string(), index);
                                        }
                                        reasoning_item_by_index.insert(index, item.clone());
                                        reasoning_text_by_index.entry(index).or_default();
                                    }
                                }
                            }
                            "response.function_call_arguments.delta" => {
                                if let Some(delta) = data.get("delta").and_then(|d| d.as_str()) {
                                    let item_id = data.get("item_id").and_then(|v| v.as_str());
                                    let index = if let Some(id) = item_id {
                                        tool_index_by_item_id.get(id).copied()
                                    } else {
                                        None
                                    }
                                    .or_else(|| {
                                        tool_item_key_from_event(&data)
                                            .and_then(|k| index_by_key.get(&k).copied())
                                    })
                                    .or(last_tool_index)
                                    .unwrap_or_else(|| {
                                        let assigned = next_content_index;
                                        next_content_index += 1;
                                        assigned
                                    });

                                    if !open_indices.contains(&index) {
                                        let start_event = json!({
                                            "type": "content_block_start",
                                            "index": index,
                                            "content_block": {
                                                "type": "tool_use",
                                                "id": data
                                                    .get("call_id")
                                                    .and_then(|v| v.as_str())
                                                    .or(item_id)
                                                    .unwrap_or(""),
                                                "name": data
                                                    .get("name")
                                                    .and_then(|v| v.as_str())
                                                    .unwrap_or("")
                                            }
                                        });
                                        let start_sse = format!("event: content_block_start\ndata: {}\n\n", serde_json::to_string(&start_event).unwrap_or_default());
                                        yield Ok(Bytes::from(start_sse));
                                        open_indices.insert(index);
                                    }

                                    tool_args_by_index
                                        .entry(index)
                                        .or_default()
                                        .push(delta.to_string());
                                    if tool_name_by_index.get(&index).map(String::as_str) == Some("Read") {
                                        continue;
                                    }

                                    let event = json!({
                                        "type": "content_block_delta",
                                        "index": index,
                                        "delta": {
                                            "type": "input_json_delta",
                                            "partial_json": delta
                                        }
                                    });
                                    let sse = format!("event: content_block_delta\ndata: {}\n\n", serde_json::to_string(&event).unwrap_or_default());
                                    yield Ok(Bytes::from(sse));
                                }
                            }
                            "response.function_call_arguments.done" => {
                                let item_id = data.get("item_id").and_then(|v| v.as_str());
                                let index = if let Some(id) = item_id {
                                    tool_index_by_item_id.get(id).copied()
                                } else {
                                    None
                                }
                                .or_else(|| {
                                    tool_item_key_from_event(&data)
                                        .and_then(|k| index_by_key.get(&k).copied())
                                })
                                .or(last_tool_index);
                                if let Some(index) = index {
                                    if !open_indices.remove(&index) {
                                        continue;
                                    }
                                    if tool_name_by_index.get(&index).map(String::as_str) == Some("Read") {
                                        let raw = data
                                            .get("arguments")
                                            .and_then(|value| value.as_str())
                                            .map(str::to_string)
                                            .unwrap_or_else(|| {
                                                tool_args_by_index
                                                    .get(&index)
                                                    .map(|chunks| buffered_tool_arguments(chunks))
                                                    .unwrap_or_default()
                                            });
                                        let sanitized = sanitize_anthropic_tool_use_input_json("Read", &raw);
                                        if !sanitized.is_empty() {
                                            let event = json!({
                                                "type": "content_block_delta",
                                                "index": index,
                                                "delta": {
                                                    "type": "input_json_delta",
                                                    "partial_json": sanitized
                                                }
                                            });
                                            let sse = format!("event: content_block_delta\ndata: {}\n\n", serde_json::to_string(&event).unwrap_or_default());
                                            yield Ok(Bytes::from(sse));
                                        }
                                    } else {
                                        let has_accumulated = tool_args_by_index
                                            .get(&index)
                                            .is_some_and(|chunks| chunks.iter().any(|chunk| !chunk.is_empty()));
                                        if !has_accumulated {
                                            if let Some(arguments) = data
                                                .get("arguments")
                                                .and_then(|value| value.as_str())
                                                .filter(|arguments| !arguments.is_empty())
                                            {
                                                let event = json!({
                                                    "type": "content_block_delta",
                                                    "index": index,
                                                    "delta": {
                                                        "type": "input_json_delta",
                                                        "partial_json": arguments
                                                    }
                                                });
                                                let sse = format!("event: content_block_delta\ndata: {}\n\n", serde_json::to_string(&event).unwrap_or_default());
                                                yield Ok(Bytes::from(sse));
                                            }
                                        }
                                    }
                                    let event = json!({ "type": "content_block_stop", "index": index });
                                    let sse = format!("event: content_block_stop\ndata: {}\n\n", serde_json::to_string(&event).unwrap_or_default());
                                    yield Ok(Bytes::from(sse));
                                    if let Some(item_id) = item_id {
                                        tool_index_by_item_id.remove(item_id);
                                    }
                                    tool_name_by_index.remove(&index);
                                    tool_args_by_index.remove(&index);
                                }
                            }
                            "response.reasoning_summary_text.delta"
                            | "response.reasoning_text.delta"
                            | "response.reasoning.delta" => {
                                if let Some(delta) = data
                                    .get("delta")
                                    .or_else(|| data.get("text"))
                                    .and_then(|d| d.as_str())
                                {
                                    if let Some(index) = current_text_index.take() {
                                        if open_indices.remove(&index) {
                                            let stop_event = json!({
                                                "type": "content_block_stop",
                                                "index": index
                                            });
                                            yield Ok(anthropic_sse("content_block_stop", &stop_event));
                                        }
                                        if fallback_open_index == Some(index) {
                                            fallback_open_index = None;
                                        }
                                    }
                                    let item_id = data.get("item_id").and_then(Value::as_str);
                                    let item_key = reasoning_item_key(&data, None);
                                    let is_keyless = item_id.is_none() && item_key.is_none();
                                    let index = item_id
                                        .and_then(|id| reasoning_index_by_item_id.get(id).copied())
                                        .or_else(|| {
                                            item_key
                                                .as_ref()
                                                .and_then(|key| index_by_key.get(key).copied())
                                        })
                                        .or_else(|| {
                                            is_keyless
                                                .then_some(legacy_reasoning_index)
                                                .flatten()
                                        })
                                        .unwrap_or_else(|| {
                                            let assigned = next_content_index;
                                            next_content_index += 1;
                                            if let Some(key) = item_key {
                                                index_by_key.insert(key, assigned);
                                            }
                                            if let Some(id) = item_id {
                                                reasoning_index_by_item_id
                                                    .insert(id.to_string(), assigned);
                                            } else if is_keyless {
                                                legacy_reasoning_index = Some(assigned);
                                            }
                                            assigned
                                        });

                                    if !open_indices.contains(&index) {
                                        let start_event = json!({
                                            "type": "content_block_start",
                                            "index": index,
                                            "content_block": {
                                                "type": "thinking",
                                                "thinking": ""
                                            }
                                        });
                                        yield Ok(anthropic_sse("content_block_start", &start_event));
                                        open_indices.insert(index);
                                    }

                                    reasoning_text_by_index
                                        .entry(index)
                                        .or_default()
                                        .push_str(delta);

                                    let event = json!({
                                        "type": "content_block_delta",
                                        "index": index,
                                        "delta": {
                                            "type": "thinking_delta",
                                            "thinking": delta
                                        }
                                    });
                                    yield Ok(anthropic_sse("content_block_delta", &event));
                                }
                            }

                            // ================================================
                            // Official done events carry the complete visible text. If a
                            // gateway omitted deltas, emit the text here. The block stays
                            // open until output_item.done supplies encrypted_content.
                            // ================================================
                            "response.reasoning_summary_text.done"
                            | "response.reasoning_text.done" => {
                                let item_id = data.get("item_id").and_then(Value::as_str);
                                let item_key = reasoning_item_key(&data, None);
                                let index = item_id
                                    .and_then(|id| reasoning_index_by_item_id.get(id).copied())
                                    .or_else(|| {
                                        item_key
                                            .as_ref()
                                            .and_then(|key| index_by_key.get(key).copied())
                                    })
                                    .or_else(|| {
                                        (item_id.is_none() && item_key.is_none())
                                            .then_some(legacy_reasoning_index)
                                            .flatten()
                                    });
                                if let Some(index) = index {
                                    let already_emitted = reasoning_text_by_index
                                        .get(&index)
                                        .is_some_and(|value| !value.is_empty());
                                    if !already_emitted {
                                        if let Some(text) = data
                                            .get("text")
                                            .and_then(Value::as_str)
                                            .filter(|value| !value.is_empty())
                                        {
                                            if !open_indices.contains(&index) {
                                                let start_event = json!({
                                                    "type": "content_block_start",
                                                    "index": index,
                                                    "content_block": {"type": "thinking", "thinking": ""}
                                                });
                                                yield Ok(anthropic_sse("content_block_start", &start_event));
                                                open_indices.insert(index);
                                            }
                                            reasoning_text_by_index
                                                .entry(index)
                                                .or_default()
                                                .push_str(text);
                                            let event = json!({
                                                "type": "content_block_delta",
                                                "index": index,
                                                "delta": {"type": "thinking_delta", "thinking": text}
                                            });
                                            yield Ok(anthropic_sse("content_block_delta", &event));
                                        }
                                    }
                                }
                            }

                            // Legacy gateways do not emit output_item.done, so retain the
                            // old close behavior for their non-standard done event.
                            "response.reasoning.done" => {
                                let item_id = data.get("item_id").and_then(Value::as_str);
                                let item_key = reasoning_item_key(&data, None);
                                let index = item_id
                                    .and_then(|id| reasoning_index_by_item_id.get(id).copied())
                                    .or_else(|| {
                                        item_key
                                            .as_ref()
                                            .and_then(|key| index_by_key.get(key).copied())
                                    })
                                    .or_else(|| {
                                        (item_id.is_none() && item_key.is_none())
                                            .then_some(legacy_reasoning_index)
                                            .flatten()
                                    });
                                if let Some(index) = index {
                                    if open_indices.remove(&index) {
                                        let event = json!({"type": "content_block_stop", "index": index});
                                        yield Ok(anthropic_sse("content_block_stop", &event));
                                    }
                                    if legacy_reasoning_index == Some(index) {
                                        legacy_reasoning_index = None;
                                    }
                                }
                            }

                            "response.completed" | "response.incomplete" => {
                                let response_obj = response_object_from_event(&data);
                                if matches!(
                                    response_obj.get("status").and_then(Value::as_str),
                                    Some("failed" | "cancelled")
                                ) || response_obj
                                    .get("error")
                                    .is_some_and(|error| !error.is_null())
                                {
                                    let (message, error_type) = responses_error_details(
                                        &data,
                                        "Responses upstream returned a failed terminal response",
                                    );
                                    stream_completion.record_error(message.clone());
                                    yield Ok(error_sse(&message, &error_type));
                                    return;
                                }
                                if !has_sent_message_start {
                                    if let Some(id) = response_obj.get("id").and_then(Value::as_str) {
                                        message_id = Some(id.to_string());
                                    }
                                    if let Some(model) = response_obj.get("model").and_then(Value::as_str) {
                                        current_model = Some(model.to_string());
                                    }
                                    yield Ok(message_start_sse(
                                        &message_id,
                                        &current_model,
                                        json!({ "input_tokens": 0, "output_tokens": 0 }),
                                    ));
                                }
                                let terminal_status = response_obj
                                    .get("status")
                                    .and_then(Value::as_str)
                                    .or(match event_name {
                                        "response.incomplete" => Some("incomplete"),
                                        _ => Some("completed"),
                                    });
                                let stop_reason = map_responses_stop_reason(
                                    terminal_status,
                                    has_tool_use,
                                    response_obj
                                        .pointer("/incomplete_details/reason")
                                        .and_then(|r| r.as_str()),
                                );

                                if !open_indices.is_empty() {
                                    let mut remaining: Vec<u32> = open_indices.iter().copied().collect();
                                    remaining.sort_unstable();
                                    for index in remaining {
                                        let stop_event = json!({ "type": "content_block_stop", "index": index });
                                        let stop_sse = format!("event: content_block_stop\ndata: {}\n\n", serde_json::to_string(&stop_event).unwrap_or_default());
                                        yield Ok(Bytes::from(stop_sse));
                                        open_indices.remove(&index);
                                    }
                                }
                                let delta_event = json!({
                                    "type": "message_delta",
                                    "delta": {
                                        "stop_reason": stop_reason,
                                        "stop_sequence": null
                                    },
                                    "usage": response_obj
                                        .get("usage")
                                        .map_or_else(
                                            || build_anthropic_usage_from_responses(None),
                                            |u| build_anthropic_usage_from_responses(Some(u))
                                        )
                                });
                                yield Ok(anthropic_sse("message_delta", &delta_event));

                                let stop_event = json!({"type": "message_stop"});
                                stream_completion.record_success();
                                yield Ok(anthropic_sse("message_stop", &stop_event));
                                return;
                            }
                            "response.failed" | "error" => {
                                let (message, error_type) = responses_error_details(
                                    &data,
                                    if event_name == "response.failed" {
                                        "Responses upstream reported response.failed"
                                    } else {
                                        "Responses upstream emitted an error event"
                                    },
                                );
                                stream_completion.record_error(message.clone());
                                yield Ok(error_sse(&message, &error_type));
                                return;
                            }
                            "response.output_text.done" => {
                                if let Some(index) = current_text_index.take() {
                                    if open_indices.remove(&index) {
                                        let stop_event = json!({ "type": "content_block_stop", "index": index });
                                        let stop_sse = format!("event: content_block_stop\ndata: {}\n\n", serde_json::to_string(&stop_event).unwrap_or_default());
                                        yield Ok(Bytes::from(stop_sse));
                                    }
                                    if fallback_open_index == Some(index) {
                                        fallback_open_index = None;
                                    }
                                }
                            }
                            "response.output_item.done" => {
                                let Some(item) = data.get("item") else { continue; };
                                if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                                    let item_id = item
                                        .get("id")
                                        .and_then(Value::as_str)
                                        .or_else(|| data.get("item_id").and_then(Value::as_str));
                                    let index = item_id
                                        .and_then(|id| reasoning_index_by_item_id.get(id).copied())
                                        .or_else(|| {
                                            reasoning_item_key(&data, Some(item))
                                                .and_then(|key| index_by_key.get(&key).copied())
                                        })
                                        .unwrap_or_else(|| {
                                            let assigned = next_content_index;
                                            next_content_index += 1;
                                            assigned
                                        });
                                    reasoning_item_by_index.insert(index, item.clone());

                                    let final_item = reasoning_item_by_index
                                        .get(&index)
                                        .cloned()
                                        .unwrap_or_else(|| item.clone());
                                    let full_text = reasoning_summary_text(&final_item);
                                    let emitted_text = reasoning_text_by_index
                                        .get(&index)
                                        .cloned()
                                        .unwrap_or_default();
                                    if emitted_text.is_empty() && !full_text.is_empty() {
                                        let start_event = json!({
                                            "type": "content_block_start",
                                            "index": index,
                                            "content_block": {"type": "thinking", "thinking": ""}
                                        });
                                        yield Ok(anthropic_sse("content_block_start", &start_event));
                                        open_indices.insert(index);
                                        let delta_event = json!({
                                            "type": "content_block_delta",
                                            "index": index,
                                            "delta": {"type": "thinking_delta", "thinking": full_text}
                                        });
                                        yield Ok(anthropic_sse("content_block_delta", &delta_event));
                                    } else if open_indices.contains(&index) {
                                        // Deltas may cover only the start of the summary
                                        // (e.g. a later summary part arrived without
                                        // deltas). Emit the missing tail; snapshots that
                                        // do not extend the streamed text are ignored.
                                        if let Some(suffix) = full_text
                                            .strip_prefix(emitted_text.as_str())
                                            .filter(|suffix| !suffix.is_empty())
                                        {
                                            let delta_event = json!({
                                                "type": "content_block_delta",
                                                "index": index,
                                                "delta": {"type": "thinking_delta", "thinking": suffix}
                                            });
                                            yield Ok(anthropic_sse("content_block_delta", &delta_event));
                                        }
                                    }

                                    let encrypted = final_item
                                        .get("encrypted_content")
                                        .and_then(Value::as_str)
                                        .is_some_and(|value| !value.is_empty());
                                    if encrypted {
                                        if let Some(envelope) = encode_openai_reasoning_item(&final_item) {
                                            if open_indices.contains(&index) {
                                                let signature_event = json!({
                                                    "type": "content_block_delta",
                                                    "index": index,
                                                    "delta": {
                                                        "type": "signature_delta",
                                                        "signature": envelope
                                                    }
                                                });
                                                yield Ok(anthropic_sse("content_block_delta", &signature_event));
                                            } else {
                                                // Encrypted-only item: an empty thinking block
                                                // carries the envelope (Claude Code's VS Code
                                                // extension cannot render redacted_thinking).
                                                let start_event = json!({
                                                    "type": "content_block_start",
                                                    "index": index,
                                                    "content_block": {"type": "thinking", "thinking": ""}
                                                });
                                                yield Ok(anthropic_sse("content_block_start", &start_event));
                                                open_indices.insert(index);
                                                let signature_event = json!({
                                                    "type": "content_block_delta",
                                                    "index": index,
                                                    "delta": {
                                                        "type": "signature_delta",
                                                        "signature": envelope
                                                    }
                                                });
                                                yield Ok(anthropic_sse("content_block_delta", &signature_event));
                                            }
                                        }
                                    }
                                    if open_indices.remove(&index) {
                                        let stop_event = json!({"type": "content_block_stop", "index": index});
                                        yield Ok(anthropic_sse("content_block_stop", &stop_event));
                                    }
                                    if let Some(id) = item_id {
                                        reasoning_index_by_item_id.remove(id);
                                    }
                                    reasoning_item_by_index.remove(&index);
                                    reasoning_text_by_index.remove(&index);
                                }
                            }

                            "response.in_progress"
                            | "response.queued" => {}
                            _ => {}
                        }
                    }
                }
                Err(error) => {
                    stream_completion.record_error(error.to_string());
                    yield Ok(error_sse(&format!("Stream error: {error}"), "stream_error"));
                    return;
                }
            }
        }

        let message = "Responses upstream stream ended before a terminal event";
        stream_completion.record_error(message.to_string());
        yield Ok(error_sse(message, "stream_truncated"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{stream, StreamExt};

    async fn collect_stream(input: Vec<Bytes>) -> (String, StreamCompletion) {
        let upstream = stream::iter(input.into_iter().map(Ok::<_, std::io::Error>));
        let completion = StreamCompletion::default();
        let converted = create_anthropic_sse_stream_from_responses(upstream, completion.clone());
        let chunks: Vec<_> = converted.collect().await;
        let merged = chunks
            .into_iter()
            .map(|chunk| String::from_utf8_lossy(chunk.unwrap().as_ref()).to_string())
            .collect::<String>();
        (merged, completion)
    }

    fn parse_anthropic_events(merged: &str) -> Vec<Value> {
        merged
            .split("\n\n")
            .filter_map(|block| {
                let data = block
                    .lines()
                    .find_map(|line| strip_sse_field(line, "data"))?;
                serde_json::from_str::<Value>(data).ok()
            })
            .collect()
    }

    #[tokio::test]
    async fn terminal_only_event_emits_complete_lifecycle_without_trailing_blank_line() {
        let input = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_terminal\",\"model\":\"gpt-5.4\",\"status\":\"completed\"}}";

        let (merged, completion) = collect_stream(vec![Bytes::from(input)]).await;
        let events = parse_anthropic_events(&merged);
        let event_types: Vec<_> = events
            .iter()
            .filter_map(|event| event["type"].as_str())
            .collect();

        assert_eq!(
            event_types,
            ["message_start", "message_delta", "message_stop"]
        );
        assert_eq!(events[0]["message"]["id"], "resp_terminal");
        assert_eq!(events[0]["message"]["model"], "gpt-5.4");
        assert_eq!(completion.outcome(), Some(Ok(())));
    }

    #[tokio::test]
    async fn failed_responses_emit_error_without_message_stop() {
        let cases = [
            (
                "response.failed",
                "event: response.failed\ndata: {\"response\":{\"status\":\"failed\",\"error\":{\"type\":\"server_error\",\"message\":\"upstream failed\"}}}\n\n",
                "upstream failed",
            ),
            (
                "standalone error",
                "event: error\ndata: {\"error\":{\"type\":\"rate_limit_error\",\"message\":\"too many requests\"}}\n\n",
                "too many requests",
            ),
            (
                "failed completion",
                "event: response.completed\ndata: {\"response\":{\"status\":\"failed\",\"error\":{\"type\":\"server_error\",\"message\":\"failed wrapper\"}}}\n\n",
                "failed wrapper",
            ),
            (
                "cancelled completion",
                "event: response.completed\ndata: {\"response\":{\"status\":\"cancelled\"}}\n\n",
                "Responses upstream returned a failed terminal response",
            ),
        ];

        for (case, input, expected_message) in cases {
            let (merged, completion) = collect_stream(vec![Bytes::from(input)]).await;
            let events = parse_anthropic_events(&merged);

            assert_eq!(events.len(), 1, "{case}");
            assert_eq!(events[0]["type"], "error", "{case}");
            assert_eq!(events[0]["error"]["message"], expected_message, "{case}");
            assert!(!merged.contains("event: message_stop"), "{case}");
            assert_eq!(
                completion.outcome(),
                Some(Err(expected_message.to_string())),
                "{case}"
            );
        }
    }

    #[tokio::test]
    async fn eof_without_terminal_event_is_reported_as_truncated() {
        let input = concat!(
            "event: response.created\n",
            "data: {\"response\":{\"id\":\"resp_truncated\",\"model\":\"gpt-5.4\"}}\n\n"
        );

        let (merged, completion) = collect_stream(vec![Bytes::from(input)]).await;
        let events = parse_anthropic_events(&merged);

        assert_eq!(events[0]["type"], "message_start");
        assert_eq!(events[1]["type"], "error");
        assert_eq!(events[1]["error"]["type"], "stream_truncated");
        assert!(!merged.contains("event: message_stop"));
        assert_eq!(
            completion.outcome(),
            Some(Err(
                "Responses upstream stream ended before a terminal event".to_string()
            ))
        );
    }

    #[tokio::test]
    async fn completed_event_ends_stream_without_waiting_for_upstream_eof() {
        let input = concat!(
            "event: response.created\ndata: {\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-4.1-mini\",\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
            "event: response.completed\ndata: {\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":2,\"output_tokens\":1}}}\n\n"
        );
        let upstream = stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(input))])
            .chain(stream::pending::<Result<Bytes, std::io::Error>>());
        let completion = StreamCompletion::default();
        let converted = create_anthropic_sse_stream_from_responses(upstream, completion.clone());

        let chunks: Vec<_> =
            tokio::time::timeout(std::time::Duration::from_millis(100), converted.collect())
                .await
                .expect("stream should end at response.completed");
        let merged = chunks
            .into_iter()
            .map(|chunk| String::from_utf8_lossy(chunk.unwrap().as_ref()).to_string())
            .collect::<String>();

        assert!(merged.contains("event: message_stop"));
        assert_eq!(completion.outcome(), Some(Ok(())));
    }

    #[tokio::test]
    async fn parses_crlf_sse_events() {
        let input = "event: response.created\r\n\
data: {\"response\":{\"id\":\"resp_crlf\",\"model\":\"gpt-5.4\"}}\r\n\
\r\n\
event: response.completed\r\n\
data: {\"response\":{\"status\":\"completed\"}}\r\n\
\r\n";

        let (merged, completion) = collect_stream(vec![Bytes::from(input)]).await;

        assert!(merged.contains("\"id\":\"resp_crlf\""));
        assert!(merged.contains("event: message_stop"));
        assert_eq!(completion.outcome(), Some(Ok(())));
    }

    #[tokio::test]
    async fn preserves_split_utf8_chunks() {
        let input = concat!(
            "event: response.created\n",
            "data: {\"response\":{\"id\":\"resp_cn\",\"model\":\"gpt-5.4\"}}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"delta\":\"你好世界\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"status\":\"completed\"}}\n\n"
        );
        let bytes = input.as_bytes();
        let split_at = bytes
            .windows("你".len())
            .position(|window| window == "你".as_bytes())
            .expect("find Chinese character")
            + 2;

        let (merged, _) = collect_stream(vec![
            Bytes::copy_from_slice(&bytes[..split_at]),
            Bytes::copy_from_slice(&bytes[split_at..]),
        ])
        .await;

        assert!(merged.contains("你好世界"));
        assert!(!merged.contains('\u{FFFD}'));
    }

    #[tokio::test]
    async fn emits_done_only_tool_arguments_before_stop() {
        let input = concat!(
            "event: response.created\n",
            "data: {\"response\":{\"id\":\"resp_tool\",\"model\":\"gpt-5.4\"}}\n\n",
            "event: response.output_item.added\n",
            "data: {\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"get_weather\"}}\n\n",
            "event: response.function_call_arguments.done\n",
            "data: {\"item_id\":\"fc_1\",\"arguments\":\"{\\\"city\\\":\\\"Tokyo\\\"}\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"status\":\"completed\"}}\n\n"
        );

        let (merged, _) = collect_stream(vec![Bytes::from(input)]).await;

        assert!(merged.contains("\"type\":\"input_json_delta\""));
        assert!(merged.contains("\\\"city\\\":\\\"Tokyo\\\""));
        assert!(merged.contains("\"stop_reason\":\"tool_use\""));
    }

    #[tokio::test]
    async fn tool_first_message_start_is_spec_complete() {
        // A response that opens with a function_call (no preceding text) emits
        // message_start from the tool path; it must still carry the spec-required
        // fields or the official Anthropic SDK stream accumulator crashes.
        let input = concat!(
            "event: response.created\n",
            "data: {\"response\":{\"id\":\"resp_tool\",\"model\":\"gpt-5.4\"}}\n\n",
            "event: response.output_item.added\n",
            "data: {\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"get_weather\"}}\n\n",
            "event: response.function_call_arguments.done\n",
            "data: {\"item_id\":\"fc_1\",\"arguments\":\"{}\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"status\":\"completed\"}}\n\n"
        );

        let (merged, _) = collect_stream(vec![Bytes::from(input)]).await;
        let events = parse_anthropic_events(&merged);
        let message = &events
            .iter()
            .find(|event| event["type"] == "message_start")
            .expect("message_start event")["message"];

        assert_eq!(message["content"], serde_json::json!([]));
        assert_eq!(message["stop_reason"], Value::Null);
        assert_eq!(message["stop_sequence"], Value::Null);
    }

    #[tokio::test]
    async fn non_read_tool_empty_delta_still_uses_done_arguments() {
        let input = concat!(
            "event: response.created\n",
            "data: {\"response\":{\"id\":\"resp_tool\",\"model\":\"gpt-5.4\"}}\n\n",
            "event: response.output_item.added\n",
            "data: {\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"get_weather\"}}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"item_id\":\"fc_1\",\"delta\":\"\"}\n\n",
            "event: response.function_call_arguments.done\n",
            "data: {\"item_id\":\"fc_1\",\"arguments\":\"{\\\"city\\\":\\\"Tokyo\\\"}\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"status\":\"completed\"}}\n\n"
        );

        let (merged, _) = collect_stream(vec![Bytes::from(input)]).await;

        assert!(merged.contains("\\\"city\\\":\\\"Tokyo\\\""));
        assert!(merged.contains("\"stop_reason\":\"tool_use\""));
    }

    #[tokio::test]
    async fn read_tool_arguments_are_buffered_and_sanitized() {
        let input = concat!(
            "event: response.created\n",
            "data: {\"response\":{\"id\":\"resp_read\",\"model\":\"gpt-5.4\"}}\n\n",
            "event: response.output_item.added\n",
            "data: {\"item\":{\"id\":\"fc_read\",\"type\":\"function_call\",\"call_id\":\"call_read\",\"name\":\"Read\"}}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"item_id\":\"fc_read\",\"delta\":\"{\\\"file_path\\\":\\\"/tmp/demo.py\\\",\\\"limit\\\":2000,\\\"offset\\\":0,\\\"pages\\\":\\\"\\\"}\"}\n\n",
            "event: response.function_call_arguments.done\n",
            "data: {\"item_id\":\"fc_read\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"status\":\"completed\"}}\n\n"
        );

        let (merged, _) = collect_stream(vec![Bytes::from(input)]).await;

        assert!(merged.contains("\"name\":\"Read\""));
        assert!(merged.contains("\"partial_json\":\"{\\\"file_path\\\":\\\"/tmp/demo.py\\\",\\\"limit\\\":2000,\\\"offset\\\":0}"));
        assert!(!merged.contains("\\\"pages\\\":\\\"\\\""));
    }

    #[tokio::test]
    async fn read_tool_duplicate_start_preserves_buffered_arguments() {
        let input = concat!(
            "event: response.created\n",
            "data: {\"response\":{\"id\":\"resp_read\",\"model\":\"gpt-5.4\"}}\n\n",
            "event: response.output_item.added\n",
            "data: {\"item\":{\"id\":\"fc_read\",\"type\":\"function_call\",\"call_id\":\"call_read\",\"name\":\"Read\"}}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"item_id\":\"fc_read\",\"delta\":\"{\\\"file_path\\\":\\\"/tmp/demo.py\\\",\\\"limit\\\":2000,\\\"offset\\\":0,\\\"pages\\\":\\\"\\\"}\"}\n\n",
            "event: response.output_item.added\n",
            "data: {\"item\":{\"id\":\"fc_read\",\"type\":\"function_call\",\"call_id\":\"call_read\",\"name\":\"Read\"}}\n\n",
            "event: response.function_call_arguments.done\n",
            "data: {\"item_id\":\"fc_read\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"status\":\"completed\"}}\n\n"
        );

        let (merged, _) = collect_stream(vec![Bytes::from(input)]).await;

        assert_eq!(merged.matches("event: content_block_start").count(), 1);
        assert_eq!(merged.matches("event: content_block_stop").count(), 1);
        assert!(merged.contains("\"partial_json\":\"{\\\"file_path\\\":\\\"/tmp/demo.py\\\",\\\"limit\\\":2000,\\\"offset\\\":0}"));
        assert!(!merged.contains("\\\"pages\\\":\\\"\\\""));
    }

    #[tokio::test]
    async fn read_tool_split_argument_deltas_are_still_joined() {
        let input = concat!(
            "event: response.created\n",
            "data: {\"response\":{\"id\":\"resp_read\",\"model\":\"gpt-5.4\"}}\n\n",
            "event: response.output_item.added\n",
            "data: {\"item\":{\"id\":\"fc_read\",\"type\":\"function_call\",\"call_id\":\"call_read\",\"name\":\"Read\"}}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"item_id\":\"fc_read\",\"delta\":\"{\\\"file_path\\\":\\\"/tmp/demo.py\\\",\\\"limit\\\":2000,\"}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"item_id\":\"fc_read\",\"delta\":\"\\\"offset\\\":0,\\\"pages\\\":\\\"\\\"}\"}\n\n",
            "event: response.function_call_arguments.done\n",
            "data: {\"item_id\":\"fc_read\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"status\":\"completed\"}}\n\n"
        );

        let (merged, _) = collect_stream(vec![Bytes::from(input)]).await;

        assert!(merged.contains("\"partial_json\":\"{\\\"file_path\\\":\\\"/tmp/demo.py\\\",\\\"limit\\\":2000,\\\"offset\\\":0}"));
        assert!(!merged.contains("\\\"pages\\\":\\\"\\\""));
    }

    #[tokio::test]
    async fn read_tool_snapshot_argument_deltas_use_latest_complete_json() {
        let input = concat!(
            "event: response.created\n",
            "data: {\"response\":{\"id\":\"resp_read\",\"model\":\"gpt-5.4\"}}\n\n",
            "event: response.output_item.added\n",
            "data: {\"item\":{\"id\":\"fc_read\",\"type\":\"function_call\",\"call_id\":\"call_read\",\"name\":\"Read\"}}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"item_id\":\"fc_read\",\"delta\":\"{\\\"file_path\\\":\\\"/tmp/demo.py\\\",\\\"limit\\\":2000,\\\"offset\\\":320,\\\"pages\\\":\\\"\\\"}\"}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"item_id\":\"fc_read\",\"delta\":\"{\\\"file_path\\\":\\\"/tmp/demo.py\\\",\\\"limit\\\":2000,\\\"offset\\\":320,\\\"pages\\\":\\\"\\\"}\"}\n\n",
            "event: response.function_call_arguments.done\n",
            "data: {\"item_id\":\"fc_read\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"status\":\"completed\"}}\n\n"
        );

        let (merged, _) = collect_stream(vec![Bytes::from(input)]).await;

        assert!(merged.contains("\"partial_json\":\"{\\\"file_path\\\":\\\"/tmp/demo.py\\\",\\\"limit\\\":2000,\\\"offset\\\":320}"));
        assert!(!merged.contains("\\\"offset\\\":320,\\\"pages\\\":\\\"\\\"}{"));
        assert!(!merged.contains("\\\"pages\\\":\\\"\\\""));
    }

    #[tokio::test]
    async fn read_tool_incomplete_argument_deltas_are_not_treated_as_snapshots() {
        let input = concat!(
            "event: response.created\n",
            "data: {\"response\":{\"id\":\"resp_read\",\"model\":\"gpt-5.4\"}}\n\n",
            "event: response.output_item.added\n",
            "data: {\"item\":{\"id\":\"fc_read\",\"type\":\"function_call\",\"call_id\":\"call_read\",\"name\":\"Read\"}}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"item_id\":\"fc_read\",\"delta\":\"{\\\"file_path\\\":\\\"/tmp/demo.py\\\",\\\"limit\\\":\"}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"item_id\":\"fc_read\",\"delta\":\"2000\"}\n\n",
            "event: response.function_call_arguments.done\n",
            "data: {\"item_id\":\"fc_read\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"status\":\"completed\"}}\n\n"
        );

        let (merged, _) = collect_stream(vec![Bytes::from(input)]).await;

        assert!(merged.contains(
            "\"partial_json\":\"{\\\"file_path\\\":\\\"/tmp/demo.py\\\",\\\"limit\\\":2000"
        ));
    }

    #[tokio::test]
    async fn tool_start_closes_open_text_block_first() {
        let input = concat!(
            "event: response.created\n",
            "data: {\"response\":{\"id\":\"resp_mixed\",\"model\":\"gpt-5.4\"}}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"delta\":\"Checking\"}\n\n",
            "event: response.output_item.added\n",
            "data: {\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"get_weather\"}}\n\n",
            "event: response.function_call_arguments.done\n",
            "data: {\"item_id\":\"fc_1\",\"arguments\":\"{\\\"city\\\":\\\"Tokyo\\\"}\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"status\":\"completed\"}}\n\n"
        );

        let (merged, _) = collect_stream(vec![Bytes::from(input)]).await;
        let events = parse_anthropic_events(&merged);
        let text_start = events
            .iter()
            .position(|event| {
                event.get("type").and_then(|value| value.as_str()) == Some("content_block_start")
                    && event
                        .pointer("/content_block/type")
                        .and_then(|value| value.as_str())
                        == Some("text")
            })
            .expect("text block should start");
        let first_stop = events
            .iter()
            .position(|event| {
                event.get("type").and_then(|value| value.as_str()) == Some("content_block_stop")
            })
            .expect("text block should stop before tool");
        let tool_start = events
            .iter()
            .position(|event| {
                event.get("type").and_then(|value| value.as_str()) == Some("content_block_start")
                    && event
                        .pointer("/content_block/type")
                        .and_then(|value| value.as_str())
                        == Some("tool_use")
                    && event
                        .pointer("/content_block/id")
                        .and_then(|value| value.as_str())
                        == Some("call_1")
            })
            .expect("tool block should start");

        assert!(text_start < first_stop);
        assert!(first_stop < tool_start);
    }

    #[tokio::test]
    async fn completed_without_usage_emits_zero_usage_object() {
        let input = concat!(
            "event: response.created\n",
            "data: {\"response\":{\"id\":\"resp_usage\",\"model\":\"gpt-5.4\"}}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"status\":\"completed\"}}\n\n"
        );

        let (merged, _) = collect_stream(vec![Bytes::from(input)]).await;

        assert!(merged.contains("\"usage\":{\"input_tokens\":0,\"output_tokens\":0}"));
        assert!(!merged.contains("\"usage\":null"));
    }

    #[tokio::test]
    async fn encrypted_reasoning_stream_round_trips_through_tool_history() {
        use super::super::{
            reasoning_bridge::decode_openai_reasoning_item,
            transform_responses::anthropic_to_responses,
        };
        // Full deltas, a missing summary tail, done-only text, and encrypted-only items.
        for (summary, delta) in [
            ("Need a tool.", Some("Need a tool.")),
            ("Need a tool.", Some("Need ")),
            ("Need a tool.", None),
            ("", None),
        ] {
            let mut items = Vec::new();
            let mut upstream = vec![
                json!({"type":"response.created", "response":{"id":"resp_1", "model":"grok-4.7"}}),
            ];
            // Interleaved items must keep separate signatures and stable indices.
            for index in 0..2 {
                let id = format!("rs_{index}");
                upstream.push(json!({"type":"response.output_item.added", "output_index":index, "item":{"type":"reasoning", "id":id, "summary":[]}}));
                if let Some(delta) = delta {
                    upstream.push(json!({"type":"response.reasoning_summary_text.delta", "item_id":id, "output_index":index, "delta":delta}));
                    upstream.push(json!({"type":"response.reasoning_summary_text.done", "item_id":id, "output_index":index, "text":delta}));
                }
                items.push(json!({"type":"reasoning", "id":id, "summary":[{"type":"summary_text", "text":summary}], "encrypted_content":format!("opaque-{index}")}));
            }
            for index in (0..2).rev() {
                let mut completed_item = items[index].clone();
                completed_item["status"] = json!("completed");
                upstream.push(json!({"type":"response.output_item.done", "output_index":index, "item":completed_item}));
            }
            upstream.push(json!({"type":"response.output_item.added", "output_index":2, "item":{"type":"function_call", "id":"fc_1", "call_id":"call_1", "name":"lookup"}}));
            upstream.push(json!({"type":"response.function_call_arguments.delta", "item_id":"fc_1", "delta":"{}"}));
            upstream.push(json!({"type":"response.function_call_arguments.done", "item_id":"fc_1", "arguments":"{}"}));
            upstream.push(json!({"type":"response.completed", "response":{"status":"completed"}}));
            let wire = upstream
                .into_iter()
                .map(|event| {
                    format!(
                        "event: {}\ndata: {event}\n\n",
                        event["type"].as_str().unwrap()
                    )
                })
                .collect::<String>();
            let (output, completion) = collect_stream(
                wire.as_bytes()
                    .chunks(17)
                    .map(Bytes::copy_from_slice)
                    .collect(),
            )
            .await;
            assert_eq!(completion.outcome(), Some(Ok(())));
            let events = parse_anthropic_events(&output);
            let mut blocks = std::collections::BTreeMap::new();
            let mut closed = std::collections::HashSet::new();
            for event in &events {
                let index = event["index"].as_u64().unwrap_or(u64::MAX);
                match event["type"].as_str().unwrap() {
                    "content_block_start" => {
                        assert!(blocks
                            .insert(index, event["content_block"].clone())
                            .is_none());
                    }
                    "content_block_delta"
                        if event["delta"]["type"] == "thinking_delta"
                            || event["delta"]["type"] == "signature_delta" =>
                    {
                        assert!(!closed.contains(&index));
                        let block = blocks.get_mut(&index).unwrap();
                        let field = if event["delta"]["type"] == "thinking_delta" {
                            "thinking"
                        } else {
                            "signature"
                        };
                        block[field] = json!(format!(
                            "{}{}",
                            block[field].as_str().unwrap_or(""),
                            event["delta"][field].as_str().unwrap()
                        ));
                    }
                    "content_block_stop" => {
                        assert!(closed.insert(index));
                    }
                    _ => {}
                }
            }
            let mut history = Vec::new();
            for (index, item) in items.iter().enumerate() {
                let block = blocks.get(&(index as u64)).unwrap();
                assert_eq!(block["thinking"], summary);
                assert_eq!(
                    decode_openai_reasoning_item(block["signature"].as_str().unwrap()),
                    Some(item.clone())
                );
                history.push(block.clone());
            }
            history.push(json!({"type":"tool_use", "id":"call_1", "name":"lookup", "input":{}}));
            let replay = anthropic_to_responses(json!({"messages":[{"role":"assistant", "content":history}, {"role":"user", "content":[{"type":"tool_result", "tool_use_id":"call_1", "content":"ok"}]}]}), None, false, false).unwrap();
            assert_eq!(&replay["input"].as_array().unwrap()[..2], &items);
            assert_eq!(replay["input"][2]["type"], "function_call");
            assert_eq!(replay["input"][3]["type"], "function_call_output");
        }
    }
}
