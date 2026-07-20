//! Single source of truth for per-page key bindings.
//!
//! Each page declares one binding table; the key handler resolves incoming
//! keys through `intent_for`, and the page key bar is generated from the
//! same table via `key_bar_items`. This keeps the shown hints and the
//! actual handlers from drifting apart — a hint can only exist if there is
//! a binding, and every visible binding is labeled from the same row.
//!
//! Dispatch intentionally ignores the `shown` predicate: guards (and their
//! feedback toasts) stay in the handler bodies, so hidden aliases keep
//! working and pressing a disabled key can still explain itself.

use crossterm::event::KeyCode;

/// One key binding on a page: the keys that trigger it, the intent handed
/// to the page's key handler, and how it appears in the page key bar.
pub(crate) struct Binding<I: Copy> {
    /// Key text shown in the hint bar, e.g. "Space".
    pub display: &'static str,
    /// All key codes that trigger this intent (aliases included).
    pub keys: &'static [KeyCode],
    pub intent: I,
    /// Hint-bar label; may depend on app state (e.g. switch vs add/remove).
    pub label: LabelFn,
    /// Whether the chip appears in the page key bar right now.
    pub shown: PredicateFn,
}

pub(crate) type LabelFn = fn(&super::app::App, &super::data::UiData) -> &'static str;
pub(crate) type PredicateFn = fn(&super::app::App, &super::data::UiData) -> bool;

/// Resolve a key press to the page intent it triggers, if any.
pub(crate) fn intent_for<I: Copy>(bindings: &[Binding<I>], key: KeyCode) -> Option<I> {
    bindings
        .iter()
        .find(|binding| binding.keys.contains(&key))
        .map(|binding| binding.intent)
}

/// Build the page key bar entries: every currently-shown binding, in table
/// (priority) order.
pub(crate) fn key_bar_items<I: Copy>(
    bindings: &[Binding<I>],
    app: &super::app::App,
    data: &super::data::UiData,
) -> Vec<(&'static str, &'static str)> {
    bindings
        .iter()
        .filter(|binding| (binding.shown)(app, data))
        .map(|binding| (binding.display, (binding.label)(app, data)))
        .collect()
}

/// Sentinel `shown` predicate for bindings that never appear in the key bar
/// (hidden aliases, e.g. a reverse-direction key already covered by another
/// chip). The help-sheet generator skips these by function identity — so use
/// this named function rather than an inline `|_, _| false` closure.
pub(crate) fn never(_: &super::app::App, _: &super::data::UiData) -> bool {
    false
}

/// Hide a secondary binding from the compact key bar while retaining it in
/// the full help sheet. This differs from `never`, which hides the binding
/// from both surfaces.
pub(crate) fn help_only(_: &super::app::App, _: &super::data::UiData) -> bool {
    false
}

/// (display, label) pairs for the help sheet: every binding except the
/// `never`-shown aliases, labeled for the given app/data. Unlike
/// `key_bar_items` this does not evaluate each binding's `shown` state, so
/// the catalog lists a page's full key vocabulary regardless of the current
/// selection.
pub(crate) fn help_items<I: Copy>(
    bindings: &[Binding<I>],
    app: &super::app::App,
    data: &super::data::UiData,
) -> Vec<(&'static str, &'static str)> {
    bindings
        .iter()
        .filter(|binding| !std::ptr::fn_addr_eq(binding.shown, never as PredicateFn))
        .map(|binding| (binding.display, (binding.label)(app, data)))
        .collect()
}

pub(crate) mod providers {
    use crossterm::event::KeyCode;

    use super::Binding;
    use crate::app_config::AppType;
    use crate::cli::i18n::texts;
    use crate::cli::tui::app::{
        supports_failover_controls, supports_temporary_provider_launch, visible_providers, App,
    };
    use crate::cli::tui::data::{self, ProviderRow, UiData};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Intent {
        /// Enter: import the live config on a truly-empty list, otherwise
        /// open the edit form for the selected row.
        Primary,
        Switch,
        Add,
        Copy,
        Edit,
        Delete,
        Test,
        RefreshQuota,
        LaunchTemp,
        Failover,
        SetDefault,
    }

