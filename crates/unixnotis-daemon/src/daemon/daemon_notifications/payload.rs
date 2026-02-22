//! Payload construction and sanitization for notifications
//!
//! This module turns raw D-Bus values into bounded internal model values

use std::collections::HashMap;
use std::time::{Duration, Instant};

use unixnotis_core::{Action, Config, Notification, NotificationImage, Urgency};
use zbus::zvariant::{OwnedValue, Value};

use super::limits::{
    MAX_ACTIONS, MAX_ACTION_KEY_BYTES, MAX_ACTION_LABEL_BYTES, MAX_APP_ICON_BYTES,
    MAX_APP_NAME_BYTES, MAX_BODY_BYTES, MAX_CATEGORY_BYTES, MAX_HINT_ENTRIES, MAX_HINT_KEY_BYTES,
    MAX_HINT_STRING_BYTES, MAX_SUMMARY_BYTES,
};
use super::sender::SenderMetadata;

// Unbroken tokens longer than this are folded with an ellipsis to avoid UI overflow spikes.
const MAX_CONTIGUOUS_TOKEN_CHARS: usize = 96;

pub(super) struct NotificationInput {
    pub(super) app_name: String,
    pub(super) app_icon: String,
    pub(super) summary: String,
    pub(super) body: String,
    pub(super) actions: Vec<String>,
    pub(super) hints: HashMap<String, OwnedValue>,
    pub(super) sender: SenderMetadata,
    pub(super) expire_timeout: i32,
}

pub(super) fn build_notification(input: NotificationInput) -> Notification {
    let NotificationInput {
        app_name,
        app_icon,
        summary,
        body,
        actions,
        hints,
        sender,
        expire_timeout,
    } = input;

    // Derive commonly used metadata before consuming `hints`
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
            // Keep explicit fallback text for empty callers
            "Unknown".to_string()
        } else {
            truncate_utf8_bytes(&app_name, MAX_APP_NAME_BYTES)
        },
        app_icon: truncate_utf8_bytes(&app_icon, MAX_APP_ICON_BYTES),
        // Truncate bytes first, then fold long contiguous runs to keep UTF-8 boundaries valid
        // Fold very long unbroken runs so renderer width remains bounded.
        summary: normalize_text_for_layout(
            &truncate_utf8_bytes(&summary, MAX_SUMMARY_BYTES),
            MAX_CONTIGUOUS_TOKEN_CHARS,
        ),
        // Apply the same order for body so renderer sees consistent text constraints
        // Body can be much larger, so apply the same run-folding protection here.
        body: normalize_text_for_layout(
            &truncate_utf8_bytes(&body, MAX_BODY_BYTES),
            MAX_CONTIGUOUS_TOKEN_CHARS,
        ),
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

pub(super) fn resolve_expiration(config: &Config, notification: &Notification) -> Option<Instant> {
    // Explicit timeout=0 and resident notifications never auto-expire
    if notification.expire_timeout == 0 || notification.is_resident {
        return None;
    }

    // Positive values are caller-provided milliseconds
    let timeout_ms = if notification.expire_timeout > 0 {
        notification.expire_timeout as u64
    } else {
        // Negative values request defaults by urgency
        match notification.urgency {
            Urgency::Critical => config.popups.critical_timeout_ms?,
            _ => config.popups.default_timeout_ms,
        }
    };

    if timeout_ms == 0 {
        return None;
    }

    Some(Instant::now() + Duration::from_millis(timeout_ms))
}

fn parse_actions(raw: Vec<String>) -> Vec<Action> {
    let mut actions = Vec::with_capacity(raw.len().min(MAX_ACTIONS));
    let mut iter = raw.into_iter();

    // The protocol sends actions as [key, label, key, label, ...]
    while let Some(key) = iter.next() {
        if let Some(label) = iter.next() {
            if actions.len() >= MAX_ACTIONS {
                // Hard stop keeps button rows bounded even when sender floods action pairs
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
    // Pre-sizing avoids rehash churn on adversarial hint fanout
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
            // Keep only hints that matter for daemon behavior and rendering
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
    // Accept both byte and integer variants from mixed clients
    if let Ok(raw) = u8::try_from(value) {
        return Some((raw as u32).min(2));
    }
    if let Ok(raw) = u32::try_from(value) {
        return Some(raw.min(2));
    }
    None
}

fn owned_to_string(value: &OwnedValue) -> Option<String> {
    value
        .try_clone()
        .ok()
        .and_then(|owned| String::try_from(owned).ok())
}

fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> String {
    if max_bytes == 0 {
        return String::new();
    }
    if value.len() <= max_bytes {
        // Fast path for common short payloads
        return value.to_string();
    }

    // Move backward until UTF-8 stays valid
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn normalize_text_for_layout(value: &str, max_contiguous: usize) -> String {
    if value.is_empty() || max_contiguous == 0 {
        return value.to_string();
    }

    // Reserve original length to keep this pass allocation-stable on long input
    let mut out = String::with_capacity(value.len());
    let mut run_len = 0usize;
    let mut folded_run = false;

    // Walk characters so non-ASCII content remains valid after normalization.
    for ch in value.chars() {
        if ch.is_whitespace() {
            // Whitespace resets contiguous-run accounting
            run_len = 0;
            folded_run = false;
            out.push(ch);
            continue;
        }

        if run_len < max_contiguous {
            out.push(ch);
            run_len += 1;
            continue;
        }

        // Add one ellipsis when a contiguous token crosses the safety threshold.
        if !folded_run {
            out.push('…');
            folded_run = true;
        }
        // Remaining chars in this run are dropped until whitespace appears again
    }

    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use zbus::zvariant::OwnedValue;

    use super::{
        build_notification, normalize_text_for_layout, owned_to_string, parse_actions,
        sanitize_hints_for_storage, string_to_owned_value, truncate_utf8_bytes, NotificationInput,
        SenderMetadata, MAX_ACTIONS, MAX_BODY_BYTES, MAX_SUMMARY_BYTES,
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

        let notification = build_notification(NotificationInput {
            app_name: "app".to_string(),
            app_icon: "icon".to_string(),
            summary,
            body,
            actions: Vec::new(),
            hints: HashMap::<String, OwnedValue>::new(),
            sender: SenderMetadata {
                sender_name: Some(":1.test".to_string()),
                sender_pid: Some(42),
                sender_executable: Some("/usr/bin/test-app".to_string()),
            },
            expire_timeout: 0,
        });

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

    #[test]
    fn normalize_text_for_layout_folds_long_unbroken_tokens() {
        let input = "x".repeat(200);
        let normalized = normalize_text_for_layout(&input, 96);
        assert!(normalized.contains('…'));
        let longest = normalized
            .split_whitespace()
            .map(|part| part.chars().filter(|ch| ch.is_ascii_alphanumeric()).count())
            .max()
            .unwrap_or(0);
        assert!(longest <= 96);
    }
}
