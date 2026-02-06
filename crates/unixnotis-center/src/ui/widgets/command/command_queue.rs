//! Command worker queues and scheduling logic.
//!
//! Owns thread spawning, backpressure, and dispatch so command execution
//! stays responsive under load without unbounded memory growth.

use std::collections::{HashMap, VecDeque};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel as channel;
use tracing::warn;
use unixnotis_core::{util, PanelDebugLevel};

use crate::debug;

use super::command_exec::{build_command_runtime, run_command_with_timeout};
use super::{CommandKind, CommandPlan};

const COMMAND_WORKERS: usize = 2;
// Bound the command queue to avoid unbounded memory growth during stalls.
const COMMAND_QUEUE_CAPACITY: usize = 128;
// Fallback queue keeps user actions flowing without spawning unbounded threads.
const COMMAND_FALLBACK_QUEUE_CAPACITY: usize = 32;
// Rate-limit queue saturation warnings to avoid log spam under misconfiguration.
const COMMAND_QUEUE_WARN_INTERVAL_SECS: u64 = 5;
// Coalesced refresh backlog cap to prevent unbounded memory under persistent overload.
const COALESCED_REFRESH_CAPACITY: usize = 256;
const COALESCED_RETRY_DELAY_MS: u64 = 25;

struct CommandJob {
    // Command text is stored once per job to keep worker threads independent.
    cmd: String,
    plan: CommandPlan,
    respond: Option<async_channel::Sender<Result<std::process::Output, std::io::Error>>>,
}

struct CommandWorker {
    tx: channel::Sender<CommandJob>,
    inline_fallback: bool,
}

impl CommandWorker {
    fn global() -> &'static CommandWorker {
        static WORKER: OnceLock<CommandWorker> = OnceLock::new();
        WORKER.get_or_init(|| CommandWorker::new(COMMAND_WORKERS))
    }

    fn new(worker_count: usize) -> Self {
        let (tx, rx) = channel::bounded(COMMAND_QUEUE_CAPACITY);
        let mut spawned = 0usize;
        for idx in 0..worker_count.max(1) {
            let rx = rx.clone();
            match std::thread::Builder::new()
                .name(format!("unixnotis-command-worker-{idx}"))
                .spawn(move || run_worker(rx))
            {
                Ok(_) => spawned += 1,
                Err(err) => {
                    warn!(?err, "failed to spawn command worker thread");
                }
            }
        }
        if spawned == 0 {
            // Inline fallback is enabled when no worker threads could be created.
            warn!("no command worker threads available; falling back to inline execution");
        }
        Self {
            tx,
            inline_fallback: spawned == 0,
        }
    }
}

pub(super) fn enqueue_command(
    cmd: String,
    plan: CommandPlan,
    respond: Option<async_channel::Sender<Result<std::process::Output, std::io::Error>>>,
) {
    let worker = CommandWorker::global();
    if worker.inline_fallback {
        // Fall back to a dedicated worker thread to avoid blocking the GTK loop.
        if std::thread::Builder::new()
            .name("unixnotis-command-fallback".to_string())
            .spawn({
                let job_for_thread = CommandJob {
                    cmd: cmd.clone(),
                    plan,
                    respond: respond.clone(),
                };
                move || handle_job(job_for_thread, None)
            })
            .is_err()
        {
            warn!("failed to spawn fallback command worker; running inline");
            handle_job(CommandJob { cmd, plan, respond }, None);
        }
        return;
    }
    let job = CommandJob { cmd, plan, respond };
    match worker.tx.try_send(job) {
        Ok(()) => {}
        Err(channel::TrySendError::Full(job)) => {
            // Avoid unbounded memory growth; prefer executing user actions immediately.
            if job.plan.kind == CommandKind::Action {
                if should_warn_queue_full() {
                    warn!("command queue full; routing action to fallback worker");
                }
                if FallbackWorker::global().submit(job).is_err() && should_warn_queue_full() {
                    warn!("fallback command queue full; dropping action");
                }
            } else {
                if should_warn_queue_full() {
                    warn!("command queue full; coalescing refresh command");
                }
                let coalescer = CoalescedRefreshQueue::global();
                coalescer.ensure_drain_thread(worker.tx.clone());
                coalescer.enqueue(job);
            }
        }
        Err(channel::TrySendError::Disconnected(_job)) => {
            warn!("command worker channel closed");
        }
    }
}