    pub(crate) const BINDINGS: &[Binding<Intent>] = &[
        Binding {
            display: "Enter",
            keys: &[KeyCode::Enter],
            intent: Intent::Primary,
            label: primary_label,
            shown: primary_shown,
        },
        Binding {
            display: "Space",
            keys: &[KeyCode::Char(' '), KeyCode::Char('s')],
            intent: Intent::Switch,
            label: switch_label,
            shown: switch_shown,
        },
        Binding {
            display: "a",
            keys: &[KeyCode::Char('a')],
            intent: Intent::Add,
            label: add_label,
            shown: add_shown,
        },
        Binding {
            display: "c",
            keys: &[KeyCode::Char('c')],
            intent: Intent::Copy,
            label: |_, _| texts::tui_key_copy(),
            shown: any_visible,
        },
        Binding {
            display: "e",
            keys: &[KeyCode::Char('e')],
            intent: Intent::Edit,
            label: |_, _| texts::tui_key_edit(),
            shown: super::help_only,
        },
        Binding {
            display: "d",
            keys: &[KeyCode::Char('d')],
            intent: Intent::Delete,
            label: |_, _| texts::tui_key_delete(),
            shown: selected_editable,
        },
        Binding {
            display: "t",
            keys: &[KeyCode::Char('t')],
            intent: Intent::Test,
            label: |_, _| texts::tui_key_test(),
            shown: any_visible,
        },
        Binding {
            display: "r",
            keys: &[KeyCode::Char('r')],
            intent: Intent::RefreshQuota,
            label: |_, _| texts::tui_key_refresh(),
            shown: quota_shown,
        },
        Binding {
            display: "o",
            keys: &[KeyCode::Char('o')],
            intent: Intent::LaunchTemp,
            label: |_, _| texts::tui_key_launch_temp(),
            shown: launch_shown,
        },
        Binding {
            display: "f",
            keys: &[KeyCode::Char('f')],
            intent: Intent::Failover,
            label: |_, _| texts::tui_key_failover(),
            shown: failover_shown,
        },
        Binding {
            display: "x",
            keys: &[KeyCode::Char('x')],
            intent: Intent::SetDefault,
            label: set_default_label,
            shown: set_default_shown,
        },
    ];

    pub(crate) fn intent_for(key: KeyCode) -> Option<Intent> {
        super::intent_for(BINDINGS, key)
    }

    pub(crate) fn key_bar_items(app: &App, data: &UiData) -> Vec<(&'static str, &'static str)> {
        super::key_bar_items(BINDINGS, app, data)
    }

