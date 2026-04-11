//! Import validation helpers for hostile preset content
//!
//! These checks run before import writes anything to disk so
//! crafted bundles fail early instead of escaping through later setup steps

use anyhow::{anyhow, Context, Result};
use std::path::{Component, Path, PathBuf};
use unixnotis_core::{Config, ThemePaths};

pub(super) fn validate_imported_theme_paths_stay_in_root(
    config_dir: &Path,
    config_bytes: &[u8],
) -> Result<()> {
    // The bundle config is trusted during post-import setup, so its theme targets must stay local
    let config_text =
        std::str::from_utf8(config_bytes).context("preset config.toml is not valid UTF-8")?;
    let config: Config =
        toml::from_str(config_text).context("parse bundled config.toml for import validation")?;
    validate_config_theme_paths_stay_in_root(config_dir, &config)
}

pub(super) fn validate_config_theme_paths_stay_in_root(
    config_dir: &Path,
    config: &Config,
) -> Result<()> {
    // Resolve against the target config root because that is where import will later materialize CSS files
    let theme_paths = config
        .resolve_theme_paths_from(config_dir)
        .context("resolve bundled theme paths for import validation")?;
    validate_resolved_theme_paths_stay_in_root(config_dir, &theme_paths)
}

fn validate_resolved_theme_paths_stay_in_root(
    config_dir: &Path,
    theme_paths: &ThemePaths,
) -> Result<()> {
    // Normalize the root first so `../` tricks are compared against the real final location
    let normalized_root = normalize_lexical_path(config_dir);

    for (slot_name, path) in [
        ("base_css", &theme_paths.base_css),
        ("panel_css", &theme_paths.panel_css),
        ("popup_css", &theme_paths.popup_css),
        ("widgets_css", &theme_paths.widgets_css),
        ("media_css", &theme_paths.media_css),
    ] {
        // Normalize each target so lexical parent traversal cannot hide outside writes
        let normalized_path = normalize_lexical_path(path);
        // Absolute or host-specific theme targets would let post-import setup escape the config root
        if !normalized_path.starts_with(&normalized_root) {
            return Err(anyhow!(
                "preset import blocked because theme.{} tries to leave the UnixNotis config directory: {}",
                slot_name,
                path.display()
            ));
        }
    }

    Ok(())
}

fn normalize_lexical_path(path: &Path) -> PathBuf {
    // This stays purely lexical so it works even when the target path does not exist yet
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            // Keep the platform prefix intact on paths that use one
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            // Root anchors the normalized path before later components are folded in
            Component::RootDir => normalized.push(Path::new("/")),
            // `.` adds no meaning to the final target
            Component::CurDir => {}
            // Normal segments are preserved in order
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => match normalized.components().next_back() {
                // A normal tail can be folded away by one `..` segment
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                // Extra parents at the filesystem root stay pinned to the root
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                // Relative paths may still carry leading `..` segments at this stage
                _ => normalized.push(".."),
            },
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::validate_imported_theme_paths_stay_in_root;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn temp_root(name: &str) -> PathBuf {
        // Unique absolute paths keep these lexical checks stable under parallel cargo runs
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock moved backwards")
            .as_nanos();
        let serial = TEST_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "unixnotis-preset-import-checks-{}-{}-{}",
            name, stamp, serial
        ))
    }

    #[test]
    fn imported_theme_checks_reject_parent_traversal_targets() {
        // `../` theme paths should be treated the same as any other root escape
        let config_dir = temp_root("relative-escape");
        let config = b"[theme]\nbase_css = \"../escaped-base.css\"\npanel_css = \"panel.css\"\npopup_css = \"popup.css\"\nwidgets_css = \"widgets.css\"\nmedia_css = \"media.css\"\n";

        let error = validate_imported_theme_paths_stay_in_root(&config_dir, config)
            .expect_err("reject relative theme escape");

        assert!(error
            .to_string()
            .contains("tries to leave the UnixNotis config directory"));
    }
}
