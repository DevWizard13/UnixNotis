//! Install and uninstall filesystem assets.

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context, Result};

use crate::paths::{format_with_home, home_dir, InstallPaths};

use super::{log_line, run_command, ActionContext};
use unixnotis_core::program_in_path;

const HYPR_BOOTSTRAP_START: &str = "# BEGIN UNIXNOTIS SESSION BOOTSTRAP";
const HYPR_BOOTSTRAP_END: &str = "# END UNIXNOTIS SESSION BOOTSTRAP";
const HYPR_IMPORT_VARS: [&str; 7] = [
    "WAYLAND_DISPLAY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_SESSION_DESKTOP",
    "DISPLAY",
    "XDG_RUNTIME_DIR",
    "PATH",
];
const HYPR_REQUIRED_VARS: [&str; 2] = ["WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"];

pub fn install_binaries(ctx: &mut ActionContext) -> Result<()> {
    let binaries = [
        "unixnotis-daemon",
        "unixnotis-popups",
        "unixnotis-center",
        "noticenterctl",
    ];

    fs::create_dir_all(&ctx.paths.bin_dir).with_context(|| "failed to create bin directory")?;

    for binary in binaries {
        let source = ctx.paths.release_dir.join(binary);
        let destination = ctx.paths.bin_dir.join(binary);
        copy_binary(ctx, &source, &destination)?;
    }

    Ok(())
}

pub fn install_service(ctx: &mut ActionContext) -> Result<()> {
    fs::create_dir_all(&ctx.paths.unit_dir)
        .with_context(|| "failed to create systemd user directory")?;

    let exec_start = format_exec_start(ctx.paths);
    let unit_contents = [
        "[Unit]".to_string(),
        "Description=UnixNotis Notification Daemon".to_string(),
        "After=graphical-session.target".to_string(),
        "Wants=graphical-session.target".to_string(),
        "".to_string(),
        "[Service]".to_string(),
        "Type=simple".to_string(),
        format!("ExecStart={}", exec_start),
        "Restart=on-failure".to_string(),
        "RestartSec=1".to_string(),
        "".to_string(),
        "[Install]".to_string(),
        "WantedBy=default.target".to_string(),
        "".to_string(),
    ]
    .join("\n");

    fs::write(&ctx.paths.unit_path, unit_contents)
        .with_context(|| "failed to write systemd user unit")?;

    log_line(
        ctx,
        format!(
            "Installed systemd unit to {}",
            format_with_home(&ctx.paths.unit_path)
        ),
    );

    Ok(())
}

pub fn enable_service(ctx: &mut ActionContext) -> Result<()> {
    let mut daemon_reload = Command::new("systemctl");
    daemon_reload.args(["--user", "daemon-reload"]);
    run_command(ctx, "systemctl --user daemon-reload", daemon_reload, None)?;
    let mut enable = Command::new("systemctl");
    enable.args(["--user", "enable", "--now", "unixnotis-daemon.service"]);
    run_command(
        ctx,
        "systemctl --user enable --now unixnotis-daemon.service",
        enable,
        None,
    )?;
    // Keep the environment in sync after enabling the service so future restarts inherit it.
    sync_user_environment(ctx)?;
    // Hyprland exec-once ensures session vars are synced once per login without extra hooks.
    ensure_hyprland_autostart(ctx);
    Ok(())
}

pub fn uninstall_service(ctx: &mut ActionContext) -> Result<()> {
    let unit = &ctx.paths.unit_path;
    let unit_display = format_with_home(unit);

    if unit.exists() {
        let mut disable = Command::new("systemctl");
        disable.args(["--user", "disable", "--now", "unixnotis-daemon.service"]);
        if let Err(err) = run_command(
            ctx,
            "systemctl --user disable --now unixnotis-daemon.service",
            disable,
            None,
        ) {
            log_line(ctx, format!("Warning: {}", err));
        }
        let mut daemon_reload = Command::new("systemctl");
        daemon_reload.args(["--user", "daemon-reload"]);
        fs::remove_file(unit).with_context(|| "failed to remove systemd unit")?;
        run_command(ctx, "systemctl --user daemon-reload", daemon_reload, None)?;
        log_line(ctx, format!("Removed systemd unit at {}", unit_display));
    } else {
        log_line(ctx, format!("Systemd unit not found at {}", unit_display));
    }

    remove_hyprland_autostart(ctx);
    Ok(())
}

