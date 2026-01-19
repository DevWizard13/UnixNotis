//! Shared command templates for widget defaults and runtime migrations.

pub(crate) const WIFI_STATE_NMCLI: &str = "nmcli radio wifi";
pub(crate) const WIFI_ON_NMCLI: &str = "nmcli radio wifi on";
pub(crate) const WIFI_OFF_NMCLI: &str = "nmcli radio wifi off";
pub(crate) const WIFI_WATCH_NMCLI: &str = "nmcli -t monitor";

pub(crate) const BLUETOOTH_STATE_BLUETOOTHCTL: &str = "bluetoothctl show";
pub(crate) const BLUETOOTH_STATE_RFKILL: &str =
    "rfkill list bluetooth | awk '/Soft blocked:/ { seen=1; if ($3 != \"no\") bad=1 } END { exit (seen && !bad) ? 0 : 1 }'";
pub(crate) const BLUETOOTH_STATE_SYSTEMCTL: &str = "systemctl is-active bluetooth";
pub(crate) const BLUETOOTH_ON_BLUETOOTHCTL: &str = "bluetoothctl power on";
pub(crate) const BLUETOOTH_OFF_BLUETOOTHCTL: &str = "bluetoothctl power off";
pub(crate) const BLUETOOTH_ON_RFKILL: &str = "rfkill unblock bluetooth";
pub(crate) const BLUETOOTH_OFF_RFKILL: &str = "rfkill block bluetooth";
pub(crate) const BLUETOOTH_ON_SYSTEMCTL: &str = "systemctl start bluetooth";
pub(crate) const BLUETOOTH_OFF_SYSTEMCTL: &str = "systemctl stop bluetooth";
// D-Bus monitor keeps updates flowing without a controlling TTY.
pub(crate) const BLUETOOTH_WATCH_DBUS: &str = "dbus-monitor --system type=signal,sender=org.bluez";
pub(crate) const BLUETOOTH_WATCH_RFKILL: &str = "rfkill event";

pub(crate) const AIRPLANE_STATE_CMD: &str =
    "rfkill list all | awk '/Soft blocked:/ { seen=1; if ($3 != \"yes\") bad=1 } END { exit (seen && !bad) ? 0 : 1 }'";
pub(crate) const AIRPLANE_ON_CMD: &str = "rfkill block all";
pub(crate) const AIRPLANE_OFF_CMD: &str = "rfkill unblock all";
pub(crate) const AIRPLANE_WATCH_CMD: &str = "udevadm monitor --udev --subsystem-match=rfkill";

// Gammastep state is process-based because it lacks a lightweight query mode.
pub(crate) const NIGHT_GAMMASTEP_STATE: &str = "pgrep -x gammastep >/dev/null 2>&1";
pub(crate) const NIGHT_GAMMASTEP_ON: &str =
    "nohup gammastep -m wayland -l 0:0 -t 4500:4500 -P >/dev/null 2>&1 &";
// Reset the gamma ramps after stopping to keep state consistent across restarts.
pub(crate) const NIGHT_GAMMASTEP_OFF: &str =
    "pkill -x gammastep >/dev/null 2>&1; gammastep -x >/dev/null 2>&1";

// Hyprsunset uses Hyprland's CTM protocol, so it is preferred on Hyprland sessions.
// v0.3.x IPC only supports temperature queries, so state is derived from Kelvin output.
// Temperatures <= 5000K are treated as "night" for UI state purposes.
pub(crate) const NIGHT_HYPRSUNSET_STATE: &str =
    "pgrep -x hyprsunset >/dev/null 2>&1 && hyprctl hyprsunset temperature 2>/dev/null | \
     awk 'NF && $1 ~ /^[0-9]+$/ { ok = ($1 <= 5000) ? 1 : 0 } END { exit (ok == 1) ? 0 : 1 }'";
// Prefer IPC for running instances; otherwise start a background hyprsunset process.
pub(crate) const NIGHT_HYPRSUNSET_ON: &str =
    "if pgrep -x hyprsunset >/dev/null 2>&1; then hyprctl hyprsunset temperature 4500; else nohup hyprsunset --temperature 4500 >/dev/null 2>&1 & fi";
// 6000K aligns with hyprsunset defaults and represents a neutral baseline.
pub(crate) const NIGHT_HYPRSUNSET_OFF: &str =
    "if pgrep -x hyprsunset >/dev/null 2>&1; then hyprctl hyprsunset temperature 6000; else exit 0; fi";

// Wlsunset does not expose an IPC, so process presence is the state signal.
pub(crate) const NIGHT_WLSUNSET_STATE: &str = "pgrep -x wlsunset >/dev/null 2>&1";
pub(crate) const NIGHT_WLSUNSET_ON: &str =
    "nohup wlsunset -l 0 -L 0 -t 4500 -T 6500 >/dev/null 2>&1 &";
pub(crate) const NIGHT_WLSUNSET_OFF: &str = "pkill -x wlsunset >/dev/null 2>&1";

pub(crate) const TOGGLE_KIND_WIFI: &str = "wifi";
pub(crate) const TOGGLE_KIND_BLUETOOTH: &str = "bluetooth";
pub(crate) const TOGGLE_KIND_AIRPLANE: &str = "airplane";
pub(crate) const TOGGLE_KIND_NIGHT: &str = "night";
