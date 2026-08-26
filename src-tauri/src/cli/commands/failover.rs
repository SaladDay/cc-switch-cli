use clap::{Subcommand, ValueEnum};

use crate::app_config::AppType;
use crate::cli::failover_policy::{ensure_auto_failover_queue_ready, inspect_auto_failover_gate};
use crate::cli::ui::{create_table, highlight, info, success};
use crate::database::FailoverQueueItem;
use crate::error::AppError;
use crate::proxy::types::ProxyTakeoverStatus;
use crate::services::provider::ProviderSortUpdate;
use crate::services::ProviderService;
use crate::AppState;

#[derive(Subcommand, Debug, Clone)]
pub enum FailoverCommand {
    /// Show automatic failover status and queue
    Show,

    /// Enable automatic failover for the selected app
    Enable,

    /// Disable automatic failover for the selected app
    Disable,

    /// List queued failover providers
    List,

    /// List providers that can be added to the failover queue
    Available,

    /// Add a provider to the failover queue
    Add { id: String },

    /// Remove a provider from the failover queue
    Remove { id: String },

    /// Move a queued provider up or down
    Move {
        id: String,
        #[arg(value_enum)]
        direction: FailoverMoveDirection,
    },

    /// Clear the failover queue
    Clear {
        /// Confirm clearing the queue
        #[arg(long)]
        yes: bool,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailoverMoveDirection {
    Up,
    Down,
}

pub fn execute(cmd: FailoverCommand, app: Option<AppType>) -> Result<(), AppError> {
    let app_type = app.unwrap_or(AppType::Claude);
    match cmd {
        FailoverCommand::Show => show_failover(app_type),
        FailoverCommand::Enable => set_auto_failover(app_type, true),
        FailoverCommand::Disable => set_auto_failover(app_type, false),
        FailoverCommand::List => list_queue(app_type),
        FailoverCommand::Available => list_available(app_type),
        FailoverCommand::Add { id } => add_provider(app_type, &id),
        FailoverCommand::Remove { id } => remove_provider(app_type, &id),
        FailoverCommand::Move { id, direction } => move_provider(app_type, &id, direction),
        FailoverCommand::Clear { yes } => clear_queue(app_type, yes),
    }
}

fn get_state() -> Result<AppState, AppError> {
    AppState::try_new()
}

fn create_runtime() -> Result<tokio::runtime::Runtime, AppError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("failed to create async runtime: {e}")))
}

fn ensure_failover_supported(app_type: &AppType) -> Result<(), AppError> {
    if app_type.supports_failover() {
        Ok(())
    } else {
        Err(AppError::InvalidInput(format!(
            "failover is not supported for {}",
            app_type.as_str()
        )))
    }
}

