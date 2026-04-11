//! Preset export flow for the live UnixNotis config tree
//!
//! Export reads the active config root, applies explicit exclusions,
//! rejects host-specific escape paths, and writes one shareable bundle file

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use unixnotis_core::Config;

use super::archive::write_bundle;
use super::command_paths::{
    rewrite_host_specific_command_paths, validate_config_command_paths_stay_in_root,
    HostSpecificCommandPath,
};
use super::config_root::{collect_config_files, override_collected_file_contents};
use super::css_asset_refs::{collect_external_css_asset_refs_from_sources, ExternalCssAssetRef};
use super::manifest::{PresetManifest, PresetManifestFile};
use super::pathing::{
    bundle_name_from_path, confirm_continue_or_abort, format_relative_path, normalize_lexical_path,
    parse_except_paths, resolve_cli_bundle_path, validate_preset_bundle_path,
};

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
    // CLI export accepts a missing extension and can append it after confirmation
    let output_path = resolve_cli_bundle_path(output_path)?;
    let summary = export_preset_from(&config_dir, &output_path, except, force)?;

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
    // The shared helper keeps the real prompt path and the test path on the same export logic
    export_preset_from_with_confirm(
        config_dir,
        output_path,
        except,
        force,
        confirm_export_external_css_refs,
        prompt_to_fix_host_specific_command_paths,
    )
}

