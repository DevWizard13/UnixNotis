//! Runtime adjustments for configuration defaults.
//!
//! Selects backend commands based on runtime availability.

pub(super) use super::config_runtime_sanitize::sanitize_config;
pub(super) use super::config_runtime_toggles::apply_toggle_backends;
pub(super) use super::config_runtime_widgets::{apply_brightness_backend, apply_volume_backend};
