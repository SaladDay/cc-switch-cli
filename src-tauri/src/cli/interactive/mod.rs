mod config;
mod mcp;
mod prompts;
mod provider;
mod settings;
mod utils;

use inquire::Select;

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::cli::ui::{error, highlight, info, success};
use crate::error::AppError;
use crate::services::{McpService, PromptService, ProviderService};

use utils::pause;

pub fn run(app: Option<AppType>) -> Result<(), AppError> {
    let mut app_type = app.unwrap_or(AppType::Claude);

    print_welcome(&app_type);

    loop {
        match show_main_menu(&app_type)? {
            MainMenuChoice::ManageProviders => {
                if let Err(e) = provider::manage_providers_menu(&app_type) {
                    println!("\n{}", error(&format!("{}: {}", texts::error_prefix(), e)));
                    pause();
                }
            }
            MainMenuChoice::ManageMCP => {
                if let Err(e) = mcp::manage_mcp_menu(&app_type) {
                    println!("\n{}", error(&format!("{}: {}", texts::error_prefix(), e)));
                    pause();
                }
            }
            MainMenuChoice::ManagePrompts => {
                if let Err(e) = prompts::manage_prompts_menu(&app_type) {
                    println!("\n{}", error(&format!("{}: {}", texts::error_prefix(), e)));
                    pause();
                }
            }
            MainMenuChoice::ManageConfig => {
                if let Err(e) = config::manage_config_menu() {
                    println!("\n{}", error(&format!("{}: {}", texts::error_prefix(), e)));
                    pause();
                }
            }
            MainMenuChoice::ViewCurrentConfig => {
                if let Err(e) = view_current_config(&app_type) {
                    println!("\n{}", error(&format!("{}: {}", texts::error_prefix(), e)));
                    pause();
                }
            }
            MainMenuChoice::SwitchApp => {
                if let Ok(new_app) = select_app() {
                    app_type = new_app;
                    print_welcome(&app_type);
                }
            }
            MainMenuChoice::Settings => {
                if let Err(e) = settings::settings_menu() {
                    println!("\n{}", error(&format!("{}: {}", texts::error_prefix(), e)));
                    pause();
                }
            }
            MainMenuChoice::Exit => {
                println!("\n{}", success(texts::goodbye()));
                break;
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
enum MainMenuChoice {
    ManageProviders,
    ManageMCP,
    ManagePrompts,
    ManageConfig,
    ViewCurrentConfig,
    SwitchApp,
    Settings,
    Exit,
}

impl std::fmt::Display for MainMenuChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ManageProviders => write!(f, "{}", texts::menu_manage_providers()),
            Self::ManageMCP => write!(f, "{}", texts::menu_manage_mcp()),
            Self::ManagePrompts => write!(f, "{}", texts::menu_manage_prompts()),
            Self::ManageConfig => write!(f, "{}", texts::menu_manage_config()),
            Self::ViewCurrentConfig => write!(f, "{}", texts::menu_view_config()),
            Self::SwitchApp => write!(f, "{}", texts::menu_switch_app()),
            Self::Settings => write!(f, "{}", texts::menu_settings()),
            Self::Exit => write!(f, "{}", texts::menu_exit()),
        }
    }
}

fn print_welcome(app_type: &AppType) {
    println!("\n{}", "═".repeat(60));
    println!("{}", highlight(texts::welcome_title()));
    println!("{}", "═".repeat(60));
    println!(
        "{} {}: {}",
        info("📱"),
        texts::application(),
        highlight(app_type.as_str())
    );
    println!("{}", "─".repeat(60));
    println!();
}

fn show_main_menu(app_type: &AppType) -> Result<MainMenuChoice, AppError> {
    let choices = vec![
        MainMenuChoice::ManageProviders,
        MainMenuChoice::ManageMCP,
        MainMenuChoice::ManagePrompts,
        MainMenuChoice::ManageConfig,
        MainMenuChoice::ViewCurrentConfig,
        MainMenuChoice::SwitchApp,
        MainMenuChoice::Settings,
        MainMenuChoice::Exit,
    ];

    let choice = Select::new(&texts::main_menu_prompt(app_type.as_str()), choices)
        .prompt()
        .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

    Ok(choice)
}

fn select_app() -> Result<AppType, AppError> {
    let apps = vec![AppType::Claude, AppType::Codex, AppType::Gemini];

    let app = Select::new(texts::select_application(), apps)
        .prompt()
        .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

    println!("\n{}", success(&texts::switched_to_app(app.as_str())));
    pause();

    Ok(app)
}

fn view_current_config(app_type: &AppType) -> Result<(), AppError> {
    use utils::get_state;

    println!("\n{}", highlight(texts::current_configuration()));
    println!("{}", "─".repeat(60));

    let state = get_state()?;

    // Provider info
    let current_provider = ProviderService::current(&state, app_type.clone())?;
    let providers = ProviderService::list(&state, app_type.clone())?;
    if let Some(provider) = providers.get(&current_provider) {
        println!("{}", highlight(texts::provider_label()));
        println!(
            "  {}: {}",
            texts::header_name().trim_end_matches(':'),
            provider.name
        );
        let api_url = provider::extract_api_url(&provider.settings_config, &app_type)
            .unwrap_or_else(|| "N/A".to_string());
        println!("  API URL: {}", api_url);
        println!();
    }

    // MCP servers count
    let mcp_servers = McpService::get_all_servers(&state)?;
    let enabled_count = mcp_servers
        .values()
        .filter(|s| s.apps.is_enabled_for(app_type))
        .count();
    println!("{}", highlight(texts::mcp_servers_label()));
    println!("  {}:   {}", texts::total(), mcp_servers.len());
    println!("  {}: {}", texts::enabled(), enabled_count);
    println!();

    // Prompts
    let prompts = PromptService::get_prompts(&state, app_type.clone())?;
    let active_prompt = prompts.iter().find(|(_, p)| p.enabled);
    println!("{}", highlight(texts::prompts_label()));
    println!("  {}:  {}", texts::total(), prompts.len());
    if let Some((_, p)) = active_prompt {
        println!("  {}: {}", texts::active(), p.name);
    } else {
        println!("  {}: {}", texts::active(), texts::none());
    }

    println!();
    pause();

    Ok(())
}