struct FallbackWorker {
    tx: channel::Sender<CommandJob>,
}

impl FallbackWorker {
    fn global() -> &'static FallbackWorker {
        static WORKER: OnceLock<FallbackWorker> = OnceLock::new();
        WORKER.get_or_init(|| FallbackWorker::new(COMMAND_FALLBACK_QUEUE_CAPACITY))
    }

    fn new(capacity: usize) -> Self {
        let (tx, rx) = channel::bounded(capacity);
        if std::thread::Builder::new()
            .name("unixnotis-command-fallback".to_string())
            .spawn(move || run_worker(rx))
            .is_err()
        {
            warn!("failed to spawn fallback command worker");
        }
        Self { tx }
    }

    fn submit(&self, job: CommandJob) -> Result<(), channel::TrySendError<CommandJob>> {
        self.tx.try_send(job)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct RefreshCommandKey {
    cmd: String,
    kind: CommandKind,
    timeout_ms: Option<u64>,
}

impl RefreshCommandKey {
    fn from_job(job: &CommandJob) -> Self {
        Self {
            cmd: job.cmd.clone(),
            kind: job.plan.kind,
            timeout_ms: job
                .plan
                .timeout_override
                .map(|timeout| timeout.as_millis().min(u64::MAX as u128) as u64),
        }
    }
}

struct CoalescedRefreshState {
    pending: HashMap<RefreshCommandKey, CommandJob>,
    order: VecDeque<RefreshCommandKey>,
}

struct CoalescedRefreshQueue {
    state: Mutex<CoalescedRefreshState>,
    wake: Condvar,
}

impl CoalescedRefreshQueue {
    fn global() -> &'static Self {
        static QUEUE: OnceLock<CoalescedRefreshQueue> = OnceLock::new();
        QUEUE.get_or_init(|| CoalescedRefreshQueue {
            state: Mutex::new(CoalescedRefreshState {
                pending: HashMap::new(),
                order: VecDeque::new(),
            }),
            wake: Condvar::new(),
        })
    }

    fn ensure_drain_thread(&'static self, worker_tx: channel::Sender<CommandJob>) {
        static STARTED: OnceLock<()> = OnceLock::new();
        STARTED.get_or_init(|| {
            let queue = self;
            if let Err(err) = std::thread::Builder::new()
                .name("unixnotis-command-coalescer".to_string())
                .spawn(move || queue.drain_loop(worker_tx))
            {
                warn!(?err, "failed to spawn refresh coalescer thread");
            }
        });
    }

    fn enqueue(&self, job: CommandJob) {
        let mut state = self.state.lock().expect("coalesced refresh lock poisoned");
        insert_coalesced_job(&mut state, job);
        self.wake.notify_one();
    }

    fn drain_loop(&'static self, worker_tx: channel::Sender<CommandJob>) {
        loop {
            let (key, job) = {
                let mut state = self.state.lock().expect("coalesced refresh lock poisoned");
                while state.order.is_empty() {
                    state = self
                        .wake
                        .wait(state)
                        .expect("coalesced refresh wait lock poisoned");
                }
                let Some(key) = state.order.pop_front() else {
                    continue;
                };
                let Some(job) = state.pending.remove(&key) else {
                    continue;
                };
                (key, job)
            };

            match worker_tx.try_send(job) {
                Ok(()) => {}
                Err(channel::TrySendError::Full(job)) => {
                    let mut state = self.state.lock().expect("coalesced refresh lock poisoned");
                    if !state.pending.contains_key(&key)
                        && state.pending.len() >= COALESCED_REFRESH_CAPACITY
                    {
                        if let Some(oldest) = state.order.pop_front() {
                            state.pending.remove(&oldest);
                        }
                    }
                    if !state.pending.contains_key(&key) {
                        state.order.push_front(key.clone());
                    }
                    state.pending.insert(key, job);
                    drop(state);
                    std::thread::sleep(Duration::from_millis(COALESCED_RETRY_DELAY_MS));
                }
                Err(channel::TrySendError::Disconnected(_job)) => return,
            }
        }
    }
}

