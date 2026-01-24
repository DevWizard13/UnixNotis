//! D-Bus runtime for popup UI events and control updates.

use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use tokio::sync::mpsc::{self, UnboundedSender};
use tracing::{debug, info, warn};
use unixnotis_core::{CloseReason, ControlProxy, ControlState, NotificationView};
use zbus::{Connection, Result as ZbusResult};

// Backoff settings throttle reconnect attempts while keeping recovery responsive.
const BACKOFF_BASE_MS: u64 = 250;
const BACKOFF_MAX_MS: u64 = 5000;
const BACKOFF_JITTER_MS: u64 = 120;
// Retry warnings are rate-limited to avoid noisy logs during long outages.
const RETRY_WARN_INTERVAL_SECS: u64 = 30;
// Seed retries tolerate short startup hiccups without blocking indefinitely.
const SEED_RETRY_BASE_MS: u64 = 250;
const SEED_RETRY_MAX_MS: u64 = 2000;
const SEED_RETRY_BUDGET_SECS: u64 = 30;
const SEED_RETRY_LOG_INTERVAL_SECS: u64 = 10;

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

// Rate-limited logger used to avoid warning spam during retry loops.
struct RetryLog {
    interval: Duration,
    last_warn: Instant,
}

impl RetryLog {
    fn new(interval: Duration) -> Self {
        let mut log = Self {
            interval,
            last_warn: Instant::now(),
        };
        log.reset();
        log
    }

    fn reset(&mut self) {
        // Allow the next failure after a success to emit a warning immediately.
        self.last_warn = Instant::now() - self.interval;
    }

    fn warn_or_debug<E: std::fmt::Debug>(&mut self, err: &E, message: &str) {
        if self.last_warn.elapsed() >= self.interval {
            self.last_warn = Instant::now();
            warn!(?err, "{message}");
        } else {
            debug!(?err, "{message}");
        }
    }

    fn warn_or_debug_seed(&mut self, err: &SeedError, message: &str) {
        if self.last_warn.elapsed() >= self.interval {
            self.last_warn = Instant::now();
            warn!(
                state_error = ?err.state_error,
                active_error = ?err.active_error,
                "{message}"
            );
        } else {
            debug!(
                state_error = ?err.state_error,
                active_error = ?err.active_error,
                "{message}"
            );
        }
    }
}

// Captures seed failures without forcing an immediate reconnect.
#[derive(Debug)]
struct SeedError {
    state_error: Option<String>,
    active_error: Option<String>,
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
            let mut connect_log = RetryLog::new(Duration::from_secs(RETRY_WARN_INTERVAL_SECS));
            let mut subscribe_log = RetryLog::new(Duration::from_secs(RETRY_WARN_INTERVAL_SECS));

            loop {
                let connection = loop {
                    match Connection::session().await {
                        Ok(connection) => {
                            connect_backoff.reset();
                            connect_log.reset();
                            break connection;
                        }
                        Err(err) => {
                            connect_log.warn_or_debug(&err, "failed to connect to session bus; retrying");
                            tokio::time::sleep(connect_backoff.next_sleep()).await;
                        }
                    }
                };

                let proxy = match ControlProxy::new(&connection).await {
                    Ok(proxy) => proxy,
                    Err(err) => {
                        subscribe_log.warn_or_debug(&err, "control interface unavailable, retrying");
                        drain_offline_commands(&mut command_rx);
                        tokio::time::sleep(subscribe_backoff.next_sleep()).await;
                        continue;
                    }
                };
                subscribe_backoff.reset();
                subscribe_log.reset();
                info!("connected to unixnotis control interface");
                seed_state_with_retry(&proxy, &sender).await;

                let mut added_stream = match proxy.receive_notification_added().await {
                    Ok(stream) => stream,
                    Err(err) => {
                        subscribe_log.warn_or_debug(&err, "failed to subscribe to notification_added");
                        tokio::time::sleep(subscribe_backoff.next_sleep()).await;
                        continue;
                    }
                };
                let mut updated_stream = match proxy.receive_notification_updated().await {
                    Ok(stream) => stream,
                    Err(err) => {
                        subscribe_log.warn_or_debug(&err, "failed to subscribe to notification_updated");
                        tokio::time::sleep(subscribe_backoff.next_sleep()).await;
                        continue;
                    }
                };
                let mut closed_stream = match proxy.receive_notification_closed().await {
                    Ok(stream) => stream,
                    Err(err) => {
                        subscribe_log.warn_or_debug(&err, "failed to subscribe to notification_closed");
                        tokio::time::sleep(subscribe_backoff.next_sleep()).await;
                        continue;
                    }
                };
                let mut state_stream = match proxy.receive_state_changed().await {
                    Ok(stream) => stream,
                    Err(err) => {
                        subscribe_log.warn_or_debug(&err, "failed to subscribe to state_changed");
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

async fn seed_state_with_retry(proxy: &ControlProxy<'_>, sender: &async_channel::Sender<UiEvent>) {
    // Seed retries are bounded to keep startup responsive while tolerating transient failures.
    let mut backoff = Backoff::new(SEED_RETRY_BASE_MS, SEED_RETRY_MAX_MS);
    let deadline = Instant::now() + Duration::from_secs(SEED_RETRY_BUDGET_SECS);
    let mut log = RetryLog::new(Duration::from_secs(SEED_RETRY_LOG_INTERVAL_SECS));

    loop {
        match seed_state(proxy, sender).await {
            Ok(()) => return,
            Err(err) => {
                if Instant::now() >= deadline {
                    warn!(
                        state_error = ?err.state_error,
                        active_error = ?err.active_error,
                        "failed to seed popup state; giving up until reconnect"
                    );
                    return;
                }
                log.warn_or_debug_seed(&err, "failed to seed popup state; retrying");
                tokio::time::sleep(backoff.next_sleep()).await;
            }
        }
    }
}

async fn seed_state(
    proxy: &ControlProxy<'_>,
    sender: &async_channel::Sender<UiEvent>,
) -> Result<(), SeedError> {
    let state = proxy.get_state().await;
    let active = proxy.list_active().await;

    match (state, active) {
        (Ok(state), Ok(active)) => {
            let _ = sender.send(UiEvent::Seed { state, active }).await;
            Ok(())
        }
        (state, active) => Err(SeedError {
            state_error: state.err().map(|err| err.to_string()),
            active_error: active.err().map(|err| err.to_string()),
        }),
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
