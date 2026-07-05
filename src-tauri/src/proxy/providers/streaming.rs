use bytes::Bytes;
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};

use crate::proxy::response::StreamCompletion;
use crate::proxy::sse::{append_utf8_safe, strip_sse_field, take_sse_block};

#[derive(Debug, Deserialize)]
struct OpenAIStreamChunk {
    // `id`/`model`/`choices` default so that minimal usage-only tail chunks such as
    // `{"choices":[],"usage":{...}}` (which some OpenAI-compatible upstreams emit with
    // stream_options.include_usage) still deserialize instead of being dropped (#323).
    #[serde(default)]
    id: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
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
        let mut utf8_remainder: Vec<u8> = Vec::new();
        let mut message_id = None;
        let mut current_model = None;
        let mut next_content_index: u32 = 0;
        let mut has_sent_message_start = false;
        // Most recent usage seen on any chunk (including the trailing usage-only chunk
        // that OpenAI-compatible upstreams emit with an empty `choices` array). The
        // message_delta is deferred until [DONE]/EOF so this can be folded in; emitting
        // it eagerly on the finish_reason chunk would lock in that chunk's usage, which
        // is usually absent → all-zero tokens (see #323).
        let mut latest_usage: Option<serde_json::Value> = None;
        let mut pending_message_delta: Option<serde_json::Value> = None;
        let mut current_non_tool_block_type: Option<&'static str> = None;
        let mut current_non_tool_block_index: Option<u32> = None;
        let mut tool_blocks_by_index: HashMap<usize, ToolBlockState> = HashMap::new();
        let mut open_tool_block_indices: HashSet<u32> = HashSet::new();
        let mut legacy_function_name: Option<String> = None;
        let mut legacy_function_block_index: Option<u32> = None;

        tokio::pin!(stream);

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    // CRLF-delimited SSE and multibyte UTF-8 split across chunks must be
                    // handled, matching the sibling streaming converters (#323).
                    append_utf8_safe(&mut buffer, &mut utf8_remainder, &bytes);

