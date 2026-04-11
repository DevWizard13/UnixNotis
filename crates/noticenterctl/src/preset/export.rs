//! Preset export flow for the live UnixNotis config tree
//!
//! Export reads the active config root, applies explicit exclusions,
//! rejects host-specific escape paths, and writes one shareable bundle file

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use std::path::{Path, PathBuf};
use unixnotis_core::Config;

use super::archive::write_bundle;
use super::files::{
    bundle_name_from_path, collect_config_files, format_relative_path, parse_except_paths,
    validate_preset_bundle_path, validate_theme_paths_stay_in_root,
};
use super::manifest::{PresetManifest, PresetManifestFile};

#[derive(Debug)]
pub(super) struct ExportSummary {
    // Final bundle file path shown back to the CLI caller
    pub(super) bundle_path: PathBuf,
    // Count of regular files actually stored in the bundle
    pub(super) file_count: usize,
    // Symlinks are reported so the caller can clean them up if needed
    pub(super) skipped_symlinks: Vec<PathBuf>,
    // Non-regular paths are ignored because they are not portable preset content
    pub(super) skipped_non_regular: Vec<PathBuf>,
}

pub(super) fn run_export(output_path: &Path, except: &[String], force: bool) -> Result<()> {
    // Resolve the live config root exactly once for the CLI path
    let config_dir = Config::default_config_dir().context("resolve config directory")?;
    let summary = export_preset_from(&config_dir, output_path, except, force)?;

    println!(
        "preset export ok: {} file(s) -> {}",
        summary.file_count,
        summary.bundle_path.display()
    );
    if !summary.skipped_symlinks.is_empty() {
        eprintln!(
            "preset export warning: skipped {} symlink path(s)",
            summary.skipped_symlinks.len()
        );
    }
    if !summary.skipped_non_regular.is_empty() {
        eprintln!(
            "preset export warning: skipped {} non-regular path(s)",
            summary.skipped_non_regular.len()
        );
    }
    Ok(())
}

pub(super) fn export_preset_from(
    config_dir: &Path,
    output_path: &Path,
    except: &[String],
    force: bool,
) -> Result<ExportSummary> {
    // The user-facing preset extension is part of the public contract
    validate_preset_bundle_path(output_path)?;
    if !config_dir.exists() {
        return Err(anyhow!(
            "config directory not found: {}",
            config_dir.display()
        ));
    }
    if !config_dir.is_dir() {
        return Err(anyhow!(
            "config path is not a directory: {}",
            config_dir.display()
        ));
    }
    if output_path.exists() && !force {
        return Err(anyhow!(
            "preset bundle already exists (use --force to overwrite): {}",
            output_path.display()
        ));
    }

    let config_path = config_dir.join("config.toml");
    if !config_path.exists() {
        return Err(anyhow!(
            "preset export requires config.toml in {}",
            config_dir.display()
        ));
    }

    // Loading the live config up front catches broken bundles before export starts
    let config =
        Config::load_from_path(&config_path).context("load config.toml for preset export")?;
    let theme_paths = config
        .resolve_theme_paths_from(config_dir)
        .context("resolve active theme paths for preset export")?;
    // Active theme targets must stay inside the config root so the bundle is truly portable
    validate_theme_paths_stay_in_root(
        config_dir,
        &[
            ("base_css", &theme_paths.base_css),
            ("panel_css", &theme_paths.panel_css),
            ("popup_css", &theme_paths.popup_css),
            ("widgets_css", &theme_paths.widgets_css),
            ("media_css", &theme_paths.media_css),
        ],
    )?;

    let exclusions = parse_except_paths(except)?;
    if exclusions
        .iter()
        .any(|path| path == Path::new("config.toml"))
    {
        // Import depends on config.toml to describe the shared setup
        return Err(anyhow!(
            "preset export cannot exclude config.toml because the bundle would not be importable"
        ));
    }

    // File collection walks the whole config tree and filters backup dirs and excluded paths
    let collected = collect_config_files(config_dir, Some(output_path), &exclusions)?;
    if !collected
        .files
        .iter()
        .any(|file| file.relative_path == Path::new("config.toml"))
    {
        return Err(anyhow!(
            "preset export did not capture config.toml after applying exclusions"
        ));
    }
    if collected.files.is_empty() {
        return Err(anyhow!("preset export found no files to bundle"));
    }

    let manifest_files = collected
        .files
        .iter()
        .map(|file| PresetManifestFile {
            // Manifest stores slash-separated relative paths for stable cross-platform output
            path: format_relative_path(&file.relative_path),
            size: file.size,
        })
        .collect::<Vec<_>>();
    // Manifest metadata is lightweight and lets inspect work without unpacking to disk
    let manifest = PresetManifest::new(
        bundle_name_from_path(output_path)?,
        Local::now().to_rfc3339(),
        env!("CARGO_PKG_VERSION").to_string(),
        manifest_files,
    );
    write_bundle(output_path, &manifest, &collected).context("write preset bundle")?;

    Ok(ExportSummary {
        bundle_path: output_path.to_path_buf(),
        file_count: collected.files.len(),
        skipped_symlinks: collected.skipped_symlinks,
        skipped_non_regular: collected.skipped_non_regular,
    })
}

#[cfg(test)]
mod tests {
    use super::export_preset_from;
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
            // Unique temp roots keep preset tests isolated from each other
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock moved backwards")
                .as_nanos();
            let serial = TEST_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "unixnotis-preset-export-{}-{}-{}",
                name, stamp, serial
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn write(&self, relative_path: &str, contents: &str) {
            // Helper keeps test setup focused on intent instead of path plumbing
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
    fn export_builds_bundle_from_config_root() {
        // Export should pack the live config tree into one bundle file
        let root = TempDirGuard::new("bundle");
        root.write("config.toml", "[theme]\nbase_css = \"base.css\"\n");
        root.write("base.css", ".panel { color: red; }");
        root.write("assets/bg.png", "png");

        let bundle_path = root.path.join("demo.unixnotis");
        let summary = export_preset_from(&root.path, &bundle_path, &[], false).expect("export");

        assert_eq!(summary.file_count, 3);
        assert!(bundle_path.exists());
    }

    #[test]
    fn export_rejects_excluding_config_toml() {
        // config.toml is required for a usable preset
        let root = TempDirGuard::new("exclude-config");
        root.write("config.toml", "[theme]\nbase_css = \"base.css\"\n");
        root.write("base.css", ".panel { color: red; }");

        let bundle_path = root.path.join("demo.unixnotis");
        let error = export_preset_from(
            &root.path,
            &bundle_path,
            &["config.toml".to_string()],
            false,
        )
        .expect_err("reject config exclusion");

        assert!(error.to_string().contains("cannot exclude config.toml"));
    }
}
