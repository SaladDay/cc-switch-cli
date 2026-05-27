use bytes::Bytes;
use futures::stream::{Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

use crate::proxy::response::StreamCompletion;
use crate::proxy::sse::{append_utf8_safe, strip_sse_field, take_sse_block};

use super::transform_responses::{
    build_anthropic_usage_from_responses, map_responses_stop_reason,
    sanitize_anthropic_tool_use_input_json,
};

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
        let mut tool_args_by_index: HashMap<u32, String> = HashMap::new();
        let mut last_tool_index: Option<u32> = None;

        tokio::pin!(stream);

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    append_utf8_safe(&mut buffer, &mut utf8_remainder, &bytes);

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
                                        tool_name_by_index.insert(index, name.to_string());
                                        last_tool_index = Some(index);

                                        if open_indices.contains(&index) {
                                            continue;
                                        }

                                        tool_args_by_index.insert(index, String::new());

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

                                    tool_args_by_index.entry(index).or_default().push_str(delta);
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
                                                    .cloned()
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
                                        let accumulated = tool_args_by_index
                                            .get(&index)
                                            .map(String::as_str)
                                            .unwrap_or("");
                                        if accumulated.is_empty() {
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
                            "response.reasoning.delta" => {
                                if let Some(delta) = data
                                    .get("delta")
                                    .or_else(|| data.get("text"))
                                    .and_then(|d| d.as_str())
                                {
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
                            "response.reasoning.done" => {
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
                            "response.completed" => {
                                let response_obj = response_object_from_event(&data);
                                let stop_reason = map_responses_stop_reason(
                                    response_obj.get("status").and_then(|s| s.as_str()),
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
                                let sse = format!("event: message_delta\ndata: {}\n\n", serde_json::to_string(&delta_event).unwrap_or_default());
                                yield Ok(Bytes::from(sse));

                                let stop_event = json!({"type": "message_stop"});
                                let stop_sse = format!("event: message_stop\ndata: {}\n\n", serde_json::to_string(&stop_event).unwrap_or_default());
                                stream_completion.record_success();
                                yield Ok(Bytes::from(stop_sse));
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
                            "response.output_item.done" | "response.in_progress" => {}
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
}
