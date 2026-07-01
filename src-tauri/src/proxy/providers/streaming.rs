use bytes::Bytes;
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};

use crate::proxy::response::StreamCompletion;

#[derive(Debug, Deserialize)]
struct OpenAIStreamChunk {
    id: String,
    model: String,
    choices: Vec<StreamChoice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: Delta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default, alias = "reasoning_content")]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<DeltaToolCall>>,
    #[serde(default)]
    function_call: Option<DeltaFunction>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeltaToolCall {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(rename = "type", default)]
    call_type: Option<String>,
    #[serde(default)]
    function: Option<DeltaFunction>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeltaFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    #[serde(default)]
    prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: u32,
}

#[derive(Debug, Clone)]
struct ToolBlockState {
    anthropic_index: u32,
    id: String,
    name: String,
    started: bool,
    pending_args: String,
}

pub fn create_anthropic_sse_stream(
    stream: impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
    stream_completion: StreamCompletion,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    async_stream::stream! {
        let mut buffer = String::new();
        let mut message_id = None;
        let mut current_model = None;
        let mut next_content_index: u32 = 0;
        let mut has_sent_message_start = false;
        let mut current_non_tool_block_type: Option<&'static str> = None;
        let mut current_non_tool_block_index: Option<u32> = None;
        let mut tool_blocks_by_index: HashMap<usize, ToolBlockState> = HashMap::new();
        let mut open_tool_block_indices: HashSet<u32> = HashSet::new();
        let mut legacy_function_name: Option<String> = None;
        let mut legacy_function_block_index: Option<u32> = None;
        // Final usage and stop reason are deferred: OpenAI-compatible upstreams emit
        // them in separate trailing chunks (the usage chunk has empty `choices`), so
        // we cache them here and flush a single message_delta at [DONE]/stream end.
        let mut latest_usage: Option<Usage> = None;
        let mut pending_stop_reason: Option<String> = None;

        tokio::pin!(stream);

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));

                    while let Some(pos) = buffer.find("\n\n") {
                        let line = buffer[..pos].to_string();
                        buffer = buffer[pos + 2..].to_string();

                        if line.trim().is_empty() {
                            continue;
                        }

                        for raw_line in line.lines() {
                            let Some(data) = raw_line.strip_prefix("data:").map(str::trim_start) else {
                                continue;
                            };

                            if data.trim() == "[DONE]" {
                                if has_sent_message_start && pending_stop_reason.is_some() {
                                    let event = build_message_delta_event(
                                        pending_stop_reason.as_deref(),
                                        latest_usage.as_ref(),
                                    );
                                    let sse_data = format!(
                                        "event: message_delta\ndata: {}\n\n",
                                        serde_json::to_string(&event).unwrap_or_default()
                                    );
                                    yield Ok(Bytes::from(sse_data));
                                }
                                let event = json!({"type": "message_stop"});
                                let sse_data = format!(
                                    "event: message_stop\ndata: {}\n\n",
                                    serde_json::to_string(&event).unwrap_or_default()
                                );
                                stream_completion.record_success();
                                yield Ok(Bytes::from(sse_data));
                                return;
                            }

                            let Ok(chunk) = serde_json::from_str::<OpenAIStreamChunk>(data) else {
                                continue;
                            };

                            if message_id.is_none() {
                                message_id = Some(chunk.id.clone());
                            }
                            if current_model.is_none() {
                                current_model = Some(chunk.model.clone());
                            }

                            // Cache usage from any chunk (incl. the trailing
                            // `choices: []` usage chunk that include_usage produces).
                            // Must happen before the choices.first() guard below, which
                            // otherwise drops that chunk and zeroes token/cost/cache
                            // accounting. Mirrors streaming_codex_chat's latest_usage.
                            if let Some(usage) = &chunk.usage {
                                latest_usage = Some(usage.clone());
                            }

                            let Some(choice) = chunk.choices.first() else {
                                continue;
                            };

                            if !has_sent_message_start {
                                let mut start_usage = json!({
                                    "input_tokens": 0,
                                    "output_tokens": 0
                                });
                                if let Some(usage) = &chunk.usage {
                                    start_usage["input_tokens"] = json!(usage.prompt_tokens);
                                    if let Some(cached) = extract_cache_read_tokens(usage) {
                                        start_usage["cache_read_input_tokens"] = json!(cached);
                                    }
                                    if let Some(created) = usage.cache_creation_input_tokens {
                                        start_usage["cache_creation_input_tokens"] = json!(created);
                                    }
                                }

                                let event = json!({
                                    "type": "message_start",
                                    "message": {
                                        "id": message_id.clone().unwrap_or_default(),
                                        "type": "message",
                                        "role": "assistant",
                                        // Anthropic SSE spec requires these fields in the
                                        // message snapshot; the official SDK's stream
                                        // accumulator does `snapshot.content.push(...)` and
                                        // crashes ("Cannot read properties of undefined")
                                        // when `content` is missing, forcing clients into
                                        // non-streaming fallback (or silent stream loss).
                                        "content": [],
                                        "stop_reason": serde_json::Value::Null,
                                        "stop_sequence": serde_json::Value::Null,
                                        "model": current_model.clone().unwrap_or_default(),
                                        "usage": start_usage
                                    }
                                });
                                let sse_data = format!(
                                    "event: message_start\ndata: {}\n\n",
                                    serde_json::to_string(&event).unwrap_or_default()
                                );
                                yield Ok(Bytes::from(sse_data));
                                has_sent_message_start = true;
                            }

                            if let Some(reasoning) = &choice.delta.reasoning {
                                if current_non_tool_block_type != Some("thinking") {
                                    if let Some(index) = current_non_tool_block_index.take() {
                                        let event = json!({
                                            "type": "content_block_stop",
                                            "index": index
                                        });
                                        let sse_data = format!(
                                            "event: content_block_stop\ndata: {}\n\n",
                                            serde_json::to_string(&event).unwrap_or_default()
                                        );
                                        yield Ok(Bytes::from(sse_data));
                                    }
                                    let index = next_content_index;
                                    next_content_index += 1;
                                    let event = json!({
                                        "type": "content_block_start",
                                        "index": index,
                                        "content_block": {
                                            "type": "thinking",
                                            "thinking": ""
                                        }
                                    });
                                    let sse_data = format!(
                                        "event: content_block_start\ndata: {}\n\n",
                                        serde_json::to_string(&event).unwrap_or_default()
                                    );
                                    yield Ok(Bytes::from(sse_data));
                                    current_non_tool_block_type = Some("thinking");
                                    current_non_tool_block_index = Some(index);
                                }

                                if let Some(index) = current_non_tool_block_index {
                                    let event = json!({
                                        "type": "content_block_delta",
                                        "index": index,
                                        "delta": {
                                            "type": "thinking_delta",
                                            "thinking": reasoning
                                        }
                                    });
                                    let sse_data = format!(
                                        "event: content_block_delta\ndata: {}\n\n",
                                        serde_json::to_string(&event).unwrap_or_default()
                                    );
                                    yield Ok(Bytes::from(sse_data));
                                }
                            }

                            if let Some(content) = &choice.delta.content {
                                if !content.is_empty() {
                                    if current_non_tool_block_type != Some("text") {
                                        if let Some(index) = current_non_tool_block_index.take() {
                                            let event = json!({
                                                "type": "content_block_stop",
                                                "index": index
                                            });
                                            let sse_data = format!(
                                                "event: content_block_stop\ndata: {}\n\n",
                                                serde_json::to_string(&event).unwrap_or_default()
                                            );
                                            yield Ok(Bytes::from(sse_data));
                                        }

                                        let index = next_content_index;
                                        next_content_index += 1;
                                        let event = json!({
                                            "type": "content_block_start",
                                            "index": index,
                                            "content_block": {
                                                "type": "text",
                                                "text": ""
                                            }
                                        });
                                        let sse_data = format!(
                                            "event: content_block_start\ndata: {}\n\n",
                                            serde_json::to_string(&event).unwrap_or_default()
                                        );
                                        yield Ok(Bytes::from(sse_data));
                                        current_non_tool_block_type = Some("text");
                                        current_non_tool_block_index = Some(index);
                                    }

                                    if let Some(index) = current_non_tool_block_index {
                                        let event = json!({
                                            "type": "content_block_delta",
                                            "index": index,
                                            "delta": {
                                                "type": "text_delta",
                                                "text": content
                                            }
                                        });
                                        let sse_data = format!(
                                            "event: content_block_delta\ndata: {}\n\n",
                                            serde_json::to_string(&event).unwrap_or_default()
                                        );
                                        yield Ok(Bytes::from(sse_data));
                                    }
                                }
                            }

                            if let Some(tool_calls) = &choice.delta.tool_calls {
                                if !tool_calls.is_empty() {
                                    if let Some(index) = current_non_tool_block_index.take() {
                                        let event = json!({
                                            "type": "content_block_stop",
                                            "index": index
                                        });
                                        let sse_data = format!(
                                            "event: content_block_stop\ndata: {}\n\n",
                                            serde_json::to_string(&event).unwrap_or_default()
                                        );
                                        yield Ok(Bytes::from(sse_data));
                                    }
                                    current_non_tool_block_type = None;

                                    for tool_call in tool_calls {
                                        let (
                                            anthropic_index,
                                            id,
                                            name,
                                            should_start,
                                            pending_after_start,
                                            immediate_delta,
                                        ) = {
                                            let state = tool_blocks_by_index
                                                .entry(tool_call.index)
                                                .or_insert_with(|| {
                                                    let index = next_content_index;
                                                    next_content_index += 1;
                                                    ToolBlockState {
                                                        anthropic_index: index,
                                                        id: String::new(),
                                                        name: String::new(),
                                                        started: false,
                                                        pending_args: String::new(),
                                                    }
                                                });

                                            if let Some(id) = &tool_call.id {
                                                state.id = id.clone();
                                            }
                                            if let Some(function) = &tool_call.function {
                                                if let Some(name) = &function.name {
                                                    state.name = name.clone();
                                                }
                                            }

                                            let should_start = !state.started
                                                && !state.id.is_empty()
                                                && !state.name.is_empty();
                                            if should_start {
                                                state.started = true;
                                            }
                                            let pending_after_start = if should_start
                                                && !state.pending_args.is_empty()
                                            {
                                                Some(std::mem::take(&mut state.pending_args))
                                            } else {
                                                None
                                            };
                                            let args_delta = tool_call
                                                .function
                                                .as_ref()
                                                .and_then(|f| f.arguments.clone());
                                            let immediate_delta = if let Some(args) = args_delta {
                                                if state.started {
                                                    Some(args)
                                                } else {
                                                    state.pending_args.push_str(&args);
                                                    None
                                                }
                                            } else {
                                                None
                                            };

                                            (
                                                state.anthropic_index,
                                                state.id.clone(),
                                                state.name.clone(),
                                                should_start,
                                                pending_after_start,
                                                immediate_delta,
                                            )
                                        };

                                        if should_start {
                                            let event = json!({
                                                "type": "content_block_start",
                                                "index": anthropic_index,
                                                "content_block": {
                                                    "type": "tool_use",
                                                    "id": id,
                                                    "name": name,
                                                    "input": {}
                                                }
                                            });
                                            let sse_data = format!(
                                                "event: content_block_start\ndata: {}\n\n",
                                                serde_json::to_string(&event).unwrap_or_default()
                                            );
                                            yield Ok(Bytes::from(sse_data));
                                            open_tool_block_indices.insert(anthropic_index);
                                        }

                                        if let Some(args) = pending_after_start {
                                            let event = json!({
                                                "type": "content_block_delta",
                                                "index": anthropic_index,
                                                "delta": {
                                                    "type": "input_json_delta",
                                                    "partial_json": args
                                                }
                                            });
                                            let sse_data = format!(
                                                "event: content_block_delta\ndata: {}\n\n",
                                                serde_json::to_string(&event).unwrap_or_default()
                                            );
                                            yield Ok(Bytes::from(sse_data));
                                        }

                                        if let Some(args) = immediate_delta {
                                            let event = json!({
                                                "type": "content_block_delta",
                                                "index": anthropic_index,
                                                "delta": {
                                                    "type": "input_json_delta",
                                                    "partial_json": args
                                                }
                                            });
                                            let sse_data = format!(
                                                "event: content_block_delta\ndata: {}\n\n",
                                                serde_json::to_string(&event).unwrap_or_default()
                                            );
                                            yield Ok(Bytes::from(sse_data));
                                        }
                                    }
                                }
                            }

                            if let Some(function_call) = &choice.delta.function_call {
                                if let Some(name) = &function_call.name {
                                    legacy_function_name = Some(name.clone());
                                }

                                if function_call.name.is_some() || function_call.arguments.is_some() {
                                    if let Some(index) = current_non_tool_block_index.take() {
                                        let event = json!({
                                            "type": "content_block_stop",
                                            "index": index
                                        });
                                        let sse_data = format!(
                                            "event: content_block_stop\ndata: {}\n\n",
                                            serde_json::to_string(&event).unwrap_or_default()
                                        );
                                        yield Ok(Bytes::from(sse_data));
                                    }
                                    current_non_tool_block_type = None;

                                    if legacy_function_block_index.is_none() {
                                        let index = next_content_index;
                                        next_content_index += 1;
                                        legacy_function_block_index = Some(index);
                                        let event = json!({
                                            "type": "content_block_start",
                                            "index": index,
                                            "content_block": {
                                                "type": "tool_use",
                                                "id": "",
                                                "name": legacy_function_name.clone().unwrap_or_default(),
                                                "input": {}
                                            }
                                        });
                                        let sse_data = format!(
                                            "event: content_block_start\ndata: {}\n\n",
                                            serde_json::to_string(&event).unwrap_or_default()
                                        );
                                        yield Ok(Bytes::from(sse_data));
                                    }

                                    if let Some(arguments) = &function_call.arguments {
                                        if let Some(index) = legacy_function_block_index {
                                            let event = json!({
                                                "type": "content_block_delta",
                                                "index": index,
                                                "delta": {
                                                    "type": "input_json_delta",
                                                    "partial_json": arguments
                                                }
                                            });
                                            let sse_data = format!(
                                                "event: content_block_delta\ndata: {}\n\n",
                                                serde_json::to_string(&event).unwrap_or_default()
                                            );
                                            yield Ok(Bytes::from(sse_data));
                                        }
                                    }
                                }
                            }

                            if let Some(finish_reason) = &choice.finish_reason {
                                if let Some(index) = current_non_tool_block_index.take() {
                                    let event = json!({
                                        "type": "content_block_stop",
                                        "index": index
                                    });
                                    let sse_data = format!(
                                        "event: content_block_stop\ndata: {}\n\n",
                                        serde_json::to_string(&event).unwrap_or_default()
                                    );
                                    yield Ok(Bytes::from(sse_data));
                                }
                                current_non_tool_block_type = None;

                                let mut late_tool_starts: Vec<(u32, String, String, String)> =
                                    Vec::new();
                                for (tool_idx, state) in tool_blocks_by_index.iter_mut() {
                                    if state.started {
                                        continue;
                                    }
                                    let has_payload = !state.pending_args.is_empty()
                                        || !state.id.is_empty()
                                        || !state.name.is_empty();
                                    if !has_payload {
                                        continue;
                                    }
                                    let fallback_id = if state.id.is_empty() {
                                        format!("tool_call_{tool_idx}")
                                    } else {
                                        state.id.clone()
                                    };
                                    let fallback_name = if state.name.is_empty() {
                                        "unknown_tool".to_string()
                                    } else {
                                        state.name.clone()
                                    };
                                    state.started = true;
                                    let pending = std::mem::take(&mut state.pending_args);
                                    late_tool_starts.push((
                                        state.anthropic_index,
                                        fallback_id,
                                        fallback_name,
                                        pending,
                                    ));
                                }
                                late_tool_starts.sort_unstable_by_key(|(index, _, _, _)| *index);
                                for (index, id, name, pending) in late_tool_starts {
                                    let event = json!({
                                        "type": "content_block_start",
                                        "index": index,
                                        "content_block": {
                                            "type": "tool_use",
                                            "id": id,
                                            "name": name,
                                            "input": {}
                                        }
                                    });
                                    let sse_data = format!(
                                        "event: content_block_start\ndata: {}\n\n",
                                        serde_json::to_string(&event).unwrap_or_default()
                                    );
                                    yield Ok(Bytes::from(sse_data));
                                    open_tool_block_indices.insert(index);
                                    if !pending.is_empty() {
                                        let delta_event = json!({
                                            "type": "content_block_delta",
                                            "index": index,
                                            "delta": {
                                                "type": "input_json_delta",
                                                "partial_json": pending
                                            }
                                        });
                                        let delta_sse = format!(
                                            "event: content_block_delta\ndata: {}\n\n",
                                            serde_json::to_string(&delta_event).unwrap_or_default()
                                        );
                                        yield Ok(Bytes::from(delta_sse));
                                    }
                                }

                                if let Some(index) = legacy_function_block_index.take() {
                                    let event = json!({
                                        "type": "content_block_stop",
                                        "index": index
                                    });
                                    let sse_data = format!(
                                        "event: content_block_stop\ndata: {}\n\n",
                                        serde_json::to_string(&event).unwrap_or_default()
                                    );
                                    yield Ok(Bytes::from(sse_data));
                                }

                                if !open_tool_block_indices.is_empty() {
                                    let mut tool_indices: Vec<u32> =
                                        open_tool_block_indices.iter().copied().collect();
                                    tool_indices.sort_unstable();
                                    for index in tool_indices {
                                        let event = json!({
                                            "type": "content_block_stop",
                                            "index": index
                                        });
                                        let sse_data = format!(
                                            "event: content_block_stop\ndata: {}\n\n",
                                            serde_json::to_string(&event).unwrap_or_default()
                                        );
                                        yield Ok(Bytes::from(sse_data));
                                    }
                                    open_tool_block_indices.clear();
                                }

                                // Defer the message_delta: the final usage often arrives
                                // in a *later* chunk than finish_reason (OpenAI-compatible
                                // streams send `choices:[{finish_reason}]` then a separate
                                // `choices:[]` usage chunk). Emitting now would lock in
                                // output_tokens=0; instead record the stop reason and flush
                                // one message_delta with the cached usage at stream end.
                                pending_stop_reason = Some(finish_reason.clone());
                            }
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
                    let sse_data = format!(
                        "event: error\ndata: {}\n\n",
                        serde_json::to_string(&error_event).unwrap_or_default()
                    );
                    yield Ok(Bytes::from(sse_data));
                    return;
                }
            }
        }

        // Flush the deferred message_delta if the upstream closed without a [DONE]
        // marker, so the final usage still reaches the client.
        if has_sent_message_start && pending_stop_reason.is_some() {
            let event = build_message_delta_event(
                pending_stop_reason.as_deref(),
                latest_usage.as_ref(),
            );
            let sse_data = format!(
                "event: message_delta\ndata: {}\n\n",
                serde_json::to_string(&event).unwrap_or_default()
            );
            yield Ok(Bytes::from(sse_data));
        }

        stream_completion.record_success();
    }
}

