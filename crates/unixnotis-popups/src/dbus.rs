//! D-Bus runtime for popup UI events and control updates

// Submodules keep retry policy, seeding, and runtime wiring out of the
// public entrypoint so this file stays easy to scan
mod dbus_backoff;
mod dbus_commands;
mod dbus_runtime;
mod dbus_seed;
mod dbus_types;

pub use dbus_runtime::start_dbus_runtime;
pub use dbus_types::{UiCommand, UiEvent};
