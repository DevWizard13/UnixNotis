//! External widget plugin parsing helpers.
//!
//! Keeps versioned plugin contract handling separate from widget rendering code.

mod plugin_parse;

pub(in crate::ui::widgets) use plugin_parse::{
    parse_card_plugin_payload, parse_stat_plugin_payload, PluginOutputLimits,
};
