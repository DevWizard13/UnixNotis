//! Widget configuration types and defaults.
//!
//! Groups sliders, toggles, stats, and cards for maintainability.

use serde::{Deserialize, Serialize};

use super::config_commands;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct WidgetsConfig {
    pub volume: SliderWidgetConfig,
    pub brightness: SliderWidgetConfig,
    pub toggles: Vec<ToggleWidgetConfig>,
    pub stats: Vec<StatWidgetConfig>,
    pub cards: Vec<CardWidgetConfig>,
    pub refresh_interval_ms: u64,
    pub refresh_interval_slow_ms: u64,
}

impl Default for WidgetsConfig {
    fn default() -> Self {
        Self {
            volume: SliderWidgetConfig::default_volume(),
            brightness: SliderWidgetConfig::default_brightness(),
            toggles: vec![
                ToggleWidgetConfig::default_wifi(),
                ToggleWidgetConfig::default_bluetooth(),
                ToggleWidgetConfig::default_airplane(),
                ToggleWidgetConfig::default_night(),
            ],
            stats: vec![
                StatWidgetConfig::default_cpu(),
                StatWidgetConfig::default_memory(),
                StatWidgetConfig::default_battery(),
            ],
            cards: vec![
                CardWidgetConfig::default_calendar(),
                CardWidgetConfig::default_weather(),
            ],
            refresh_interval_ms: 1000,
            refresh_interval_slow_ms: 3000,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct SliderWidgetConfig {
    pub enabled: bool,
    pub label: String,
    pub icon: String,
    pub icon_muted: Option<String>,
    pub get_cmd: String,
    pub set_cmd: String,
    pub toggle_cmd: Option<String>,
    pub watch_cmd: Option<String>,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    /// Controls how numeric command output is interpreted for slider values.
    pub parse_mode: NumericParseMode,
}

impl SliderWidgetConfig {
    // wpctl (PipeWire/WirePlumber CLI) volume commands.
    // These are fast and avoid a shell wrapper when available.
    pub(super) const WPCTL_GET: &'static str = "wpctl get-volume @DEFAULT_AUDIO_SINK@";
    pub(super) const WPCTL_SET: &'static str = "wpctl set-volume @DEFAULT_AUDIO_SINK@ {value}%";
    pub(super) const WPCTL_TOGGLE: &'static str = "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle";

    // pactl (PulseAudio / pipewire-pulse) volume commands.
    // PACTL_GET relies on shell sequencing to capture volume + mute in one call.
    pub(super) const PACTL_GET: &'static str =
        "pactl get-sink-volume @DEFAULT_SINK@; pactl get-sink-mute @DEFAULT_SINK@";
    pub(super) const PACTL_SET: &'static str = "pactl set-sink-volume @DEFAULT_SINK@ {value}%";
    pub(super) const PACTL_TOGGLE: &'static str = "pactl set-sink-mute @DEFAULT_SINK@ toggle";

    // Long-running watcher for audio changes; emits events and stays open.
    // The UI/daemon can listen to this and refresh on demand instead of polling.
    pub(super) const PACTL_WATCH: &'static str = "pactl subscribe";

    fn default_volume() -> Self {
        // Default config for the Volume slider widget.
        // Uses wpctl by default (common on PipeWire setups), with runtime fallback support elsewhere.
        Self {
            enabled: true, // Enabled in the stock config; disable in config to hide.
            label: "Volume".to_string(),
            icon: "audio-volume-high-symbolic".to_string(),
            icon_muted: Some("audio-volume-muted-symbolic".to_string()),

            // Commands are templates; runtime replaces tokens like {value} and default sink placeholders.
            get_cmd: Self::WPCTL_GET.to_string(),
            set_cmd: Self::WPCTL_SET.to_string(),
            toggle_cmd: Some(Self::WPCTL_TOGGLE.to_string()),

            // Watcher is applied at runtime when a supported long-running command is available.
            // Keeping this None in defaults avoids silently configuring a watcher that may not exist.
            watch_cmd: None,

            // Slider range and granularity (UI uses these for adjustment and formatting).
            min: 0.0,
            max: 100.0,
            step: 1.0,
            parse_mode: NumericParseMode::Auto,
        }
    }

    fn default_brightness() -> Self {
        // Default config for the Brightness slider widget.
        // brightnessctl typically supports get/set, but it does not have a universal watch mode.
        Self {
            enabled: true,
            label: "Brightness".to_string(),
            icon: "display-brightness-symbolic".to_string(),
            icon_muted: None,

            // -m outputs machine-readable values; parsing stays stable.
            get_cmd: "brightnessctl -m".to_string(),
            set_cmd: "brightnessctl s {value}%".to_string(),
            toggle_cmd: None,

            // Watch mode is not reliably supported by brightnessctl; leaving this here means
            // spawning may fail and the widget will fall back to polling.
            // Runtime clears invalid watchers, so this value is treated as None.
            watch_cmd: Some("brightnessctl -w".to_string()),

            min: 0.0,
            max: 100.0,
            step: 1.0,
            parse_mode: NumericParseMode::Auto,
        }
    }
}

impl Default for SliderWidgetConfig {
    fn default() -> Self {
        Self::default_volume()
    }
}

#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NumericParseMode {
    /// Uses heuristic parsing for mixed output formats.
    #[default]
    Auto,
    /// Interprets values as percentages without normalization.
    Percent,
    /// Interprets values as 0.0-1.0 ratios and scales to percent.
    Ratio,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ToggleWidgetConfig {
    pub enabled: bool,
    /// Stable identifier for runtime defaults (kept independent from the display label).
    /// Use this to preserve backend selection when the label is customized.
    #[serde(alias = "id")]
    pub kind: Option<String>,
    pub label: String,
    pub icon: String,
    pub state_cmd: Option<String>,
    pub on_cmd: Option<String>,
    pub off_cmd: Option<String>,
    pub watch_cmd: Option<String>,
}

impl ToggleWidgetConfig {
    fn default_wifi() -> Self {
        Self {
            enabled: true,
            kind: Some(config_commands::TOGGLE_KIND_WIFI.to_string()),
            label: "Wi-Fi".to_string(),
            icon: "network-wireless-signal-excellent-symbolic".to_string(),
            state_cmd: Some(config_commands::WIFI_STATE_NMCLI.to_string()),
            on_cmd: Some(config_commands::WIFI_ON_NMCLI.to_string()),
            off_cmd: Some(config_commands::WIFI_OFF_NMCLI.to_string()),
            watch_cmd: Some(config_commands::WIFI_WATCH_NMCLI.to_string()),
        }
    }

    fn default_bluetooth() -> Self {
        Self {
            enabled: true,
            kind: Some(config_commands::TOGGLE_KIND_BLUETOOTH.to_string()),
            label: "Bluetooth".to_string(),
            icon: "bluetooth-active-symbolic".to_string(),
            state_cmd: Some(config_commands::BLUETOOTH_STATE_BLUETOOTHCTL.to_string()),
            on_cmd: Some(config_commands::BLUETOOTH_ON_BLUETOOTHCTL.to_string()),
            off_cmd: Some(config_commands::BLUETOOTH_OFF_BLUETOOTHCTL.to_string()),
            // D-Bus monitoring avoids TTY requirements and updates quickly when BlueZ emits events.
            watch_cmd: Some(config_commands::BLUETOOTH_WATCH_DBUS.to_string()),
        }
    }

    fn default_airplane() -> Self {
        Self {
            enabled: true,
            kind: Some(config_commands::TOGGLE_KIND_AIRPLANE.to_string()),
            label: "Airplane".to_string(),
            icon: "airplane-mode-symbolic".to_string(),
            // Airplane is treated as enabled only when all soft blocks are active.
            state_cmd: Some(config_commands::AIRPLANE_STATE_CMD.to_string()),
            on_cmd: Some(config_commands::AIRPLANE_ON_CMD.to_string()),
            off_cmd: Some(config_commands::AIRPLANE_OFF_CMD.to_string()),
            watch_cmd: Some(config_commands::AIRPLANE_WATCH_CMD.to_string()),
        }
    }

    fn default_night() -> Self {
        Self {
            enabled: true,
            kind: Some(config_commands::TOGGLE_KIND_NIGHT.to_string()),
            label: "Night".to_string(),
            icon: "weather-clear-night-symbolic".to_string(),
            // Default to gammastep; runtime logic swaps to wlsunset when gammastep is missing.
            // A fixed temperature avoids geoclue dependency and keeps the process active.
            // Hyprland sessions prefer hyprsunset at runtime when available.
            state_cmd: Some(config_commands::NIGHT_GAMMASTEP_STATE.to_string()),
            on_cmd: Some(config_commands::NIGHT_GAMMASTEP_ON.to_string()),
            off_cmd: Some(config_commands::NIGHT_GAMMASTEP_OFF.to_string()),
            watch_cmd: None,
        }
    }
}

impl Default for ToggleWidgetConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            kind: None,
            label: "Toggle".to_string(),
            icon: "applications-system-symbolic".to_string(),
            state_cmd: None,
            on_cmd: None,
            off_cmd: None,
            watch_cmd: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct WidgetPluginConfig {
    /// Versioned widget plugin contract.
    pub api_version: u32,
    /// Plugin command executed by the widget worker.
    pub command: String,
    /// Maximum allowed command runtime before timeout (milliseconds).
    pub timeout_ms: u64,
    /// Maximum accepted stdout payload size before parse rejection.
    pub max_output_bytes: usize,
}

impl WidgetPluginConfig {
    pub const API_VERSION_V1: u32 = 1;
    const DEFAULT_TIMEOUT_MS: u64 = 2_000;
    const DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024;
}

impl Default for WidgetPluginConfig {
    fn default() -> Self {
        Self {
            api_version: Self::API_VERSION_V1,
            command: String::new(),
            timeout_ms: Self::DEFAULT_TIMEOUT_MS,
            max_output_bytes: Self::DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct StatWidgetConfig {
    pub enabled: bool,
    pub label: String,
    pub icon: Option<String>,
    pub kind: Option<String>,
    pub cmd: Option<String>,
    /// External plugin source for this stat (preferred over cmd when set).
    pub plugin: Option<WidgetPluginConfig>,
    pub min_height: i32,
}

impl StatWidgetConfig {
    fn default_cpu() -> Self {
        Self {
            enabled: true,
            label: "CPU".to_string(),
            icon: Some("utilities-system-monitor-symbolic".to_string()),
            kind: None,
            cmd: Some("builtin:cpu".to_string()),
            plugin: None,
            min_height: 72,
        }
    }

    fn default_memory() -> Self {
        Self {
            enabled: true,
            label: "RAM".to_string(),
            icon: Some("drive-harddisk-symbolic".to_string()),
            kind: None,
            cmd: Some("builtin:memory".to_string()),
            plugin: None,
            min_height: 72,
        }
    }

    fn default_battery() -> Self {
        Self {
            enabled: true,
            label: "Battery".to_string(),
            icon: Some("battery-full-symbolic".to_string()),
            kind: None,
            cmd: Some("builtin:battery".to_string()),
            plugin: None,
            min_height: 72,
        }
    }
}

impl Default for StatWidgetConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            label: "Stat".to_string(),
            icon: None,
            kind: None,
            cmd: None,
            plugin: None,
            min_height: 72,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct CardWidgetConfig {
    pub enabled: bool,
    pub kind: Option<String>,
    pub title: String,
    pub subtitle: Option<String>,
    pub icon: Option<String>,
    pub cmd: Option<String>,
    /// External plugin source for this card (preferred over cmd when set).
    pub plugin: Option<WidgetPluginConfig>,
    pub min_height: i32,
    pub monospace: bool,
}

impl CardWidgetConfig {
    fn default_calendar() -> Self {
        Self {
            enabled: true,
            kind: Some("calendar".to_string()),
            title: "Calendar".to_string(),
            subtitle: None,
            icon: Some("x-office-calendar-symbolic".to_string()),
            cmd: None,
            plugin: None,
            min_height: 180,
            monospace: false,
        }
    }

    fn default_weather() -> Self {
        Self {
            enabled: true,
            kind: Some("weather".to_string()),
            title: "Weather".to_string(),
            subtitle: Some("No data".to_string()),
            icon: Some("weather-clear-symbolic".to_string()),
            cmd: None,
            plugin: None,
            min_height: 160,
            monospace: false,
        }
    }
}

impl Default for CardWidgetConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            kind: None,
            title: "Card".to_string(),
            subtitle: None,
            icon: None,
            cmd: None,
            plugin: None,
            min_height: 120,
            monospace: false,
        }
    }
}
