//! Config and theme file creation/reset logic.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use unixnotis_core::Config;

use crate::paths::format_with_home;
use unixnotis_core::util;

use super::actions_config_backup::{
    backup_existing_file, create_backup_dir, ensure_installer_config, load_installer_config,
    write_atomic,
};
use super::{log_line, ActionContext};

const DND_STATE_FILE: &str = "state.json";
pub fn ensure_config(ctx: &mut ActionContext) -> Result<()> {
    let config = Config::default();
    let config_dir = Config::default_config_dir().map_err(|err| anyhow!(err.to_string()))?;
    let config_path = Config::default_config_path().map_err(|err| anyhow!(err.to_string()))?;
    log_line(
        ctx,
        format!("Config directory: {}", format_with_home(&config_dir)),
    );

    // Ensure the config directory exists before creating defaults.
    fs::create_dir_all(&config_dir).with_context(|| "failed to create config directory")?;

    if config_path.exists() {
        log_line(
            ctx,
            format!("Config file present: {}", format_with_home(&config_path)),
        );
    } else {
        // Write a default config.toml when missing so users have a base to edit.
        let config_toml = render_default_config_toml(&config)?;
        write_atomic(&config_path, &config_toml).with_context(|| "failed to write config.toml")?;
        log_line(
            ctx,
            format!("Config file created: {}", format_with_home(&config_path)),
        );
    }

    ensure_installer_config(ctx, &config_dir)?;

    let theme_paths = config
        .resolve_theme_paths()
        .map_err(|err| anyhow!(err.to_string()))?;

    let theme_entries = [
        ("base.css", &theme_paths.base_css),
        ("panel.css", &theme_paths.panel_css),
        ("popup.css", &theme_paths.popup_css),
        ("widgets.css", &theme_paths.widgets_css),
        ("media.css", &theme_paths.media_css),
    ];

    let pre_existing = theme_entries
        .iter()
        .map(|(_, path)| path.exists())
        .collect::<Vec<_>>();

    config
        .ensure_theme_files(&theme_paths)
        .map_err(|err| anyhow!(err.to_string()))?;

    for ((name, path), existed) in theme_entries.iter().zip(pre_existing.iter()) {
        let status = if *existed { "present" } else { "created" };
        log_line(
            ctx,
            format!(
                "Theme file {}: {} ({})",
                name,
                status,
                format_with_home(path)
            ),
        );
    }

    Ok(())
}

pub fn reset_config(ctx: &mut ActionContext) -> Result<()> {
    let config = Config::default();
    let config_dir = Config::default_config_dir().map_err(|err| anyhow!(err.to_string()))?;
    let config_path = Config::default_config_path().map_err(|err| anyhow!(err.to_string()))?;

    fs::create_dir_all(&config_dir).with_context(|| "failed to create config directory")?;

    ensure_installer_config(ctx, &config_dir)?;

    let installer_config = load_installer_config(&config_dir, ctx);
    let backup_dir = create_backup_dir(ctx, &config_dir, installer_config.backups.keep)?;

    // Preserve existing config before overwriting so customizations are recoverable.
    backup_existing_file(ctx, &config_path, "config.toml", backup_dir.as_deref())?;

    let config_toml = render_default_config_toml(&config)?;
    write_atomic(&config_path, &config_toml).with_context(|| "failed to write config.toml")?;

    log_line(
        ctx,
        format!(
            "Reset config file to defaults: {}",
            format_with_home(&config_path)
        ),
    );

    let theme_paths = config
        .resolve_theme_paths()
        .map_err(|err| anyhow!(err.to_string()))?;

    // Backup theme files before reset to avoid accidental loss of user styling.
    backup_existing_file(
        ctx,
        &theme_paths.base_css,
        "base.css",
        backup_dir.as_deref(),
    )?;
    backup_existing_file(
        ctx,
        &theme_paths.panel_css,
        "panel.css",
        backup_dir.as_deref(),
    )?;
    backup_existing_file(
        ctx,
        &theme_paths.popup_css,
        "popup.css",
        backup_dir.as_deref(),
    )?;
    backup_existing_file(
        ctx,
        &theme_paths.widgets_css,
        "widgets.css",
        backup_dir.as_deref(),
    )?;
    backup_existing_file(
        ctx,
        &theme_paths.media_css,
        "media.css",
        backup_dir.as_deref(),
    )?;

    write_atomic(&theme_paths.base_css, unixnotis_core::DEFAULT_BASE_CSS)
        .with_context(|| "failed to write base.css")?;
    write_atomic(&theme_paths.panel_css, unixnotis_core::DEFAULT_PANEL_CSS)
        .with_context(|| "failed to write panel.css")?;
    write_atomic(&theme_paths.popup_css, unixnotis_core::DEFAULT_POPUP_CSS)
        .with_context(|| "failed to write popup.css")?;
    write_atomic(
        &theme_paths.widgets_css,
        unixnotis_core::DEFAULT_WIDGETS_CSS,
    )
    .with_context(|| "failed to write widgets.css")?;
    write_atomic(&theme_paths.media_css, unixnotis_core::DEFAULT_MEDIA_CSS)
        .with_context(|| "failed to write media.css")?;

    log_line(
        ctx,
        format!("Reset theme files in {}", format_with_home(&config_dir)),
    );

    Ok(())
}

