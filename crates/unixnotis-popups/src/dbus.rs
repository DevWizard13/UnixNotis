//! D-Bus runtime for popup UI events and control updates.

use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use tokio::sync::mpsc::{self, UnboundedSender};
use tracing::{info, warn};
use unixnotis_core::{CloseReason, ControlProxy, ControlState, NotificationView};
use zbus::{Connection, Result as ZbusResult};

// Backoff settings throttle reconnect attempts while keeping recovery responsive.
const BACKOFF_BASE_MS: u64 = 250;
const BACKOFF_MAX_MS: u64 = 5000;
const BACKOFF_JITTER_MS: u64 = 120;

struct Backoff {
    base: Duration,
    current: Duration,
    max: Duration,
}

impl Backoff {
    fn new(base_ms: u64, max_ms: u64) -> Self {
        let base = Duration::from_millis(base_ms);
        Self {
            base,
            current: base,
            max: Duration::from_millis(max_ms),
        }
    }

    fn reset(&mut self) {
        self.current = self.base;
    }

    fn next_sleep(&mut self) -> Duration {
        let jitter = jitter_duration(BACKOFF_JITTER_MS);
        let sleep = self.current;
        self.current = (self.current * 2).min(self.max);
        sleep + jitter
    }
}

fn jitter_duration(max_ms: u64) -> Duration {
    if max_ms == 0 {
        return Duration::from_millis(0);
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    let jitter_ms = (nanos % (max_ms * 1_000_000)) / 1_000_000;
    Duration::from_millis(jitter_ms)
}

/// Events delivered to the GTK main loop.
#[derive(Debug, Clone)]
pub enum UiEvent {
    Seed {
        state: ControlState,
        active: Vec<NotificationView>,
    },
    NotificationAdded(NotificationView, bool),
    NotificationUpdated(NotificationView, bool),
    NotificationClosed(u32, CloseReason),
    StateChanged(ControlState),
    CssReload,
    ConfigReload,
}

/// Commands sent from GTK handlers to the D-Bus runtime.
#[derive(Debug, Clone)]
pub enum UiCommand {
    Dismiss(u32),
    InvokeAction { id: u32, action_key: String },
}

pub fn start_dbus_runtime(sender: async_channel::Sender<UiEvent>) -> UnboundedSender<UiCommand> {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel();

    thread::spawn(move || {
        // Dedicated runtime keeps async D-Bus work off the GTK main thread.
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(err) => {
                warn!(?err, "failed to initialize tokio runtime");
                return;
            }
        };
        runtime.block_on(async move {
            let mut connect_backoff = Backoff::new(BACKOFF_BASE_MS, BACKOFF_MAX_MS);
            let mut subscribe_backoff = Backoff::new(BACKOFF_BASE_MS, BACKOFF_MAX_MS);

            loop {
                let connection = loop {
                    match Connection::session().await {
                        Ok(connection) => {
                            connect_backoff.reset();
                            break connection;
                        }
                        Err(err) => {
                            warn!(?err, "failed to connect to session bus; retrying");
                            tokio::time::sleep(connect_backoff.next_sleep()).await;
                        }
                    }
                };

                let proxy = match ControlProxy::new(&connection).await {
                    Ok(proxy) => proxy,
                    Err(err) => {
                        warn!(?err, "control interface unavailable, retrying");
                        drain_offline_commands(&mut command_rx);
                        tokio::time::sleep(subscribe_backoff.next_sleep()).await;
                        continue;
                    }
                };
                subscribe_backoff.reset();
                info!("connected to unixnotis control interface");
                seed_state(&proxy, &sender).await;

                let mut added_stream = match proxy.receive_notification_added().await {
                    Ok(stream) => stream,
                    Err(err) => {
                        warn!(?err, "failed to subscribe to notification_added");
                        tokio::time::sleep(subscribe_backoff.next_sleep()).await;
                        continue;
                    }
                };
                let mut updated_stream = match proxy.receive_notification_updated().await {
                    Ok(stream) => stream,
                    Err(err) => {
                        warn!(?err, "failed to subscribe to notification_updated");
                        tokio::time::sleep(subscribe_backoff.next_sleep()).await;
                        continue;
                    }
                };
                let mut closed_stream = match proxy.receive_notification_closed().await {
                    Ok(stream) => stream,
                    Err(err) => {
                        warn!(?err, "failed to subscribe to notification_closed");
                        tokio::time::sleep(subscribe_backoff.next_sleep()).await;
                        continue;
                    }
                };
                let mut state_stream = match proxy.receive_state_changed().await {
                    Ok(stream) => stream,
                    Err(err) => {
                        warn!(?err, "failed to subscribe to state_changed");
                        tokio::time::sleep(subscribe_backoff.next_sleep()).await;
                        continue;
                    }
                };

                loop {
                    tokio::select! {
                        command = command_rx.recv() => {
                            let Some(command) = command else {
                                break;
                            };
                            if let Err(err) = handle_command(&proxy, command).await {
                                warn!(?err, "control command failed");
                            }
                        }
                        signal = added_stream.next() => {
                            let Some(signal) = signal else {
                                warn!("notification_added stream ended");
                                break;
                            };
                            if let Ok(args) = signal.args() {
                                let _ = sender
                                    .send(UiEvent::NotificationAdded(
                                        args.notification().clone(),
                                        *args.show_popup(),
                                    ))
                                    .await;
                            }
                        }
                        signal = updated_stream.next() => {
                            let Some(signal) = signal else {
                                warn!("notification_updated stream ended");
                                break;
                            };
                            if let Ok(args) = signal.args() {
                                let _ = sender
                                    .send(UiEvent::NotificationUpdated(
                                        args.notification().clone(),
                                        *args.show_popup(),
                                    ))
                                    .await;
                            }
                        }
                        signal = closed_stream.next() => {
                            let Some(signal) = signal else {
                                warn!("notification_closed stream ended");
                                break;
                            };
                            if let Ok(args) = signal.args() {
                                let _ = sender
                                    .send(UiEvent::NotificationClosed(
                                        *args.id(),
                                        *args.reason(),
                                    ))
                                    .await;
                            }
                        }
                        signal = state_stream.next() => {
                            let Some(signal) = signal else {
                                warn!("state_changed stream ended");
                                break;
                            };
                            if let Ok(args) = signal.args() {
                                let _ = sender.send(UiEvent::StateChanged(args.state().clone())).await;
                            }
                        }
                    }
                }
                tokio::time::sleep(subscribe_backoff.next_sleep()).await;
            }
        });
    });

    command_tx
}

async fn seed_state(proxy: &ControlProxy<'_>, sender: &async_channel::Sender<UiEvent>) {
    let state = proxy.get_state().await;
    let active = proxy.list_active().await;

    match (state, active) {
        (Ok(state), Ok(active)) => {
            let _ = sender.send(UiEvent::Seed { state, active }).await;
        }
        (state, active) => {
            if let Err(err) = state {
                warn!(?err, "failed to fetch popup state");
            }
            if let Err(err) = active {
                warn!(?err, "failed to fetch active notifications for popups");
            }
        }
    }
}

async fn handle_command(proxy: &ControlProxy<'_>, command: UiCommand) -> ZbusResult<()> {
    match command {
        UiCommand::Dismiss(id) => proxy.dismiss(id).await,
        UiCommand::InvokeAction { id, action_key } => proxy.invoke_action(id, &action_key).await,
    }
}

fn drain_offline_commands(command_rx: &mut mpsc::UnboundedReceiver<UiCommand>) {
    while command_rx.try_recv().is_ok() {
        warn!("dropping control command while interface is unavailable");
    }
}
