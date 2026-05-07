use std::path::Path;
use std::sync::PoisonError;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("配置错误: {0}")]
    Config(String),
    #[error("数据库错误: {0}")]
    Database(String),
    #[error("无效输入: {0}")]
    InvalidInput(String),
    #[error("IO 错误: {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{context}: {source}")]
    IoContext {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("JSON 解析错误: {path}: {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("JSON 序列化失败: {source}")]
    JsonSerialize {
        #[source]
        source: serde_json::Error,
    },
    #[error("TOML 解析错误: {path}: {source}")]
    Toml {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("锁获取失败: {0}")]
    Lock(String),
    #[error("MCP 校验失败: {0}")]
    McpValidation(String),
    #[error("{0}")]
    Message(String),
    #[error("{zh} ({en})")]
    Localized {
        key: &'static str,
        zh: String,
        en: String,
    },
}

impl AppError {
    pub fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.as_ref().display().to_string(),
            source,
        }
    }

    pub fn json(path: impl AsRef<Path>, source: serde_json::Error) -> Self {
        Self::Json {
            path: path.as_ref().display().to_string(),
            source,
        }
    }

    pub fn toml(path: impl AsRef<Path>, source: toml::de::Error) -> Self {
        Self::Toml {
            path: path.as_ref().display().to_string(),
            source,
        }
    }

    pub fn localized(key: &'static str, zh: impl Into<String>, en: impl Into<String>) -> Self {
        Self::Localized {
            key,
            zh: zh.into(),
            en: en.into(),
        }
    }

    /// 警告：子进程无法分配到 Job Object（例如已经处于不允许嵌套的 Job 中），将降级回退。
    /// i18n key: `windows.job_assign_failed_fallback`
    pub fn windows_job_assign_failed_fallback(reason: impl std::fmt::Display) -> Self {
        Self::localized(
            "windows.job_assign_failed_fallback",
            format!("无法将子进程分配到 Job Object，将降级回退: {reason}"),
            format!("Failed to assign child process to Job Object; falling back: {reason}"),
        )
    }

    /// 错误：ResumeThread 调用失败。
    /// i18n key: `windows.resume_thread_failed`
    pub fn windows_resume_thread_failed(code: u32) -> Self {
        Self::localized(
            "windows.resume_thread_failed",
            format!("ResumeThread 调用失败，Win32 错误码: {code}"),
            format!("ResumeThread failed with Win32 error code: {code}"),
        )
    }

    /// 错误：CreateProcessW 调用失败。
    /// i18n key: `windows.create_process_failed`
    pub fn windows_create_process_failed(code: u32) -> Self {
        Self::localized(
            "windows.create_process_failed",
            format!("CreateProcessW 调用失败，Win32 错误码: {code}"),
            format!("CreateProcessW failed with Win32 error code: {code}"),
        )
    }

    /// 错误：创建 Job Object 失败。
    /// i18n key: `windows.create_job_object_failed`
    pub fn windows_create_job_object_failed(code: u32) -> Self {
        Self::localized(
            "windows.create_job_object_failed",
            format!("创建 Job Object 失败，Win32 错误码: {code}"),
            format!("Failed to create Job Object, Win32 error: {code}"),
        )
    }

    /// 错误：设置 Job Object 信息失败。
    /// i18n key: `windows.set_job_information_failed`
    pub fn windows_set_job_information_failed(code: u32) -> Self {
        Self::localized(
            "windows.set_job_information_failed",
            format!("设置 Job Object 信息失败，Win32 错误码: {code}"),
            format!("Failed to set Job Object information, Win32 error: {code}"),
        )
    }
}

impl<T> From<PoisonError<T>> for AppError {
    fn from(err: PoisonError<T>) -> Self {
        Self::Lock(err.to_string())
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Database(err.to_string())
    }
}

impl From<AppError> for String {
    fn from(err: AppError) -> Self {
        err.to_string()
    }
}

/// 格式化为 JSON 错误字符串，前端可解析为结构化错误
pub fn format_skill_error(
    code: &str,
    context: &[(&str, &str)],
    suggestion: Option<&str>,
) -> String {
    use serde_json::json;

    let mut ctx_map = serde_json::Map::new();
    for (key, value) in context {
        ctx_map.insert(key.to_string(), json!(value));
    }

    let error_obj = json!({
        "code": code,
        "context": ctx_map,
        "suggestion": suggestion,
    });

    serde_json::to_string(&error_obj).unwrap_or_else(|_| {
        // 如果 JSON 序列化失败，返回简单格式
        format!("ERROR:{code}")
    })
}