    fn selected_row<'a>(app: &App, data: &'a UiData) -> Option<&'a ProviderRow> {
        visible_providers(&app.app_type, &app.filter, data)
            .get(app.provider_idx)
            .copied()
    }

    fn any_visible(app: &App, data: &UiData) -> bool {
        selected_row(app, data).is_some()
    }

    fn switch_shown(app: &App, data: &UiData) -> bool {
        any_visible(app, data)
            && (!supports_failover_controls(&app.app_type) || !data.proxy.auto_failover_enabled)
    }

    fn selected_editable(app: &App, data: &UiData) -> bool {
        selected_row(app, data).is_some_and(|row| !data::provider_is_read_only(&app.app_type, row))
    }

    fn import_shown(_app: &App, data: &UiData) -> bool {
        data.providers.rows.is_empty() && !data.providers.loading
    }

    fn primary_shown(app: &App, data: &UiData) -> bool {
        import_shown(app, data) || selected_editable(app, data)
    }

    fn primary_label(_app: &App, data: &UiData) -> &'static str {
        if data.providers.rows.is_empty() {
            texts::tui_key_import_current_config()
        } else {
            texts::tui_key_edit()
        }
    }

    fn add_shown(_app: &App, data: &UiData) -> bool {
        // While a cold-switched app is still loading, the list isn't really
        // empty — offer nothing rather than a misleading "add first".
        !(data.providers.rows.is_empty() && data.providers.loading)
    }

    fn add_label(_app: &App, data: &UiData) -> &'static str {
        if data.providers.rows.is_empty() {
            texts::tui_key_add_provider()
        } else {
            texts::tui_key_add()
        }
    }

    fn switch_label(app: &App, _data: &UiData) -> &'static str {
        if app.app_type.is_additive_mode() {
            texts::tui_key_add_remove()
        } else {
            texts::tui_key_switch()
        }
    }

    fn quota_shown(app: &App, data: &UiData) -> bool {
        selected_row(app, data)
            .is_some_and(|row| data::quota_target_for_provider(&app.app_type, row).is_some())
    }

    fn launch_shown(app: &App, data: &UiData) -> bool {
        any_visible(app, data) && supports_temporary_provider_launch(&app.app_type)
    }

    fn failover_shown(app: &App, data: &UiData) -> bool {
        any_visible(app, data) && supports_failover_controls(&app.app_type)
    }

    fn set_default_shown(app: &App, data: &UiData) -> bool {
        matches!(app.app_type, AppType::OpenClaw | AppType::Hermes)
            && selected_row(app, data).is_some_and(|row| row.is_in_config)
    }

    fn set_default_label(app: &App, _data: &UiData) -> &'static str {
        if matches!(app.app_type, AppType::Hermes) {
            texts::tui_key_enable()
        } else {
            texts::tui_key_set_default()
        }
    }
}

pub(crate) mod mcp {
    use crossterm::event::KeyCode;

    use super::Binding;
    use crate::cli::i18n::texts;
    use crate::cli::tui::app::{visible_mcp, App};
    use crate::cli::tui::data::UiData;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Intent {
        Toggle,
        Apps,
        Add,
        Edit,
        Import,
        Delete,
    }

    pub(crate) const BINDINGS: &[Binding<Intent>] = &[
        Binding {
            display: "Space",
            keys: &[KeyCode::Char(' ')],
            intent: Intent::Toggle,
            label: |_, _| texts::tui_key_toggle(),
            shown: any_visible,
        },
        Binding {
            display: "m",
            keys: &[KeyCode::Char('m')],
            intent: Intent::Apps,
            label: |_, _| texts::tui_key_apps(),
            shown: any_visible,
        },
        Binding {
            display: "a",
            keys: &[KeyCode::Char('a')],
            intent: Intent::Add,
            label: |_, _| texts::tui_key_add(),
            shown: |_, _| true,
        },
        Binding {
            display: "e",
            keys: &[KeyCode::Char('e')],
            intent: Intent::Edit,
            label: |_, _| texts::tui_key_edit(),
            shown: any_visible,
        },
        Binding {
            display: "i",
            keys: &[KeyCode::Char('i')],
            intent: Intent::Import,
            label: |_, _| texts::tui_mcp_action_import_existing(),
            shown: |_, _| true,
        },
        Binding {
            display: "d",
            keys: &[KeyCode::Char('d')],
            intent: Intent::Delete,
            label: |_, _| texts::tui_key_delete(),
            shown: any_visible,
        },
    ];

    pub(crate) fn intent_for(key: KeyCode) -> Option<Intent> {
        super::intent_for(BINDINGS, key)
    }

    pub(crate) fn key_bar_items(app: &App, data: &UiData) -> Vec<(&'static str, &'static str)> {
        super::key_bar_items(BINDINGS, app, data)
    }

    pub(crate) fn help_items(app: &App, data: &UiData) -> Vec<(&'static str, &'static str)> {
        super::help_items(BINDINGS, app, data)
    }

    fn any_visible(app: &App, data: &UiData) -> bool {
        visible_mcp(&app.filter, data)
            .get(app.mcp_idx)
            .copied()
            .is_some()
    }
}

pub(crate) mod prompts {
    use crossterm::event::KeyCode;

    use super::Binding;
    use crate::cli::i18n::texts;
    use crate::cli::tui::app::{visible_prompts, App};
    use crate::cli::tui::data::UiData;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Intent {
        Toggle,
        Add,
        View,
        Edit,
        Delete,
    }

