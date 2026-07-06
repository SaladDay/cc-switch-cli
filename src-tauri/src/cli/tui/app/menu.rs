use super::*;

const PROXY_ACTIVITY_WINDOW: usize = 48;
const PROXY_ACTIVITY_POLL_INTERVAL_TICKS: u64 = 5;

impl App {
    pub(crate) fn clear_openclaw_daily_memory_search_state(&mut self) {
        self.filter.active = false;
        self.filter.input.set("");
        self.openclaw_daily_memory_search_query.clear();
        self.openclaw_daily_memory_search_results.clear();
        self.daily_memory_idx = 0;
    }

    pub(crate) fn displayed_filter_input(&self) -> &TextInput {
        match self.displayed_filter_scope() {
            FilterScope::Global => &self.filter.input,
            FilterScope::SessionMessages => &self.sessions.message_filter,
        }
    }

    pub(crate) fn should_show_filter_bar(&self) -> bool {
        self.filter.active || !self.displayed_filter_input().value.trim().is_empty()
    }

    fn displayed_filter_scope(&self) -> FilterScope {
        if self.filter.active {
            return self.filter.scope;
        }
        if matches!(self.route, Route::Sessions)
            && matches!(self.focus, Focus::Content)
            && matches!(self.sessions.pane, SessionsPane::Detail)
            && !self.sessions.message_filter.value.trim().is_empty()
        {
            return FilterScope::SessionMessages;
        }
        FilterScope::Global
    }

    fn active_filter_input_mut(&mut self) -> &mut TextInput {
        match self.filter.scope {
            FilterScope::Global => &mut self.filter.input,
            FilterScope::SessionMessages => &mut self.sessions.message_filter,
        }
    }

    pub fn new(app_override: Option<AppType>) -> Self {
        let app_type = app_override.unwrap_or(AppType::Claude);
        Self {
            app_type,
            route: Route::Main,
            route_stack: Vec::new(),
            focus: Focus::Nav,
            nav_idx: 0,
            filter: FilterState::new(),
            editor: None,
            form: None,
            pending_overlay: None,
            overlay: Overlay::None,
            toast: None,
            should_quit: false,
            pending_deep_search: None,
            last_size: Size::new(0, 0),
            tick: 0,
            proxy_input_activity_samples: Vec::new(),
            proxy_output_activity_samples: Vec::new(),
            proxy_activity_last_input_tokens: None,
            proxy_activity_last_output_tokens: None,
            proxy_visual_state: None,
            proxy_visual_transition: None,
            quota_auto_target_key: None,
            quota_last_auto_tick: None,
            prompt_import_prompted_apps: HashSet::new(),
            common_config_notice_confirmed: true,
            usage_query_notice_confirmed: true,
            local_env_results: Vec::new(),
            local_env_loading: true,
            usage: UsageState::default(),
            pricing: PricingState::default(),
            sessions: SessionsState::default(),
            provider_idx: 0,
            mcp_idx: 0,
            prompt_idx: 0,
            skills_idx: 0,
            skills_discover_idx: 0,
            skills_repo_idx: 0,
            skills_unmanaged_idx: 0,
            skills_discover_results: Vec::new(),
            skills_discover_query: String::new(),
            skills_discover_source: SkillsDiscoverSource::Repos,
            skills_discover_loading: false,
            skills_discover_request_id: 0,
            skills_discover_active_request_id: None,
            skills_discover_cache: HashMap::new(),
            skills_unmanaged_results: Vec::new(),
            skills_unmanaged_selected: HashSet::new(),
            config_idx: 0,
            workspace_idx: 0,
            daily_memory_idx: 0,
            hermes_memory_idx: 0,
            openclaw_tools_form: None,
            openclaw_agents_form: None,
            openclaw_daily_memory_search_query: String::new(),
            openclaw_daily_memory_search_results: Vec::new(),
            config_webdav_idx: 0,
            webdav_quick_setup_username: None,
            language_idx: 0,
            settings_idx: 0,
            settings_proxy_idx: 0,
            settings_managed_accounts_idx: 0,
            managed_auth_status: None,
            managed_auth_loading: false,
            managed_auth_login: None,
        }
    }

    pub fn nav_item(&self) -> NavItem {
        self.nav_items()
            .get(self.nav_idx)
            .copied()
            .unwrap_or(NavItem::Main)
    }

