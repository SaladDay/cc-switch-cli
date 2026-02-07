<div align="center">

# CC-Switch CLI

[![Version](https://img.shields.io/badge/version-4.7.1-blue.svg)](https://github.com/saladday/cc-switch-cli/releases)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg)](https://github.com/saladday/cc-switch-cli/releases)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

**Claude Code、Codex 与 Gemini CLI 的命令行管理工具**

统一管理 Claude Code、Codex 与 Gemini CLI 的供应商配置、MCP 服务器、Skills 扩展和系统提示词。

[English](README.md) | [中文](README_ZH.md)

</div>

---

## 📖 关于本项目

本项目是原版 [CC-Switch](https://github.com/farion1231/cc-switch) 的 **CLI 分支**。


**致谢：** 原始架构和核心功能来自 [farion1231/cc-switch](https://github.com/farion1231/cc-switch)

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

<details>
  <summary>旧版 UI（暂不维护）</summary>

> [!WARNING]
> 旧版交互 UI 已暂时停止维护，请以新版 TUI 为准。

<table>
  <tr>
    <th>交互式主界面</th>
    <th>供应商管理</th>
  </tr>
  <tr>
    <td><img src="assets/screenshots/main-ch.png" alt="旧版主界面" width="100%"/></td>
    <td><img src="assets/screenshots/add-ch.png" alt="旧版供应商管理" width="100%"/></td>
  </tr>
</table>

</details>

---

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
cc-switch mcp sync                   # 同步 MCP 服务器

# 使用全局 `--app` 参数来指定目标应用：
cc-switch --app claude provider list    # 管理 Claude 供应商
cc-switch --app codex mcp sync          # 同步 Codex MCP 服务器
cc-switch --app gemini prompts list     # 列出 Gemini 提示词

# 支持的应用：`claude`（默认）、`codex`、`gemini`
```

完整命令列表请参考下方「功能特性」章节。

---

## ✨ 功能特性

### 🔌 供应商管理

管理 **Claude Code**、**Codex** 和 **Gemini** 的 API 配置。

**功能：** 一键切换、多端点支持、API 密钥管理、速度测试、供应商复制。

```bash
cc-switch provider list              # 列出所有供应商
cc-switch provider current           # 显示当前供应商
cc-switch provider switch <id>       # 切换供应商
cc-switch provider add               # 添加新供应商
cc-switch provider edit <id>         # 编辑现有供应商
cc-switch provider duplicate <id>    # 复制供应商
cc-switch provider delete <id>       # 删除供应商
cc-switch provider speedtest <id>    # 测试 API 延迟
```

#### PackyCode 预设（赞助商）

在新版 TUI 的「添加供应商」表单中，可选择 `★ PackyCode` 模板自动填充端点：

- 官网：`https://www.packyapi.com`
- 注册链接：`https://www.packyapi.com/register?aff=cc-switch-cli`
- 优惠码：`cc-switch-cli`（9 折）
- Claude（Anthropic）：`https://www.packyapi.com`
- Codex（OpenAI）：`https://www.packyapi.com/v1`
- Gemini：`https://www.packyapi.com`

### 🛠️ MCP 服务器管理

跨 Claude/Codex/Gemini 管理模型上下文协议服务器。

**功能：** 统一管理、多应用支持、三种传输类型（stdio/http/sse）、自动同步、智能 TOML 解析器。

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

**跨应用支持：** Claude (`CLAUDE.md`)、Codex (`AGENTS.md`)、Gemini (`GEMINI.md`)。

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

通过社区技能扩展 Claude Code/Codex/Gemini 的能力。

**功能：** SSOT 技能仓库、多应用启用/禁用、同步到应用目录、扫描/导入未管理技能、仓库发现。

```bash
cc-switch skills list                # 列出已安装技能
cc-switch skills search <query>      # 搜索可用技能
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
```

### ⚙️ 配置管理

管理配置文件的备份、导入和导出。

**功能：** 自定义备份命名、交互式备份选择、自动轮换（保留 10 个）、导入/导出。

```bash
cc-switch config show                # 显示配置
cc-switch config path                # 显示配置文件路径
cc-switch config validate            # 验证配置文件

# 通用配置片段（跨所有供应商共享设置）
cc-switch --app claude config common show
cc-switch --app claude config common set --json '{"env":{"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":1},"includeCoAuthoredBy":false}' --apply
cc-switch --app claude config common clear --apply

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

cc-switch config reset               # 重置为默认配置
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
cc-switch update --version v4.7.1    # 更新到指定版本
```

---

## 📥 安装

### 方法 1：下载预编译二进制（推荐）

从 [GitHub Releases](https://github.com/saladday/cc-switch-cli/releases) 下载最新版本。

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

## 🏗️ 架构

### 核心设计

- **SSOT**：所有配置存于 `~/.cc-switch/config.json`，实时配置是生成的产物
- **安全 Live 同步（默认）**：若目标应用尚未初始化，将跳过写入 live 文件（避免意外创建 `~/.claude`、`~/.codex`、`~/.gemini`）
- **原子写入**：临时文件 + 重命名模式防止损坏
- **服务层复用**：100% 复用原 GUI 版本
- **并发安全**：RwLock 配合作用域守卫

### 配置文件

**CC-Switch 存储：**
- `~/.cc-switch/config.json` - 主配置（SSOT）
- `~/.cc-switch/settings.json` - 设置
- `~/.cc-switch/backups/` - 自动轮换（保留 10 个）

**实时配置：**
- Claude: `~/.claude/settings.json`, `~/.claude.json` (MCP), `~/.claude/CLAUDE.md` (提示词)
- Codex: `~/.codex/auth.json`, `~/.codex/config.toml` (MCP), `~/.codex/AGENTS.md` (提示词)
- Gemini: `~/.gemini/.env`, `~/.gemini/settings.json` (MCP), `~/.gemini/GEMINI.md` (提示词)

---

## ❓ 常见问题 (FAQ)

<details>
<summary><b>为什么切换供应商后配置没有生效？</b></summary>

<br>

首先确认目标 CLI 已经至少运行过一次（即对应配置目录已存在）。如果应用未初始化，CC-Switch 会出于安全原因跳过写入 live 文件，并提示一条 warning。请先运行一次目标 CLI（例如 `claude --help` / `codex --help` / `gemini --help`），然后再切换一次供应商。

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

CC-Switch 目前支持三个 AI 编程助手：
- **Claude Code** (`--app claude`，默认)
- **Codex** (`--app codex`)
- **Gemini** (`--app gemini`)

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
│   ├── commands/          # CLI 子命令（provider, mcp, prompts, config）
│   ├── interactive/       # 交互式 TUI 模式
│   └── ui.rs              # UI 实用工具（表格、颜色）
├── services/              # 业务逻辑
├── main.rs                # CLI 入口点
└── ...
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