pub fn remove_binaries(ctx: &mut ActionContext) -> Result<()> {
    let binaries = [
        "unixnotis-daemon",
        "unixnotis-popups",
        "unixnotis-center",
        "noticenterctl",
    ];

    for binary in binaries {
        let path = ctx.paths.bin_dir.join(binary);
        if path.exists() {
            fs::remove_file(&path).with_context(|| "failed to remove binary")?;
            log_line(ctx, format!("Removed binary {}", format_with_home(&path)));
        } else {
            log_line(
                ctx,
                format!("Binary not found at {}", format_with_home(&path)),
            );
        }
    }

    Ok(())
}

fn copy_binary(ctx: &mut ActionContext, source: &Path, destination: &Path) -> Result<()> {
    if !source.exists() {
        return Err(anyhow!(
            "missing build artifact: {}",
            format_with_home(source)
        ));
    }

    let source_display = format_with_home(source);
    let destination_display = format_with_home(destination);
    fs::copy(source, destination).map_err(|err| {
        anyhow!(
            "failed to install {} -> {}: {}",
            source_display,
            destination_display,
            err
        )
    })?;
    log_line(
        ctx,
        format!(
            "Installed {} -> {}",
            source.file_name().unwrap_or_default().to_string_lossy(),
            format_with_home(destination)
        ),
    );
    Ok(())
}

fn format_exec_start(paths: &InstallPaths) -> String {
    let path = paths.bin_dir.join("unixnotis-daemon");
    let rendered = format_with_home(&path);
    if let Some(tail) = rendered.strip_prefix("$HOME") {
        format!("%h{}", tail)
    } else {
        path.display().to_string()
    }
}

fn sync_user_environment(ctx: &mut ActionContext) -> Result<()> {
    if !program_in_path("systemctl") {
        let message = "systemctl not found; cannot sync user environment";
        log_line(ctx, format!("Error: {}", message));
        return Err(anyhow!(message));
    }

    // Require a minimal Wayland session context before attempting import.
    let missing_required = HYPR_REQUIRED_VARS
        .iter()
        .copied()
        .filter(|var| env::var(var).is_err())
        .collect::<Vec<_>>();
    if !missing_required.is_empty() {
        let message = format!(
            "missing session variables: {}; run from a Wayland session",
            missing_required.join(", ")
        );
        log_line(ctx, format!("Error: {}", message));
        return Err(anyhow!(message));
    }

    // Track whether any environment sync step completed successfully.
    let mut updated = false;
    if program_in_path("dbus-update-activation-environment") {
        // Prefer dbus-update-activation-environment to keep systemd --user in sync with the session.
        let mut command = Command::new("dbus-update-activation-environment");
        command.args(["--systemd", "--all"]);
        if let Err(err) = run_command(
            ctx,
            "dbus-update-activation-environment --systemd --all",
            command,
            None,
        ) {
            log_line(ctx, format!("Warning: {}", err));
        } else {
            updated = true;
        }
    } else {
        log_line(
            ctx,
            "Warning: dbus-update-activation-environment not found; session env may be stale",
        );
    }

    // Import session variables that are commonly missing from systemd --user.
    let vars = HYPR_IMPORT_VARS
        .iter()
        .copied()
        .filter(|var| env::var(var).is_ok())
        .collect::<Vec<_>>();
    if vars.is_empty() {
        let message = "no session environment variables found to import for systemd --user";
        log_line(ctx, format!("Error: {}", message));
        return Err(anyhow!(message));
    } else {
        let mut command = Command::new("systemctl");
        command.args(["--user", "--no-pager", "import-environment"]);
        command.args(&vars);
        if let Err(err) = run_command(
            ctx,
            "systemctl --user --no-pager import-environment",
            command,
            None,
        ) {
            log_line(ctx, format!("Warning: {}", err));
        } else {
            updated = true;
        }
    }

    if !updated {
        let message = "failed to synchronize systemd --user environment";
        log_line(ctx, format!("Error: {}", message));
        return Err(anyhow!(message));
    }

    if updated {
        // Refresh the daemon so newly imported environment variables are picked up.
        let mut command = Command::new("systemctl");
        command.args([
            "--user",
            "--no-pager",
            "restart",
            "unixnotis-daemon.service",
        ]);
        if let Err(err) = run_command(
            ctx,
            "systemctl --user --no-pager restart unixnotis-daemon.service",
            command,
            None,
        ) {
            log_line(ctx, format!("Warning: {}", err));
        }
    }
    Ok(())
}

