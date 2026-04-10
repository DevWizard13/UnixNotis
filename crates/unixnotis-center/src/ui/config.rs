//! Config reload and widget rebuild logic for `UiState`.
//!
//! Keeps dynamic configuration changes isolated from event handling and
//! visibility logic.

use tracing::debug;
use unixnotis_core::{Config, PanelDebugLevel, ThemePaths};

use super::list;
use super::widget_builders::{build_extra_widgets, build_quick_controls, clear_container};
use super::{panel, UiState};

struct ReloadInputs {
    config: Config,
    theme_paths: ThemePaths,
}

impl UiState {
    pub(super) fn reload_config(&mut self) {
        let Some(reload) = self.load_reload_inputs() else {
            return;
        };
        let widgets_changed = self.config.widgets != reload.config.widgets;

        // Store the new config early so shared helpers see one consistent state
        self.config = reload.config.clone();
        debug!("config reloaded");

        self.apply_reloaded_theme(&reload);
        self.apply_reloaded_panel(&reload.config);
        // Media depends on panel geometry, so it needs the new width before widgets rebuild
        self.apply_media_config(&reload.config);
        self.apply_widget_sections_after_reload(&reload.config, widgets_changed);
        self.apply_list_config_after_reload(&reload.config);
        self.finish_reload_runtime(&reload.config);
    }

    fn load_reload_inputs(&self) -> Option<ReloadInputs> {
        let config = match Config::load_from_path(&self.config_path) {
            Ok(config) => config,
            Err(err) => {
                // Reload failures keep the old state so the live panel stays usable
                tracing::warn!(?err, "failed to reload config");
                return None;
            }
        };
        let theme_base = match Config::config_dir_for_path(&self.config_path) {
            Ok(path) => path,
            Err(err) => {
                // Theme lookup needs a stable base dir before any CSS path work starts
                tracing::warn!(?err, "failed to resolve config dir");
                return None;
            }
        };
        let theme_paths = match config.resolve_theme_paths_from(&theme_base) {
            Ok(paths) => paths,
            Err(err) => {
                // Theme path errors should not partially swap the running config
                tracing::warn!(?err, "failed to resolve theme paths");
                return None;
            }
        };

        Some(ReloadInputs {
            config,
            theme_paths,
        })
    }

    fn apply_reloaded_theme(&mut self, reload: &ReloadInputs) {
        self.css
            .update_theme(reload.theme_paths.clone(), reload.config.theme.clone());
        self.css.reload(unixnotis_ui::css::DEFAULT_CSS);
        // New theme assets may replace old cache misses, so clear the miss cache now
        self.icon_resolver.clear_missing_cache();
    }

    fn apply_reloaded_panel(&mut self, config: &Config) {
        // Geometry goes first so later sections can size themselves from the final panel width
        panel::apply_panel_config(&self.panel, config, self.work_area);
        self.log_debug(PanelDebugLevel::Info, || {
            "panel config applied after reload".to_string()
        });
    }

    fn apply_widget_sections_after_reload(&mut self, config: &Config, widgets_changed: bool) {
        if widgets_changed {
            // Widget rebuilds are the expensive part, so skip them when structure is unchanged
            self.apply_widget_config(config);
        } else {
            debug!("widget config unchanged; skipping rebuild");
        }
    }

    fn apply_list_config_after_reload(&mut self, config: &Config) {
        let list_config = list::NotificationListConfig {
            max_active: config.history.max_active,
            max_entries: config.history.max_entries,
            transient_to_history: config.history.transient_to_history,
            empty_text: config.panel.empty_text.clone(),
            empty_offset_top: config.panel.empty_offset_top,
        };
        let has_widgets = !self.widgets_collapsed && self.has_any_widgets();
        self.list.apply_config(&list_config, has_widgets);
        self.set_widgets_collapsed(self.widgets_collapsed);
    }

    fn finish_reload_runtime(&mut self, config: &Config) {
        // Refresh timers may need new intervals even when widget structure is unchanged
        self.restart_refresh_timer();
        if config.panel.respect_work_area {
            self.work_area = None;
            // Work area is refreshed after reload so compositor margins can update one more time
            super::hyprland::refresh_reserved_work_area(
                config.panel.output.clone(),
                self.event_tx.clone(),
            );
        }
    }

    fn apply_widget_config(&mut self, config: &Config) {
        // Old children are cleared first so the rebuild can treat each section as fresh state
        clear_container(&self.panel.quick_controls);
        let (volume, brightness) = build_quick_controls(&self.panel, config);
        self.volume = volume;
        self.brightness = brightness;
        clear_container(&self.panel.toggle_container);
        clear_container(&self.panel.stat_container);
        clear_container(&self.panel.card_container);
        let (toggles, stats, cards) = build_extra_widgets(&self.panel, config);
        self.toggles = toggles;
        self.stats = stats;
        self.cards = cards;
    }
}
