//! Path, walk, and write helpers for preset export and import
//!
//! This module owns path normalization, tree walking, backup directory naming,
//! and safe file writes so export and import can stay focused on workflow

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) const PRESET_EXTENSION: &str = "unixnotis";
// Manifest lives at the root of the archive for easy manual inspection
pub(super) const MANIFEST_ARCHIVE_PATH: &str = "manifest.toml";
// Payload files live under one prefix so manifest and data never collide
pub(super) const PAYLOAD_ARCHIVE_DIR: &str = "payload";
const BACKUP_PREFIX: &str = "Backup-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PresetFileSource {
    // Relative path as it should appear in the bundle and on import
    pub(super) relative_path: PathBuf,
    // Real on-disk source path used while export streams files into the archive
    pub(super) source_path: PathBuf,
    // Cached size goes into the manifest so later validation stays cheap
    pub(super) size: u64,
}

#[derive(Debug, Default)]
pub(super) struct CollectedConfigFiles {
    // Portable regular files that should go into the bundle
    pub(super) files: Vec<PresetFileSource>,
    // Symlinks are skipped because they do not round-trip safely across machines
    pub(super) skipped_symlinks: Vec<PathBuf>,
    // Sockets and special files are skipped for the same portability reason
    pub(super) skipped_non_regular: Vec<PathBuf>,
}

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

pub(super) fn collect_config_files(
    config_dir: &Path,
    output_path: Option<&Path>,
    exclusions: &[PathBuf],
) -> Result<CollectedConfigFiles> {
    // Canonical root keeps traversal checks stable while walking the tree
    let canonical_root = fs::canonicalize(config_dir)
        .with_context(|| format!("resolve config directory {}", config_dir.display()))?;
    let output_path = output_path.map(resolve_working_path).transpose()?;

    // A small stack keeps the walk iterative and memory usage flat
    let mut stack = vec![config_dir.to_path_buf()];
    let mut collected = CollectedConfigFiles::default();

    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir)
            .with_context(|| format!("read config directory {}", dir.display()))?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let relative = normalize_relative_path(
                path.strip_prefix(config_dir)
                    .with_context(|| format!("strip config root from {}", path.display()))?,
            )?;

            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                // Presets skip symlinks so imports never depend on host-specific links
                collected.skipped_symlinks.push(relative);
                continue;
            }

            if file_type.is_dir() {
                if is_backup_dir(&relative)
                    || relative_path_matches_exclusion(&relative, exclusions)
                {
                    continue;
                }

                // Stay inside the real config tree even if the directory moved under a bind mount
                let canonical = fs::canonicalize(&path)
                    .with_context(|| format!("resolve config subdirectory {}", path.display()))?;
                if !canonical.starts_with(&canonical_root) {
                    return Err(anyhow!(
                        "config directory contains an entry outside the config root: {}",
                        path.display()
                    ));
                }
                stack.push(path);
                continue;
            }

            if relative_path_matches_exclusion(&relative, exclusions) {
                continue;
            }

            if output_path.as_ref().is_some_and(|output| *output == path) {
                // Exporting into the config tree should not capture the bundle into itself
                continue;
            }

            if !file_type.is_file() {
                // Sockets and device nodes are not portable preset content
                collected.skipped_non_regular.push(relative);
                continue;
            }

            let metadata = entry.metadata()?;
            collected.files.push(PresetFileSource {
                relative_path: relative,
                source_path: path,
                size: metadata.len(),
            });
        }
    }

    collected
        .files
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    collected.skipped_symlinks.sort();
    collected.skipped_non_regular.sort();
    Ok(collected)
}