fn ensure_hyprland_autostart(ctx: &mut ActionContext) {
    // Locate the canonical Hyprland config to avoid assumptions about custom includes.
    let hypr_config = match hyprland_config_path() {
        Ok(path) => path,
        Err(err) => {
            log_line(ctx, format!("Warning: {}", err));
            return;
        }
    };
    if !hypr_config.exists() {
        log_line(
            ctx,
            format!(
                "Hyprland config not found at {}; skipping env bootstrap",
                format_with_home(&hypr_config)
            ),
        );
        return;
    }

    let contents = match fs::read_to_string(&hypr_config) {
        Ok(contents) => contents,
        Err(err) => {
            log_line(
                ctx,
                format!(
                    "Warning: failed to read {}: {}",
                    format_with_home(&hypr_config),
                    err
                ),
            );
            return;
        }
    };

    // Remove any previously managed block so it can be rewritten cleanly.
    let (stripped, block_found) =
        match strip_hyprland_bootstrap_block(ctx, &contents, &hypr_config) {
            Some(result) => result,
            None => return,
        };

    // Only append missing exec-once directives; existing lines remain untouched.
    // Build the minimal set of exec-once directives required for a clean login sync.
    let mut additions = Vec::new();
    if !stripped.contains("dbus-update-activation-environment --systemd --all") {
        additions.push("exec-once = dbus-update-activation-environment --systemd --all".to_string());
    }
    let has_import = stripped.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.contains("systemctl --user import-environment")
            && trimmed.contains("PATH")
            && trimmed.contains("WAYLAND_DISPLAY")
    });
    if !has_import {
        additions.push(hyprland_import_line());
    }
    let has_restart = stripped.contains("unixnotis-daemon.service")
        && (stripped.contains("systemctl --user restart")
            || stripped.contains("systemctl --user --no-pager restart"));
    if !has_restart {
        additions.push(
            "exec-once = systemctl --user --no-pager restart unixnotis-daemon.service"
                .to_string(),
        );
    }

    if additions.is_empty() {
        // When no directives are missing, drop any stale managed block and keep the file stable.
        if block_found {
            if let Err(err) = fs::write(&hypr_config, stripped) {
                log_line(ctx, format!("Warning: failed to update Hyprland config: {}", err));
            } else {
                log_line(
                    ctx,
                    format!(
                        "Removed redundant UnixNotis bootstrap from {}",
                        format_with_home(&hypr_config)
                    ),
                );
            }
        }
        log_line(ctx, "Hyprland config already includes UnixNotis env sync");
        return;
    }

    let mut updated_contents = stripped;
    if !updated_contents.ends_with('\n') {
        updated_contents.push('\n');
    }
    // Append the managed block to preserve the existing user config ordering.
    updated_contents.push_str(&render_hyprland_bootstrap_block(&additions));

    if let Err(err) = fs::write(&hypr_config, updated_contents) {
        log_line(ctx, format!("Warning: failed to update Hyprland config: {}", err));
    } else {
        log_line(
            ctx,
            format!(
                "Updated Hyprland config at {}",
                format_with_home(&hypr_config)
            ),
        );
    }
}

