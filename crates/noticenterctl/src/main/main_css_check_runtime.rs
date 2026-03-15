use std::path::Path;

use anyhow::Result;
use unixnotis_core::{Config, PANEL_RUNTIME_WIDTH_MIN};

pub(super) fn lint_runtime_config(config_dir: &Path, display_root: &str) -> Result<usize> {
    let config_path = Config::default_config_path()?;
    if !config_path.exists() {
        return Ok(0);
    }

    let config = Config::load_from_path(&config_path)?;
    let mut warnings = 0usize;

    if let Some(message) = panel_width_floor_warning(&config) {
        warnings += 1;
        eprintln!(
            "css warning: {}: {}",
            display_config_path(config_dir, display_root, &config_path),
            message
        );
    }

    Ok(warnings)
}

fn display_config_path(config_dir: &Path, display_root: &str, config_path: &Path) -> String {
    config_path
        .strip_prefix(config_dir)
        .map(|path| format!("{display_root}/{}", path.display()))
        .unwrap_or_else(|_| config_path.display().to_string())
}

pub(super) fn panel_width_floor_warning(config: &Config) -> Option<String> {
    if config.panel.width >= PANEL_RUNTIME_WIDTH_MIN {
        return None;
    }

    // Small panel widths can look ignored because the center clamps them later at runtime
    Some(format!(
        "[panel].width={} is below the runtime floor of {}; unixnotis-center will clamp it and the panel may look wider than config or css edits suggest",
        config.panel.width, PANEL_RUNTIME_WIDTH_MIN
    ))
}
