<div align="center">

# CC-Switch TUI

[![Version](https://img.shields.io/badge/version-0.0.1-blue.svg)](https://github.com/handy-sun/cc-switch-tui/releases)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg)](https://github.com/handy-sun/cc-switch-tui/releases)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

<a href="https://trendshift.io/repositories/22544" target="_blank"><img src="https://trendshift.io/api/badge/repositories/22544" alt="SaladDay%2Fcc-switch-tui | Trendshift" style="width: 250px; height: 55px;" width="250" height="55"/></a>

**Command-Line Management Tool for Claude Code, Codex, Gemini, OpenCode & OpenClaw**

Unified management for Claude Code, Codex, Gemini, OpenCode, and OpenClaw provider configurations, plus app-specific support for MCP servers, skills, prompts, local proxy routes, and environment checks.

English | [中文](README_ZH.md)

</div>

---

## 📖 About

This project is a **TUI fork** of [CC-Switch](https://github.com/farion1231/cc-switch). 

🔄 The WebDAV sync feature is fully compatible with the upstream project.


**Credits:** Original architecture and core functionality from [farion1231/cc-switch](https://github.com/farion1231/cc-switch)

**Changelog:** [CHANGELOG.md](CHANGELOG.md)

---

## 📸 Screenshots

<div align="center">
  <h3>Home</h3>
  <img src="assets/screenshots/home-en.png" alt="Home" width="70%"/>
</div>

<br/>

<table>
  <tr>
    <th>Switch</th>
    <th>Settings</th>
  </tr>
  <tr>
    <td><img src="assets/screenshots/switch-en.png" alt="Switch" width="100%"/></td>
    <td><img src="assets/screenshots/settings-en.png" alt="Settings" width="100%"/></td>
  </tr>
</table>

## 🚀 Quick Start

**Interactive Mode (Recommended)**
```bash
cc-switch
```
🤩 Follow on-screen menus to explore features.

**Command-Line Mode**
```bash
cc-switch-tui provider list              # List providers
cc-switch-tui provider switch <id>       # Switch provider
cc-switch-tui provider export <id>       # Export a Claude provider to a standalone settings file
cc-switch-tui provider stream-check <id> # Check provider stream health
cc-switch-tui config webdav show         # Inspect WebDAV sync settings
cc-switch-tui env tools                  # Check local CLI tools
cc-switch-tui mcp sync                   # Sync MCP servers
cc-switch-tui proxy show                 # Inspect proxy routes and status

# Use the global `--app` flag to target specific applications:
cc-switch-tui --app claude provider list    # Manage Claude providers
cc-switch-tui --app codex mcp sync          # Sync Codex MCP servers
cc-switch-tui --app gemini prompts list     # List Gemini prompts
cc-switch-tui --app openclaw provider list  # Manage OpenClaw providers

# Supported apps: `claude` (default), `codex`, `gemini`, `opencode`, `openclaw`
```

See the "Features" section for full command list.

---

## 📥 Installation

### Method 1: Quick Install (macOS / Linux)

> Windows users: see Manual Installation below.

```bash
curl -fsSL https://github.com/handy-sun/cc-switch-tui/releases/latest/download/install.sh | bash
```

This installs  to `~/.local/bin`. Set `CC_SWITCH_INSTALL_DIR` to change the target directory.

- If the target already exists, the installer prompts in TTY and refuses to overwrite in non-interactive shells unless `CC_SWITCH_FORCE=1` is set.
- On Linux, set `CC_SWITCH_LINUX_LIBC=glibc` if you need the glibc build.

<details>
<summary>Manual Installation</summary>

#### macOS

```bash
# Download Universal Binary (recommended, supports Apple Silicon + Intel)
curl -LO https://github.com/handy-sun/cc-switch-tui/releases/latest/download/cc-switch-tui-darwin-universal.tar.gz

# Extract
tar -xzf cc-switch-tui-darwin-universal.tar.gz

# Add execute permission
chmod +x cc-switch

# Move to PATH
sudo mv cc-switch-tui /usr/local/bin/

# If you encounter "cannot be verified" warning
xattr -cr /usr/local/bin/cc-switch
```

#### Linux (x64)

```bash
# Download
curl -LO https://github.com/handy-sun/cc-switch-tui/releases/latest/download/cc-switch-tui-linux-x64-musl.tar.gz

# Extract
tar -xzf cc-switch-tui-linux-x64-musl.tar.gz

# Add execute permission
chmod +x cc-switch

# Move to PATH
sudo mv cc-switch-tui /usr/local/bin/
```

#### Linux (ARM64)

```bash
# For Raspberry Pi or ARM servers
curl -LO https://github.com/handy-sun/cc-switch-tui/releases/latest/download/cc-switch-tui-linux-arm64-musl.tar.gz
tar -xzf cc-switch-tui-linux-arm64-musl.tar.gz
chmod +x cc-switch
sudo mv cc-switch-tui /usr/local/bin/
```

#### Windows

```powershell
# Download the zip file
# https://github.com/handy-sun/cc-switch-tui/releases/latest/download/cc-switch-tui-windows-x64.zip

# After extracting, move cc-switch-tui.exe to a PATH directory, e.g.:
move cc-switch-tui.exe C:\Windows\System32\

# Or run directly
.\cc-switch-tui.exe
```

</details>

### Method 2: Build from Source

**Prerequisites:**
- Rust 1.85+ ([install via rustup](https://rustup.rs/))

**Build:**
```bash
git clone https://github.com/handy-sun/cc-switch-tui.git
cd cc-switch-tui/src-tauri
cargo build --release

# Binary location: ./target/release/cc-switch
```

**Install to System:**
```bash
# macOS/Linux
sudo cp target/release/cc-switch-tui /usr/local/bin/

# Windows
copy target\release\cc-switch-tui.exe C:\Windows\System32\
```

---

## ✨ Features

### 🔌 Provider Management

Manage API configurations for **Claude Code**, **Codex**, **Gemini**, **OpenCode**, and **OpenClaw**.

**Features:** One-click switching, standalone Claude settings export, multi-endpoint support, API key management, remote model discovery, and per-app diagnostics such as speed testing or stream health checks where supported.

```bash
cc-switch-tui provider list              # List all providers
cc-switch-tui provider current           # Show current provider
cc-switch-tui provider switch <id>       # Switch provider
cc-switch-tui provider add               # Add new provider
cc-switch-tui provider edit <id>         # Edit existing provider
cc-switch-tui provider duplicate <id>    # Duplicate a provider
cc-switch-tui provider delete <id>       # Delete provider
cc-switch-tui provider export <id>       # Export to ./.claude/settings.local.json for Claude auto-load
cc-switch-tui provider speedtest <id>    # Test API latency
cc-switch-tui provider stream-check <id> # Run stream health check
cc-switch-tui provider fetch-models <id> # Fetch remote model list
cc-switch-tui provider export <id> --output ~/.claude/settings-demo.json # Custom settings file path
```

### 🛠️ MCP Server Management

Manage Model Context Protocol servers across Claude, Codex, Gemini, and OpenCode.

**Features:** Unified management, multi-app support, three transport types (stdio/http/sse), automatic sync, and live-config adapters for TOML and JSON targets.

```bash
cc-switch-tui mcp list                   # List all MCP servers
cc-switch-tui mcp add                    # Add new MCP server (interactive)
cc-switch-tui mcp edit <id>              # Edit MCP server
cc-switch-tui mcp delete <id>            # Delete MCP server
cc-switch-tui mcp enable <id> --app claude   # Enable for specific app
cc-switch-tui mcp disable <id> --app claude  # Disable for specific app
cc-switch-tui mcp validate <command>     # Validate command in PATH
cc-switch-tui mcp sync                   # Sync to live files
cc-switch-tui mcp import --app claude    # Import from live config
```

### 💬 Prompts Management

Manage system prompt presets for AI coding assistants.

**Cross-app support:** Claude (`CLAUDE.md`), Codex (`AGENTS.md`), Gemini (`GEMINI.md`), OpenCode (`AGENTS.md`), OpenClaw (`AGENTS.md`).

```bash
cc-switch-tui prompts list               # List prompt presets
cc-switch-tui prompts current            # Show current active prompt
cc-switch-tui prompts activate <id>      # Activate prompt
cc-switch-tui prompts deactivate         # Deactivate current active prompt
cc-switch-tui prompts create [name]      # Create a prompt preset, optionally naming it up front
cc-switch-tui prompts rename <id> [name] # Rename prompt preset, interactive if name is omitted
cc-switch-tui prompts edit <id>          # Edit prompt preset
cc-switch-tui prompts show <id>          # Display full content
cc-switch-tui prompts delete <id>        # Delete prompt
```

### 🎯 Skills Management

Manage and extend Claude Code/Codex/Gemini/OpenCode capabilities with community skills.

**Features:** SSOT-based skills store, multi-app enable/disable, sync to app directories, unmanaged scan/import, repo discovery.

```bash
cc-switch-tui skills list                # List installed skills
cc-switch-tui skills discover <query>      # Discover available skills (alias: search)
cc-switch-tui skills install <name>      # Install a skill
cc-switch-tui skills uninstall <name>    # Uninstall a skill
cc-switch-tui skills enable <name>       # Enable for current app (--app)
cc-switch-tui skills disable <name>      # Disable for current app (--app)
cc-switch-tui skills info <name>         # Show skill information
cc-switch-tui skills sync                # Sync enabled skills to app dirs
cc-switch-tui skills sync-method [m]     # Show/set sync method (auto|symlink|copy)
cc-switch-tui skills scan-unmanaged      # Scan unmanaged skills in app dirs
cc-switch-tui skills import-from-apps    # Import unmanaged skills into SSOT
cc-switch-tui skills repos list          # List skill repositories
cc-switch-tui skills repos add <repo>    # Add repo (owner/name[@branch] or GitHub URL)
cc-switch-tui skills repos remove <repo> # Remove repo (owner/name or GitHub URL)
cc-switch-tui skills repos enable <repo> # Enable repo without changing branch
cc-switch-tui skills repos disable <repo> # Disable repo without changing branch
```

### ⚙️ Configuration Management

Manage configuration backups, imports, and exports.

**Features:** Custom backup naming, interactive backup selection, automatic rotation (keep 10), import/export, common snippets, WebDAV sync.

```bash
cc-switch-tui config show                # Display configuration
cc-switch-tui config path                # Show config file paths
cc-switch-tui config validate            # Validate config file

# Common snippet (shared settings across providers)
# Tries to refresh live config when applicable (`--apply` is kept only as a compatibility flag)
cc-switch-tui --app claude config common show
cc-switch-tui --app claude config common set --snippet '{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false}'
cc-switch-tui --app claude config common clear

# Backup
cc-switch-tui config backup              # Create backup (auto-named)
cc-switch-tui config backup --name my-backup  # Create backup with custom name

# Restore
cc-switch-tui config restore             # Interactive: select from backup list
cc-switch-tui config restore --backup <id>    # Restore specific backup by ID
cc-switch-tui config restore --file <path>    # Restore from external file

# Import/Export
cc-switch-tui config export <path>       # Export to external file
cc-switch-tui config import <path>       # Import from external file

# WebDAV sync
cc-switch-tui config webdav show
cc-switch-tui config webdav set --base-url <url> --username <user> --password <password> --enable
cc-switch-tui config webdav jianguoyun --username <user> --password <password>
cc-switch-tui config webdav check-connection
cc-switch-tui config webdav upload
cc-switch-tui config webdav download
cc-switch-tui config webdav migrate-v1-to-v2

cc-switch-tui config reset               # Reset to default configuration
```

### 🌉 Proxy Management

Inspect and control the local multi-app proxy used by supported apps.

**Features:** Persisted enable/disable switch, current route inspection, dashboard telemetry, and foreground serve mode for debugging.

```bash
cc-switch-tui proxy show                 # Show proxy configuration and routes
cc-switch-tui proxy enable               # Enable the persisted proxy switch
cc-switch-tui proxy disable              # Disable the persisted proxy switch
cc-switch-tui proxy serve                # Run the proxy in foreground
```

### 🧪 Environment & Local Tools

Inspect environment conflicts and whether required local CLIs are installed.

```bash
cc-switch-tui env check                  # Check environment conflicts
cc-switch-tui env list                   # List relevant environment variables
cc-switch-tui env tools                  # Check Claude/Codex/Gemini/OpenCode CLIs
```

### 🌐 Multi-language Support

Interactive mode supports English and Chinese, language settings are automatically saved.

- Default language: English
- Go to `⚙️ Settings` menu to switch language

### 🔧 Utilities

Shell completions, environment management, and other utilities.

```bash
# Shell completions
cc-switch-tui completions install --activate   # Recommended: install + activate for bash/zsh
cc-switch-tui completions install              # Conservative: install only, no rc edits
cc-switch-tui completions status               # Inspect managed completion status
cc-switch-tui completions uninstall            # Remove managed completion assets
cc-switch-tui completions bash                 # Compatibility raw generator path
cc-switch-tui completions fish                 # Raw generation still works for non-managed shells

# Environment management
cc-switch-tui env check                  # Check for environment conflicts
cc-switch-tui env list                   # List environment variables

# Self-update
cc-switch-tui update                     # Update to latest release
cc-switch-tui update --version vX.Y.Z    # Update to a specific version
```

Automated install/activation currently targets `bash` and `zsh` only. Other shells remain available through the raw generator path, for example `cc-switch-tui completions fish`.

---

## 🏗️ Architecture

### Core Design

- **SQLite-backed state**: Core data lives in `~/.cc-switch-tui/cc-switch.db` by default (or under `$CC_SWITCH_TUI_CONFIG_DIR/` when set); legacy `config.json` is kept only for older import and migration paths
- **Skills SSOT**: Skill source files live in `~/.cc-switch-tui/skills/` by default (or under `$CC_SWITCH_TUI_CONFIG_DIR/skills/` when set), while install state and app enablement stay in the database
- **Safe Live Sync (Default)**: Skip writing live files for apps that haven't been initialized yet (prevents creating `~/.claude`, `~/.codex`, `~/.gemini`, `~/.config/opencode`, or `~/.openclaw` unexpectedly)
- **Atomic Writes**: Temp file + rename pattern prevents corruption
- **Service Layer Reuse**: 100% reused from original GUI version
- **Concurrency Safe**: RwLock with scoped guards

### Configuration Files

**CC-Switch Storage** (default: `~/.cc-switch-tui`, override: `CC_SWITCH_TUI_CONFIG_DIR`):
- `~/.cc-switch-tui/cc-switch.db` - Main database for providers, MCP, prompts, and app state
- `~/.cc-switch-tui/settings.json` - Settings
- `~/.cc-switch-tui/skills/` - Installed skill sources (SSOT)
- `~/.cc-switch-tui/backups/` - Auto-rotation (keep 10)
- `~/.cc-switch-tui/config.json` - Legacy JSON kept for compatibility and import flows

When `CC_SWITCH_TUI_CONFIG_DIR` is set, CC-Switch uses that directory as its config root; existing data under `~/.cc-switch-tui` is not migrated automatically.

**Live Configs:**
- Claude: `~/.claude/settings.json` (provider/common config), `~/.claude.json` (MCP), `~/.claude/CLAUDE.md` (prompts)
- Codex: `~/.codex/auth.json` (auth state), `~/.codex/config.toml` (provider/common config + MCP), `~/.codex/AGENTS.md` (prompts)
- Gemini: `~/.gemini/.env` (provider env), `~/.gemini/settings.json` (settings + MCP), `~/.gemini/GEMINI.md` (prompts)
- OpenCode: `~/.config/opencode/opencode.json` (providers + MCP + runtime config), `~/.config/opencode/AGENTS.md` (prompts)
- OpenClaw: `~/.openclaw/openclaw.json` (providers + env/tools/agents defaults), `~/.openclaw/AGENTS.md` (prompts)

---

## ❓ FAQ (Frequently Asked Questions)

<details>
<summary><b>Why doesn't my configuration take effect after switching providers?</b></summary>

<br>

First, make sure the target CLI has been initialized at least once (i.e. its config directory exists). CC-Switch may skip live sync for uninitialized apps; you will see a warning. Run the target CLI once (e.g. `claude --help`, `codex --help`, `gemini --help`, `opencode --help`, `openclaw --help`), then switch again.

This is usually caused by **environment variable conflicts**. If you have API keys set in system environment variables (like `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`), they will override CC-Switch's configuration.

**Solution:**

1. Check for conflicts:
   ```bash
   cc-switch-tui env check --app claude
   ```

2. List all related environment variables:
   ```bash
   cc-switch-tui env list --app claude
   ```

3. If conflicts are found, manually remove them:
   - **macOS/Linux**: Edit your shell config file (`~/.bashrc`, `~/.zshrc`, etc.)
     ```bash
     # Find and delete the line with the environment variable
     nano ~/.zshrc
     # Or use your preferred text editor: vim, code, etc.
     ```
   - **Windows**: Open System Properties → Environment Variables and delete the conflicting variables

4. Restart your terminal for changes to take effect.

</details>

<details>
<summary><b>Which apps are supported?</b></summary>

<br>

CC-Switch currently supports five AI coding assistants:
- **Claude Code** (`--app claude`, default)
- **Codex** (`--app codex`)
- **Gemini** (`--app gemini`)
- **OpenCode** (`--app opencode`)
- **OpenClaw** (`--app openclaw`)

Use the global `--app` flag to specify which app to manage:
```bash
cc-switch-tui --app codex provider list
```

</details>

<details>
<summary><b>How do I report bugs or request features?</b></summary>

<br>

Please open an issue on our [GitHub Issues](https://github.com/handy-sun/cc-switch-tui/issues) page with:
- Detailed description of the problem or feature request
- Steps to reproduce (for bugs)
- Your system information (OS, version)
- Relevant logs or error messages

</details>

---

## 🛠️ Development

### Requirements

- **Rust**: 1.85+ ([rustup](https://rustup.rs/))
- **Cargo**: Bundled with Rust

### Commands

```bash
cd src-tauri

cargo run                            # Development mode
cargo run -- provider list           # Run specific command
cargo build --release                # Build release

cargo fmt                            # Format code
cargo clippy                         # Lint code
cargo test                           # Run tests
```

### Code Structure

```
src-tauri/src/
├── cli/
│   ├── commands/          # CLI subcommands (provider, mcp, prompts, skills, proxy, env, ...)
│   ├── tui/               # Interactive TUI mode (ratatui)
│   ├── interactive/       # Interactive entrypoint / TTY gate
│   └── ui/                # UI utilities (tables, colors)
├── services/              # Business logic (provider, mcp, prompt, webdav, ...)
├── database/              # SQLite storage, migrations, backup
├── main.rs                # CLI entry point
└── ...                    # App-specific configs, proxy, error handling
```


## 🤝 Contributing

Contributions welcome! This fork focuses on CLI functionality.

**Before submitting PRs:**
- ✅ Pass format check: `cargo fmt --check`
- ✅ Pass linter: `cargo clippy`
- ✅ Pass tests: `cargo test`
- 💡 Open an issue for discussion first

---

## 📜 License

- MIT © Original Author: Jason Young
- CLI Fork Maintainer: saladday
