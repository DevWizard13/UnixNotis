//! Config-root helpers for preset export and import
//!
//! This module stays focused on the live UnixNotis config tree:
//! walking files for export and filtering out internal snapshot directories

use anyhow::{anyhow, Context, Result};
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use super::pathing::{normalize_relative_path, relative_path_matches_exclusion};

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
        // Each directory is read fresh so concurrent changes surface as real IO errors
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
                // Backup roots are internal snapshots and should never be exported as live content
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

            // File size is cached once here so manifest generation does not reopen the file later
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
    use super::collect_config_files;
    use crate::preset::pathing::format_relative_path;
    use std::fs;
    use std::path::PathBuf;
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
                "unixnotis-preset-filesystem-{}-{}-{}",
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
}
