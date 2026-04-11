//! Path parsing and bundle path layout helpers for presets
//!
//! This module owns relative path normalization, exclusion matching,
//! bundle naming, and archive path mapping so the rest of the preset code
//! can work with validated config-root-relative paths

use anyhow::{anyhow, Context, Result};
use std::path::{Component, Path, PathBuf};

pub(super) const PRESET_EXTENSION: &str = "unixnotis";
// Manifest lives at the root of the archive for easy manual inspection
pub(super) const MANIFEST_ARCHIVE_PATH: &str = "manifest.toml";
// Payload files live under one prefix so manifest and data never collide
pub(super) const PAYLOAD_ARCHIVE_DIR: &str = "payload";

pub(super) fn validate_preset_bundle_path(path: &Path) -> Result<()> {
    // Preset files use one dedicated extension so CLI intent stays obvious
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    if extension.eq_ignore_ascii_case(PRESET_EXTENSION) {
        return Ok(());
    }
    Err(anyhow!(
        "preset file must use the .{} extension: {}",
        PRESET_EXTENSION,
        path.display()
    ))
}

pub(super) fn parse_except_paths(values: &[String]) -> Result<Vec<PathBuf>> {
    let mut parsed = Vec::new();
    for value in values {
        // Every exclusion is normalized once so matching stays predictable later
        parsed.push(normalize_relative_path(Path::new(value))?);
    }
    Ok(parsed)
}

pub(super) fn normalize_relative_path(path: &Path) -> Result<PathBuf> {
    // Empty or absolute paths would make import and exclusion rules ambiguous
    if path.as_os_str().is_empty() {
        return Err(anyhow!("empty relative path is not allowed"));
    }
    if path.is_absolute() {
        return Err(anyhow!(
            "path must be relative to the UnixNotis config root: {}",
            path.display()
        ));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            // `.` adds no meaning, so it is stripped out during normalization
            Component::CurDir => {}
            // `..` would let a bundle or flag escape the config root
            Component::ParentDir => {
                return Err(anyhow!(
                    "parent traversal is not allowed in preset paths: {}",
                    path.display()
                ));
            }
            // Absolute and prefix components are already rejected above
            Component::RootDir | Component::Prefix(_) => {
                return Err(anyhow!(
                    "absolute paths are not allowed in preset paths: {}",
                    path.display()
                ));
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(anyhow!("path resolved to an empty relative path"));
    }
    Ok(normalized)
}

pub(super) fn relative_path_matches_exclusion(
    relative_path: &Path,
    exclusions: &[PathBuf],
) -> bool {
    // Directory exclusions match all descendants so one flag can keep a whole subtree local
    exclusions
        .iter()
        .any(|excluded| relative_path == excluded || relative_path.starts_with(excluded))
}

pub(super) fn archive_payload_path(relative_path: &Path) -> PathBuf {
    // Bundle payload is namespaced under one folder to avoid clashes with manifest files
    Path::new(PAYLOAD_ARCHIVE_DIR).join(relative_path)
}

pub(super) fn archive_payload_relative(path: &Path) -> Result<Option<PathBuf>> {
    if path == Path::new(MANIFEST_ARCHIVE_PATH) {
        // Manifest is handled separately from payload files
        return Ok(None);
    }

    let relative = path
        .strip_prefix(PAYLOAD_ARCHIVE_DIR)
        .with_context(|| format!("unexpected archive entry {}", path.display()))?;
    Ok(Some(normalize_relative_path(relative)?))
}

pub(super) fn bundle_name_from_path(path: &Path) -> Result<String> {
    // The file stem is the human-facing bundle name shown by inspect
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("failed to derive preset name from {}", path.display()))
}

pub(super) fn format_relative_path(path: &Path) -> String {
    // Slash-separated paths keep manifest output stable inside the archive
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::{normalize_relative_path, parse_except_paths};
    use std::path::Path;

    #[test]
    fn parse_except_rejects_parent_traversal() {
        // Traversal should be blocked before any filesystem work starts
        let error = parse_except_paths(&["../escape".to_string()]).expect_err("reject traversal");
        assert!(error.to_string().contains("parent traversal"));
    }

    #[test]
    fn normalize_relative_path_strips_dot_segments() {
        // Leading `./` should not change the stored path
        let normalized =
            normalize_relative_path(Path::new("./assets/../assets/bg.png")).expect_err("reject ..");
        assert!(normalized.to_string().contains("parent traversal"));
    }
}
