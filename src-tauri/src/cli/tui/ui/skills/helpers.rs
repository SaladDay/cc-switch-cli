use super::*;

pub(super) fn skills_installed_filtered<'a>(
    app: &App,
    data: &'a UiData,
) -> Vec<&'a crate::services::skill::InstalledSkill> {
    let query = app.filter.query_lower();
    data.skills
        .installed
        .iter()
        .filter(|skill| match &query {
            None => true,
            Some(q) => {
                skill.name.to_lowercase().contains(q)
                    || skill.directory.to_lowercase().contains(q)
                    || skill.id.to_lowercase().contains(q)
            }
        })
        .collect()
}

pub(super) fn skill_display_name<'a>(name: &'a str, directory: &'a str) -> &'a str {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        directory
    } else {
        trimmed
    }
}

pub(super) fn enabled_skill_apps_text(apps: &crate::app_config::SkillApps) -> String {
    let mut enabled = Vec::new();
    if apps.claude {
        enabled.push("Claude");
    }
    if apps.codex {
        enabled.push("Codex");
    }
    if apps.gemini {
        enabled.push("Gemini");
    }
    if apps.opencode {
        enabled.push("OpenCode");
    }
    if apps.hermes {
        enabled.push("Hermes");
    }

    if enabled.is_empty() {
        texts::none().to_string()
    } else {
        enabled.join(", ")
    }
}
