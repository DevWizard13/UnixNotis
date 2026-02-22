//! D-Bus server for com.unixnotis.Control.
//!
//! Provides panel control, state queries, and inhibitor management.

use std::sync::Arc;

use futures_util::stream::{self, StreamExt};
use tracing::warn;
use unixnotis_core::{
    CloseReason, ControlState, InhibitorInfo, NotificationView, PanelDebugLevel, PanelRequest,
    CONTROL_OBJECT_PATH,
};
use zbus::message::Header;
use zbus::{interface, SignalContext};

use super::{to_fdo_error, DaemonState, NotificationServer, NOTIFICATIONS_OBJECT_PATH};

// Split auth logic out so this file stays focused on control interface behavior
#[path = "daemon_control/auth.rs"]
mod auth;
// Split input normalization out so validation is shared and easy to test
#[path = "daemon_control/sanitize.rs"]
mod sanitize;
// Split owner-watch logic out so background cleanup code is isolated
#[path = "daemon_control/watch.rs"]
mod watch;

/// D-Bus server for com.unixnotis.Control.
pub struct ControlServer {
    // Shared daemon state used by all control methods
    state: Arc<DaemonState>,
}

// Keep clear-all signal fanout bounded to avoid a burst of tiny tasks
const CLEAR_ALL_CONCURRENCY: usize = 64;
// Cap inhibitor count so memory use stays bounded even under abusive clients
const MAX_ACTIVE_INHIBITORS: u32 = 128;

impl ControlServer {
    pub fn new(state: Arc<DaemonState>) -> Self {
        // Lightweight wrapper around the shared daemon state
        Self { state }
    }

    async fn authorize_control_call(
        &self,
        header: &Header<'_>,
        method: &'static str,
    ) -> zbus::fdo::Result<()> {
        // Keep access control centralized so every privileged method uses the same rules.
        auth::authorize_control_call(&self.state, header, method).await
    }
}

#[interface(name = "com.unixnotis.Control")]
impl ControlServer {
    async fn get_state(&self) -> ControlState {
        // Single lock read keeps state snapshot internally consistent
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
        // Guard against untrusted callers before reading any notification content
        self.authorize_control_call(&header, "ListActive").await?;
        let store = self.state.store.lock().await;
        // Active list is used for popup seeding and panel hydration.
        Ok(store.list_active())
    }

    async fn list_history(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<Vec<NotificationView>> {
        // History can contain sensitive content, so it uses the same auth gate
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
        // Capture the previous state so persistence failures can roll back cleanly
        let (changed, persist, previous) = {
            let mut store = self.state.store.lock().await;
            let previous = store.dnd_enabled();
            let (changed, persist) = store.set_dnd(enabled);
            (changed, persist, previous)
        };
        if let Some(store) = persist {
            // Persist outside the main store lock to avoid blocking notify paths on I/O
            if let Err(err) = store.persist(enabled) {
                warn!(?err, "failed to persist do-not-disturb state");
                // Revert in-memory state so success always implies durable state.
                let mut state = self.state.store.lock().await;
                let _ = state.set_dnd(previous);
                return Err(zbus::fdo::Error::Failed(
                    "failed to persist do-not-disturb state".to_string(),
                ));
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
        // Convert incoming values into safe canonical values before storing.
        let normalized_scope = sanitize::normalize_inhibit_scope(scope)?;
        let sanitized_reason = sanitize::sanitize_inhibit_reason(reason);
        // Track inhibitors by unique bus name so cleanup on disconnect is reliable.
        let (id, active, count) = {
            let mut store = self.state.store.lock().await;
            if store.inhibitor_count() >= MAX_ACTIVE_INHIBITORS {
                // Hard cap blocks unbounded growth from accidental loops or hostile callers
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
        // Uninhibit trusts ownership on the bus sender, not executable allowlists
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
        // Delegate to shared state helper so all close signals stay consistent
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
        // Reuse the freedesktop action signal path for compatibility with listeners
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
            // Keep clear-all semantics simple: active + history are both reset
            store.clear_history();
            ids
        };
        if ids.is_empty() {
            if let Err(err) = self.state.emit_state_changed().await {
                warn!(?err, "failed to emit state_changed after clear_all");
            }
            return Ok(());
        }
        let notif_ctx = SignalContext::new(self.state.connection(), NOTIFICATIONS_OBJECT_PATH).ok();
        let control_ctx = SignalContext::new(self.state.connection(), CONTROL_OBJECT_PATH).ok();
        if notif_ctx.is_none() || control_ctx.is_none() {
            // Local store mutation already happened, so signal errors are logged but non-fatal
            warn!("failed to build signal context for clear_all; continuing with local state");
        }
        // Emit close signals with a bounded concurrency limit to avoid task spikes.
        stream::iter(ids)
            .for_each_concurrent(CLEAR_ALL_CONCURRENCY, move |id| {
                let notif_ctx = notif_ctx.clone();
                let control_ctx = control_ctx.clone();
                async move {
                    if let Some(notif_ctx) = notif_ctx.as_ref() {
                        if let Err(err) = NotificationServer::notification_closed(
                            notif_ctx,
                            id,
                            CloseReason::DismissedByUser as u32,
                        )
                        .await
                        {
                            warn!(
                                ?err,
                                id, "failed to emit notification_closed during clear_all"
                            );
                        }
                    }
                    if let Some(control_ctx) = control_ctx.as_ref() {
                        if let Err(err) = ControlServer::notification_closed(
                            control_ctx,
                            id,
                            CloseReason::DismissedByUser,
                        )
                        .await
                        {
                            warn!(
                                ?err,
                                id, "failed to emit control notification_closed during clear_all"
                            );
                        }
                    }
                }
            })
            .await;
        if let Err(err) = self.state.emit_state_changed().await {
            // State was updated locally even if listeners missed this broadcast
            warn!(?err, "failed to emit state_changed after clear_all");
        }
        Ok(())
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
    // Delegate to a focused module so the interface file stays small and readable.
    watch::spawn_inhibitor_owner_watch(state).await
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use unixnotis_core::{INHIBIT_SCOPE_ALL, INHIBIT_SCOPE_POPUPS};

    use super::auth::is_trusted_control_executable_path;
    use super::sanitize::{normalize_inhibit_scope, sanitize_inhibit_reason};

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