    pub(crate) const BINDINGS: &[Binding<Intent>] = &[
        Binding {
            display: "Space",
            keys: &[KeyCode::Char(' ')],
            intent: Intent::Toggle,
            label: |_, _| texts::tui_key_toggle(),
            shown: any_visible,
        },
        Binding {
            display: "a",
            keys: &[KeyCode::Char('a')],
            intent: Intent::Add,
            label: |_, _| texts::tui_key_add(),
            shown: |_, _| true,
        },
        Binding {
            display: "Enter",
            keys: &[KeyCode::Enter],
            intent: Intent::View,
            label: |_, _| texts::tui_key_view(),
            shown: any_visible,
        },
        Binding {
            display: "e",
            keys: &[KeyCode::Char('e')],
            intent: Intent::Edit,
            label: |_, _| texts::tui_key_edit(),
            shown: any_visible,
        },
        Binding {
            display: "d",
            keys: &[KeyCode::Char('d')],
            intent: Intent::Delete,
            label: |_, _| texts::tui_key_delete(),
            shown: any_visible,
        },
    ];

    pub(crate) fn intent_for(key: KeyCode) -> Option<Intent> {
        super::intent_for(BINDINGS, key)
    }

    pub(crate) fn key_bar_items(app: &App, data: &UiData) -> Vec<(&'static str, &'static str)> {
        super::key_bar_items(BINDINGS, app, data)
    }

    pub(crate) fn help_items(app: &App, data: &UiData) -> Vec<(&'static str, &'static str)> {
        super::help_items(BINDINGS, app, data)
    }

    fn any_visible(app: &App, data: &UiData) -> bool {
        visible_prompts(&app.filter, data)
            .get(app.prompt_idx)
            .copied()
            .is_some()
    }
}

pub(crate) mod skills_installed {
    use crossterm::event::KeyCode;

    use super::Binding;
    use crate::cli::i18n::texts;
    use crate::cli::tui::app::{visible_skills_installed, App};
    use crate::cli::tui::data::UiData;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Intent {
        Details,
        Toggle,
        Apps,
        Discover,
        Import,
        Uninstall,
    }

    pub(crate) const BINDINGS: &[Binding<Intent>] = &[
        Binding {
            display: "Enter",
            keys: &[KeyCode::Enter],
            intent: Intent::Details,
            label: |_, _| texts::tui_key_details(),
            shown: any_visible,
        },
        Binding {
            display: "Space",
            keys: &[KeyCode::Char(' ')],
            intent: Intent::Toggle,
            label: |_, _| texts::tui_key_toggle(),
            shown: any_visible,
        },
        Binding {
            display: "m",
            keys: &[KeyCode::Char('m')],
            intent: Intent::Apps,
            label: |_, _| texts::tui_key_apps(),
            shown: any_visible,
        },
        Binding {
            display: "f",
            keys: &[KeyCode::Char('f')],
            intent: Intent::Discover,
            label: |_, _| texts::tui_key_discover(),
            shown: |_, _| true,
        },
        Binding {
            display: "i",
            keys: &[KeyCode::Char('i')],
            intent: Intent::Import,
            label: |_, _| texts::tui_skills_action_import_existing(),
            shown: |_, _| true,
        },
        Binding {
            display: "d",
            keys: &[KeyCode::Char('d')],
            intent: Intent::Uninstall,
            label: |_, _| texts::tui_key_uninstall(),
            shown: any_visible,
        },
    ];

    pub(crate) fn intent_for(key: KeyCode) -> Option<Intent> {
        super::intent_for(BINDINGS, key)
    }

    pub(crate) fn key_bar_items(app: &App, data: &UiData) -> Vec<(&'static str, &'static str)> {
        super::key_bar_items(BINDINGS, app, data)
    }

    pub(crate) fn help_items(app: &App, data: &UiData) -> Vec<(&'static str, &'static str)> {
        super::help_items(BINDINGS, app, data)
    }

