//! Popup state seeding helpers

use std::time::{Duration, Instant};

use tracing::{debug, warn};
use unixnotis_core::ControlProxy;

use super::dbus_backoff::{Backoff, RetryLog};
use super::dbus_types::UiEvent;

// Seed retries tolerate short startup hiccups without blocking indefinitely
const SEED_RETRY_BASE_MS: u64 = 250;
const SEED_RETRY_MAX_MS: u64 = 2000;
const SEED_RETRY_BUDGET_SECS: u64 = 30;
const SEED_RETRY_LOG_INTERVAL_SECS: u64 = 10;

// Seed failures are tracked without forcing an immediate reconnect
#[derive(Debug)]
struct SeedError {
    state_error: Option<String>,
    active_error: Option<String>,
}

pub(crate) async fn seed_state_with_retry(
    proxy: &ControlProxy<'_>,
    sender: &async_channel::Sender<UiEvent>,
) {
    // Seed retries stay bounded so startup can recover without hanging forever
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
                log_seed_retry(&mut log, &err, "failed to seed popup state; retrying");
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

fn log_seed_retry(log: &mut RetryLog, err: &SeedError, message: &str) {
    log.log_with(
        || {
            warn!(
                state_error = ?err.state_error,
                active_error = ?err.active_error,
                "{message}"
            );
        },
        || {
            debug!(
                state_error = ?err.state_error,
                active_error = ?err.active_error,
                "{message}"
            );
        },
    );
}
