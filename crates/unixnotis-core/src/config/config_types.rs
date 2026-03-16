//! Configuration types and defaults for UnixNotis.
//!
//! Keeps high-level config categories together and delegates detailed schemas
//! to focused modules for maintainability.

use serde::{Deserialize, Serialize};

use super::config_layout::{PanelConfig, PopupConfig};
use super::config_rules::RuleConfig;
use super::config_theme::ThemeConfig;
use super::config_widgets::WidgetsConfig;

/// Top-level configuration loaded from config.toml.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    // Main config sections
    pub general: GeneralConfig,
    pub inhibit: InhibitConfig,
    pub popups: PopupConfig,
    pub panel: PanelConfig,
    pub history: HistoryConfig,
    pub media: MediaConfig,
    pub widgets: WidgetsConfig,
    pub sound: SoundConfig,
    pub theme: ThemeConfig,
    pub rules: Vec<RuleConfig>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub dnd_default: bool,
    pub log_level: Option<String>,
}

/// Inhibit behavior controls how the daemon handles suppression requests.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct InhibitConfig {
    pub mode: InhibitMode,
}

impl Default for InhibitConfig {
    fn default() -> Self {
        Self {
            mode: InhibitMode::NoPopups,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InhibitMode {
    /// Store notifications but suppress popup rendering.
    NoPopups,
    /// Drop incoming notifications entirely while inhibited.
    DropAll,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct HistoryConfig {
    // Saved items
    pub max_entries: usize,
    // Live items
    pub max_active: usize,
    // Save transient items too
    pub transient_to_history: bool,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            max_entries: 200,
            // Match the daemon cap
            max_active: 12,
            transient_to_history: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct MediaConfig {
    /// Enable the media widget in the notification center.
    pub enabled: bool,
    /// Structural preset for the media card.
    pub layout: MediaLayout,
    /// Include web browser media players.
    pub include_browsers: bool,
    /// Browser-identifying substrings for MPRIS bus names or identities (case-insensitive).
    pub browser_tokens: Vec<String>,
    /// Characters allowed before marquee scrolling begins.
    pub title_char_limit: usize,
    /// Show the source label row above the title.
    pub show_source: bool,
    /// Show the player position text alongside the source label.
    pub show_position: bool,
    /// Allowlist of player identifiers or bus names (case-insensitive substrings).
    #[serde(alias = "whitelist")]
    pub allowlist: Vec<String>,
    /// Denylist of player identifiers or bus names (case-insensitive substrings).
    #[serde(alias = "blacklist")]
    pub denylist: Vec<String>,
    /// Controls which players may trigger remote media artwork fetches.
    pub remote_art_policy: MediaRemoteArtPolicy,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MediaRemoteArtPolicy {
    /// Disable remote artwork fetches for every player.
    Disabled,
    /// Allow remote artwork only for non-browser players.
    NativeOnly,
    /// Allow remote artwork for browsers too.
    BrowsersToo,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MediaLayout {
    /// Existing carousel layout with navigation buttons outside the card.
    Carousel,
    /// Single card layout with nav buttons folded into the transport strip.
    Inline,
    /// Vertical card layout with a separate control strip under the metadata row.
    Stacked,
    /// Wide dashboard layout with a dedicated action rail on the right.
    Showcase,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            layout: MediaLayout::Carousel,
            include_browsers: true,
            browser_tokens: vec![
                "firefox".to_string(),
                "librewolf".to_string(),
                "waterfox".to_string(),
                "floorp".to_string(),
                "brave".to_string(),
                "chromium".to_string(),
                "chrome".to_string(),
                "vivaldi".to_string(),
                "edge".to_string(),
                "opera".to_string(),
                "epiphany".to_string(),
                "midori".to_string(),
                "zen".to_string(),
            ],
            title_char_limit: 32,
            show_source: true,
            show_position: true,
            allowlist: Vec::new(),
            denylist: vec!["playerctld".to_string()],
            // Browsers stay opt-in because webpage metadata can choose artwork URLs.
            remote_art_policy: MediaRemoteArtPolicy::NativeOnly,
        }
    }
}

/// Notification sound behavior.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SoundConfig {
    /// Enables sound playback when the daemon receives notifications.
    pub enabled: bool,
    /// Default named sound from the freedesktop sound theme.
    pub default_name: Option<String>,
    /// Default sound file path, resolves relative to the UnixNotis config dir.
    pub default_file: Option<String>,
    /// Directory containing custom sound files, resolves relative to config dir.
    pub default_dir: Option<String>,
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_name: Some("message-new-instant".to_string()),
            default_file: None,
            default_dir: None,
        }
    }
}
