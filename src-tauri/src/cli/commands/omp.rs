//! OMP-native model and model-role commands.
//!
//! OMP does not have a single "current provider". Providers live alongside
//! one another in `models.yml`; the model selector resolves a concrete model
//! through `config.yml`'s `modelRoles` map. These commands deliberately edit
//! those native files instead of projecting them onto CC Switch's legacy
//! single-provider model.

use clap::Subcommand;
use serde_json::{Map, Value};

use crate::app_config::AppType;
use crate::cli::ui::{create_table, highlight, info, success};
use crate::error::AppError;

#[derive(Debug, Subcommand)]
pub enum OmpModelCommand {
    /// List models from all OMP providers, or one provider.
    List {
        /// Restrict output to one provider key.
        #[arg(long)]
        provider: Option<String>,
    },
    /// Add a model to an OMP provider.
    Add(ModelSetArgs),
    /// Update an existing OMP model.
    Edit(ModelSetArgs),
    /// Remove a model from an OMP provider.
    Delete {
        /// OMP provider key.
        provider: String,
        /// Model identifier.
        model: String,
    },
}

#[derive(Debug, clap::Args)]
pub struct ModelSetArgs {
    /// OMP provider key.
    pub provider: String,
    /// Model identifier.
    pub model: String,
    /// Complete model object as JSON. When omitted, field options update the
    /// existing model (or build a minimal object for `add`).
    #[arg(long, conflicts_with_all = ["name", "api", "reasoning", "context_window", "max_tokens"])]
    pub config: Option<String>,
    /// Display name.
    #[arg(long)]
    pub name: Option<String>,
    /// Per-model OMP API protocol override.
    #[arg(long)]
    pub api: Option<String>,
    /// Whether the model supports reasoning.
    #[arg(long)]
    pub reasoning: Option<bool>,
    /// Context window size in tokens.
    #[arg(long)]
    pub context_window: Option<u64>,
    /// Maximum output token count.
    #[arg(long)]
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Subcommand)]
pub enum OmpRoleCommand {
    /// List model-role assignments from OMP config.yml.
    List,
    /// Set one role to a provider/model selector.
    Set {
        /// Role name, for example `default`, `slow`, or `plan`.
        role: String,
        /// OMP selector such as `openai/gpt-5.6:high`, `@smol`, or `*`.
        selector: String,
    },
    /// Remove one role assignment so OMP falls back to its default resolution.
    Delete {
        /// Role name.
        role: String,
    },
}

/// Execute one of the top-level `cc-switch --app omp model ...` commands.
pub fn execute_model(command: OmpModelCommand, app_type: Option<AppType>) -> Result<(), AppError> {
    require_omp(app_type)?;
    match command {
        OmpModelCommand::List { provider } => list_models(provider),
        OmpModelCommand::Add(args) => set_model(args, false),
        OmpModelCommand::Edit(args) => set_model(args, true),
        OmpModelCommand::Delete { provider, model } => {
            if crate::omp_config::remove_omp_model(&provider, &model)? {
                println!(
                    "{}",
                    success(&format!("Removed OMP model '{provider}/{model}'"))
                );
            } else {
                println!(
                    "{}",
                    info(&format!("OMP model '{provider}/{model}' was not found"))
                );
            }
            Ok(())
        }
    }
}

/// Execute one of the top-level `cc-switch --app omp role ...` commands.
pub fn execute_role(command: OmpRoleCommand, app_type: Option<AppType>) -> Result<(), AppError> {
    require_omp(app_type)?;
    match command {
        OmpRoleCommand::List => list_roles(),
        OmpRoleCommand::Set { role, selector } => {
            crate::omp_config::set_omp_model_role(&role, Some(&selector), None)?;
            println!(
                "{}",
                success(&format!("Set OMP model role '{role}' to '{selector}'"))
            );
            Ok(())
        }
        OmpRoleCommand::Delete { role } => {
            crate::omp_config::set_omp_model_role(&role, None, None)?;
            println!("{}", success(&format!("Removed OMP model role '{role}'")));
            Ok(())
        }
    }
}

fn require_omp(app_type: Option<AppType>) -> Result<(), AppError> {
    if app_type == Some(AppType::Omp) {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "OMP model and role commands require --app omp".to_string(),
        ))
    }
}

