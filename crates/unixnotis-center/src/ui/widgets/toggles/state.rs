//! Toggle async state refresh helpers
//!
//! This module isolates command execution, parsing, and bounded retry behavior

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use tracing::warn;

use super::super::util::run_command_capture_status_async;

// Staggered retry delays keep UI responsive without long-lived polling loops
const TOGGLE_REFRESH_DELAYS_MS: &[u64] = &[0, 50, 100, 200, 400, 800];

pub(super) fn refresh_toggle_state(
    cmd: &str,
    button: &gtk::ToggleButton,
    guard: &Rc<Cell<bool>>,
    refresh_gen: &Arc<AtomicU64>,
) {
    // Own the command string so spawned tasks stay self-contained
    let cmd = cmd.to_string();

    // Each refresh claims a generation so stale tasks cannot overwrite newer state
    let gen = refresh_gen.fetch_add(1, Ordering::Relaxed) + 1;
    let button = button.clone();
    let guard = guard.clone();
    let refresh_gen = Arc::clone(refresh_gen);

    glib::MainContext::default().spawn_local(async move {
        let Some(active) = fetch_toggle_state(&cmd, true).await else {
            return;
        };

        // Drop stale result when a newer refresh has already started
        if refresh_gen.load(Ordering::Relaxed) != gen {
            return;
        }

        if button.is_active() != active {
            // Guard blocks feedback loops through connect_toggled
            guard.set(true);
            button.set_active(active);
            guard.set(false);
        }
    });
}

pub(super) fn schedule_toggle_refresh_with_retry(
    state_cmd: String,
    expected: bool,
    button: gtk::ToggleButton,
    guard: Rc<Cell<bool>>,
    refresh_gen: Arc<AtomicU64>,
) {
    // Bounded retries reconcile optimistic UI state with eventually-consistent commands
    let gen = refresh_gen.fetch_add(1, Ordering::Relaxed) + 1;

    // Weak refs avoid extending widget lifetimes from detached async tasks
    let button_weak = button.downgrade();
    let guard_weak = Rc::downgrade(&guard);
    let refresh_gen_weak = Arc::downgrade(&refresh_gen);

    glib::MainContext::default().spawn_local(async move {
        for (attempt, delay_ms) in TOGGLE_REFRESH_DELAYS_MS.iter().enumerate() {
            // Delay sequence smooths transient backend lag
            if *delay_ms > 0 {
                glib::timeout_future(Duration::from_millis(*delay_ms)).await;
            }

            let Some(refresh_gen) = refresh_gen_weak.upgrade() else {
                return;
            };
            if refresh_gen.load(Ordering::Relaxed) != gen {
                return;
            }

            // Keep warnings bounded to the first failed probe per action
            let log_failures = attempt == 0;
            let Some(active) = fetch_toggle_state(&state_cmd, log_failures).await else {
                continue;
            };

            if refresh_gen.load(Ordering::Relaxed) != gen {
                return;
            }

            // Widgets may have been destroyed while command was running
            let (Some(button), Some(guard)) = (button_weak.upgrade(), guard_weak.upgrade()) else {
                return;
            };

            if button.is_active() != active {
                // Apply corrected state without retriggering command dispatch
                guard.set(true);
                button.set_active(active);
                guard.set(false);
            }

            if active == expected {
                // Stop retrying once backend and UI agree
                return;
            }
        }
    });
}

async fn fetch_toggle_state(cmd: &str, log_failures: bool) -> Option<bool> {
    // Command helper returns receiver so execution stays off the GTK thread
    let rx = run_command_capture_status_async(cmd);

    let output = match rx.recv().await {
        Ok(output) => output,
        Err(_) => return None,
    };

    let output = match output {
        Ok(output) => output,
        Err(err) => {
            if log_failures {
                warn!(?cmd, ?err, "toggle state command failed");
            }
            return None;
        }
    };

    let success = output.status.success();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Empty stdout falls back to exit status semantics
    let active = if stdout.trim().is_empty() {
        success
    } else {
        parse_toggle_state(&stdout)
    };

    Some(active)
}

fn parse_toggle_state(output: &str) -> bool {
    // Handle bluetoothctl style structured output first for predictable power semantics
    for line in output.lines() {
        let lower = line.trim().to_ascii_lowercase();
        if lower.contains("powered") || lower.contains("powerstate") {
            if lower.contains("no")
                || lower.contains("off")
                || lower.contains("false")
                || lower.contains("disabled")
            {
                return false;
            }
            if lower.contains("yes")
                || lower.contains("on")
                || lower.contains("true")
                || lower.contains("enabled")
            {
                return true;
            }
        }
    }

    let value = output.trim().to_ascii_lowercase();

    // systemctl is-active style output has strong explicit states
    if matches!(value.as_str(), "active" | "activated") {
        return true;
    }
    if matches!(value.as_str(), "inactive" | "failed" | "dead") {
        return false;
    }

    // Single-token affirmative results are treated as enabled
    if matches!(
        value.as_str(),
        "1" | "on" | "yes" | "true" | "enabled" | "up"
    ) {
        return true;
    }

    // Tokenized fallback catches mixed plain-text command outputs
    value
        // Non-alphanumeric separators cover outputs like "state: on"
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| matches!(token, "on" | "yes" | "true" | "enabled" | "up" | "active"))
}
