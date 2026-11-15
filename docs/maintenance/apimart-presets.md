# APIMart presets

APIMart is the second sponsor preset. PatewayAI remains available for Claude and
Codex, in the penultimate sponsor position before FennoAI. Existing provider
records are not rewritten.

Endpoint sources (checked 2026-10-16):

- [Claude Messages](https://docs.apimart.ai/cn/api-reference/texts/general/claude-messages):
  `https://api.apimart.ai/v1/messages`; the Anthropic SDK base URL is
  `https://api.apimart.ai`, with API-key authentication.
- [Gemini native API](https://docs.apimart.ai/cn/api-reference/texts/gemini/quickstart):
  `https://api.apimart.ai/v1beta/models/...`; the base URL is
  `https://api.apimart.ai`.
- [OpenAI Responses](https://docs.apimart.ai/cn/api-reference/texts/openai/responses)
  and [OpenAI-compatible SDKs](https://docs.apimart.ai/cn):
  use `https://api.apimart.ai/v1` for Codex, OpenCode, OpenClaw, Hermes and Pi.
  Pi uses the Responses protocol and the documentation's `gpt-5.2-pro` example.

Presets reuse existing CLI/TUI provider configuration paths. Model selection for
other applications follows the existing template behavior and can be edited.
The image/video sponsorship copy does not add image/video task APIs to the CLI.

README image and signup links, and preset registration links, use
`https://go.apimart.ai/gh-cc-switch-cli`.
