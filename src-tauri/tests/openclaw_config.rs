use serde_json::json;
use serial_test::serial;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::Path;

mod error {
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
        #[error("JSON 序列化失败: {source}")]
        JsonSerialize {
            #[source]
            source: serde_json::Error,
        },
        #[error("锁获取失败: {0}")]
        Lock(String),
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

        pub fn localized(key: &'static str, zh: impl Into<String>, en: impl Into<String>) -> Self {
            Self::Localized {
                key,
                zh: zh.into(),
                en: en.into(),
            }
        }
    }

    impl<T> From<PoisonError<T>> for AppError {
        fn from(err: PoisonError<T>) -> Self {
            Self::Lock(err.to_string())
        }
    }
}

mod config {
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use crate::error::AppError;

    pub fn home_dir() -> Option<PathBuf> {
        crate::test_support::test_home_override().or_else(dirs::home_dir)
    }

    pub fn get_app_config_dir() -> PathBuf {
        home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".cc-switch")
    }

    pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), AppError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| AppError::io(parent, err))?;
        }

        let parent = path
            .parent()
            .ok_or_else(|| AppError::Config("无效的路径".to_string()))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| AppError::Config("无效的文件名".to_string()))?
            .to_string_lossy()
            .to_string();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = parent.join(format!("{file_name}.tmp.{ts}"));

        {
            let mut file = fs::File::create(&tmp).map_err(|err| AppError::io(&tmp, err))?;
            file.write_all(data)
                .map_err(|err| AppError::io(&tmp, err))?;
            file.flush().map_err(|err| AppError::io(&tmp, err))?;
        }

        fs::rename(&tmp, path).map_err(|err| AppError::io(path, err))?;
        Ok(())
    }
}

mod provider {
    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use std::collections::HashMap;

    #[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
    #[serde(rename_all = "camelCase")]
    pub struct OpenClawProviderConfig {
        #[serde(skip_serializing_if = "Option::is_none")]
        pub base_url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub api_key: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub api: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub models: Vec<OpenClawModelEntry>,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        pub headers: HashMap<String, String>,
        #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
        pub extra: HashMap<String, Value>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
    #[serde(rename_all = "camelCase")]
    pub struct OpenClawModelEntry {
        pub id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub alias: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub cost: Option<OpenClawModelCost>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub context_window: Option<u32>,
        #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
        pub extra: HashMap<String, Value>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
    pub struct OpenClawModelCost {
        pub input: f64,
        pub output: f64,
        #[serde(flatten, default, skip_serializing_if = "HashMap::is_empty")]
        pub extra: HashMap<String, Value>,
    }
}

mod settings {
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;
    use std::sync::{OnceLock, RwLock};

    use crate::error::AppError;

    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct AppSettings {
        pub openclaw_config_dir: Option<String>,
        pub backup_retain_count: Option<u32>,
    }

    fn settings_store() -> &'static RwLock<AppSettings> {
        static STORE: OnceLock<RwLock<AppSettings>> = OnceLock::new();
        STORE.get_or_init(|| RwLock::new(AppSettings::default()))
    }

    pub fn get_settings() -> AppSettings {
        settings_store().read().expect("read settings lock").clone()
    }

    pub fn update_settings(new_settings: AppSettings) -> Result<(), AppError> {
        let mut guard = settings_store().write().expect("write settings lock");
        *guard = new_settings;
        Ok(())
    }

    pub(crate) fn reload_test_settings() {
        // The integration-test shim keeps settings in memory only so fixture cleanup
        // cannot accidentally write back to the developer's real HOME.
    }

    pub fn get_openclaw_override_dir() -> Option<PathBuf> {
        get_settings().openclaw_config_dir.map(PathBuf::from)
    }

    pub fn effective_backup_retain_count() -> usize {
        get_settings()
            .backup_retain_count
            .map(|count| usize::try_from(count).unwrap_or(usize::MAX).max(1))
            .unwrap_or(10)
    }
}

mod test_support {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard, OnceLock, RwLock};

    pub(crate) type TestHomeSettingsLock = MutexGuard<'static, ()>;

    pub(crate) fn lock_test_home_and_settings() -> TestHomeSettingsLock {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn test_home_store() -> &'static RwLock<Option<PathBuf>> {
        static STORE: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();
        STORE.get_or_init(|| RwLock::new(None))
    }

