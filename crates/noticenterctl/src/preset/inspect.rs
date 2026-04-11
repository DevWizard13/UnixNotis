//! Preset inspect flow for printing bundle contents and command references
//!
//! Inspect is read-only and is meant to answer two questions quickly:
//! what files are inside the preset, and what command-bearing config fields it carries

use anyhow::{Context, Result};
use std::path::Path;
use unixnotis_core::Config;

use super::archive::read_bundle;
use super::pathing::{resolve_cli_bundle_path, validate_preset_bundle_path};

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandReference {
    // Human-readable config slot name shown in inspect output
    slot: String,
    // Raw command string carried by the parsed config
    command: String,
}

pub(super) fn run_inspect(input_path: &Path) -> Result<()> {
    // CLI inspect accepts a missing extension and can append it after confirmation
    let input_path = resolve_cli_bundle_path(input_path)?;
    // CLI path just prints the already-formatted report
    let report = inspect_preset_at(&input_path)?;
    print!("{report}");
    Ok(())
}

pub(super) fn inspect_preset_at(input_path: &Path) -> Result<String> {
    validate_preset_bundle_path(input_path)?;
    // Inspect uses the same reader as import so both commands see the same validation rules
    let bundle = read_bundle(input_path).context("read preset bundle for inspect")?;

    let mut out = String::new();
    out.push_str(&format!("preset: {}\n", bundle.manifest.bundle_name));
    out.push_str(&format!(
        "format version: {}\n",
        bundle.manifest.format_version
    ));
    out.push_str(&format!("exported at: {}\n", bundle.manifest.exported_at));
    out.push_str(&format!("tool version: {}\n", bundle.manifest.tool_version));
    out.push_str(&format!("files: {}\n", bundle.manifest.files.len()));
    out.push_str(&format!("assets: {}\n", yes_no(bundle.manifest.has_assets)));
    out.push_str(&format!(
        "scripts: {}\n",
        yes_no(bundle.manifest.has_scripts)
    ));

    if let Some(config_file) = bundle
        .files
        .iter()
        .find(|file| file.relative_path == Path::new("config.toml"))
    {
        // config.toml is parsed from the bundle bytes without touching the local config root
        match std::str::from_utf8(&config_file.contents) {
            Ok(contents) => match collect_command_references(contents) {
                Ok(commands) => {
                    out.push_str(&format!("command refs: {}\n", commands.len()));
                    if commands.is_empty() {
                        out.push_str("  none\n");
                    } else {
                        for command in commands {
                            out.push_str(&format!("  - {} = {}\n", command.slot, command.command));
                        }
                    }
                }
                Err(err) => {
                    out.push_str(&format!("command refs: unavailable ({err})\n"));
                }
            },
            Err(err) => {
                out.push_str(&format!("command refs: unavailable ({err})\n"));
            }
        }
    } else {
        out.push_str("command refs: unavailable (config.toml missing)\n");
    }

    out.push_str("file list:\n");
    for file in &bundle.manifest.files {
        out.push_str(&format!("  - {}\n", file.path));
    }
    Ok(out)
}

