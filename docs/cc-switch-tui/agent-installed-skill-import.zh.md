# Agent 已安装 Skill 导入流程

最后更新：2026-05-13

本文档记录 Skills 页面新增的 `s` 导入流程：从当前 agent 自己已经安装的
skills 中读取可导入项，弹出选择对话框，确认后同步到 cc-switch-tui 管理的
skill 单一来源目录和数据库记录中。本文用于后续维护；如果行为变化，以源码为最终依据。

## 使用方式

入口：TUI 的 Skills 页面。

操作流程：

1. 进入 Skills 页面。
2. 按 `s`，打开 `Import Agent Skills` 对话框。
3. 对话框会列出当前 agent skill 目录中发现、且尚未被 cc-switch-tui 管理的 skills。
4. 默认会全选所有可导入项。
5. 用 `Up` / `Down` 移动选中行。
6. 用 `Space` 或 `x` 切换当前行是否导入。
7. 按 `i` 或 `Enter` 确认导入。
8. 按 `Esc` 取消。
9. 在对话框中按 `r` 重新扫描 agent 已安装 skills。

如果没有找到可导入项，会关闭对话框并显示提示：

- agent skill 目录不存在；
- agent skill 目录为空；
- skill 已经被 cc-switch-tui 管理；
- skill 目录名以 `.` 开头，被视为隐藏目录。

导入完成后，skills 会出现在 cc-switch-tui 的已安装 skill 列表里。对于从具体
agent 工具目录发现的 skill，导入会同时保留它已经安装到该工具的事实，例如
`~/.hermes/skills/foo` 会让 `foo` 的 Hermes 启用状态变为开启。对于通用
`~/.agents/skills` 来源，如果没有同时在某个具体工具目录中发现同名 skill，则只纳入
cc-switch-tui 管理，不会自动启用到某个 app。

## 扫描来源

源码入口：`src-tauri/src/services/skill.rs`

`SkillService::scan_agent_installed()` 扫描当前支持 skills 的 agent 工具目录，以及
通用 agent skill 目录。

当前扫描来源按优先级为：

1. `~/.agents/skills`
2. 已支持 app 的 skill 目录：
   - Claude：优先 `$CLAUDE_CONFIG_DIR/skills`，没有可扫描 skills 目录时回退到
     cc-switch settings override 或 `~/.claude/skills`
   - Codex：优先 `$CODEX_HOME/skills`，没有可扫描 skills 目录时回退到
     cc-switch settings override 或 `~/.codex/skills`
   - Hermes：优先 `$HERMES_HOME/skills`，没有可扫描 skills 目录时回退到
     cc-switch settings override 或 `~/.hermes/skills`
   - `~/.gemini/skills`
   - `~/.config/opencode/skills`
   - `~/.openclaw/skills`

如果两个来源路径相同，只保留一个来源，避免重复扫描。

每个来源下只读取一层子目录。一个子目录会被当作 skill 候选项，目录中的
`SKILL.md` 用于读取展示名称和描述；如果读取不到元数据，则使用目录名和空描述作为
fallback。

扫描结果按 skill 名称排序。多个来源中出现同名目录时会去重为一条记录，并把来源标签
合并到 `found_in` 中。

来源标签：

- `agents`：来自 `~/.agents/skills`
- `claude`、`codex`、`gemini`、`opencode`、`openclaw`、`hermes`：来自对应 app 的
  skill 目录

## 过滤规则

扫描时会先读取 cc-switch-tui 当前管理的 skill index。

下列目录不会出现在导入对话框中：

- 非目录条目；
- 目录名以 `.` 开头的隐藏目录；
- 根目录下没有 `SKILL.md` 的目录，例如 Hermes 的分类目录；
- Hermes `.bundled_manifest` 中声明的内置技能；
- 目录名已经存在于 cc-switch-tui 管理记录中的 skill；
- 读取目录失败的条目。

这里的去重和过滤以目录名为 key。也就是说，同名目录会合并成一条导入候选，并把发现
来源合并到 `found_in`。如果 cc-switch-tui 已经管理了同名 directory，agent 导入不会再
提示或重复导入它。

## 导入逻辑

源码入口：

- TUI action：`Action::SkillsOpenAgentImport`
- 打开对话框：`open_agent_skills_import_picker()`
- 确认导入：`Action::SkillsImportFromAgent`
- 服务层导入：`SkillService::import_from_agent()`

确认导入后，流程如下：

1. 重新读取 cc-switch-tui skill index。
2. 解析 `~/.agents/.skill-lock.json`，得到可用的 GitHub repo 元数据。
3. 确保 SSOT 目录存在。
4. 按 agent 来源优先级查找每个被选中的 directory。
5. 合并所有发现来源对应的 app 启用状态。
6. 找到来源目录后，把它复制到 cc-switch-tui 的 SSOT skill 目录。
7. 如果 directory 已经在 index 中，跳过，不重复导入、不补齐 app 启用状态。
8. 如果 directory 还没有管理记录，从目标目录的 `SKILL.md` 读取 name 和 description，并生成
   `InstalledSkill` 管理记录。
