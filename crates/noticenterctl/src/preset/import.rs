//! Preset import flow for applying a bundle into the live config tree
//!
//! Import validates the bundle first, builds a write plan, optionally reports it,
//! then backs up only the files that will actually be overwritten

use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use unixnotis_core::Config;

use crate::main_css_check::run_css_check;

use super::archive::{read_bundle, BundleFile};
use super::filesystem::{
    create_backup_dir, open_secure_dir_all, read_relative_file_secure, remove_relative_file_secure,
    write_relative_file_atomic_secure,
};
use super::filesystem_checks::{
    ensure_dir_fd_matches_live_path, ensure_no_symlink_ancestors, ensure_safe_target_path,
};
use super::import_checks::{
    validate_config_theme_paths_stay_in_root, validate_imported_theme_paths_stay_in_root,
};
use super::pathing::{
    parse_except_paths, relative_path_matches_exclusion, resolve_cli_bundle_path,
    validate_preset_bundle_path,
};

#[derive(Debug)]
pub(super) struct ImportSummary {
    // Number of files that will be or were applied from the bundle
    pub(super) file_count: usize,
    // Files that did not exist locally before import
    pub(super) created: usize,
    // Files that already existed and needed a backup first
    pub(super) overwritten: usize,
    // Bundle files intentionally left untouched because of --except
    pub(super) excluded: usize,
    // Backup directory is present only when an overwrite happened
    pub(super) backup_dir: Option<PathBuf>,
    // Dry-run keeps the same output shape without touching the filesystem
    pub(super) dry_run: bool,
}

#[derive(Debug)]
struct ImportPlanItem {
    // Bundle file contents plus its relative target path
    file: BundleFile,
    // Final on-disk write location inside the config root
    target_path: PathBuf,
    // Tracks whether a backup copy is needed before writing
    overwrite_existing: bool,
}

#[derive(Debug)]
struct AppliedImportItem {
    // Relative path written through the secure config-root fd
    relative_path: PathBuf,
    // Original file bytes are kept only for overwrite cases so rollback can restore them
    previous_contents: Option<Vec<u8>>,
    // Original mode is needed when rollback restores an overwritten file
    previous_mode: Option<u32>,
}

pub(super) fn run_import(input_path: &Path, except: &[String], dry_run: bool) -> Result<()> {
    // Resolve the live config root once for the CLI path
    let config_dir = Config::default_config_dir().context("resolve config directory")?;
    // CLI import accepts a missing extension and can append it after confirmation
    let input_path = resolve_cli_bundle_path(input_path)?;
    let summary = import_preset_into(&config_dir, &input_path, except, dry_run)?;

    println!(
        "preset import {}: {} file(s), {} created, {} overwritten, {} excluded",
        if summary.dry_run { "dry-run ok" } else { "ok" },
        summary.file_count,
        summary.created,
        summary.overwritten,
        summary.excluded
    );
    if let Some(backup_dir) = summary.backup_dir {
        println!("preset import backup: {}", backup_dir.display());
    }

    if !summary.dry_run {
        // Reload the active config after import so css-check validates the setup that was just applied
        let config_path = config_dir.join("config.toml");
        let config = Config::load_from_path(&config_path)
            .context("load imported config.toml before css-check")?;
        // Recheck the live config so `--except config.toml` cannot reuse an unsafe local theme path
        validate_config_theme_paths_stay_in_root(&config_dir, &config)?;

        // Imported presets should be checked right away so broken shared CSS is obvious
        println!("preset import check: running css-check on imported theme files");
        if let Err(err) = run_css_check() {
            // Import already wrote files, so this reports post-apply validation failure instead of rollback
            return Err(anyhow!(
                "preset import completed, but css-check failed after import: {err}"
            ));
        }
    }

    Ok(())
}

