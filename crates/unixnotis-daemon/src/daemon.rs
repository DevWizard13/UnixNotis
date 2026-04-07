//! D-Bus server implementation and daemon state coordination.
//!
//! The notification and control interfaces are split into submodules to keep
//! responsibilities clear and files smaller.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use tokio::sync::Mutex;
use tracing::info;
use unixnotis_core::{
    CloseReason, Config, ControlState, PopupGateState, CONTROL_BUS_NAME, CONTROL_OBJECT_PATH,
};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::{Connection, SignalContext};

use crate::expire::ExpirationScheduler;
use crate::sound::SoundSettings;
use crate::store::NotificationStore;

#[path = "daemon/daemon_control.rs"]
mod daemon_control;
#[path = "daemon/daemon_notifications.rs"]
mod daemon_notifications;

pub use daemon_control::{spawn_inhibitor_owner_watch, ControlServer};
pub use daemon_notifications::NotificationServer;

pub(crate) const NOTIFICATIONS_OBJECT_PATH: &str = "/org/freedesktop/Notifications";

/// Shared daemon state guarded behind an async mutex.
pub struct DaemonState {
    pub store: Mutex<NotificationStore>,
    /// Immutable sound settings resolved at startup.
    pub sound: SoundSettings,
    connection: Connection,
    // Panel control should only succeed once the center has subscribed
    // This avoids accepting requests that no live listener can receive
    panel_ready: AtomicBool,
    popups_running: AtomicBool,
    // Scheduler is installed after state startup so close paths can cancel timers
    scheduler: OnceLock<ExpirationScheduler>,
    // Cache the last control-state snapshot so no-op signals can be skipped
    last_emitted_state: StdMutex<Option<ControlState>>,
    // Popup UIs only care about the gate, not panel history counters
    last_emitted_popup_gate: StdMutex<Option<PopupGateState>>,
}

impl DaemonState {
    pub fn new(connection: Connection, config: Config, sound: SoundSettings) -> Arc<Self> {
        let store = NotificationStore::new(config);
        Arc::new(Self {
            store: Mutex::new(store),
            sound,
            connection,
            panel_ready: AtomicBool::new(false),
            popups_running: AtomicBool::new(false),
            scheduler: OnceLock::new(),
            last_emitted_state: StdMutex::new(None),
            last_emitted_popup_gate: StdMutex::new(None),
        })
    }

    pub fn set_scheduler(&self, scheduler: ExpirationScheduler) {
        // Scheduler is wired once during daemon startup
        let _ = self.scheduler.set(scheduler);
    }

    fn scheduler(&self) -> Option<ExpirationScheduler> {
        // Cloning the sender handle is cheap and keeps await points simple
        self.scheduler.get().cloned()
    }

    async fn cancel_expiration(&self, id: u32) {
        // Missing scheduler means startup is still incomplete, so skip quietly
        let Some(scheduler) = self.scheduler() else {
            return;
        };
        scheduler.schedule(id, None).await;
    }

    pub async fn cancel_expirations(&self, ids: &[u32]) {
        // Cancel timers for every removed active id so stale wakeups do not build up
        // Per-id cancel keeps the existing lazy heap design simple and predictable
        let Some(scheduler) = self.scheduler() else {
            return;
        };
        for id in ids {
            scheduler.schedule(*id, None).await;
        }
    }

    pub async fn close_notification(&self, id: u32, reason: CloseReason) -> zbus::Result<()> {
        let removed = {
            let mut store = self.store.lock().await;
            store.close(id)
        };
        if removed.is_none() {
            return Ok(());
        }
        // Timer cancel happens before signal fanout so stale wakeups stop right away
        self.cancel_expiration(id).await;

        let notif_ctx = SignalContext::new(&self.connection, NOTIFICATIONS_OBJECT_PATH)?;
        NotificationServer::notification_closed(&notif_ctx, id, reason as u32).await?;

        let control_ctx = SignalContext::new(&self.connection, CONTROL_OBJECT_PATH)?;
        ControlServer::notification_closed(&control_ctx, id, reason).await?;
        self.emit_state_changed().await?;

        Ok(())
    }

