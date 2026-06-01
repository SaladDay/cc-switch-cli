use clap::Args;

use crate::cli::ui::{info, success};
use crate::error::AppError;
use crate::store::AppState;

#[derive(Args, Debug, Clone)]
pub struct DeeplinkCommand {
    /// The ccswitch://v1/import?... URL to import
    pub url: String,
}

pub fn execute(cmd: DeeplinkCommand) -> Result<(), AppError> {
    // The deep link URL carries its own `app`/`apps` parameters, so the global
    // `--app` flag is intentionally ignored here.
    let request = crate::parse_deeplink_url(&cmd.url)?;
    let state = AppState::try_new()?;

    match request.resource.as_str() {
        "provider" => import_provider(&state, request),
        "mcp" => import_mcp(&state, request),
        "prompt" => import_prompt(&state, request),
        "skill" => import_skill(&state, request),
        other => Err(AppError::InvalidInput(format!(
            "Unsupported resource type: {other}"
        ))),
    }
}

fn import_provider(
    state: &AppState,
    request: crate::DeepLinkImportRequest,
) -> Result<(), AppError> {
    let app_label = request.app.clone().unwrap_or_default();
    let name = request.name.clone().unwrap_or_default();
    let switched = request.enabled == Some(true);

    let provider_id = crate::import_provider_from_deeplink(state, request)?;

    println!(
        "{}",
        success(&format!(
            "✓ Imported provider '{name}' (id: {provider_id}) for {app_label}"
        ))
    );
    if switched {
        println!("{}", info(&format!("  Switched to '{provider_id}'")));
    }
    Ok(())
}

fn import_mcp(state: &AppState, request: crate::DeepLinkImportRequest) -> Result<(), AppError> {
    let apps_label = request.apps.clone().unwrap_or_default();
    let result = crate::import_mcp_from_deeplink(state, request)?;

    println!(
        "{}",
        success(&format!(
            "✓ Imported {} MCP server(s) for {apps_label}",
            result.imported_count
        ))
    );
    for id in &result.imported_ids {
        println!("{}", info(&format!("  • {id}")));
    }
    for failure in &result.failed {
        println!(
            "{}",
            crate::cli::ui::warning(&format!("  ✗ {}: {}", failure.id, failure.error))
        );
    }
    Ok(())
}

fn import_prompt(state: &AppState, request: crate::DeepLinkImportRequest) -> Result<(), AppError> {
    let app_label = request.app.clone().unwrap_or_default();
    let name = request.name.clone().unwrap_or_default();
    let enabled = request.enabled == Some(true);

    let prompt_id = crate::import_prompt_from_deeplink(state, request)?;

    println!(
        "{}",
        success(&format!(
            "✓ Imported prompt '{name}' (id: {prompt_id}) for {app_label}"
        ))
    );
    if enabled {
        println!("{}", info(&format!("  Enabled '{prompt_id}'")));
    }
    Ok(())
}

fn import_skill(state: &AppState, request: crate::DeepLinkImportRequest) -> Result<(), AppError> {
    let repo_id = crate::import_skill_from_deeplink(state, request)?;

    println!("{}", success(&format!("✓ Added skill repo '{repo_id}'")));
    println!(
        "{}",
        info("  Run `cc-switch skills repos sync` to install skills from it")
    );
    Ok(())
}