pub(super) fn import_preset_into(
    config_dir: &Path,
    input_path: &Path,
    except: &[String],
    dry_run: bool,
) -> Result<ImportSummary> {
    validate_preset_bundle_path(input_path)?;
    // The whole config-root path must be free of symlink hops before any write plan is built
    ensure_no_symlink_ancestors(config_dir)?;

    let exclusions = parse_except_paths(except)?;
    // A kept-local config.toml means the bundle config never drives post-import theme setup
    let imports_config_toml =
        !relative_path_matches_exclusion(Path::new("config.toml"), &exclusions);
    // Read and validate the full bundle before touching the local config tree
    let bundle = read_bundle(input_path).context("read preset bundle for import")?;

    if !bundle
        .files
        .iter()
        .any(|file| file.relative_path == Path::new("config.toml"))
    {
        return Err(anyhow!(
            "preset bundle is missing config.toml and cannot be imported"
        ));
    }
    // Import should validate the config that will actually drive post-import theme setup
    let effective_config_bytes = if imports_config_toml {
        let bundled_config = bundle
            .files
            .iter()
            // Reuse the already validated bundle payload instead of reading from disk again
            .find(|file| file.relative_path == Path::new("config.toml"))
            .ok_or_else(|| {
                anyhow!("preset bundle is missing config.toml and cannot be imported")
            })?;
        bundled_config.contents.clone()
    } else {
        let local_config_path = config_dir.join("config.toml");
        // Keeping the local config means its theme paths still control the later css-check setup
        fs::read(&local_config_path).with_context(|| {
            format!(
                "read existing config.toml kept by --except from {}",
                local_config_path.display()
            )
        })?
    };
    // This closes both bundled and kept-local config chains before any file is written
    validate_imported_theme_paths_stay_in_root(config_dir, &effective_config_bytes)?;

    let mut excluded = 0usize;
    let mut plan = Vec::new();
    for file in bundle.files {
        // Import exclusions keep selected local files as-is even when the bundle carries them
        if relative_path_matches_exclusion(&file.relative_path, &exclusions) {
            excluded += 1;
            continue;
        }

        // Safe path resolution blocks traversal and symlink write-through
        let target_path = ensure_safe_target_path(config_dir, &file.relative_path)?;
        let overwrite_existing = target_path.exists();
        if overwrite_existing {
            let metadata = fs::symlink_metadata(&target_path)
                .with_context(|| format!("inspect existing target {}", target_path.display()))?;
            if !metadata.is_file() {
                return Err(anyhow!(
                    "preset import refuses to overwrite a non-file path: {}",
                    target_path.display()
                ));
            }
        }

        plan.push(ImportPlanItem {
            file,
            target_path,
            overwrite_existing,
        });
    }

    let overwritten = plan.iter().filter(|item| item.overwrite_existing).count();
    let created = plan.len().saturating_sub(overwritten);

    if dry_run {
        // Dry-run reports the exact write plan without creating backups or files
        return Ok(ImportSummary {
            file_count: plan.len(),
            created,
            overwritten,
            excluded,
            backup_dir: None,
            dry_run: true,
        });
    }

    // Real import opens the config root through a directory fd so later writes stay inside it
    let config_root_fd = open_secure_dir_all(config_dir)
        .with_context(|| format!("open secure config directory {}", config_dir.display()))?;
    let backup_dir = if overwritten > 0 {
        // Backups are only created when there is something to preserve
        Some(create_backup_dir(config_dir)?)
    } else {
        None
    };
    let backup_root_fd =
        if let Some(backup_dir) = backup_dir.as_ref() {
            Some(open_secure_dir_all(backup_dir).with_context(|| {
                format!("open secure backup directory {}", backup_dir.display())
            })?)
        } else {
            None
        };
    let mut applied_items = Vec::new();

    for item in &plan {
        // Stop if the visible config root changed after the secure fd was opened
        ensure_live_config_root_or_rollback(&config_root_fd, config_dir, &applied_items)?;

        let mut previous_contents = None;
        let mut previous_mode = None;

        if item.overwrite_existing {
            let Some(backup_dir) = backup_dir.as_ref() else {
                return Err(anyhow!("internal error: missing backup directory"));
            };
            let Some(backup_root_fd) = backup_root_fd.as_ref() else {
                return Err(anyhow!("internal error: missing secure backup directory"));
            };
            // Read and rewrite through secure dir fds so a late symlink swap cannot redirect backups
            let (existing_bytes, existing_mode) =
                read_relative_file_secure(&config_root_fd, &item.file.relative_path).with_context(
                    || {
                        format!(
                            "read existing imported file {}",
                            item.file.relative_path.display()
                        )
                    },
                )?;
            previous_contents = Some(existing_bytes.clone());
            previous_mode = Some(existing_mode);
            let backup_path = backup_dir.join(&item.file.relative_path);
            write_relative_file_atomic_secure(
                backup_root_fd,
                &item.file.relative_path,
                &existing_bytes,
                existing_mode,
            )
            .with_context(|| format!("write backup file {}", backup_path.display()))?;
        }

        // Final writes go through the secure root fd so a symlink planted mid-import cannot win the race
        write_relative_file_atomic_secure(
            &config_root_fd,
            &item.file.relative_path,
            &item.file.contents,
            item.file.mode,
        )
        .with_context(|| format!("write imported file {}", item.target_path.display()))?;

        applied_items.push(AppliedImportItem {
            relative_path: item.file.relative_path.clone(),
            previous_contents,
            previous_mode,
        });
    }

    // Catch a root-dir move that happened after the last write but before import returned
    ensure_live_config_root_or_rollback(&config_root_fd, config_dir, &applied_items)?;

    Ok(ImportSummary {
        file_count: plan.len(),
        created,
        overwritten,
        excluded,
        backup_dir,
        dry_run: false,
    })
}

