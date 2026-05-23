use bytes::Bytes;
use futures::stream::{Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

use crate::proxy::response::StreamCompletion;

use super::transform_responses::{
    build_anthropic_usage_from_responses, map_responses_stop_reason,
    sanitize_anthropic_tool_use_input,
};

fn sanitize_tool_arguments_json(tool_name: &str, arguments: &str) -> String {
    let Ok(input) = serde_json::from_str::<Value>(arguments) else {
        return arguments.to_string();
    };

    serde_json::to_string(&sanitize_anthropic_tool_use_input(tool_name, input))
        .unwrap_or_else(|_| arguments.to_string())
}

#[inline]
fn response_object_from_event(data: &Value) -> &Value {
    data.get("response").unwrap_or(data)
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
        let mut message_id: Option<String> = None;
        let mut current_model: Option<String> = None;
        let mut has_sent_message_start = false;
        let mut has_tool_use = false;
        let mut next_content_index: u32 = 0;
        let mut index_by_key: HashMap<String, u32> = HashMap::new();
        let mut open_indices: HashSet<u32> = HashSet::new();
        let mut fallback_open_index: Option<u32> = None;
        let mut tool_index_by_item_id: HashMap<String, u32> = HashMap::new();
        let mut tool_name_by_index: HashMap<u32, String> = HashMap::new();
        let mut read_arguments_by_index: HashMap<u32, String> = HashMap::new();
        let mut last_tool_index: Option<u32> = None;

        tokio::pin!(stream);

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    buffer.push_str(&text);

                    while let Some(pos) = buffer.find("\n\n") {
                        let block = buffer[..pos].to_string();
                        buffer = buffer[pos + 2..].to_string();

                        if block.trim().is_empty() {
                            continue;
                        }

                        let mut event_type: Option<String> = None;
                        let mut data_parts: Vec<String> = Vec::new();

                        for line in block.lines() {
                            if let Some(evt) = line.strip_prefix("event: ") {
                                event_type = Some(evt.trim().to_string());
                            } else if let Some(d) = line.strip_prefix("data: ") {
                                data_parts.push(d.to_string());
                            }
                        }

                        if data_parts.is_empty() {
                            continue;
                        }

                        let data_str = data_parts.join("\n");
                        let event_name = event_type.as_deref().unwrap_or("");
                        let data: Value = match serde_json::from_str(&data_str) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

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
                                let event = json!({
                                    "type": "message_start",
                                    "message": {
                                        "id": message_id.clone().unwrap_or_default(),
                                        "type": "message",
                                        "role": "assistant",
                                        "model": current_model.clone().unwrap_or_default(),
                                        "usage": build_anthropic_usage_from_responses(response_obj.get("usage"))
                                    }
                                });
                                let sse = format!("event: message_start\ndata: {}\n\n", serde_json::to_string(&event).unwrap_or_default());
                                yield Ok(Bytes::from(sse));
                            }
                            "response.content_part.added" => {
                                if !has_sent_message_start {
                                    let start_event = json!({
                                        "type": "message_start",
                                        "message": {
                                            "id": message_id.clone().unwrap_or_default(),
                                            "type": "message",
                                            "role": "assistant",
                                            "model": current_model.clone().unwrap_or_default(),
                                            "usage": { "input_tokens": 0, "output_tokens": 0 }
                                        }
                                    });
                                    let sse = format!("event: message_start\ndata: {}\n\n", serde_json::to_string(&start_event).unwrap_or_default());
                                    yield Ok(Bytes::from(sse));
                                    has_sent_message_start = true;
                                }

                                if let Some(part) = data.get("part") {
                                    let part_type = part.get("type").and_then(|t| t.as_str());
                                    if matches!(part_type, Some("output_text") | Some("refusal")) {
                                        let index = resolve_content_index(
                                            &data,
                                            &mut next_content_index,
                                            &mut index_by_key,
                                            &mut fallback_open_index,
                                        );
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
                                    let index = resolve_content_index(
                                        &data,
                                        &mut next_content_index,
                                        &mut index_by_key,
                                        &mut fallback_open_index,
                                    );

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
                            "response.content_part.done" | "response.refusal.done" | "response.reasoning.done" => {
                                let key = content_part_key(&data);
                                let index = if let Some(k) = key {
                                    index_by_key.get(&k).copied()
                                } else {
                                    fallback_open_index
                                };
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
                                        if !has_sent_message_start {
                                            let start_event = json!({
                                                "type": "message_start",
                                                "message": {
                                                    "id": message_id.clone().unwrap_or_default(),
                                                    "type": "message",
                                                    "role": "assistant",
                                                    "model": current_model.clone().unwrap_or_default(),
                                                    "usage": { "input_tokens": 0, "output_tokens": 0 }
                                                }
                                            });
                                            let sse = format!("event: message_start\ndata: {}\n\n", serde_json::to_string(&start_event).unwrap_or_default());
                                            yield Ok(Bytes::from(sse));
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
                                        last_tool_index = Some(index);
                                        tool_name_by_index.insert(index, name.to_string());

                                        if open_indices.contains(&index) {
                                            continue;
                                        }

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

                                    if let Some(item_id) = item_id {
                                        tool_index_by_item_id
                                            .entry(item_id.to_string())
                                            .or_insert(index);
                                    }
                                    last_tool_index = Some(index);

                                    if let Some(name) = data.get("name").and_then(|v| v.as_str()) {
                                        tool_name_by_index.insert(index, name.to_string());
                                    }
                                    let is_read_tool = tool_name_by_index
                                        .get(&index)
                                        .is_some_and(|name| name == "Read");

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

                                    if is_read_tool {
                                        read_arguments_by_index
                                            .entry(index)
                                            .or_default()
                                            .push_str(delta);
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
                                    if tool_name_by_index
                                        .get(&index)
                                        .is_some_and(|name| name == "Read")
                                    {
                                        let arguments = data
                                            .get("arguments")
                                            .and_then(|v| v.as_str())
                                            .map(ToOwned::to_owned)
                                            .or_else(|| read_arguments_by_index.remove(&index))
                                            .unwrap_or_default();
                                        let sanitized = sanitize_tool_arguments_json("Read", &arguments);
                                        let delta_event = json!({
                                            "type": "content_block_delta",
                                            "index": index,
                                            "delta": {
                                                "type": "input_json_delta",
                                                "partial_json": sanitized
                                            }
                                        });
                                        let delta_sse = format!("event: content_block_delta\ndata: {}\n\n", serde_json::to_string(&delta_event).unwrap_or_default());
                                        yield Ok(Bytes::from(delta_sse));
                                    }
                                    let event = json!({ "type": "content_block_stop", "index": index });
                                    let sse = format!("event: content_block_stop\ndata: {}\n\n", serde_json::to_string(&event).unwrap_or_default());
                                    yield Ok(Bytes::from(sse));
                                    tool_name_by_index.remove(&index);
                                    read_arguments_by_index.remove(&index);
                                    if let Some(item_id) = item_id {
                                        tool_index_by_item_id.remove(item_id);
                                    }
                                }
                            }
                            "response.reasoning.delta" => {
                                if let Some(delta) = data
                                    .get("delta")
                                    .or_else(|| data.get("text"))
                                    .and_then(|d| d.as_str())
                                {
                                    let index = resolve_content_index(
                                        &data,
                                        &mut next_content_index,
                                        &mut index_by_key,
                                        &mut fallback_open_index,
                                    );

                                    if !open_indices.contains(&index) {
                                        let start_event = json!({
                                            "type": "content_block_start",
                                            "index": index,
                                            "content_block": { "type": "thinking", "thinking": "" }
                                        });
                                        let start_sse = format!("event: content_block_start\ndata: {}\n\n", serde_json::to_string(&start_event).unwrap_or_default());
                                        yield Ok(Bytes::from(start_sse));
                                        open_indices.insert(index);
                                    }

                                    let event = json!({
                                        "type": "content_block_delta",
                                        "index": index,
                                        "delta": { "type": "thinking_delta", "thinking": delta }
                                    });
                                    let sse = format!("event: content_block_delta\ndata: {}\n\n", serde_json::to_string(&event).unwrap_or_default());
                                    yield Ok(Bytes::from(sse));
                                }
                            }
                            "response.completed" => {
                                let response_obj = response_object_from_event(&data);
                                let stop_reason = map_responses_stop_reason(
                                    response_obj.get("status").and_then(|s| s.as_str()),
                                    has_tool_use,
                                    response_obj
                                        .pointer("/incomplete_details/reason")
                                        .and_then(|r| r.as_str()),
                                );

                                if !read_arguments_by_index.is_empty() {
                                    let mut buffered_indices: Vec<u32> = read_arguments_by_index
                                        .keys()
                                        .copied()
                                        .collect();
                                    buffered_indices.sort_unstable();
                                    for index in buffered_indices {
                                        if !open_indices.contains(&index) {
                                            continue;
                                        }
                                        if !tool_name_by_index
                                            .get(&index)
                                            .is_some_and(|name| name == "Read")
                                        {
                                            continue;
                                        }
                                        let arguments = read_arguments_by_index
                                            .remove(&index)
                                            .unwrap_or_default();
                                        let sanitized = sanitize_tool_arguments_json("Read", &arguments);
                                        let delta_event = json!({
                                            "type": "content_block_delta",
                                            "index": index,
                                            "delta": {
                                                "type": "input_json_delta",
                                                "partial_json": sanitized
                                            }
                                        });
                                        let delta_sse = format!("event: content_block_delta\ndata: {}\n\n", serde_json::to_string(&delta_event).unwrap_or_default());
                                        yield Ok(Bytes::from(delta_sse));
                                    }
                                }

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
                                        .map(|u| build_anthropic_usage_from_responses(Some(u)))
                                });
                                let sse = format!("event: message_delta\ndata: {}\n\n", serde_json::to_string(&delta_event).unwrap_or_default());
                                yield Ok(Bytes::from(sse));

                                let stop_event = json!({"type": "message_stop"});
                                let stop_sse = format!("event: message_stop\ndata: {}\n\n", serde_json::to_string(&stop_event).unwrap_or_default());
                                stream_completion.record_success();
                                yield Ok(Bytes::from(stop_sse));
                                return;
                            }
                            "response.output_text.done" | "response.output_item.done" | "response.in_progress" => {}
                            _ => {}
                        }
                    }
                }
                Err(error) => {
                    stream_completion.record_error(error.to_string());
                    let error_event = json!({
                        "type": "error",
                        "error": {
                            "type": "stream_error",
                            "message": format!("Stream error: {error}")
                        }
                    });
                    let sse = format!("event: error\ndata: {}\n\n", serde_json::to_string(&error_event).unwrap_or_default());
                    yield Ok(Bytes::from(sse));
                    return;
                }
            }
        }

        stream_completion.record_success();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{stream, StreamExt};

    async fn collect_converted_events(input: &'static str) -> Vec<Value> {
        let upstream = stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(input))]);
        let converted =
            create_anthropic_sse_stream_from_responses(upstream, StreamCompletion::default());
        let merged = converted
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|chunk| String::from_utf8_lossy(chunk.unwrap().as_ref()).to_string())
            .collect::<String>();

        merged
            .split("\n\n")
            .filter_map(|block| {
                block.lines().find_map(|line| {
                    line.strip_prefix("data: ")
                        .and_then(|data| serde_json::from_str(data).ok())
                })
            })
            .collect()
    }

    #[tokio::test]
    async fn streaming_responses_buffers_and_sanitizes_read_tool_arguments() {
        let input = concat!(
            "event: response.created\ndata: {\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-4.1-mini\"}}\n\n",
            "event: response.output_item.added\ndata: {\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_read\",\"name\":\"Read\"}}\n\n",
            "event: response.function_call_arguments.delta\ndata: {\"item_id\":\"fc_1\",\"delta\":\"{\\\"file_path\\\":\"}\n\n",
            "event: response.function_call_arguments.delta\ndata: {\"item_id\":\"fc_1\",\"delta\":\"\\\"/tmp/example.txt\\\",\\\"pages\\\":\\\"\\\"}\"}\n\n",
            "event: response.function_call_arguments.done\ndata: {\"item_id\":\"fc_1\"}\n\n",
            "event: response.completed\ndata: {\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":2,\"output_tokens\":1}}}\n\n"
        );

        let events = collect_converted_events(input).await;
        let partial_json = events
            .iter()
            .find_map(|event| {
                (event["type"] == "content_block_delta")
                    .then(|| event["delta"]["partial_json"].as_str())
                    .flatten()
            })
            .expect("sanitized input_json_delta");

        assert_eq!(
            serde_json::from_str::<Value>(partial_json).expect("valid JSON"),
            json!({"file_path": "/tmp/example.txt"})
        );
    }

    #[tokio::test]
    async fn streaming_responses_preserves_non_read_tool_argument_deltas() {
        let input = concat!(
            "event: response.created\ndata: {\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-4.1-mini\"}}\n\n",
            "event: response.output_item.added\ndata: {\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_other\",\"name\":\"OtherTool\"}}\n\n",
            "event: response.function_call_arguments.delta\ndata: {\"item_id\":\"fc_1\",\"delta\":\"{\\\"pages\\\":\\\"\\\"}\"}\n\n",
            "event: response.function_call_arguments.done\ndata: {\"item_id\":\"fc_1\"}\n\n",
            "event: response.completed\ndata: {\"response\":{\"status\":\"completed\"}}\n\n"
        );

        let events = collect_converted_events(input).await;
        let partial_json = events
            .iter()
            .find_map(|event| {
                (event["type"] == "content_block_delta")
                    .then(|| event["delta"]["partial_json"].as_str())
                    .flatten()
            })
            .expect("forwarded input_json_delta");

        assert_eq!(partial_json, "{\"pages\":\"\"}");
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
}
