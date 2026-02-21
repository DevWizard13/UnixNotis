//! Toggle icon resolution helpers
//!
//! Keeps icon fallback policy focused and testable away from widget lifecycle code

use unixnotis_core::ToggleWidgetConfig;

/// Resolves the icon name for a toggle while keeping fallback semantics
/// inside the same domain as the toggle kind
pub(super) fn resolve_toggle_icon_name(config: &ToggleWidgetConfig) -> String {
    // Empty and whitespace-only values are treated the same
    let requested = config.icon.trim();

    // Empty configured icons still get a stable generic symbol
    if requested.is_empty() {
        return "applications-system-symbolic".to_string();
    }

    // Display can be unavailable during early startup windows
    let Some(display) = gtk::gdk::Display::default() else {
        return requested.to_string();
    };
    let theme = gtk::IconTheme::for_display(&display);
    // Kind is inferred once and reused across preference and fallback paths
    let kind = infer_toggle_icon_kind(config, requested);

    // Some themes ship a low-quality airplane-mode glyph while also providing
    // better flight-mode status icons. Prefer the status-style icon family for
    // default airplane requests so visuals stay consistent with other toggles
    if let Some(preferred) = preferred_airplane_icon(kind, requested, &theme) {
        return preferred;
    }

    // Keep configured icon when the active theme has it
    if theme.has_icon(requested) {
        return requested.to_string();
    }

    // Some themes expose only symbolic or only non-symbolic aliases
    if let Some(alias) = resolve_symbolic_alias(requested, &theme) {
        return alias;
    }

    // Infer semantic kind from kind/label/requested name so custom configs still map well
    for fallback in toggle_icon_fallbacks(kind) {
        if theme.has_icon(fallback) {
            return fallback.to_string();
        }
    }

    // Keep original request when no safe semantic fallback exists
    requested.to_string()
}