    pub(crate) fn set_test_home_override(path: Option<&Path>) {
        let mut guard = test_home_store()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = path.map(Path::to_path_buf);
    }

    pub(crate) fn test_home_override() -> Option<PathBuf> {
        test_home_store()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

#[path = "../src/openclaw_config.rs"]
mod openclaw_config_impl;

use openclaw_config_impl::{
    get_default_model, get_openclaw_config_path, read_openclaw_config, remove_provider,
    scan_openclaw_config_health, set_agents_defaults, set_default_model, set_env_config,
    set_model_catalog, set_provider, set_tools_config, OpenClawAgentsDefaults,
    OpenClawDefaultModel, OpenClawEnvConfig, OpenClawModelCatalogEntry, OpenClawToolsConfig,
};
use settings::{get_settings, update_settings, AppSettings};
use tempfile::TempDir;

struct FixtureGuard {
    _temp: TempDir,
    old_home: Option<OsString>,
    old_test_home: Option<OsString>,
    old_settings: AppSettings,
}

impl FixtureGuard {
    fn new(source: &str) -> Self {
        let temp = tempfile::tempdir().expect("create tempdir");
        let openclaw_dir = temp.path().join(".openclaw");
        fs::create_dir_all(&openclaw_dir).expect("create openclaw dir");
        fs::write(openclaw_dir.join("openclaw.json"), source).expect("seed openclaw config");

        let old_home = std::env::var_os("HOME");
        let old_test_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        let old_settings = get_settings();

        std::env::set_var("HOME", temp.path());
        std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());

        update_settings(AppSettings {
            openclaw_config_dir: Some(openclaw_dir.display().to_string()),
            backup_retain_count: None,
        })
        .expect("set openclaw override dir");

        Self {
            _temp: temp,
            old_home,
            old_test_home,
            old_settings,
        }
    }
}

impl Drop for FixtureGuard {
    fn drop(&mut self) {
        if let Some(value) = self.old_home.take() {
            std::env::set_var("HOME", value);
        } else {
            std::env::remove_var("HOME");
        }

        if let Some(value) = self.old_test_home.take() {
            std::env::set_var("CC_SWITCH_TEST_HOME", value);
        } else {
            std::env::remove_var("CC_SWITCH_TEST_HOME");
        }

        update_settings(self.old_settings.clone()).expect("restore settings");
    }
}

fn with_fixture<T>(source: &str, test: impl FnOnce(&Path) -> T) -> T {
    let _guard = FixtureGuard::new(source);
    let config_path = get_openclaw_config_path();
    test(&config_path)
}

fn warning_codes() -> Vec<String> {
    scan_openclaw_config_health()
        .expect("scan config health")
        .into_iter()
        .map(|warning| warning.code)
        .collect()
}

fn shared_round_trip_boundary_fixture() -> &'static str {
    r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        baseUrl: 'https://keep.example/v1',
        apiKey: 'sk-keep',
        models: [
          {
            id: 'model-primary',
          },
        ],
        unknownProviderKey: 'keep-me',
        nestedUnknown: {
          enabled: true,
        },
      },
      other: {
        baseUrl: 'https://other.example/v1',
        models: [
          {
            id: 'model-fallback',
          },
        ],
        headers: {
          'X-Other': 'preserve-me',
        },
      },
      remove: {
        baseUrl: 'https://remove.example/v1',
      },
    },
  },
  agents: {
    defaults: {
      timeoutSeconds: 45,
    },
    sibling: {
      enabled: true,
      retries: 2,
    },
  },
  tools: {
    profile: 'coding',
  },
}
"#
}

#[test]
#[serial]
fn openclaw_health_scan_reports_parse_failures_from_backend_source_of_truth() {
    with_fixture("{ broken: [ }", |config_path| {
        let warnings = scan_openclaw_config_health().expect("scan parse warning");
        let expected_path = config_path.display().to_string();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "config_parse_failed");
        assert_eq!(warnings[0].path.as_deref(), Some(expected_path.as_str()));
    });
}

