use crate::cli::i18n::{current_language, set_language, texts, Language};
use crate::cli::ui::{highlight, success};
use crate::error::AppError;

use super::utils::{clear_screen, pause, prompt_confirm, prompt_select};

pub fn settings_menu() -> Result<(), AppError> {
    loop {
        clear_screen();
        println!("\n{}", highlight(texts::settings_title()));
        println!("{}", texts::tui_rule(60));

        let lang = current_language();
        println!(
            "{}: {}",
            texts::current_language_label(),
            highlight(lang.display_name())
        );

        let skip_claude_onboarding = crate::settings::get_skip_claude_onboarding();
        println!(
            "{}: {}",
            texts::skip_claude_onboarding_label(),
            highlight(if skip_claude_onboarding {
                texts::enabled()
            } else {
                texts::disabled()
            })
        );
        println!();

        let choices = vec![
            texts::change_language(),
            texts::skip_claude_onboarding(),
            texts::back_to_main(),
        ];

        let Some(choice) = prompt_select(texts::choose_action(), choices)? else {
            break;
        };

        if choice == texts::change_language() {
            change_language_interactive()?;
        } else if choice == texts::skip_claude_onboarding() {
            toggle_skip_claude_onboarding_interactive()?;
        } else {
            break;
        }
    }

    Ok(())
}

fn change_language_interactive() -> Result<(), AppError> {
    clear_screen();
    let languages = vec![Language::English, Language::Chinese];

    let Some(selected) = prompt_select(texts::select_language(), languages)? else {
        return Ok(());
    };

    set_language(selected)?;

    println!("\n{}", success(texts::language_changed()));
    pause();

    Ok(())
}

fn toggle_skip_claude_onboarding_interactive() -> Result<(), AppError> {
    clear_screen();

    let current = crate::settings::get_skip_claude_onboarding();
    let next = !current;

    let path = crate::config::get_claude_mcp_path();
    let confirm_prompt =
        texts::skip_claude_onboarding_confirm(next, path.to_string_lossy().as_ref());
    let Some(confirm) = prompt_confirm(&confirm_prompt, true)? else {
        return Ok(());
    };
    if !confirm {
        return Ok(());
    }

    crate::settings::set_skip_claude_onboarding(next)?;

    println!(
        "\n{}",
        success(&texts::skip_claude_onboarding_changed(next))
    );
    pause();

    Ok(())
}
