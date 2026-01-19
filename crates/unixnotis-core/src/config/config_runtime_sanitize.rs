//! Runtime sanitization and validation for configuration values.

use super::{Config, PanelConfig, PopupConfig, ThemeConfig};
use crate::program_in_path;
use tracing::warn;

const MIN_REFRESH_MS: u64 = 100;
const MAX_REFRESH_MS: u64 = 60_000;
const MAX_REFRESH_SLOW_MS: u64 = 120_000;
const MAX_PANEL_WIDTH: i32 = 4096;
const MAX_PANEL_HEIGHT: i32 = 4096;
const MAX_POPUP_WIDTH: i32 = 2048;
const MAX_SPACING: i32 = 256;
const MAX_MARGIN: i32 = 512;
const MAX_CARD_HEIGHT: i32 = 2048;
// Theme guard rails keep layout values within reasonable bounds.
const MAX_BORDER_WIDTH: u8 = 16;
const MAX_CARD_RADIUS: u8 = 64;

pub(super) fn sanitize_config(config: &mut Config) {
    // Clamp refresh intervals to avoid busy loops or runaway timers.
    let fast = config
        .widgets
        .refresh_interval_ms
        .clamp(MIN_REFRESH_MS, MAX_REFRESH_MS);
    let slow = config
        .widgets
        .refresh_interval_slow_ms
        .clamp(fast, MAX_REFRESH_SLOW_MS);
    config.widgets.refresh_interval_ms = fast;
    config.widgets.refresh_interval_slow_ms = slow;

    // Normalize panel sizing; keep height 0 as "auto".
    if config.panel.width <= 0 {
        config.panel.width = PanelConfig::default().width;
    }
    config.panel.width = config.panel.width.clamp(1, MAX_PANEL_WIDTH);
    if config.panel.height < 0 {
        config.panel.height = 0;
    }
    if config.panel.height > 0 {
        config.panel.height = config.panel.height.clamp(1, MAX_PANEL_HEIGHT);
    }

    // Normalize popup sizing and spacing.
    if config.popups.width <= 0 {
        config.popups.width = PopupConfig::default().width;
    }
    config.popups.width = config.popups.width.clamp(1, MAX_POPUP_WIDTH);
    if config.popups.spacing < 0 {
        config.popups.spacing = 0;
    }
    config.popups.spacing = config.popups.spacing.clamp(0, MAX_SPACING);

    // Clamp margins to non-negative values to avoid inverted geometry.
    config.popups.margin.top = config.popups.margin.top.clamp(0, MAX_MARGIN);
    config.popups.margin.right = config.popups.margin.right.clamp(0, MAX_MARGIN);
    config.popups.margin.bottom = config.popups.margin.bottom.clamp(0, MAX_MARGIN);
    config.popups.margin.left = config.popups.margin.left.clamp(0, MAX_MARGIN);
    config.panel.margin.top = config.panel.margin.top.clamp(0, MAX_MARGIN);
    config.panel.margin.right = config.panel.margin.right.clamp(0, MAX_MARGIN);
    config.panel.margin.bottom = config.panel.margin.bottom.clamp(0, MAX_MARGIN);
    config.panel.margin.left = config.panel.margin.left.clamp(0, MAX_MARGIN);

    for stat in &mut config.widgets.stats {
        if stat.min_height < 0 {
            stat.min_height = 0;
        }
        stat.min_height = stat.min_height.clamp(0, MAX_CARD_HEIGHT);
    }
    for card in &mut config.widgets.cards {
        if card.min_height < 0 {
            card.min_height = 0;
        }
        card.min_height = card.min_height.clamp(0, MAX_CARD_HEIGHT);
    }

    let theme_defaults = ThemeConfig::default();
    clamp_alpha(&mut config.theme.surface_alpha, theme_defaults.surface_alpha);
    clamp_alpha(
        &mut config.theme.surface_strong_alpha,
        theme_defaults.surface_strong_alpha,
    );
    clamp_alpha(&mut config.theme.card_alpha, theme_defaults.card_alpha);
    clamp_alpha(
        &mut config.theme.shadow_soft_alpha,
        theme_defaults.shadow_soft_alpha,
    );
    clamp_alpha(
        &mut config.theme.shadow_strong_alpha,
        theme_defaults.shadow_strong_alpha,
    );
    // Clamp border styling to keep generated CSS within sensible bounds.
    config.theme.border_width = config.theme.border_width.min(MAX_BORDER_WIDTH);
    config.theme.card_radius = config.theme.card_radius.min(MAX_CARD_RADIUS);
    warn_missing_shell(config);
}

fn clamp_alpha(value: &mut f32, fallback: f32) {
    if !value.is_finite() {
        *value = fallback;
        return;
    }
    *value = value.clamp(0.0, 1.0);
}

fn warn_missing_shell(config: &Config) {
    if program_in_path("sh") {
        return;
    }
    if !config_requires_shell(config) {
        return;
    }
    // Shell-backed commands depend on sh being present for pipes, redirects, and control flow.
    warn!("POSIX shell (sh) not found in PATH; widget commands using pipes or redirects will fail");
}

fn config_requires_shell(config: &Config) -> bool {
    // Walk all configured commands to detect whether shell metacharacters are present.
    let volume = &config.widgets.volume;
    if command_requires_shell(&volume.get_cmd)
        || command_requires_shell(&volume.set_cmd)
        || command_requires_shell_opt(&volume.toggle_cmd)
        || command_requires_shell_opt(&volume.watch_cmd)
    {
        return true;
    }

    let brightness = &config.widgets.brightness;
    if command_requires_shell(&brightness.get_cmd)
        || command_requires_shell(&brightness.set_cmd)
        || command_requires_shell_opt(&brightness.toggle_cmd)
        || command_requires_shell_opt(&brightness.watch_cmd)
    {
        return true;
    }

    if config.widgets.toggles.iter().any(|toggle| {
        command_requires_shell_opt(&toggle.state_cmd)
            || command_requires_shell_opt(&toggle.on_cmd)
            || command_requires_shell_opt(&toggle.off_cmd)
            || command_requires_shell_opt(&toggle.watch_cmd)
    }) {
        return true;
    }

    if config
        .widgets
        .stats
        .iter()
        .any(|stat| command_requires_shell_opt(&stat.cmd))
    {
        return true;
    }

    config
        .widgets
        .cards
        .iter()
        .any(|card| command_requires_shell_opt(&card.cmd))
}

fn command_requires_shell_opt(value: &Option<String>) -> bool {
    value.as_deref().map(command_requires_shell).unwrap_or(false)
}

fn command_requires_shell(cmd: &str) -> bool {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return false;
    }
    // Strip known runtime placeholders so braces do not trigger false positives.
    let cmd = cmd.replace("{value}", "0");
    const META: [char; 15] = [
        '|', '&', ';', '<', '>', '$', '`', '(', ')', '{', '}', '[', ']', '*', '?',
    ];
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for ch in cmd.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && !in_single {
            escaped = true;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            continue;
        }
        if in_single {
            continue;
        }
        if in_double {
            // Shell expansions still apply inside double quotes for `$` and backticks.
            if ch == '$' || ch == '`' {
                return true;
            }
            continue;
        }
        if META.contains(&ch) || ch == '~' || ch == '\n' || ch == '\r' {
            return true;
        }
    }

    // Shell-style env assignments at the start need a shell to expand.
    let first = cmd.split_whitespace().next().unwrap_or_default();
    if first.starts_with('"') || first.starts_with('\'') {
        return false;
    }
    first.contains('=') && !first.starts_with('/') && !first.starts_with("./")
}