fn insert_coalesced_job(state: &mut CoalescedRefreshState, job: CommandJob) {
    let key = RefreshCommandKey::from_job(&job);
    if !state.pending.contains_key(&key) {
        if state.pending.len() >= COALESCED_REFRESH_CAPACITY {
            if let Some(oldest) = state.order.pop_front() {
                // Drop the oldest queued refresh to preserve bounded memory usage.
                state.pending.remove(&oldest);
            }
        }
        // FIFO order keeps refresh fairness across widget keys.
        state.order.push_back(key.clone());
    }
    // Existing key replacement keeps only the newest payload per refresh key.
    state.pending.insert(key, job);
}

fn should_warn_queue_full() -> bool {
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    static LAST_WARN: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let last = LAST_WARN.load(AtomicOrdering::Relaxed);
    if now.saturating_sub(last) >= COMMAND_QUEUE_WARN_INTERVAL_SECS {
        // Relaxed ordering is sufficient because this is best-effort log throttling.
        LAST_WARN.store(now, AtomicOrdering::Relaxed);
        return true;
    }
    false
}

fn run_worker(rx: channel::Receiver<CommandJob>) {
    // Each worker owns a small current-thread runtime to avoid per-command OS threads.
    let runtime = build_command_runtime();
    for job in rx.iter() {
        // Jobs are processed sequentially per worker to preserve ordering semantics
        // for commands enqueued from the same widget.
        handle_job(job, runtime.as_ref());
    }
}

fn handle_job(job: CommandJob, runtime: Option<&tokio::runtime::Runtime>) {
    let cmd_snip = util::log_snippet(&job.cmd);
    debug::log(PanelDebugLevel::Verbose, || {
        format!("command start kind={:?} cmd={}", job.plan.kind, cmd_snip)
    });
    let started = Instant::now();
    let jitter = job.plan.jitter();
    if !jitter.is_zero() {
        // Jitter reduces synchronized polling when multiple widgets refresh together.
        std::thread::sleep(jitter);
    }
    let result = run_command_with_timeout(&job.cmd, job.plan.timeout(), runtime);
    let elapsed_ms = started.elapsed().as_millis();
    if let Some(tx) = job.respond {
        // Direct responses skip warning paths because the caller owns error handling.
        let _ = tx.send_blocking(result);
        return;
    }
    match result {
        Ok(output) => {
            if !output.status.success() {
                warn!(command = %cmd_snip, "command returned non-zero status");
                debug::log(PanelDebugLevel::Warn, || {
                    format!(
                        "command failed kind={:?} status={:?} elapsed_ms={elapsed_ms}",
                        job.plan.kind,
                        output.status.code()
                    )
                });
            } else {
                debug::log(PanelDebugLevel::Verbose, || {
                    format!(
                        "command ok kind={:?} status={:?} elapsed_ms={elapsed_ms}",
                        job.plan.kind,
                        output.status.code()
                    )
                });
            }
        }
        Err(err) => {
            warn!(command = %cmd_snip, ?err, "command failed");
            debug::log(PanelDebugLevel::Warn, || {
                format!(
                    "command error kind={:?} elapsed_ms={elapsed_ms} err={err}",
                    job.plan.kind
                )
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        insert_coalesced_job, CoalescedRefreshState, CommandJob, CommandKind, CommandPlan,
    };
    use std::collections::{HashMap, VecDeque};

    fn job(cmd: &str, kind: CommandKind) -> CommandJob {
        CommandJob {
            cmd: cmd.to_string(),
            plan: CommandPlan {
                kind,
                timeout_override: None,
            },
            respond: None,
        }
    }

    #[test]
    fn coalesced_insert_replaces_existing_key() {
        let mut state = CoalescedRefreshState {
            pending: HashMap::new(),
            order: VecDeque::new(),
        };
        insert_coalesced_job(&mut state, job("echo a", CommandKind::Fast));
        insert_coalesced_job(&mut state, job("echo a", CommandKind::Fast));

        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.order.len(), 1);
    }

    #[test]
    fn coalesced_insert_keeps_distinct_keys() {
        let mut state = CoalescedRefreshState {
            pending: HashMap::new(),
            order: VecDeque::new(),
        };
        insert_coalesced_job(&mut state, job("echo a", CommandKind::Fast));
        insert_coalesced_job(&mut state, job("echo a", CommandKind::Slow));

        assert_eq!(state.pending.len(), 2);
        assert_eq!(state.order.len(), 2);
    }
}