#[test]
#[serial]
fn openclaw_health_scan_reports_profile_and_env_shape_warnings() {
    let source = r#"{
  models: {
    mode: 'merge',
    providers: {},
  },
  tools: {
    profile: 'default',
  },
  agents: {
    defaults: {
      timeout: 30,
    },
  },
  env: {
    vars: '[object Object]',
    shellEnv: false,
  },
}
"#;

    with_fixture(source, |_| {
        let codes = warning_codes();
        assert!(codes.contains(&"invalid_tools_profile".to_string()));
        assert!(codes.contains(&"legacy_agents_timeout".to_string()));
        assert!(codes.contains(&"stringified_env_vars".to_string()));
        assert!(codes.contains(&"stringified_env_shell_env".to_string()));
    });
}

#[test]
#[serial]
fn set_env_config_preserves_other_root_sections() {
    let source = r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        baseUrl: 'https://keep.example/v1',
        opaque: { nested: true },
      },
    },
  },
  tools: {
    profile: 'coding',
    telemetry: true,
  },
  agents: {
    defaults: {
      timeoutSeconds: 45,
    },
    sibling: {
      enabled: true,
    },
  },
}
"#;

    with_fixture(source, |_| {
        set_env_config(&OpenClawEnvConfig {
            vars: HashMap::from([
                ("vars".to_string(), json!({ "TOKEN": "value" })),
                ("shellEnv".to_string(), json!({ "PATH": "/usr/bin" })),
            ]),
        })
        .expect("write env section");

        let config = read_openclaw_config().expect("read written config");
        assert_eq!(config["env"]["vars"]["TOKEN"], json!("value"));
        assert_eq!(config["env"]["shellEnv"]["PATH"], json!("/usr/bin"));
        assert_eq!(config["models"]["mode"], json!("merge"));
        assert_eq!(
            config["models"]["providers"]["keep"]["opaque"]["nested"],
            json!(true)
        );
        assert_eq!(config["tools"]["profile"], json!("coding"));
        assert_eq!(config["agents"]["sibling"]["enabled"], json!(true));
    });
}

#[test]
#[serial]
fn set_tools_config_preserves_other_root_sections() {
    let source = r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        baseUrl: 'https://keep.example/v1',
      },
    },
  },
  env: {
    vars: {
      TOKEN: 'old',
    },
  },
  agents: {
    defaults: {
      timeoutSeconds: 45,
    },
    sibling: {
      enabled: true,
    },
  },
}
"#;

    with_fixture(source, |_| {
        set_tools_config(&OpenClawToolsConfig {
            profile: Some("coding".to_string()),
            allow: vec!["Read".to_string()],
            deny: vec!["Bash(rm:*)".to_string()],
            extra: HashMap::from([("telemetry".to_string(), json!(true))]),
        })
        .expect("write tools section");

        let config = read_openclaw_config().expect("read written config");
        assert_eq!(config["tools"]["profile"], json!("coding"));
        assert_eq!(config["tools"]["telemetry"], json!(true));
        assert_eq!(config["env"]["vars"]["TOKEN"], json!("old"));
        assert_eq!(
            config["models"]["providers"]["keep"]["baseUrl"],
            json!("https://keep.example/v1")
        );
        assert_eq!(config["agents"]["sibling"]["enabled"], json!(true));
    });
}

#[test]
#[serial]
fn set_tools_config_writes_effectively_empty_tools_object() {
    let source = r#"{
  models: {
    mode: 'merge',
    providers: {},
  },
}
"#;

    with_fixture(source, |config_path| {
        set_tools_config(&OpenClawToolsConfig::default())
            .expect("write empty tools section without panicking");

        let written = fs::read_to_string(config_path).expect("read config after empty tools write");
        let parsed: serde_json::Value =
            json5::from_str(&written).expect("parse config after empty tools write");

        assert_eq!(parsed["tools"], json!({}));
        assert!(
            written.contains("tools: {}"),
            "empty tools config should stay a valid empty object: {written}"
        );
    });
}

#[test]
#[serial]
fn provider_point_updates_preserve_models_mode_and_other_provider_keys() {
    with_fixture(shared_round_trip_boundary_fixture(), |_| {
        set_provider(
            "added",
            json!({
                "baseUrl": "https://added.example/v1",
                "apiKey": "sk-added",
            }),
        )
        .expect("add provider");
        remove_provider("remove").expect("remove provider");

        let config = read_openclaw_config().expect("read written config");
        assert_eq!(config["models"]["mode"], json!("merge"));
        assert_eq!(
            config["models"]["providers"]["keep"]["unknownProviderKey"],
            json!("keep-me")
        );
        assert_eq!(
            config["models"]["providers"]["added"]["apiKey"],
            json!("sk-added")
        );
        assert!(config["models"]["providers"].get("remove").is_none());
    });
}

