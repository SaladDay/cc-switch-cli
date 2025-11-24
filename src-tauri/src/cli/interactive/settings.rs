use inquire::Select;

use crate::cli::i18n::{texts, Language, current_language, set_language};
use crate::cli::ui::{highlight, success};
use crate::error::AppError;

use super::utils::pause;

pub fn settings_menu() -> Result<(), AppError> {
    loop {
        println!("\n{}", highlight(texts::settings_title()));
        println!("{}", "─".repeat(60));

        let lang = current_language();
        println!(
            "{}: {}",
            texts::current_language_label(),
            highlight(lang.display_name())
        );
        println!();

        let choices = vec![texts::change_language(), texts::back_to_main()];

        let choice = Select::new(texts::choose_action(), choices)
            .prompt()
            .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

        if choice == texts::change_language() {
            change_language_interactive()?;
        } else {
            break;
        }
    }

    Ok(())
}

fn change_language_interactive() -> Result<(), AppError> {
    let languages = vec![Language::English, Language::Chinese];

    let selected = Select::new(texts::select_language(), languages)
        .prompt()
        .map_err(|_| AppError::Message("Selection cancelled".to_string()))?;

    set_language(selected)?;

    println!("\n{}", success(texts::language_changed()));
    pause();

    Ok(())
}
