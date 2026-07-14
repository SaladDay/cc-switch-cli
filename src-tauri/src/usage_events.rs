//! Usage event notification shim for the CLI build.
//!
//! Upstream desktop emits a Tauri event after usage-log writes. This crate is
//! CLI-only, so the backend keeps the call sites for upstream parity and makes
//! notification a no-op.

pub fn notify_log_recorded() {}
