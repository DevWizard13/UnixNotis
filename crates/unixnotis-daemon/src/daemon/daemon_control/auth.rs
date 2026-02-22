//! Authorization helpers for privileged control methods
//!
//! This file isolates caller validation so the interface file can focus on D-Bus methods

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::warn;
use zbus::fdo::DBusProxy;
use zbus::message::Header;

use crate::daemon::{to_fdo_error, DaemonState};

// Only these binaries are allowed to call privileged control methods
const TRUSTED_CONTROL_EXECUTABLES: [&str; 4] = [
    "noticenterctl",
    "unixnotis-center",
    "unixnotis-popups",
    "unixnotis-daemon",
];

pub(super) async fn authorize_control_call(
    state: &Arc<DaemonState>,
    header: &Header<'_>,
    method: &'static str,
) -> zbus::fdo::Result<()> {
    // Reject calls that do not include a sender identity
    let sender = header
        .sender()
        .ok_or_else(|| zbus::fdo::Error::AccessDenied("missing sender".to_string()))?;
    let sender_name = sender.as_str().to_string();

    // Ask the bus for sender metadata so payload fields cannot spoof identity
    let proxy = DBusProxy::new(state.connection())
        .await
        .map_err(to_fdo_error)?;
    let bus_name = zbus::names::BusName::try_from(sender_name.as_str())
        .map_err(|_| zbus::fdo::Error::AccessDenied("invalid sender".to_string()))?;

    // Only the current desktop user can control panel behavior
    let caller_uid = proxy.get_connection_unix_user(bus_name.clone()).await?;
    // SAFETY: `geteuid` is thread-safe and has no memory ownership requirements
    let expected_uid = unsafe { libc::geteuid() };
    if caller_uid != expected_uid {
        warn!(
            method,
            sender = %sender_name,
            uid = caller_uid,
            expected_uid,
            "rejected control caller with mismatched uid"
        );
        return Err(zbus::fdo::Error::AccessDenied(
            "caller uid is not authorized for control operation".to_string(),
        ));
    }

    // The executable path check prevents unrelated same-user programs from control calls
    let pid = proxy.get_connection_unix_process_id(bus_name).await?;
    let exe_path = read_process_executable_path(pid).await;
    if !exe_path
        .as_deref()
        .is_some_and(is_trusted_control_executable_path)
    {
        warn!(
            method,
            sender = %sender_name,
            pid,
            executable = exe_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            "rejected untrusted control caller"
        );
        return Err(zbus::fdo::Error::AccessDenied(
            "caller is not authorized for control operation".to_string(),
        ));
    }

    Ok(())
}

#[cfg(target_os = "linux")]
async fn read_process_executable_path(pid: u32) -> Option<PathBuf> {
    // Linux exposes the real executable path via /proc
    let path = format!("/proc/{pid}/exe");
    tokio::fs::read_link(path).await.ok()
}

#[cfg(not(target_os = "linux"))]
async fn read_process_executable_path(_pid: u32) -> Option<PathBuf> {
    // Non-Linux targets skip executable path authorization
    None
}

pub(super) fn is_trusted_control_executable_path(path: &Path) -> bool {
    // Canonicalize observed path first so comparisons use a stable filesystem identity
    let observed = canonicalize_best_effort(path);
    // Require an exact trusted binary name, not a suffix-alike alias
    let Some(observed_name) = observed.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if !TRUSTED_CONTROL_EXECUTABLES.contains(&observed_name) {
        return false;
    }
    // Candidate set includes only discovered binaries with exact trusted names
    trusted_control_executable_paths().contains(&observed)
}

fn trusted_control_executable_paths() -> std::collections::HashSet<PathBuf> {
    use std::collections::HashSet;

    // Build a deduplicated set so lookups stay O(1)
    let mut candidates = HashSet::new();
    let mut directories = Vec::new();

    // Include the current binary directory for local builds and dev runs
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            directories.push(parent.to_path_buf());
        }
    }
    // Include standard install locations used by packaged builds
    directories.push(PathBuf::from("/usr/local/bin"));
    directories.push(PathBuf::from("/usr/bin"));
    directories.push(PathBuf::from("/bin"));
    // Include common per-user install location used by UnixNotis installer
    if let Some(home) = std::env::var_os("HOME") {
        directories.push(PathBuf::from(home).join(".local/bin"));
    }

    for directory in directories {
        for executable in TRUSTED_CONTROL_EXECUTABLES {
            // Exact name match only; suffix aliases like *.exe are not accepted on Linux
            let candidate = directory.join(executable);
            // Keep only files that exist at discovery time
            if candidate.exists() {
                candidates.insert(canonicalize_best_effort(&candidate));
            }
        }
    }
    candidates
}

fn canonicalize_best_effort(path: &Path) -> PathBuf {
    // If canonicalization fails, compare the original path as a fallback
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
