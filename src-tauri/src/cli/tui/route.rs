use crate::app_config::AppType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Main,
    Providers,
    Sessions,
    ProviderDetail { id: String },
    Mcp,
    Prompts,
    HermesMemory,
    Config,
    ConfigOpenClawWorkspace,
    ConfigOpenClawDailyMemory,
    ConfigOpenClawEnv,
    ConfigOpenClawTools,
    ConfigOpenClawAgents,
    ConfigWebDav,
    Skills,
    SkillsDiscover,
    SkillsRepos,
    SkillDetail { directory: String },
    Settings,
    SettingsProxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavItem {
    Main,
    Providers,
    Sessions,
    Mcp,
    Prompts,
    HermesMemory,
    Config,
    Skills,
    OpenClawWorkspace,
    OpenClawEnv,
    OpenClawTools,
    OpenClawAgents,
    Settings,
    Exit,
}

impl NavItem {
    pub const ALL: [NavItem; 9] = [
        NavItem::Main,
        NavItem::Providers,
        NavItem::Mcp,
        NavItem::Skills,
        NavItem::Sessions,
        NavItem::Prompts,
        NavItem::Config,
        NavItem::Settings,
        NavItem::Exit,
    ];

    pub const OPENCLAW_ALL: [NavItem; 10] = [
        NavItem::Main,
        NavItem::Providers,
        NavItem::Sessions,
        NavItem::OpenClawWorkspace,
        NavItem::OpenClawEnv,
        NavItem::OpenClawTools,
        NavItem::OpenClawAgents,
        NavItem::Config,
        NavItem::Settings,
        NavItem::Exit,
    ];

    pub const HERMES_ALL: [NavItem; 9] = [
        NavItem::Main,
        NavItem::Providers,
        NavItem::Mcp,
        NavItem::Skills,
        NavItem::Sessions,
        NavItem::HermesMemory,
        NavItem::Config,
        NavItem::Settings,
        NavItem::Exit,
    ];

    pub fn all_for_app(app_type: &AppType) -> &'static [NavItem] {
        match app_type {
            AppType::OpenClaw => &Self::OPENCLAW_ALL,
            AppType::Hermes => &Self::HERMES_ALL,
            _ => &Self::ALL,
        }
    }

    pub fn to_route(self) -> Option<Route> {
        match self {
            NavItem::Main => Some(Route::Main),
            NavItem::Providers => Some(Route::Providers),
            NavItem::Sessions => Some(Route::Sessions),
            NavItem::Mcp => Some(Route::Mcp),
            NavItem::Prompts => Some(Route::Prompts),
            NavItem::HermesMemory => Some(Route::HermesMemory),
            NavItem::Config => Some(Route::Config),
            NavItem::Skills => Some(Route::Skills),
            NavItem::OpenClawWorkspace => Some(Route::ConfigOpenClawWorkspace),
            NavItem::OpenClawEnv => Some(Route::ConfigOpenClawEnv),
            NavItem::OpenClawTools => Some(Route::ConfigOpenClawTools),
            NavItem::OpenClawAgents => Some(Route::ConfigOpenClawAgents),
            NavItem::Settings => Some(Route::Settings),
            NavItem::Exit => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NavItem;

    #[test]
    fn skills_appears_before_prompts_in_nav() {
        let skills = NavItem::ALL
            .iter()
            .position(|item| matches!(item, NavItem::Skills))
            .expect("skills nav item should exist");
        let prompts = NavItem::ALL
            .iter()
            .position(|item| matches!(item, NavItem::Prompts))
            .expect("prompts nav item should exist");

        assert!(
            skills < prompts,
            "skills should appear above prompts in the left nav"
        );
    }

    #[test]
    fn sessions_appears_after_mcp_and_skills_before_prompts_in_nav() {
        let sessions = NavItem::ALL
            .iter()
            .position(|item| matches!(item, NavItem::Sessions))
            .expect("sessions nav item should exist");
        let mcp = NavItem::ALL
            .iter()
            .position(|item| matches!(item, NavItem::Mcp))
            .expect("mcp nav item should exist");
        let skills = NavItem::ALL
            .iter()
            .position(|item| matches!(item, NavItem::Skills))
            .expect("skills nav item should exist");
        let prompts = NavItem::ALL
            .iter()
            .position(|item| matches!(item, NavItem::Prompts))
            .expect("prompts nav item should exist");

        assert!(mcp < sessions && skills < sessions && sessions < prompts);
    }

    #[test]
    fn hermes_nav_uses_memory_instead_of_prompts() {
        assert!(NavItem::HERMES_ALL
            .iter()
            .any(|item| matches!(item, NavItem::HermesMemory)));
        assert!(NavItem::HERMES_ALL
            .iter()
            .any(|item| matches!(item, NavItem::Config)));
        assert!(!NavItem::HERMES_ALL
            .iter()
            .any(|item| matches!(item, NavItem::Prompts)));
    }

    #[test]
    fn openclaw_nav_keeps_generic_config_entry() {
        assert!(NavItem::OPENCLAW_ALL
            .iter()
            .any(|item| matches!(item, NavItem::Config)));
    }
}
