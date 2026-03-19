use std::collections::HashMap;

use serde_json::Value;

use crate::app_config::AppType;
use crate::codex_config::{get_codex_auth_path, get_codex_config_path};
use crate::config::{delete_file, get_claude_settings_path, read_json_file, write_json_file};
use crate::error::AppError;

#[derive(Clone)]
pub(super) enum LiveSnapshot {
    Claude {
        settings: Option<Value>,
    },
    Codex {
        auth: Option<Value>,
        config: Option<String>,
    },
    Gemini {
        env: Option<HashMap<String, String>>,
        config: Option<Value>,
    },
    OpenCode {
        config: Option<Value>,
    },
    OpenClaw {
        config: Option<Value>,
    },
}

impl LiveSnapshot {
    pub(super) fn restore(&self) -> Result<(), AppError> {
        match self {
            LiveSnapshot::Claude { settings } => {
                let path = get_claude_settings_path();
                if let Some(value) = settings {
                    write_json_file(&path, value)?;
                } else if path.exists() {
                    delete_file(&path)?;
                }
            }
            LiveSnapshot::Codex { auth, config } => {
                let auth_path = get_codex_auth_path();
                let config_path = get_codex_config_path();
                if let Some(value) = auth {
                    write_json_file(&auth_path, value)?;
                } else if auth_path.exists() {
                    delete_file(&auth_path)?;
                }

                if let Some(text) = config {
                    crate::config::write_text_file(&config_path, text)?;
                } else if config_path.exists() {
                    delete_file(&config_path)?;
                }
            }
            LiveSnapshot::Gemini { env, config } => {
                use crate::gemini_config::{
                    get_gemini_env_path, get_gemini_settings_path, write_gemini_env_atomic,
                };

                let path = get_gemini_env_path();
                if let Some(env_map) = env {
                    write_gemini_env_atomic(env_map)?;
                } else if path.exists() {
                    delete_file(&path)?;
                }

                let settings_path = get_gemini_settings_path();
                match config {
                    Some(cfg) => {
                        write_json_file(&settings_path, cfg)?;
                    }
                    None if settings_path.exists() => {
                        delete_file(&settings_path)?;
                    }
                    _ => {}
                }
            }
            LiveSnapshot::OpenCode { config } => {
                let path = crate::opencode_config::get_opencode_config_path();
                if let Some(value) = config {
                    write_json_file(&path, value)?;
                } else if path.exists() {
                    delete_file(&path)?;
                }
            }
            LiveSnapshot::OpenClaw { config } => {
                let path = crate::openclaw_config::get_openclaw_config_path();
                if let Some(value) = config {
                    write_json_file(&path, value)?;
                } else if path.exists() {
                    delete_file(&path)?;
                }
            }
        }
        Ok(())
    }
}

pub(super) fn capture_live_snapshot(app_type: &AppType) -> Result<LiveSnapshot, AppError> {
    match app_type {
        AppType::Claude => {
            let path = get_claude_settings_path();
            let settings = if path.exists() {
                Some(read_json_file(&path)?)
            } else {
                None
            };
            Ok(LiveSnapshot::Claude { settings })
        }
        AppType::Codex => {
            let auth_path = get_codex_auth_path();
            let config_path = get_codex_config_path();
            let auth = if auth_path.exists() {
                Some(read_json_file(&auth_path)?)
            } else {
                None
            };
            let config = if config_path.exists() {
                Some(crate::codex_config::read_and_validate_codex_config_text()?)
            } else {
                None
            };
            Ok(LiveSnapshot::Codex { auth, config })
        }
        AppType::Gemini => {
            use crate::gemini_config::{
                get_gemini_env_path, get_gemini_settings_path, read_gemini_env,
            };

            let env_path = get_gemini_env_path();
            let env = if env_path.exists() {
                Some(read_gemini_env()?)
            } else {
                None
            };
            let settings_path = get_gemini_settings_path();
            let config = if settings_path.exists() {
                Some(read_json_file(&settings_path)?)
            } else {
                None
            };
            Ok(LiveSnapshot::Gemini { env, config })
        }
        AppType::OpenCode => {
            let path = crate::opencode_config::get_opencode_config_path();
            let config = if path.exists() {
                Some(crate::opencode_config::read_opencode_config()?)
            } else {
                None
            };
            Ok(LiveSnapshot::OpenCode { config })
        }
        AppType::OpenClaw => {
            let path = crate::openclaw_config::get_openclaw_config_path();
            let config = if path.exists() {
                Some(crate::openclaw_config::read_openclaw_config()?)
            } else {
                None
            };
            Ok(LiveSnapshot::OpenClaw { config })
        }
    }
}
