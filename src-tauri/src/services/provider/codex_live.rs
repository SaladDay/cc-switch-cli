use super::codex_login::*;
use serde_json::Value;
// CLI entry-point adapter for upstream codex_login, CodexProjection and the
// durable live-file engine. Upstream managed accounts and Stack are not exposed
// by this CLI. Credential decisions and file recovery use upstream code.
pub(crate) fn stash_path() -> std::path::PathBuf {
    crate::live::engine::DeviceStore::for_device().file(STASH_FILENAME)
}

pub(crate) fn read_stash_bytes() -> Result<Option<Vec<u8>>, crate::error::AppError> {
    let path = stash_path();
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(crate::error::AppError::io(&path, err)),
    }
}

pub(crate) fn restore_stash(bytes: Option<&[u8]>) -> Result<(), crate::error::AppError> {
    let path = stash_path();
    match bytes {
        Some(bytes) => crate::config::atomic_write_private(&path, bytes),
        None if path.exists() => crate::config::delete_file(&path),
        None => Ok(()),
    }
}

pub(crate) fn prepare(
    official: bool,
    row_auth: &Value,
    config: &str,
    rows: &[crate::provider::Provider],
    previous: Option<&crate::provider::Provider>,
) -> Result<Prepared, crate::error::AppError> {
    use super::ProviderService;
    use crate::codex_config::*;
    let is_official = |provider: &&crate::provider::Provider| {
        ProviderService::codex_live_write_category(provider) == Some("official")
    };
    let mut keys: Vec<String> = rows
        .iter()
        .filter(|p| !is_official(p))
        .filter_map(|p| {
            p.settings_config
                .get("auth")
                .and_then(extract_codex_auth_api_key)
        })
        .collect();
    if !official {
        if let Some(key) = extract_codex_auth_api_key(row_auth) {
            keys.push(key);
        }
    }
    let bytes = read_stash_bytes()?;
    let stash_pre = crate::live::engine::digest(bytes.as_deref());
    let (stash, unreadable) = match bytes {
        Some(bytes) => match serde_json::from_slice::<LoginStash>(&bytes) {
            Ok(mut stash) => {
                stash.initialized = true;
                (stash, false)
            }
            Err(_) => (
                LoginStash {
                    initialized: true,
                    ..Default::default()
                },
                true,
            ),
        },
        None => (
            LoginStash::seeded_from_rows(
                rows.iter()
                    .filter(is_official)
                    .filter_map(|p| p.settings_config.get("auth"))
                    .chain(official.then_some(row_auth)),
            ),
            false,
        ),
    };
    let path = get_codex_auth_path();
    let auth_bytes = crate::live::engine::read_current(&path)?;
    let auth_pre = crate::live::engine::digest(auth_bytes.as_deref());
    let live = auth_bytes
        .as_deref()
        .map(serde_json::from_slice::<Value>)
        .transpose()
        .map_err(|source| crate::error::AppError::JsonSerialize { source })?;
    let preserve = crate::settings::preserve_codex_official_auth_on_switch();
    let proxy = !official
        && extract_codex_auth_api_key(row_auth).as_deref()
            == Some(crate::live::project::codex::PROXY_TOKEN_PLACEHOLDER);
    let plan = plan(AuthInput {
        live: live.as_ref(),
        live_is_managed: false,
        third_party_keys: &keys,
        leaving_official: previous
            .filter(|p| is_official(p))
            .and_then(|p| p.settings_config.get("auth")),
        target: if official {
            AuthTarget::Official { row_auth }
        } else if proxy {
            AuthTarget::ProxyThirdParty
        } else {
            AuthTarget::ThirdParty { preserve }
        },
        stash,
    });
    if unreadable && plan.stash.is_some() {
        return Err(crate::error::AppError::Config(format!(
            "Cannot update unreadable Codex login stash {}; repair or move it before switching",
            stash_path().display()
        )));
    }
    use crate::live::project::codex::{
        requires_openai_auth, CodexConfigPatch, CodexProjection, Route, RouteWrite, RowInput,
    };
    let settings = serde_json::json!({ "auth": row_auth, "config": config });
    let projection = CodexProjection::of(&RowInput {
        settings: &settings,
        official,
        proxy_injected_oauth: false,
    })?;
    let live_config = crate::codex_config::read_and_validate_codex_config_text()?;
    let login = match codex_config_auth_store_mode(&live_config) {
        CodexAuthStoreMode::File => plan.login_on_disk,
        CodexAuthStoreMode::Ephemeral => false,
        _ => official || proxy || preserve,
    };
    let route = match projection.route {
        Route::Official if crate::settings::unify_codex_session_history() => {
            RouteWrite::OfficialMirror
        }
        Route::Official => RouteWrite::Official {
            dormant_base_url: format!(
                "http://127.0.0.1:{}/v1",
                crate::proxy::types::ProxyConfig::default().listen_port
            ),
        },
        Route::Custom { mut table, auth } => {
            table.insert(
                "requires_openai_auth",
                toml_edit::value(requires_openai_auth(auth, login)),
            );
            RouteWrite::Custom(table)
        }
        Route::BuiltIn { id, table } => RouteWrite::BuiltIn { id, table },
        Route::Default => RouteWrite::Default,
    };
    let outgoing = previous
        .and_then(|p| {
            CodexProjection::of(&RowInput {
                settings: &p.settings_config,
                official: is_official(&p),
                proxy_injected_oauth: false,
            })
            .ok()
        })
        .map(|p| p.exclusive)
        .unwrap_or_default();
    Ok(Prepared {
        common: Default::default(),
        auth: plan,
        auth_pre,
        stash_pre,
        config: CodexConfigPatch {
            top: projection.top,
            nested: projection.nested,
            exclusive: projection.exclusive,
            outgoing,
            route,
            catalog: false,
            retired: retired_tables(rows),
        },
    })
}