#[test]
#[serial]
fn set_provider_allows_default_model_refs_to_become_dangling_without_rewriting_agents_section() {
    let source = r#"{
  // preserve root comment
  models: {
    mode: 'merge',
    providers: {
      keep: {
        baseUrl: 'https://keep.example/v1',
        apiKey: 'sk-keep',
        models: [
          { id: 'primary-model' },
          { id: 'fallback-model' },
        ],
      },
    },
  },
  agents: {
    defaults: {
      model: {
        primary: 'keep/fallback-model',
        fallbacks: ['keep/primary-model'],
      },
    },
  },
}
"#;

    with_fixture(source, |config_path| {
        set_provider(
            "keep",
            json!({
                "baseUrl": "https://keep.example/v2",
                "apiKey": "sk-keep",
                "models": [{ "id": "primary-model" }]
            }),
        )
        .expect("provider write should allow default model refs to become dangling");

        let written = fs::read_to_string(config_path).expect("read config after edit");
        let parsed: serde_json::Value = json5::from_str(&written).expect("parse rewritten config");
        assert_eq!(
            parsed["models"]["providers"]["keep"]["baseUrl"],
            json!("https://keep.example/v2")
        );
        assert_eq!(
            parsed["models"]["providers"]["keep"]["models"],
            json!([{ "id": "primary-model" }])
        );
        assert_eq!(
            parsed["agents"]["defaults"]["model"]["primary"],
            json!("keep/fallback-model")
        );
    });
}

#[test]
#[serial]
fn remove_provider_allows_default_model_refs_to_become_dangling_without_rewriting_agents_section() {
    let source = r#"{
  // preserve root comment
  models: {
    mode: 'merge',
    providers: {
      keep: {
        baseUrl: 'https://keep.example/v1',
        apiKey: 'sk-keep',
        models: [
          { id: 'primary-model' },
          { id: 'fallback-model' },
        ],
      },
    },
  },
  agents: {
    defaults: {
      model: {
        primary: 'keep/fallback-model',
        fallbacks: ['keep/primary-model'],
      },
    },
  },
}
"#;

    with_fixture(source, |config_path| {
        remove_provider("keep").expect("provider removal should allow dangling default model refs");

        let written = fs::read_to_string(config_path).expect("read config after remove");
        let parsed: serde_json::Value = json5::from_str(&written).expect("parse rewritten config");
        assert!(parsed["models"]["providers"].get("keep").is_none());
        assert_eq!(
            parsed["agents"]["defaults"]["model"]["primary"],
            json!("keep/fallback-model")
        );
    });
}

#[test]
#[serial]
fn set_provider_allows_agents_defaults_models_refs_to_become_dangling_and_keeps_agents_text() {
    let source = r#"{
  // preserve root comment
  models: {
    mode: 'merge',
    providers: {
      keep: {
        baseUrl: 'https://keep.example/v1',
        apiKey: 'sk-keep',
        models: [
          { id: 'primary-model' },
          { id: 'fallback-model' },
        ],
      },
    },
  },
  agents: {
    defaults: {
      models: {
        'keep/fallback-model': {
          alias: 'Fallback',
        },
      },
    },
  },
}
"#;

    with_fixture(source, |config_path| {
        set_provider(
            "keep",
            json!({
                "baseUrl": "https://keep.example/v2",
                "apiKey": "sk-keep",
                "models": [{ "id": "primary-model" }]
            }),
        )
        .expect("provider write should allow agents.defaults.models refs to become dangling");

        let written = fs::read_to_string(config_path).expect("read config after edit");
        let parsed: serde_json::Value = json5::from_str(&written).expect("parse rewritten config");
        assert_eq!(
            parsed["models"]["providers"]["keep"]["baseUrl"],
            json!("https://keep.example/v2")
        );
        assert_eq!(
            parsed["agents"]["defaults"]["models"]["keep/fallback-model"]["alias"],
            json!("Fallback")
        );
    });
}