fn remove_hyprland_autostart(ctx: &mut ActionContext) {
    // Only remove the managed block, leaving unrelated Hyprland config intact.
    let hypr_config = match hyprland_config_path() {
        Ok(path) => path,
        Err(err) => {
            log_line(ctx, format!("Warning: {}", err));
            return;
        }
    };
    if !hypr_config.exists() {
        return;
    }

    let contents = match fs::read_to_string(&hypr_config) {
        Ok(contents) => contents,
        Err(err) => {
            log_line(
                ctx,
                format!(
                    "Warning: failed to read {}: {}",
                    format_with_home(&hypr_config),
                    err
                ),
            );
            return;
        }
    };

    let (stripped, block_found) =
        match strip_hyprland_bootstrap_block(ctx, &contents, &hypr_config) {
            Some(result) => result,
            None => return,
        };
    if !block_found {
        return;
    }
    if let Err(err) = fs::write(&hypr_config, stripped) {
        log_line(ctx, format!("Warning: failed to update Hyprland config: {}", err));
    } else {
        log_line(
            ctx,
            format!(
                "Removed UnixNotis bootstrap from {}",
                format_with_home(&hypr_config)
            ),
        );
    }
}

fn hyprland_config_path() -> Result<std::path::PathBuf> {
    // Respect XDG_CONFIG_HOME when defined to support non-default config roots.
    if let Ok(base) = env::var("XDG_CONFIG_HOME") {
        if !base.trim().is_empty() {
            return Ok(std::path::PathBuf::from(base)
                .join("hypr")
                .join("hyprland.conf"));
        }
    }
    // Fall back to the conventional ~/.config path when XDG_CONFIG_HOME is unset.
    Ok(home_dir()?.join(".config").join("hypr").join("hyprland.conf"))
}

fn render_hyprland_bootstrap_block(lines: &[String]) -> String {
    // The block markers allow a clean uninstall without touching unrelated config content.
    let mut block = String::new();
    block.push_str(HYPR_BOOTSTRAP_START);
    block.push('\n');
    block.push_str("# UnixNotis session bootstrap\n");
    block.push_str("# Ensures systemd user environment carries Wayland session variables.\n");
    block.push_str("# Managed by unixnotis-installer; safe to remove with uninstall.\n");
    for line in lines {
        block.push_str(line);
        block.push('\n');
    }
    block.push_str(HYPR_BOOTSTRAP_END);
    block.push('\n');
    block
}

fn hyprland_import_line() -> String {
    // Keep the import list in one place so Hyprland exec-once stays consistent.
    format!(
        "exec-once = systemctl --user import-environment {}",
        HYPR_IMPORT_VARS.join(" ")
    )
}

fn strip_hyprland_bootstrap_block(
    ctx: &mut ActionContext,
    contents: &str,
    config_path: &Path,
) -> Option<(String, bool)> {
    // Use the marker range to avoid disturbing user-maintained content.
    let Some(start) = contents.find(HYPR_BOOTSTRAP_START) else {
        return Some((contents.to_string(), false));
    };
    let Some(end_rel) = contents[start..].find(HYPR_BOOTSTRAP_END) else {
        log_line(
            ctx,
            format!(
                "Error: unterminated UnixNotis block in {}; manual cleanup required before install",
                format_with_home(config_path)
            ),
        );
        return None;
    };
    // Merge the remaining sections with minimal whitespace adjustments.
    let end = start + end_rel + HYPR_BOOTSTRAP_END.len();
    let before = contents[..start].trim_end_matches('\n');
    let after = contents[end..].trim_start_matches('\n');
    let mut merged = String::new();
    merged.push_str(before);
    if !before.is_empty() && !after.is_empty() {
        merged.push('\n');
    }
    merged.push_str(after);
    Some((merged, true))
}
