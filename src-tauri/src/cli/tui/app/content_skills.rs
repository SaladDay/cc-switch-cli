use super::*;

impl App {
    pub(crate) fn main_proxy_action(&self, data: &UiData) -> Action {
        let Some(current_app_routed) = data.proxy.routes_current_app_through_proxy(&self.app_type)
        else {
            return Action::None;
        };

        if data.proxy.running && !data.proxy.managed_runtime && !current_app_routed {
            return Action::None;
        }

        Action::SetManagedProxyForCurrentApp {
            app_type: self.app_type.clone(),
            enabled: !current_app_routed,
        }
    }

    pub(crate) fn on_skills_installed_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        use crate::cli::tui::keymap::skills_installed::Intent;

        let visible = visible_skills_installed(&self.filter, data);

        match key.code {
            KeyCode::Up => {
                self.skills_idx = self.skills_idx.saturating_sub(1);
                return Action::None;
            }
            KeyCode::Down => {
                if !visible.is_empty() {
                    self.skills_idx = (self.skills_idx + 1).min(visible.len() - 1);
                }
                return Action::None;
            }
            _ => {}
        }

        let Some(intent) = crate::cli::tui::keymap::skills_installed::intent_for(key.code) else {
            return Action::None;
        };

        match intent {
            Intent::Details => {
                let Some(skill) = visible.get(self.skills_idx) else {
                    return Action::None;
                };
                self.push_route_and_switch(Route::SkillDetail {
                    directory: skill.directory.clone(),
                })
            }
            Intent::Toggle => {
                let Some(skill) = visible.get(self.skills_idx) else {
                    return Action::None;
                };
                let enabled = !skill.apps.is_enabled_for(&self.app_type);
                Action::SkillsToggle {
                    directory: skill.directory.clone(),
                    enabled,
                }
            }
            Intent::Apps => {
                let Some(skill) = visible.get(self.skills_idx) else {
                    return Action::None;
                };
                self.overlay = Overlay::SkillsAppsPicker {
                    directory: skill.directory.clone(),
                    name: skill.name.clone(),
                    selected: four_app_picker_index(&self.app_type),
                    apps: skill.apps.clone(),
                };
                Action::None
            }
            Intent::Uninstall => {
                let Some(skill) = visible.get(self.skills_idx) else {
                    return Action::None;
                };
                self.overlay = Overlay::Confirm(ConfirmOverlay {
                    title: texts::tui_skills_uninstall_title().to_string(),
                    message: texts::tui_confirm_uninstall_skill_message(
                        &skill.name,
                        &skill.directory,
                    ),
                    action: ConfirmAction::SkillsUninstall {
                        directory: skill.directory.clone(),
                    },
                });
                Action::None
            }
            Intent::Import => Action::SkillsOpenImport,
            Intent::Discover => self.push_route_and_switch(Route::SkillsDiscover),
        }
    }

    pub(crate) fn on_skills_discover_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Up => {
                self.skills_discover_idx = self.skills_discover_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                let visible = visible_skills_discover(&self.filter, &self.skills_discover_results);
                if !visible.is_empty() {
                    self.skills_discover_idx =
                        (self.skills_discover_idx + 1).min(visible.len() - 1);
                }
                Action::None
            }
            KeyCode::Char('f') => {
                self.overlay = Overlay::TextInput(TextInputState {
                    title: texts::tui_skills_discover_title().to_string(),
                    prompt: if matches!(
                        self.skills_discover_source,
                        SkillsDiscoverSource::Marketplace
                    ) {
                        texts::tui_skills_skillssh_search_prompt().to_string()
                    } else {
                        texts::tui_skills_discover_prompt().to_string()
                    },
                    input: TextInput::new(self.skills_discover_query.clone()),
                    submit: TextSubmit::SkillsDiscoverQuery,
                });
                Action::None
            }
            KeyCode::Tab => {
                self.skills_discover_source = self.skills_discover_source.toggled();
                self.skills_discover_idx = 0;
                let cache_key = (
                    self.skills_discover_source,
                    self.skills_discover_query.trim().to_lowercase(),
                );
                if let Some(skills) = self.skills_discover_cache.get(&cache_key) {
                    self.skills_discover_results = skills.clone();
                    self.skills_discover_loading = false;
                    return Action::None;
                }

                self.skills_discover_results.clear();
                self.skills_discover_loading = false;
                if matches!(self.skills_discover_source, SkillsDiscoverSource::Repos) {
                    Action::SkillsDiscover {
                        query: self.skills_discover_query.clone(),
                        source: self.skills_discover_source,
                        force: false,
                    }
                } else {
                    Action::None
                }
            }
            KeyCode::Char('r') => Action::SkillsDiscover {
                query: self.skills_discover_query.clone(),
                source: self.skills_discover_source,
                force: true,
            },
            KeyCode::Enter => {
                let visible = visible_skills_discover(&self.filter, &self.skills_discover_results);
                let Some(skill) = visible.get(self.skills_discover_idx) else {
                    return Action::None;
                };
                if skill.installed {
                    self.push_toast(texts::tui_toast_skill_already_installed(), ToastKind::Info);
                    return Action::None;
                }
                Action::SkillsInstall {
                    spec: skill.key.clone(),
                }
            }
            KeyCode::Char('e') => self.push_route_and_switch(Route::SkillsRepos),
            _ => Action::None,
        }
    }

    pub(crate) fn on_skills_repos_key(&mut self, key: KeyEvent, data: &UiData) -> Action {
        let visible = visible_skills_repos(&self.filter, data);
        match key.code {
            KeyCode::Up => {
                self.skills_repo_idx = self.skills_repo_idx.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                if !visible.is_empty() {
                    self.skills_repo_idx = (self.skills_repo_idx + 1).min(visible.len() - 1);
                }
                Action::None
            }
            KeyCode::Char('a') => {
                self.overlay = Overlay::TextInput(TextInputState {
                    title: texts::tui_skills_repos_add_title().to_string(),
                    prompt: texts::tui_skills_repos_add_prompt().to_string(),
                    input: TextInput::new(""),
                    submit: TextSubmit::SkillsRepoAdd,
                });
                Action::None
            }
            KeyCode::Char('d') => {
                let Some(repo) = visible.get(self.skills_repo_idx) else {
                    return Action::None;
                };
                self.overlay = Overlay::Confirm(ConfirmOverlay {
                    title: texts::tui_skills_repos_remove_title().to_string(),
                    message: texts::tui_confirm_remove_repo_message(&repo.owner, &repo.name),
                    action: ConfirmAction::SkillsRepoRemove {
                        owner: repo.owner.clone(),
                        name: repo.name.clone(),
                    },
                });
                Action::None
            }
            KeyCode::Char(' ') => {
                let Some(repo) = visible.get(self.skills_repo_idx) else {
                    return Action::None;
                };
                Action::SkillsRepoToggleEnabled {
                    owner: repo.owner.clone(),
                    name: repo.name.clone(),
                    enabled: !repo.enabled,
                }
            }
            _ => Action::None,
        }
    }

    pub(crate) fn on_skill_detail_key(
        &mut self,
        key: KeyEvent,
        data: &UiData,
        directory: &str,
    ) -> Action {
        let Some(skill) = data
            .skills
            .installed
            .iter()
            .find(|s| s.directory.eq_ignore_ascii_case(directory))
        else {
            return Action::None;
        };

        match key.code {
            KeyCode::Char(' ') => Action::SkillsToggle {
                directory: skill.directory.clone(),
                enabled: !skill.apps.is_enabled_for(&self.app_type),
            },
            KeyCode::Char('m') => {
                self.overlay = Overlay::SkillsAppsPicker {
                    directory: skill.directory.clone(),
                    name: skill.name.clone(),
                    selected: four_app_picker_index(&self.app_type),
                    apps: skill.apps.clone(),
                };
                Action::None
            }
            KeyCode::Char('d') => {
                self.overlay = Overlay::Confirm(ConfirmOverlay {
                    title: texts::tui_skills_uninstall_title().to_string(),
                    message: texts::tui_confirm_uninstall_skill_message(
                        &skill.name,
                        &skill.directory,
                    ),
                    action: ConfirmAction::SkillsUninstall {
                        directory: skill.directory.clone(),
                    },
                });
                Action::None
            }
            KeyCode::Char('s') => Action::SkillsSync {
                app: Some(self.app_type.clone()),
            },
            KeyCode::Char('S') => Action::SkillsSync { app: None },
            _ => Action::None,
        }
    }
}
