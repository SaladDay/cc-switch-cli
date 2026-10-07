# Codex compressed proxy requests

The request decoder and `proxy/content_encoding.rs` (including its tests) come from
[farion1231/cc-switch at d35726e](https://github.com/farion1231/cc-switch/tree/d35726e28695844deaf0098450b34911f5be7b78/src-tauri/src/proxy).

Codex Chat Completions, Responses and compact handlers collect bytes, decode
Content-Encoding, and then parse JSON. Successful decoding removes the original
Content-Encoding, Content-Length and Transfer-Encoding headers before forwarding.
Unsupported encodings and corrupt compressed data return an invalid-request error.

The CLI retains its existing Axum byte-extractor body limit, JSON rejection format,
and forwarding pipeline. Response decoding is unchanged. Like the referenced
request decoder, decoding does not impose a separate decompressed-output limit;
the shared module also exposes the upstream bounded decoding API.