#[derive(Clone)]
pub(crate) struct Prepared {
    pub common: crate::live::patch::toml::TomlPatch,
    pub auth: AuthPlan,
    pub auth_pre: Option<String>,
    pub stash_pre: Option<String>,
    pub config: crate::live::project::codex::CodexConfigPatch,
}

pub(crate) fn apply(
    prepared: &Prepared,
    catalog: Option<&Value>,
    db_target: Option<(&crate::Database, Option<&str>)>,
) -> Result<(), crate::error::AppError> {
    use crate::live::engine::{lock_app, DeviceStore, LiveFile};
    use crate::live::patch::{Guarded, WholeFile};
    use crate::mode::{operation, state::PendingTarget};
    let store = DeviceStore::for_device();
    let guard = lock_app("codex");
    let commit = |target: &PendingTarget| {
        if let Some((db, _)) = db_target {
            operation::commit_target(db, &store, "codex", target)
        } else if target.pointer.is_some() {
            Err(crate::error::AppError::Config(
                "An unfinished Codex switch needs startup recovery before syncing".into(),
            ))
        } else {
            Ok(())
        }
    };
    operation::recover_before_write(&store, &guard, &commit)?;
    let auth = prepared
        .auth
        .auth
        .as_ref()
        .map(|auth| match auth {
            Some(auth) => serde_json::to_vec_pretty(auth).map(WholeFile::Write),
            None => Ok(WholeFile::Delete),
        })
        .transpose()
        .map_err(|source| crate::error::AppError::JsonSerialize { source })?
        .map(|then| Guarded {
            expected_pre: prepared.auth_pre.clone(),
            then,
        });
    let stash = prepared
        .auth
        .stash
        .as_ref()
        .map(serde_json::to_vec_pretty)
        .transpose()
        .map_err(|source| crate::error::AppError::JsonSerialize { source })?
        .map(|bytes| Guarded {
            expected_pre: prepared.stash_pre.clone(),
            then: WholeFile::Write(bytes),
        });
    let catalog = catalog
        .map(serde_json::to_vec_pretty)
        .transpose()
        .map_err(|source| crate::error::AppError::JsonSerialize { source })?
        .map(WholeFile::Write);
    let config_steps =
        crate::live::patch::toml::TomlSteps(vec![&prepared.common, &prepared.config]);
    let mut changes = vec![operation::FileChange {
        file: LiveFile::private(crate::codex_config::get_codex_config_path()),
        patch: &config_steps,
    }];
    if let Some(patch) = &auth {
        changes.push(operation::FileChange {
            file: LiveFile::private(crate::codex_config::get_codex_auth_path()),
            patch,
        });
    }
    if let Some(patch) = &catalog {
        changes.push(operation::FileChange {
            file: LiveFile::shared(crate::codex_config::get_codex_model_catalog_path()),
            patch,
        });
    }
    if let Some(patch) = &stash {
        changes.push(operation::FileChange {
            file: LiveFile::private(stash_path()),
            patch,
        });
    }
    operation::run(
        &store,
        &guard,
        "switch",
        &changes,
        PendingTarget::pointer(db_target.and_then(|(_, id)| id.map(str::to_owned))),
        &commit,
    )?;
    Ok(())
}

