//! D-Bus server for org.freedesktop.Notifications.
//!
//! Keeps notification delivery logic separate from the control-plane interface.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::debug;
use unixnotis_core::{
    Action, CloseReason, Config, Notification, NotificationImage, Urgency, CONTROL_OBJECT_PATH,
};
use zbus::fdo::DBusProxy;
use zbus::message::Header;
use zbus::zvariant::{OwnedValue, Value};
use zbus::{interface, Connection, SignalContext};

use crate::expire::ExpirationScheduler;

use super::{to_fdo_error, ControlServer, DaemonState, NOTIFICATIONS_OBJECT_PATH};

// Defensive caps for untrusted D-Bus payload fields.
const MAX_APP_NAME_BYTES: usize = 256;
const MAX_APP_ICON_BYTES: usize = 1024;
const MAX_SUMMARY_BYTES: usize = 1024;
const MAX_BODY_BYTES: usize = 16 * 1024;
const MAX_CATEGORY_BYTES: usize = 256;
const MAX_ACTIONS: usize = 32;
const MAX_ACTION_KEY_BYTES: usize = 128;
const MAX_ACTION_LABEL_BYTES: usize = 256;
const MAX_HINT_ENTRIES: usize = 16;
const MAX_HINT_KEY_BYTES: usize = 64;
const MAX_HINT_STRING_BYTES: usize = 2048;

/// D-Bus server for org.freedesktop.Notifications.
pub struct NotificationServer {
    state: Arc<DaemonState>,
    scheduler: ExpirationScheduler,
}

impl NotificationServer {
    pub fn new(state: Arc<DaemonState>, scheduler: ExpirationScheduler) -> Self {
        Self { state, scheduler }
    }
}