fn list_models(provider_filter: Option<String>) -> Result<(), AppError> {
    println!(
        "OMP models: {}",
        crate::omp_config::get_omp_models_path()?.display()
    );
    let providers = crate::omp_config::read_omp_native_providers()?;
    let models = crate::omp_config::read_omp_native_models()?;
    let models = models
        .into_iter()
        .filter(|model| {
            provider_filter
                .as_deref()
                .is_none_or(|id| id == model.provider_id)
        })
        .collect::<Vec<_>>();

    if models.is_empty() {
        println!("{}", info("No OMP models found."));
        return Ok(());
    }

    let mut table = create_table();
    table.set_header(vec!["Provider", "Model", "Name", "API", "Reasoning"]);
    for model in models {
        let name = model
            .config
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("");
        let api = model
            .config
            .get("api")
            .and_then(Value::as_str)
            .or_else(|| {
                providers
                    .get(&model.provider_id)
                    .and_then(|provider| provider.get("api"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("");
        let reasoning = model
            .config
            .get("reasoning")
            .and_then(Value::as_bool)
            .map(|value| if value { "yes" } else { "no" })
            .unwrap_or("");
        table.add_row(vec![
            model.provider_id,
            model.model_id,
            name.to_string(),
            api.to_string(),
            reasoning.to_string(),
        ]);
    }
    println!("{}", table);
    Ok(())
}

fn list_roles() -> Result<(), AppError> {
    let (roles, path, _) = crate::omp_config::read_omp_model_roles_with_metadata()?;
    println!("OMP role writes: {}", path.display());
    if roles.is_empty() {
        println!("{}", info("No OMP model roles configured."));
        println!(
            "{}",
            highlight(&format!(
                "Built-in roles: {}",
                crate::omp_config::OMP_BUILTIN_MODEL_ROLES.join(", ")
            ))
        );
        return Ok(());
    }
    let mut table = create_table();
    table.set_header(vec!["Role", "Model"]);
    for (role, selector) in roles {
        table.add_row(vec![role, selector]);
    }
    println!("{}", table);
    Ok(())
}

fn set_model(args: ModelSetArgs, editing: bool) -> Result<(), AppError> {
    let (_, expected_revision) = crate::omp_config::read_omp_models_yaml()?;
    let existing = crate::omp_config::read_omp_native_models()?
        .into_iter()
        .find(|model| model.provider_id == args.provider && model.model_id == args.model)
        .map(|model| model.config);

    if editing && existing.is_none() {
        return Err(AppError::InvalidInput(format!(
            "OMP model '{}/{}' not found",
            args.provider, args.model
        )));
    }
    if !editing && existing.is_some() {
        return Err(AppError::InvalidInput(format!(
            "OMP model '{}/{}' already exists",
            args.provider, args.model
        )));
    }

    let mut model = match args.config {
        Some(config) => serde_json::from_str::<Value>(&config).map_err(|error| {
            AppError::InvalidInput(format!("--config must be valid JSON: {error}"))
        })?,
        None => existing.unwrap_or_else(|| Value::Object(Map::new())),
    };
    let object = model.as_object_mut().ok_or_else(|| {
        AppError::InvalidInput("OMP model configuration must be a JSON object".to_string())
    })?;
    object.insert("id".to_string(), Value::String(args.model.clone()));
    if let Some(value) = args.name {
        object.insert("name".to_string(), Value::String(value));
    }
    if let Some(value) = args.api {
        crate::omp_config::validate_api_protocol(&value)?;
        object.insert("api".to_string(), Value::String(value));
    }
    if let Some(value) = args.reasoning {
        object.insert("reasoning".to_string(), Value::Bool(value));
    }
    if let Some(value) = args.context_window {
        object.insert("contextWindow".to_string(), Value::from(value));
    }
    if let Some(value) = args.max_tokens {
        object.insert("maxTokens".to_string(), Value::from(value));
    }
    crate::omp_config::upsert_omp_model_checked(
        &args.provider,
        &args.model,
        model,
        &expected_revision,
    )?;
    let action = if editing { "Updated" } else { "Added" };
    println!(
        "{}",
        success(&format!(
            "{action} OMP model '{}/{}'",
            args.provider, args.model
        ))
    );
    Ok(())
}
