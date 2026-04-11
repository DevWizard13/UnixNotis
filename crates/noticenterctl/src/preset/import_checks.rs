//! Import validation helpers for hostile preset content
//!
//! These checks run before import writes anything to disk so
//! crafted bundles fail early instead of escaping through later setup steps

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use unixnotis_core::Config;

pub(super) fn validate_imported_theme_paths_stay_in_root(
    config_dir: &Path,
    config_bytes: &[u8],
) -> Result<()> {
    // The bundle config is trusted during post-import setup, so its theme targets must stay local
    let config_text =
        std::str::from_utf8(config_bytes).context("preset config.toml is not valid UTF-8")?;
    let config: Config =
        toml::from_str(config_text).context("parse bundled config.toml for import validation")?;
    // Resolve against the target config root because that is where import will later materialize CSS files
    let theme_paths = config
        .resolve_theme_paths_from(config_dir)
        .context("resolve bundled theme paths for import validation")?;

    for (slot_name, path) in [
        ("base_css", &theme_paths.base_css),
        ("panel_css", &theme_paths.panel_css),
        ("popup_css", &theme_paths.popup_css),
        ("widgets_css", &theme_paths.widgets_css),
        ("media_css", &theme_paths.media_css),
    ] {
        // `starts_with` is enough here because resolved theme paths are concrete filesystem paths now
        // Absolute or host-specific theme targets would let post-import setup escape the config root
        if !path.starts_with(config_dir) {
            return Err(anyhow!(
                "preset import requires theme.{} to stay under the config root: {}",
                slot_name,
                path.display()
            ));
        }
    }

    Ok(())
}
