# Legacy function_call streaming

The OpenAI Chat → Anthropic SSE converter follows `farion1231/cc-switch`
revision `efd236a474aee8d31c51c99684a7a6df712ad90e` for legacy function calls:
`Delta` accepts `tool_calls` and ignores the unknown `function_call` field.
The fork-specific legacy conversion branch has been removed.

This prevents empty legacy placeholders from creating empty `tool_use` blocks.
Providers must return modern `tool_calls` for streamed tool invocation; nonempty
legacy `function_call` payloads are also ignored. The existing upstream
`finish_reason: function_call` → `stop_reason: tool_use` mapping is retained,
even though a legacy-only response has no corresponding tool block. Changing
that mapping would be a separate compatibility decision.

Regression coverage checks text preservation and clean completion for the empty
placeholder, and ignored nonempty legacy payloads. Existing modern tool-call
streaming tests remain in place. This change aligns this specific path, not the
entire streaming converter.