#[test]
#[serial]
fn remove_provider_allows_agents_defaults_models_refs_to_become_dangling_and_keeps_agents_text() {
    let source = r#"{
  // preserve root comment
  models: {
    mode: 'merge',
    providers: {
      keep: {
        baseUrl: 'https://keep.example/v1',
        apiKey: 'sk-keep',
        models: [
          { id: 'primary-model' },
          { id: 'fallback-model' },
        ],
      },
    },
  },
  agents: {
    defaults: {
      models: {
        'keep/fallback-model': {
          alias: 'Fallback',
        },
      },
    },
  },
}
"#;

    with_fixture(source, |config_path| {
        remove_provider("keep")
            .expect("provider removal should allow dangling agents.defaults.models refs");

        let written = fs::read_to_string(config_path).expect("read config after remove");
        let parsed: serde_json::Value = json5::from_str(&written).expect("parse rewritten config");
        assert!(parsed["models"]["providers"].get("keep").is_none());
        assert_eq!(
            parsed["agents"]["defaults"]["models"]["keep/fallback-model"]["alias"],
            json!("Fallback")
        );
    });
}

#[test]
#[serial]
fn set_provider_ignores_invalid_default_model_reference_format() {
    let source = r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        baseUrl: 'https://keep.example/v1',
        models: [{ id: 'primary-model' }],
      },
    },
  },
  agents: {
    defaults: {
      model: {
        primary: 'keep/primary-model/extra',
        fallbacks: ['keep/', '/primary-model'],
      },
    },
  },
}
"#;

    with_fixture(source, |config_path| {
        set_provider(
            "keep",
            json!({
                "baseUrl": "https://keep.example/v2",
                "models": [{ "id": "primary-model" }]
            }),
        )
        .expect("invalid default model ref format should not block provider writes");

        let written = fs::read_to_string(config_path).expect("read config after write");
        let parsed: serde_json::Value = json5::from_str(&written).expect("parse rewritten config");
        assert_eq!(
            parsed["models"]["providers"]["keep"]["baseUrl"],
            json!("https://keep.example/v2")
        );
        assert_eq!(
            parsed["agents"]["defaults"]["model"]["primary"],
            json!("keep/primary-model/extra")
        );
    });
}

#[test]
#[serial]
fn remove_provider_ignores_invalid_model_catalog_reference_format() {
    let source = r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        baseUrl: 'https://keep.example/v1',
        models: [{ id: 'primary-model' }],
      },
    },
  },
  agents: {
    defaults: {
      models: {
        'keep/primary-model/extra': {
          alias: 'Broken',
        },
      },
    },
  },
}
"#;

    with_fixture(source, |config_path| {
        remove_provider("keep")
            .expect("invalid model catalog ref format should not block provider removal");

        let written = fs::read_to_string(config_path).expect("read config after remove");
        let parsed: serde_json::Value = json5::from_str(&written).expect("parse rewritten config");
        assert!(parsed["models"]["providers"].get("keep").is_none());
        assert_eq!(
            parsed["agents"]["defaults"]["models"]["keep/primary-model/extra"]["alias"],
            json!("Broken")
        );
    });
}

#[test]
#[serial]
fn set_agents_defaults_preserves_sibling_agents_keys() {
    with_fixture(shared_round_trip_boundary_fixture(), |_| {
        set_agents_defaults(&OpenClawAgentsDefaults {
            model: Some(OpenClawDefaultModel {
                primary: "keep/model-primary".to_string(),
                fallbacks: vec!["other/model-fallback".to_string()],
                extra: HashMap::from([("reasoningEffort".to_string(), json!("high"))]),
            }),
            models: None,
            extra: HashMap::from([("timeoutSeconds".to_string(), json!(60))]),
        })
        .expect("write agents.defaults");

        let config = read_openclaw_config().expect("read written config");
        assert_eq!(config["agents"]["defaults"]["timeoutSeconds"], json!(60));
        assert_eq!(config["agents"]["sibling"]["enabled"], json!(true));
        assert_eq!(config["agents"]["sibling"]["retries"], json!(2));
    });
}

