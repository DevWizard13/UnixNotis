//! Shared command reference collection and path checks for preset config
//!
//! Preset import, export, inspect, and css-check all need the same answer:
//! which command slots carry explicit filesystem paths, and do those paths stay
//! under the UnixNotis config root

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use unixnotis_core::{util, Config};

use super::pathing::{format_relative_path, normalize_lexical_path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandReference {
    // Human-readable config slot name shown in inspect and warning output
    pub(crate) slot: String,
    // Raw command string carried by the parsed config
    pub(crate) command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutsideCommandPath {
    // Config slot that carried the outside path
    pub(crate) slot: String,
    // Raw command string from the config
    pub(crate) command: String,
    // Resolved first-token path used by the validator
    pub(crate) resolved_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostSpecificCommandPath {
    // Config slot that carried the host-specific path
    pub(crate) slot: String,
    // Raw command string from the config
    pub(crate) command: String,
    // Resolved first-token path under the config root
    pub(crate) resolved_path: PathBuf,
}

pub(crate) fn collect_command_references_from_config(config: &Config) -> Vec<CommandReference> {
    let mut commands = Vec::new();

    // Each widget family is collected separately so later checks can reason about real slot names
    collect_slider_commands(
        &mut commands,
        "widgets.volume",
        &config.widgets.volume.get_cmd,
        &config.widgets.volume.set_cmd,
        config.widgets.volume.toggle_cmd.as_deref(),
        config.widgets.volume.watch_cmd.as_deref(),
    );
    collect_slider_commands(
        &mut commands,
        "widgets.brightness",
        &config.widgets.brightness.get_cmd,
        &config.widgets.brightness.set_cmd,
        config.widgets.brightness.toggle_cmd.as_deref(),
        config.widgets.brightness.watch_cmd.as_deref(),
    );
    for (index, toggle) in config.widgets.toggles.iter().enumerate() {
        push_optional_command(
            &mut commands,
            &format!("widgets.toggles[{index}].state_cmd"),
            toggle.state_cmd.as_deref(),
        );
        push_optional_command(
            &mut commands,
            &format!("widgets.toggles[{index}].on_cmd"),
            toggle.on_cmd.as_deref(),
        );
        push_optional_command(
            &mut commands,
            &format!("widgets.toggles[{index}].off_cmd"),
            toggle.off_cmd.as_deref(),
        );
        push_optional_command(
            &mut commands,
            &format!("widgets.toggles[{index}].watch_cmd"),
            toggle.watch_cmd.as_deref(),
        );
    }
    for (index, stat) in config.widgets.stats.iter().enumerate() {
        push_optional_command(
            &mut commands,
            &format!("widgets.stats[{index}].cmd"),
            stat.cmd.as_deref(),
        );
        push_optional_command(
            &mut commands,
            &format!("widgets.stats[{index}].plugin.command"),
            stat.plugin.as_ref().map(|plugin| plugin.command.as_str()),
        );
    }
    for (index, card) in config.widgets.cards.iter().enumerate() {
        push_optional_command(
            &mut commands,
            &format!("widgets.cards[{index}].cmd"),
            card.cmd.as_deref(),
        );
        push_optional_command(
            &mut commands,
            &format!("widgets.cards[{index}].plugin.command"),
            card.plugin.as_ref().map(|plugin| plugin.command.as_str()),
        );
    }

    commands
}

pub(crate) fn collect_outside_command_paths(
    config_dir: &Path,
    config: &Config,
) -> Vec<OutsideCommandPath> {
    let normalized_root = normalize_lexical_path(config_dir);

    collect_command_references_from_config(config)
        .into_iter()
        .filter_map(|reference| {
            let resolved_path = resolve_command_path_token(config_dir, &reference.command)?;
            // Only explicit path commands are checked here
            let normalized_path = normalize_lexical_path(&resolved_path);
            if normalized_path.starts_with(&normalized_root) {
                return None;
            }

            Some(OutsideCommandPath {
                slot: reference.slot,
                command: reference.command,
                resolved_path,
            })
        })
        .collect()
}

pub(crate) fn collect_host_specific_command_paths(
    config_dir: &Path,
    config: &Config,
) -> Vec<HostSpecificCommandPath> {
    let normalized_root = normalize_lexical_path(config_dir);

    collect_command_references_from_config(config)
        .into_iter()
        .filter_map(|reference| {
            let token = first_command_token(&reference.command)?;
            let resolved_path = resolve_command_path_token(config_dir, &reference.command)?;
            let normalized_path = normalize_lexical_path(&resolved_path);
            // Only absolute host-local command paths under the config root are warned here
            if !normalized_path.starts_with(&normalized_root) || !is_host_specific_path_token(token)
            {
                return None;
            }

            Some(HostSpecificCommandPath {
                slot: reference.slot,
                command: reference.command,
                resolved_path,
            })
        })
        .collect()
}

pub(crate) fn rewrite_host_specific_command_paths(
    config_dir: &Path,
    config: &mut Config,
) -> Vec<HostSpecificCommandPath> {
    let leaks = collect_host_specific_command_paths(config_dir, config);
    if leaks.is_empty() {
        return leaks;
    }

    // Each command-bearing slot is rewritten in place so only the exported bundle changes
    rewrite_slider_commands(config_dir, &mut config.widgets.volume);
    rewrite_slider_commands(config_dir, &mut config.widgets.brightness);
    for toggle in &mut config.widgets.toggles {
        rewrite_optional_command(config_dir, &mut toggle.state_cmd);
        rewrite_optional_command(config_dir, &mut toggle.on_cmd);
        rewrite_optional_command(config_dir, &mut toggle.off_cmd);
        rewrite_optional_command(config_dir, &mut toggle.watch_cmd);
    }
    for stat in &mut config.widgets.stats {
        rewrite_optional_command(config_dir, &mut stat.cmd);
        if let Some(plugin) = stat.plugin.as_mut() {
            rewrite_inline_command(config_dir, &mut plugin.command);
        }
    }
    for card in &mut config.widgets.cards {
        rewrite_optional_command(config_dir, &mut card.cmd);
        if let Some(plugin) = card.plugin.as_mut() {
            rewrite_inline_command(config_dir, &mut plugin.command);
        }
    }

    leaks
}

pub(crate) fn validate_config_command_paths_stay_in_root(
    config_dir: &Path,
    config: &Config,
    mode_label: &str,
) -> Result<()> {
    let outside_paths = collect_outside_command_paths(config_dir, config);
    if outside_paths.is_empty() {
        return Ok(());
    }

    let first = &outside_paths[0];
    Err(anyhow!(
        "{} because {} points outside the UnixNotis config directory: {}",
        mode_label,
        first.slot,
        first.command
    ))
}

pub(crate) fn validate_command_paths_in_config_bytes(
    config_dir: &Path,
    config_bytes: &[u8],
    mode_label: &str,
) -> Result<()> {
    let config_text =
        std::str::from_utf8(config_bytes).context("preset config.toml is not valid UTF-8")?;
    let config: Config =
        toml::from_str(config_text).context("parse bundled config.toml for command path checks")?;
    validate_config_command_paths_stay_in_root(config_dir, &config, mode_label)
}

fn collect_slider_commands(
    commands: &mut Vec<CommandReference>,
    base_slot: &str,
    get_cmd: &str,
    set_cmd: &str,
    toggle_cmd: Option<&str>,
    watch_cmd: Option<&str>,
) {
    // Sliders always expose read and write commands, so those are always listed
    commands.push(CommandReference {
        slot: format!("{base_slot}.get_cmd"),
        command: get_cmd.to_string(),
    });
    commands.push(CommandReference {
        slot: format!("{base_slot}.set_cmd"),
        command: set_cmd.to_string(),
    });
    push_optional_command(commands, &format!("{base_slot}.toggle_cmd"), toggle_cmd);
    push_optional_command(commands, &format!("{base_slot}.watch_cmd"), watch_cmd);
}

fn rewrite_slider_commands(config_dir: &Path, slider: &mut unixnotis_core::SliderWidgetConfig) {
    rewrite_inline_command(config_dir, &mut slider.get_cmd);
    rewrite_inline_command(config_dir, &mut slider.set_cmd);
    rewrite_optional_command(config_dir, &mut slider.toggle_cmd);
    rewrite_optional_command(config_dir, &mut slider.watch_cmd);
}

fn rewrite_optional_command(config_dir: &Path, value: &mut Option<String>) {
    let Some(command) = value.as_mut() else {
        return;
    };
    rewrite_inline_command(config_dir, command);
}

fn rewrite_inline_command(config_dir: &Path, command: &mut String) {
    let Some(rewritten) = rewrite_command_to_config_relative(config_dir, command) else {
        return;
    };
    *command = rewritten;
}

fn push_optional_command(commands: &mut Vec<CommandReference>, slot: &str, value: Option<&str>) {
    let Some(command) = value else {
        return;
    };
    let trimmed = command.trim();
    if trimmed.is_empty() {
        // Blank values are treated the same as missing values in reports
        return;
    }

    commands.push(CommandReference {
        slot: slot.to_string(),
        command: trimmed.to_string(),
    });
}

fn resolve_command_path_token(config_dir: &Path, command: &str) -> Option<PathBuf> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Shell-backed commands can hide paths in many places, so this check only targets
    // explicit path commands where the executable itself is a path token
    if !util::is_simple_command(trimmed) {
        return None;
    }

    let first = first_command_token(trimmed)?;
    if !looks_like_path_token(first) {
        return None;
    }

    let expanded = PathBuf::from(util::expand_tilde(first).into_owned());
    if expanded.is_absolute() {
        return Some(expanded);
    }
    Some(config_dir.join(expanded))
}

fn rewrite_command_to_config_relative(config_dir: &Path, command: &str) -> Option<String> {
    let trimmed = command.trim();
    if trimmed.is_empty() || !util::is_simple_command(trimmed) {
        return None;
    }

    let first = first_command_token(trimmed)?;
    if !is_host_specific_path_token(first) {
        return None;
    }

    let resolved_path = resolve_command_path_token(config_dir, trimmed)?;
    let normalized_root = normalize_lexical_path(config_dir);
    let normalized_path = normalize_lexical_path(&resolved_path);
    // Only paths that really live under the config root can be rewritten safely
    let relative_path = normalized_path.strip_prefix(&normalized_root).ok()?;
    let rewritten_first = format_relative_path(relative_path);
    if rewritten_first.is_empty() {
        return None;
    }

    // Keep the rest of the command string as-is so flags and placeholders survive
    let rest = trimmed[first.len()..].trim_start();
    if rest.is_empty() {
        return Some(rewritten_first);
    }
    Some(format!("{rewritten_first} {rest}"))
}

fn first_command_token(command: &str) -> Option<&str> {
    command.split_whitespace().next()
}

fn looks_like_path_token(token: &str) -> bool {
    token == "~"
        || token.starts_with("~/")
        || token.starts_with("./")
        || token.starts_with("../")
        || token.contains('/')
}

fn is_host_specific_path_token(token: &str) -> bool {
    token.starts_with('/') || token == "~" || token.starts_with("~/")
}

#[cfg(test)]
mod tests {
    use super::{
        collect_command_references_from_config, collect_host_specific_command_paths,
        collect_outside_command_paths, rewrite_host_specific_command_paths,
        validate_command_paths_in_config_bytes,
    };
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use unixnotis_core::Config;

    static TEST_TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn temp_root(name: &str) -> PathBuf {
        // Unique paths keep lexical path checks stable under parallel test runs
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock moved backwards")
            .as_nanos();
        let serial = TEST_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "unixnotis-preset-command-paths-{}-{}-{}",
            name, stamp, serial
        ))
    }

    #[test]
    fn collects_widget_command_references() {
        let config: Config = toml::from_str(
            "\
[theme]\nbase_css = \"base.css\"\n\
[[widgets.stats]]\nlabel = \"Probe\"\n\
[widgets.stats.plugin]\napi_version = 1\ncommand = \"scripts/fetch.sh\"\n",
        )
        .expect("parse config");

        let commands = collect_command_references_from_config(&config);

        assert!(commands
            .iter()
            .any(|command| command.slot == "widgets.stats[0].plugin.command"));
    }

    #[test]
    fn outside_command_paths_include_absolute_plugin_command() {
        let config_dir = temp_root("outside-plugin");
        let config = "\
[theme]\nbase_css = \"base.css\"\n\
[[widgets.stats]]\nlabel = \"Probe\"\n\
[widgets.stats.plugin]\napi_version = 1\ncommand = \"/tmp/outside-plugin\"\n";

        let parsed = toml::from_str(config).expect("parse config");
        let outside = collect_outside_command_paths(&config_dir, &parsed);

        assert_eq!(outside.len(), 1);
        assert_eq!(outside[0].slot, "widgets.stats[0].plugin.command");
    }

    #[test]
    fn validation_rejects_relative_command_path_that_leaves_root() {
        let config_dir = temp_root("relative-command");
        let config = b"[theme]\nbase_css = \"base.css\"\n[[widgets.toggles]]\nlabel = \"Probe\"\nicon = \"applications-system-symbolic\"\nwatch_cmd = \"../outside-watch\"\n";

        let error =
            validate_command_paths_in_config_bytes(&config_dir, config, "preset import blocked")
                .expect_err("reject relative command escape");

        assert!(error
            .to_string()
            .contains("points outside the UnixNotis config directory"));
    }

    #[test]
    fn host_specific_command_paths_include_absolute_path_inside_root() {
        let config_dir = temp_root("inside-root-host-specific");
        let script_path = config_dir.join("scripts/unixnotis-thermal-stat");
        let config = format!(
            "\
[theme]\nbase_css = \"base.css\"\n\
[[widgets.stats]]\nlabel = \"Probe\"\n\
[widgets.stats.plugin]\napi_version = 1\ncommand = {:?}\n",
            script_path.display().to_string()
        );

        let parsed = toml::from_str(&config).expect("parse config");
        let leaks = collect_host_specific_command_paths(&config_dir, &parsed);

        assert_eq!(leaks.len(), 1);
        assert_eq!(leaks[0].slot, "widgets.stats[0].plugin.command");
    }

    #[test]
    fn rewrite_host_specific_command_paths_makes_commands_config_relative() {
        let config_dir = temp_root("rewrite");
        let script_path = config_dir.join("scripts/unixnotis-thermal-stat");
        let config = format!(
            "\
[theme]\nbase_css = \"base.css\"\n\
[[widgets.stats]]\nlabel = \"Probe\"\n\
[widgets.stats.plugin]\napi_version = 1\ncommand = {:?}\n",
            format!("{} --json", script_path.display())
        );

        let mut parsed: Config = toml::from_str(&config).expect("parse config");
        let rewritten = rewrite_host_specific_command_paths(&config_dir, &mut parsed);

        assert_eq!(rewritten.len(), 1);
        assert_eq!(
            parsed.widgets.stats[0]
                .plugin
                .as_ref()
                .expect("plugin")
                .command,
            "scripts/unixnotis-thermal-stat --json"
        );
    }
}