pub(super) fn validate_theme_paths_stay_in_root(
    config_dir: &Path,
    theme_paths: &[(&'static str, &Path)],
) -> Result<()> {
    // A shareable preset should not depend on files stored outside the config root
    for (slot_name, path) in theme_paths {
        if !path.starts_with(config_dir) {
            return Err(anyhow!(
                "preset export requires {} to live under the config root: {}",
                slot_name,
                path.display()
            ));
        }
    }
    Ok(())
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

pub(super) fn ensure_safe_target_path(config_dir: &Path, relative_path: &Path) -> Result<PathBuf> {
    let relative_path = normalize_relative_path(relative_path)?;
    let target_path = config_dir.join(&relative_path);

    // Existing symlink components could redirect writes outside the config tree
    let mut probe = config_dir.to_path_buf();
    for component in relative_path.components() {
        if let Component::Normal(part) = component {
            probe.push(part);
            if !probe.exists() {
                break;
            }

            let metadata = fs::symlink_metadata(&probe)
                .with_context(|| format!("inspect target path {}", probe.display()))?;
            if metadata.file_type().is_symlink() {
                return Err(anyhow!(
                    "preset import refuses to write through symlinked paths: {}",
                    probe.display()
                ));
            }
        }
    }

    Ok(target_path)
}

pub(super) fn create_backup_dir(config_dir: &Path) -> Result<PathBuf> {
    // Backup naming matches the installer so both tools produce familiar snapshots
    let stamp = Local::now().format("%Y-%m-%d").to_string();
    let mut candidate = config_dir.join(format!("{BACKUP_PREFIX}{stamp}"));
    let mut suffix = 1usize;
    while candidate.exists() {
        candidate = config_dir.join(format!("{BACKUP_PREFIX}{stamp}-{suffix:03}"));
        suffix += 1;
    }

    fs::create_dir_all(&candidate)
        .with_context(|| format!("create backup directory {}", candidate.display()))?;
    Ok(candidate)
}

pub(super) fn write_atomic_bytes(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    // Parents are created first so imports can restore nested asset and script trees
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("target path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create parent directory {}", parent.display()))?;

    // Temp file lives beside the target so rename stays atomic on one filesystem
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock moved backwards")
        .as_nanos();
    let temp_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("preset"),
        stamp
    ));

    fs::write(&temp_path, contents)
        .with_context(|| format!("write temp file {}", temp_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        // Script bundles need the execute bit preserved on restore
        fs::set_permissions(&temp_path, fs::Permissions::from_mode(mode))
            .with_context(|| format!("set permissions on {}", temp_path.display()))?;
    }

    // Rename replaces the old file in one step after the new bytes are fully written
    fs::rename(&temp_path, path)
        .with_context(|| format!("replace target file {}", path.display()))?;
    Ok(())
}

fn resolve_working_path(path: &Path) -> Result<PathBuf> {
    // Relative export targets are resolved from the current shell directory
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    Ok(env::current_dir()
        .context("resolve current working directory")?
        .join(path))
}

fn is_backup_dir(relative_path: &Path) -> bool {
    // Only the first path segment matters for backup dir detection
    relative_path
        .components()
        .next()
        .and_then(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .map(|name| name.starts_with(BACKUP_PREFIX))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{
        collect_config_files, format_relative_path, normalize_relative_path, parse_except_paths,
        write_atomic_bytes,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempDirGuard {
        path: PathBuf,
    }

    impl TempDirGuard {
        fn new(name: &str) -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock moved backwards")
                .as_nanos();
            let serial = TEST_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "unixnotis-preset-files-{}-{}-{}",
                name, stamp, serial
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn write(&self, relative_path: &str, contents: &str) {
            let path = self.path.join(relative_path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent dirs");
            }
            fs::write(path, contents).expect("write file");
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn parse_except_rejects_parent_traversal() {
        // Traversal should be blocked before any filesystem work starts
        let error = parse_except_paths(&["../escape".to_string()]).expect_err("reject traversal");
        assert!(error.to_string().contains("parent traversal"));
    }

    #[test]
    fn collect_config_files_skips_backups_symlinks_and_output_file() {
        // Export should keep the tree portable and avoid self-inclusion
        let root = TempDirGuard::new("collect");
        root.write("config.toml", "demo = true");
        root.write("assets/bg.png", "png");
        root.write("Backup-2026-04-11/config.toml", "old");
        root.write("scripts/run.sh", "echo hi");
        root.write("bundle.unixnotis", "old bundle");
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            root.path.join("assets/bg.png"),
            root.path.join("linked.png"),
        )
        .expect("create symlink");

        let collected = collect_config_files(
            &root.path,
            Some(&root.path.join("bundle.unixnotis")),
            &[PathBuf::from("scripts")],
        )
        .expect("collect files");

        let paths = collected
            .files
            .iter()
            .map(|file| format_relative_path(&file.relative_path))
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["assets/bg.png", "config.toml"]);
        #[cfg(unix)]
        assert_eq!(
            collected
                .skipped_symlinks
                .iter()
                .map(|path| format_relative_path(path))
                .collect::<Vec<_>>(),
            vec!["linked.png"]
        );
    }

    #[test]
    fn normalize_relative_path_strips_dot_segments() {
        // Leading `./` should not change the stored path
        let normalized =
            normalize_relative_path(Path::new("./assets/../assets/bg.png")).expect_err("reject ..");
        assert!(normalized.to_string().contains("parent traversal"));
    }

    #[test]
    fn write_atomic_bytes_replaces_existing_file() {
        // Atomic writes should leave the final file in place with new contents
        let root = TempDirGuard::new("atomic");
        let target = root.path.join("scripts/run.sh");
        root.write("scripts/run.sh", "old");

        write_atomic_bytes(&target, b"new", 0o755).expect("write file");

        assert_eq!(fs::read_to_string(&target).expect("read file"), "new");
    }
}
