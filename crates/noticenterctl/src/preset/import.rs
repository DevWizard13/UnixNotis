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
    create_backup_dir, ensure_no_symlink_ancestors, ensure_safe_target_path, write_atomic_bytes,
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
        // Mirror app startup so css-check sees the same theme file set the UI would use
        let config_path = config_dir.join("config.toml");
        let config = Config::load_from_path(&config_path)
            .context("load imported config.toml before css-check")?;
        let theme_paths = config
            .resolve_theme_paths_from(&config_dir)
            .context("resolve imported theme paths before css-check")?;
        config
            .ensure_theme_files(&theme_paths)
            .context("materialize imported theme files before css-check")?;

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
    if imports_config_toml {
        // Parse the bundled config early so hostile theme paths fail before any file is written
        let bundled_config = bundle
            .files
            .iter()
            // Reuse the already validated bundle payload instead of reading from disk again
            .find(|file| file.relative_path == Path::new("config.toml"))
            .ok_or_else(|| {
                anyhow!("preset bundle is missing config.toml and cannot be imported")
            })?;
        // This closes the chain where a safe archive could still point theme files outside the root
        validate_imported_theme_paths_stay_in_root(config_dir, &bundled_config.contents)?;
    }

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

    // Real import creates the config root late so failed validation stays side-effect free
    fs::create_dir_all(config_dir)
        .with_context(|| format!("create config directory {}", config_dir.display()))?;
    let backup_dir = if overwritten > 0 {
        // Backups are only created when there is something to preserve
        Some(create_backup_dir(config_dir)?)
    } else {
        None
    };

    for item in &plan {
        if item.overwrite_existing {
            let Some(backup_dir) = backup_dir.as_ref() else {
                return Err(anyhow!("internal error: missing backup directory"));
            };
            // Backup layout mirrors the config tree so manual restore stays straightforward
            let backup_path = backup_dir.join(&item.file.relative_path);
            if let Some(parent) = backup_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create backup parent {}", parent.display()))?;
            }
            // Copy the live file first so a later write failure still leaves a rollback path
            fs::copy(&item.target_path, &backup_path).with_context(|| {
                format!(
                    "backup existing file {} -> {}",
                    item.target_path.display(),
                    backup_path.display()
                )
            })?;
        }

        // Final write is atomic so a half-written config file is not left behind
        write_atomic_bytes(&item.target_path, &item.file.contents, item.file.mode)
            .with_context(|| format!("write imported file {}", item.target_path.display()))?;
    }

    Ok(ImportSummary {
        file_count: plan.len(),
        created,
        overwritten,
        excluded,
        backup_dir,
        dry_run: false,
    })
}

fn validate_imported_theme_paths_stay_in_root(
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

        assert!(error.to_string().contains("stay under the config root"));
        assert!(!outside_theme.exists());
        assert!(!import_root.path.join("config.toml").exists());
    }
}
