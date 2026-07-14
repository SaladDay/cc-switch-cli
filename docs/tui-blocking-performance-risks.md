# TUI Blocking Performance Risks

This note tracks the blocking paths found while benchmarking `cc-switch` with
generated provider, usage, session, MCP, and skill data. The focus here is not
raw page-load time, but work that can occupy the TUI event loop or delay the
first frame enough to feel frozen.

## 1. First-frame TUI startup load

The highest-priority blocking path is TUI startup. `run()` creates the terminal
and then calls `initialize_app_state_with(..., UiData::load, ...)` before the
main render loop draws its first frame. `UiData::load()` opens app state, loads
provider/live config snapshots, MCP, prompts, config, skills, proxy state, and
usage/pricing data synchronously.

Impact: users can see a blank or non-interactive terminal while startup IO,
SQLite reads, live config sync, or usage aggregation completes.

Preferred direction: draw a lightweight initial UI first, then load the full
`UiData` on the existing app-data worker and apply the result on completion.
This should preserve the existing `UiData::load()` logic and only move it off
the first-frame path.

## 2. Provider write actions followed by synchronous full reload

Provider actions such as switch, import live config, delete, failover changes,
and default model changes run inside key-event handling. Several paths call
`load_state()`, perform DB/live-config writes, then assign `*data =
UiData::load(...)` before returning control to the event loop.

Impact: pressing a key can freeze the TUI until state writes, live config sync,
plugin sync, and full data reload complete. Benchmarks show the happy path is
not large, but this path is sensitive to real config size, disk latency, and DB
lock contention.

Preferred direction: keep the write semantics unchanged, but move the post-write
full reload to the app-data worker or use an optimistic local update plus
background refresh.

## 3. Synchronous proxy snapshot polling on tick

The main loop periodically calls `data.refresh_proxy_snapshot(...)` from the
tick path when proxy activity polling is due.

Impact: if proxy snapshot reads become slow because of daemon state, database
contention, or filesystem latency, the UI can stutter periodically even when the
user is not interacting with the proxy page.

Preferred direction: move proxy snapshot refresh to the proxy worker or cache and
throttle snapshot reads so ticks only consume already-available data.

## Non-blocking but still worth monitoring

Sessions and usage pages have the largest raw benchmark numbers, but their heavy
work already runs through worker threads:

- session scan/message load: `cc-switch-sessions`
- usage/pricing aggregation: `cc-switch-usage-pricing`

Those paths should still be optimized for large histories, but they are less
likely to freeze the whole TUI than the three paths above.