    pub async fn dismiss_from_panel(&self, id: u32) -> zbus::Result<()> {
        let outcome = {
            let mut store = self.store.lock().await;
            store.dismiss_from_panel(id)
        };

        if !outcome.removed_any() {
            return Ok(());
        }

        if outcome.removed_active {
            // Panel dismiss removes the active entry, so its timer must go too
            self.cancel_expiration(id).await;
            let notif_ctx = SignalContext::new(&self.connection, NOTIFICATIONS_OBJECT_PATH)?;
            NotificationServer::notification_closed(
                &notif_ctx,
                id,
                CloseReason::DismissedByUser as u32,
            )
            .await?;
        }

        let control_ctx = SignalContext::new(&self.connection, CONTROL_OBJECT_PATH)?;
        ControlServer::notification_closed(&control_ctx, id, CloseReason::DismissedByUser).await?;
        self.emit_state_changed().await?;

        Ok(())
    }

    async fn emit_state_changed(&self) -> zbus::Result<()> {
        let state = {
            let store = self.store.lock().await;
            let history_count = store.history_len() as u32;
            // Panel consumers still need history and inhibitor counters in one snapshot
            ControlState {
                dnd_enabled: store.dnd_enabled(),
                history_count,
                inhibited: store.inhibited(),
                inhibitor_count: store.inhibitor_count(),
            }
        };
        // Popup policy only depends on the gate, so history churn should not wake it up
        let popup_gate = PopupGateState {
            dnd_enabled: state.dnd_enabled,
            inhibited: state.inhibited,
        };
        // Duplicate broadcasts add D-Bus churn without changing UI behavior
        let should_emit_state = should_emit_cached(&self.last_emitted_state, &state);
        let should_emit_popup_gate = should_emit_cached(&self.last_emitted_popup_gate, &popup_gate);
        if !should_emit_state && !should_emit_popup_gate {
            return Ok(());
        }
        let control_ctx = SignalContext::new(&self.connection, CONTROL_OBJECT_PATH)?;
        if should_emit_state {
            ControlServer::state_changed(&control_ctx, state).await?;
        }
        if should_emit_popup_gate {
            ControlServer::popup_gate_changed(&control_ctx, popup_gate).await?;
        }
        Ok(())
    }

    pub async fn emit_snapshot_invalidated(&self) -> zbus::Result<()> {
        // This signal tells clients their local materialized view may be stale
        let control_ctx = SignalContext::new(&self.connection, CONTROL_OBJECT_PATH)?;
        ControlServer::snapshot_invalidated(&control_ctx).await
    }

    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }

    pub(crate) fn set_panel_ready(&self, ready: bool) {
        // SeqCst keeps state changes easy to follow during crash recovery
        self.panel_ready.store(ready, Ordering::SeqCst);
    }

    pub(crate) fn set_popups_running(&self, running: bool) {
        // Popup health is tracked for supervision and diagnostics
        self.popups_running.store(running, Ordering::SeqCst);
    }

    pub(crate) fn panel_ready(&self) -> bool {
        self.panel_ready.load(Ordering::SeqCst)
    }
}

pub async fn request_well_known_name(
    connection: &Connection,
    replace_existing: bool,
) -> zbus::Result<RequestNameReply> {
    let flags = if replace_existing {
        zbus::fdo::RequestNameFlags::ReplaceExisting | zbus::fdo::RequestNameFlags::AllowReplacement
    } else {
        // Avoid being replaceable in non-trial mode to prevent silent takeovers.
        zbus::fdo::RequestNameFlags::DoNotQueue.into()
    };
    connection
        .request_name_with_flags("org.freedesktop.Notifications", flags)
        .await
}

pub async fn request_control_name(connection: &Connection) -> zbus::Result<RequestNameReply> {
    let flags = RequestNameFlags::DoNotQueue;
    connection
        .request_name_with_flags(CONTROL_BUS_NAME, flags.into())
        .await
}

pub fn log_name_reply(reply: &RequestNameReply) {
    match reply {
        RequestNameReply::PrimaryOwner => {
            info!("acquired org.freedesktop.Notifications");
        }
        RequestNameReply::InQueue => {
            info!("queued for org.freedesktop.Notifications");
        }
        RequestNameReply::AlreadyOwner => {
            info!("already owns org.freedesktop.Notifications");
        }
        RequestNameReply::Exists => {
            info!("org.freedesktop.Notifications is already owned");
        }
    }
}

pub(crate) fn to_fdo_error(err: zbus::Error) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(err.to_string())
}

fn should_emit_cached<T: Clone + PartialEq>(cache: &StdMutex<Option<T>>, value: &T) -> bool {
    // Sync mutex is enough here because this cache is tiny and never held across await points
    let mut last_value = match cache.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if last_value
        .as_ref()
        .is_some_and(|previous| previous == value)
    {
        // Identical state would only burn CPU in zbus and the listeners
        return false;
    }
    // Clone once on change so later comparisons stay allocation-free for equal values
    *last_value = Some(value.clone());
    true
}
