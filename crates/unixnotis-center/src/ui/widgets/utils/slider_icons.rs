//! Slider icon resolution helpers

pub(super) fn resolve_slider_icon_name(label: &str, requested: &str) -> String {
    // Trim whitespace so edited config values keep resolving consistently
    let requested = requested.trim();
    if requested.is_empty() {
        // Empty icon fields still need a stable rendered glyph
        return "applications-system-symbolic".to_string();
    }

    // During startup there may be no display yet; keep requested name in that case
    let Some(display) = gtk::gdk::Display::default() else {
        return requested.to_string();
    };
    let theme = gtk::IconTheme::for_display(&display);

    // Fast path keeps configured icon unchanged when present in the active theme
    if theme.has_icon(requested) {
        return requested.to_string();
    }

    // Symbolic/non-symbolic aliases are checked before semantic fallbacks
    if let Some(alias) = resolve_symbolic_alias(requested, &theme) {
        return alias;
    }

    // Label and icon text are both used as hints for widget intent
    let label = label.to_ascii_lowercase();
    let requested_lower = requested.to_ascii_lowercase();
    let brightness_hint = label.contains("brightness")
        || requested_lower.contains("brightness")
        || requested_lower.contains("display");
    let volume_hint = label.contains("volume")
        || requested_lower.contains("volume")
        || requested_lower.contains("audio");

    if brightness_hint {
        // Candidate ordering prefers symbolic brightness glyphs for consistent tinting
        for candidate in [
            "display-brightness-symbolic",
            "video-display-brightness-symbolic",
            "display-brightness",
            "video-display-brightness",
            "weather-clear-night-symbolic",
        ] {
            if theme.has_icon(candidate) {
                // First theme hit wins to keep icon choice deterministic
                return candidate.to_string();
            }
        }
    }

    if volume_hint {
        // Candidate ordering prefers symbolic speaker glyphs before non-symbolic fallback
        for candidate in [
            "audio-volume-high-symbolic",
            "audio-volume-medium-symbolic",
            "audio-volume-low-symbolic",
            "audio-volume-muted-symbolic",
            "audio-volume-high",
        ] {
            if theme.has_icon(candidate) {
                // First theme hit wins to keep icon choice deterministic
                return candidate.to_string();
            }
        }
    }

    // Final fallback returns configured name to preserve explicit user intent
    requested.to_string()
}

fn resolve_symbolic_alias(requested: &str, theme: &gtk::IconTheme) -> Option<String> {
    // Drop -symbolic suffix when only full-color icon names are provided
    if let Some(base) = requested.strip_suffix("-symbolic") {
        // Prefer non-symbolic fallback when symbolic variant is missing
        if theme.has_icon(base) {
            return Some(base.to_string());
        }
    } else {
        // Add -symbolic suffix when theme only ships symbolic variants
        let symbolic = format!("{requested}-symbolic");
        if theme.has_icon(&symbolic) {
            return Some(symbolic);
        }
    }
    None
}
