# State: CC-Switch CLI

**Last updated:** 2026-06-11
**Active milestone:** Milestone 1 — Per-Model Provider Routing
**Current phase:** Phase 1 (planned, ready to execute)

## Project Reference

See: `.planning/PROJECT.md` (updated 2026-06-11)

**Core value:** 一键切换 AI 编程工具的底层 provider，零配置摩擦
**Current focus:** 实现 per-model provider routing（根据模型名称将代理请求路由到不同 provider）

## Milestone Progress

| Phase | Status | Est. Effort | Started | Completed |
|-------|--------|-------------|---------|-----------|
| Phase 1: Database | 📋 Planned | 2-3h | — | — |
| Phase 2: Router Engine | ⬜ Pending | 4-6h | — | — |
| Phase 3: CLI Commands | ⬜ Pending | 1-2h | — | — |
| Phase 4: TUI Interface | ⬜ Pending | 6-10h | — | — |
| Phase 5: Sync Integration | ⬜ Pending | 0.5-1h | — | — |
| Phase 6: Testing & PR Prep | ⬜ Pending | 3-5h | — | — |

## Reference Artifacts

- Codebase map: `.planning/codebase/` (7 documents, 2391 lines, generated 2026-06-11)
- Phase 1 Research: `.planning/phase-1/RESEARCH.md`
- Phase 1 Plan: `.planning/phases/01-database/01-01-PLAN.md` (1 plan, 3 tasks, 1 wave)

## Working State

- **Branch:** `main` (clean)
- **Last commit:** `b085799 docs(01-database): create phase plan`
- **Schema version:** v10

## Quick Start (Next Session)

```bash
# Execute Phase 1:
/gsd-execute-phase 01-database
```

## Notes

- 上游 PR #4081 于 2026-06-11 提交，当前状态 OPEN，有一次 codex review 但无实质性修改要求
- cc-switch-cli 与 cc-switch 的关键差异：无 React 前端、ratatui TUI、代理架构细节可能不同
- Phase 4 (TUI) 是最大的工作量来源（35-40%），取决于现有 TUI 组件的复用程度