#[test]
#[serial]
fn set_default_model_allows_dangling_refs() {
    let source = r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        baseUrl: 'https://keep.example/v1',
        models: [{ id: 'primary-model' }],
      },
    },
  },
}
"#;

    with_fixture(source, |config_path| {
        set_default_model(&OpenClawDefaultModel {
            primary: "keep/missing-model".to_string(),
            fallbacks: vec!["keep/primary-model".to_string()],
            extra: HashMap::new(),
        })
        .expect("dangling default-model refs should be writable");

        let written =
            fs::read_to_string(config_path).expect("read config after default-model write");
        let parsed: serde_json::Value = json5::from_str(&written).expect("parse rewritten config");
        assert_eq!(
            parsed["agents"]["defaults"]["model"]["primary"],
            json!("keep/missing-model")
        );
        assert_eq!(
            parsed["agents"]["defaults"]["model"]["fallbacks"],
            json!(["keep/primary-model"])
        );
    });
}

#[test]
#[serial]
fn set_model_catalog_allows_invalid_reference_format() {
    let source = r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        baseUrl: 'https://keep.example/v1',
        models: [{ id: 'primary-model' }],
      },
    },
  },
}
"#;

    with_fixture(source, |config_path| {
        set_model_catalog(&HashMap::from([(
            "keep/primary-model/extra".to_string(),
            OpenClawModelCatalogEntry {
                alias: Some("Broken".to_string()),
                extra: HashMap::new(),
            },
        )]))
        .expect("invalid model catalog refs should still be writable");

        let written =
            fs::read_to_string(config_path).expect("read config after model-catalog write");
        let parsed: serde_json::Value = json5::from_str(&written).expect("parse rewritten config");
        assert_eq!(
            parsed["agents"]["defaults"]["models"]["keep/primary-model/extra"]["alias"],
            json!("Broken")
        );
    });
}

#[test]
#[serial]
fn set_agents_defaults_allows_dangling_model_catalog_refs() {
    let source = r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        baseUrl: 'https://keep.example/v1',
        models: [{ id: 'primary-model' }],
      },
    },
  },
}
"#;

    with_fixture(source, |config_path| {
        set_agents_defaults(&OpenClawAgentsDefaults {
            model: Some(OpenClawDefaultModel {
                primary: "keep/primary-model".to_string(),
                fallbacks: Vec::new(),
                extra: HashMap::new(),
            }),
            models: Some(HashMap::from([(
                "missing/fallback-model".to_string(),
                OpenClawModelCatalogEntry {
                    alias: Some("Fallback".to_string()),
                    extra: HashMap::new(),
                },
            )])),
            extra: HashMap::new(),
        })
        .expect("dangling agents.defaults.models refs should be writable");

        let written =
            fs::read_to_string(config_path).expect("read config after agents.defaults write");
        let parsed: serde_json::Value = json5::from_str(&written).expect("parse rewritten config");
        assert_eq!(
            parsed["agents"]["defaults"]["models"]["missing/fallback-model"]["alias"],
            json!("Fallback")
        );
        assert_eq!(
            parsed["agents"]["defaults"]["model"]["primary"],
            json!("keep/primary-model")
        );
    });
}

#[test]
#[serial]
fn set_agents_defaults_allows_recovery_write_after_remove_provider_leaves_dangling_default_model() {
    let source = r#"{
  models: {
    mode: 'merge',
    providers: {
      keep: {
        baseUrl: 'https://keep.example/v1',
        models: [{ id: 'primary-model' }],
      },
      other: {
        baseUrl: 'https://other.example/v1',
        models: [{ id: 'other-model' }],
      },
    },
  },
  agents: {
    defaults: {
      model: {
        primary: 'keep/primary-model',
      },
    },
  },
}
"#;

    with_fixture(source, |_| {
        remove_provider("keep").expect("provider removal should create dangling default model");

        set_agents_defaults(&OpenClawAgentsDefaults {
            model: Some(OpenClawDefaultModel {
                primary: "keep/primary-model".to_string(),
                fallbacks: Vec::new(),
                extra: HashMap::new(),
            }),
            models: Some(HashMap::from([(
                "other/other-model".to_string(),
                OpenClawModelCatalogEntry {
                    alias: Some("Other".to_string()),
                    extra: HashMap::new(),
                },
            )])),
            extra: HashMap::from([("timeoutSeconds".to_string(), json!(30))]),
        })
        .expect(
            "agents.defaults should remain writable after provider removal leaves dangling refs",
        );

        let config = read_openclaw_config().expect("read config after recovery write");
        assert!(config["models"]["providers"].get("keep").is_none());
        assert_eq!(
            config["agents"]["defaults"]["model"]["primary"],
            json!("keep/primary-model")
        );
        assert_eq!(
            config["agents"]["defaults"]["models"]["other/other-model"]["alias"],
            json!("Other")
        );
        assert_eq!(config["agents"]["defaults"]["timeoutSeconds"], json!(30));
    });
}