fn build_message_delta_event(
    stop_reason: Option<&str>,
    usage: Option<&Usage>,
) -> serde_json::Value {
    let usage_json = if let Some(usage) = usage {
        let mut usage_json = json!({
            "input_tokens": usage.prompt_tokens,
            "output_tokens": usage.completion_tokens
        });
        if let Some(cached) = extract_cache_read_tokens(usage) {
            usage_json["cache_read_input_tokens"] = json!(cached);
        }
        if let Some(created) = usage.cache_creation_input_tokens {
            usage_json["cache_creation_input_tokens"] = json!(created);
        }
        usage_json
    } else {
        json!({ "output_tokens": 0 })
    };
    json!({
        "type": "message_delta",
        "delta": {
            "stop_reason": map_stop_reason(stop_reason),
            "stop_sequence": null
        },
        "usage": usage_json
    })
}

fn map_stop_reason(finish_reason: Option<&str>) -> Option<String> {
    finish_reason.map(|reason| {
        match reason {
            "tool_calls" | "function_call" => "tool_use",
            "stop" => "end_turn",
            "length" => "max_tokens",
            "content_filter" => "end_turn",
            other => {
                log::warn!("[Claude/OpenAI] Unknown finish_reason in streaming: {other}");
                "end_turn"
            }
        }
        .to_string()
    })
}

