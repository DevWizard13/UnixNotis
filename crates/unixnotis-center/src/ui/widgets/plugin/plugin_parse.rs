//! Parsing logic for external widget plugin payloads.

use serde::Deserialize;
use unixnotis_core::WidgetPluginConfig;

const MAX_STAT_TEXT_CHARS: usize = 256;
const MAX_CARD_TITLE_CHARS: usize = 64;
const MAX_CARD_TEXT_CHARS: usize = 4096;

#[derive(Clone, Copy)]
pub(in crate::ui::widgets) struct PluginOutputLimits {
    pub(in crate::ui::widgets) max_output_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::ui::widgets) struct StatPluginData {
    pub(in crate::ui::widgets) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::ui::widgets) struct CardPluginData {
    pub(in crate::ui::widgets) title: Option<String>,
    pub(in crate::ui::widgets) text: String,
}

#[derive(Deserialize)]
struct StatPayload {
    api_version: u32,
    text: String,
}

#[derive(Deserialize)]
struct CardPayload {
    api_version: u32,
    text: String,
    title: Option<String>,
}

pub(in crate::ui::widgets) fn parse_stat_plugin_payload(
    payload: &[u8],
    limits: PluginOutputLimits,
) -> Result<StatPluginData, String> {
    validate_payload_bounds(payload, limits)?;
    let parsed: StatPayload = serde_json::from_slice(payload)
        .map_err(|err| format!("invalid stat plugin JSON payload: {err}"))?;
    validate_api_version(parsed.api_version)?;

    let text = clamp_chars(parsed.text.trim(), MAX_STAT_TEXT_CHARS);
    if text.is_empty() {
        return Err("stat plugin payload field \"text\" is empty".to_string());
    }
    Ok(StatPluginData { text })
}

pub(in crate::ui::widgets) fn parse_card_plugin_payload(
    payload: &[u8],
    limits: PluginOutputLimits,
) -> Result<CardPluginData, String> {
    validate_payload_bounds(payload, limits)?;
    let parsed: CardPayload = serde_json::from_slice(payload)
        .map_err(|err| format!("invalid card plugin JSON payload: {err}"))?;
    validate_api_version(parsed.api_version)?;

    let text = clamp_chars(parsed.text.trim(), MAX_CARD_TEXT_CHARS);
    if text.is_empty() {
        return Err("card plugin payload field \"text\" is empty".to_string());
    }

    let title = parsed
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| clamp_chars(value, MAX_CARD_TITLE_CHARS));

    Ok(CardPluginData { title, text })
}

fn validate_payload_bounds(payload: &[u8], limits: PluginOutputLimits) -> Result<(), String> {
    if payload.is_empty() {
        return Err("plugin payload is empty".to_string());
    }
    if payload.len() > limits.max_output_bytes {
        return Err(format!(
            "plugin payload exceeded configured max_output_bytes ({} > {})",
            payload.len(),
            limits.max_output_bytes
        ));
    }
    Ok(())
}

fn validate_api_version(version: u32) -> Result<(), String> {
    if version != WidgetPluginConfig::API_VERSION_V1 {
        return Err(format!(
            "unsupported plugin api_version {version}, expected {}",
            WidgetPluginConfig::API_VERSION_V1
        ));
    }
    Ok(())
}

fn clamp_chars(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut out = String::new();
    for ch in value.chars().take(max_chars) {
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stat_payload_accepts_v1() {
        let payload = br#"{"api_version":1,"text":" 42% "}"#;
        let parsed = parse_stat_plugin_payload(
            payload,
            PluginOutputLimits {
                max_output_bytes: 1024,
            },
        )
        .expect("payload should parse");
        assert_eq!(parsed.text, "42%");
    }

    #[test]
    fn parse_stat_payload_rejects_wrong_version() {
        let payload = br#"{"api_version":2,"text":"42%"}"#;
        let err = parse_stat_plugin_payload(
            payload,
            PluginOutputLimits {
                max_output_bytes: 1024,
            },
        )
        .expect_err("version should be rejected");
        assert!(err.contains("unsupported plugin api_version"));
    }

    #[test]
    fn parse_card_payload_accepts_title_override() {
        let payload = br#"{"api_version":1,"title":"Weather","text":"72F, clear"}"#;
        let parsed = parse_card_plugin_payload(
            payload,
            PluginOutputLimits {
                max_output_bytes: 1024,
            },
        )
        .expect("payload should parse");
        assert_eq!(parsed.title.as_deref(), Some("Weather"));
        assert_eq!(parsed.text, "72F, clear");
    }

    #[test]
    fn parse_card_payload_rejects_oversized_payload() {
        let payload = br#"{"api_version":1,"text":"ok"}"#;
        let err = parse_card_plugin_payload(
            payload,
            PluginOutputLimits {
                max_output_bytes: 8,
            },
        )
        .expect_err("payload should exceed limit");
        assert!(err.contains("max_output_bytes"));
    }
}
