//! Formatting helpers for detection summaries.

use crate::detect::DetectedDaemon;

pub fn summarize_owner(owner: &Option<crate::detect::OwnerInfo>) -> String {
    match owner {
        Some(info) => {
            let name = info.comm.as_deref().unwrap_or("unknown");
            let pid = info
                .pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            format!("{} (pid {})", name, pid)
        }
        None => "none detected".to_string(),
    }
}

pub fn format_daemon_status(daemon: &DetectedDaemon) -> String {
    let mut status = Vec::new();
    if daemon.is_owner {
        status.push("dbus-owner".to_string());
    }
    if daemon.systemd_active {
        status.push("systemd-active".to_string());
    }
    if let Some(err) = daemon.systemd_error.as_ref() {
        status.push(format!("systemd-error: {}", err));
    }
    if !daemon.running_pids.is_empty() {
        let ids = daemon
            .running_pids
            .iter()
            .map(|pid| pid.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        status.push(format!("pid {}", ids));
    }
    if status.is_empty() {
        status.push("not running".to_string());
    }
    status.join(", ")
}

#[cfg(test)]
mod tests {
    use super::{format_daemon_status, summarize_owner};
    use crate::detect::{DetectedDaemon, OwnerInfo};

    #[test]
    fn summarize_owner_includes_comm_and_pid() {
        // Verifies formatted owner output includes both fields when available.
        let owner = OwnerInfo {
            pid: Some(4242),
            comm: Some("unixnotis-daemon".to_string()),
        };
        let rendered = summarize_owner(&Some(owner));
        assert_eq!(rendered, "unixnotis-daemon (pid 4242)");
    }

    #[test]
    fn summarize_owner_handles_missing_owner() {
        // Ensures the empty-owner branch renders a stable placeholder string.
        let rendered = summarize_owner(&None);
        assert_eq!(rendered, "none detected");
    }

    #[test]
    fn format_daemon_status_reports_owner_and_pids() {
        // Confirms the formatted status lists ownership, active state, and running PIDs.
        let daemon = DetectedDaemon {
            name: "unixnotis-daemon".to_string(),
            unit: "unixnotis-daemon.service".to_string(),
            systemd_active: true,
            systemd_error: None,
            running_pids: vec![101, 202],
            is_owner: true,
        };
        let rendered = format_daemon_status(&daemon);
        assert!(rendered.contains("dbus-owner"));
        assert!(rendered.contains("systemd-active"));
        assert!(rendered.contains("pid 101, 202"));
    }

    #[test]
    fn format_daemon_status_reports_inactive_with_error() {
        // Ensures errors surface even when the daemon is otherwise inactive.
        let daemon = DetectedDaemon {
            name: "other".to_string(),
            unit: "other.service".to_string(),
            systemd_active: false,
            systemd_error: Some("systemctl failure".to_string()),
            running_pids: Vec::new(),
            is_owner: false,
        };
        let rendered = format_daemon_status(&daemon);
        assert!(rendered.contains("systemd-error: systemctl failure"));
        assert!(!rendered.contains("systemd-active"));
    }
}
