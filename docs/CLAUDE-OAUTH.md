# Claude OAuth (draft)

This fork contains a first-stage native Claude Code OAuth authorization-code
flow. It is intentionally separate from provider switching and from the
existing reader for credentials created by the Claude CLI.

## Flow

1. Call `AuthService::start("claude_oauth", None)` (or provide a localhost
   callback such as `http://localhost:54545/callback`).
2. Open the returned `authorization_url` in a browser.
3. Pass the complete browser callback URL to
   `AuthService::complete("claude_oauth", callback_url)`.
4. The native service validates the redirect origin, callback `state`, expiry,
   and authorization errors, then exchanges the code with Anthropic.

`AuthService` is provider-neutral: `start(provider, redirect_uri)` dispatches
to a provider strategy and returns either a `device_code` or `browser` tagged
response. Adding another browser OAuth provider therefore adds an adapter and
dispatch entry, not `start_<provider>_login` methods to the facade.

The public result contains only an authorization URL or an account summary.
Access and refresh tokens stay in the native config store and are never
serialized into the JavaScript/N-API response or provider settings.

## Scope of this draft

The draft covers PKCE (S256), state binding, callback validation, token
exchange, and a single native account record. It does not yet provide account
listing, refresh, logout, profile/roles enrichment, or an N-API binding. Those
should land as separate follow-up changes after the protocol and storage
contract are reviewed.

The callback listener is expected to be owned by the desktop shell. The
service accepts a callback URL so a shell can use its own one-shot localhost
listener or a deep-link bridge without exposing token material to the UI.

## Compatibility note

The endpoint and client values follow the Claude Code OAuth protocol used by
the current CLIProxyAPI implementation. They are not a promise that Anthropic
will support arbitrary third-party clients or redirect URIs; production use
needs an explicit compatibility/security review.
