//! D-Bus server for com.unixnotis.Control.
//!
//! Provides panel control, state queries, and inhibitor management.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::stream::{self, StreamExt, TryStreamExt};
use tracing::warn;
use unixnotis_core::{
    CloseReason, ControlState, InhibitorInfo, NotificationView, PanelDebugLevel, PanelRequest,
    CONTROL_OBJECT_PATH, INHIBIT_SCOPE_ALL, INHIBIT_SCOPE_POPUPS,
};
use zbus::fdo::DBusProxy;
use zbus::message::Header;
use zbus::{interface, SignalContext};

use super::{to_fdo_error, DaemonState, NotificationServer, NOTIFICATIONS_OBJECT_PATH};

/// D-Bus server for com.unixnotis.Control.
pub struct ControlServer {
    state: Arc<DaemonState>,
}

const CLEAR_ALL_CONCURRENCY: usize = 64;
// Restrict privileged control calls to known UnixNotis frontends.
const TRUSTED_CONTROL_EXECUTABLES: [&str; 4] = [
    "noticenterctl",
    "unixnotis-center",
    "unixnotis-popups",
    "unixnotis-daemon",
];
const MAX_INHIBITOR_REASON_BYTES: usize = 256;
const MAX_ACTIVE_INHIBITORS: u32 = 128;

impl ControlServer {
    pub fn new(state: Arc<DaemonState>) -> Self {
        Self { state }
    }

    async fn authorize_control_call(
        &self,
        header: &Header<'_>,
        method: &'static str,
    ) -> zbus::fdo::Result<()> {
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::AccessDenied("missing sender".to_string()))?;
        let sender_name = sender.as_str().to_string();
        let proxy = DBusProxy::new(self.state.connection())
            .await
            .map_err(to_fdo_error)?;
        let bus_name = zbus::names::BusName::try_from(sender_name.as_str())
            .map_err(|_| zbus::fdo::Error::AccessDenied("invalid sender".to_string()))?;
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
}

#[interface(name = "com.unixnotis.Control")]
impl ControlServer {
    async fn get_state(&self) -> ControlState {
        let store = self.state.store.lock().await;
        // Read cached inhibitor values to keep state queries constant-time.
        ControlState {
            dnd_enabled: store.dnd_enabled(),
            history_count: store.history_len() as u32,
            inhibited: store.inhibited(),
            inhibitor_count: store.inhibitor_count(),
        }
    }