fn ensure_live_config_root_or_rollback(
    config_root_fd: &std::os::fd::OwnedFd,
    config_dir: &Path,
    applied_items: &[AppliedImportItem],
) -> Result<()> {
    if let Err(err) = ensure_dir_fd_matches_live_path(config_root_fd, config_dir) {
        rollback_applied_import_items(config_root_fd, applied_items)?;
        return Err(err);
    }
    Ok(())
}

fn rollback_applied_import_items(
    config_root_fd: &std::os::fd::OwnedFd,
    applied_items: &[AppliedImportItem],
) -> Result<()> {
    for item in applied_items.iter().rev() {
        if let (Some(previous_contents), Some(previous_mode)) =
            (item.previous_contents.as_ref(), item.previous_mode)
        {
            // Overwrites are restored byte-for-byte so a failed import does not leave drift behind
            write_relative_file_atomic_secure(
                config_root_fd,
                &item.relative_path,
                previous_contents,
                previous_mode,
            )
            .with_context(|| format!("restore imported file {}", item.relative_path.display()))?;
        } else {
            // Newly created files are simply removed during rollback
            remove_relative_file_secure(config_root_fd, &item.relative_path).with_context(
                || format!("remove imported file {}", item.relative_path.display()),
            )?;
        }
    }

    Ok(())
}
#[cfg(test)]
mod tests {
    use super::import_preset_into;
    use crate::preset::archive::write_bundle;
    use crate::preset::export::export_preset_from;
    use crate::preset::filesystem::{CollectedConfigFiles, PresetFileSource};
    use crate::preset::manifest::{PresetManifest, PresetManifestFile};
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
            // Unique temp roots keep import tests isolated from the real config tree
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock moved backwards")
                .as_nanos();
            let serial = TEST_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "unixnotis-preset-import-{}-{}-{}",
                name, stamp, serial
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn write(&self, relative_path: &str, contents: &str) {
            // Helper keeps test setup compact when building fake config roots
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
    fn import_dry_run_reports_create_and_overwrite_counts() {
        // Dry-run should plan writes without changing the target tree
        let export_root = TempDirGuard::new("dry-run-export");
        export_root.write("config.toml", "[theme]\nbase_css = \"base.css\"\n");
        export_root.write("base.css", ".a { color: red; }");
        let bundle_path = export_root.path.join("demo.unixnotis");
        export_preset_from(&export_root.path, &bundle_path, &[], false).expect("export bundle");

        let import_root = TempDirGuard::new("dry-run-import");
        import_root.write("config.toml", "old = true");
        let summary = import_preset_into(
            &import_root.path,
            &bundle_path,
            &["base.css".to_string()],
            true,
        )
        .expect("dry-run import");

        assert_eq!(summary.file_count, 1);
        assert_eq!(summary.created, 0);
        assert_eq!(summary.overwritten, 1);
        assert_eq!(summary.excluded, 1);
        assert_eq!(
            fs::read_to_string(import_root.path.join("config.toml")).expect("read config"),
            "old = true"
        );
    }

    #[test]
    fn import_writes_files_and_creates_backup_for_overwrites() {
        // Real import should overwrite live files and keep a rollback copy
        let export_root = TempDirGuard::new("real-export");
        export_root.write("config.toml", "[theme]\nbase_css = \"base.css\"\n");
        export_root.write("base.css", ".a { color: red; }");
        let bundle_path = export_root.path.join("demo.unixnotis");
        export_preset_from(&export_root.path, &bundle_path, &[], false).expect("export bundle");

        let import_root = TempDirGuard::new("real-import");
        import_root.write("config.toml", "old = true");
        let summary =
            import_preset_into(&import_root.path, &bundle_path, &[], false).expect("import");

        assert_eq!(summary.file_count, 2);
        assert_eq!(summary.created, 1);
        assert_eq!(summary.overwritten, 1);
        let backup_dir = summary.backup_dir.expect("backup dir");
        assert!(backup_dir.join("config.toml").exists());
        assert_eq!(
            fs::read_to_string(import_root.path.join("config.toml")).expect("read config"),
            "[theme]\nbase_css = \"base.css\"\n"
        );
    }

    #[test]
    fn import_rejects_bundled_absolute_theme_paths_before_writing() {
        // A hostile config should not make post-import setup create files outside the config root
        let export_root = TempDirGuard::new("absolute-theme-export");
        let outside_theme = export_root.path.join("outside-theme.css");
        export_root.write(
            "config.toml",
            &format!(
                // The bundle looks normal on the surface, but one theme slot points outside the root
                "[theme]\nbase_css = {:?}\npanel_css = \"panel.css\"\npopup_css = \"popup.css\"\nwidgets_css = \"widgets.css\"\nmedia_css = \"media.css\"\n",
                outside_theme.display().to_string()
            ),
        );
        export_root.write("panel.css", ".panel { color: red; }");
        export_root.write("popup.css", ".popup { color: blue; }");
        export_root.write("widgets.css", ".widgets { color: green; }");
        export_root.write("media.css", ".media { color: yellow; }");
        let bundle_path = export_root.path.join("demo.unixnotis");
        let collected = CollectedConfigFiles {
            files: [
                ("config.toml", "config.toml"),
                ("panel.css", "panel.css"),
                ("popup.css", "popup.css"),
                ("widgets.css", "widgets.css"),
                ("media.css", "media.css"),
            ]
            .into_iter()
            .map(|(relative_path, source_path)| {
                let source_path = export_root.path.join(source_path);
                PresetFileSource {
                    // Build the archive by hand so export-side root checks do not hide the import bug
                    relative_path: PathBuf::from(relative_path),
                    size: fs::metadata(&source_path).expect("metadata").len(),
                    source_path,
                }
            })
            .collect(),
            ..Default::default()
        };
        let manifest = PresetManifest::new(
            "demo".to_string(),
            "2026-04-11T00:00:00Z".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
            collected
                .files
                .iter()
                .map(|file| PresetManifestFile {
                    path: file.relative_path.display().to_string(),
                    size: file.size,
                })
                .collect(),
        );
        write_bundle(&bundle_path, &manifest, &collected).expect("write bundle");
        let import_root = TempDirGuard::new("absolute-theme-import");

        let error = import_preset_into(&import_root.path, &bundle_path, &[], false)
            .expect_err("reject absolute bundled theme path");

        assert!(error
            .to_string()
            .contains("tries to leave the UnixNotis config directory"));
        assert!(!outside_theme.exists());
        assert!(!import_root.path.join("config.toml").exists());
    }

    #[test]
    fn import_rejects_kept_local_config_that_points_outside_root() {
        // Keeping the local config should not reopen the later theme-materialization escape path
        let export_root = TempDirGuard::new("kept-local-config-export");
        export_root.write("config.toml", "[theme]\nbase_css = \"base.css\"\n");
        export_root.write("base.css", ".a { color: red; }");
        let bundle_path = export_root.path.join("demo.unixnotis");
        export_preset_from(&export_root.path, &bundle_path, &[], false).expect("export bundle");

        let import_root = TempDirGuard::new("kept-local-config-import");
        let outside_theme = import_root.path.with_file_name("outside-theme.css");
        import_root.write(
            "config.toml",
            &format!(
                "[theme]\nbase_css = {:?}\npanel_css = \"panel.css\"\npopup_css = \"popup.css\"\nwidgets_css = \"widgets.css\"\nmedia_css = \"media.css\"\n",
                outside_theme.display().to_string()
            ),
        );

        let error = import_preset_into(
            &import_root.path,
            &bundle_path,
            &["config.toml".to_string()],
            false,
        )
        .expect_err("reject kept local config escape");

        assert!(error
            .to_string()
            .contains("tries to leave the UnixNotis config directory"));
        assert!(!outside_theme.exists());
        assert!(!import_root.path.join("base.css").exists());
    }
}