#[interface(name = "org.freedesktop.Notifications")]
impl NotificationServer {
    async fn get_capabilities(&self) -> Vec<String> {
        let mut caps = vec![
            "actions".to_string(),
            "body".to_string(),
            "body-markup".to_string(),
            "icon-static".to_string(),
        ];
        if self.state.sound.supports_sound() {
            caps.push("sound".to_string());
        }
        caps
    }

    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        expire_timeout: i32,
    ) -> zbus::fdo::Result<u32> {
        if tracing::enabled!(tracing::Level::DEBUG) {
            let summary_snip = unixnotis_core::util::log_snippet(&summary);
            debug!(
                app = %app_name,
                summary = %summary_snip,
                summary_len = summary.len(),
                body_len = body.len(),
                replaces_id,
                expire_timeout,
                "received notification"
            );
            if unixnotis_core::util::diagnostic_mode() {
                let body_snip = unixnotis_core::util::log_snippet(&body);
                debug!(body = %body_snip, "notification body snippet");
            }
        }
        let sender = resolve_sender_metadata(self.state.connection(), &header).await;
        if sender
            .sender_executable
            .as_deref()
            .is_some_and(|exe| !app_name_matches_sender(&app_name, exe))
        {
            debug!(
                app_name = %app_name,
                sender = sender.sender_name.as_deref().unwrap_or("unknown"),
                sender_executable = sender.sender_executable.as_deref().unwrap_or("unknown"),
                "notification app_name does not match sender executable"
            );
        }
        let notification = build_notification(
            app_name,
            app_icon,
            summary,
            body,
            actions,
            hints,
            sender,
            expire_timeout,
        );

        let (outcome, expiration) = {
            let mut store = self.state.store.lock().await;
            let outcome = store.insert(notification, replaces_id);
            let expiration = if outcome.dropped {
                None
            } else {
                let expiration = resolve_expiration(store.config(), &outcome.notification);
                store.set_expiration(outcome.notification.id, expiration);
                expiration
            };
            (outcome, expiration)
        };
        if outcome.dropped {
            // Drop-all mode skips scheduling, sound, and control-plane signals entirely.
            debug!(
                id = outcome.notification.id,
                app = %outcome.notification.app_name,
                "notification dropped due to active inhibitor"
            );
            return Ok(outcome.notification.id);
        }
        self.scheduler
            .schedule(outcome.notification.id, expiration)
            .await;
        // Sound playback is driven by hints plus configured defaults.
        self.state
            .sound
            .play_from_hints(&outcome.notification.hints, outcome.allow_sound);

        let control_ctx = SignalContext::new(self.state.connection(), CONTROL_OBJECT_PATH)
            .map_err(to_fdo_error)?;
        if outcome.replaced {
            ControlServer::notification_updated(
                &control_ctx,
                outcome.notification.to_view(),
                outcome.show_popup,
            )
            .await
            .map_err(to_fdo_error)?;
        } else {
            ControlServer::notification_added(
                &control_ctx,
                outcome.notification.to_view(),
                outcome.show_popup,
            )
            .await
            .map_err(to_fdo_error)?;
        }
        // Evictions occur when active history limits are exceeded.
        self.handle_evicted(outcome.evicted).await?;
        self.state
            .emit_state_changed()
            .await
            .map_err(to_fdo_error)?;

        Ok(outcome.notification.id)
    }

    async fn handle_evicted(&self, evicted: Vec<u32>) -> zbus::fdo::Result<()> {
        if evicted.is_empty() {
            return Ok(());
        }
        // Emit close signals for evicted notifications to keep UI state consistent.
        let notif_ctx = SignalContext::new(self.state.connection(), NOTIFICATIONS_OBJECT_PATH)
            .map_err(to_fdo_error)?;
        let control_ctx = SignalContext::new(self.state.connection(), CONTROL_OBJECT_PATH)
            .map_err(to_fdo_error)?;
        for id in evicted {
            NotificationServer::notification_closed(&notif_ctx, id, CloseReason::Undefined as u32)
                .await
                .map_err(to_fdo_error)?;
            ControlServer::notification_closed(&control_ctx, id, CloseReason::Undefined)
                .await
                .map_err(to_fdo_error)?;
        }
        Ok(())
    }

    async fn close_notification(
        &self,
        id: u32,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        debug!(id, "close notification requested");
        // Freedesktop close requests can only target notifications owned by the same sender.
        // Unauthorized closes are treated as a no-op for compatibility.
        let sender = resolve_sender_metadata(self.state.connection(), &header).await;
        let Some(sender_name) = sender.sender_name.as_deref() else {
            return Ok(());
        };
        let owned = {
            let store = self.state.store.lock().await;
            store.is_notification_owned_by(id, sender_name, sender.sender_pid)
        };
        if !owned {
            debug!(
                id,
                sender = sender_name,
                sender_pid = sender.sender_pid,
                "ignoring close for unowned notification"
            );
            return Ok(());
        }
        self.state
            .close_notification(id, CloseReason::ClosedByCall)
            .await
            .map_err(to_fdo_error)
    }

    async fn get_server_information(&self) -> (String, String, String, String) {
        (
            "UnixNotis".to_string(),
            "UnixNotis".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
            "1.2".to_string(),
        )
    }

    #[zbus(signal)]
    pub(crate) async fn notification_closed(
        ctx: &SignalContext<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn action_invoked(
        ctx: &SignalContext<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;
}

fn build_notification(
    app_name: String,
    app_icon: String,
    summary: String,
    body: String,
    actions: Vec<String>,
    hints: HashMap<String, OwnedValue>,
    sender: SenderMetadata,
    expire_timeout: i32,
) -> Notification {
    // Derive common hints first so the UI and rule engine can make decisions.
    let urgency = Urgency::from_hint(hints.get("urgency"));
    let category = hints
        .get("category")
        .and_then(owned_to_string)
        .map(|value| truncate_utf8_bytes(&value, MAX_CATEGORY_BYTES));
    let is_transient = hints
        .get("transient")
        .and_then(|value| bool::try_from(value).ok())
        .unwrap_or(false);
    let is_resident = hints
        .get("resident")
        .and_then(|value| bool::try_from(value).ok())
        .unwrap_or(false);
    let image = NotificationImage::from_hints(&app_name, &app_icon, &hints);

    Notification {
        id: 0,
        app_name: if app_name.is_empty() {
            "Unknown".to_string()
        } else {
            truncate_utf8_bytes(&app_name, MAX_APP_NAME_BYTES)
        },
        app_icon: truncate_utf8_bytes(&app_icon, MAX_APP_ICON_BYTES),
        summary: truncate_utf8_bytes(&summary, MAX_SUMMARY_BYTES),
        body: truncate_utf8_bytes(&body, MAX_BODY_BYTES),
        actions: parse_actions(actions),
        hints: sanitize_hints_for_storage(hints),
        urgency,
        category,
        is_transient,
        is_resident,
        suppress_popup: false,
        suppress_sound: false,
        image,
        expire_timeout,
        received_at: chrono::Utc::now(),
        sender_name: sender.sender_name,
        sender_pid: sender.sender_pid,
        sender_executable: sender.sender_executable,
    }
}

struct SenderMetadata {
    // Unique bus sender name (:1.x) used for compatibility checks
    sender_name: Option<String>,
    // Process id used for reconnect-safe ownership checks
    sender_pid: Option<u32>,
    // Executable path used for diagnostics and spoofing analysis
    sender_executable: Option<String>,
}

async fn resolve_sender_metadata(connection: &Connection, header: &Header<'_>) -> SenderMetadata {
    // Sender details are best-effort; failures must not break notification delivery
    let sender_name = header.sender().map(|sender| sender.as_str().to_string());
    let Some(sender_name_str) = sender_name.as_deref() else {
        return SenderMetadata {
            sender_name,
            sender_pid: None,
            sender_executable: None,
        };
    };
    let Ok(bus_name) = zbus::names::BusName::try_from(sender_name_str) else {
        return SenderMetadata {
            sender_name,
            sender_pid: None,
            sender_executable: None,
        };
    };
    let Ok(proxy) = DBusProxy::new(connection).await else {
        return SenderMetadata {
            sender_name,
            sender_pid: None,
            sender_executable: None,
        };
    };
    // Process metadata comes from the bus owner and cannot be forged by payload fields
    let sender_pid = proxy.get_connection_unix_process_id(bus_name).await.ok();
    let sender_executable = match sender_pid {
        Some(pid) => read_process_executable_path(pid)
            .await
            .map(|path| path.display().to_string()),
        None => None,
    };
    SenderMetadata {
        sender_name,
        sender_pid,
        sender_executable,
    }
}

fn app_name_matches_sender(app_name: &str, sender_executable: &str) -> bool {
    // Match is advisory-only to keep protocol compatibility for apps with custom labels
    let app = app_name.trim().to_ascii_lowercase();
    if app.is_empty() {
        return true;
    }
    let Some(exe_name) = Path::new(sender_executable)
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
    else {
        return true;
    };
    app == exe_name || app.replace(' ', "-") == exe_name || exe_name.contains(&app)
}

#[cfg(target_os = "linux")]
async fn read_process_executable_path(pid: u32) -> Option<std::path::PathBuf> {
    let path = format!("/proc/{pid}/exe");
    tokio::fs::read_link(path).await.ok()
}

#[cfg(not(target_os = "linux"))]
async fn read_process_executable_path(_pid: u32) -> Option<std::path::PathBuf> {
    None
}

fn parse_actions(raw: Vec<String>) -> Vec<Action> {
    let mut actions = Vec::with_capacity(raw.len().min(MAX_ACTIONS));
    let mut iter = raw.into_iter();
    // D-Bus actions arrive as [key, label] pairs; drop any trailing key without a label.
    while let Some(key) = iter.next() {
        if let Some(label) = iter.next() {
            if actions.len() >= MAX_ACTIONS {
                break;
            }
            actions.push(Action {
                key: truncate_utf8_bytes(&key, MAX_ACTION_KEY_BYTES),
                label: truncate_utf8_bytes(&label, MAX_ACTION_LABEL_BYTES),
            });
        }
    }
    actions
}

fn sanitize_hints_for_storage(hints: HashMap<String, OwnedValue>) -> HashMap<String, OwnedValue> {
    let mut sanitized = HashMap::with_capacity(hints.len().min(MAX_HINT_ENTRIES));
    for (key, value) in hints {
        if sanitized.len() >= MAX_HINT_ENTRIES {
            break;
        }
        let key = truncate_utf8_bytes(key.trim(), MAX_HINT_KEY_BYTES);
        if key.is_empty() {
            continue;
        }
        let value = match key.as_str() {
            // Keep only scalar hints that are actively used by sound logic and metadata derivation.
            "sound-name" | "sound-file" | "category" => owned_to_string(&value).and_then(|text| {
                let bounded = truncate_utf8_bytes(&text, MAX_HINT_STRING_BYTES);
                string_to_owned_value(&bounded)
            }),
            "transient" | "resident" | "suppress-sound" => {
                bool::try_from(&value).ok().map(OwnedValue::from)
            }
            "urgency" => parse_urgency_hint(&value).map(OwnedValue::from),
            _ => None,
        };
        if let Some(value) = value {
            sanitized.insert(key, value);
        }
    }
    sanitized
}

fn string_to_owned_value(value: &str) -> Option<OwnedValue> {
    OwnedValue::try_from(Value::from(value)).ok()
}

fn parse_urgency_hint(value: &OwnedValue) -> Option<u32> {
    if let Ok(raw) = u8::try_from(value) {
        return Some((raw as u32).min(2));
    }
    if let Ok(raw) = u32::try_from(value) {
        return Some(raw.min(2));
    }
    None
}

fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> String {
    if max_bytes == 0 {
        return String::new();
    }
    if value.len() <= max_bytes {
        return value.to_string();
    }
    // Backtrack to the nearest UTF-8 boundary so truncation never produces invalid text.
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn resolve_expiration(config: &Config, notification: &Notification) -> Option<Instant> {
    // Explicit timeouts and resident notifications override defaults.
    if notification.expire_timeout == 0 || notification.is_resident {
        return None;
    }

    let timeout_ms = if notification.expire_timeout > 0 {
        notification.expire_timeout as u64
    } else {
        match notification.urgency {
            Urgency::Critical => config.popups.critical_timeout_ms?,
            _ => config.popups.default_timeout_ms,
        }
    };

    if timeout_ms == 0 {
        return None;
    }

    // Convert the resolved timeout into an absolute instant for the scheduler.
    Some(Instant::now() + Duration::from_millis(timeout_ms))
}

fn owned_to_string(value: &OwnedValue) -> Option<String> {
    value
        .try_clone()
        .ok()
        .and_then(|owned| String::try_from(owned).ok())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use zbus::zvariant::OwnedValue;

    use super::{
        build_notification, owned_to_string, parse_actions, sanitize_hints_for_storage,
        string_to_owned_value, truncate_utf8_bytes, SenderMetadata, MAX_ACTIONS, MAX_BODY_BYTES,
        MAX_SUMMARY_BYTES,
    };

    #[test]
    fn truncate_utf8_bytes_preserves_character_boundaries() {
        let value = "abc🙂def";
        let truncated = truncate_utf8_bytes(value, 5);
        assert_eq!(truncated, "abc");
    }

    #[test]
    fn build_notification_clamps_summary_and_body_sizes() {
        let summary = "S".repeat(MAX_SUMMARY_BYTES + 128);
        let body = "B".repeat(MAX_BODY_BYTES + 512);
        let notification = build_notification(
            "app".to_string(),
            "icon".to_string(),
            summary,
            body,
            Vec::new(),
            HashMap::<String, OwnedValue>::new(),
            SenderMetadata {
                sender_name: Some(":1.test".to_string()),
                sender_pid: Some(42),
                sender_executable: Some("/usr/bin/test-app".to_string()),
            },
            0,
        );
        assert!(notification.summary.len() <= MAX_SUMMARY_BYTES);
        assert!(notification.body.len() <= MAX_BODY_BYTES);
    }

    #[test]
    fn parse_actions_caps_pairs() {
        let mut raw = Vec::new();
        for idx in 0..(MAX_ACTIONS + 10) {
            raw.push(format!("key-{idx}"));
            raw.push(format!("label-{idx}"));
        }
        let actions = parse_actions(raw);
        assert_eq!(actions.len(), MAX_ACTIONS);
    }

    #[test]
    fn sanitize_hints_drops_untrusted_and_bounds_strings() {
        let mut hints = HashMap::<String, OwnedValue>::new();
        hints.insert("transient".to_string(), OwnedValue::from(true));
        hints.insert(
            "sound-name".to_string(),
            string_to_owned_value(&"n".repeat(5000)).expect("sound-name"),
        );
        hints.insert("image-data".to_string(), OwnedValue::from(123u32));
        hints.insert(
            "x-custom".to_string(),
            string_to_owned_value("custom").expect("custom"),
        );

        let sanitized = sanitize_hints_for_storage(hints);
        assert_eq!(sanitized.len(), 2);
        assert!(sanitized.contains_key("transient"));
        assert!(sanitized.contains_key("sound-name"));
        let sound_name = owned_to_string(
            sanitized
                .get("sound-name")
                .expect("sound-name should remain"),
        )
        .expect("sound-name should be string");
        assert!(sound_name.len() <= 2048);
    }
}
