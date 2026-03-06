//! Output formatting helpers for noticenterctl.

use unixnotis_core::{util, NotificationView};

pub(crate) fn print_notifications(label: &str, notifications: &[NotificationView], full: bool) {
    print!("{}", format_notifications(label, notifications, full));
}

pub(crate) fn print_inhibitors(inhibitors: &[(u64, String, u32, String)]) {
    print!("{}", format_inhibitors(inhibitors));
}

fn format_notifications(label: &str, notifications: &[NotificationView], full: bool) -> String {
    // Respect the diagnostic mode guard so secrets are not printed unintentionally
    let limit = if full {
        util::diagnostic_log_limit()
    } else {
        util::default_log_limit()
    };

    // Build the whole payload first so tests can assert exact CLI output
    let mut out = String::new();
    out.push_str(&format!("{label} notifications: {}\n", notifications.len()));

    for notification in notifications {
        // Every user-facing field is sanitized so hostile notifications stay single-line safe
        let app = util::sanitize_log_value(&notification.app_name, limit);
        let summary = util::sanitize_log_value(&notification.summary, limit);
        let action_count = notification.actions.len();
        out.push_str(&format!(
            "- #{id} [{app}] {summary} (actions={actions})\n",
            id = notification.id,
            app = app,
            summary = summary,
            actions = action_count
        ));

        if full {
            // Full mode includes the body text while still stripping control characters
            let body = util::sanitize_log_value(&notification.body, limit);
            out.push_str(&format!("  body: {body}\n"));
        }
    }

    out
}

fn format_inhibitors(inhibitors: &[(u64, String, u32, String)]) -> String {
    // Default log limit is enough here because inhibitor rows are operational metadata
    let limit = util::default_log_limit();
    let mut out = String::new();
    out.push_str(&format!("inhibitors: {}\n", inhibitors.len()));

    for (id, reason, scope, owner) in inhibitors {
        // Reason and owner both originate outside the CLI and must be terminal-safe
        let owner = util::sanitize_log_value(owner, limit);
        let reason = util::sanitize_log_value(reason, limit);
        out.push_str(&format!(
            "- #{id} scope={scope} owner={owner} reason={reason}\n"
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use unixnotis_core::{Action, NotificationImage, NotificationView};

    use super::{format_inhibitors, format_notifications};

    fn sample_notification() -> NotificationView {
        NotificationView {
            id: 7,
            app_name: "mailer\n\x1b[31m".to_string(),
            summary: "subject\rline".to_string(),
            body: "body\ttext\nnext".to_string(),
            actions: vec![Action {
                key: "open".to_string(),
                label: "Open".to_string(),
            }],
            urgency: 1,
            is_transient: false,
            is_resident: false,
            received_at_unix_ms: 0,
            image: NotificationImage::default(),
        }
    }

    #[test]
    fn format_notifications_sanitizes_terminal_control_sequences() {
        let output = format_notifications("active", &[sample_notification()], false);
        assert!(output.contains("mailer"));
        assert!(output.contains("[31m]"));
        assert!(output.contains("subject line"));
        assert!(!output.contains('\n') || output.lines().count() == 2);
        assert!(!output.contains('\u{1b}'));
    }

    #[test]
    fn format_notifications_full_mode_includes_body() {
        let output = format_notifications("history", &[sample_notification()], true);
        assert!(output.contains("body: body text next"));
    }

    #[test]
    fn format_inhibitors_sanitizes_reason_and_owner() {
        let output =
            format_inhibitors(&[(5, "present\nmode".to_string(), 1, ":1.2\r".to_string())]);
        assert!(output.contains("owner=:1.2 "));
        assert!(output.contains("reason=present mode"));
    }
}