    fn any_visible(app: &App, data: &UiData) -> bool {
        visible_skills_installed(&app.filter, data)
            .get(app.skills_idx)
            .copied()
            .is_some()
    }
}

pub(crate) mod usage {
    use crossterm::event::KeyCode;

    use super::Binding;
    use crate::cli::i18n::texts;
    use crate::cli::tui::app::App;
    use crate::cli::tui::data::UiData;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Intent {
        RangeToday,
        RangeSevenDays,
        RangeThirtyDays,
        CustomRange,
        NextMetric,
        PrevMetric,
        OpenLogs,
        OpenPricing,
        Reload,
    }

    pub(crate) const BINDINGS: &[Binding<Intent>] = &[
        Binding {
            display: "1",
            keys: &[KeyCode::Char('1')],
            intent: Intent::RangeToday,
            label: |_, _| crate::t!("Today", "今日"),
            shown: |_, _| true,
        },
        Binding {
            display: "2",
            keys: &[KeyCode::Char('2')],
            intent: Intent::RangeSevenDays,
            label: |_, _| crate::t!("7 days", "7天"),
            shown: |_, _| true,
        },
        Binding {
            display: "3",
            keys: &[KeyCode::Char('3')],
            intent: Intent::RangeThirtyDays,
            label: |_, _| crate::t!("30 days", "30天"),
            shown: |_, _| true,
        },
        Binding {
            display: "c",
            keys: &[KeyCode::Char('c'), KeyCode::Char('C')],
            intent: Intent::CustomRange,
            label: |_, _| crate::t!("custom range", "自定义区间"),
            shown: |_, _| true,
        },
        // Tab cycles the trend metric here; pane switching only exists on
        // the UsageLogs route. Shift+Tab arrives as either BackTab or
        // Tab+SHIFT, so the handler pre-checks the modifier variant.
        Binding {
            display: "Tab",
            keys: &[KeyCode::Tab],
            intent: Intent::NextMetric,
            label: |_, _| crate::t!("switch metric", "切换指标"),
            shown: |_, _| true,
        },
        Binding {
            display: "Shift+Tab",
            keys: &[KeyCode::BackTab],
            intent: Intent::PrevMetric,
            label: |_, _| crate::t!("switch metric", "切换指标"),
            shown: super::never,
        },
        Binding {
            display: "L",
            keys: &[KeyCode::Char('L')],
            intent: Intent::OpenLogs,
            label: |_, _| crate::t!("details", "详情"),
            shown: |_, _| true,
        },
        Binding {
            display: "P",
            keys: &[KeyCode::Char('P'), KeyCode::Char('p')],
            intent: Intent::OpenPricing,
            label: |_, _| crate::t!("pricing", "模型定价"),
            shown: |_, _| true,
        },
        Binding {
            display: "r",
            keys: &[KeyCode::Char('r')],
            intent: Intent::Reload,
            label: |_, _| texts::tui_key_refresh(),
            shown: |_, _| true,
        },
    ];

    pub(crate) fn intent_for(key: KeyCode) -> Option<Intent> {
        super::intent_for(BINDINGS, key)
    }

    pub(crate) fn key_bar_items(app: &App, data: &UiData) -> Vec<(&'static str, &'static str)> {
        super::key_bar_items(BINDINGS, app, data)
    }

    pub(crate) fn help_items(app: &App, data: &UiData) -> Vec<(&'static str, &'static str)> {
        super::help_items(BINDINGS, app, data)
    }
}

pub(crate) mod sessions {
    use crossterm::event::KeyCode;

    use super::Binding;
    use crate::cli::i18n::texts;
    use crate::cli::tui::app::App;
    use crate::cli::tui::data::UiData;

