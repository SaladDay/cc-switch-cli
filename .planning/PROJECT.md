# CC-Switch CLI

## What This Is

CC-Switch CLI 是一个 Rust 命令行管理工具，面向使用多个 AI 编程助手（Claude Code、Codex、Gemini、OpenCode、Hermes、OpenClaw）的开发者。它统一管理 provider 配置、MCP 服务器、prompts、skills、WebDAV 同步、本地代理路由、故障转移和守护进程，让开发者在不同 AI 工具之间自由切换 provider 而无需手动修改各工具的配置文件。

## Core Value

**一键切换 AI 编程工具的底层 provider，零配置摩擦。** 如果这做不到，其他功能都无意义。

## Requirements

### Validated

- ✓ Multi-app provider 切换（Claude/Codex/Gemini/OpenCode/Hermes/OpenClaw）— v5.x
- ✓ 本地 HTTP 代理（Axum），支持请求转发和 provider 路由 — v5.x
- ✓ SQLite 持久化存储，DAO 模式 — v5.x
- ✓ TUI 交互界面（ratatui）— v5.x
- ✓ 故障转移队列（failover）— v5.x
- ✓ WebDAV/S3 配置同步 — v5.x
- ✓ Unix 守护进程模式 — v5.x
- ✓ MCP 服务器管理 — v5.x
- ✓ Prompts/Skills 管理 — v5.x

### Active

- [ ] Per-model provider routing：根据请求的 model 名称（如 `*sonnet*`）将代理请求路由到不同的 provider

### Out of Scope

- 新增 AI 编程工具支持（目前 6 个已覆盖）
- 多用户/多租户支持 — 当前为单用户本地工具
- Windows 服务集成 — 当前仅 Unix daemon

## Context

- **代码库规模**: Rust ~80K+ 行，SQLite 单文件数据库，ratatui TUI
- **Schema 版本**: 当前 v10（已支持 Hermes Agent）
- **上游参考**: [cc-switch PR #4081](https://github.com/farion1231/cc-switch/pull/4081) 已实现 per-model routing 的后端（Rust）+ 前端（React）
- **关键差异**: cc-switch-cli 无 React 前端，需用 ratatui TUI 重新实现管理界面；代理架构可能有细节差异需要适配
- **CI**: GitHub Actions 运行 `cargo fmt --check`、单元测试、集成测试
- **已知技术债务**: `services/proxy.rs` 7085 行单体文件、513 处 `unwrap()` 调用、DB 单 Mutex 瓶颈

## Constraints

- **Tech stack**: Rust 2021 Edition，MSRV 1.91.1，tokio + axum + ratatui + rusqlite
- **Compatibility**: Schema v10→v11 升级路径必须平滑，向后兼容空路由表
- **No frontend**: 无 React/Web 前端，所有管理 UI 通过 CLI 子命令或 TUI 实现
- **Testing**: 需覆盖数据库迁移、路由匹配逻辑、代理集成
- **PR quality**: 最终产出应是可直接合并的纯净 Git 分支（仅包含功能代码，无 .planning/）

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| 参考 cc-switch PR #4081 作为上游设计 | PR 已经过 review，后端设计可复用 | — Pending |
| 使用 TUI 替代 React 前端 | cc-switch-cli 无前端，需用 ratatui 构建管理界面 | — Pending |
| Schema 升级到 v11 | 需要新 `model_routes` 表存储路由规则 | — Pending |

---
*Last updated: 2026-06-11 after milestone init*