fn retired_tables(
    rows: &[crate::provider::Provider],
) -> Vec<crate::live::project::codex::KnownTable> {
    use crate::live::project::codex::{KnownTable, ROUTE_ID};
    use toml_edit::Item;
    let mut retired = Vec::new();
    for provider in rows {
        if super::ProviderService::codex_live_write_category(provider) == Some("official") {
            continue;
        }
        let Some(doc) = provider
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        else {
            continue;
        };
        let providers = doc.get("model_providers").and_then(Item::as_table_like);
        let base_url_of = |id: &str| {
            providers
                .and_then(|table| table.get(id))
                .and_then(Item::as_table_like)
                .and_then(|table| table.get("base_url"))
                .and_then(Item::as_str)
                .map(|url| url.trim().to_string())
        };
        let selector = doc
            .get("model_provider")
            .and_then(Item::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty() && *id != ROUTE_ID);
        if let Some((id, base_url)) = selector.and_then(|id| Some((id, base_url_of(id)?))) {
            retired.push(KnownTable {
                id: id.to_string(),
                base_url,
            });
        }
        if let Some(base_url) = doc
            .get("openai_base_url")
            .and_then(Item::as_str)
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty())
        {
            retired.push(KnownTable {
                id: "cc-switch".to_string(),
                base_url,
            });
        }
    }
    retired
}

/// The CLI still exposes a common-snippet editor. Express its explicit edits as
/// upstream TOML patch operations; switching itself only projects provider keys.
pub(crate) fn common_patch(
    current: Option<&str>,
    previous: Option<&str>,
) -> Result<crate::live::patch::toml::TomlPatch, crate::error::AppError> {
    use crate::live::patch::{toml::TomlPatch, KeyPath};
    fn entries(
        table: &dyn toml_edit::TableLike,
        prefix: &[String],
        out: &mut Vec<(KeyPath, toml_edit::Item)>,
    ) {
        for (key, item) in table.iter() {
            let mut path = prefix.to_vec();
            path.push(key.to_string());
            if let Some(table) = item.as_table_like() {
                entries(table, &path, out);
            } else {
                out.push((KeyPath(path), item.clone()));
            }
        }
    }
    let parse = |text: &str| -> Result<_, crate::error::AppError> {
        let doc = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|err| crate::error::AppError::Config(err.to_string()))?;
        let mut out = Vec::new();
        entries(doc.as_table(), &[], &mut out);
        Ok(out)
    };
    let set = current.map(parse).transpose()?.unwrap_or_default();
    // Legacy invalid snippets are tolerated during replacement, as in the CLI's
    // existing editor. Only successfully parsed, unchanged old values are removed.
    let remove_if = previous
        .and_then(|text| parse(text).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(path, item)| item.as_value().map(|v| (path, vec![v.clone()])))
        .collect();
    Ok(TomlPatch {
        set,
        remove_if,
        ..Default::default()
    })
}