fn render_default_config_toml(config: &Config) -> Result<String> {
    let mut config_toml = toml::to_string_pretty(config).map_err(|err| anyhow!(err.to_string()))?;
    let panel_height_line = format!("height = {}\n", config.panel.height);
    let panel_height_block = format!(
        "# Vertical size as a percent of usable monitor height after margins\n\
# and reserved work area\n\
height = {}\n\
\n\
# Exact pixel height override for advanced users\n\
# height_override = 1487\n",
        config.panel.height
    );

    if !config_toml.contains(&panel_height_line) {
        return Err(anyhow!("default config template missing panel height line"));
    }

    config_toml = config_toml.replacen(&panel_height_line, &panel_height_block, 1);
    Ok(config_toml)
}

pub fn remove_state(ctx: &mut ActionContext) -> Result<()> {
    let Some(state_dir) = resolve_state_dir() else {
        log_line(ctx, "State directory not resolved; skipping state cleanup.");
        return Ok(());
    };

    let state_root = state_dir.join("unixnotis");
    let outcome = match remove_state_file(&state_root) {
        Ok(outcome) => outcome,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            log_line(ctx, "State file not present; nothing to remove.");
            return Ok(());
        }
        Err(err) => return Err(err).with_context(|| "failed to remove state file"),
    };

    if outcome.removed_file {
        let path = state_root.join(DND_STATE_FILE);
        log_line(
            ctx,
            format!(
                "Removed persisted state file: {}",
                format_with_state_env(&path)
            ),
        );
    }

    if outcome.removed_dir {
        log_line(
            ctx,
            format!(
                "Removed empty state directory: {}",
                format_with_state_env(&state_root)
            ),
        );
    }

    Ok(())
}

#[derive(Debug, Default)]
struct RemoveStateOutcome {
    removed_file: bool,
    removed_dir: bool,
}

fn remove_state_file(state_root: &Path) -> std::io::Result<RemoveStateOutcome> {
    let state_file = state_root.join(DND_STATE_FILE);
    let removed_file = match fs::remove_file(&state_file) {
        Ok(()) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(err),
    };

    if !removed_file {
        return Ok(RemoveStateOutcome::default());
    }

    let removed_dir =
        is_dir_empty(state_root).unwrap_or(false) && fs::remove_dir(state_root).is_ok();

    Ok(RemoveStateOutcome {
        removed_file,
        removed_dir,
    })
}

fn is_dir_empty(path: &Path) -> std::io::Result<bool> {
    let mut entries = fs::read_dir(path)?;
    Ok(entries.next().is_none())
}

fn resolve_state_dir() -> Option<PathBuf> {
    util::resolve_state_dir()
}

fn format_with_state_env(path: &Path) -> String {
    // Prefer XDG_STATE_HOME for display when available to avoid absolute paths in logs.
    if let Ok(state_home) = std::env::var("XDG_STATE_HOME") {
        if !state_home.trim().is_empty() {
            let state_root = PathBuf::from(state_home);
            if let Ok(stripped) = path.strip_prefix(&state_root) {
                let mut rendered = PathBuf::from("$XDG_STATE_HOME");
                rendered.push(stripped);
                return rendered.display().to_string();
            }
        }
    }

    format_with_home(path)
}

#[cfg(test)]
#[path = "actions_config_tests.rs"]
mod tests;
