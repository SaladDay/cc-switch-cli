# Auto Visible Apps Temporary Plan

**Goal:** Reduce first-launch clutter as supported apps grow, while keeping the user's manual visible-app choices authoritative.

**Decision:** Add an auto-detection mode for visible apps. Claude and Codex are baseline apps and are not controlled by auto detection. Users can still manually hide or show Claude and Codex in Settings.

---

## Product Rules

### Modes

- **Auto mode:** CC Switch adjusts visible apps from local CLI detection.
- **Manual mode:** CC Switch never changes visible apps automatically.

The default for new users should be auto mode. Existing users should keep their current visible-app settings and enter manual mode unless they explicitly enable auto mode.

### App Detection Scope

Auto detection only controls:

- Gemini
- OpenCode
- Hermes
- OpenClaw

Auto detection does not control:

- Claude
- Codex

Claude and Codex should keep their existing defaults for new users. They remain user-toggleable in Settings.

### Installed App Signal

Use the existing local environment check path as the source of truth:

- `claude`
- `codex`
- `gemini`
- `opencode`
- `hermes`
- `openclaw`

An app counts as installed only when its binary can be found and version probing does not fail. A missing binary or version-probe error should not auto-enable that app.

### Startup Behavior

On TUI startup:

1. Load settings.
2. If auto mode is enabled, detect controlled apps.
3. Update visible apps for controlled apps to match detected installed state.
4. Leave Claude and Codex exactly as the user last saved them.
5. If the current app becomes hidden, switch to the next visible app using the existing visible-app order.
6. Show a toast summarizing changes only when visibility actually changed.

No modal should appear during normal auto mode startup.

### Manual Mode Behavior

Manual mode should not mutate visible apps.

If a hidden controlled app is detected as installed, show a toast once for that app:

- This is persisted. It should not appear on every startup.
- It may appear again only after a real state transition, for example the app was previously seen as not installed and later becomes installed again.
- The toast should be informational and should not open a modal.
- It should point the user to Settings > Visible Apps.

Claude and Codex are excluded from this hidden-installed toast because they are excluded from auto detection.

### First-Run Prompt

For new users, show a one-time modal only if detection would hide or show any controlled app compared with the default visible-app set.

The modal should ask whether CC Switch may adjust visible apps based on installed CLIs:

- **Use Auto:** enable auto mode and apply the detected controlled-app visibility.
- **Keep Current:** keep the current visible apps and use manual mode.

This prompt is one-time. Persist the decision so it does not come back on every launch.

Existing users should not see this modal automatically after upgrade. They can opt into auto mode from Settings.

---

## Settings Model

Extend `AppSettings` with a small visible-apps preference block:

```rust
pub struct VisibleAppsSettings {
    pub mode: VisibleAppsMode,
    pub auto_prompt_decided: bool,
    pub manual_hidden_installed_notices: HashMap<String, bool>,
    pub last_detected_installed: HashMap<String, bool>,
}

pub enum VisibleAppsMode {
    Auto,
    Manual,
}
```

Keep the existing `visible_apps: VisibleApps` field as the persisted visibility state used by rendering and navigation. The new block controls how that state is maintained.

Migration rules:

- New settings file: `mode = Auto`, `auto_prompt_decided = false`.
- Existing settings file: `mode = Manual`, `auto_prompt_decided = true`.
- Missing notice maps default to empty.

---

## TUI Changes

### Settings Page

Add a compact mode row near `Visible Apps`:

- English: `Visible Apps Mode`
- Chinese: `可见应用模式`
- Values: `auto` / `manual`, localized in Chinese.

Keep `Visible Apps` as the app picker entry. In manual mode it edits the saved visible apps directly. In auto mode it can still allow manual overrides for Claude/Codex, but controlled apps should be shown as detection-managed.

Recommended picker behavior in auto mode:

- Claude/Codex remain normal toggle rows.
- Gemini/OpenCode/Hermes/OpenClaw display their detected state and are not directly toggleable while auto mode is on.
- Provide a clear toast if the user tries to toggle a detection-managed row.

This keeps the rule simple: auto mode owns controlled apps, the user owns Claude/Codex.

### First-Run Modal

Use the existing centered dialog style. Do not add inline keyboard guidance to normal panels.

The modal should be short:

