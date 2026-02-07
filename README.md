<div align="center">

# CC-Switch CLI

[![Version](https://img.shields.io/badge/version-4.7.1-blue.svg)](https://github.com/saladday/cc-switch-cli/releases)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg)](https://github.com/saladday/cc-switch-cli/releases)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

**Command-Line Management Tool for Claude Code, Codex & Gemini CLI**

Unified management for Claude Code, Codex & Gemini CLI provider configurations, MCP servers, Skills extensions, and system prompts.

[English](README.md) | [中文](README_ZH.md)

</div>

---

## 📖 About

This project is a **CLI fork** of [CC-Switch](https://github.com/farion1231/cc-switch).


**Credits:** Original architecture and core functionality from [farion1231/cc-switch](https://github.com/farion1231/cc-switch)

---

## ❤️ Sponsor

<table>
  <tr>
    <td width="180">
      <a href="https://www.packyapi.com/register?aff=cc-switch-cli">
        <img src="assets/partners/logos/packycode.png" alt="PackyCode" width="150">
      </a>
    </td>
    <td>
      Thanks to <b>PackyCode</b> for sponsoring this project! PackyCode is a reliable and efficient API relay service provider, offering relay services for Claude Code, Codex, Gemini, and more. <br/>
      PackyCode provides special discounts for our software users: register via <a href="https://www.packyapi.com/register?aff=cc-switch-cli">this link</a> and use promo code <code>cc-switch-cli</code> when recharging to get <b>10% off</b>.
    </td>
  </tr>
</table>

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

<details>
  <summary>Legacy UI (no longer actively maintained)</summary>

> [!WARNING]
> The legacy interactive UI is temporarily not maintained. Please use the new TUI.

<table>
  <tr>
    <th>Interactive Main Menu</th>
    <th>Provider Management</th>
  </tr>
  <tr>
    <td><img src="assets/screenshots/main-en.png" alt="Legacy Main Menu" width="100%"/></td>
    <td><img src="assets/screenshots/add-en.png" alt="Legacy Provider Management" width="100%"/></td>
  </tr>
</table>

</details>

---

## 🚀 Quick Start

**Interactive Mode (Recommended)**
```bash
cc-switch
```
🤩 Follow on-screen menus to explore features.

**Command-Line Mode**
```bash
cc-switch provider list              # List providers
cc-switch provider switch <id>       # Switch provider
cc-switch mcp sync                   # Sync MCP servers

# Use the global `--app` flag to target specific applications:
cc-switch --app claude provider list    # Manage Claude providers
cc-switch --app codex mcp sync          # Sync Codex MCP servers
cc-switch --app gemini prompts list     # List Gemini prompts

# Supported apps: `claude` (default), `codex`, `gemini`
```

See the "Features" section below for full command list.

---

## ✨ Features

### 🔌 Provider Management

Manage API configurations for **Claude Code**, **Codex**, and **Gemini**.

**Features:** One-click switching, multi-endpoint support, API key management, speed testing, provider duplication.

```bash
cc-switch provider list              # List all providers
cc-switch provider current           # Show current provider
cc-switch provider switch <id>       # Switch provider
cc-switch provider add               # Add new provider
cc-switch provider edit <id>         # Edit existing provider
cc-switch provider duplicate <id>    # Duplicate a provider
cc-switch provider delete <id>       # Delete provider
cc-switch provider speedtest <id>    # Test API latency
```

#### PackyCode preset (Sponsor)

In the TUI "Add Provider" form, choose the `★ PackyCode` template to prefill endpoints:

- Website: `https://www.packyapi.com`
- Register: `https://www.packyapi.com/register?aff=cc-switch-cli`
- Promo code: `cc-switch-cli` (10% off)
- Claude (Anthropic): `https://www.packyapi.com`
- Codex (OpenAI): `https://www.packyapi.com/v1`
- Gemini: `https://www.packyapi.com`

### 🛠️ MCP Server Management

Manage Model Context Protocol servers across Claude/Codex/Gemini.

**Features:** Unified management, multi-app support, three transport types (stdio/http/sse), automatic sync, smart TOML parser.

```bash
cc-switch mcp list                   # List all MCP servers
cc-switch mcp add                    # Add new MCP server (interactive)
cc-switch mcp edit <id>              # Edit MCP server
cc-switch mcp delete <id>            # Delete MCP server
cc-switch mcp enable <id> --app claude   # Enable for specific app
cc-switch mcp disable <id> --app claude  # Disable for specific app
cc-switch mcp validate <command>     # Validate command in PATH
cc-switch mcp sync                   # Sync to live files
cc-switch mcp import --app claude    # Import from live config
```

### 💬 Prompts Management

Manage system prompt presets for AI coding assistants.

**Cross-app support:** Claude (`CLAUDE.md`), Codex (`AGENTS.md`), Gemini (`GEMINI.md`).

```bash
cc-switch prompts list               # List prompt presets
cc-switch prompts current            # Show current active prompt
cc-switch prompts activate <id>      # Activate prompt
cc-switch prompts deactivate         # Deactivate current active prompt
cc-switch prompts create             # Create new prompt preset
cc-switch prompts edit <id>          # Edit prompt preset
cc-switch prompts show <id>          # Display full content
cc-switch prompts delete <id>        # Delete prompt
```

### 🎯 Skills Management

Manage and extend Claude Code/Codex/Gemini capabilities with community skills.

**Features:** SSOT-based skills store, multi-app enable/disable, sync to app directories, unmanaged scan/import, repo discovery.

```bash
cc-switch skills list                # List installed skills
cc-switch skills search <query>      # Search available skills
cc-switch skills install <name>      # Install a skill
cc-switch skills uninstall <name>    # Uninstall a skill
cc-switch skills enable <name>       # Enable for current app (--app)
cc-switch skills disable <name>      # Disable for current app (--app)
cc-switch skills info <name>         # Show skill information
cc-switch skills sync                # Sync enabled skills to app dirs
cc-switch skills sync-method [m]     # Show/set sync method (auto|symlink|copy)
cc-switch skills scan-unmanaged      # Scan unmanaged skills in app dirs
cc-switch skills import-from-apps    # Import unmanaged skills into SSOT
cc-switch skills repos list          # List skill repositories
cc-switch skills repos add <repo>    # Add repo (owner/name[@branch] or GitHub URL)
cc-switch skills repos remove <repo> # Remove repo (owner/name or GitHub URL)
```

### ⚙️ Configuration Management

Manage configuration backups, imports, and exports.

**Features:** Custom backup naming, interactive backup selection, automatic rotation (keep 10), import/export.

```bash
cc-switch config show                # Display configuration
cc-switch config path                # Show config file paths
cc-switch config validate            # Validate config file

# Common snippet (shared settings across providers)
cc-switch --app claude config common show
cc-switch --app claude config common set --json '{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false}' --apply
cc-switch --app claude config common clear --apply

# Backup
cc-switch config backup              # Create backup (auto-named)
cc-switch config backup --name my-backup  # Create backup with custom name

# Restore
cc-switch config restore             # Interactive: select from backup list
cc-switch config restore --backup <id>    # Restore specific backup by ID
cc-switch config restore --file <path>    # Restore from external file

# Import/Export
cc-switch config export <path>       # Export to external file
cc-switch config import <path>       # Import from external file

cc-switch config reset               # Reset to default configuration
```

### 🌐 Multi-language Support

Interactive mode supports English and Chinese, language settings are automatically saved.

- Default language: English
- Go to `⚙️ Settings` menu to switch language

### 🔧 Utilities

Shell completions, environment management, and other utilities.

```bash
# Shell completions
cc-switch completions <shell>        # Generate shell completions (bash/zsh/fish/powershell)

# Environment management
cc-switch env check                  # Check for environment conflicts
cc-switch env list                   # List environment variables

# Self-update
cc-switch update                     # Update to latest release
cc-switch update --version v4.7.1    # Update to a specific version
```

---

## 📥 Installation

### Method 1: Download Pre-built Binaries (Recommended)

Download the latest release from [GitHub Releases](https://github.com/saladday/cc-switch-cli/releases).

#### macOS

```bash
# Download Universal Binary (recommended, supports Apple Silicon + Intel)
curl -LO https://github.com/saladday/cc-switch-cli/releases/latest/download/cc-switch-cli-darwin-universal.tar.gz

# Extract
tar -xzf cc-switch-cli-darwin-universal.tar.gz

# Add execute permission
chmod +x cc-switch

# Move to PATH
sudo mv cc-switch /usr/local/bin/

# If you encounter "cannot be verified" warning
xattr -cr /usr/local/bin/cc-switch
```

#### Linux (x64)

```bash
# Download
curl -LO https://github.com/saladday/cc-switch-cli/releases/latest/download/cc-switch-cli-linux-x64-musl.tar.gz

# Extract
tar -xzf cc-switch-cli-linux-x64-musl.tar.gz

# Add execute permission
chmod +x cc-switch

# Move to PATH
sudo mv cc-switch /usr/local/bin/
```

#### Linux (ARM64)

```bash
# For Raspberry Pi or ARM servers
curl -LO https://github.com/saladday/cc-switch-cli/releases/latest/download/cc-switch-cli-linux-arm64-musl.tar.gz
tar -xzf cc-switch-cli-linux-arm64-musl.tar.gz
chmod +x cc-switch
sudo mv cc-switch /usr/local/bin/
```

#### Windows

```powershell
# Download the zip file
# https://github.com/saladday/cc-switch-cli/releases/latest/download/cc-switch-cli-windows-x64.zip

# After extracting, move cc-switch.exe to a PATH directory, e.g.:
move cc-switch.exe C:\Windows\System32\

# Or run directly
.\cc-switch.exe
```

### Method 2: Build from Source

**Prerequisites:**
- Rust 1.85+ ([install via rustup](https://rustup.rs/))

**Build:**
```bash
git clone https://github.com/saladday/cc-switch-cli.git
cd cc-switch-cli/src-tauri
cargo build --release

# Binary location: ./target/release/cc-switch
```

**Install to System:**
```bash
# macOS/Linux
sudo cp target/release/cc-switch /usr/local/bin/

# Windows
copy target\release\cc-switch.exe C:\Windows\System32\
```

---

## 🏗️ Architecture

### Core Design

- **SSOT**: All config in `~/.cc-switch/config.json`, live configs are generated artifacts
- **Safe Live Sync (Default)**: Skip writing live files for apps that haven't been initialized yet (prevents creating `~/.claude`, `~/.codex`, `~/.gemini` unexpectedly)
- **Atomic Writes**: Temp file + rename pattern prevents corruption
- **Service Layer Reuse**: 100% reused from original GUI version
- **Concurrency Safe**: RwLock with scoped guards

### Configuration Files

**CC-Switch Storage:**
- `~/.cc-switch/config.json` - Main configuration (SSOT)
- `~/.cc-switch/settings.json` - Settings
- `~/.cc-switch/backups/` - Auto-rotation (keep 10)

**Live Configs:**
- Claude: `~/.claude/settings.json`, `~/.claude.json` (MCP), `~/.claude/CLAUDE.md` (prompts)
- Codex: `~/.codex/auth.json`, `~/.codex/config.toml` (MCP), `~/.codex/AGENTS.md` (prompts)
- Gemini: `~/.gemini/.env`, `~/.gemini/settings.json` (MCP), `~/.gemini/GEMINI.md` (prompts)

---

## ❓ FAQ (Frequently Asked Questions)

<details>
<summary><b>Why doesn't my configuration take effect after switching providers?</b></summary>

<br>

First, make sure the target CLI has been initialized at least once (i.e. its config directory exists). CC-Switch may skip live sync for uninitialized apps; you will see a warning. Run the target CLI once (e.g. `claude --help`, `codex --help`, `gemini --help`), then switch again.

This is usually caused by **environment variable conflicts**. If you have API keys set in system environment variables (like `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`), they will override CC-Switch's configuration.

**Solution:**

1. Check for conflicts:
   ```bash
   cc-switch env check --app claude
   ```

2. List all related environment variables:
   ```bash
   cc-switch env list --app claude
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

CC-Switch currently supports three AI coding assistants:
- **Claude Code** (`--app claude`, default)
- **Codex** (`--app codex`)
- **Gemini** (`--app gemini`)

Use the global `--app` flag to specify which app to manage:
```bash
cc-switch --app codex provider list
```

</details>

<details>
<summary><b>How do I report bugs or request features?</b></summary>

<br>

Please open an issue on our [GitHub Issues](https://github.com/saladday/cc-switch-cli/issues) page with:
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
│   ├── commands/          # CLI subcommands (provider, mcp, prompts, config)
│   ├── interactive/       # Interactive TUI mode
│   └── ui.rs              # UI utilities (tables, colors)
├── services/              # Business logic
├── main.rs                # CLI entry point
└── ...
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
