<div align="center">

# CC-Switch CLI

[![Version](https://img.shields.io/badge/version-5.3.1-blue.svg)](https://github.com/saladday/cc-switch-cli/releases)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg)](https://github.com/saladday/cc-switch-cli/releases)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

**Claude Code、Codex、Gemini、OpenCode 与 OpenClaw 的命令行管理工具**

统一管理 Claude Code、Codex、Gemini、OpenCode 与 OpenClaw 的供应商配置，并按应用提供 MCP 服务器、Skills 扩展、提示词、本地代理路由和环境检查等能力。

[English](README.md) | 中文

</div>

---

## 📖 关于本项目

本项目是原版 [CC-Switch](https://github.com/farion1231/cc-switch) 的 **CLI 分支**。🔄 WebDAV 同步功能与上游项目完全兼容。


**致谢：** 原始架构和核心功能来自 [farion1231/cc-switch](https://github.com/farion1231/cc-switch)

---

## 🆕 5.3.1 更新内容

<div align="center">
  <h3><strong>🩹 面向 Codex 官方认证安全、代理热切换和新版数据库导入的补丁版本</strong></h3>
</div>

- 保留 Codex 官方 provider 的认证快照，避免编辑或切回官方渠道时把已有登录状态写丢。
- 在 takeover 已运行时，切换 provider 会立即更新代理目标，不再需要手动重启 proxy。
- 支持导入并升级更新后的 schema v8 数据库，补齐当前后端所需的分阶段迁移路径。
- 让迁移后的数据库与新建数据库在定价修正和新增跟踪表上保持一致。
- 详细补丁说明见：[release-note-v5.3.1-zh.md](docs/release-note-v5.3.1-zh.md)

---

## ❤️赞助商

<table>
  <tr>
    <td width="180">
      <a href="https://www.packyapi.com/register?aff=cc-switch-cli">
        <img src="assets/partners/logos/packycode.png" alt="PackyCode" width="150">
      </a>
    </td>
    <td>
      感谢 <b>PackyCode</b> 赞助本项目！<br/>
      官网：<a href="https://www.packyapi.com">https://www.packyapi.com</a><br/>
      CC-Switch CLI 专属优惠：通过
      <a href="https://www.packyapi.com/register?aff=cc-switch-cli">此链接</a>
      注册，并在充值时填写优惠码 <code>cc-switch-cli</code>，即可享受 <b>9 折优惠</b>。
    </td>
  </tr>
  <tr>
    <td width="180">
      <a href="https://www.aicodemirror.com/register?invitecode=77V9EA">
        <img src="assets/partners/logos/aicodemirror.png" alt="AICodeMirror" width="150">
      </a>
    </td>
    <td>
      感谢 <b>AICodeMirror</b> 赞助本项目！<b>AICodeMirror</b> 提供 Claude Code / Codex / Gemini CLI 官方高稳定中转服务，支持企业级并发、快速开票与 7x24 专属技术支持。Claude Code / Codex / Gemini 官方通道价格低至原价的 <b>38% / 2% / 9%</b>，充值另有折上折！<br/>
      <b>AICodeMirror</b> 为 cc-switch-cli 用户提供专属福利：通过<a href="https://www.aicodemirror.com/register?invitecode=77V9EA">此链接</a>注册，首充可享 <b>8 折</b>，即 <b>20% off</b>，企业客户最高可享 <b>75 折</b>，即 <b>25% off</b>。
    </td>
  </tr>
  <tr>
    <td width="180">
      <a href="https://www.right.codes/register?aff=ccswitch-cli">
        <img src="assets/partners/logos/rightcode.jpg" alt="RightCode" width="150">
      </a>
    </td>
    <td>
      感谢 <b>RightCode</b> 赞助本项目！<br/>
      RightCode 为 Claude Code、Codex、Gemini 等模型提供稳定的路由服务，拥有高性价比的 Codex 月付方案，且<b>支持额度滚存——当天未用完的额度可顺延至次日使用。</b><br/>
      RightCode 为 CC-Switch CLI 用户提供了特别优惠：通过<a href="https://www.right.codes/register?aff=ccswitch-cli">此链接</a>注册，每次充值均可获得实付金额 <b>25%</b> 的按量额度！
    </td>
  </tr>
</table>

---

## 📸 截图预览

<div align="center">
  <h3>首页</h3>
  <img src="assets/screenshots/home-zh.png" alt="首页" width="70%"/>
</div>

<br/>

<table>
  <tr>
    <th>切换</th>
    <th>设置</th>
  </tr>
  <tr>
    <td><img src="assets/screenshots/switch-zh.png" alt="切换" width="100%"/></td>
    <td><img src="assets/screenshots/settings-zh.png" alt="设置" width="100%"/></td>
  </tr>
</table>

## 🚀 快速开始

**交互模式（推荐）**
```bash
cc-switch
```
🤩 按照屏幕菜单探索功能。

**命令行模式**
```bash
cc-switch provider list              # 列出供应商
cc-switch provider switch <id>       # 切换供应商
cc-switch provider export <id>       # 导出 Claude 供应商为独立 settings 文件
cc-switch provider stream-check <id> # 检查供应商流式健康
cc-switch config webdav show         # 查看 WebDAV 同步设置
cc-switch env tools                  # 检查本地 CLI 工具
cc-switch mcp sync                   # 同步 MCP 服务器
cc-switch proxy show                 # 查看代理路由和状态

# 使用全局 `--app` 参数来指定目标应用：
cc-switch --app claude provider list    # 管理 Claude 供应商
cc-switch --app codex mcp sync          # 同步 Codex MCP 服务器
cc-switch --app gemini prompts list     # 列出 Gemini 提示词
cc-switch --app openclaw provider list  # 管理 OpenClaw 供应商

# 支持的应用：`claude`（默认）、`codex`、`gemini`、`opencode`、`openclaw`
```

完整命令列表请参考「功能特性」章节。

---

## 📥 安装

### 方法 1：快速安装（macOS / Linux）

> Windows 用户请参考下方手动安装。

```bash
curl -fsSL https://github.com/SaladDay/cc-switch-cli/releases/latest/download/install.sh | bash
```

默认安装到 `~/.local/bin`。设置 `CC_SWITCH_INSTALL_DIR` 可自定义安装目录。

- 如果目标文件已存在，安装脚本会在 TTY 中提示确认；在非交互环境中，只有设置 `CC_SWITCH_FORCE=1` 才会覆盖。
- Linux 如需 glibc 构建，可设置 `CC_SWITCH_LINUX_LIBC=glibc`。

<details>
<summary>手动安装</summary>

#### macOS

```bash
# 下载 Universal Binary（推荐，支持 Apple Silicon + Intel）
curl -LO https://github.com/saladday/cc-switch-cli/releases/latest/download/cc-switch-cli-darwin-universal.tar.gz

# 解压
tar -xzf cc-switch-cli-darwin-universal.tar.gz

# 添加执行权限
chmod +x cc-switch

# 移动到 PATH
sudo mv cc-switch /usr/local/bin/

# 如遇 "无法验证开发者" 提示
xattr -cr /usr/local/bin/cc-switch
```

#### Linux (x64)

```bash
# 下载
curl -LO https://github.com/saladday/cc-switch-cli/releases/latest/download/cc-switch-cli-linux-x64-musl.tar.gz

# 解压
tar -xzf cc-switch-cli-linux-x64-musl.tar.gz

# 添加执行权限
chmod +x cc-switch

# 移动到 PATH
sudo mv cc-switch /usr/local/bin/
```

#### Linux (ARM64)

```bash
# 适用于树莓派或 ARM 服务器
curl -LO https://github.com/saladday/cc-switch-cli/releases/latest/download/cc-switch-cli-linux-arm64-musl.tar.gz
tar -xzf cc-switch-cli-linux-arm64-musl.tar.gz
chmod +x cc-switch
sudo mv cc-switch /usr/local/bin/
```

#### Windows

```powershell
# 下载 zip 文件
# https://github.com/saladday/cc-switch-cli/releases/latest/download/cc-switch-cli-windows-x64.zip

# 解压后将 cc-switch.exe 移动到 PATH 目录，例如：
move cc-switch.exe C:\Windows\System32\

# 或者直接运行
.\cc-switch.exe
```

</details>

### 方法 2：从源码构建

**前提条件：**
- Rust 1.85+（[通过 rustup 安装](https://rustup.rs/)）

**构建：**
```bash
git clone https://github.com/saladday/cc-switch-cli.git
cd cc-switch-cli/src-tauri
cargo build --release

# 二进制位置：./target/release/cc-switch
```

**安装到系统：**
```bash
# macOS/Linux
sudo cp target/release/cc-switch /usr/local/bin/

# Windows
copy target\release\cc-switch.exe C:\Windows\System32\
```

---

## ✨ 功能特性

### 🔌 供应商管理

管理 **Claude Code**、**Codex**、**Gemini**、**OpenCode** 与 **OpenClaw** 的 API 配置。

**功能：** 一键切换、Claude 独立 settings 导出、多端点支持、API 密钥管理、远端模型发现，以及按应用提供的速度测试、流式健康检查等诊断能力。

```bash
cc-switch provider list              # 列出所有供应商
cc-switch provider current           # 显示当前供应商
cc-switch provider switch <id>       # 切换供应商
cc-switch provider add               # 添加新供应商
cc-switch provider edit <id>         # 编辑现有供应商
cc-switch provider duplicate <id>    # 复制供应商
cc-switch provider delete <id>       # 删除供应商
cc-switch provider export <id>       # 导出到当前目录 ./.claude/settings.local.json 并供 Claude 自动加载
cc-switch provider speedtest <id>    # 测试 API 延迟
cc-switch provider stream-check <id> # 执行流式健康检查
cc-switch provider fetch-models <id> # 拉取远端模型列表
cc-switch provider export <id> --output ~/.claude/settings-demo.json # 自定义 settings 文件路径
```

### 🛠️ MCP 服务器管理

跨 Claude、Codex、Gemini 与 OpenCode 管理模型上下文协议服务器。

**功能：** 统一管理、多应用支持、三种传输类型（stdio/http/sse）、自动同步，以及面向 TOML / JSON live 配置的格式适配。

```bash
cc-switch mcp list                   # 列出所有 MCP 服务器
cc-switch mcp add                    # 添加新 MCP 服务器（交互式）
cc-switch mcp edit <id>              # 编辑 MCP 服务器
cc-switch mcp delete <id>            # 删除 MCP 服务器
cc-switch mcp enable <id> --app claude   # 为特定应用启用
cc-switch mcp disable <id> --app claude  # 为特定应用禁用
cc-switch mcp validate <command>     # 验证命令在 PATH 中
cc-switch mcp sync                   # 同步到实时文件
cc-switch mcp import --app claude    # 从实时配置导入
```

### 💬 Prompts 管理

管理 AI 编码助手的系统提示词预设。

**跨应用支持：** Claude (`CLAUDE.md`)、Codex (`AGENTS.md`)、Gemini (`GEMINI.md`)、OpenCode (`AGENTS.md`)、OpenClaw (`AGENTS.md`)。

```bash
cc-switch prompts list               # 列出提示词预设
cc-switch prompts current            # 显示当前活动提示词
cc-switch prompts activate <id>      # 激活提示词
cc-switch prompts deactivate         # 停用当前激活的提示词
cc-switch prompts create             # 创建新提示词预设
cc-switch prompts edit <id>          # 编辑提示词预设
cc-switch prompts show <id>          # 显示完整内容
cc-switch prompts delete <id>        # 删除提示词
```

### 🎯 Skills 管理

通过社区技能扩展 Claude Code/Codex/Gemini/OpenCode 的能力。

**功能：** SSOT 技能仓库、多应用启用/禁用、同步到应用目录、扫描/导入未管理技能、仓库发现。

```bash
cc-switch skills list                # 列出已安装技能
cc-switch skills discover <query>      # 发现可用技能（别名：search）
cc-switch skills install <name>      # 安装技能
cc-switch skills uninstall <name>    # 卸载技能
cc-switch skills enable <name>       # 为当前应用启用（配合 --app）
cc-switch skills disable <name>      # 为当前应用禁用（配合 --app）
cc-switch skills info <name>         # 显示技能信息
cc-switch skills sync                # 同步已启用技能到应用目录
cc-switch skills sync-method [m]     # 查看/设置同步方式（auto|symlink|copy）
cc-switch skills scan-unmanaged      # 扫描未管理技能
cc-switch skills import-from-apps    # 导入未管理技能到 SSOT
cc-switch skills repos list          # 查看仓库列表
cc-switch skills repos add <repo>    # 添加仓库（owner/name[@branch] 或 GitHub URL）
cc-switch skills repos remove <repo> # 移除仓库（owner/name 或 GitHub URL）
cc-switch skills repos enable <repo> # 启用仓库但保留当前分支
cc-switch skills repos disable <repo> # 禁用仓库但保留当前分支
```

### ⚙️ 配置管理

管理配置文件的备份、导入和导出。

**功能：** 自定义备份命名、交互式备份选择、自动轮换（保留 10 个）、导入/导出、通用配置片段、WebDAV 同步。

```bash
cc-switch config show                # 显示配置
cc-switch config path                # 显示配置文件路径
cc-switch config validate            # 验证配置文件

# 通用配置片段（跨所有供应商共享设置）
# 会在适用时尝试刷新 live config（`--apply` 仅保留为兼容参数）
cc-switch --app claude config common show
cc-switch --app claude config common set --snippet '{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false}'
cc-switch --app claude config common clear

# 备份
cc-switch config backup              # 创建备份（自动命名）
cc-switch config backup --name my-backup  # 创建备份（自定义名称）

# 恢复
cc-switch config restore             # 交互式：从备份列表选择
cc-switch config restore --backup <id>    # 通过 ID 恢复特定备份
cc-switch config restore --file <path>    # 从外部文件恢复

# 导入/导出
cc-switch config export <path>       # 导出到外部文件
cc-switch config import <path>       # 从外部文件导入

# WebDAV 同步
cc-switch config webdav show
cc-switch config webdav set --base-url <url> --username <user> --password <password> --enable
cc-switch config webdav jianguoyun --username <user> --password <password>
cc-switch config webdav check-connection
cc-switch config webdav upload
cc-switch config webdav download
cc-switch config webdav migrate-v1-to-v2

cc-switch config reset               # 重置为默认配置
```

### 🌉 代理管理

查看并控制服务于各应用的本地多应用代理。

**功能：** 持久化开关、当前路由检查、首页遥测，以及用于调试的前台运行模式。

```bash
cc-switch proxy show                 # 显示代理配置和路由
cc-switch proxy enable               # 启用持久化代理开关
cc-switch proxy disable              # 禁用持久化代理开关
cc-switch proxy serve                # 以前台模式运行代理
```

### 🧪 环境与本地工具

检查环境变量冲突，以及 Claude/Codex/Gemini/OpenCode CLI 是否已经装好。

```bash
cc-switch env check                  # 检查环境变量冲突
cc-switch env list                   # 列出相关环境变量
cc-switch env tools                  # 检查 Claude/Codex/Gemini/OpenCode CLI
```

### 🌐 多语言支持

交互模式支持中英文切换，语言设置会自动保存。

- 默认语言：English
- 进入 `⚙️ 设置` 菜单切换语言

### 🔧 实用工具

Shell 补全、环境管理等实用功能。

```bash
# Shell 补全
cc-switch completions <shell>        # 生成 shell 补全（bash/zsh/fish/powershell）

# 环境管理
cc-switch env check                  # 检查环境冲突
cc-switch env list                   # 列出环境变量

# 自更新
cc-switch update                     # 更新到最新版本
cc-switch update --version vX.Y.Z    # 更新到指定版本
```

---

## 🏗️ 架构

### 核心设计

- **SQLite 持久化**：核心数据默认存放在 `~/.cc-switch/cc-switch.db`（若设置 `CC_SWITCH_CONFIG_DIR` 则改为该目录下）；旧版 `config.json` 仅保留给兼容与迁移路径使用
- **Skills SSOT**：技能源文件默认保存在 `~/.cc-switch/skills/`（若设置 `CC_SWITCH_CONFIG_DIR` 则改为 `$CC_SWITCH_CONFIG_DIR/skills/`），安装状态和启用状态由数据库统一记录
- **安全 Live 同步（默认）**：若目标应用尚未初始化，将跳过写入 live 文件（避免意外创建 `~/.claude`、`~/.codex`、`~/.gemini`、`~/.config/opencode` 或 `~/.openclaw`）
- **原子写入**：临时文件 + 重命名模式防止损坏
- **服务层复用**：100% 复用原 GUI 版本
- **并发安全**：RwLock 配合作用域守卫

### 配置文件

**CC-Switch 存储**（默认：`~/.cc-switch`，可用 `CC_SWITCH_CONFIG_DIR` 覆盖）：
- `~/.cc-switch/cc-switch.db` - 供应商、MCP、提示词和应用状态的主数据库
- `~/.cc-switch/settings.json` - 设置
- `~/.cc-switch/skills/` - 已安装技能源码（SSOT）
- `~/.cc-switch/backups/` - 自动轮换（保留 10 个）
- `~/.cc-switch/config.json` - 为兼容与导入流程保留的旧版 JSON

设置 `CC_SWITCH_CONFIG_DIR` 后，CC-Switch 会改用该目录作为配置根目录；这不会自动迁移 `~/.cc-switch` 中的现有数据。

**实时配置：**
- Claude: `~/.claude/settings.json`（供应商 / 通用配置）, `~/.claude.json`（MCP）, `~/.claude/CLAUDE.md`（提示词）
- Codex: `~/.codex/auth.json`（认证状态）, `~/.codex/config.toml`（供应商 / 通用配置 + MCP）, `~/.codex/AGENTS.md`（提示词）
- Gemini: `~/.gemini/.env`（供应商环境变量）, `~/.gemini/settings.json`（设置 + MCP）, `~/.gemini/GEMINI.md`（提示词）
- OpenCode: `~/.config/opencode/opencode.json`（供应商 + MCP + 运行时配置）, `~/.config/opencode/AGENTS.md`（提示词）
- OpenClaw: `~/.openclaw/openclaw.json`（供应商 + Env/Tools/Agents Defaults）, `~/.openclaw/AGENTS.md`（提示词）

---

## ❓ 常见问题 (FAQ)

<details>
<summary><b>为什么切换供应商后配置没有生效？</b></summary>

<br>

首先确认目标 CLI 已经至少运行过一次（即对应配置目录已存在）。如果应用未初始化，CC-Switch 会出于安全原因跳过写入 live 文件，并提示一条 warning。请先运行一次目标 CLI（例如 `claude --help` / `codex --help` / `gemini --help` / `opencode --help` / `openclaw --help`），然后再切换一次供应商。

这通常是由**环境变量冲突**引起的。如果你在系统环境变量中设置了 API 密钥（如 `ANTHROPIC_API_KEY`、`OPENAI_API_KEY`），它们会覆盖 CC-Switch 的配置。

**解决方案：**

1. 检查冲突：
   ```bash
   cc-switch env check --app claude
   ```

2. 列出所有相关环境变量：
   ```bash
   cc-switch env list --app claude
   ```

3. 如果发现冲突，手动删除它们：
   - **macOS/Linux**：编辑 shell 配置文件（`~/.bashrc`、`~/.zshrc` 等）
     ```bash
     # 找到环境变量所在行并删除
     nano ~/.zshrc
     # 或使用你喜欢的编辑器：vim、code 等
     ```
   - **Windows**：打开系统属性 → 环境变量，删除冲突的变量

4. 重启终端使更改生效。

</details>

<details>
<summary><b>支持哪些应用？</b></summary>

<br>

CC-Switch 目前支持五个 AI 编程助手：
- **Claude Code** (`--app claude`，默认)
- **Codex** (`--app codex`)
- **Gemini** (`--app gemini`)
- **OpenCode** (`--app opencode`)
- **OpenClaw** (`--app openclaw`)

使用全局 `--app` 参数指定要管理的应用：
```bash
cc-switch --app codex provider list
```

</details>

<details>
<summary><b>如何报告 bug 或请求新功能？</b></summary>

<br>

请在我们的 [GitHub Issues](https://github.com/saladday/cc-switch-cli/issues) 页面提交问题，并包含：
- 问题或功能请求的详细描述
- 复现步骤（针对 bug）
- 你的系统信息（操作系统、版本）
- 相关日志或错误信息

</details>

---

## 🛠️ 开发

### 环境要求

- **Rust**：1.85+（[rustup](https://rustup.rs/)）
- **Cargo**：与 Rust 捆绑

### 开发命令

```bash
cd src-tauri

cargo run                            # 开发模式
cargo run -- provider list           # 运行特定命令
cargo build --release                # 构建 release

cargo fmt                            # 代码格式化
cargo clippy                         # 代码检查
cargo test                           # 运行测试
```

### 代码结构

```
src-tauri/src/
├── cli/
│   ├── commands/          # CLI 子命令（provider, mcp, prompts, skills, proxy, env, ...）
│   ├── tui/               # 交互式 TUI 模式（ratatui）
│   ├── interactive/       # 交互入口 / TTY 检查
│   └── ui/                # UI 实用工具（表格、颜色）
├── services/              # 业务逻辑（provider, mcp, prompt, webdav, ...）
├── database/              # SQLite 存储、迁移、备份
├── main.rs                # CLI 入口点
└── ...                    # 各应用配置、代理、错误处理
```


## 🤝 贡献

欢迎贡献！本分支专注于 CLI 功能。

**提交 PR 前：**
- ✅ 通过格式检查：`cargo fmt --check`
- ✅ 通过代码检查：`cargo clippy`
- ✅ 通过测试：`cargo test`
- 💡 先开 issue 讨论

---

## 📜 许可证

- MIT © 原作者：Jason Young
- CLI 分支维护者：saladday
