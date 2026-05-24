use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::settings::{self, VisibleApps, VisibleAppsMode};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleAppsDetection {
    pub installed: HashMap<AppType, bool>,
}

impl Default for VisibleAppsDetection {
    fn default() -> Self {
        Self {
            installed: CONTROLLED_APPS
                .iter()
                .cloned()
                .map(|app| (app, false))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisibleAppsNotice {
    AutoUpdated { apps: Vec<AppType> },
    ManualHiddenInstalled { apps: Vec<AppType> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleAppsStartupOutcome {
    pub visible_apps: VisibleApps,
    pub notices: Vec<VisibleAppsNotice>,
    pub should_prompt: bool,
}

const CONTROLLED_APPS: [AppType; 4] = [
    AppType::Gemini,
    AppType::OpenCode,
    AppType::Hermes,
    AppType::OpenClaw,
];

pub fn detect_visible_app_installation() -> VisibleAppsDetection {
    let installed = CONTROLLED_APPS
        .iter()
        .cloned()
        .map(|app| {
            let installed = crate::services::local_env_check::check_tool_installed(&app);
            (app, installed)
        })
        .collect();
    VisibleAppsDetection { installed }
}

pub fn apply_startup_policy(
    detection: &VisibleAppsDetection,
) -> Result<VisibleAppsStartupOutcome, AppError> {
    let mut app_settings = settings::get_settings();
    let mut visible_apps = app_settings.visible_apps.clone();
    let mut visible_settings = app_settings.visible_apps_settings.clone();
    let mut notices = Vec::new();

    let detection_would_change_defaults =
        detection_would_change(&settings::default_visible_apps(), detection);
    let should_prompt = visible_settings.mode == VisibleAppsMode::Auto
        && !visible_settings.auto_prompt_decided
        && detection_would_change_defaults;

    if should_prompt {
        visible_settings.last_detected_installed = detection_string_map(detection);
        app_settings.visible_apps_settings = visible_settings;
        settings::update_settings(app_settings)?;
        return Ok(VisibleAppsStartupOutcome {
            visible_apps,
            notices,
            should_prompt: true,
        });
    }

    if visible_settings.mode == VisibleAppsMode::Auto && !visible_settings.auto_prompt_decided {
        visible_settings.auto_prompt_decided = true;
    }

    if visible_settings.mode == VisibleAppsMode::Auto {
        let changed = apply_detection_to_visible_apps(&mut visible_apps, detection);
        ensure_visible_apps_has_fallback(&mut visible_apps);
        if !changed.is_empty() {
            notices.push(VisibleAppsNotice::AutoUpdated { apps: changed });
        }
    } else {
        let mut hidden_installed_apps = Vec::new();
        for app in CONTROLLED_APPS {
            let installed = detection.is_installed(&app);
            let key = app.as_str().to_string();
            let previous = visible_settings
                .last_detected_installed
                .get(&key)
                .copied()
                .unwrap_or(false);

            if !installed && previous {
                visible_settings
                    .manual_hidden_installed_notices
                    .insert(key.clone(), false);
            }

            if installed && !visible_apps.is_enabled_for(&app) {
                let already_notified = visible_settings
                    .manual_hidden_installed_notices
                    .get(&key)
                    .copied()
                    .unwrap_or(false);
                if !already_notified {
                    hidden_installed_apps.push(app.clone());
                    visible_settings
                        .manual_hidden_installed_notices
                        .insert(key, true);
                }
            }
        }
        if !hidden_installed_apps.is_empty() {
            notices.push(VisibleAppsNotice::ManualHiddenInstalled {
                apps: hidden_installed_apps,
            });
        }
    }

    visible_apps.normalize();
    visible_settings.last_detected_installed = detection_string_map(detection);
    app_settings.visible_apps = visible_apps.clone();
    app_settings.visible_apps_settings = visible_settings;
    settings::update_settings(app_settings)?;

    Ok(VisibleAppsStartupOutcome {
        visible_apps,
        notices,
        should_prompt: false,
    })
}

pub fn accept_auto_detection(detection: &VisibleAppsDetection) -> Result<Vec<AppType>, AppError> {
    let mut app_settings = settings::get_settings();
    let mut changed_visible_apps = app_settings.visible_apps.clone();
    let changed = apply_detection_to_visible_apps(&mut changed_visible_apps, detection);
    ensure_visible_apps_has_fallback(&mut changed_visible_apps);
    changed_visible_apps.normalize();

    app_settings.visible_apps = changed_visible_apps;
    app_settings.visible_apps_settings.mode = VisibleAppsMode::Auto;
    app_settings.visible_apps_settings.auto_prompt_decided = true;
    app_settings.visible_apps_settings.last_detected_installed = detection_string_map(detection);
    settings::update_settings(app_settings)?;
    Ok(changed)
}

pub fn keep_manual_visibility(detection: &VisibleAppsDetection) -> Result<(), AppError> {
    let mut app_settings = settings::get_settings();
    app_settings.visible_apps_settings.mode = VisibleAppsMode::Manual;
    app_settings.visible_apps_settings.auto_prompt_decided = true;
    app_settings.visible_apps_settings.last_detected_installed = detection_string_map(detection);
    settings::update_settings(app_settings)
}

pub fn notice_message(notice: &VisibleAppsNotice) -> String {
    match notice {
        VisibleAppsNotice::AutoUpdated { apps } => {
            texts::tui_toast_visible_apps_auto_updated(&app_names(apps))
        }
        VisibleAppsNotice::ManualHiddenInstalled { apps } => {
            texts::tui_toast_visible_apps_manual_hidden_installed(&app_names(apps))
        }
    }
}

fn list_separator() -> &'static str {
    if crate::cli::i18n::current_language() == crate::cli::i18n::Language::Chinese {
        "、"
    } else {
        ", "
    }
}

pub fn app_display_name(app: &AppType) -> &'static str {
    match app {
        AppType::Claude => "Claude",
        AppType::Codex => "Codex",
        AppType::Gemini => "Gemini",
        AppType::OpenCode => "OpenCode",
        AppType::Hermes => "Hermes",
        AppType::OpenClaw => "OpenClaw",
    }
}

fn apply_detection_to_visible_apps(
    visible_apps: &mut VisibleApps,
    detection: &VisibleAppsDetection,
) -> Vec<AppType> {
    let mut changed = Vec::new();
    for app in CONTROLLED_APPS {
        let installed = detection.is_installed(&app);
        if visible_apps.is_enabled_for(&app) != installed {
            visible_apps.set_enabled_for(&app, installed);
            changed.push(app);
        }
    }
    changed
}

fn ensure_visible_apps_has_fallback(visible_apps: &mut VisibleApps) {
    if visible_apps.ordered_enabled().is_empty() {
        visible_apps.claude = true;
    }
}

fn app_names(apps: &[AppType]) -> String {
    apps.iter()
        .map(app_display_name)
        .collect::<Vec<_>>()
        .join(list_separator())
}

fn detection_would_change(apps: &VisibleApps, detection: &VisibleAppsDetection) -> bool {
    CONTROLLED_APPS
        .iter()
        .any(|app| apps.is_enabled_for(app) != detection.is_installed(app))
}

fn detection_string_map(detection: &VisibleAppsDetection) -> HashMap<String, bool> {
    CONTROLLED_APPS
        .iter()
        .cloned()
        .map(|app| (app.as_str().to_string(), detection.is_installed(&app)))
        .collect()
}

impl VisibleAppsDetection {
    #[cfg(test)]
    fn new(installed: HashMap<AppType, bool>) -> Self {
        Self { installed }
    }

    pub fn is_installed(&self, app: &AppType) -> bool {
        self.installed.get(app).copied().unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{VisibleAppsMode, VisibleAppsSettings};
    use crate::test_support::{
        lock_test_home_and_settings, set_test_home_override, TestHomeSettingsLock,
    };
    use serial_test::serial;
    use std::ffi::OsString;
    use std::path::Path;
    use tempfile::TempDir;

    struct EnvGuard {
        _lock: TestHomeSettingsLock,
        old_home: Option<OsString>,
        old_userprofile: Option<OsString>,
        old_config_dir: Option<OsString>,
    }

    impl EnvGuard {
        fn set_home(home: &Path) -> Self {
            let lock = lock_test_home_and_settings();
            let old_home = std::env::var_os("HOME");
            let old_userprofile = std::env::var_os("USERPROFILE");
            let old_config_dir = std::env::var_os("CC_SWITCH_CONFIG_DIR");
            std::env::set_var("HOME", home);
            std::env::set_var("USERPROFILE", home);
            std::env::set_var("CC_SWITCH_CONFIG_DIR", home.join(".cc-switch"));
            set_test_home_override(Some(home));
            crate::settings::reload_test_settings();
            Self {
                _lock: lock,
                old_home,
                old_userprofile,
                old_config_dir,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.old_userprofile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
            match &self.old_config_dir {
                Some(value) => std::env::set_var("CC_SWITCH_CONFIG_DIR", value),
                None => std::env::remove_var("CC_SWITCH_CONFIG_DIR"),
            }
            set_test_home_override(self.old_home.as_deref().map(Path::new));
            crate::settings::reload_test_settings();
        }
    }

    fn detection(installed: &[(AppType, bool)]) -> VisibleAppsDetection {
        VisibleAppsDetection::new(installed.iter().cloned().collect())
    }

    #[test]
    #[serial(home_settings)]
    fn auto_mode_updates_only_detection_controlled_apps() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let mut settings = crate::settings::get_settings();
        settings.visible_apps = VisibleApps {
            claude: false,
            codex: true,
            gemini: false,
            opencode: true,
            hermes: true,
            openclaw: false,
        };
        settings.visible_apps_settings = VisibleAppsSettings {
            mode: VisibleAppsMode::Auto,
            auto_prompt_decided: true,
            ..VisibleAppsSettings::default()
        };
        crate::settings::update_settings(settings).expect("save settings");

        let outcome = apply_startup_policy(&detection(&[
            (AppType::Gemini, true),
            (AppType::OpenCode, false),
            (AppType::Hermes, true),
            (AppType::OpenClaw, true),
        ]))
        .expect("apply policy");

        assert!(!outcome.visible_apps.claude);
        assert!(outcome.visible_apps.codex);
        assert!(outcome.visible_apps.gemini);
        assert!(!outcome.visible_apps.opencode);
        assert!(outcome.visible_apps.hermes);
        assert!(outcome.visible_apps.openclaw);
        assert!(matches!(
            outcome.notices.as_slice(),
            [VisibleAppsNotice::AutoUpdated { apps }]
                if apps == &vec![AppType::Gemini, AppType::OpenCode, AppType::OpenClaw]
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn manual_mode_hidden_installed_notice_appears_once_until_state_transition() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let mut settings = crate::settings::get_settings();
        settings.visible_apps = VisibleApps {
            claude: true,
            codex: true,
            gemini: false,
            opencode: false,
            hermes: false,
            openclaw: true,
        };
        settings.visible_apps_settings = VisibleAppsSettings {
            mode: VisibleAppsMode::Manual,
            auto_prompt_decided: true,
            ..VisibleAppsSettings::default()
        };
        crate::settings::update_settings(settings).expect("save settings");

        let installed = detection(&[(AppType::Gemini, true)]);
        let first = apply_startup_policy(&installed).expect("first policy");
        assert!(matches!(
            first.notices.as_slice(),
            [VisibleAppsNotice::ManualHiddenInstalled { apps }]
                if apps == &vec![AppType::Gemini]
        ));

        let second = apply_startup_policy(&installed).expect("second policy");
        assert!(second.notices.is_empty());

        let missing = detection(&[(AppType::Gemini, false)]);
        apply_startup_policy(&missing).expect("missing policy");
        let third = apply_startup_policy(&installed).expect("third policy");
        assert!(matches!(
            third.notices.as_slice(),
            [VisibleAppsNotice::ManualHiddenInstalled { apps }]
                if apps == &vec![AppType::Gemini]
        ));
    }

    #[test]
    #[serial(home_settings)]
    fn first_run_prompt_is_one_time_when_detection_would_change_defaults() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());

        let outcome = apply_startup_policy(&detection(&[
            (AppType::Gemini, true),
            (AppType::OpenCode, true),
            (AppType::Hermes, true),
            (AppType::OpenClaw, true),
        ]))
        .expect("apply policy");

        assert!(outcome.should_prompt);
        assert_eq!(
            crate::settings::get_visible_apps(),
            crate::settings::default_visible_apps()
        );
        assert!(
            !crate::settings::get_visible_apps_settings().auto_prompt_decided,
            "prompt should remain pending until user decides"
        );
    }

    #[test]
    #[serial(home_settings)]
    fn auto_mode_falls_back_to_claude_when_detection_hides_every_controlled_app() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = EnvGuard::set_home(temp_home.path());
        let mut settings = crate::settings::get_settings();
        settings.visible_apps = VisibleApps {
            claude: false,
            codex: false,
            gemini: true,
            opencode: true,
            hermes: true,
            openclaw: true,
        };
        settings.visible_apps_settings = VisibleAppsSettings {
            mode: VisibleAppsMode::Auto,
            auto_prompt_decided: true,
            ..VisibleAppsSettings::default()
        };
        crate::settings::update_settings(settings).expect("save settings");

        let outcome = apply_startup_policy(&VisibleAppsDetection::default()).expect("apply policy");
        assert_eq!(
            outcome.visible_apps.ordered_enabled(),
            vec![AppType::Claude]
        );
    }
}