    async fn list_active(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<Vec<NotificationView>> {
        self.authorize_control_call(&header, "ListActive").await?;
        let store = self.state.store.lock().await;
        // Active list is used for popup seeding and panel hydration.
        Ok(store.list_active())
    }

    async fn list_history(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<Vec<NotificationView>> {
        self.authorize_control_call(&header, "ListHistory").await?;
        let store = self.state.store.lock().await;
        // History list is used for panel hydration and pagination.
        Ok(store.list_history())
    }

    async fn open_panel(&self, #[zbus(header)] header: Header<'_>) -> zbus::fdo::Result<()> {
        self.authorize_control_call(&header, "OpenPanel").await?;
        let ctx = SignalContext::new(self.state.connection(), CONTROL_OBJECT_PATH)
            .map_err(to_fdo_error)?;
        // Use a signal to keep UI and daemon loosely coupled.
        ControlServer::panel_requested(&ctx, PanelRequest::open())
            .await
            .map_err(to_fdo_error)
    }

    async fn open_panel_debug(
        &self,
        level: PanelDebugLevel,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        self.authorize_control_call(&header, "OpenPanelDebug")
            .await?;
        let ctx = SignalContext::new(self.state.connection(), CONTROL_OBJECT_PATH)
            .map_err(to_fdo_error)?;
        // Debug open keeps the same panel request path with extra verbosity metadata.
        ControlServer::panel_requested(&ctx, PanelRequest::open_debug(level))
            .await
            .map_err(to_fdo_error)
    }

    async fn close_panel(&self, #[zbus(header)] header: Header<'_>) -> zbus::fdo::Result<()> {
        self.authorize_control_call(&header, "ClosePanel").await?;
        let ctx = SignalContext::new(self.state.connection(), CONTROL_OBJECT_PATH)
            .map_err(to_fdo_error)?;
        // Close is a signal so the UI can apply its own visibility rules.
        ControlServer::panel_requested(&ctx, PanelRequest::close())
            .await
            .map_err(to_fdo_error)
    }

    async fn toggle_panel(&self, #[zbus(header)] header: Header<'_>) -> zbus::fdo::Result<()> {
        self.authorize_control_call(&header, "TogglePanel").await?;
        let ctx = SignalContext::new(self.state.connection(), CONTROL_OBJECT_PATH)
            .map_err(to_fdo_error)?;
        // Toggle is emitted as a request to avoid tight coupling to UI state.
        ControlServer::panel_requested(&ctx, PanelRequest::toggle())
            .await
            .map_err(to_fdo_error)
    }

    async fn set_dnd(
        &self,
        enabled: bool,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        self.authorize_control_call(&header, "SetDnd").await?;
        let (changed, persist) = {
            let mut store = self.state.store.lock().await;
            store.set_dnd(enabled)
        };
        if let Some(store) = persist {
            if let Err(err) = store.persist(enabled) {
                warn!(?err, "failed to persist do-not-disturb state");
            }
        }
        if changed {
            // Emit state updates only when the value changes to avoid log noise.
            self.state
                .emit_state_changed()
                .await
                .map_err(to_fdo_error)?;
        }
        Ok(())
    }

    async fn inhibit(
        &self,
        reason: &str,
        scope: u32,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<u64> {
        self.authorize_control_call(&header, "Inhibit").await?;
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("missing sender".to_string()))?;
        let normalized_scope = normalize_inhibit_scope(scope)?;
        let sanitized_reason = sanitize_inhibit_reason(reason);
        // Track inhibitors by unique bus name so cleanup on disconnect is reliable.
        let (id, active, count) = {
            let mut store = self.state.store.lock().await;
            if store.inhibitor_count() >= MAX_ACTIVE_INHIBITORS {
                return Err(zbus::fdo::Error::Failed(format!(
                    "inhibitor limit reached ({MAX_ACTIVE_INHIBITORS})"
                )));
            }
            let id = store.add_inhibitor(sender.to_string(), sanitized_reason, normalized_scope);
            let active = store.inhibited();
            let count = store.inhibitor_count();
            (id, active, count)
        };
        let ctx = SignalContext::new(self.state.connection(), CONTROL_OBJECT_PATH)
            .map_err(to_fdo_error)?;
        // Emit inhibitors_changed before state_changed so UIs can update counts immediately.
        ControlServer::inhibitors_changed(&ctx, active, count)
            .await
            .map_err(to_fdo_error)?;
        self.state
            .emit_state_changed()
            .await
            .map_err(to_fdo_error)?;
        Ok(id)
    }

    async fn uninhibit(
        &self,
        id: u64,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("missing sender".to_string()))?;
        let owner = sender.to_string();
        // Owner checks prevent one client from clearing another client's inhibitor.
        let (removed, active, count) = {
            let mut store = self.state.store.lock().await;
            match store.remove_inhibitor(id, &owner) {
                Ok(removed) => {
                    let active = store.inhibited();
                    let count = store.inhibitor_count();
                    (removed, active, count)
                }
                Err(err) => {
                    return Err(zbus::fdo::Error::AccessDenied(err.message()));
                }
            }
        };
        if !removed {
            // Unknown IDs are treated as a no-op to keep clients resilient.
            return Ok(());
        }
        let ctx = SignalContext::new(self.state.connection(), CONTROL_OBJECT_PATH)
            .map_err(to_fdo_error)?;
        // Broadcast inhibitor updates so UI clients can refresh badges.
        ControlServer::inhibitors_changed(&ctx, active, count)
            .await
            .map_err(to_fdo_error)?;
        self.state
            .emit_state_changed()
            .await
            .map_err(to_fdo_error)?;
        Ok(())
    }

    async fn list_inhibitors(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<Vec<InhibitorInfo>> {
        self.authorize_control_call(&header, "ListInhibitors")
            .await?;
        let store = self.state.store.lock().await;
        // Returned list is already sorted for deterministic output.
        Ok(store.list_inhibitors())
    }

    async fn dismiss(&self, id: u32, #[zbus(header)] header: Header<'_>) -> zbus::fdo::Result<()> {
        self.authorize_control_call(&header, "Dismiss").await?;
        self.state
            .dismiss_from_panel(id)
            .await
            .map_err(to_fdo_error)
    }

    async fn invoke_action(
        &self,
        id: u32,
        action_key: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        self.authorize_control_call(&header, "InvokeAction").await?;
        let ctx = SignalContext::new(self.state.connection(), NOTIFICATIONS_OBJECT_PATH)
            .map_err(to_fdo_error)?;
        // Action signals re-use the freedesktop notification interface path.
        NotificationServer::action_invoked(&ctx, id, action_key)
            .await
            .map_err(to_fdo_error)
    }

    async fn clear_all(&self, #[zbus(header)] header: Header<'_>) -> zbus::fdo::Result<()> {
        self.authorize_control_call(&header, "ClearAll").await?;
        // Drain active notifications in one lock to avoid quadratic scans.
        let ids = {
            let mut store = self.state.store.lock().await;
            let ids = store.drain_active_ids();
            store.clear_history();
            ids
        };
        if ids.is_empty() {
            return self.state.emit_state_changed().await.map_err(to_fdo_error);
        }
        let notif_ctx = SignalContext::new(self.state.connection(), NOTIFICATIONS_OBJECT_PATH)
            .map_err(to_fdo_error)?;
        let control_ctx = SignalContext::new(self.state.connection(), CONTROL_OBJECT_PATH)
            .map_err(to_fdo_error)?;
        // Emit close signals with a bounded concurrency limit to avoid task spikes.
        stream::iter(ids)
            .map(|id| {
                let notif_ctx = notif_ctx.clone();
                let control_ctx = control_ctx.clone();
                async move {
                    NotificationServer::notification_closed(
                        &notif_ctx,
                        id,
                        CloseReason::DismissedByUser as u32,
                    )
                    .await?;
                    ControlServer::notification_closed(
                        &control_ctx,
                        id,
                        CloseReason::DismissedByUser,
                    )
                    .await?;
                    Ok::<(), zbus::Error>(())
                }
            })
            .buffer_unordered(CLEAR_ALL_CONCURRENCY)
            .try_for_each(|_| async { Ok(()) })
            .await
            .map_err(to_fdo_error)?;
        self.state.emit_state_changed().await.map_err(to_fdo_error)
    }

    #[zbus(signal)]
    pub(crate) async fn notification_added(
        ctx: &SignalContext<'_>,
        notification: NotificationView,
        show_popup: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn notification_updated(
        ctx: &SignalContext<'_>,
        notification: NotificationView,
        show_popup: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn notification_closed(
        ctx: &SignalContext<'_>,
        id: u32,
        reason: CloseReason,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn state_changed(
        ctx: &SignalContext<'_>,
        state: ControlState,
    ) -> zbus::Result<()>;

    /// Emitted when inhibitor state toggles or count changes.
    #[zbus(signal)]
    pub(crate) async fn inhibitors_changed(
        ctx: &SignalContext<'_>,
        active: bool,
        count: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(crate) async fn panel_requested(
        ctx: &SignalContext<'_>,
        request: PanelRequest,
    ) -> zbus::Result<()>;
}

pub async fn spawn_inhibitor_owner_watch(state: Arc<DaemonState>) -> zbus::Result<()> {
    let proxy = DBusProxy::new(state.connection()).await?;
    let mut stream = proxy.receive_name_owner_changed().await?;
    tokio::spawn(async move {
        while let Some(signal) = stream.next().await {
            let args = match signal.args() {
                Ok(args) => args,
                Err(err) => {
                    warn!(?err, "failed to decode NameOwnerChanged args");
                    continue;
                }
            };
            if args.new_owner().is_some() {
                continue;
            }
            let owner = args.name().to_string();
            // Drop inhibitors for disconnected clients to avoid stale suppression.
            let (changed, active, count) = {
                let mut store = state.store.lock().await;
                let changed = store.remove_inhibitors_by_owner(&owner);
                let active = store.inhibited();
                let count = store.inhibitor_count();
                (changed, active, count)
            };
            if !changed {
                continue;
            }
            let ctx = match SignalContext::new(state.connection(), CONTROL_OBJECT_PATH) {
                Ok(ctx) => ctx,
                Err(err) => {
                    warn!(?err, "failed to build signal context for inhibitor cleanup");
                    continue;
                }
            };
            if let Err(err) = ControlServer::inhibitors_changed(&ctx, active, count).await {
                warn!(
                    ?err,
                    "failed to emit inhibitors_changed after owner disconnect"
                );
            }
            if let Err(err) = state.emit_state_changed().await {
                warn!(?err, "failed to emit state_changed after owner disconnect");
            }
        }
    });
    Ok(())
}

#[cfg(target_os = "linux")]
async fn read_process_executable_path(pid: u32) -> Option<PathBuf> {
    // Resolve /proc/<pid>/exe so authorization is based on the real executable name.
    let path = format!("/proc/{pid}/exe");
    tokio::fs::read_link(path).await.ok()
}

#[cfg(not(target_os = "linux"))]
async fn read_process_executable_path(_pid: u32) -> Option<PathBuf> {
    None
}

fn is_trusted_control_executable_path(path: &Path) -> bool {
    let observed = canonicalize_best_effort(path);
    trusted_control_executable_paths()
        .into_iter()
        .any(|candidate| canonicalize_best_effort(&candidate) == observed)
}

fn trusted_control_executable_paths() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            directories.push(parent.to_path_buf());
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            directories.push(PathBuf::from(home).join(".local").join("bin"));
        }
    }
    directories.push(PathBuf::from("/usr/local/bin"));
    directories.push(PathBuf::from("/usr/bin"));
    directories.push(PathBuf::from("/bin"));

    let mut candidates = Vec::new();
    for directory in directories {
        for executable in TRUSTED_CONTROL_EXECUTABLES {
            candidates.push(directory.join(executable));
            candidates.push(directory.join(format!("{executable}.exe")));
        }
    }
    candidates
}

fn canonicalize_best_effort(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn sanitize_inhibit_reason(reason: &str) -> String {
    let trimmed = reason.trim();
    if trimmed.is_empty() {
        return "manual".to_string();
    }
    truncate_utf8_bytes(trimmed, MAX_INHIBITOR_REASON_BYTES)
}

fn normalize_inhibit_scope(scope: u32) -> zbus::fdo::Result<u32> {
    if scope == INHIBIT_SCOPE_ALL {
        return Ok(INHIBIT_SCOPE_ALL);
    }
    let normalized = scope & INHIBIT_SCOPE_POPUPS;
    if normalized == 0 {
        return Err(zbus::fdo::Error::Failed(
            "unsupported inhibit scope".to_string(),
        ));
    }
    Ok(normalized)
}

fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> String {
    if max_bytes == 0 {
        return String::new();
    }
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use unixnotis_core::{INHIBIT_SCOPE_ALL, INHIBIT_SCOPE_POPUPS};

    use super::{
        is_trusted_control_executable_path, normalize_inhibit_scope, sanitize_inhibit_reason,
    };

    #[test]
    fn trusted_control_executable_paths_in_known_dirs() {
        let current_dir = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
            .expect("current exe parent");
        assert!(is_trusted_control_executable_path(
            &current_dir.join("noticenterctl")
        ));
        assert!(is_trusted_control_executable_path(
            &current_dir.join("unixnotis-center")
        ));
        assert!(is_trusted_control_executable_path(
            &current_dir.join("unixnotis-popups")
        ));
    }

    #[test]
    fn rejects_unknown_or_untrusted_paths() {
        assert!(!is_trusted_control_executable_path(Path::new(
            "/tmp/noticenterctl"
        )));
        assert!(!is_trusted_control_executable_path(Path::new(
            "/usr/bin/python3"
        )));
    }

    #[test]
    fn sanitize_inhibit_reason_trims_and_bounds() {
        assert_eq!(sanitize_inhibit_reason("   "), "manual");
        let long = format!("{}🙂", "a".repeat(512));
        let bounded = sanitize_inhibit_reason(&long);
        assert!(bounded.len() <= 256);
    }

    #[test]
    fn normalize_inhibit_scope_accepts_supported_values() {
        assert_eq!(
            normalize_inhibit_scope(INHIBIT_SCOPE_ALL).expect("scope"),
            0
        );
        assert_eq!(
            normalize_inhibit_scope(INHIBIT_SCOPE_POPUPS).expect("scope"),
            INHIBIT_SCOPE_POPUPS
        );
        assert!(normalize_inhibit_scope(2).is_err());
    }
}
