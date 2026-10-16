//! CLI transaction adapter for upstream Claude/Gemini projections and durable
//! writes. All field ownership, patching and recovery live in the upstream engine.
use super::PreparedLiveWrite;
use crate::error::AppError;
use crate::live::engine::{lock_app, DeviceStore, LiveFile};
use crate::mode::{operation, state::PendingTarget};

pub(super) fn apply(
    prepared: &PreparedLiveWrite,
    db_target: Option<(&crate::Database, Option<&str>)>,
) -> Result<(), AppError> {
    let (app, changes) = match prepared {
        PreparedLiveWrite::Claude { patch } => (
            "claude",
            vec![operation::FileChange {
                file: LiveFile::private(crate::config::get_claude_settings_path()),
                patch: patch as &dyn crate::live::patch::LivePatch,
            }],
        ),
        PreparedLiveWrite::Gemini { env, settings } => (
            "gemini",
            vec![
                operation::FileChange {
                    file: LiveFile::private(crate::gemini_config::get_gemini_env_path()),
                    patch: env as &dyn crate::live::patch::LivePatch,
                },
                operation::FileChange {
                    file: LiveFile::shared(crate::gemini_config::get_gemini_settings_path()),
                    patch: settings as &dyn crate::live::patch::LivePatch,
                },
            ],
        ),
        _ => return Ok(()),
    };
    if let Some((db, pointer)) = db_target {
        let write = operation::AppWrite::begin(db, app)?;
        write.run(
            "switch",
            &changes,
            PendingTarget::pointer(pointer.map(str::to_owned)),
        )?;
    } else {
        // Temporary CLI launches have no database target. They still use the
        // same lock and recovery rules, but cannot settle a pending DB switch.
        let store = DeviceStore::for_device();
        let guard = lock_app(app);
        let commit = |target: &PendingTarget| {
            if target.pointer.is_some() {
                Err(AppError::Config(
                    "An unfinished switch needs startup recovery before syncing".into(),
                ))
            } else {
                Ok(())
            }
        };
        operation::recover_before_write(&store, &guard, &commit)?;
        operation::run(
            &store,
            &guard,
            "apply",
            &changes,
            PendingTarget::default(),
            &commit,
        )?;
    }
    Ok(())
}