fn export_preset_from_with_confirm<F, G>(
    config_dir: &Path,
    output_path: &Path,
    except: &[String],
    force: bool,
    confirm_external_css_refs: F,
    prompt_fix_host_specific_command_paths: G,
) -> Result<ExportSummary>
where
    F: FnOnce(&[ExternalCssAssetRef]) -> Result<()>,
    G: FnOnce(&[HostSpecificCommandPath]) -> Result<bool>,
{
    // Tests can inject a fixed answer here so they do not depend on terminal state
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
    let mut config =
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
    // Shared presets should not ship explicit command paths that depend on outside host files
    validate_config_command_paths_stay_in_root(
        config_dir,
        &config,
        "preset export requires explicit command paths to stay under the config root",
    )?;
    // Absolute command paths under the config root still leak the local machine layout into the preset
    let leaked_command_paths = rewrite_host_specific_command_paths_if_requested(
        config_dir,
        &mut config,
        prompt_fix_host_specific_command_paths,
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
    let mut collected = collect_config_files(config_dir, Some(output_path), &exclusions)?;
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

    if !leaked_command_paths.is_empty() {
        // Only the bundled config is rewritten so the live config tree stays untouched
        let config_bytes = toml::to_string_pretty(&config)
            .context("encode fixed config.toml for preset export")?
            .into_bytes();
        override_collected_file_contents(&mut collected, Path::new("config.toml"), config_bytes)?;
        eprintln!(
            "preset export note: rewrote {} host-specific command path(s) in the bundled config.toml",
            leaked_command_paths.len()
        );
    }

    // Warn before writing the bundle when shared CSS depends on outside assets
    let external_css_refs =
        collect_external_css_asset_refs_from_sources(config_dir, &collected.files)?;
    confirm_external_css_refs(&external_css_refs)?;

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

fn confirm_export_external_css_refs(external_refs: &[ExternalCssAssetRef]) -> Result<()> {
    if external_refs.is_empty() {
        return Ok(());
    }

    // The warning prints first so the caller can see exactly which CSS file caused the prompt
    let details = format_external_css_ref_lines(external_refs);
    eprintln!(
        "preset export warning: found {} CSS asset reference(s) outside the UnixNotis config directory",
        external_refs.len()
    );
    for line in &details {
        eprintln!("{line}");
    }

    confirm_continue_or_abort(
        "External CSS asset references were found; continue exporting anyway?",
        &format!(
            "preset export found CSS asset references outside the UnixNotis config directory; rerun interactively to confirm anyway\n{}",
            details.join("\n")
        ),
    )
}

fn format_external_css_ref_lines(external_refs: &[ExternalCssAssetRef]) -> Vec<String> {
    external_refs
        .iter()
        .map(|asset_ref| {
            // One line per asset keeps warning output readable when several files are involved
            format!(
                "  - {} -> {} ({})",
                asset_ref.css_file.display(),
                asset_ref.asset_ref,
                asset_ref.reason
            )
        })
        .collect()
}

fn rewrite_host_specific_command_paths_if_requested<G>(
    config_dir: &Path,
    config: &mut Config,
    prompt_fix_host_specific_command_paths: G,
) -> Result<Vec<HostSpecificCommandPath>>
where
    G: FnOnce(&[HostSpecificCommandPath]) -> Result<bool>,
{
    let leaked_paths = super::command_paths::collect_host_specific_command_paths(config_dir, config);
    if leaked_paths.is_empty() {
        return Ok(Vec::new());
    }

    let details = format_host_specific_command_path_lines(&leaked_paths);
    eprintln!(
        "preset export warning: found {} host-specific command path(s) under the UnixNotis config directory",
        leaked_paths.len()
    );
    for line in &details {
        eprintln!("{line}");
    }

    // Declining the helper keeps the bundle valid, but the warning stays visible
    if !prompt_fix_host_specific_command_paths(&leaked_paths)? {
        eprintln!(
            "preset export warning: leaving host-specific command paths unchanged in the bundle"
        );
        return Ok(Vec::new());
    }

    Ok(rewrite_host_specific_command_paths(config_dir, config))
}

fn prompt_to_fix_host_specific_command_paths(
    _leaked_paths: &[HostSpecificCommandPath],
) -> Result<bool> {
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        return super::pathing::prompt_yes_no(
            "Host-specific command paths were found; let noticenterctl rewrite them in the exported preset?",
        );
    }

    // Non-interactive export should not silently decide whether to rewrite command paths
    Err(anyhow!(
        "preset export found host-specific command paths under the UnixNotis config directory; rerun interactively to let noticenterctl rewrite them"
    ))
}

fn format_host_specific_command_path_lines(
    leaked_paths: &[HostSpecificCommandPath],
) -> Vec<String> {
    leaked_paths
        .iter()
        .map(|leak| {
            // These rows point straight at the leaking slot so the config can be rewritten to scripts/... form
            format!(
                "  - {} = {} (absolute path under the config root; let noticenterctl rewrite it to a config-root-relative command)",
                leak.slot, leak.command
            )
        })
        .collect()
}

fn validate_theme_paths_stay_in_root(
    config_dir: &Path,
    theme_paths: &[(&'static str, &Path)],
) -> Result<()> {
    let normalized_root = normalize_lexical_path(config_dir);

    // A shareable preset should not depend on files stored outside the config root
    for (slot_name, path) in theme_paths {
        let normalized_path = normalize_lexical_path(path);
        if !normalized_path.starts_with(&normalized_root) {
            return Err(anyhow!(
                "preset export requires {} to live under the config root: {}",
                slot_name,
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{export_preset_from, export_preset_from_with_confirm};
    use anyhow::anyhow;
    use crate::preset::archive::read_bundle;
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

    #[test]
    fn export_rejects_theme_paths_that_leave_root_through_parent_traversal() {
        // Lexical `../` escapes should be blocked during export, not discovered later on import
        let root = TempDirGuard::new("parent-theme-escape");
        root.write(
            "config.toml",
            "[theme]\nbase_css = \"../outside.css\"\npanel_css = \"panel.css\"\npopup_css = \"popup.css\"\nwidgets_css = \"widgets.css\"\nmedia_css = \"media.css\"\n",
        );
        root.write("panel.css", ".panel { color: red; }");
        root.write("popup.css", ".popup { color: red; }");
        root.write("widgets.css", ".widgets { color: red; }");
        root.write("media.css", ".media { color: red; }");

        let bundle_path = root.path.join("demo.unixnotis");
        let error = export_preset_from(&root.path, &bundle_path, &[], false)
            .expect_err("reject parent theme escape");

        assert!(error
            .to_string()
            .contains("requires base_css to live under the config root"));
        assert!(!bundle_path.exists());
    }

    #[test]
    fn export_rejects_absolute_plugin_command_outside_root() {
        // Shared presets should stay self-contained when commands point at local scripts
        let root = TempDirGuard::new("outside-command");
        root.write(
            "config.toml",
            "[theme]\nbase_css = \"base.css\"\n[[widgets.stats]]\nlabel = \"Probe\"\n[widgets.stats.plugin]\napi_version = 1\ncommand = \"/tmp/outside-plugin\"\n",
        );
        root.write("base.css", ".panel { color: red; }");

        let bundle_path = root.path.join("demo.unixnotis");
        let error = export_preset_from(&root.path, &bundle_path, &[], false)
            .expect_err("reject outside plugin command");

        assert!(error
            .to_string()
            .contains("explicit command paths to stay under the config root"));
        assert!(!bundle_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn export_replaces_symlink_output_instead_of_following_it() {
        // Export should replace the symlink path itself, not overwrite the symlink target
        let root = TempDirGuard::new("symlink-output");
        root.write("config.toml", "[theme]\nbase_css = \"base.css\"\n");
        root.write("base.css", ".panel { color: red; }");

        let outside_target = root.path.with_file_name("outside-target.unixnotis");
        fs::write(&outside_target, "ORIGINAL").expect("write outside target");
        let bundle_path = root.path.join("bundle.unixnotis");
        std::os::unix::fs::symlink(&outside_target, &bundle_path).expect("create output symlink");

        let summary = export_preset_from(&root.path, &bundle_path, &[], true).expect("export");

        assert_eq!(summary.file_count, 2);
        assert_eq!(
            fs::read_to_string(&outside_target).expect("read outside target"),
            "ORIGINAL"
        );
        assert!(bundle_path.exists());
        assert!(!fs::symlink_metadata(&bundle_path)
            .expect("stat bundle path")
            .file_type()
            .is_symlink());
    }

    #[test]
    fn export_rejects_outside_css_asset_refs_in_noninteractive_runs() {
        // Shared presets should pause when CSS reaches outside the config root for assets
        let root = TempDirGuard::new("external-css-asset");
        root.write("config.toml", "[theme]\nbase_css = \"base.css\"\n");
        root.write(
            "base.css",
            ".panel { background-image: url(\"../outside.png\"); }\n",
        );

        let bundle_path = root.path.join("demo.unixnotis");
        // The injected rejector makes this test stable even when cargo test has a tty
        let error =
            export_preset_from_with_confirm(
                &root.path,
                &bundle_path,
                &[],
                false,
                |_refs| {
                    Err(anyhow!(
                        "preset export found CSS asset references outside the UnixNotis config directory"
                    ))
                },
                |_leaks| Ok(false),
            )
            .expect_err("reject outside css asset refs without confirmation");

        assert!(error
            .to_string()
            .contains("CSS asset references outside the UnixNotis config directory"));
        assert!(!bundle_path.exists());
    }

    #[test]
    fn export_can_rewrite_host_specific_command_paths_in_bundle_config() {
        // Shared presets can fix leaked home paths in the bundled config without touching the live tree
        let root = TempDirGuard::new("host-specific-command-path");
        let script_path = root.path.join("scripts/unixnotis-thermal-stat");
        root.write(
            "config.toml",
            &format!(
                "[theme]\nbase_css = \"base.css\"\n[[widgets.stats]]\nlabel = \"Probe\"\n[widgets.stats.plugin]\napi_version = 1\ncommand = {:?}\n",
                script_path.display().to_string()
            ),
        );
        root.write("base.css", ".panel { color: red; }");
        root.write("scripts/unixnotis-thermal-stat", "#!/bin/sh\necho 42\n");

        let bundle_path = root.path.join("demo.unixnotis");
        let summary = export_preset_from_with_confirm(
            &root.path,
            &bundle_path,
            &[],
            false,
            |_refs| Ok(()),
            |_leaks| Ok(true),
        )
        .expect("export with rewrite");

        assert_eq!(summary.file_count, 3);
        let bundle = read_bundle(&bundle_path).expect("read bundle");
        let config_file = bundle
            .files
            .iter()
            .find(|file| file.relative_path == Path::new("config.toml"))
            .expect("bundled config");
        let config_text = std::str::from_utf8(&config_file.contents).expect("utf8 config");

        assert!(config_text.contains("scripts/unixnotis-thermal-stat"));
        assert!(!config_text.contains(&script_path.display().to_string()));
        let live_config = fs::read_to_string(root.path.join("config.toml")).expect("live config");
        assert!(live_config.contains(&script_path.display().to_string()));
    }

    #[test]
    fn export_can_keep_host_specific_command_paths_when_fix_is_declined() {
        let root = TempDirGuard::new("keep-host-specific-command-path");
        let script_path = root.path.join("scripts/unixnotis-thermal-stat");
        root.write(
            "config.toml",
            &format!(
                "[theme]\nbase_css = \"base.css\"\n[[widgets.stats]]\nlabel = \"Probe\"\n[widgets.stats.plugin]\napi_version = 1\ncommand = {:?}\n",
                script_path.display().to_string()
            ),
        );
        root.write("base.css", ".panel { color: red; }");
        root.write("scripts/unixnotis-thermal-stat", "#!/bin/sh\necho 42\n");

        let bundle_path = root.path.join("demo.unixnotis");
        export_preset_from_with_confirm(
            &root.path,
            &bundle_path,
            &[],
            false,
            |_refs| Ok(()),
            |_leaks| Ok(false),
        )
        .expect("export without rewrite");

        let bundle = read_bundle(&bundle_path).expect("read bundle");
        let config_file = bundle
            .files
            .iter()
            .find(|file| file.relative_path == Path::new("config.toml"))
            .expect("bundled config");
        let config_text = std::str::from_utf8(&config_file.contents).expect("utf8 config");

        assert!(config_text.contains(&script_path.display().to_string()));
    }
}