    /// Action keys only. Pane/list navigation (arrows, h/l, PageUp/Down,
    /// Home/End) is handled globally and in the handler's explicit arms
    /// because it is pane-dependent and reused while the filter is active;
    /// only these actions resolve through the table. The pane checks for
    /// `View` stay in the handler body (the Providers pattern).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Intent {
        View,
        Restore,
        Delete,
        Refresh,
        Project,
    }

    pub(crate) const BINDINGS: &[Binding<Intent>] = &[
        Binding {
            display: "Enter",
            keys: &[KeyCode::Enter],
            intent: Intent::View,
            label: |_, _| texts::tui_key_view(),
            shown: |_, _| true,
        },
        Binding {
            display: "R",
            keys: &[KeyCode::Char('R')],
            intent: Intent::Restore,
            label: |_, _| texts::tui_key_restore(),
            shown: |_, _| true,
        },
        Binding {
            display: "d",
            keys: &[KeyCode::Char('d')],
            intent: Intent::Delete,
            label: |_, _| texts::tui_key_delete(),
            shown: |_, _| true,
        },
        Binding {
            display: "r",
            keys: &[KeyCode::Char('r')],
            intent: Intent::Refresh,
            label: |_, _| texts::tui_key_refresh(),
            shown: |_, _| true,
        },
        Binding {
            display: "p",
            keys: &[KeyCode::Char('p')],
            intent: Intent::Project,
            label: |_, _| texts::tui_key_sessions_project(),
            shown: |_, _| true,
        },
    ];

    pub(crate) fn intent_for(key: KeyCode) -> Option<Intent> {
        super::intent_for(BINDINGS, key)
    }

    pub(crate) fn key_bar_items(app: &App, data: &UiData) -> Vec<(&'static str, &'static str)> {
        super::key_bar_items(BINDINGS, app, data)
    }

    pub(crate) fn help_items(app: &App, data: &UiData) -> Vec<(&'static str, &'static str)> {
        super::help_items(BINDINGS, app, data)
    }
}

#[cfg(test)]
mod tests {
    use super::providers::{self, Intent};
    use super::sessions;
    use crossterm::event::KeyCode;

    #[test]
    fn providers_keys_resolve_to_documented_intents() {
        assert_eq!(providers::intent_for(KeyCode::Enter), Some(Intent::Primary));
        assert_eq!(
            providers::intent_for(KeyCode::Char(' ')),
            Some(Intent::Switch)
        );
        // Hidden alias: `s` switches like Space.
        assert_eq!(
            providers::intent_for(KeyCode::Char('s')),
            Some(Intent::Switch)
        );
        assert_eq!(providers::intent_for(KeyCode::Char('a')), Some(Intent::Add));
        assert_eq!(
            providers::intent_for(KeyCode::Char('c')),
            Some(Intent::Copy)
        );
        assert_eq!(
            providers::intent_for(KeyCode::Char('e')),
            Some(Intent::Edit)
        );
        assert_eq!(
            providers::intent_for(KeyCode::Char('d')),
            Some(Intent::Delete)
        );
        assert_eq!(
            providers::intent_for(KeyCode::Char('t')),
            Some(Intent::Test)
        );
        assert_eq!(
            providers::intent_for(KeyCode::Char('r')),
            Some(Intent::RefreshQuota)
        );
        assert_eq!(
            providers::intent_for(KeyCode::Char('o')),
            Some(Intent::LaunchTemp)
        );
        assert_eq!(
            providers::intent_for(KeyCode::Char('f')),
            Some(Intent::Failover)
        );
        assert_eq!(
            providers::intent_for(KeyCode::Char('x')),
            Some(Intent::SetDefault)
        );
        assert_eq!(providers::intent_for(KeyCode::Char('z')), None);
    }

    #[test]
    fn every_intent_has_exactly_one_binding() {
        let mut seen = Vec::new();
        for binding in providers::BINDINGS {
            assert!(
                !seen.contains(&binding.intent),
                "duplicate binding for {:?}",
                binding.intent
            );
            seen.push(binding.intent);
        }
    }

    #[test]
    fn sessions_a_is_not_a_registered_action() {
        assert_eq!(sessions::intent_for(KeyCode::Char('a')), None);
        assert_eq!(
            sessions::intent_for(KeyCode::Char('p')),
            Some(sessions::Intent::Project)
        );
    }
}