9. 保存 skill index。
10. 重新加载 TUI 数据，并刷新 agent 导入扫描结果缓存。

SSOT 目录由 `SkillService::get_ssot_dir()` 决定，位于当前应用配置目录下的
skills 目录。应用配置目录不要硬编码为 `~/.cc-switch-tui`；它受
`CC_SWITCH_TUI_CONFIG_DIR` 等配置目录解析逻辑影响。

导入复制规则：

- 只在 SSOT 目标目录不存在时复制；
- 复制的是整个 skill 目录；
- 如果 SSOT 目标目录已存在但 index 中没有记录，会复用已有 SSOT 内容读取元数据并创建
  管理记录；
- 导入不会删除 agent 原目录；
- 导入不会把通用 `~/.agents/skills` 来源自动启用到某个 app；
- 导入会把具体 app-local skill 目录反映到对应 app 的启用状态。

## Metadata 与仓库信息

只有 `~/.agents/skills` 来源会使用 `~/.agents/.skill-lock.json` 的元数据。

当 `.skill-lock.json` 中对应 skill 满足以下条件时，会从 lock file 填充 repo 信息：

- `source_type` 是 `github`；
- `source` 形如 `owner/repo`；
- 可选的 `branch`、`source_branch` 或 `source_url` 能解析出分支；
- 可选的 `skill_path` 用于构造 README URL。

导入时还会把 lock file 中涉及的 GitHub repos 合并到 cc-switch-tui 的 skill repo
列表中，便于后续发现、展示和维护。

来自具体 app 目录或 app 环境变量技能目录的 skill 不读取
`~/.agents/.skill-lock.json`。这类导入会使用本地记录：

- `id = local:{directory}`
- `repo_owner = None`
- `repo_name = None`
- `repo_branch = None`
- `readme_url = None`

## 启用状态

Agent 导入会区分通用 agent 来源和具体工具来源。

如果 skill 只存在于 `~/.agents/skills`，新建记录的 `apps` 使用默认值，也就是所有 app
都未启用。

如果新导入的 skill 存在于具体工具目录中，新建记录会启用对应 app。例如：

- `~/.hermes/skills/foo` 会设置 `foo.apps.hermes = true`
- `~/.claude/skills/foo` 会设置 `foo.apps.claude = true`
- `$CODEX_HOME/skills/foo` 会设置 `foo.apps.codex = true`
- `$CLAUDE_CONFIG_DIR/skills/foo` 会设置 `foo.apps.claude = true`
- `$HERMES_HOME/skills/foo` 会设置 `foo.apps.hermes = true`

如果 skill 已经在 cc-switch-tui 管理记录中，`S` 不会重复导入；但 Skills 列表加载时会
检查已管理 skill 是否实际存在于具体 app 的 skills 目录，若存在则回填对应 app 启用状态，
避免目录已经存在但界面未打勾。

后续仍可由用户显式调整：

- 在 Skills 页面按 `m` 选择 app；
- 或使用已有的 skill toggle / sync 命令。

## 与“导入已有”流程的区别

Skills 页面原有的 `i` 是导入已有 skills，主要用于把 app-local 或 SSOT 中尚未管理的
skills 纳入管理。

新增的 `s` 是导入 agent 已安装 skills，范围更窄：

- 扫描 agent 工具自己的安装目录和通用 agent skill 目录；
- 使用独立对话框和独立空结果提示；
- 与 `i` 共用选择、全选、切换、确认的交互形态；
- 导入后会保留具体工具目录所代表的 app 启用状态；
- 当 `~/.agents/skills` 和具体工具目录中存在同名目录时，优先使用
  `~/.agents/skills` 作为复制内容，同时合并具体工具目录的启用状态。

## 维护注意事项

- `scan_agent_installed()` 的职责是扫描 agent 工具目录；如果新增支持 skills 的 app，需要
  确认它是否应加入 `supported_skill_apps()`。
- 不要把通用 `~/.agents/skills` 直接映射成某个 app 的启用状态；只有具体工具目录才能设置
  对应 app flag。
- 修改来源优先级时，需要同步更新同名目录冲突场景的测试。
- 修改 `.skill-lock.json` 解析时，需要保持非 GitHub source 被忽略的行为。
- 修改对话框按键时，需要同时更新 TUI handler、渲染提示和 UI 测试。

相关测试：

- `src-tauri/tests/skills_service.rs`
- `src-tauri/src/cli/tui/tests.rs`
- `src-tauri/src/cli/tui/app/tests.rs`
- `src-tauri/src/cli/tui/ui/tests.rs`
