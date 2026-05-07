# Codex MCP Raw TOML 重构方案

## 📋 目录

- [背景与目标](#背景与目标)
- [核心设计](#核心设计)
- [技术架构](#技术架构)
- [实施计划](#实施计划)
- [风险控制](#风险控制)
- [测试验证](#测试验证)

---

## 背景与目标

### 当前问题

1. **数据丢失**：Codex MCP 配置在 TOML ↔ JSON 转换时丢失注释、格式、特殊值类型
2. **配置复杂**：Codex TOML 支持复杂嵌套结构，强制结构化存储限制灵活性
3. **用户体验差**：无法保留用户手写的注释和格式偏好

### 设计目标

1. **保真存储**：Codex MCP 使用 raw TOML 字符串存储，完全避免序列化损失
2. **架构分离**：Claude/Gemini 继续用结构化 JSON，Codex 用原始文本
3. **UI 解耦**：MCP 管理面板与当前 app 切换彻底分离
4. **增量实施**：零改动现有 Claude/Gemini 逻辑，风险可控

---

## 核心设计

### 数据结构设计

#### config.json 顶层结构

```json
{
  "providers": [
    // 现有 provider 列表，不改
  ],
  "mcp": {
    // 统一 MCP 结构，仅用于 Claude & Gemini
    // ✅ 完全移除 Codex 相关逻辑，apps 字段仅包含 claude/gemini
    "servers": {
      "fetch": {
        "id": "fetch",
        "name": "Fetch MCP",
        "server": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@modelcontextprotocol/server-fetch"]
        },
        "apps": {
          "claude": true,
          "gemini": false
        },
        "description": null,
        "homepage": null,
        "docs": null,
        "tags": []
      }
    }
  },
  "codexMcp": {
    "rawToml": "[mcp]\n# Codex 专用 MCP TOML 片段\n..."
  }
}
```

#### Rust 数据结构

```rust
// src-tauri/src/app_config.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexMcpConfig {
    /// 完整的 MCP TOML 片段（包含 [mcp] 等）
    pub raw_toml: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiAppConfig {
    /// 版本号（v2 起）
    #[serde(default = "default_version")]
    pub version: u32,

    /// 应用管理器（claude/codex/gemini）
    #[serde(flatten)]
    pub apps: HashMap<String, ProviderManager>,

    /// MCP 配置（统一结构 + 旧结构，用于迁移）
    #[serde(default)]
    pub mcp: McpRoot,

    /// Prompt 配置（按客户端分治）
    #[serde(default)]
    pub prompts: PromptRoot,

    /// 通用配置片段（按应用分治）
    #[serde(default)]
    pub common_config_snippets: CommonConfigSnippets,

    /// Claude 通用配置片段（旧字段，用于向后兼容迁移）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_common_config_snippet: Option<String>,

    /// Codex MCP raw TOML（新字段，仅 Codex 使用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_mcp: Option<CodexMcpConfig>,
}
```

### 分层架构

```
┌─────────────────────────────────────────────────┐
│                   UI 层                         │
│  ┌─────────────────────────────────────────┐   │
│  │  MCP 面板（与 app 切换完全解耦）        │   │
│  │  ├─ Tab1: Claude & Gemini (结构化 JSON) │   │
│  │  │   - 仅管理 mcp.servers               │   │
│  │  │   - apps 字段仅含 claude/gemini      │   │
│  │  └─ Tab2: Codex (raw TOML 编辑器)       │   │
│  │      - 独立管理 codexMcp.rawToml        │   │
│  └─────────────────────────────────────────┘   │
└─────────────────────────────────────────────────┘
                      ↓
┌─────────────────────────────────────────────────┐
│                 应用层                          │
│  switch_app() 根据 app 类型选择数据源：         │
│  - Claude/Gemini → mcp.servers (过滤 apps)      │
│  - Codex → codexMcp.rawToml (完全独立)          │
│                                                 │
│  ✅ 无优先级冲突：两者完全隔离                   │
└─────────────────────────────────────────────────┘
                      ↓
┌─────────────────────────────────────────────────┐
│                 数据层                          │
│  config.json:                                   │
│  - mcp.servers: 仅 Claude & Gemini              │
│  - codexMcp.rawToml: 仅 Codex                   │
│                                                 │
│  ✅ 单一职责：互不干扰                           │
└─────────────────────────────────────────────────┘
```

### MCP 配置职责划分

| 配置源 | 职责 | 数据格式 | 管理方式 |
|--------|------|----------|----------|
| `mcp.servers` | Claude & Gemini MCP | 结构化 JSON | UI 表单（Tab1） |
| `codexMcp.rawToml` | Codex MCP | 原始 TOML 字符串 | 代码编辑器（Tab2） |

**关键原则**：
- ✅ `mcp.servers` 中的 `apps` 字段**永不包含 `codex`**
- ✅ Codex MCP **仅存储**在 `codexMcp.rawToml`
- ✅ 切换逻辑**完全独立**，无优先级判断

---

## 技术架构

### 后端架构（Rust）

#### 1. 配置管理

**文件**：`src-tauri/src/app_config.rs`

```rust
impl MultiAppConfig {
    pub fn load() -> Result<Self, AppError> {
        // 1. 按 v2 结构加载 MultiAppConfig
        let mut config = /* ... 现有 load 实现 ... */;

        let mut updated = false;

        // 2. 执行 Codex MCP → raw TOML 迁移
        //    - 仅迁移 v3.6.2 的 mcp.codex.servers → codexMcp.rawToml
        //    - 迁移后清空 mcp.codex.servers，避免被后续 unified 迁移处理
        if migration::migrate_codex_mcp_to_raw_toml(&mut config)? {
            updated = true;
        }

        // 3. 执行 unified MCP 迁移（mcp.claude/gemini → mcp.servers）
        //    - ✅ 此时 mcp.codex 已清空，不会被迁移到 unified
        //    - unified 结构中 apps 字段仅包含 claude/gemini
        if config.migrate_mcp_to_unified()? {
            updated = true;
        }

        // 4. 其他迁移（Prompt、通用片段等）
        //    ...

        if updated {
            config.save()?;
        }

        Ok(config)
    }

    pub fn save(&self) -> Result<(), AppError> {
        // 序列化时包含 codexMcp 字段
        // ...
    }
}
```

#### 2. 数据迁移

**文件**：`src-tauri/src/migration.rs`

```rust
/// 将 v3.6.2 的 mcp.codex.servers 迁移为 codexMcp.rawToml
///
/// **关键行为**：
/// 1. 仅在 codex_mcp 为空且存在旧的 mcp.codex.servers 时执行
/// 2. 转换后**立即清空 mcp.codex.servers**，避免被 unified 迁移重复处理
/// 3. 返回 true 表示发生了迁移，需要保存配置
pub fn migrate_codex_mcp_to_raw_toml(
    config: &mut MultiAppConfig,
) -> Result<bool, AppError> {
    // 已迁移过，跳过
    if config.codex_mcp.is_some() {
        return Ok(false);
    }

    let legacy_servers = &config.mcp.codex.servers;
    if legacy_servers.is_empty() {
        // 没有旧的 Codex MCP 配置，跳过
        return Ok(false);
    }

    // 转换为 TOML
    let toml = convert_legacy_codex_mcp_to_toml(legacy_servers)?;
    config.codex_mcp = Some(CodexMcpConfig { raw_toml: toml });

    // ✅ 关键：清空旧数据，确保 unified 迁移不会处理 Codex
    config.mcp.codex.servers.clear();

    log::info!(
        "Migrated {} Codex MCP servers to raw TOML and cleared legacy storage",
        legacy_servers.len()
    );

    Ok(true)
}

/// 将 v3.6.2 时代的 mcp.codex.servers (HashMap<String, serde_json::Value>)
/// 转换为 Codex 所需的 MCP TOML 片段
fn convert_legacy_codex_mcp_to_toml(
    servers: &HashMap<String, serde_json::Value>,
) -> Result<String, AppError> {
    let mut toml = String::from("[mcp]\n\n");

    for (id, entry) in servers {
        // 旧结构：entry 是宽松 JSON 对象，包含 name/server/enabled 等字段
        let obj = entry
            .as_object()
            .ok_or_else(|| AppError::Config(format!(
                "无效的 Codex MCP 条目 '{}': 必须为 JSON 对象",
                id
            )))?;

        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(id);

        let server = obj.get("server").ok_or_else(|| {
            AppError::Config(format!(
                "无效的 Codex MCP 条目 '{}': 缺少 server 字段",
                id
            ))
        })?;

        let server_obj = server.as_object().ok_or_else(|| {
            AppError::Config(format!(
                "无效的 Codex MCP 条目 '{}': server 必须是 JSON 对象",
                id
            ))
        })?;

        toml.push_str("[[mcp.servers]]\n");
        toml.push_str(&format!("name = \"{}\"\n", name));

        // stdio 类型字段
        if let Some(cmd) = server_obj.get("command").and_then(|v| v.as_str()) {
            toml.push_str(&format!("command = \"{}\"\n", cmd));
        }

        if let Some(args) = server_obj.get("args").and_then(|v| v.as_array()) {
            let args_str = args
                .iter()
                .filter_map(|a| a.as_str())
                .map(|a| format!("\"{}\"", a))
                .collect::<Vec<_>>()
                .join(", ");
            if !args_str.is_empty() {
                toml.push_str(&format!("args = [{}]\n", args_str));
            }
        }

        if let Some(env) = server_obj.get("env").and_then(|v| v.as_object()) {
            if !env.is_empty() {
                toml.push_str("\n[mcp.servers.env]\n");
                for (k, v) in env {
                    if let Some(val) = v.as_str() {
                        toml.push_str(&format!("{} = \"{}\"\n", k, val));
                    }
                }
            }
        }

        if let Some(cwd) = server_obj.get("cwd").and_then(|v| v.as_str()) {
            toml.push_str(&format!("cwd = \"{}\"\n", cwd));
        }

        // http 类型字段
        if let Some(url) = server_obj.get("url").and_then(|v| v.as_str()) {
            toml.push_str(&format!("url = \"{}\"\n", url));
        }

        if let Some(t) = server_obj.get("type").and_then(|v| v.as_str()) {
            toml.push_str(&format!("type = \"{}\"\n", t));
        }

        if let Some(headers) = server_obj.get("headers").and_then(|v| v.as_object()) {
            if !headers.is_empty() {
                toml.push_str("\n[mcp.servers.headers]\n");
                for (k, v) in headers {
                    if let Some(val) = v.as_str() {
                        toml.push_str(&format!("{} = \"{}\"\n", k, val));
                    }
                }
            }
        }

        toml.push_str("\n");
    }

    Ok(toml)
}
```

#### 3. Tauri 命令

**文件**：`src-tauri/src/commands/mcp.rs`

```rust
/// 获取 Codex MCP 配置
#[tauri::command]
pub async fn get_codex_mcp_config(
    state: State<'_, AppState>
) -> Result<String, String> {
    let config = state.config.read().unwrap();

    if let Some(codex_mcp) = &config.codex_mcp {
        Ok(codex_mcp.raw_toml.clone())
    } else {
        // 返回默认模板
        Ok(String::from(
            "[mcp]\n# 在这里填写 Codex MCP 配置\n# 示例：\n# [[mcp.servers]]\n# name = \"example\"\n# command = \"npx\"\n# args = [\"-y\", \"@modelcontextprotocol/server-example\"]\n"
        ))
    }
}

/// 更新 Codex MCP 配置
#[tauri::command]
pub async fn update_codex_mcp_config(
    state: State<'_, AppState>,
    raw_toml: String,
) -> Result<(), String> {
    // 1. 语法验证
    toml::from_str::<toml::Value>(&raw_toml)
        .map_err(|e| format!("TOML syntax error: {}", e))?;

    // 2. 可选警告
    if !raw_toml.contains("[mcp") {
        log::warn!("Codex MCP TOML doesn't contain [mcp] section");
    }

    // 3. 保存
    let mut config = state.config.write().unwrap();
    config.codex_mcp = Some(CodexMcpConfig {
        raw_toml: raw_toml.clone(),
    });
    config.save()
        .map_err(|e| format!("Failed to save config: {}", e))?;

    Ok(())
}

/// 验证 TOML 语法（前端可在保存前调用）
#[tauri::command]
pub async fn validate_codex_mcp_toml(
    raw_toml: String
) -> Result<ValidateResult, String> {
    match toml::from_str::<toml::Value>(&raw_toml) {
        Ok(_) => Ok(ValidateResult {
            valid: true,
            error: None,
            warnings: vec![],
        }),
        Err(e) => Ok(ValidateResult {
            valid: false,
            error: Some(e.to_string()),
            warnings: vec![],
        }),
    }
}

#[derive(Debug, Serialize)]
pub struct ValidateResult {
    pub valid: bool,
    pub error: Option<String>,
    pub warnings: Vec<String>,
}

/// 从 Codex live 配置导入 MCP 段
#[tauri::command]
pub async fn import_codex_mcp_from_live() -> Result<String, String> {
    let config_path = get_codex_config_path()
        .map_err(|e| e.to_string())?;

    if !config_path.exists() {
        return Ok(String::from("[mcp]\n# No existing Codex config found\n"));
    }

    let content = fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read Codex config: {}", e))?;

    let mcp_section = extract_mcp_section_from_toml(&content)?;
    Ok(mcp_section)
}

fn extract_mcp_section_from_toml(content: &str) -> Result<String, String> {
    use toml_edit::DocumentMut;

    let doc = content.parse::<DocumentMut>()
        .map_err(|e| format!("Invalid TOML: {}", e))?;

    if let Some(mcp_item) = doc.get("mcp") {
        let mut result = String::from("[mcp]\n");
        result.push_str(&mcp_item.to_string());
        Ok(result)
    } else {
        Ok(String::from("[mcp]\n# No MCP config found in live file\n"))
    }
}
```

#### 3. 切换逻辑

**文件**：`src-tauri/src/services/provider.rs`

```rust
impl ProviderService {
    /// 切换到 Codex provider
    pub fn switch_to_codex(
        &self,
        provider: &Provider
    ) -> Result<(), AppError> {
        // 1. 读取 Codex MCP 配置（完全独立于 unified）
        let codex_mcp = {
            let config = self.state.config.read().unwrap();
            config.codex_mcp.clone()
        };

        // 2. 生成最终配置（base + MCP）
        let final_toml = self.apply_codex_config(provider, &codex_mcp)?;

        // 3. 写入 live 文件
        self.write_codex_config(&final_toml)?;

        Ok(())
    }

    fn apply_codex_config(
        &self,
        provider: &Provider,
        codex_mcp: &Option<CodexMcpConfig>,
    ) -> Result<String, AppError> {
        // 1. 生成基础配置（不含 MCP）
        let mut base_config = self.generate_codex_base_config(provider)?;

        // 2. 追加 MCP 配置（如果有）
        if let Some(mcp_cfg) = codex_mcp {
            let trimmed = mcp_cfg.raw_toml.trim();
            if !trimmed.is_empty() {
                // 确保有换行分隔
                if !base_config.ends_with('\n') {
                    base_config.push('\n');
                }
                base_config.push('\n');
                base_config.push_str(trimmed);
            }
        }

        // 3. 验证最终 TOML 可解析
        toml::from_str::<toml::Value>(&base_config)
            .map_err(|e| AppError::Config(format!(
                "Generated Codex config is invalid: {}",
                e
            )))?;

        Ok(base_config)
    }

    /// 切换到 Claude/Gemini
    pub fn switch_to_claude_or_gemini(
        &self,
        provider: &Provider,
        app_type: AppType,
    ) -> Result<(), AppError> {
        // 从 unified MCP 读取配置（apps 字段仅含 claude/gemini）
        let mcp_servers = {
            let config = self.state.config.read().unwrap();
            config.mcp.servers
                .values()
                .filter(|s| s.apps.get(&app_type.to_string()).unwrap_or(&false))
                .cloned()
                .collect::<Vec<_>>()
        };

        // 生成并写入配置
        // ...

        Ok(())
    }
}
```

**关键点**：
- ✅ Codex 切换**完全不读取** `mcp.servers`
- ✅ Claude/Gemini 切换**完全不读取** `codexMcp`
- ✅ 无优先级判断，逻辑简单清晰

### 前端架构（React + TypeScript）

#### 1. API 层

**文件**：`src/lib/api/mcp.ts`

```typescript
export const codexMcpApi = {
  /**
   * 获取 Codex MCP 配置（raw TOML）
   */
  get: () => invoke<string>('get_codex_mcp_config'),

  /**
   * 更新 Codex MCP 配置
   */
  update: (rawToml: string) =>
    invoke('update_codex_mcp_config', { rawToml }),

  /**
   * 验证 TOML 语法
   */
  validate: (rawToml: string) =>
    invoke<ValidateResult>('validate_codex_mcp_toml', { rawToml }),

  /**
   * 从 Codex live 配置导入
   */
  importFromLive: () =>
    invoke<string>('import_codex_mcp_from_live'),
};

export interface ValidateResult {
  valid: boolean;
  error?: string;
  warnings: string[];
}
```

#### 2. Hooks

**文件**：`src/hooks/useCodexMcp.ts`

```typescript
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { codexMcpApi } from '@/lib/api/mcp';
import { toast } from 'sonner';

export function useCodexMcp() {
  const queryClient = useQueryClient();

  // 查询
  const query = useQuery({
    queryKey: ['codexMcp'],
    queryFn: codexMcpApi.get,
  });

  // 更新
  const updateMutation = useMutation({
    mutationFn: codexMcpApi.update,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['codexMcp'] });
      toast.success('Codex MCP 配置已保存');
    },
    onError: (error: Error) => {
      toast.error(`保存失败: ${error.message}`);
    },
  });

  // 验证
  const validateMutation = useMutation({
    mutationFn: codexMcpApi.validate,
  });

  // 导入
  const importMutation = useMutation({
    mutationFn: codexMcpApi.importFromLive,
    onSuccess: (data) => {
      queryClient.setQueryData(['codexMcp'], data);
      toast.success('已从 Codex 配置导入 MCP');
    },
    onError: (error: Error) => {
      toast.error(`导入失败: ${error.message}`);
    },
  });

  return {
    rawToml: query.data ?? '',
    isLoading: query.isLoading,
    update: updateMutation.mutate,
    // 保存前需要拿到校验结果，因此对外暴露 mutateAsync，便于 await
    validate: validateMutation.mutateAsync,
    importFromLive: importMutation.mutate,
  };
}
```

#### 3. UI 组件

**文件**：`src/components/mcp/McpPanel.tsx`

```typescript
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { ClaudeGeminiMcpTab } from './ClaudeGeminiMcpTab';
import { CodexMcpTab } from './CodexMcpTab';

export function McpPanel() {
  return (
    <Tabs defaultValue="claude-gemini" className="w-full">
      <TabsList>
        <TabsTrigger value="claude-gemini">
          Claude & Gemini
        </TabsTrigger>
        <TabsTrigger value="codex">
          Codex
        </TabsTrigger>
      </TabsList>

      <TabsContent value="claude-gemini">
        <ClaudeGeminiMcpTab />
      </TabsContent>

      <TabsContent value="codex">
        <CodexMcpTab />
      </TabsContent>
    </Tabs>
  );
}
```

**文件**：`src/components/mcp/ClaudeGeminiMcpTab.tsx`

```typescript
import { Alert, AlertDescription } from '@/components/ui/alert';
import { useMcp } from '@/hooks/useMcp';

/**
 * Claude & Gemini 的 MCP 管理 Tab
 *
 * ✅ 仅操作 mcp.servers
 * ✅ apps 字段仅含 claude/gemini（不含 codex）
 * ✅ 完全独立于当前选中的 app
 */
export function ClaudeGeminiMcpTab() {
  const { servers, addServer, updateServer, deleteServer } = useMcp();

  return (
    <div className="space-y-4">
      <Alert>
        <AlertDescription>
          管理 Claude 和 Gemini 的 MCP 服务器。
          <br />
          <strong>注意：Codex MCP 在专用 Tab 管理（raw TOML 格式）。</strong>
        </AlertDescription>
      </Alert>

      {/* 现有 MCP 列表组件，但需确保： */}
      {/* 1. 表单中 apps 选项仅显示 claude/gemini */}
      {/* 2. 过滤掉可能的历史遗留 codex 数据 */}
      <McpServerList
        servers={servers.filter(s => !s.apps.codex)}
        onAdd={addServer}
        onUpdate={updateServer}
        onDelete={deleteServer}
        availableApps={['claude', 'gemini']} // ✅ 限制可选应用
      />
    </div>
  );
}
```

**文件**：`src/components/mcp/CodexMcpTab.tsx`

```typescript
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { CodexMcpEditor } from './CodexMcpEditor';
import { useCodexMcp } from '@/hooks/useCodexMcp';
import { Alert, AlertDescription } from '@/components/ui/alert';

export function CodexMcpTab() {
  const { rawToml, isLoading, update, validate, importFromLive } = useCodexMcp();
  const [localValue, setLocalValue] = useState(rawToml);
  const [validationError, setValidationError] = useState<string | null>(null);

  // 当后端数据加载完成或导入时，同步到本地编辑器
  useEffect(() => {
    setLocalValue(rawToml);
  }, [rawToml]);

  const handleSave = async () => {
    // 保存前验证
    const result = await validate(localValue);

    if (!result.valid) {
      setValidationError(result.error ?? 'Unknown error');
      return;
    }

    setValidationError(null);
    update(localValue);
  };

  const handleImport = () => {
    importFromLive();
  };

  if (isLoading) {
    return <div>加载中...</div>;
  }

  return (
    <div className="space-y-4">
      <Alert>
        <AlertDescription>
          直接编辑 Codex MCP TOML 配置。修改会在下次切换到 Codex 时生效。
        </AlertDescription>
      </Alert>

      {validationError && (
        <Alert variant="destructive">
          <AlertDescription>
            TOML 语法错误: {validationError}
          </AlertDescription>
        </Alert>
      )}

      <CodexMcpEditor
        value={localValue}
        onChange={setLocalValue}
      />

      <div className="flex gap-2">
        <Button onClick={handleSave}>保存</Button>
        <Button variant="outline" onClick={handleImport}>
          从 Codex 配置导入
        </Button>
        <Button
          variant="outline"
          onClick={() => setLocalValue(rawToml)}
        >
          重置
        </Button>
      </div>
    </div>
  );
}
```

**文件**：`src/components/mcp/CodexMcpEditor.tsx`

```typescript
import { useEffect, useRef } from 'react';
import { EditorView, basicSetup } from 'codemirror';
import { toml } from '@codemirror/lang-toml';
import { oneDark } from '@codemirror/theme-one-dark';
import { linter, Diagnostic } from '@codemirror/lint';
import * as TOML from 'smol-toml';

const tomlLinter = linter((view) => {
  const diagnostics: Diagnostic[] = [];
  const content = view.state.doc.toString();

  try {
    TOML.parse(content);
  } catch (e: any) {
    diagnostics.push({
      from: 0,
      to: content.length,
      severity: 'error',
      message: `TOML Syntax Error: ${e.message}`,
    });
  }

  return diagnostics;
});

interface Props {
  value: string;
  onChange: (value: string) => void;
}

export function CodexMcpEditor({ value, onChange }: Props) {
  const editorRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView>();

  useEffect(() => {
    if (!editorRef.current) return;

    const view = new EditorView({
      doc: value,
      extensions: [
        basicSetup,
        toml(),
        oneDark,
        tomlLinter,
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            onChange(update.state.doc.toString());
          }
        }),
      ],
      parent: editorRef.current,
    });

    viewRef.current = view;

    return () => view.destroy();
  }, []);

  // 外部值变化时更新编辑器
  useEffect(() => {
    if (!viewRef.current) return;
    const currentValue = viewRef.current.state.doc.toString();
    if (currentValue !== value) {
      viewRef.current.dispatch({
        changes: {
          from: 0,
          to: currentValue.length,
          insert: value,
        },
      });
    }
  }, [value]);

  return (
    <div
      ref={editorRef}
      className="border rounded-md overflow-hidden min-h-[400px]"
    />
  );
}
```

---

## 实施计划

### Phase 0: 准备工作

**时间**：0.5 天

- [ ] 创建开发分支 `feature/codex-mcp-raw-toml`
- [ ] 安装前端依赖：`pnpm add @codemirror/lang-toml`
- [ ] 备份现有配置文件用于测试

### Phase 1: 后端基础（P0）

**时间**：1.5 天

**任务**：

- [ ] 在 `app_config.rs` 中定义 `CodexMcpConfig`
- [ ] 修改 `MultiAppConfig` 添加 `codex_mcp` 字段
- [ ] 更新 `MultiAppConfig::load()` 和 `save()` 支持新字段
- [ ] 编写迁移函数 `migrate_codex_mcp_to_raw_toml`
  - [ ] 实现 `convert_servers_map_to_toml`
  - [ ] 处理 stdio 类型服务器
  - [ ] 处理 http 类型服务器
- [ ] 在 `lib.rs` 启动时执行迁移
- [ ] 单元测试：迁移逻辑正确性

**验收标准**：

- 现有配置可正确迁移为 raw TOML
- config.json 包含 `codexMcp` 字段
- 迁移不影响 Claude/Gemini 配置

### Phase 2: 命令层（P0）

**时间**：1 天

**任务**：

- [ ] 在 `commands/mcp.rs` 实现命令：
  - [ ] `get_codex_mcp_config`
  - [ ] `update_codex_mcp_config`
  - [ ] `validate_codex_mcp_toml`
  - [ ] `import_codex_mcp_from_live`
- [ ] 实现 `extract_mcp_section_from_toml` 辅助函数
- [ ] 在 `lib.rs` 注册新命令
- [ ] 集成测试：命令调用正确性

**验收标准**：

- 所有命令可通过 Tauri invoke 正常调用
- TOML 语法验证准确
- 从 live 配置导入功能正常

### Phase 3: 切换逻辑（P0）

**时间**：1 天

**任务**：

- [ ] 修改 `services/provider.rs` 的 Codex 切换逻辑
  - [ ] 实现 `switch_to_codex`（仅读取 `codexMcp`）
  - [ ] 实现 `apply_codex_config`（拼接 base + raw TOML）
  - [ ] 添加最终 TOML 验证
- [ ] 确保 Claude/Gemini 切换逻辑不读取 `codexMcp`
- [ ] 原子写入机制验证
- [ ] 集成测试：Codex 切换后配置正确

**验收标准**：

- Codex 切换时，config.toml 包含 raw TOML 的 MCP 段
- Claude/Gemini 切换时，仅使用 `mcp.servers` 中 `apps.claude/gemini=true` 的项
- 生成的配置可被对应应用正确解析
- 切换失败时不损坏现有配置

### Phase 4: 前端 API（P0）

**时间**：0.5 天

**任务**：

- [ ] 在 `lib/api/mcp.ts` 创建 `codexMcpApi`
- [ ] 定义 TypeScript 类型 `ValidateResult`
- [ ] 在 `hooks/useCodexMcp.ts` 创建 Hook
  - [ ] useQuery 读取配置
  - [ ] useMutation 更新配置
  - [ ] useMutation 验证语法
  - [ ] useMutation 导入配置

**验收标准**：

- API 调用成功返回数据
- Hook 状态管理正确
- 错误处理完善

### Phase 5: UI 实现（P0）

**时间**：2 天

**任务**：

- [ ] 重构 `McpPanel.tsx` 为 Tabs 布局
- [ ] 创建 `ClaudeGeminiMcpTab.tsx`
  - [ ] 移除对 `currentApp` 的依赖
  - [ ] 直接操作 `mcp.servers`
  - [ ] **限制 `availableApps` 为 `['claude', 'gemini']`**
  - [ ] **过滤掉 `apps.codex` 的历史数据**
  - [ ] 添加提示："Codex MCP 在专用 Tab 管理"
- [ ] 创建 `CodexMcpTab.tsx`
  - [ ] 集成编辑器组件
  - [ ] 实现保存/导入/重置逻辑
  - [ ] 添加验证错误提示
- [ ] 创建 `CodexMcpEditor.tsx`
  - [ ] 集成 CodeMirror 6
  - [ ] 配置 TOML 语法高亮
  - [ ] 集成 TOML linter
  - [ ] 实现双向绑定
- [ ] 国际化：添加相关翻译 key
- [ ] **更新现有 MCP 表单组件，移除 `codex` 选项**

**验收标准**：

- MCP 面板有两个独立 Tab
- Tab1 (Claude & Gemini)：
  - `apps` 选项仅显示 claude/gemini
  - 不显示任何 `apps.codex=true` 的服务器
  - 无法添加/编辑 Codex MCP
- Tab2 (Codex)：
  - 可正常编辑 raw TOML
  - 语法错误有实时提示
  - 保存后配置持久化

### Phase 6: 增强功能（P1）

**时间**：1 天

**任务**：

- [ ] 添加 TOML 模板快捷插入功能
- [ ] 导出到 Codex live 配置功能
- [ ] 配置历史记录（可选）
- [ ] 改进错误提示（显示行号）

**验收标准**：

- 模板插入功能可用
- 导出功能正常

### Phase 7: 测试与文档（P0）

**时间**：1 天

**任务**：

- [ ] 端到端测试：
  - [ ] 新用户首次启动
  - [ ] 现有用户迁移场景（v3.6.2 → v3.7.0）
  - [ ] 验证迁移后 `mcp.codex.servers` 被清空
  - [ ] 验证 unified MCP 不包含 `apps.codex`
  - [ ] Claude ↔ Codex ↔ Gemini 切换
  - [ ] Codex MCP 编辑后切换生效
  - [ ] Tab1 无法操作 Codex MCP
- [ ] 更新 `CLAUDE.md` 文档
  - [ ] 明确 MCP 配置职责划分
  - [ ] 更新配置文件路径说明
- [ ] 编写 migration guide
- [ ] 添加 CHANGELOG 条目

**验收标准**：

- 所有测试用例通过
- 文档完整准确
- 迁移逻辑无数据丢失

---

## 风险控制

### 1. 数据丢失风险

**风险**：迁移过程中旧配置丢失

**控制措施**：

- ✅ 迁移前自动备份 config.json（带时间戳）
- ✅ **迁移后清空 `mcp.codex.servers`，但不删除 `mcp.codex` 根节点**（保留结构用于回滚）
- ✅ 迁移日志记录详细信息（服务器数量、时间戳等）
- ✅ 提供回滚命令（Phase 6+）

### 2. TOML 格式错误

**风险**：用户手写 TOML 导致 Codex 配置损坏

**控制措施**：

- ✅ 保存前强制验证语法
- ✅ 实时 linting 提示错误
- ✅ 切换前再次验证最终配置
- ✅ 写入失败时自动回滚（已有 `.bak` 机制）

### 3. 并发写入

**风险**：多实例同时修改配置

**控制措施**：

- ✅ 使用 RwLock 保护 config 访问
- ✅ 使用 tauri-plugin-single-instance（已集成）

### 4. Unified MCP 污染

**风险**：历史数据中存在 `apps.codex=true` 的服务器

**控制措施**：

- ✅ **迁移时清空 `mcp.codex.servers`**，阻止 unified 迁移处理 Codex
- ✅ **前端过滤**：Tab1 显示时过滤掉 `apps.codex=true` 的项
- ✅ **表单限制**：`availableApps` 仅包含 `['claude', 'gemini']`
- ✅ **后端验证**（可选）：保存 unified MCP 时检查并拒绝包含 `codex` 的 apps

---

## 测试验证

### 单元测试

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_stdio_server_to_toml() {
        let server = McpServer {
            name: "test".into(),
            server: ServerSpec::Stdio {
                command: "npx".into(),
                args: Some(vec!["-y".into(), "test".into()]),
                env: Some(HashMap::from([
                    ("KEY".into(), "value".into())
                ])),
                cwd: None,
            },
            // ...
        };

        let toml = convert_server_to_toml("test", &server).unwrap();

        assert!(toml.contains("command = \"npx\""));
        assert!(toml.contains("args = [\"-y\", \"test\"]"));
        assert!(toml.contains("KEY = \"value\""));
    }

    #[test]
    fn test_toml_validation() {
        let valid_toml = "[mcp]\n[[mcp.servers]]\nname = \"test\"\n";
        assert!(validate_toml(valid_toml).is_ok());

        let invalid_toml = "[mcp\n[[mcp.servers]]\n";
        assert!(validate_toml(invalid_toml).is_err());
    }
}
```

### 集成测试场景

| 场景 | 步骤 | 预期结果 |
|------|------|----------|
| 新用户首次启动 | 1. 删除 config.json<br>2. 启动应用<br>3. 打开 Codex MCP Tab | 显示默认模板 |
| 现有用户迁移 | 1. 使用 v3.6.2 config.json（含 `mcp.codex.servers`）<br>2. 启动应用<br>3. 检查 config.json | - `codexMcp.rawToml` 存在且内容正确<br>- `mcp.codex.servers` 为空对象 `{}`<br>- `mcp.servers` 不含 `apps.codex` |
| 编辑 Codex MCP | 1. 在 Tab2 编辑 TOML<br>2. 保存<br>3. 检查 config.json | `codexMcp.rawToml` 更新 |
| 切换到 Codex | 1. 编辑 Codex MCP<br>2. 切换到 Codex provider<br>3. 检查 `~/.codex/config.toml` | MCP 段正确写入，与 raw TOML 一致 |
| 切换到 Claude | 1. 在 Tab1 添加 Claude MCP<br>2. 切换到 Claude provider<br>3. 检查 `~/.claude/settings.json` | 仅包含 `apps.claude=true` 的服务器 |
| TOML 语法错误 | 1. 在 Tab2 输入错误 TOML<br>2. 保存 | 显示错误提示，拒绝保存 |
| Tab1 隔离性 | 1. 打开 Tab1<br>2. 尝试添加服务器 | - `apps` 选项仅显示 claude/gemini<br>- 无法选择 codex |
| 历史数据过滤 | 1. 手动在 config.json 添加 `apps.codex=true` 的服务器<br>2. 打开 Tab1 | 该服务器不在列表中显示 |
| 从 live 导入 | 1. 手动编辑 `~/.codex/config.toml`<br>2. 点击 Tab2 "导入"<br>3. 检查编辑器 | 显示导入的 MCP 配置 |

### 性能测试

- [ ] 大型 TOML（>10KB）编辑性能
- [ ] CodeMirror 初始化时间（<500ms）
- [ ] 配置切换时间（<200ms）

---

## 依赖项

### 前端新增

```bash
pnpm add @codemirror/lang-toml
```

### 后端（已有）

- `toml = "0.8"`
- `toml_edit = "0.22"`

---

## 时间线

| Phase | 工作量 | 累计 |
|-------|--------|------|
| Phase 0: 准备工作 | 0.5 天 | 0.5 天 |
| Phase 1: 后端基础 | 1.5 天 | 2 天 |
| Phase 2: 命令层 | 1 天 | 3 天 |
| Phase 3: 切换逻辑 | 1 天 | 4 天 |
| Phase 4: 前端 API | 0.5 天 | 4.5 天 |
| Phase 5: UI 实现 | 2 天 | 6.5 天 |
| Phase 6: 增强功能（可选）| 1 天 | 7.5 天 |
| Phase 7: 测试与文档 | 1 天 | 8.5 天 |

**总计**：8.5 天（约 2 周）

**MVP（最小可行产品）**：Phase 0-5 + Phase 7 = 7 天

---

## 回滚计划

如果重构出现严重问题，执行以下步骤：

1. **恢复代码**：
   ```bash
   git checkout main
   git branch -D feature/codex-mcp-raw-toml
   ```

2. **恢复配置**：
   ```bash
   # 迁移时会自动备份为 config.v3.backup.<timestamp>.json
   cp ~/.cc-switch/config.v3.backup.*.json ~/.cc-switch/config.json
   ```

3. **重启应用**

---

## 成功标准

- ✅ 现有用户配置无损迁移
- ✅ Codex MCP 配置保留注释和格式
- ✅ MCP 面板与 app 切换完全解耦
- ✅ Claude/Gemini 逻辑零改动
- ✅ 所有测试用例通过
- ✅ 文档完整更新

---

## 附录

### 示例配置

#### 迁移前（v3.6.2）

```json
{
  "providers": [...],
  "mcp": {
    "codex": {
      "servers": {
        "fetch": {
          "id": "fetch",
          "name": "Fetch MCP",
          "server": {
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-fetch"]
          },
          "enabled": true
        }
      }
    }
  }
}
```

#### 迁移后（v3.7.0）

```json
{
  "providers": [...],
  "mcp": {
    "servers": {
      "fetch": {
        "id": "fetch",
        "name": "Fetch MCP",
        "server": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@modelcontextprotocol/server-fetch"]
        },
        "apps": {
          "claude": true,
          "gemini": false
        }
      }
    },
    "codex": {
      "servers": {}  // ✅ 已清空，但保留结构用于回滚
    }
  },
  "codexMcp": {
    "rawToml": "[mcp]\n\n[[mcp.servers]]\nname = \"Fetch MCP\"\ncommand = \"npx\"\nargs = [\"-y\", \"@modelcontextprotocol/server-fetch\"]\n"
  }
}
```

### 相关文档

- [Codex 官方 MCP 文档](https://codex.dev/docs/mcp)
- [TOML 规范](https://toml.io/en/)
- [CodeMirror 6 文档](https://codemirror.net/docs/)
- [项目 README](../README_ZH.md)

---

**文档版本**：2.0
**创建时间**：2025-11-18
**最后更新**：2025-11-18
**负责人**：Jason Young

---

## 版本历史

### v2.0 (2025-11-18)
- ✅ **架构简化**：完全移除 unified MCP 中的 Codex 支持
- ✅ **单一职责**：`mcp.servers` 仅用于 Claude/Gemini，`codexMcp.rawToml` 仅用于 Codex
- ✅ **迁移增强**：清空 `mcp.codex.servers` 避免重复处理
- ✅ **UI 隔离**：Tab1 限制 `availableApps`，过滤 Codex 数据
- ✅ **测试覆盖**：增加 Tab1 隔离性、历史数据过滤等场景

### v1.0 (2025-11-18)
- 初始版本