fn show_failover(app_type: AppType) -> Result<(), AppError> {
    ensure_failover_supported(&app_type)?;
    let state = get_state()?;
    let runtime = create_runtime()?;
    let config = runtime.block_on(state.db.get_proxy_config_for_app(app_type.as_str()))?;
    let status = runtime.block_on(state.proxy_service.get_status());
    let takeovers = runtime
        .block_on(state.proxy_service.get_takeover_status())
        .map_err(AppError::Message)?;
    let queue = state.db.get_failover_queue(app_type.as_str())?;

    println!("{}", highlight("Failover"));
    println!("App: {}", app_type.as_str());
    println!(
        "Automatic failover: {}",
        if config.auto_failover_enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!(
        "Proxy running: {}",
        if status.running { "yes" } else { "no" }
    );
    println!(
        "Takeover active: {}",
        if status.running && takeover_enabled_for(&takeovers, &app_type) {
            "yes"
        } else {
            "no"
        }
    );
    println!();
    print_queue(&queue);
    Ok(())
}

fn set_auto_failover(app_type: AppType, enabled: bool) -> Result<(), AppError> {
    ensure_failover_supported(&app_type)?;
    let state = get_state()?;
    let runtime = create_runtime()?;
    if enabled {
        let gate = runtime.block_on(inspect_auto_failover_gate(&state, &app_type))?;
        ensure_auto_failover_queue_ready(&gate)?;
        if gate.needs_proxy_bootstrap() {
            if !confirm_enable_proxy_for_failover(app_type.as_str())? {
                println!("{}", info("Cancelled."));
                return Ok(());
            }
            runtime
                .block_on(
                    state
                        .proxy_service
                        .enable_proxy_and_auto_failover_for_app(app_type.as_str()),
                )
                .map_err(AppError::Message)?;
        } else {
            runtime
                .block_on(
                    state
                        .proxy_service
                        .enable_auto_failover_for_app(app_type.as_str()),
                )
                .map_err(AppError::Message)?;
        }
    } else {
        runtime
            .block_on(
                state
                    .proxy_service
                    .set_auto_failover_for_app(app_type.as_str(), false),
            )
            .map_err(AppError::Message)?;
    }

    println!(
        "{}",
        success(&format!(
            "Automatic failover {} for {}.",
            if enabled { "enabled" } else { "disabled" },
            app_type.as_str()
        ))
    );
    print_hot_update_note();
    Ok(())
}

fn confirm_enable_proxy_for_failover(app_type: &str) -> Result<bool, AppError> {
    inquire::Confirm::new(&format!(
        "Automatic failover requires proxy routing for {app_type}. Enable proxy routing for this app now?"
    ))
    .with_default(false)
    .prompt()
    .map_err(|error| match error {
        inquire::error::InquireError::OperationCanceled
        | inquire::error::InquireError::OperationInterrupted => {
            AppError::Message("__cc_switch_failover_cancelled__".to_string())
        }
        other => AppError::Message(format!("Prompt failed: {other}")),
    })
    .or_else(|error| match error {
        AppError::Message(message) if message == "__cc_switch_failover_cancelled__" => Ok(false),
        other => Err(other),
    })
}

fn list_queue(app_type: AppType) -> Result<(), AppError> {
    ensure_failover_supported(&app_type)?;
    let state = get_state()?;
    let queue = state.db.get_failover_queue(app_type.as_str())?;
    print_queue(&queue);
    Ok(())
}

fn list_available(app_type: AppType) -> Result<(), AppError> {
    ensure_failover_supported(&app_type)?;
    let state = get_state()?;
    let providers = state
        .db
        .get_available_providers_for_failover(app_type.as_str())?;
    if providers.is_empty() {
        println!("{}", info("No providers are available to add."));
        return Ok(());
    }

    let mut table = create_table();
    table.set_header(vec!["ID", "Name", "Sort"]);
    for provider in providers {
        table.add_row(vec![
            provider.id,
            provider.name,
            provider
                .sort_index
                .map(|index| index.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ]);
    }
    println!("{}", table);
    Ok(())
}

fn add_provider(app_type: AppType, id: &str) -> Result<(), AppError> {
    ensure_failover_supported(&app_type)?;
    let state = get_state()?;
    ensure_provider_exists(&state, &app_type, id)?;

    if state.db.is_in_failover_queue(app_type.as_str(), id)? {
        println!("{}", info("Provider is already in the failover queue."));
        return Ok(());
    }

    state.db.add_to_failover_queue(app_type.as_str(), id)?;
    println!("{}", success("Provider added to the failover queue."));
    print_hot_update_note_if_running(&state)?;
    Ok(())
}

fn remove_provider(app_type: AppType, id: &str) -> Result<(), AppError> {
    ensure_failover_supported(&app_type)?;
    let state = get_state()?;
    ensure_provider_exists(&state, &app_type, id)?;

    if !state.db.is_in_failover_queue(app_type.as_str(), id)? {
        println!("{}", info("Provider is not in the failover queue."));
        return Ok(());
    }

    if provider_is_last_active_failover_queue_entry(&state, &app_type, id)? {
        return Err(active_proxy_failover_queue_guard_error());
    }

    state.db.remove_from_failover_queue(app_type.as_str(), id)?;
    println!("{}", success("Provider removed from the failover queue."));
    print_hot_update_note_if_running(&state)?;
    Ok(())
}

fn clear_queue(app_type: AppType, yes: bool) -> Result<(), AppError> {
    ensure_failover_supported(&app_type)?;
    let state = get_state()?;
    let queue = state.db.get_failover_queue(app_type.as_str())?;

    if queue.is_empty() {
        println!("{}", info("Failover queue is already empty."));
        return Ok(());
    }
    if !yes {
        return Err(AppError::InvalidInput(
            "clearing the failover queue requires --yes".to_string(),
        ));
    }
    if queue_has_active_failover_guard(&state, &app_type, &queue)? {
        return Err(active_proxy_failover_queue_guard_error());
    }

    let runtime = create_runtime()?;
    state.db.clear_failover_queue(app_type.as_str())?;
    runtime.block_on(state.db.clear_provider_health_for_app(app_type.as_str()))?;
    println!("{}", success("Failover queue cleared."));
    print_hot_update_note_if_running(&state)?;
    Ok(())
}

fn move_provider(
    app_type: AppType,
    id: &str,
    direction: FailoverMoveDirection,
) -> Result<(), AppError> {
    ensure_failover_supported(&app_type)?;
    let state = get_state()?;
    ensure_provider_exists(&state, &app_type, id)?;
    let outcome = move_provider_in_state(&state, app_type, id, direction)?;
    match outcome {
        MoveOutcome::Updated => {
            println!("{}", success("Failover queue order updated."));
            print_hot_update_note_if_running(&state)?;
        }
        MoveOutcome::NotQueued => {
            println!(
                "{}",
                info("Add this provider to the failover queue before moving it.")
            );
        }
        MoveOutcome::AtEdge => {
            println!(
                "{}",
                info("Provider is already at the edge of the failover queue.")
            );
        }
    }
    Ok(())
}

fn move_provider_in_state(
    state: &AppState,
    app_type: AppType,
    id: &str,
    direction: FailoverMoveDirection,
) -> Result<MoveOutcome, AppError> {
    let mut queue = state.db.get_failover_queue(app_type.as_str())?;
    let Some(index) = queue.iter().position(|item| item.provider_id == id) else {
        return Ok(MoveOutcome::NotQueued);
    };

    let target = match direction {
        FailoverMoveDirection::Up if index > 0 => index - 1,
        FailoverMoveDirection::Down if index + 1 < queue.len() => index + 1,
        _ => return Ok(MoveOutcome::AtEdge),
    };

    queue.swap(index, target);
    let updates = queue
        .iter()
        .enumerate()
        .map(|(sort_index, item)| ProviderSortUpdate {
            id: item.provider_id.clone(),
            sort_index,
        })
        .collect::<Vec<_>>();
    ProviderService::update_sort_order(state, app_type, updates)?;
    Ok(MoveOutcome::Updated)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoveOutcome {
    Updated,
    NotQueued,
    AtEdge,
}

fn ensure_provider_exists(state: &AppState, app_type: &AppType, id: &str) -> Result<(), AppError> {
    state
        .db
        .get_provider_by_id(id, app_type.as_str())?
        .map(|_| ())
        .ok_or_else(|| AppError::InvalidInput(format!("Provider not found: {id}")))
}

fn provider_is_last_active_failover_queue_entry(
    state: &AppState,
    app_type: &AppType,
    provider_id: &str,
) -> Result<bool, AppError> {
    let queue = state.db.get_failover_queue(app_type.as_str())?;
    Ok(queue.len() == 1
        && queue
            .first()
            .is_some_and(|item| item.provider_id == provider_id)
        && active_failover_routes_app(state, app_type)?)
}

fn queue_has_active_failover_guard(
    state: &AppState,
    app_type: &AppType,
    queue: &[FailoverQueueItem],
) -> Result<bool, AppError> {
    Ok(!queue.is_empty() && active_failover_routes_app(state, app_type)?)
}

fn active_failover_routes_app(state: &AppState, app_type: &AppType) -> Result<bool, AppError> {
    let runtime = create_runtime()?;
    let config = runtime.block_on(state.db.get_proxy_config_for_app(app_type.as_str()))?;
    Ok(config.auto_failover_enabled)
}

fn active_proxy_failover_queue_guard_error() -> AppError {
    AppError::InvalidInput(
        "At least one provider must remain in the failover queue while proxy failover is active."
            .to_string(),
    )
}

fn takeover_enabled_for(takeovers: &ProxyTakeoverStatus, app_type: &AppType) -> bool {
    match app_type {
        AppType::Claude => takeovers.claude,
        AppType::Codex => takeovers.codex,
        AppType::Gemini => takeovers.gemini,
        AppType::OpenCode | AppType::Hermes | AppType::OpenClaw | AppType::Pi => false,
    }
}

fn print_queue(queue: &[FailoverQueueItem]) {
    if queue.is_empty() {
        println!("{}", info("Failover queue is empty."));
        return;
    }

    let mut table = create_table();
    table.set_header(vec!["#", "Provider ID", "Name", "Sort"]);
    for (index, item) in queue.iter().enumerate() {
        table.add_row(vec![
            (index + 1).to_string(),
            item.provider_id.clone(),
            item.provider_name.clone(),
            item.sort_index
                .map(|sort_index| sort_index.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ]);
    }
    println!("{}", table);
}

fn print_hot_update_note() {
    println!(
        "{}",
        info("Running proxy sessions will use this on subsequent requests.")
    );
}

fn print_hot_update_note_if_running(state: &AppState) -> Result<(), AppError> {
    let runtime = create_runtime()?;
    let status = runtime.block_on(state.proxy_service.get_status());
    if status.running {
        print_hot_update_note();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use crate::{Database, MultiAppConfig, ProxyService};

    use super::*;

    fn test_state() -> AppState {
        let db = Arc::new(Database::memory().expect("create memory database"));
        AppState {
            db: db.clone(),
            config: RwLock::new(MultiAppConfig::default()),
            proxy_service: ProxyService::new(db),
        }
    }

    fn provider(id: &str, name: &str, sort_index: usize) -> crate::provider::Provider {
        let mut provider = crate::provider::Provider::with_id(
            id.to_string(),
            name.to_string(),
            serde_json::json!({"api_key": "test"}),
            Some("https://example.com".to_string()),
        );
        provider.sort_index = Some(sort_index);
        provider
    }

    fn save_provider(state: &AppState, provider: crate::provider::Provider) {
        state
            .db
            .save_provider("claude", &provider)
            .expect("save provider");
        let mut config = state.config.write().expect("lock config");
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.providers.insert(provider.id.clone(), provider);
    }

    #[test]
    fn unsupported_apps_are_rejected() {
        assert!(ensure_failover_supported(&AppType::OpenCode).is_err());
        assert!(ensure_failover_supported(&AppType::OpenClaw).is_err());
    }

    #[test]
    fn moving_non_queued_provider_is_noop() {
        let state = test_state();
        save_provider(&state, provider("p1", "Provider 1", 0));

        let outcome =
            move_provider_in_state(&state, AppType::Claude, "p1", FailoverMoveDirection::Down)
                .expect("move provider");

        assert_eq!(outcome, MoveOutcome::NotQueued);
    }

    #[test]
    fn moving_provider_at_queue_edge_is_noop() {
        let state = test_state();
        state
            .db
            .save_provider("claude", &provider("p1", "Provider 1", 0))
            .expect("save provider");
        state
            .db
            .add_to_failover_queue("claude", "p1")
            .expect("queue provider");

        let outcome =
            move_provider_in_state(&state, AppType::Claude, "p1", FailoverMoveDirection::Up)
                .expect("move provider");

        assert_eq!(outcome, MoveOutcome::AtEdge);
    }

    #[test]
    fn moving_provider_updates_queue_order() {
        let state = test_state();
        save_provider(&state, provider("p1", "Provider 1", 0));
        save_provider(&state, provider("p2", "Provider 2", 1));
        state
            .db
            .add_to_failover_queue("claude", "p1")
            .expect("queue p1");
        state
            .db
            .add_to_failover_queue("claude", "p2")
            .expect("queue p2");

        let outcome =
            move_provider_in_state(&state, AppType::Claude, "p1", FailoverMoveDirection::Down)
                .expect("move provider");
        let queue = state.db.get_failover_queue("claude").expect("load queue");

        assert_eq!(outcome, MoveOutcome::Updated);
        assert_eq!(queue[0].provider_id, "p2");
        assert_eq!(queue[1].provider_id, "p1");
    }
}
