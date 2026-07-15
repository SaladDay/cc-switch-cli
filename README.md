<div align="center">

# CC-Switch CLI

[![Version](https://img.shields.io/badge/version-5.9.1-blue.svg)](https://github.com/saladday/cc-switch-cli/releases)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg)](https://github.com/saladday/cc-switch-cli/releases)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

<a href="https://trendshift.io/repositories/22544" target="_blank"><img src="https://trendshift.io/api/badge/repositories/22544" alt="SaladDay%2Fcc-switch-cli | Trendshift" style="width: 250px; height: 55px;" width="250" height="55"/></a>

**TUI + CLI dual-mode manager for Claude Code, Codex, Gemini, OpenCode, Hermes & OpenClaw**

Use the interactive TUI for daily switching, account, and session work, or the CLI for scripts and repeatable terminal workflows.

Unified management for provider configurations, MCP servers, skills, prompts, local proxy routes, usage statistics, and environment checks across supported AI coding assistants.

English | [中文](README_ZH.md)

</div>

---

## 📖 About

This project is a **CLI fork** of [CC-Switch](https://github.com/farion1231/cc-switch). 

🔄 The WebDAV sync feature is fully compatible with the upstream project.


**Credits:** Original architecture and core functionality from [farion1231/cc-switch](https://github.com/farion1231/cc-switch)

**Changelog:** [CHANGELOG.md](CHANGELOG.md)

---

## ❤️ Sponsor

<table>
  <tr>
    <td width="180">
      <a href="https://console.claudeapi.com/register?source=cc-switch-cli">
        <img src="assets/partners/logos/claudeapi.png" alt="ClaudeAPI" width="150">
      </a>
    </td>
    <td>
      Thanks to <b>ClaudeAPI</b> for supporting this project! <b>ClaudeAPI</b> is a Claude-focused API access provider built on official and AWS channels, offering high stability, low latency, and full support for Claude Code, Codex, Agent workflows, and enterprise use cases. It also supports business onboarding, team usage management, and invoicing. Exclusive offer for CC-Switch CLI users: register via this <a href="https://console.claudeapi.com/register?source=cc-switch-cli">exclusive link</a> to receive free trial credits and get started with Claude Code instantly.
    </td>
  </tr>
  <tr>
    <td width="180">
      <a href="https://s.qiniu.com/FVfiEb">
        <img src="assets/partners/logos/qiniuyun.png" alt="Qiniu Cloud AI" width="150">
      </a>
    </td>
    <td>
      Thanks to <b>Qiniu Cloud AI</b> for sponsoring this project! <b>Qiniu Cloud AI</b> is an enterprise-grade large-model MaaS platform from <b>Qiniu Cloud (HKEX: 02567)</b>, offering one-stop access to 150+ mainstream models worldwide, compatibility with the protocols of major global model vendors, and full-modality coverage across text, image, audio, video, and file processing — serving over <b>1.69 million</b> enterprise and developer users. <br/>
      Exclusive benefits: enterprise users can claim <b>12 million free tokens</b>, and referring friends can earn up to <b>tens of billions of tokens</b>. Register via <a href="https://s.qiniu.com/FVfiEb">this link</a>.
    </td>
  </tr>
  <tr>
    <td width="180">
      <a href="https://api.fenno.ai/register?redirect=/purchase?tab=subscription%26group=16&aff=Z6XB52KCVP6Y">
        <img src="assets/partners/logos/fennoai.png" alt="Fenno.ai" width="150">
      </a>
    </td>
    <td>
      Thanks to <b>Fenno.ai</b> for sponsoring this project! Fenno.ai is a stable, efficient API relay provider currently focused on Codex relay. It is compatible with both the OpenAI and Anthropic protocols and plugs smoothly into mainstream coding tools such as Codex, Claude Code, and OpenCode, reliably sustaining enterprise workloads of hundreds of billions of tokens per day, with corporate (B2B) settlement and invoicing for both domestic and overseas entities. <br/>
      Exclusive benefit for CC-Switch CLI users: register via <a href="https://api.fenno.ai/register?redirect=/purchase?tab=subscription%26group=16&aff=Z6XB52KCVP6Y">this link</a> to subscribe to a great-value Coding Plan — <b>¥9.9 for $150 of credit</b> — and earn up to a <b>20% reward</b> for referrals; the more you invite, the more you get!
    </td>
  </tr>
  <tr>
    <td width="180">
      <a href="https://runapi.co/register?aff=kTlB">
        <img src="assets/partners/logos/runapi.png" alt="RunAPI" width="150">
      </a>
    </td>
    <td>
      Thanks to <b>RunAPI</b> for sponsoring this project! <a href="https://runapi.co/register?aff=kTlB"><b>RunAPI</b></a> is a fast, stable API platform and OpenRouter alternative. One API key gives access to 150+ mainstream models, including OpenAI, Claude, Gemini, DeepSeek, and Grok, with prices as low as 10% of the original cost. It works with tools such as Claude Code and OpenClaw. Exclusive benefit for cc-switch-cli users: register and contact the admin to claim CNY 7 in free credits.
    </td>
  </tr>
  <tr>
    <td width="180">
      <a href="https://cubence.com/signup?code=SC3M1CAH&source=ccscli">
        <img src="assets/partners/logos/cubence.png" alt="Cubence" width="150">
      </a>
    </td>
    <td>
      Thanks to <b>Cubence</b> for sponsoring this project! Cubence is an API relay service provider dedicated to offering stable and efficient services to its customers. Operating since September 2025, it has provided support for various models such as Claude code, Codex, and Gemini. Register via <a href="https://cubence.com/signup?code=SC3M1CAH&source=ccscli">this link</a> and use the <code>CCSCLI</code> discount code when topping up to enjoy a 10% discount.
    </td>
  </tr>
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
  <tr>
    <td width="180">
      <a href="https://www.aicodemirror.com/register?invitecode=77V9EA">
        <img src="assets/partners/logos/aicodemirror.png" alt="AICodeMirror" width="150">
      </a>
    </td>
    <td>
      Thanks to <b>AICodeMirror</b> for sponsoring this project! <b>AICodeMirror</b> provides official high-stability relay services for Claude Code / Codex / Gemini CLI, with enterprise-grade concurrency, fast invoicing, and 24/7 dedicated technical support. Claude Code / Codex / Gemini official channels at <b>38% / 2% / 9%</b> of original price, with extra discounts on top-ups! <b>AICodeMirror</b> offers special benefits for cc-switch-cli users: register via <a href="https://www.aicodemirror.com/register?invitecode=77V9EA">this link</a> to enjoy <b>20% off</b> your first top-up, and enterprise customers can get up to <b>25% off</b>!
    </td>
  </tr>
  <tr>
    <td width="180">
      <a href="https://ddshub.short.gy/ccscli">
        <img src="assets/partners/logos/DDSHub.png" alt="DDS" width="150">
      </a>
    </td>
    <td>
      Thanks to <b>DDS</b> for sponsoring this project! DDS Hub is a reliable and high-performance Claude API proxy service. DDS Hub provides cost-effective domestic Claude direct acceleration services for both individual and enterprise users. We offer stable and low-latency Claude Max number pools, with full support for <b>Claude Haiku, Opus, Sonnet</b> and other flagship models. Invoices are available for recharges of 1000 RMB or more. Enterprise customers can also enjoy customized grouping and dedicated technical support services. <br/>
      Exclusive benefit for CC-Switch CLI users: register via <a href="https://ddshub.short.gy/ccscli">this link</a> and enjoy <b>an extra 10% credit</b> on your first recharge (please contact the group admin to claim after recharging)!
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

## 🚀 Quick Start

**TUI Mode (Recommended)**
```bash
cc-switch
```
Use the full-screen interface to switch providers, manage accounts, review sessions, and inspect proxy status.

**Command-Line Mode**
```bash
cc-switch provider list              # List providers
cc-switch provider switch <id>       # Switch provider
cc-switch use <id>                   # Switch provider (shortcut)
cc-switch provider export <id>       # Export a Claude provider to a standalone settings file
cc-switch provider stream-check <id> # Check provider stream health
cc-switch start <claude|codex> <id> --dry-run # Preview launch
cc-switch auth list                  # List managed ChatGPT/Codex OAuth accounts
cc-switch sessions list --all        # Review saved assistant sessions
cc-switch sessions sync-usage --all  # Import local session token/cost usage
cc-switch config webdav show         # Inspect WebDAV sync settings
cc-switch env tools                  # Check local CLI tools
cc-switch mcp sync                   # Sync MCP servers
cc-switch proxy show                 # Inspect proxy routes and status

# Use the global `--app` flag to target specific applications:
cc-switch --app claude provider list    # Manage Claude providers
cc-switch --app codex mcp sync          # Sync Codex MCP servers
cc-switch --app gemini prompts list     # List Gemini prompts
cc-switch --app hermes provider list    # Manage Hermes providers
cc-switch --app openclaw provider list  # Manage OpenClaw providers

# Supported apps: `claude` (default), `codex`, `gemini`, `opencode`, `hermes`, `openclaw`
```

See the "Features" section for full command list.

---

## 📥 Installation

### Method 1: Quick Install (macOS / Linux)

> Windows users: see Manual Installation below.

```bash
curl -fsSL https://github.com/SaladDay/cc-switch-cli/releases/latest/download/install.sh | bash
```

This installs `cc-switch` to `~/.local/bin`. Set `CC_SWITCH_INSTALL_DIR` to change the target directory.

- If the target already exists, the installer prompts in TTY and refuses to overwrite in non-interactive shells unless `CC_SWITCH_FORCE=1` is set.
- On Linux, set `CC_SWITCH_LINUX_LIBC=glibc` if you need the glibc build.

<details>
<summary>Manual Installation</summary>

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

</details>

### Method 2: Install via Homebrew
If you are using Homebrew on your machine, you can use Homebrew to install cc-switch.
```
brew install cc-switch-cli
```

Update:
```
brew upgrade cc-switch-cli
```
If you installed cc-switch via Homebrew, please use Homebrew to upgrade cc-switch, instead of the built-in update feature, as this breaks Homebrew formulae’s own upgrade functionality.

### Method 3: Build from Source

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

## ✨ Features

### 🔌 Provider Management

Manage API configurations for **Claude Code**, **Codex**, **Gemini**, **OpenCode**, **Hermes**, and **OpenClaw**.

**Features:** One-click switching, standalone Claude settings export, multi-endpoint support, API key management, remote model discovery, and per-app diagnostics such as speed testing or stream health checks where supported.

```bash
cc-switch provider list              # List all providers
cc-switch provider current           # Show current provider
cc-switch provider switch <id>       # Switch provider
cc-switch use <id>                   # Switch provider (shortcut)
cc-switch provider add               # Add new provider
cc-switch provider edit <id>         # Edit existing provider
cc-switch provider duplicate <id>    # Duplicate a provider
cc-switch provider delete <id>       # Delete provider
cc-switch provider export <id>       # Export to ./.claude/settings.local.json for Claude auto-load
cc-switch provider speedtest <id>    # Test API latency
cc-switch provider stream-check <id> # Run stream health check
cc-switch provider fetch-models <id> # Fetch remote model list
cc-switch provider export <id> --output ~/.claude/settings-demo.json # Custom settings file path
```

### 🔐 Managed Accounts

Manage ChatGPT/Codex OAuth accounts locally and reuse them across provider profiles, including using a Codex OAuth account as a Claude Code provider through the local proxy.

**Features:** device-flow login, account listing, default account selection, account removal, and provider binding without copying long-lived tokens into each provider.

```bash
cc-switch auth status                # Show managed account status
cc-switch auth login                 # Sign in with ChatGPT/Codex OAuth
cc-switch auth list                  # List signed-in accounts
cc-switch auth default <account-id>  # Set the default account
cc-switch auth remove <account-id>   # Remove an account
```

### 🛠️ MCP Server Management

Manage Model Context Protocol servers across Claude, Codex, Gemini, OpenCode, and Hermes.

**Features:** Unified management, multi-app support, three transport types (stdio/http/sse), automatic sync, and live-config adapters for TOML and JSON targets.

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

**Cross-app support:** Claude (`CLAUDE.md`), Codex (`AGENTS.md`), Gemini (`GEMINI.md`), OpenCode (`AGENTS.md`), Hermes (`AGENTS.md`), OpenClaw (`AGENTS.md`).

```bash
cc-switch prompts list               # List prompt presets
cc-switch prompts current            # Show current active prompt
cc-switch prompts activate <id>      # Activate prompt
cc-switch prompts deactivate         # Deactivate current active prompt
cc-switch prompts create [name]      # Create a prompt preset, optionally naming it up front
cc-switch prompts rename <id> [name] # Rename prompt preset, interactive if name is omitted
cc-switch prompts edit <id>          # Edit prompt preset
cc-switch prompts show <id>          # Display full content
cc-switch prompts delete <id>        # Delete prompt
```

### 🎯 Skills Management

Manage and extend Claude Code/Codex/Gemini/OpenCode/Hermes capabilities with community skills.

**Features:** SSOT-based skills store, multi-app enable/disable, sync to app directories, unmanaged scan/import, repo discovery, skills.sh marketplace search.

```bash
cc-switch skills list                # List installed skills
cc-switch skills discover <query>      # Discover available skills (alias: search)
cc-switch skills market <query>      # Search skills.sh marketplace
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
cc-switch skills repos enable <repo> # Enable repo without changing branch
cc-switch skills repos disable <repo> # Disable repo without changing branch
```

### 🕘 Session History & Usage Statistics

Review saved assistant sessions, resume a session with one command, delete old records, and import local session logs into token/cost statistics.

**Features:** cross-app session scanning, message preview, one-command resume, safe delete confirmation, JSON output, and usage sync for Claude, Codex, Gemini, and OpenCode.

```bash
cc-switch sessions list --all        # List saved sessions across supported apps
cc-switch sessions show <id>         # Show session metadata and messages
cc-switch sessions resume <id>       # Resume a saved session
cc-switch sessions delete <id>       # Delete a saved session
cc-switch sessions sync-usage --all  # Sync local logs into usage statistics
```

### ⚙️ Configuration Management

Manage configuration backups, imports, and exports.

**Features:** Custom backup naming, interactive backup selection, automatic rotation (keep 10), import/export, common snippets, WebDAV sync.

```bash
cc-switch config show                # Display configuration
cc-switch config path                # Show config file paths
cc-switch config validate            # Validate config file

# Common snippet (shared settings across providers)
# Tries to refresh live config when applicable (`--apply` is kept only as a compatibility flag)
cc-switch --app claude config common show
cc-switch --app claude config common set --snippet '{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false}'
cc-switch --app claude config common clear

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

# WebDAV sync
cc-switch config webdav show
cc-switch config webdav set --base-url <url> --username <user> --password <password> --enable
cc-switch config webdav jianguoyun --username <user> --password <password>
cc-switch config webdav check-connection
cc-switch config webdav upload
cc-switch config webdav download
cc-switch config webdav migrate-v1-to-v2

cc-switch config reset               # Reset to default configuration
```

### 🌉 Proxy Management & Model Relay

Inspect and control daemon-managed per-app proxy routes for supported apps.

**Features:** independent enable/disable per app, per-app listen ports, daemon-managed workers, current route inspection, dashboard telemetry, token accounting, and foreground serve mode for debugging.

The local proxy can route Claude Code, Codex, and Gemini through CC-Switch, adapt OpenAI Responses API and Chat Completions providers, and connect mainstream OpenAI-compatible models such as DeepSeek, Kimi, Qwen, OpenRouter, xAI, Groq, and Mistral where the target app supports that route.

```bash
cc-switch proxy show                              # Show proxy configuration, routes, and daemon worker status
cc-switch proxy enable                            # Enable the Claude proxy route (default app)
cc-switch --app codex proxy enable                # Enable the Codex proxy route
cc-switch --app gemini proxy disable              # Disable the Gemini proxy route
cc-switch --app claude proxy config --listen-port 15721
cc-switch --app codex proxy config --listen-port 15722
cc-switch proxy serve --takeover claude           # Foreground debug mode; refused while daemon-managed routes are active
```

Normal CLI/TUI proxy enable/disable actions are routed through the daemon. The daemon auto-starts when the first app proxy route is activated, runs one worker per active supported app (Claude, Codex, Gemini), and exits automatically when no proxy routes remain active.

> **Platform support:** The daemon-managed proxy relies on a Unix-domain-socket supervisor and is available **only on macOS and Linux**. On Windows, `proxy enable` / `proxy disable` and the `daemon` subcommand are unavailable and fail with `managed sessions are only supported on unix`. To run the local proxy on Windows, use the foreground mode instead, which starts the relay without the supervisor:
>
> ```bash
> cc-switch proxy serve --takeover claude
> ```
>
> `proxy show` and `proxy config` work on all platforms. See [#294](https://github.com/SaladDay/cc-switch-cli/issues/294).

### 🧪 Environment & Local Tools

Inspect environment conflicts and whether required local CLIs are installed.

```bash
cc-switch env check                  # Check environment conflicts
cc-switch env list                   # List relevant environment variables
cc-switch env tools                  # Check Claude/Codex/Gemini/OpenCode/Hermes/OpenClaw CLIs
```

### 🌐 Multi-language Support

Interactive mode supports English and Chinese, language settings are automatically saved.

- Default language: English
- Go to `⚙️ Settings` menu to switch language

### 🔧 Utilities

Shell completions, environment management, and other utilities.

```bash
# Shell completions
cc-switch completions install --activate   # Recommended: install + activate for bash/zsh
cc-switch completions install              # Conservative: install only, no rc edits
cc-switch completions status               # Inspect managed completion status
cc-switch completions uninstall            # Remove managed completion assets
cc-switch completions bash                 # Compatibility raw generator path
cc-switch completions fish                 # Raw generation still works for non-managed shells

# Environment management
cc-switch env check                  # Check for environment conflicts
cc-switch env list                   # List environment variables

# Self-update
cc-switch update                     # Update to latest release
cc-switch update --version vX.Y.Z    # Update to a specific version
```

Automated install/activation currently targets `bash` and `zsh` only. Other shells remain available through the raw generator path, for example `cc-switch completions fish`.

---

## 🏗️ Architecture

### Core Design

- **SQLite-backed state**: Core data lives in `~/.cc-switch/cc-switch.db` by default (or under `$CC_SWITCH_CONFIG_DIR/` when set); legacy `config.json` is kept only for older import and migration paths
- **Skills SSOT**: Skill source files live in `~/.cc-switch/skills/` by default (or under `$CC_SWITCH_CONFIG_DIR/skills/` when set), while install state and app enablement stay in the database
- **Safe Live Sync (Default)**: Skip writing live files for apps that haven't been initialized yet (prevents creating `~/.claude`, `~/.codex`, `~/.gemini`, `~/.config/opencode`, `~/.hermes`, or `~/.openclaw` unexpectedly)
- **Atomic Writes**: Temp file + rename pattern prevents corruption
- **Service Layer Reuse**: 100% reused from original GUI version
- **Concurrency Safe**: RwLock with scoped guards

### Configuration Files

**CC-Switch Storage** (default: `~/.cc-switch`, override: `CC_SWITCH_CONFIG_DIR`):
- `~/.cc-switch/cc-switch.db` - Main database for providers, MCP, prompts, and app state
- `~/.cc-switch/settings.json` - Settings
- `~/.cc-switch/skills/` - Installed skill sources (SSOT)
- `~/.cc-switch/backups/` - Auto-rotation (keep 10)
- `~/.cc-switch/config.json` - Legacy JSON kept for compatibility and import flows

When `CC_SWITCH_CONFIG_DIR` is set, CC-Switch uses that directory as its config root; existing data under `~/.cc-switch` is not migrated automatically.

**Live Configs:**
- Claude: `~/.claude/settings.json` (provider/common config), `~/.claude.json` (MCP), `~/.claude/CLAUDE.md` (prompts)
- Codex: `~/.codex/auth.json` (auth state), `~/.codex/config.toml` (provider/common config + MCP), `~/.codex/AGENTS.md` (prompts)
  - Codex config directory uses CC-Switch's manual override first. If no override is configured, CC-Switch follows Codex's `$CODEX_HOME` when it points to an existing directory, otherwise it uses `$HOME/.codex`.
- Gemini: `~/.gemini/.env` (provider env), `~/.gemini/settings.json` (settings + MCP), `~/.gemini/GEMINI.md` (prompts)
- OpenCode: `~/.config/opencode/opencode.json` (providers + MCP + runtime config), `~/.config/opencode/AGENTS.md` (prompts)
- Hermes: `~/.hermes/config.yaml` (providers + MCP + memory settings), `~/.hermes/AGENTS.md` (prompts), `~/.hermes/skills/` (skills), `~/.hermes/memories/` (memory)
- OpenClaw: `~/.openclaw/openclaw.json` (providers + env/tools/agents defaults), `~/.openclaw/AGENTS.md` (prompts)

---

## ❓ FAQ (Frequently Asked Questions)

<details>
<summary><b>Why doesn't my configuration take effect after switching providers?</b></summary>

<br>

First, make sure the target CLI has been initialized at least once (i.e. its config directory exists). CC-Switch may skip live sync for uninitialized apps; you will see a warning. Run the target CLI once (e.g. `claude --help`, `codex --help`, `gemini --help`, `opencode --help`, `openclaw --help`) or create `~/.hermes` for Hermes, then switch again.

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
<summary><b>Proxy startup fails with `Address already in use`. What should I do?</b></summary>

<br>

This means another process is already listening on the proxy port. A common case after upgrading or debugging is that an old `cc-switch daemon` / `cc-switch proxy serve` process is still running in the background, but the new process did not attach to it.

First check the current proxy port with `cc-switch proxy show`, for example `configured 15722`.

**macOS / Linux:**

```bash
# See which process owns the port. Replace 15722 with your proxy port.
lsof -nP -iTCP:15722 -sTCP:LISTEN

# List cc-switch processes and identify the daemon / proxy worker.
ps -axo pid,ppid,stat,command | grep '[c]c-switch'

# If the daemon is reachable, stop it cleanly first.
cc-switch daemon stop

# If the daemon is not reachable but the port is still occupied, terminate the matching PIDs.
kill <worker-pid> <daemon-pid>

# If they still do not exit, force terminate them.
kill -9 <worker-pid> <daemon-pid>
```

Only terminate processes that are clearly shown as `cc-switch daemon start` or `cc-switch proxy serve`. Do not kill unrelated apps just because they use a nearby port.

**Windows:**

```powershell
netstat -ano | findstr :15722
taskkill /PID <pid> /F
```

Then restart:

```bash
cc-switch proxy show
cc-switch
```

</details>

<details>
<summary><b>Which apps are supported?</b></summary>

<br>

CC-Switch currently supports six AI coding assistants:
- **Claude Code** (`--app claude`, default)
- **Codex** (`--app codex`)
- **Gemini** (`--app gemini`)
- **OpenCode** (`--app opencode`)
- **Hermes** (`--app hermes`)
- **OpenClaw** (`--app openclaw`)

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