    pub(crate) fn nav_items(&self) -> &'static [NavItem] {
        NavItem::all_for_app(&self.app_type)
    }

    pub(crate) fn nav_item_for_route(app_type: &AppType, route: &Route) -> NavItem {
        match route {
            Route::Main => NavItem::Main,
            Route::Providers => NavItem::Providers,
            Route::Usage | Route::UsageLogs | Route::UsageLogDetail { .. } | Route::Pricing => {
                NavItem::Usage
            }
            Route::Sessions => NavItem::Sessions,
            Route::Mcp => NavItem::Mcp,
            Route::Prompts => NavItem::Prompts,
            Route::HermesMemory => NavItem::HermesMemory,
            Route::Config => NavItem::Config,
            Route::ConfigOpenClawWorkspace | Route::ConfigOpenClawDailyMemory => {
                if matches!(app_type, AppType::OpenClaw) {
                    NavItem::OpenClawWorkspace
                } else {
                    NavItem::Config
                }
            }
            Route::ConfigOpenClawEnv => {
                if matches!(app_type, AppType::OpenClaw) {
                    NavItem::OpenClawEnv
                } else {
                    NavItem::Config
                }
            }
            Route::ConfigOpenClawTools => {
                if matches!(app_type, AppType::OpenClaw) {
                    NavItem::OpenClawTools
                } else {
                    NavItem::Config
                }
            }
            Route::ConfigOpenClawAgents => {
                if matches!(app_type, AppType::OpenClaw) {
                    NavItem::OpenClawAgents
                } else {
                    NavItem::Config
                }
            }
            Route::ConfigWebDav => NavItem::Config,
            Route::Skills
            | Route::SkillsDiscover
            | Route::SkillsRepos
            | Route::SkillDetail { .. } => NavItem::Skills,
            Route::Settings | Route::SettingsProxy | Route::SettingsManagedAccounts => {
                NavItem::Settings
            }
        }
    }

    pub(crate) fn set_route_no_history(&mut self, route: Route) -> Action {
        if route == self.route {
            return Action::None;
        }

        let was_daily_memory = matches!(self.route, Route::ConfigOpenClawDailyMemory);
        let is_daily_memory = matches!(route, Route::ConfigOpenClawDailyMemory);
        if was_daily_memory != is_daily_memory {
            self.clear_openclaw_daily_memory_search_state();
        }
        if !matches!(route, Route::ConfigOpenClawTools) {
            self.openclaw_tools_form = None;
        }
        if !matches!(route, Route::ConfigOpenClawAgents) {
            self.openclaw_agents_form = None;
        }
        if matches!(route, Route::Sessions) {
            self.sessions.reset_time_anchor();
        }

        self.route = route.clone();
        self.focus = route_default_focus(&route);

        let nav_item = Self::nav_item_for_route(&self.app_type, &route);
        if let Some(idx) = self.nav_items().iter().position(|item| *item == nav_item) {
            self.nav_idx = idx;
        }

        if matches!(route, Route::Main) {
            self.route_stack.clear();
            self.focus = Focus::Nav;
        }

        Action::SwitchRoute(route)
    }

    pub(crate) fn maybe_prompt_import_candidate(&mut self, data: &UiData) {
        if !matches!(self.route, Route::Prompts) {
            return;
        }
        if self.overlay.is_active() || self.form.is_some() || self.editor.is_some() {
            return;
        }
        if !data.prompts.rows.is_empty() {
            return;
        }
        let Some(candidate) = data.prompts.import_candidate.as_ref() else {
            return;
        };
        let app_key = self.app_type.as_str().to_string();
        if !self.prompt_import_prompted_apps.insert(app_key) {
            return;
        }

        self.overlay = Overlay::Confirm(ConfirmOverlay {
            title: texts::tui_confirm_import_prompt_title().to_string(),
            message: texts::tui_confirm_import_prompt_message(&candidate.filename),
            action: ConfirmAction::PromptOpenImportCandidate {
                filename: candidate.filename.clone(),
                content: candidate.content.clone(),
            },
        });
    }

    pub(crate) fn push_route_and_switch(&mut self, route: Route) -> Action {
        if route == self.route {
            return Action::None;
        }
        self.route_stack.push(self.route.clone());
        self.set_route_no_history(route)
    }

    pub(crate) fn pop_route_and_switch(&mut self) -> Action {
        if let Some(prev) = self.route_stack.pop() {
            self.set_route_no_history(prev)
        } else {
            self.set_route_no_history(Route::Main)
        }
    }

    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.expire_managed_auth_login_if_needed();

        // Deep search debounce: count ticks since last input, fire after threshold
        const DEEP_SEARCH_DEBOUNCE_TICKS: u64 = 2; // ~2 ticks ≈ 400ms at 200ms/tick
        if let Some((query, ticks)) = &mut self.sessions.deep_search_pending {
            *ticks += 1;
            if *ticks >= DEEP_SEARCH_DEBOUNCE_TICKS {
                let q = std::mem::take(query);
                self.sessions.deep_search_pending = None;
                self.pending_deep_search = Some(q);
            }
        }

        if let Some(toast) = &mut self.toast {
            if !toast.persistent && toast.remaining_ticks > 0 {
                toast.remaining_ticks -= 1;
            }
            if !toast.persistent && toast.remaining_ticks == 0 {
                self.toast = None;
            }
        }

        if let Some(transition) = self.proxy_visual_transition {
            if self.tick.saturating_sub(transition.started_tick) >= PROXY_HERO_TRANSITION_TICKS {
                self.proxy_visual_transition = None;
            }
        }
    }

    fn expire_managed_auth_login_if_needed(&mut self) {
        let Some(login) = self.managed_auth_login.as_ref() else {
            return;
        };
        if self.tick < login.expires_at_tick {
            return;
        }

        self.managed_auth_login = None;
        self.managed_auth_loading = false;
        self.clear_managed_auth_cancel_confirm();
        self.push_toast(
            texts::tui_toast_managed_auth_login_expired(),
            ToastKind::Warning,
        );
    }

    pub(crate) fn should_poll_managed_auth_login(&self) -> bool {
        self.managed_auth_login.as_ref().is_some_and(|login| {
            self.tick < login.expires_at_tick && self.tick >= login.next_poll_tick
        })
    }

    pub(crate) fn clear_codex_oauth_binding_if_removed(&mut self, account_id: &str) {
        let Some(FormState::ProviderAdd(provider)) = self.form.as_mut() else {
            return;
        };
        if provider.codex_oauth_account_id.as_deref() == Some(account_id) {
            provider.set_codex_oauth_account_id(None);
        }
    }

    pub(crate) fn observe_proxy_visual_state(&mut self, data: &UiData) {
        let current_on = data.proxy.running;

        match self.proxy_visual_state.replace(current_on) {
            None => {}
            Some(previous_on) if previous_on != current_on => {
                self.proxy_visual_transition = Some(ProxyVisualTransition {
                    from_on: previous_on,
                    to_on: current_on,
                    started_tick: self.tick,
                });
            }
            Some(_) => {}
        }
    }

    pub(crate) fn should_poll_proxy_activity(&self) -> bool {
        matches!(self.route, Route::Main)
            && self.tick.is_multiple_of(PROXY_ACTIVITY_POLL_INTERVAL_TICKS)
    }

    pub(crate) fn reset_proxy_activity(&mut self, input_tokens: u64, output_tokens: u64) {
        self.proxy_input_activity_samples.clear();
        self.proxy_output_activity_samples.clear();
        self.proxy_activity_last_input_tokens = Some(input_tokens);
        self.proxy_activity_last_output_tokens = Some(output_tokens);
    }

    pub(crate) fn observe_proxy_token_activity(&mut self, input_tokens: u64, output_tokens: u64) {
        let Some(previous_input) = self.proxy_activity_last_input_tokens.replace(input_tokens)
        else {
            return;
        };
        let Some(previous_output) = self
            .proxy_activity_last_output_tokens
            .replace(output_tokens)
        else {
            return;
        };

        let (input_delta, output_delta) =
            if input_tokens < previous_input || output_tokens < previous_output {
                self.proxy_input_activity_samples.clear();
                self.proxy_output_activity_samples.clear();
                (0, 0)
            } else {
                (
                    input_tokens.saturating_sub(previous_input),
                    output_tokens.saturating_sub(previous_output),
                )
            };

        self.proxy_input_activity_samples.push(input_delta);
        self.proxy_output_activity_samples.push(output_delta);

        if self.proxy_input_activity_samples.len() > PROXY_ACTIVITY_WINDOW {
            let overflow = self.proxy_input_activity_samples.len() - PROXY_ACTIVITY_WINDOW;
            self.proxy_input_activity_samples.drain(0..overflow);
        }
        if self.proxy_output_activity_samples.len() > PROXY_ACTIVITY_WINDOW {
            let overflow = self.proxy_output_activity_samples.len() - PROXY_ACTIVITY_WINDOW;
            self.proxy_output_activity_samples.drain(0..overflow);
        }
    }

    pub fn push_toast(&mut self, message: impl Into<String>, kind: ToastKind) {
        self.toast = Some(Toast::new(message, kind));
    }

    pub fn push_persistent_toast(&mut self, message: impl Into<String>, kind: ToastKind) {
        self.toast = Some(Toast::persistent(message, kind));
    }

    pub(crate) fn clear_managed_auth_login_toast(&mut self) {
        if self.toast.as_ref().is_some_and(|toast| toast.persistent) {
            self.toast = None;
        }
    }

    pub(crate) fn clear_managed_auth_cancel_confirm(&mut self) {
        if matches!(
            &self.overlay,
            Overlay::Confirm(ConfirmOverlay {
                action: ConfirmAction::ManagedAuthCancelLogin,
                ..
            })
        ) {
            self.close_overlay();
        }
    }

    pub(crate) fn cancel_managed_auth_login(&mut self) {
        if self.managed_auth_login.take().is_some() {
            self.managed_auth_loading = false;
            self.clear_managed_auth_login_toast();
            self.push_toast(
                texts::tui_toast_managed_auth_login_cancelled(),
                ToastKind::Info,
            );
        }
    }

    fn confirm_managed_auth_login_cancel(&mut self) {
        self.overlay = Overlay::Confirm(ConfirmOverlay {
            title: texts::tui_confirm_managed_auth_cancel_title().to_string(),
            message: texts::tui_confirm_managed_auth_cancel_message().to_string(),
            action: ConfirmAction::ManagedAuthCancelLogin,
        });
    }

    pub(crate) fn prompt_visible_apps_auto_detection(&mut self) {
        if self.overlay.is_active() || self.pending_overlay.is_some() {
            self.pending_overlay = Some(Overlay::Confirm(ConfirmOverlay {
                title: texts::tui_visible_apps_auto_prompt_title().to_string(),
                message: texts::tui_visible_apps_auto_prompt_message().to_string(),
                action: ConfirmAction::VisibleAppsAutoDetection,
            }));
        } else {
            self.overlay = Overlay::Confirm(ConfirmOverlay {
                title: texts::tui_visible_apps_auto_prompt_title().to_string(),
                message: texts::tui_visible_apps_auto_prompt_message().to_string(),
                action: ConfirmAction::VisibleAppsAutoDetection,
            });
        }
    }

    pub fn open_help(&mut self, data: &UiData) {
        if self.help_should_open_proxy_view() {
            self.open_proxy_help_view(data, None);
            return;
        }

        let help = Overlay::Help(crate::cli::tui::help::HelpState::new(
            crate::cli::tui::help::context_help_for_app(self),
        ));
        if self.overlay.can_be_covered_by_help() {
            let previous = std::mem::replace(&mut self.overlay, help);
            self.pending_overlay = Some(previous);
        } else if !self.overlay.is_active() {
            self.overlay = help;
        }
    }

    fn help_shortcut_is_available(&self) -> bool {
        if self.editor.is_some() || self.filter.active || self.form_text_input_is_active() {
            return false;
        }
        if matches!(self.overlay, Overlay::Help(_)) || self.overlay_text_input_is_active() {
            return false;
        }
        !self.overlay.is_active()
            || (self.pending_overlay.is_none() && self.overlay.can_be_covered_by_help())
    }

    fn help_should_open_proxy_view(&self) -> bool {
        if self.overlay.is_active() {
            return false;
        }
        if matches!(self.route, Route::SettingsProxy) {
            return true;
        }
        if matches!(self.route, Route::Settings) && matches!(self.focus, Focus::Content) {
            return matches!(
                SettingsItem::ALL.get(self.settings_idx),
                Some(SettingsItem::Proxy)
            );
        }
        if matches!(self.route, Route::Config) && matches!(self.focus, Focus::Content) {
            return visible_config_items(&self.filter, &self.app_type)
                .get(self.config_idx)
                .is_some_and(|item| matches!(item, ConfigItem::Proxy));
        }
        false
    }

    pub fn close_overlay(&mut self) {
        self.overlay = self.pending_overlay.take().unwrap_or(Overlay::None);
    }

    fn overlay_text_input_is_active(&self) -> bool {
        self.overlay.is_editing()
    }

    fn form_text_input_is_active(&self) -> bool {
        self.form.as_ref().is_some_and(|f| f.is_editing())
    }

    fn text_input_is_active(&self) -> bool {
        self.overlay_text_input_is_active()
            || self.editor.is_some()
            || self.filter.active
            || self.form_text_input_is_active()
    }

    fn normalize_vim_navigation_key(&self, key: KeyEvent) -> KeyEvent {
        if self.text_input_is_active() {
            return key;
        }

        match key.code {
            KeyCode::Char('h') => KeyEvent::new(KeyCode::Left, key.modifiers),
            KeyCode::Char('j') => KeyEvent::new(KeyCode::Down, key.modifiers),
            KeyCode::Char('k') => KeyEvent::new(KeyCode::Up, key.modifiers),
            KeyCode::Char('l') => KeyEvent::new(KeyCode::Right, key.modifiers),
            _ => key,
        }
    }

    fn should_route_printable_content_input_before_globals(&self, key: &KeyEvent) -> bool {
        matches!(self.focus, Focus::Content)
            && self.text_input_is_active()
            && matches!(key.code, KeyCode::Char(c) if !c.is_control())
            && !key.modifiers.contains(KeyModifiers::CONTROL)
    }

    pub fn on_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        self.clamp_selections(data);
        if !self.overlay.is_active() {
            self.pending_overlay = None;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.should_quit = true;
            return Action::Quit;
        }

        if self.managed_auth_login.is_some()
            && !self.overlay.is_active()
            && !self.text_input_is_active()
            && matches!(key.code, KeyCode::Esc)
        {
            self.confirm_managed_auth_login_cancel();
            return Action::None;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char(','))
            && !self.overlay.is_active()
            && self.editor.is_none()
            && self.form.is_none()
        {
            return self.push_route_and_switch(Route::Settings);
        }

        let key = self.normalize_vim_navigation_key(key);

        if matches!(key.code, KeyCode::Char('?')) && self.help_shortcut_is_available() {
            self.open_help(data);
            return Action::None;
        }

        if self.overlay.is_active() {
            return self.on_overlay_key(key, data);
        }

        if self.editor.is_some() {
            return self.on_editor_key(key);
        }

        if self.form.is_some() {
            return self.on_form_key(key, data);
        }

        if self.filter.active {
            return self.on_filter_key(key, data);
        }

        if self.should_route_printable_content_input_before_globals(&key) {
            return self.on_content_key(key, data);
        }

        // Global actions.
        match key.code {
            KeyCode::Char('/') => {
                self.filter.active = true;
                self.prepare_filter_focus();
                return Action::None;
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter.active = true;
                self.prepare_filter_focus();
                return Action::None;
            }
            KeyCode::Char('[') | KeyCode::Char('【') | KeyCode::Char('［') => {
                return cycle_app_type(&self.app_type, -1)
                    .map(Action::SetAppType)
                    .unwrap_or(Action::None);
            }
            KeyCode::Char(']') | KeyCode::Char('】') | KeyCode::Char('］') => {
                return cycle_app_type(&self.app_type, 1)
                    .map(Action::SetAppType)
                    .unwrap_or(Action::None);
            }
            KeyCode::Left if matches!(self.route, Route::Sessions) => {
                return self.move_sessions_focus_left();
            }
            KeyCode::Left => {
                self.focus = Focus::Nav;
                return Action::None;
            }
            KeyCode::Right if matches!(self.route, Route::Sessions) => {
                return self.move_sessions_focus_right(data);
            }
            KeyCode::Right => {
                if route_has_content_list(&self.route) {
                    self.focus = Focus::Content;
                } else {
                    self.focus = Focus::Nav;
                }
                return Action::None;
            }
            KeyCode::Tab if matches!(self.route, Route::Sessions) => {
                return if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.move_sessions_focus_left()
                } else {
                    self.move_sessions_focus_right(data)
                };
            }
            KeyCode::Tab
                if matches!(self.route, Route::Usage) && matches!(self.focus, Focus::Content) =>
            {
                return self.on_usage_key(key, data);
            }
            KeyCode::BackTab if matches!(self.route, Route::Sessions) => {
                return self.move_sessions_focus_left();
            }
            KeyCode::BackTab
                if matches!(self.route, Route::Usage) && matches!(self.focus, Focus::Content) =>
            {
                return self.on_usage_key(key, data);
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                // If in sessions "show all providers" mode, Esc exits that mode
                // instead of navigating back.
                if matches!(self.route, Route::Sessions) && self.sessions.show_all_providers {
                    self.sessions.show_all_providers = false;
                    self.sessions.loaded_once = false;
                    self.sessions.selected_idx = 0;
                    self.sessions.clear_detail();
                    return Action::None;
                }
                return self.on_back_key();
            }
            _ => {}
        }

        if matches!(self.route, Route::Main)
            && matches!(key.code, KeyCode::Char('p') | KeyCode::Char('P'))
        {
            return self.main_proxy_action(data);
        }

        // Navigation + route-specific actions.
        match self.focus {
            Focus::Nav => self.on_nav_key(key),
            Focus::Content => self.on_content_key(key, data),
        }
    }

    pub(crate) fn on_back_key(&mut self) -> Action {
        match self.route {
            Route::Main => {
                self.overlay = Overlay::Confirm(ConfirmOverlay {
                    title: crate::cli::i18n::texts::tui_confirm_exit_title().to_string(),
                    message: crate::cli::i18n::texts::tui_confirm_exit_message().to_string(),
                    action: ConfirmAction::Quit,
                });
                Action::None
            }
            _ => self.pop_route_and_switch(),
        }
    }

    pub(crate) fn on_filter_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        let scope = self.filter.scope;
        let is_daily_memory = matches!(scope, FilterScope::Global)
            && matches!(self.route, Route::ConfigOpenClawDailyMemory);
        let mut filter_changed = false;
        let action = match key.code {
            KeyCode::Esc => {
                filter_changed = !self.active_filter_input_mut().value.is_empty();
                self.filter.active = false;
                self.active_filter_input_mut().set("");
                // Clear deep search when the session list filter is cleared.
                // Scope to the Global (list) filter so clearing an in-detail
                // message filter does not wipe list-level search state, and
                // also drop any queued (debounced) search so it doesn't revive
                // after Esc.
                if matches!(self.route, Route::Sessions) && matches!(scope, FilterScope::Global) {
                    self.sessions.deep_search_query = None;
                    self.sessions.deep_search_results.clear();
                    self.sessions.deep_search_pending = None;
                    // Drop any in-flight search too, so its result is ignored on
                    // arrival and the "searching" spinner stops immediately.
                    self.sessions.deep_search_active = None;
                }
                if is_daily_memory {
                    self.openclaw_daily_memory_search_results.clear();
                    self.daily_memory_idx = 0;
                    Action::OpenClawDailyMemorySearch {
                        query: String::new(),
                    }
                } else {
                    Action::None
                }
            }
            KeyCode::Enter => {
                self.filter.active = false;
                if is_daily_memory {
                    Action::OpenClawDailyMemorySearch {
                        query: self.filter.input.value.clone(),
                    }
                } else if matches!(self.route, Route::Sessions) {
                    let q = self.filter.input.value.trim().to_string();
                    if q.is_empty() {
                        self.sessions.deep_search_query = None;
                        self.sessions.deep_search_results.clear();
                        self.sessions.deep_search_pending = None;
                        self.sessions.deep_search_active = None;
                        Action::None
                    } else {
                        // If deep search has already completed for this query,
                        // open the selected session's detail directly instead
                        // of re-triggering the search — saving the user an extra
                        // Enter press.
                        let search_completed = self.sessions.deep_search_active.is_none()
                            && self.sessions.deep_search_query.as_deref() == Some(q.as_str());
                        if search_completed {
                            // Return early to skip sync_after_filter_key —
                            // move_sessions_focus_right calls clamp_selections
                            // and sets pane to Detail internally.
                            return self.move_sessions_focus_right(data);
                        }
                        // Search not yet done — fire it now.
                        self.sessions.deep_search_pending = None;
                        Action::SessionsDeepSearch { query: q }
                    }
                } else {
                    Action::None
                }
            }
            // Allow session list navigation (Up/Down/PageUp/PageDown/Home/End)
            // while the filter is active, so users can browse search results
            // without pressing Enter to exit filter mode first.
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End
                if matches!(self.route, Route::Sessions) && !is_daily_memory =>
            {
                let action = self.on_sessions_key(key, data);
                self.clamp_selections(data);
                action
            }
            _ => {
                let Some(edit) = self.active_filter_input_mut().apply_key(key) else {
                    return Action::None;
                };
                filter_changed = edit.changed;
                // In sessions page, set pending deep search only when text content changes
                // (not on cursor movement via Left/Right keys)
                if edit.changed && matches!(self.route, Route::Sessions) {
                    let q = self.filter.input.value.trim().to_string();
                    // Only re-trigger if the query string actually changed
                    let current_query = self
                        .sessions
                        .deep_search_pending
                        .as_ref()
                        .map(|(q, _)| q.as_str())
                        .or(self.sessions.deep_search_query.as_deref());
                    if q == current_query.unwrap_or("") {
                        // Query didn't change (just cursor moved), don't reset pending
                    } else if q.is_empty() {
                        self.sessions.deep_search_pending = None;
                        self.sessions.deep_search_query = None;
                        self.sessions.deep_search_results.clear();
                        self.sessions.deep_search_active = None;
                    } else {
                        self.sessions.deep_search_pending = Some((q, 0));
                        // The query changed, so any in-flight search is now
                        // stale: invalidate it so its result is not accepted
                        // under the new query text.
                        self.sessions.deep_search_active = None;
                    }
                }
                if is_daily_memory && edit.changed && self.filter.input.value.is_empty() {
                    Action::OpenClawDailyMemorySearch {
                        query: String::new(),
                    }
                } else {
                    Action::None
                }
            }
        };
        self.sync_after_filter_key(data, filter_changed, scope);
        action
    }

    pub(crate) fn on_nav_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Up => {
                self.nav_idx = self.nav_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                self.nav_idx = (self.nav_idx + 1).min(self.nav_items().len() - 1);
                Action::None
            }
            KeyCode::Enter => {
                if let Some(route) = self.nav_item().to_route() {
                    self.push_route_and_switch(route)
                } else {
                    self.overlay = Overlay::Confirm(ConfirmOverlay {
                        title: crate::cli::i18n::texts::tui_confirm_exit_title().to_string(),
                        message: crate::cli::i18n::texts::tui_confirm_exit_message().to_string(),
                        action: ConfirmAction::Quit,
                    });
                    Action::None
                }
            }
            _ => Action::None,
        }
    }

    pub(crate) fn on_content_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        match self.route.clone() {
            Route::Providers => self.on_providers_key(key, data),
            Route::Usage => self.on_usage_key(key, data),
            Route::UsageLogs => self.on_usage_logs_key(key, data),
            Route::UsageLogDetail { request_id } => self.on_usage_log_detail_key(key, &request_id),
            Route::Pricing => self.on_pricing_key(key, data),
            Route::Sessions => self.on_sessions_key(key, data),
            Route::Mcp => self.on_mcp_key(key, data),
            Route::Prompts => self.on_prompts_key(key, data),
            Route::HermesMemory => self.on_hermes_memory_key(key, data),
            Route::Config => self.on_config_key(key, data),
            Route::ConfigOpenClawWorkspace => self.on_config_openclaw_workspace_key(key, data),
            Route::ConfigOpenClawDailyMemory => self.on_config_openclaw_daily_memory_key(key, data),
            Route::ConfigOpenClawEnv => self.on_config_openclaw_env_key(key, data),
            Route::ConfigOpenClawTools => self.on_config_openclaw_tools_key(key, data),
            Route::ConfigOpenClawAgents => self.on_config_openclaw_agents_key(key, data),
            Route::ConfigWebDav => self.on_config_webdav_key(key, data),
            Route::Skills => self.on_skills_installed_key(key, data),
            Route::SkillsDiscover => self.on_skills_discover_key(key),
            Route::SkillsRepos => self.on_skills_repos_key(key, data),
            Route::SkillDetail { directory } => self.on_skill_detail_key(key, data, &directory),
            Route::Settings => self.on_settings_key(key, data),
            Route::SettingsProxy => self.on_settings_proxy_key(key, data),
            Route::SettingsManagedAccounts => self.on_settings_managed_accounts_key(key, data),
            Route::Main => match key.code {
                KeyCode::Char('r') => Action::LocalEnvRefresh,
                KeyCode::Char('p') | KeyCode::Char('P') => self.main_proxy_action(data),
                _ => Action::None,
            },
        }
    }

    fn prepare_filter_focus(&mut self) {
        if matches!(self.route, Route::Sessions)
            && matches!(self.focus, Focus::Content)
            && matches!(self.sessions.pane, SessionsPane::Detail)
        {
            self.filter.scope = FilterScope::SessionMessages;
        } else {
            self.filter.scope = FilterScope::Global;
        }
        if matches!(self.route, Route::Sessions) && matches!(self.filter.scope, FilterScope::Global)
        {
            self.sessions.pane = SessionsPane::List;
        }
    }

    fn sync_after_filter_key(&mut self, data: &UiData, filter_changed: bool, scope: FilterScope) {
        if matches!(scope, FilterScope::SessionMessages) {
            if filter_changed {
                clamp_session_message_selection(&mut self.sessions);
            }
            return;
        }
        if matches!(self.route, Route::Sessions) {
            self.sessions.pane = SessionsPane::List;
            if filter_changed {
                self.sessions.selected_idx = 0;
            }
        }
        self.clamp_selections(data);
    }

    pub(crate) fn clamp_selections(&mut self, data: &UiData) {
        let providers_len = visible_providers(&self.app_type, &self.filter, data).len();
        if providers_len == 0 {
            self.provider_idx = 0;
        } else {
            self.provider_idx = self.provider_idx.min(providers_len - 1);
        }

        let mcp_len = visible_mcp(&self.filter, data).len();
        if mcp_len == 0 {
            self.mcp_idx = 0;
        } else {
            self.mcp_idx = self.mcp_idx.min(mcp_len - 1);
        }

        let prompt_len = visible_prompts(&self.filter, data).len();
        if prompt_len == 0 {
            self.prompt_idx = 0;
        } else {
            self.prompt_idx = self.prompt_idx.min(prompt_len - 1);
        }

        let visible_session_rows = visible_sessions_for_state(
            &self.filter,
            &self.app_type,
            self.sessions.show_all_providers,
            &self.sessions.rows,
            self.sessions.detail_key.as_deref(),
            self.sessions.messages_loaded,
            &self.sessions.messages,
            self.sessions.deep_search_query.as_deref(),
            &self.sessions.deep_search_results,
        );
        let sessions_len = visible_session_rows.len();
        if sessions_len == 0 {
            self.sessions.selected_idx = 0;
        } else {
            self.sessions.selected_idx = self.sessions.selected_idx.min(sessions_len - 1);
        }
        let session_detail_missing = self.sessions.detail_key.as_deref().is_some_and(|key| {
            !visible_session_rows
                .iter()
                .any(|session| session_key(session) == key)
        });
        if session_detail_missing {
            self.sessions.clear_detail();
        }
        clamp_session_message_selection(&mut self.sessions);

        let usage_len = usage_active_pane_len(&self.usage.pane, self.usage.range, data);
        if usage_len == 0 {
            self.usage.selected_idx = 0;
        } else {
            self.usage.selected_idx = self.usage.selected_idx.min(usage_len - 1);
        }
        let usage_logs_len = data.usage.recent_logs_for(self.usage.range).len();
        if usage_logs_len == 0 {
            self.usage.logs_idx = 0;
        } else {
            self.usage.logs_idx = self.usage.logs_idx.min(usage_logs_len - 1);
        }

        let pricing_len = visible_pricing_rows(&self.filter, data).len();
        if pricing_len == 0 {
            self.pricing.selected_idx = 0;
        } else {
            self.pricing.selected_idx = self.pricing.selected_idx.min(pricing_len - 1);
        }

        let skills_len = visible_skills_installed(&self.filter, data).len();
        if skills_len == 0 {
            self.skills_idx = 0;
        } else {
            self.skills_idx = self.skills_idx.min(skills_len - 1);
        }

        let discover_len =
            visible_skills_discover(&self.filter, &self.skills_discover_results).len();
        if discover_len == 0 {
            self.skills_discover_idx = 0;
        } else {
            self.skills_discover_idx = self.skills_discover_idx.min(discover_len - 1);
        }

        let repos_len = visible_skills_repos(&self.filter, data).len();
        if repos_len == 0 {
            self.skills_repo_idx = 0;
        } else {
            self.skills_repo_idx = self.skills_repo_idx.min(repos_len - 1);
        }

        let unmanaged_len =
            visible_skills_unmanaged(&self.filter, &self.skills_unmanaged_results).len();
        if unmanaged_len == 0 {
            self.skills_unmanaged_idx = 0;
        } else {
            self.skills_unmanaged_idx = self.skills_unmanaged_idx.min(unmanaged_len - 1);
        }

        let config_len = visible_config_items(&self.filter, &self.app_type).len();
        if config_len == 0 {
            self.config_idx = 0;
        } else {
            self.config_idx = self.config_idx.min(config_len - 1);
        }

        let workspace_len = openclaw_workspace_entry_count();
        if workspace_len == 0 {
            self.workspace_idx = 0;
        } else {
            self.workspace_idx = self.workspace_idx.min(workspace_len - 1);
        }

        let daily_memory_len = visible_openclaw_daily_memory(self, data).len();
        if daily_memory_len == 0 {
            self.daily_memory_idx = 0;
        } else {
            self.daily_memory_idx = self.daily_memory_idx.min(daily_memory_len - 1);
        }

        let hermes_memory_len = crate::cli::tui::app::HERMES_MEMORY_ROW_COUNT;
        if hermes_memory_len == 0 {
            self.hermes_memory_idx = 0;
        } else {
            self.hermes_memory_idx = self.hermes_memory_idx.min(hermes_memory_len - 1);
        }

        let config_webdav_len = visible_webdav_config_items(&self.filter).len();
        if config_webdav_len == 0 {
            self.config_webdav_idx = 0;
        } else {
            self.config_webdav_idx = self.config_webdav_idx.min(config_webdav_len - 1);
        }
    }
}