fn extract_cache_read_tokens(usage: &Usage) -> Option<u32> {
    if let Some(value) = usage.cache_read_input_tokens {
        return Some(value);
    }

    usage
        .prompt_tokens_details
        .as_ref()
        .map(|details| details.cached_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{stream, StreamExt};
    use serde_json::Value;
    use std::collections::HashMap;

    async fn collect_events(input: &str) -> Vec<Value> {
        let upstream = stream::iter(vec![Ok(Bytes::from(input.as_bytes().to_vec()))]);
        let converted = create_anthropic_sse_stream(upstream, StreamCompletion::default());
        let chunks: Vec<_> = converted.collect().await;

        chunks
            .into_iter()
            .map(|chunk| String::from_utf8_lossy(chunk.unwrap().as_ref()).to_string())
            .collect::<String>()
            .split("\n\n")
            .filter_map(|block| {
                let data = block.lines().find_map(|line| line.strip_prefix("data: "))?;
                serde_json::from_str::<Value>(data).ok()
            })
            .collect()
    }

    async fn collect_event_names_and_payload_types(input: &str) -> Vec<(String, String)> {
        let upstream = stream::iter(vec![Ok(Bytes::from(input.as_bytes().to_vec()))]);
        let converted = create_anthropic_sse_stream(upstream, StreamCompletion::default());
        let chunks: Vec<_> = converted.collect().await;

        chunks
            .into_iter()
            .map(|chunk| String::from_utf8_lossy(chunk.unwrap().as_ref()).to_string())
            .collect::<String>()
            .split("\n\n")
            .filter_map(|block| {
                let event = block
                    .lines()
                    .find_map(|line| line.strip_prefix("event: "))?
                    .to_string();
                let data = block.lines().find_map(|line| line.strip_prefix("data: "))?;
                let payload = serde_json::from_str::<Value>(data).ok()?;
                let payload_type = payload.get("type")?.as_str()?.to_string();
                Some((event, payload_type))
            })
            .collect()
    }

    #[tokio::test]
    async fn done_marker_ends_stream_without_waiting_for_upstream_eof() {
        let input = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n"
        );
        let upstream = stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(input))])
            .chain(stream::pending::<Result<Bytes, std::io::Error>>());
        let completion = StreamCompletion::default();
        let converted = create_anthropic_sse_stream(upstream, completion.clone());

        let chunks: Vec<_> =
            tokio::time::timeout(std::time::Duration::from_millis(100), converted.collect())
                .await
                .expect("stream should end at [DONE]");
        let merged = chunks
            .into_iter()
            .map(|chunk| String::from_utf8_lossy(chunk.unwrap().as_ref()).to_string())
            .collect::<String>();

        assert!(merged.contains("event: message_stop"));
        assert_eq!(completion.outcome(), Some(Ok(())));
    }

    #[test]
    fn map_stop_reason_covers_existing_and_parity_values() {
        assert_eq!(
            map_stop_reason(Some("tool_calls")),
            Some("tool_use".to_string())
        );
        assert_eq!(
            map_stop_reason(Some("function_call")),
            Some("tool_use".to_string())
        );
        assert_eq!(map_stop_reason(Some("stop")), Some("end_turn".to_string()));
        assert_eq!(
            map_stop_reason(Some("length")),
            Some("max_tokens".to_string())
        );
        assert_eq!(
            map_stop_reason(Some("content_filter")),
            Some("end_turn".to_string())
        );
    }

    #[tokio::test]
    async fn streaming_message_usage_includes_cache_token_fields() {
        let input = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{}}],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":0,\"prompt_tokens_details\":{\"cached_tokens\":5},\"cache_creation_input_tokens\":2}}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":7,\"cache_read_input_tokens\":6,\"cache_creation_input_tokens\":3}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input).await;
        let message_start = events
            .iter()
            .find(|event| event["type"] == "message_start")
            .expect("message_start event");
        let message_delta = events
            .iter()
            .find(|event| event["type"] == "message_delta")
            .expect("message_delta event");

        assert_eq!(message_start["message"]["usage"]["input_tokens"], 12);
        assert_eq!(message_start["message"]["usage"]["output_tokens"], 0);
        assert_eq!(
            message_start["message"]["usage"]["cache_read_input_tokens"],
            5
        );
        assert_eq!(
            message_start["message"]["usage"]["cache_creation_input_tokens"],
            2
        );

        assert_eq!(message_delta["usage"]["input_tokens"], 12);
        assert_eq!(message_delta["usage"]["output_tokens"], 7);
        assert_eq!(message_delta["usage"]["cache_read_input_tokens"], 6);
        assert_eq!(message_delta["usage"]["cache_creation_input_tokens"], 3);
    }

    #[tokio::test]
    async fn streaming_usage_preserves_zero_cached_tokens() {
        let input = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{}}],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":0,\"prompt_tokens_details\":{\"cached_tokens\":0}}}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":7}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input).await;
        let message_start = events
            .iter()
            .find(|event| event["type"] == "message_start")
            .expect("message_start event");

        assert_eq!(
            message_start["message"]["usage"]["cache_read_input_tokens"],
            0
        );
    }

    #[tokio::test]
    async fn streaming_usage_from_trailing_empty_choices_chunk_is_mapped() {
        // OpenAI-compatible upstreams (LiteLLM, kimi, MiniMax, ...) send the final
        // usage in a separate `choices: []` chunk *after* the finish_reason chunk
        // when stream_options.include_usage=true. The choices.first() guard used to
        // drop it, leaving message_delta usage at 0, zeroing token/cost accounting.
        let input = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-5.5\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-5.5\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-5.5\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":9,\"total_tokens\":21}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input).await;
        let message_delta = events
            .iter()
            .find(|event| event["type"] == "message_delta")
            .expect("message_delta event");

        assert_eq!(message_delta["delta"]["stop_reason"], "end_turn");
        assert_eq!(message_delta["usage"]["input_tokens"], 12);
        assert_eq!(message_delta["usage"]["output_tokens"], 9);

        // Exactly one message_delta is emitted, and it precedes message_stop.
        let delta_count = events
            .iter()
            .filter(|event| event["type"] == "message_delta")
            .count();
        assert_eq!(delta_count, 1);
        let names: Vec<&str> = events
            .iter()
            .filter_map(|event| event["type"].as_str())
            .collect();
        let delta_pos = names.iter().position(|n| *n == "message_delta");
        let stop_pos = names.iter().position(|n| *n == "message_stop");
        assert!(
            delta_pos < stop_pos,
            "message_delta must precede message_stop"
        );
    }

    #[tokio::test]
    async fn streaming_message_start_is_spec_complete() {
        // message_start must carry content/stop_reason/stop_sequence or the
        // official Anthropic SDK stream accumulator crashes on snapshot.content.
        let input = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input).await;
        let message = &events
            .iter()
            .find(|event| event["type"] == "message_start")
            .expect("message_start event")["message"];

        assert_eq!(message["content"], serde_json::json!([]));
        assert_eq!(message["stop_reason"], serde_json::Value::Null);
        assert_eq!(message["stop_sequence"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn streaming_accepts_deepseek_reasoning_content_alias() {
        let input = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"deepseek-v4-pro\",\"choices\":[{\"delta\":{\"reasoning_content\":\"think\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"deepseek-v4-pro\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input).await;
        let thinking_delta = events
            .iter()
            .find(|event| event["type"] == "content_block_delta")
            .expect("thinking delta should be emitted");

        assert_eq!(thinking_delta["delta"]["type"], "thinking_delta");
        assert_eq!(thinking_delta["delta"]["thinking"], "think");
    }

    #[tokio::test]
    async fn legacy_function_call_stream_emits_tool_use_block_and_argument_delta() {
        let input = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"function_call\":{\"name\":\"get_weather\"}}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"function_call\":{\"arguments\":\"{\\\"location\\\":\\\"Tokyo\\\"}\"}}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{},\"finish_reason\":\"function_call\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input).await;
        let tool_start = events
            .iter()
            .find(|event| {
                event["type"] == "content_block_start"
                    && event["content_block"]["type"] == "tool_use"
            })
            .expect("tool_use block start");
        let tool_delta = events
            .iter()
            .find(|event| {
                event["type"] == "content_block_delta"
                    && event["delta"]["type"] == "input_json_delta"
            })
            .expect("tool_use argument delta");
        let message_delta = events
            .iter()
            .find(|event| event["type"] == "message_delta")
            .expect("message_delta event");

        assert_eq!(tool_start["content_block"]["name"], "get_weather");
        assert_eq!(tool_start["content_block"]["input"], json!({}));
        assert_eq!(
            tool_delta["delta"]["partial_json"],
            "{\"location\":\"Tokyo\"}"
        );
        assert_eq!(message_delta["delta"]["stop_reason"], "tool_use");
    }

    #[tokio::test]
    async fn streaming_tool_calls_route_arguments_by_index() {
        let input = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_0\",\"type\":\"function\",\"function\":{\"name\":\"first_tool\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"second_tool\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"function\":{\"arguments\":\"{\\\"b\\\":2}\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"a\\\":1}\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":8,\"completion_tokens\":4}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input).await;
        let mut tool_index_by_call: HashMap<String, u64> = HashMap::new();
        for event in &events {
            if event["type"] == "content_block_start"
                && event["content_block"]["type"] == "tool_use"
            {
                if let (Some(call_id), Some(index)) = (
                    event.pointer("/content_block/id").and_then(|v| v.as_str()),
                    event.get("index").and_then(|v| v.as_u64()),
                ) {
                    tool_index_by_call.insert(call_id.to_string(), index);
                }
            }
        }

        assert_eq!(tool_index_by_call.len(), 2);
        assert_ne!(
            tool_index_by_call.get("call_0"),
            tool_index_by_call.get("call_1")
        );

        let deltas: Vec<(u64, String)> = events
            .iter()
            .filter(|event| {
                event["type"] == "content_block_delta"
                    && event["delta"]["type"] == "input_json_delta"
            })
            .filter_map(|event| {
                let index = event.get("index").and_then(|v| v.as_u64())?;
                let partial_json = event
                    .pointer("/delta/partial_json")
                    .and_then(|v| v.as_str())?
                    .to_string();
                Some((index, partial_json))
            })
            .collect();

        let second_idx = deltas
            .iter()
            .find_map(|(index, payload)| (payload == "{\"b\":2}").then_some(*index))
            .expect("second tool delta index");
        let first_idx = deltas
            .iter()
            .find_map(|(index, payload)| (payload == "{\"a\":1}").then_some(*index))
            .expect("first tool delta index");

        assert_eq!(second_idx, *tool_index_by_call.get("call_1").unwrap());
        assert_eq!(first_idx, *tool_index_by_call.get("call_0").unwrap());
    }

    #[tokio::test]
    async fn streaming_tool_call_schema_is_valid_when_finish_chunk_omits_usage() {
        let input = concat!(
            "data: {\"id\":\"chatcmpl_tool\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_0\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_tool\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"location\\\":\\\"Tokyo\\\"}\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_tool\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input).await;
        let tool_start = events
            .iter()
            .find(|event| {
                event["type"] == "content_block_start"
                    && event["content_block"]["type"] == "tool_use"
            })
            .expect("tool_use block start");
        let message_delta = events
            .iter()
            .find(|event| event["type"] == "message_delta")
            .expect("message_delta event");

        assert_eq!(tool_start["content_block"]["id"], "call_0");
        assert_eq!(tool_start["content_block"]["name"], "get_weather");
        assert_eq!(tool_start["content_block"]["input"], json!({}));
        assert_eq!(message_delta["delta"]["stop_reason"], "tool_use");
        assert!(message_delta["usage"].is_object());
        assert_eq!(message_delta["usage"]["output_tokens"], 0);
        assert!(events.iter().any(|event| event["type"] == "message_stop"));
    }

    #[tokio::test]
    async fn streaming_tool_calls_delay_start_until_id_and_name_ready() {
        let input = concat!(
            "data: {\"id\":\"chatcmpl_2\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"a\\\":\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_2\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_0\",\"type\":\"function\",\"function\":{\"name\":\"first_tool\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_2\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"1}\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_2\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":6,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input).await;
        let starts: Vec<&Value> = events
            .iter()
            .filter(|event| {
                event["type"] == "content_block_start"
                    && event["content_block"]["type"] == "tool_use"
            })
            .collect();

        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0]["content_block"]["id"], "call_0");
        assert_eq!(starts[0]["content_block"]["name"], "first_tool");

        let deltas: Vec<&str> = events
            .iter()
            .filter(|event| {
                event["type"] == "content_block_delta"
                    && event["delta"]["type"] == "input_json_delta"
            })
            .filter_map(|event| {
                event
                    .pointer("/delta/partial_json")
                    .and_then(|v| v.as_str())
            })
            .collect();
        assert!(deltas.contains(&"{\"a\":"));
        assert!(deltas.contains(&"1}"));
    }

    #[tokio::test]
    async fn streaming_tool_calls_finish_reason_flushes_pending_and_closes_in_order() {
        let input = concat!(
            "data: {\"id\":\"chatcmpl_3\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"a\\\":1}\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_3\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input).await;
        let tool_start_pos = events
            .iter()
            .position(|event| {
                event["type"] == "content_block_start"
                    && event["content_block"]["type"] == "tool_use"
            })
            .expect("tool_use start emitted at finish_reason");
        let tool_delta_pos = events
            .iter()
            .position(|event| {
                event["type"] == "content_block_delta"
                    && event["delta"]["type"] == "input_json_delta"
            })
            .expect("pending args flushed");
        let tool_stop_pos = events
            .iter()
            .position(|event| event["type"] == "content_block_stop")
            .expect("tool block closed");
        let message_delta_pos = events
            .iter()
            .position(|event| event["type"] == "message_delta")
            .expect("message_delta emitted");

        assert_eq!(events[tool_start_pos]["content_block"]["id"], "tool_call_0");
        assert_eq!(events[tool_start_pos]["content_block"]["input"], json!({}));
        assert_eq!(
            events[tool_start_pos]["content_block"]["name"],
            "unknown_tool"
        );
        assert_eq!(events[tool_delta_pos]["delta"]["partial_json"], "{\"a\":1}");
        assert!(tool_start_pos < tool_delta_pos);
        assert!(tool_delta_pos < tool_stop_pos);
        assert!(tool_stop_pos < message_delta_pos);
    }

    #[tokio::test]
    async fn empty_content_delta_does_not_open_text_block() {
        let input = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":0}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input).await;
        let text_block_starts: Vec<_> = events
            .iter()
            .filter(|event| {
                event["type"] == "content_block_start" && event["content_block"]["type"] == "text"
            })
            .collect();

        assert!(
            text_block_starts.is_empty(),
            "empty content deltas should not open text blocks"
        );
    }

    #[tokio::test]
    async fn content_block_sse_event_names_match_payload_types() {
        let input = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"reasoning\":\"thinking\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_0\",\"type\":\"function\",\"function\":{\"name\":\"lookup\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_event_names_and_payload_types(input).await;
        for (event_name, payload_type) in events
            .iter()
            .filter(|(_, payload_type)| payload_type.starts_with("content_block_"))
        {
            assert_eq!(event_name, payload_type);
        }
    }

    #[tokio::test]
    async fn empty_tool_calls_delta_does_not_close_text_block() {
        let input = concat!(
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"tool_calls\":[]}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input).await;
        let text_block_starts = events
            .iter()
            .filter(|event| {
                event["type"] == "content_block_start" && event["content_block"]["type"] == "text"
            })
            .count();
        let text_block_stops = events
            .iter()
            .filter(|event| event["type"] == "content_block_stop")
            .count();

        assert_eq!(text_block_starts, 1);
        assert_eq!(text_block_stops, 1);
    }
}
