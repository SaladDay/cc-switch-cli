# 88API preset data

`88api.json` mirrors the native settings in `farion1231/cc-switch` at
`e217bfcbbbbbf82f4619ab62f08fb002635b7858`, from
`src/config/{claude,gemini,opencode,openclaw,hermes,pi}ProviderPresets.ts`.
Pi model capabilities are expanded from `src/config/piModelCatalog.ts`;
TypeScript-only model catalog references are not native configuration fields.
Codex uses the existing shared TOML builder with `88api`,
`https://api.88api.ai/v1`, and `gpt-6.1-sol`, matching the upstream preset.

The CLI uses its existing native configuration writers and form serialization.
OpenClaw's separate suggested-defaults metadata is not a provider settings field;
the existing CLI default-selection flow remains in use. Registration links use
this project's `https://88api.ai/sign-up?aff=TeNl` rather than the upstream code.
The README images and bilingual sponsor copy come from the same upstream tree.