- Title: visible-app auto detection.
- Body: CC Switch can show installed apps and hide apps that are not installed.
- Actions: `Use Auto`, `Keep Current`.

### Toasts

Auto mode changed apps:

- English: `Visible apps updated: Gemini, Hermes`
- Chinese: `已更新可见应用：Gemini、Hermes`

Manual mode hidden installed app:

- English: `Hermes is installed but hidden. Enable it in Settings > Visible Apps.`
- Chinese: `Hermes 已安装但被隐藏。可在设置 > 可见应用中启用。`

Persist the manual-mode toast per app so it is not repeated on every startup.

---

## Implementation Tasks

### Task 1: Add Settings State

Files:

- `src-tauri/src/settings.rs`

Work:

- Add `VisibleAppsMode` and `VisibleAppsSettings`.
- Add migration behavior for new vs existing settings.
- Add getters/setters for visible-app mode and notice state.
- Keep `VisibleApps::validate()` unchanged so zero visible apps remain invalid.

### Task 2: Extract Detection Helper

Files:

- `src-tauri/src/services/local_env_check.rs`
- Possibly create `src-tauri/src/services/visible_apps.rs`

Work:

- Reuse the existing `LocalTool` definitions.
- Add a helper that returns installed status by `AppType`.
- Treat version-probe errors as not installed for auto visibility.
- Keep detection synchronous unless startup latency becomes noticeable.

### Task 3: Apply Startup Policy

Files:

- `src-tauri/src/cli/tui/mod.rs`
- `src-tauri/src/cli/tui/runtime_actions/settings.rs`
- Possibly `src-tauri/src/cli/tui/app/helpers.rs`

Work:

- Run visible-app policy during TUI startup before final app selection.
- Apply auto-mode controlled-app changes.
- Leave Claude/Codex untouched.
- In manual mode, emit persisted one-time hidden-installed toasts.
- If the requested/current app is hidden after policy application, reuse existing next-visible-app fallback.

### Task 4: Add First-Run Modal

Files:

- `src-tauri/src/cli/tui/app/types.rs`
- `src-tauri/src/cli/tui/app/overlay_handlers/dialogs.rs`
- `src-tauri/src/cli/tui/ui/overlay/render.rs`
- `src-tauri/src/cli/i18n.rs`

Work:

- Add one dialog overlay for the new-user auto-detection decision.
- Persist the decision.
- Do not show it for existing users migrated to manual mode.
- Do not show it again after either action.

### Task 5: Update Settings UI

Files:

- `src-tauri/src/cli/tui/app/app_state.rs`
- `src-tauri/src/cli/tui/app/content_config.rs`
- `src-tauri/src/cli/tui/app/overlay_handlers/pickers.rs`
- `src-tauri/src/cli/tui/ui/config.rs`
- `src-tauri/src/cli/tui/ui/overlay/pickers.rs`
- `src-tauri/src/cli/i18n.rs`

Work:

- Add visible-app mode row.
- Keep the visible-app picker in the current overlay style.
- In auto mode, prevent toggling detection-managed apps and show a toast.
- Ensure separators stay non-focusable.

### Task 6: Tests

Files:

- `src-tauri/src/settings.rs`
- `src-tauri/src/services/local_env_check.rs`
- `src-tauri/src/cli/tui/app/tests.rs`
- `src-tauri/src/cli/tui/runtime_actions/mod.rs`
- `src-tauri/src/cli/tui/ui/tests.rs`

Test cases:

- New users default to auto mode and undecided prompt state.
- Existing settings migrate to manual mode and no prompt.
- Auto mode updates only Gemini/OpenCode/Hermes/OpenClaw.
- Auto mode never changes Claude/Codex.
- Manual mode does not mutate visible apps.
- Manual mode hidden-installed toast is persisted and appears once per app state transition.
- Visible-app picker rejects zero visible apps as before.
- Auto-mode picker blocks toggling detection-managed rows.
- Startup falls back to the next visible app if the requested app is hidden.

Verification commands:

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml visible_apps -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml local_env_check -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml tui::app -- --test-threads=1
```

---

## Open Decisions Before Coding

- Whether auto mode should run only at TUI startup or also when opening Settings.
- Whether version-probe errors should be surfaced in the auto-mode picker as `found, version unknown` instead of treated as not installed.
- Whether the first-run modal should appear in CLI-only flows. Current recommendation: TUI only.