/// The CLI still has an explicit common-snippet command. Only an explicit edit
/// submits these global keys; ordinary switches never replay saved snippets.
pub(super) fn edit_common(
    prepared: &mut PreparedLiveWrite,
    current: Option<&str>,
    previous: Option<&str>,
) -> Result<(), AppError> {
    use crate::live::patch::KeyPath;
    use serde_json::Value;
    fn parse(text: &str) -> Result<Value, AppError> {
        let value: Value =
            serde_json::from_str(text).map_err(|source| AppError::Config(source.to_string()))?;
        if !value.is_object() {
            return Err(AppError::Config(
                "Common config must be a JSON object".into(),
            ));
        }
        Ok(value)
    }
    fn entries(value: &Value, path: KeyPath, out: &mut Vec<(KeyPath, Value)>) {
        if let Some(map) = value.as_object() {
            for (key, value) in map {
                entries(value, path.child(key), out);
            }
        } else {
            out.push((path, value.clone()));
        }
    }
    fn global_claude(path: &KeyPath) -> bool {
        use crate::live::floor;
        match path.0.as_slice() {
            [top, key, ..] if top == "env" => {
                !floor::claude_floor_env(key) && !floor::claude_exclusive_env(key)
            }
            [top, ..] => top != "env" && !floor::claude_floor_top(top),
            _ => false,
        }
    }
    let current = current
        .map(parse)
        .transpose()?
        .unwrap_or(serde_json::json!({}));
    let previous = previous
        .and_then(|s| parse(s).ok())
        .unwrap_or(serde_json::json!({}));
    match prepared {
        PreparedLiveWrite::Claude { patch } => {
            let mut set = Vec::new();
            entries(&current, KeyPath::root(), &mut set);
            // Explicit common edits may change global settings; provider keys
            // retain the values selected by the provider projection.
            set.retain(|(path, _)| {
                global_claude(path) && !patch.set.iter().any(|(key, _)| key == path)
            });
            patch.set.extend(set);
            let mut old = Vec::new();
            entries(&previous, KeyPath::root(), &mut old);
            old.retain(|(path, _)| global_claude(path));
            // Objects that only held removed common keys would otherwise stay
            // behind as `{}`; objects the new snippet still declares are kept.
            for (path, _) in &old {
                for depth in 1..path.0.len() {
                    let parent = KeyPath(path.0[..depth].to_vec());
                    if crate::live::patch::json::value_at(&current, &parent).is_none()
                        && !patch.prune_emptied.contains(&parent)
                    {
                        patch.prune_emptied.push(parent);
                    }
                }
            }
            patch
                .remove_if
                .extend(old.into_iter().map(|(path, value)| (path, vec![value])));
        }
        PreparedLiveWrite::Gemini { env, .. } => {
            for (key, value) in current.as_object().unwrap() {
                if crate::live::floor::gemini_floor_env(key) {
                    continue;
                }
                if !env.set.iter().any(|(name, _)| name == key) {
                    if let Some(value) = value.as_str() {
                        env.set.push((key.clone(), value.to_owned()));
                    }
                }
            }
            for (key, value) in previous.as_object().unwrap() {
                if crate::live::floor::gemini_floor_env(key) {
                    continue;
                }
                if let Some(value) = value.as_str() {
                    env.remove_if.push((key.clone(), vec![value.to_owned()]));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

impl super::ProviderService {
    pub(crate) fn is_official_gemini(provider: &crate::provider::Provider) -> bool {
        provider.category.as_deref() == Some("official")
            || matches!(
                Self::detect_gemini_auth_type(provider),
                super::GeminiAuthType::GoogleOfficial
            )
    }
    pub(crate) fn clear_key_fields(
        db: &crate::Database,
        app: &crate::app_config::AppType,
    ) -> Result<(), AppError> {
        use crate::app_config::AppType;
        let prepared = match app {
            AppType::Claude => PreparedLiveWrite::Claude {
                patch: crate::live::project::claude::direct_patch(None, &Default::default()),
            },
            AppType::Gemini => {
                let empty = crate::live::project::gemini::GeminiProjection::empty();
                PreparedLiveWrite::Gemini {
                    env: empty.env_patch(),
                    settings: empty.settings_patch(),
                }
            }
            _ => {
                return Err(AppError::Config(
                    "Unsupported key-field JSON adapter".into(),
                ))
            }
        };
        apply(&prepared, Some((db, None)))
    }

    /// Adapter for the CLI proxy lifecycle. Its target snapshot supplies only
    /// provider-owned fields; user configuration is always read by the engine.
    pub(crate) fn write_key_fields(
        db: &crate::Database,
        app: &crate::app_config::AppType,
        settings: &serde_json::Value,
        previous: Option<&crate::provider::Provider>,
        official: bool,
    ) -> Result<(), AppError> {
        use crate::app_config::AppType;
        let prepared = match app {
            AppType::Claude => {
                use crate::live::project::claude::{direct_patch, ClaudeProjection};
                let previous = previous.map(|p| ClaudeProjection::of(&p.settings_config));
                PreparedLiveWrite::Claude {
                    patch: direct_patch(previous.as_ref(), &ClaudeProjection::of(settings)),
                }
            }
            AppType::Gemini => {
                let projection =
                    crate::live::project::gemini::GeminiProjection::of(settings, official);
                PreparedLiveWrite::Gemini {
                    env: projection.env_patch(),
                    settings: projection.settings_patch(),
                }
            }
            _ => {
                return Err(AppError::Config(
                    "Unsupported key-field JSON adapter".into(),
                ))
            }
        };
        apply(&prepared, Some((db, None)))
    }
}