                    while let Some(line) = take_sse_block(&mut buffer) {
                        if line.trim().is_empty() {
                            continue;
                        }

                        for raw_line in line.lines() {
                            let Some(data) = strip_sse_field(raw_line, "data") else {
                                continue;
                            };

                            if data.trim() == "[DONE]" {
                                if let Some(stop_reason) = pending_message_delta.take() {
                                    let event = build_message_delta_event(
                                        stop_reason,
                                        latest_usage.clone(),
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

                            if message_id.is_none() && !chunk.id.is_empty() {
                                message_id = Some(chunk.id.clone());
                            }
                            if current_model.is_none() && !chunk.model.is_empty() {
                                current_model = Some(chunk.model.clone());
                            }

                            // Capture usage before the choices check: OpenAI-compatible
                            // upstreams send the final usage in a trailing chunk whose
                            // `choices` is empty, which the skip below would drop (#323).
                            if let Some(usage) = chunk.usage.as_ref() {
                                latest_usage = Some(build_stream_usage_json(usage));
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
                                    // Subtract cache to report fresh input, mirroring
                                    // build_stream_usage_json; output_tokens stays 0 at start.
                                    let cached = extract_cache_read_tokens(usage).unwrap_or(0);
                                    let cache_creation =
                                        usage.cache_creation_input_tokens.unwrap_or(0);
                                    start_usage["input_tokens"] = json!(usage
                                        .prompt_tokens
                                        .saturating_sub(cached)
                                        .saturating_sub(cache_creation));
                                    if cached > 0 {
                                        start_usage["cache_read_input_tokens"] = json!(cached);
                                    }
                                    if cache_creation > 0 {
                                        start_usage["cache_creation_input_tokens"] =
                                            json!(cache_creation);
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

                                // Defer the message_delta: the final usage frequently
                                // arrives in a later usage-only chunk. It is flushed with
                                // the freshest `latest_usage` at [DONE]/EOF (#323).
                                // Only the FIRST terminal reason is recorded — some
                                // upstreams (e.g. OpenRouter kimi) send a second
                                // finish_reason chunk, which must not downgrade an earlier
                                // `tool_use` to `end_turn` (mirrors upstream's
                                // has_emitted_message_delta guard). Usage keeps updating
                                // via `latest_usage` regardless.
                                if pending_message_delta.is_none() {
                                    pending_message_delta =
                                        Some(json!(map_stop_reason(Some(finish_reason))));
                                }
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

        // The upstream ended without a `[DONE]` marker. Flush any deferred message_delta
        // (with the final usage) followed by message_stop so the client still gets a
        // spec-complete terminal sequence instead of a truncated stream.
        if let Some(stop_reason) = pending_message_delta.take() {
            let event = build_message_delta_event(stop_reason, latest_usage.clone());
            let sse_data = format!(
                "event: message_delta\ndata: {}\n\n",
                serde_json::to_string(&event).unwrap_or_default()
            );
            yield Ok(Bytes::from(sse_data));

            let event = json!({"type": "message_stop"});
            let sse_data = format!(
                "event: message_stop\ndata: {}\n\n",
                serde_json::to_string(&event).unwrap_or_default()
            );
            yield Ok(Bytes::from(sse_data));
        }

        stream_completion.record_success();
    }
}

/// Build the Anthropic `usage` object for a streamed OpenAI usage payload.
///
/// OpenAI `prompt_tokens` is inclusive of cache hits, but Anthropic `input_tokens` is
/// fresh input only. This path is billed as app_type="claude" (the cost calculator does
/// NOT subtract cache again), so `cache_read` + `cache_creation` are subtracted here to
/// avoid counting cached tokens both as input and in the cache buckets. The three buckets
/// are mutually exclusive: input + cache_read + cache_creation == prompt_tokens.
fn build_stream_usage_json(usage: &Usage) -> serde_json::Value {
    let cached = extract_cache_read_tokens(usage).unwrap_or(0);
    let cache_creation = usage.cache_creation_input_tokens.unwrap_or(0);
    let input_tokens = usage
        .prompt_tokens
        .saturating_sub(cached)
        .saturating_sub(cache_creation);
    let mut usage_json = json!({
        "input_tokens": input_tokens,
        "output_tokens": usage.completion_tokens
    });
    if cached > 0 {
        usage_json["cache_read_input_tokens"] = json!(cached);
    }
    if cache_creation > 0 {
        usage_json["cache_creation_input_tokens"] = json!(cache_creation);
    }
    usage_json
}

/// Build a `message_delta` event, falling back to a zero-output usage object when the
/// upstream never reported usage (keeps the field a valid object for the Anthropic SDK).
fn build_message_delta_event(
    stop_reason: serde_json::Value,
    usage_json: Option<serde_json::Value>,
) -> serde_json::Value {
    let usage = usage_json.unwrap_or_else(|| json!({ "output_tokens": 0 }));
    json!({
        "type": "message_delta",
        "delta": {
            "stop_reason": stop_reason,
            "stop_sequence": null
        },
        "usage": usage
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

    async fn collect_events_from_byte_chunks(chunks: Vec<Vec<u8>>) -> Vec<Value> {
        let upstream = stream::iter(
            chunks
                .into_iter()
                .map(|chunk| Ok::<_, std::io::Error>(Bytes::from(chunk)))
                .collect::<Vec<_>>(),
        );
        let converted = create_anthropic_sse_stream(upstream, StreamCompletion::default());
        let collected: Vec<_> = converted.collect().await;

        collected
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

    #[tokio::test]
    async fn streaming_parses_crlf_delimited_sse() {
        // Some OpenAI-compatible upstreams delimit SSE blocks with \r\n\r\n. The parser
        // must handle both LF and CRLF, otherwise no events are ever emitted (#323).
        let input = concat!(
            "data: {\"id\":\"c\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\r\n\r\n",
            "data: {\"id\":\"c\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\r\n\r\n",
            "data: [DONE]\r\n\r\n"
        );

        let events = collect_events(input).await;
        let text_delta = events
            .iter()
            .find(|event| {
                event["type"] == "content_block_delta" && event["delta"]["type"] == "text_delta"
            })
            .expect("text delta from CRLF-delimited stream");
        assert_eq!(text_delta["delta"]["text"], "hi");
        assert!(events.iter().any(|event| event["type"] == "message_stop"));
    }

    #[tokio::test]
    async fn streaming_preserves_multibyte_utf8_split_across_chunks() {
        // A multibyte character can straddle a network chunk boundary; the parser must not
        // corrupt it (the old from_utf8_lossy-per-chunk approach would).
        let head =
            "data: {\"id\":\"c\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"";
        let tail = "\"}}]}\n\ndata: [DONE]\n\n";
        let mut bytes = Vec::new();
        bytes.extend_from_slice(head.as_bytes());
        bytes.extend_from_slice("你好".as_bytes());
        bytes.extend_from_slice(tail.as_bytes());

        // Split mid-way through the first multibyte character (你 = 3 bytes).
        let split_at = head.len() + 1;
        let first = bytes[..split_at].to_vec();
        let second = bytes[split_at..].to_vec();

        let events = collect_events_from_byte_chunks(vec![first, second]).await;
        let text: String = events
            .iter()
            .filter(|event| {
                event["type"] == "content_block_delta" && event["delta"]["type"] == "text_delta"
            })
            .filter_map(|event| event["delta"]["text"].as_str())
            .collect();
        assert_eq!(text, "你好");
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

        // input_tokens is fresh input (cache subtracted): 12 - cache_read(5) - creation(2) = 5.
        assert_eq!(message_start["message"]["usage"]["input_tokens"], 5);
        assert_eq!(message_start["message"]["usage"]["output_tokens"], 0);
        assert_eq!(
            message_start["message"]["usage"]["cache_read_input_tokens"],
            5
        );
        assert_eq!(
            message_start["message"]["usage"]["cache_creation_input_tokens"],
            2
        );

        // 12 - cache_read(6) - cache_creation(3) = 3.
        assert_eq!(message_delta["usage"]["input_tokens"], 3);
        assert_eq!(message_delta["usage"]["output_tokens"], 7);
        assert_eq!(message_delta["usage"]["cache_read_input_tokens"], 6);
        assert_eq!(message_delta["usage"]["cache_creation_input_tokens"], 3);
    }

    #[tokio::test]
    async fn streaming_forwards_usage_from_trailing_usage_only_chunk() {
        // Regression for #323: OpenAI-compatible upstreams (e.g. SenseTime) report the
        // final usage in a trailing chunk whose `choices` is empty, arriving after the
        // finish_reason chunk. That usage must be folded into message_delta rather than
        // dropped, otherwise the client sees input_tokens/output_tokens == 0.
        // The trailing usage chunk is intentionally MINIMAL (no id/model) — some
        // upstreams emit it that way, and it must still deserialize and be captured.
        let input = concat!(
            "data: {\"id\":\"msg_1\",\"model\":\"sensenova-6.7-flash-lite\",\"choices\":[{\"delta\":{\"reasoning\":\"thinking...\"}}]}\n\n",
            "data: {\"id\":\"msg_1\",\"model\":\"sensenova-6.7-flash-lite\",\"choices\":[{\"delta\":{\"content\":\"你好！\"}}]}\n\n",
            "data: {\"id\":\"msg_1\",\"model\":\"sensenova-6.7-flash-lite\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":57,\"completion_tokens\":151}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input).await;
        let message_delta = events
            .iter()
            .find(|event| event["type"] == "message_delta")
            .expect("message_delta event");

        assert_eq!(message_delta["delta"]["stop_reason"], "end_turn");
        assert_eq!(message_delta["usage"]["input_tokens"], 57);
        assert_eq!(message_delta["usage"]["output_tokens"], 151);

        // message_delta must be emitted exactly once and before message_stop.
        let delta_count = events
            .iter()
            .filter(|event| event["type"] == "message_delta")
            .count();
        assert_eq!(delta_count, 1);
        let delta_pos = events
            .iter()
            .position(|event| event["type"] == "message_delta")
            .unwrap();
        let stop_pos = events
            .iter()
            .position(|event| event["type"] == "message_stop")
            .expect("message_stop event");
        assert!(delta_pos < stop_pos);
    }

    #[tokio::test]
    async fn streaming_second_finish_reason_does_not_downgrade_stop_reason() {
        // Some upstreams (e.g. OpenRouter kimi) emit a second finish_reason chunk after
        // tool-use. The first terminal reason must win — a later "stop" must not downgrade
        // an earlier "tool_calls" (tool_use), and only one message_delta may be emitted.
        let input = concat!(
            "data: {\"id\":\"c\",\"model\":\"kimi\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_0\",\"type\":\"function\",\"function\":{\"name\":\"f\",\"arguments\":\"{}\"}}]}}]}\n\n",
            "data: {\"id\":\"c\",\"model\":\"kimi\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: {\"id\":\"c\",\"model\":\"kimi\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":3}}\n\n",
            "data: [DONE]\n\n"
        );

        let events = collect_events(input).await;
        let message_deltas: Vec<&Value> = events
            .iter()
            .filter(|event| event["type"] == "message_delta")
            .collect();

        assert_eq!(
            message_deltas.len(),
            1,
            "exactly one message_delta expected"
        );
        assert_eq!(message_deltas[0]["delta"]["stop_reason"], "tool_use");
        assert_eq!(message_deltas[0]["usage"]["input_tokens"], 9);
        assert_eq!(message_deltas[0]["usage"]["output_tokens"], 3);
    }

    #[tokio::test]
    async fn streaming_flushes_message_delta_on_eof_without_done() {
        // If the upstream ends the connection without a [DONE] marker, the deferred
        // message_delta (with usage) and message_stop must still be flushed (#323).
        let input = concat!(
            "data: {\"id\":\"msg_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: {\"id\":\"msg_1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"id\":\"msg_1\",\"model\":\"gpt-4o\",\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":4}}\n\n"
        );

        let events = collect_events(input).await;
        let message_delta = events
            .iter()
            .find(|event| event["type"] == "message_delta")
            .expect("message_delta flushed at EOF");
        assert_eq!(message_delta["usage"]["input_tokens"], 10);
        assert_eq!(message_delta["usage"]["output_tokens"], 4);
        assert!(events.iter().any(|event| event["type"] == "message_stop"));
    }

    #[tokio::test]
    async fn streaming_usage_omits_zero_cache_fields() {
        // Upstream parity: a zero cache bucket is omitted rather than emitted as 0, and
        // with no cache hit input_tokens equals prompt_tokens (nothing to subtract).
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

        assert_eq!(message_start["message"]["usage"]["input_tokens"], 12);
        assert!(message_start["message"]["usage"]
            .get("cache_read_input_tokens")
            .is_none());
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