#[test]
#[serial]
fn set_default_model_preserves_models_providers_entries() {
    with_fixture(shared_round_trip_boundary_fixture(), |_| {
        set_default_model(&OpenClawDefaultModel {
            primary: "keep/model-primary".to_string(),
            fallbacks: vec!["other/model-fallback".to_string()],
            extra: HashMap::from([("reasoningEffort".to_string(), json!("medium"))]),
        })
        .expect("write default model");

        let config = read_openclaw_config().expect("read written config");
        assert_eq!(
            config["agents"]["defaults"]["model"]["primary"],
            json!("keep/model-primary")
        );
        assert_eq!(
            config["models"]["providers"]["keep"]["unknownProviderKey"],
            json!("keep-me")
        );
        assert_eq!(
            config["models"]["providers"]["keep"]["nestedUnknown"]["enabled"],
            json!(true)
        );
        assert_eq!(
            config["models"]["providers"]["other"]["headers"]["X-Other"],
            json!("preserve-me")
        );

        let default_model = get_default_model()
            .expect("read default model")
            .expect("default model should exist");
        assert_eq!(default_model.primary, "keep/model-primary");
    });
}

#[test]
#[serial]
fn shared_round_trip_fixture_preserves_provider_and_agents_contracts() {
    with_fixture(shared_round_trip_boundary_fixture(), |_| {
        set_provider(
            "added",
            json!({
                "baseUrl": "https://added.example/v1",
                "apiKey": "sk-added",
            }),
        )
        .expect("add provider");
        remove_provider("remove").expect("remove provider");

        set_agents_defaults(&OpenClawAgentsDefaults {
            model: Some(OpenClawDefaultModel {
                primary: "keep/model-primary".to_string(),
                fallbacks: vec!["other/model-fallback".to_string()],
                extra: HashMap::from([("reasoningEffort".to_string(), json!("high"))]),
            }),
            models: None,
            extra: HashMap::from([("timeoutSeconds".to_string(), json!(60))]),
        })
        .expect("write agents.defaults");

        set_default_model(&OpenClawDefaultModel {
            primary: "keep/model-primary".to_string(),
            fallbacks: vec!["other/model-fallback".to_string()],
            extra: HashMap::from([("reasoningEffort".to_string(), json!("medium"))]),
        })
        .expect("write default model");

        let config = read_openclaw_config().expect("read written config");
        assert_eq!(config["models"]["mode"], json!("merge"));
        assert_eq!(
            config["models"]["providers"]["keep"]["unknownProviderKey"],
            json!("keep-me")
        );
        assert_eq!(
            config["models"]["providers"]["keep"]["nestedUnknown"]["enabled"],
            json!(true)
        );
        assert_eq!(
            config["models"]["providers"]["other"]["headers"]["X-Other"],
            json!("preserve-me")
        );
        assert_eq!(
            config["models"]["providers"]["added"]["apiKey"],
            json!("sk-added")
        );
        assert!(config["models"]["providers"].get("remove").is_none());
        assert_eq!(config["agents"]["defaults"]["timeoutSeconds"], json!(60));
        assert_eq!(config["agents"]["sibling"]["enabled"], json!(true));
        assert_eq!(config["agents"]["sibling"]["retries"], json!(2));
        assert_eq!(
            config["agents"]["defaults"]["model"]["primary"],
            json!("keep/model-primary")
        );
    });
}

#[test]
#[serial]
fn remove_last_provider_still_rewrites_models_section_differently_from_upstream_baseline() {
    let source = r#"{
  // preserve top-level comment
  models: {
    mode: 'merge',
    // upstream drops this comment when rewriting models
    providers: {
      only: { baseUrl: 'https://only.example/v1' },
    },
  },
  tools: {
    profile: 'coding',
  },
}
"#;

    with_fixture(source, |_| {
        remove_provider("only").expect("remove last provider");
        let written = fs::read_to_string(get_openclaw_config_path()).expect("read written config");

        assert!(
            !written.contains("// upstream drops this comment when rewriting models"),
            "upstream baseline rewrites the models section on last-provider removal"
        );
    });
}
