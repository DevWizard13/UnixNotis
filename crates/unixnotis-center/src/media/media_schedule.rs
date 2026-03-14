use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;

use super::{MediaInfo, MediaSignal};

pub(super) type DelayedRefreshTasks = HashMap<String, JoinHandle<()>>;

// Short retries capture immediate state transitions after transport commands.
const COMMAND_REFRESH_DELAYS_MS: [u64; 2] = [150, 650];
// Longer retries catch players that publish metadata a little later.
const METADATA_REFRESH_DELAYS_MS: [u64; 3] = [1200, 2400, 3600];
// Command path includes both immediate retries and metadata fallback retries.
const COMMAND_METADATA_REFRESH_DELAYS_MS: [u64; 5] = [150, 650, 1200, 2400, 3600];

pub(super) fn cancel_delayed_refresh(tasks: &mut DelayedRefreshTasks, bus_name: &str) {
    // Player-specific cancellation keeps delayed refresh growth bounded.
    if let Some(task) = tasks.remove(bus_name) {
        task.abort();
    }
}

pub(super) fn prune_delayed_refreshes(tasks: &mut DelayedRefreshTasks) {
    // Drop completed handles so the task map tracks only active delayed work.
    tasks.retain(|_, task| !task.is_finished());
}

pub(super) fn schedule_command_refresh(
    tasks: &mut DelayedRefreshTasks,
    cache: &HashMap<String, MediaInfo>,
    signal_tx: Sender<MediaSignal>,
    bus_name: &str,
) {
    // Command-triggered updates should refresh quickly, then keep metadata fallback.
    if needs_metadata_fallback(cache, bus_name) {
        schedule_refresh_sequence(
            tasks,
            signal_tx,
            bus_name,
            &COMMAND_METADATA_REFRESH_DELAYS_MS,
        );
        return;
    }
    schedule_refresh_sequence(tasks, signal_tx, bus_name, &COMMAND_REFRESH_DELAYS_MS);
}

pub(super) fn schedule_metadata_fallback(
    tasks: &mut DelayedRefreshTasks,
    cache: &HashMap<String, MediaInfo>,
    signal_tx: Sender<MediaSignal>,
    bus_name: &str,
) {
    // Replace older delayed work so each player has at most one active retry plan.
    if !needs_metadata_fallback(cache, bus_name) {
        cancel_delayed_refresh(tasks, bus_name);
        return;
    }
    schedule_refresh_sequence(tasks, signal_tx, bus_name, &METADATA_REFRESH_DELAYS_MS);
}

pub(super) fn schedule_metadata_fallbacks(
    tasks: &mut DelayedRefreshTasks,
    cache: &HashMap<String, MediaInfo>,
    signal_tx: Sender<MediaSignal>,
) {
    for bus_name in cache.keys() {
        // Per-player scheduling helper handles both scheduling and cancellation.
        schedule_metadata_fallback(tasks, cache, signal_tx.clone(), bus_name);
    }
}

fn schedule_refresh_sequence(
    tasks: &mut DelayedRefreshTasks,
    signal_tx: Sender<MediaSignal>,
    bus_name: &str,
    delays_ms: &[u64],
) {
    cancel_delayed_refresh(tasks, bus_name);
    if delays_ms.is_empty() {
        return;
    }
    let key = bus_name.to_string();
    let target_name = key.clone();
    let delays: Vec<Duration> = delays_ms
        .iter()
        .copied()
        .map(Duration::from_millis)
        .collect();
    // One task per player keeps retries bounded under noisy update streams.
    let task = tokio::spawn(async move {
        let mut previous = Duration::from_millis(0);
        for delay in delays {
            let step = delay.saturating_sub(previous);
            previous = delay;
            if !step.is_zero() {
                tokio::time::sleep(step).await;
            }
            if signal_tx
                .send(MediaSignal::PropertiesChanged(target_name.clone()))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    tasks.insert(key, task);
}

fn needs_metadata_fallback(cache: &HashMap<String, MediaInfo>, bus_name: &str) -> bool {
    let Some(info) = cache.get(bus_name) else {
        return false;
    };
    info.playback_status == "Playing" && info.title.is_empty()
}