fn preferred_airplane_icon(
    kind: ToggleIconKind,
    requested: &str,
    theme: &gtk::IconTheme,
) -> Option<String> {
    // Restrict override to airplane toggles only
    if kind != ToggleIconKind::Airplane {
        return None;
    }
    // Respect custom explicit icon names outside default airplane requests
    if !matches!(requested, "airplane-mode-symbolic" | "airplane-mode") {
        return None;
    }

    // Flightmode status icons usually look better with modern theme packs
    for candidate in [
        "network-flightmode-on",
        "flightmode-on",
        "airplane-mode-symbolic",
        "airplane-mode",
    ] {
        if theme.has_icon(candidate) {
            return Some(candidate.to_string());
        }
    }
    None
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToggleIconKind {
    Wifi,
    Bluetooth,
    Airplane,
    Night,
    Unknown,
}

fn resolve_symbolic_alias(requested: &str, theme: &gtk::IconTheme) -> Option<String> {
    // First try dropping -symbolic for themes that only expose full-color names
    if let Some(base) = requested.strip_suffix("-symbolic") {
        if theme.has_icon(base) {
            return Some(base.to_string());
        }
    } else {
        // Then try adding -symbolic for themes that only expose symbolic names
        let symbolic = format!("{requested}-symbolic");
        if theme.has_icon(&symbolic) {
            return Some(symbolic);
        }
    }
    None
}

fn infer_toggle_icon_kind(config: &ToggleWidgetConfig, requested: &str) -> ToggleIconKind {
    // Normalize all signal sources so matching is case-insensitive
    let kind = config
        .kind
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let label = config.label.to_ascii_lowercase();
    let requested = requested.to_ascii_lowercase();
    // Sources include kind, label, and icon to support older custom configs
    let sources = [kind.as_str(), label.as_str(), requested.as_str()];

    if matches_any(&sources, &["bluetooth", "bluez", "network-bluetooth"]) {
        return ToggleIconKind::Bluetooth;
    }
    if matches_any(
        &sources,
        &[
            "airplane",
            "flightmode",
            "flight-mode",
            "flight_mode",
            "network-flightmode",
        ],
    ) {
        return ToggleIconKind::Airplane;
    }
    if matches_any(
        &sources,
        &["night", "sunset", "gamma", "wlsunset", "hyprsunset"],
    ) {
        return ToggleIconKind::Night;
    }
    if matches_any(
        &sources,
        &["wifi", "wi-fi", "wireless", "wlan", "network-wireless"],
    ) {
        return ToggleIconKind::Wifi;
    }
    ToggleIconKind::Unknown
}

fn matches_any(sources: &[&str], needles: &[&str]) -> bool {
    // Substring matching handles values like network-flightmode-on and flight_mode
    sources.iter().copied().any(|source| {
        needles
            .iter()
            .copied()
            .any(|needle| source.contains(needle))
    })
}

fn toggle_icon_fallbacks(kind: ToggleIconKind) -> &'static [&'static str] {
    // Keep candidate ordering deterministic so icon selection remains predictable
    match kind {
        ToggleIconKind::Wifi => &[
            // Prefer explicit signal and connected names before generic fallbacks
            "network-wireless-signal-excellent-symbolic",
            "network-wireless-signal-good-symbolic",
            "network-wireless-symbolic",
            "network-wireless-connected-symbolic",
            "network-wireless-signal-excellent",
            "network-wireless",
            "network-workgroup-symbolic",
            "network-workgroup",
        ],
        ToggleIconKind::Bluetooth => &[
            // Cover Adwaita, Breeze, and mixed custom themes
            "bluetooth-active-symbolic",
            "bluetooth-symbolic",
            "network-bluetooth-symbolic",
            "network-wireless-bluetooth-symbolic",
            "network-bluetooth",
            "preferences-system-bluetooth-activated-symbolic",
            "preferences-system-bluetooth-symbolic",
            "preferences-bluetooth-symbolic",
            "preferences-system-bluetooth",
            "bluetooth",
        ],
        ToggleIconKind::Airplane => &[
            // Prefer explicit airplane/flightmode names to avoid unrelated glyphs
            "airplane-mode-symbolic",
            "airplane-mode-disabled-symbolic",
            "network-flightmode-on",
            "network-flightmode-off",
            "flightmode-on",
            "flightmode-off",
            "airplane-mode",
            "airplane",
            "network-wireless-offline-symbolic",
        ],
        ToggleIconKind::Night => &[
            // Keep night icon semantics close to moon/brightness symbols
            "weather-clear-night-symbolic",
            "weather-clear-night",
            "display-brightness-symbolic",
            "display-brightness",
            "preferences-system-symbolic",
            "preferences-system",
        ],
        ToggleIconKind::Unknown => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::{infer_toggle_icon_kind, toggle_icon_fallbacks, ToggleIconKind};
    use unixnotis_core::ToggleWidgetConfig;

    fn test_toggle(kind: Option<&str>, label: &str, icon: &str) -> ToggleWidgetConfig {
        ToggleWidgetConfig {
            enabled: true,
            kind: kind.map(str::to_string),
            label: label.to_string(),
            icon: icon.to_string(),
            state_cmd: None,
            on_cmd: None,
            off_cmd: None,
            watch_cmd: None,
        }
    }

    #[test]
    fn infer_toggle_icon_kind_uses_kind_label_and_icon_tokens() {
        let bluetooth = test_toggle(None, "Bluetooth", "network-wireless-symbolic");
        assert_eq!(
            infer_toggle_icon_kind(&bluetooth, bluetooth.icon.as_str()),
            ToggleIconKind::Bluetooth
        );

        let airplane = test_toggle(
            Some("flight_mode"),
            "Travel",
            "applications-system-symbolic",
        );
        assert_eq!(
            infer_toggle_icon_kind(&airplane, airplane.icon.as_str()),
            ToggleIconKind::Airplane
        );

        let wifi = test_toggle(None, "Wi-Fi", "network-wireless-signal-excellent-symbolic");
        assert_eq!(
            infer_toggle_icon_kind(&wifi, wifi.icon.as_str()),
            ToggleIconKind::Wifi
        );

        let unknown = test_toggle(None, "Custom", "applications-system-symbolic");
        assert_eq!(
            infer_toggle_icon_kind(&unknown, unknown.icon.as_str()),
            ToggleIconKind::Unknown
        );
    }

    #[test]
    fn bluetooth_fallbacks_include_breeze_family_names() {
        let fallbacks = toggle_icon_fallbacks(ToggleIconKind::Bluetooth);
        assert!(fallbacks.contains(&"network-bluetooth-symbolic"));
        assert!(fallbacks.contains(&"preferences-system-bluetooth-symbolic"));
    }

    #[test]
    fn airplane_fallbacks_prefer_flightmode_family() {
        let fallbacks = toggle_icon_fallbacks(ToggleIconKind::Airplane);
        assert_eq!(fallbacks.first().copied(), Some("airplane-mode-symbolic"));
        assert!(fallbacks.contains(&"network-flightmode-on"));
    }
}
