//! Filesystem helpers for preset export and import
//!
//! This module owns the real disk work for presets:
//! walking the config tree, checking live target paths,
//! creating backup directories, and writing files safely

use anyhow::{anyhow, Context, Result};
use chrono::Local;
#[cfg(target_os = "linux")]
use rustix::fs::{mkdirat, openat2, renameat, unlinkat, AtFlags, Mode, OFlags, ResolveFlags, CWD};
use std::env;
use std::fs;
#[cfg(target_os = "linux")]
use std::io::{Read, Write};
#[cfg(target_os = "linux")]
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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

#[cfg(target_os = "linux")]
pub(super) fn open_secure_dir_all(path: &Path) -> Result<OwnedFd> {
    let mut current_fd = if path.is_absolute() {
        // Start from the real filesystem root so each later segment is opened under a stable dir fd
        openat2(
            CWD,
            "/",
            OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            secure_anchor_resolve_flags(),
        )
        .context("open filesystem root for secure directory walk")?
    } else {
        // Relative paths are anchored to the current shell directory
        openat2(
            CWD,
            ".",
            OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            secure_anchor_resolve_flags(),
        )
        .context("open current working directory for secure directory walk")?
    };

    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                return Err(anyhow!(
                    "secure directory walk does not allow parent traversal: {}",
                    path.display()
                ));
            }
            Component::Normal(part) => {
                current_fd = open_or_create_child_dir(&current_fd, part)?;
            }
        }
    }

    Ok(current_fd)
}

#[cfg(target_os = "linux")]
pub(super) fn read_relative_file_secure(
    root_dir: &OwnedFd,
    relative_path: &Path,
) -> Result<(Vec<u8>, u32)> {
    let relative_path = normalize_relative_path(relative_path)?;
    let file_fd = openat2(
        root_dir,
        &relative_path,
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
        secure_resolve_flags(),
    )
    .with_context(|| format!("open file under secure root {}", relative_path.display()))?;
    let mut file = fs::File::from(file_fd);
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect file under secure root {}", relative_path.display()))?;
    if !metadata.is_file() {
        return Err(anyhow!(
            "secure file read expected a regular file under the UnixNotis config directory: {}",
            relative_path.display()
        ));
    }

    let mut contents = Vec::new();
    file.read_to_end(&mut contents)
        .with_context(|| format!("read file under secure root {}", relative_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok((contents, metadata.permissions().mode()))
    }

    #[cfg(not(unix))]
    {
        Ok((contents, 0o644))
    }
}

#[cfg(target_os = "linux")]
pub(super) fn write_relative_file_atomic_secure(
    root_dir: &OwnedFd,
    relative_path: &Path,
    contents: &[u8],
    mode: u32,
) -> Result<()> {
    let relative_path = normalize_relative_path(relative_path)?;
    let (parent_fd, file_name) = open_or_create_parent_dir(root_dir, &relative_path)?;

    // Temp files stay in the final parent dir so the rename does not cross filesystems
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock moved backwards")
        .as_nanos();
    let temp_name = format!(".{}.{}.tmp", file_name, stamp);
    let temp_fd = openat2(
        &parent_fd,
        temp_name.as_str(),
        OFlags::WRONLY | OFlags::CLOEXEC | OFlags::CREATE | OFlags::EXCL,
        mode_from_bits(mode),
        secure_resolve_flags(),
    )
    .with_context(|| format!("create secure temp file for {}", relative_path.display()))?;
    let mut temp_file = fs::File::from(temp_fd);

    let write_result = (|| -> Result<()> {
        temp_file
            .write_all(contents)
            .with_context(|| format!("write secure temp file for {}", relative_path.display()))?;
        temp_file
            .sync_all()
            .with_context(|| format!("flush secure temp file for {}", relative_path.display()))?;
        Ok(())
    })();

    if let Err(err) = write_result {
        let _ = unlinkat(&parent_fd, temp_name.as_str(), AtFlags::empty());
        return Err(err);
    }

    renameat(
        &parent_fd,
        temp_name.as_str(),
        &parent_fd,
        file_name.as_str(),
    )
    .with_context(|| format!("replace secure target file {}", relative_path.display()))?;
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) fn remove_relative_file_secure(root_dir: &OwnedFd, relative_path: &Path) -> Result<()> {
    let relative_path = normalize_relative_path(relative_path)?;
    let (parent_fd, file_name) = open_or_create_parent_dir(root_dir, &relative_path)?;
    unlinkat(&parent_fd, file_name.as_str(), AtFlags::empty())
        .with_context(|| format!("remove secure target file {}", relative_path.display()))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn open_or_create_parent_dir(
    root_dir: &OwnedFd,
    relative_path: &Path,
) -> Result<(OwnedFd, String)> {
    let file_name = relative_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "relative file path has no file name: {}",
                relative_path.display()
            )
        })?
        .to_string();

    let mut current_fd = openat2(
        root_dir,
        ".",
        OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        secure_resolve_flags(),
    )
    .context("open secure root directory handle")?;

    if let Some(parent) = relative_path.parent() {
        for component in parent.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(anyhow!(
                        "secure parent walk does not allow this path component: {}",
                        relative_path.display()
                    ));
                }
                Component::Normal(part) => {
                    current_fd = open_or_create_child_dir(&current_fd, part)?;
                }
            }
        }
    }

    Ok((current_fd, file_name))
}

#[cfg(target_os = "linux")]
fn open_or_create_child_dir<Fd: AsFd>(parent_fd: Fd, name: &std::ffi::OsStr) -> Result<OwnedFd> {
    match openat2(
        parent_fd.as_fd(),
        name,
        OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        secure_resolve_flags(),
    ) {
        Ok(fd) => Ok(fd),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // Create the next directory segment under the already verified parent dir fd
            mkdirat(parent_fd.as_fd(), name, mode_from_bits(0o755)).with_context(|| {
                format!("create secure directory {}", Path::new(name).display())
            })?;
            openat2(
                parent_fd.as_fd(),
                name,
                OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                secure_resolve_flags(),
            )
            .with_context(|| format!("open secure directory {}", Path::new(name).display()))
        }
        Err(err) => {
            Err(err).with_context(|| format!("open secure directory {}", Path::new(name).display()))
        }
    }
}

#[cfg(target_os = "linux")]
fn secure_resolve_flags() -> ResolveFlags {
    ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS
}

#[cfg(target_os = "linux")]
fn secure_anchor_resolve_flags() -> ResolveFlags {
    ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS
}

#[cfg(target_os = "linux")]
fn mode_from_bits(mode: u32) -> Mode {
    Mode::from_raw_mode(mode)
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
    #[cfg(target_os = "linux")]
    use super::{open_secure_dir_all, write_relative_file_atomic_secure};
    use crate::preset::pathing::format_relative_path;
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

    #[cfg(target_os = "linux")]
    #[test]
    fn secure_atomic_write_replaces_existing_file() {
        // Secure writes should keep the final file in place with new contents
        let root = TempDirGuard::new("atomic");
        let target = root.path.join("scripts/run.sh");
        root.write("scripts/run.sh", "old");

        let root_fd = open_secure_dir_all(&root.path).expect("open secure root");
        write_relative_file_atomic_secure(&root_fd, Path::new("scripts/run.sh"), b"new", 0o755)
            .expect("write file");

        assert_eq!(fs::read_to_string(&target).expect("read file"), "new");
    }
}
