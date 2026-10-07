# Codex additional_tools compatibility

Port of [farion1231/cc-switch#7454](https://github.com/farion1231/cc-switch/pull/7454),
merged as `a35e5000b4ef809ed76eb36bd164bf6a35c36afc`.

The shared Codex tool registry collects both `tool_search_output` and
`additional_tools` input declarations using the existing tool conversion and
deduplication rules. Responses-to-Chat skips the declaration carrier rather than
emitting a contentless developer/system message. The shared registry also serves
the Anthropic converter. Native Responses forwarding is unchanged.

Keep the collector and carrier handling aligned with the reference implementation.
The existing namespace registry accepts function children only; custom tools inside
namespaces remain unsupported here and in the reference. Standalone custom tools
and namespaced function tools use the existing response mapping.
