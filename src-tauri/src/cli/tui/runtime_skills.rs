use std::collections::HashSet;

use crate::app_config::{AppType, InstalledSkill};
use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::services::{skill::SkillRepo, SkillService};

use super::app::{App, Overlay, ToastKind};
use super::data::UiData;

pub(crate) fn scan_unmanaged_skills_with<F>(app: &mut App, scan: F) -> Result<(), AppError>
where
    F: FnOnce() -> Result<Vec<crate::services::skill::UnmanagedSkill>, AppError>,
{
    app.skills_unmanaged_results = scan()?;
    app.skills_unmanaged_selected.clear();
    app.skills_unmanaged_idx = 0;
    app.push_toast(
        texts::tui_toast_unmanaged_scanned(app.skills_unmanaged_results.len()),
        ToastKind::Info,
    );
    Ok(())
}

pub(crate) fn scan_unmanaged_skills(app: &mut App) -> Result<(), AppError> {
    scan_unmanaged_skills_with(app, SkillService::scan_unmanaged)
}

pub(crate) fn open_skills_import_picker_with<F>(app: &mut App, scan: F) -> Result<(), AppError>
where
    F: FnOnce() -> Result<Vec<crate::services::skill::UnmanagedSkill>, AppError>,
{
    let skills = scan()?;
    app.skills_unmanaged_results = skills.clone();
    app.skills_unmanaged_selected.clear();
    app.skills_unmanaged_idx = 0;

    if skills.is_empty() {
        app.overlay = Overlay::None;
        app.push_toast(texts::skills_no_unmanaged_found(), ToastKind::Info);
        return Ok(());
    }

    let selected = skills
        .iter()
        .map(|skill| skill.directory.clone())
        .collect::<HashSet<_>>();
    app.overlay = Overlay::SkillsImportPicker {
        skills,
        selected_idx: 0,
        selected,
    };
    Ok(())
}

pub(crate) fn open_skills_import_picker(app: &mut App) -> Result<(), AppError> {
    open_skills_import_picker_with(app, SkillService::scan_unmanaged)
}

pub(crate) fn finish_skills_import_with<FImport, FLoad>(
    app: &mut App,
    data: &mut UiData,
    import: FImport,
    load_data: FLoad,
) -> Result<(), AppError>
where
    FImport: FnOnce() -> Result<Vec<InstalledSkill>, AppError>,
    FLoad: FnOnce(&AppType) -> Result<UiData, AppError>,
{
    let imported = import()?;
    app.overlay = Overlay::None;
    *data = load_data(&app.app_type)?;
    app.push_toast(
        texts::tui_toast_unmanaged_imported(imported.len()),
        ToastKind::Info,
    );
    Ok(())
}

pub(crate) fn parse_repo_spec(raw: &str) -> Result<SkillRepo, AppError> {
    let raw = raw.trim().trim_end_matches('/');
    if raw.is_empty() {
        return Err(AppError::InvalidInput(
            texts::tui_error_repo_spec_empty().to_string(),
        ));
    }

    let without_prefix = raw
        .strip_prefix("https://github.com/")
        .or_else(|| raw.strip_prefix("http://github.com/"))
        .unwrap_or(raw);

    let without_git = without_prefix.trim_end_matches(".git");

    let (path, branch) = if let Some((left, right)) = without_git.rsplit_once('@') {
        (left, Some(right))
    } else {
        (without_git, None)
    };

    let Some((owner, name)) = path.split_once('/') else {
        return Err(AppError::InvalidInput(
            texts::tui_error_repo_spec_invalid().to_string(),
        ));
    };

    Ok(SkillRepo {
        owner: owner.to_string(),
        name: name.to_string(),
        branch: branch.unwrap_or("main").to_string(),
        token_env: None,
        enabled: true,
    })
}