fn collect_command_references(contents: &str) -> Result<Vec<CommandReference>> {
    // Parsing through the real config type keeps inspect aligned with runtime fields
    let config: Config =
        toml::from_str(contents).context("parse config.toml from preset bundle")?;
    let mut commands = Vec::new();

    // Sliders always carry read and write commands in the config schema
    commands.push(CommandReference {
        slot: "widgets.volume.get_cmd".to_string(),
        command: config.widgets.volume.get_cmd,
    });
    commands.push(CommandReference {
        slot: "widgets.volume.set_cmd".to_string(),
        command: config.widgets.volume.set_cmd,
    });
    push_optional_command(
        &mut commands,
        "widgets.volume.toggle_cmd",
        config.widgets.volume.toggle_cmd,
    );
    push_optional_command(
        &mut commands,
        "widgets.volume.watch_cmd",
        config.widgets.volume.watch_cmd,
    );

    commands.push(CommandReference {
        slot: "widgets.brightness.get_cmd".to_string(),
        command: config.widgets.brightness.get_cmd,
    });
    commands.push(CommandReference {
        slot: "widgets.brightness.set_cmd".to_string(),
        command: config.widgets.brightness.set_cmd,
    });
    push_optional_command(
        &mut commands,
        "widgets.brightness.toggle_cmd",
        config.widgets.brightness.toggle_cmd,
    );
    push_optional_command(
        &mut commands,
        "widgets.brightness.watch_cmd",
        config.widgets.brightness.watch_cmd,
    );

    // Toggles, stats, and cards expose optional command slots, so only live values are printed
    for (index, toggle) in config.widgets.toggles.into_iter().enumerate() {
        push_optional_command(
            &mut commands,
            &format!("widgets.toggles[{index}].state_cmd"),
            toggle.state_cmd,
        );
        push_optional_command(
            &mut commands,
            &format!("widgets.toggles[{index}].on_cmd"),
            toggle.on_cmd,
        );
        push_optional_command(
            &mut commands,
            &format!("widgets.toggles[{index}].off_cmd"),
            toggle.off_cmd,
        );
        push_optional_command(
            &mut commands,
            &format!("widgets.toggles[{index}].watch_cmd"),
            toggle.watch_cmd,
        );
    }

    for (index, stat) in config.widgets.stats.into_iter().enumerate() {
        push_optional_command(
            &mut commands,
            &format!("widgets.stats[{index}].cmd"),
            stat.cmd,
        );
        push_optional_command(
            &mut commands,
            &format!("widgets.stats[{index}].plugin.command"),
            stat.plugin.map(|plugin| plugin.command),
        );
    }

    for (index, card) in config.widgets.cards.into_iter().enumerate() {
        push_optional_command(
            &mut commands,
            &format!("widgets.cards[{index}].cmd"),
            card.cmd,
        );
        push_optional_command(
            &mut commands,
            &format!("widgets.cards[{index}].plugin.command"),
            card.plugin.map(|plugin| plugin.command),
        );
    }

    Ok(commands)
}

fn push_optional_command(commands: &mut Vec<CommandReference>, slot: &str, value: Option<String>) {
    let Some(command) = value else {
        return;
    };
    let trimmed = command.trim();
    if trimmed.is_empty() {
        // Blank values are treated the same as missing values in inspect output
        return;
    }

    commands.push(CommandReference {
        slot: slot.to_string(),
        command: trimmed.to_string(),
    });
}

fn yes_no(value: bool) -> &'static str {
    // Small helper keeps inspect output predictable and grep-friendly
    if value {
        "yes"
    } else {
        "no"
    }
}

#[cfg(test)]
mod tests {
    use super::inspect_preset_at;
    use crate::preset::export::export_preset_from;
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
            // Unique temp roots keep inspect tests independent from export and import tests
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock moved backwards")
                .as_nanos();
            let serial = TEST_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "unixnotis-preset-inspect-{}-{}-{}",
                name, stamp, serial
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn write(&self, relative_path: &str, contents: &str) {
            // Helper keeps the test body focused on the reported output
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
    fn inspect_lists_bundle_metadata_and_commands() {
        // Inspect should expose the command-bearing parts of the shared config
        let root = TempDirGuard::new("report");
        root.write(
            "config.toml",
            "[theme]\nbase_css = \"base.css\"\n[widgets.volume]\nget_cmd = \"wpctl get-volume @DEFAULT_AUDIO_SINK@\"\n",
        );
        root.write("base.css", ".a { color: red; }");
        let bundle_path = root.path.join("demo.unixnotis");
        export_preset_from(&root.path, &bundle_path, &[], false).expect("export");

        let report = inspect_preset_at(&bundle_path).expect("inspect");

        assert!(report.contains("preset: demo"));
        assert!(report.contains("widgets.volume.get_cmd"));
        assert!(report.contains("file list:"));
        assert!(report.contains("config.toml"));
    }
}
