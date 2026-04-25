use crate::app_config::AppType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Main,
    Providers,
    ProviderDetail { id: String },
    Mcp,
    Prompts,
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
    SettingsUpstreamProxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavItem {
    Main,
    Providers,
    Mcp,
    Prompts,
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
    pub const ALL: [NavItem; 8] = [
        NavItem::Main,
        NavItem::Providers,
        NavItem::Mcp,
        NavItem::Skills,
        NavItem::Prompts,
        NavItem::Config,
        NavItem::Settings,
        NavItem::Exit,
    ];

    pub const OPENCLAW_ALL: [NavItem; 8] = [
        NavItem::Main,
        NavItem::Providers,
        NavItem::OpenClawWorkspace,
        NavItem::OpenClawEnv,
        NavItem::OpenClawTools,
        NavItem::OpenClawAgents,
        NavItem::Settings,
        NavItem::Exit,
    ];

    pub fn all_for_app(app_type: &AppType) -> &'static [NavItem] {
        if matches!(app_type, AppType::OpenClaw) {
            &Self::OPENCLAW_ALL
        } else {
            &Self::ALL
        }
    }

    pub fn to_route(self) -> Option<Route> {
        match self {
            NavItem::Main => Some(Route::Main),
            NavItem::Providers => Some(Route::Providers),
            NavItem::Mcp => Some(Route::Mcp),
            NavItem::Prompts => Some(Route::Prompts),
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
}
