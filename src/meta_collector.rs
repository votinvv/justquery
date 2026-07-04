//! Background metadata collector: periodically probes the catalog and keeps a full in-memory copy of
//! the monitored schemas — object LIST **and all column attributes** — in a shared [`SharedStore`],
//! enforcing a max `objects + attributes` budget. Runs on its own thread with its OWN short-lived
//! connection — it connects, scans, then **disconnects and sleeps** between scans (no long-lived idle
//! session on the server). Never touches `main_conn`.
//!
//! The scan is incremental: one cheap query returns a per-schema change fingerprint (folding in
//! `xmin` of relations / attributes / defaults / functions), and only schemas whose fingerprint
//! diverged are re-pulled in full and swapped into the store. If a scan fails (connect/catalog
//! error) or the whole catalog exceeds the budget, the scanner stops with the error and waits for
//! a manual Resume (toolbar Enable scan).
//!
//! Public contract (frozen — the app talks to the worker only through this):
//!   * [`start`] → [`CollectorHandle`] (command sender + [`CollectorMsg`] receiver + shared store),
//!   * commands via [`CollectorCmd`], updates via [`CollectorMsg`].

use crate::catalog::{count_meta_rows, list_schemas, scan_schema, schema_fingerprints};
use crate::connections::{connect_session_with_timeout, ConnParams};
use crate::dialog::now_hms;
use crate::metadata::{
    CollectorSettings, CollectorStatus, LogLine, MetaCol, MetaObjRow, SharedStore,
};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Commands the app sends to the collector.
pub(crate) enum CollectorCmd {
    Pause,                        // stop the periodic scan (keeps the last store)
    Resume,                       // resume periodic scanning (+ scan now)
    SetSchemas(Option<Vec<String>>), // None = all user schemas
    SetBudget(usize),
    SetInterval(u64),
    SetIdle(u64),                 // seconds of inactivity before the scanner sleeps
    Activity,                     // a user-activity ping — keeps the scanner awake
    Shutdown,
}

/// Updates the collector sends back to the app (polled in the update loop). The store itself is
/// shared memory ([`SharedStore`]) — only status and log lines travel over the channel.
pub(crate) enum CollectorMsg {
    Status(CollectorStatus),
    Log(LogLine),
}

/// App-side handle to the collector worker. Dropping it shuts the worker down.
pub(crate) struct CollectorHandle {
    cmd: Sender<CollectorCmd>,
    pub rx: Receiver<CollectorMsg>,
}

impl CollectorHandle {
    fn send(&self, c: CollectorCmd) {
        let _ = self.cmd.send(c);
    }
    pub fn pause(&self) {
        self.send(CollectorCmd::Pause);
    }
    pub fn resume(&self) {
        self.send(CollectorCmd::Resume);
    }
    pub fn set_schemas(&self, s: Option<Vec<String>>) {
        self.send(CollectorCmd::SetSchemas(s));
    }
    pub fn set_budget(&self, b: usize) {
        self.send(CollectorCmd::SetBudget(b));
    }
    pub fn set_interval(&self, i: u64) {
        self.send(CollectorCmd::SetInterval(i));
    }
    pub fn set_idle(&self, i: u64) {
        self.send(CollectorCmd::SetIdle(i));
    }
    /// Ping the scanner that the user is active (throttled by the caller).
    pub fn activity(&self) {
        self.send(CollectorCmd::Activity);
    }
}

impl Drop for CollectorHandle {
    fn drop(&mut self) {
        let _ = self.cmd.send(CollectorCmd::Shutdown);
    }
}

/// Spawn the collector worker for `params` with initial `settings`, writing into `shared`.
pub(crate) fn start(
    params: ConnParams,
    settings: CollectorSettings,
    shared: Arc<SharedStore>,
) -> CollectorHandle {
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<CollectorCmd>();
    let (msg_tx, msg_rx) = std::sync::mpsc::channel::<CollectorMsg>();
    std::thread::spawn(move || run(params, settings, shared, cmd_rx, msg_tx));
    CollectorHandle {
        cmd: cmd_tx,
        rx: msg_rx,
    }
}

/// Outcome of one scan, telling the run loop whether to keep cycling or stop.
enum Outcome {
    Done,    // scanned (or nothing changed) — keep cycling
    Stopped, // scan failed or the catalog is over budget — park until the user resumes
}

/// The collector thread body. Keeps mutable `settings`, a `stopped` flag (= `!enabled` at start),
/// the last per-schema `fingerprints` and the `last_scan` time. While stopped it blocks on
/// `recv()`. While active it scans at most once per `interval` (the cooldown between scans) and only
/// sleeps after `idle` seconds with no activity; activity pings keep it awake but never shortcut the
/// cooldown. All sends use `let _ =` — if the app dropped the handle the thread simply exits.
fn run(
    params: ConnParams,
    mut settings: CollectorSettings,
    shared: Arc<SharedStore>,
    cmd_rx: Receiver<CollectorCmd>,
    msg_tx: Sender<CollectorMsg>,
) {
    let mut stopped = !settings.enabled;
    let mut asleep = false; // idle sleep (reported separately from `stopped`)
    let mut last_objects: usize = 0;
    let mut fingerprints: HashMap<String, String> = HashMap::new();
    let mut last_activity = Instant::now(); // a connect counts as activity → first scan runs
    let mut last_scan: Option<Instant> = None; // None = a scan is due now (first run / after Resume)
    if stopped {
        let _ = msg_tx.send(CollectorMsg::Status(CollectorStatus {
            stopped: true,
            ..Default::default()
        }));
        // the status text points at the log for the stop reason — so the log must carry one
        let _ = msg_tx.send(CollectorMsg::Log(LogLine {
            time: now_hms(),
            text: "stopped: scanning is disabled for this connection".to_owned(),
        }));
    }
    loop {
        if stopped {
            // stopped: block until the app tells us what to do next
            match cmd_rx.recv() {
                Ok(CollectorCmd::Shutdown) | Err(_) => return,
                Ok(CollectorCmd::Resume) => {
                    last_activity = Instant::now();
                    last_scan = Some(Instant::now());
                    // stay stopped if the resumed scan fails or is still over budget
                    stopped = matches!(
                        scan(&params, &settings, &shared, &mut fingerprints, &msg_tx, &mut last_objects),
                        Outcome::Stopped
                    );
                }
                Ok(CollectorCmd::SetSchemas(o)) => settings.schemas = o,
                Ok(CollectorCmd::SetBudget(b)) => settings.budget = b,
                Ok(CollectorCmd::SetInterval(i)) => settings.interval = i,
                Ok(CollectorCmd::SetIdle(i)) => settings.idle = i,
                // Pause (already stopped) / Activity → nothing while stopped
                Ok(_) => {}
            }
            continue;
        }
        // active window = the user has been active within the last `idle` seconds. Inside it we scan
        // at most once per `interval` (the 30s "cooldown" between scans); activity pings only keep the
        // window open, they do NOT trigger an early scan. Past the window we sleep (no DB churn).
        let active_window = last_activity.elapsed() < Duration::from_secs(settings.idle.max(30));
        let scan_due = last_scan
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(settings.interval.max(5)));
        if active_window {
            asleep = false;
            if scan_due {
                match scan(&params, &settings, &shared, &mut fingerprints, &msg_tx, &mut last_objects) {
                    Outcome::Stopped => {
                        // scan failed / catalog too big → stop and wait for a manual Resume (no DB
                        // churn meanwhile); the reason is already the last activity-log line
                        stopped = true;
                        continue;
                    }
                    // break starts from scan end → next scan is `interval` seconds later
                    Outcome::Done => last_scan = Some(Instant::now()),
                }
            }
        } else if !asleep {
            // just went idle → report the sleep once (drives the yellow chip), then wait for a ping
            asleep = true;
            let _ = msg_tx.send(CollectorMsg::Status(CollectorStatus {
                asleep: true,
                ..Default::default()
            }));
            let _ = msg_tx.send(CollectorMsg::Log(LogLine {
                time: now_hms(),
                text: "asleep: no user activity; scans resume on activity".to_owned(),
            }));
        }
        match cmd_rx.recv_timeout(Duration::from_secs(settings.interval.max(5))) {
            Ok(CollectorCmd::Shutdown) => return,
            Ok(cmd) => {
                last_activity = Instant::now(); // any command means the user is interacting
                match cmd {
                    CollectorCmd::Pause => {
                        stopped = true;
                        let _ = msg_tx.send(CollectorMsg::Status(CollectorStatus {
                            stopped: true,
                            ..Default::default()
                        }));
                        let _ = msg_tx.send(CollectorMsg::Log(LogLine {
                            time: now_hms(),
                            text: "stopped: disabled manually".to_owned(),
                        }));
                    }
                    CollectorCmd::SetSchemas(o) => settings.schemas = o,
                    CollectorCmd::SetBudget(b) => settings.budget = b,
                    CollectorCmd::SetInterval(i) => settings.interval = i,
                    CollectorCmd::SetIdle(i) => settings.idle = i,
                    // Resume / Activity → just keep the window open (activity reset above)
                    _ => {}
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Run one incremental scan: cheap budget guard, then a per-schema fingerprint diff that re-pulls
/// only diverged schemas (objects + columns) and swaps them into `shared` under the write lock.
/// Emits `Status` + `Log`. Never panics: a connect/query error reports via [`fail`] and returns
/// [`Outcome::Stopped`] — the run loop parks until the user resumes (toolbar Enable scan).
fn scan(
    params: &ConnParams,
    settings: &CollectorSettings,
    shared: &Arc<SharedStore>,
    fingerprints: &mut HashMap<String, String>,
    msg_tx: &Sender<CollectorMsg>,
    last_objects: &mut usize,
) -> Outcome {
    // announce the scan started (keep the previously-known object count visible)
    let _ = msg_tx.send(CollectorMsg::Status(CollectorStatus {
        running: true,
        ..Default::default()
    }));
    let started = Instant::now();

    // fresh session per scan — we never hold an idle connection between scans; a full catalog scan
    // must not be cut off mid-read, so the statement timeout is disabled (0 = unlimited)
    let mut client = match connect_session_with_timeout(params, 0) {
        Ok(c) => c,
        Err(e) => {
            fail(msg_tx, e);
            return Outcome::Stopped;
        }
    };

    // every user schema — the store lists them all (the modal's schema checklist needs them)
    let all_schemas = match list_schemas(&mut client) {
        Ok(s) => s,
        Err(e) => {
            fail(msg_tx, e);
            return Outcome::Stopped;
        }
    };
    // which schemas to actually scan: the configured set (intersected with reality) or all of them
    let monitored: Vec<String> = match &settings.schemas {
        Some(sel) => sel
            .iter()
            .filter(|s| all_schemas.iter().any(|a| a == *s))
            .cloned()
            .collect(),
        None => all_schemas.clone(),
    };

    // --- budget guard: refuse cheaply before pulling anything ---
    let total = match count_meta_rows(&mut client, &monitored) {
        Ok(n) => n,
        Err(e) => {
            fail(msg_tx, e);
            return Outcome::Stopped;
        }
    };
    if total > settings.budget {
        let _ = msg_tx.send(CollectorMsg::Status(CollectorStatus {
            stopped: true,
            last_error: Some(format!(
                "catalog too large: {total} objects+attributes > budget {}",
                settings.budget
            )),
            ..Default::default()
        }));
        let _ = msg_tx.send(CollectorMsg::Log(LogLine {
            time: now_hms(),
            text: format!(
                "stopped: {total} objects+attributes exceeds budget {} — adjust the budget or the monitored schemas, then Enable scan",
                settings.budget
            ),
        }));
        return Outcome::Stopped;
    }

    // --- fingerprint diff: only re-pull schemas whose digest moved ---
    let fps = match schema_fingerprints(&mut client, &monitored) {
        Ok(m) => m,
        Err(e) => {
            fail(msg_tx, e);
            return Outcome::Stopped;
        }
    };
    let changed: Vec<String> = monitored
        .iter()
        .filter(|s| {
            let now = fps.get(*s).map(String::as_str).unwrap_or("");
            let prev = fingerprints.get(*s).map(String::as_str).unwrap_or("");
            now != prev
        })
        .cloned()
        .collect();

    // pull the full object+column set for each changed schema (skip a schema on a transient error)
    let mut fresh: Vec<(String, Vec<MetaObjRow>)> = Vec::with_capacity(changed.len());
    for schema in &changed {
        match scan_schema(&mut client, schema) {
            Ok(rows) => {
                let mapped = rows
                    .into_iter()
                    .map(|(kind, name, cols)| MetaObjRow {
                        schema: schema.clone(),
                        kind,
                        name,
                        cols: cols
                            .into_iter()
                            .map(|(name, ty, nullable, default)| MetaCol { name, ty, nullable, default })
                            .collect(),
                    })
                    .collect();
                fresh.push((schema.clone(), mapped));
            }
            Err(_) => continue, // one bad schema query: skip it, keep the rest
        }
    }

    drop(client); // close the session — no idle connection between scans

    // --- swap the changed schemas into the shared store under the write lock ---
    let monitored_set: std::collections::HashSet<&String> = monitored.iter().collect();
    let changed_set: std::collections::HashSet<&String> = changed.iter().collect();
    let mut wrote = false; // did the store write actually happen (vs a poisoned-lock skip)?
    let (count, est_bytes) = if let Ok(mut store) = shared.store.write() {
        let schemas_changed = store.schemas != all_schemas;
        store.schemas = all_schemas;
        // drop rows for unmonitored / dropped schemas and for the schemas we are about to replace
        let before = store.objects.len();
        store
            .objects
            .retain(|o| monitored_set.contains(&o.schema) && !changed_set.contains(&o.schema));
        let removed = before - store.objects.len();
        let mut added = 0usize;
        for (_, rows) in fresh {
            added += rows.len();
            store.objects.extend(rows);
        }
        // bump the generation (drives the "new data" marker) ONLY on a real change — not every
        // cooldown — so an unchanged catalog doesn't flag the tree as stale every interval
        if schemas_changed || removed > 0 || added > 0 {
            shared.generation.fetch_add(1, Ordering::SeqCst); // under the lock → readers stay consistent
        }
        let count = store.objects.len();
        let est_bytes: usize = store
            .objects
            .iter()
            .map(|o| {
                o.schema.len()
                    + o.kind.len()
                    + o.name.len()
                    + 48
                    + o.cols
                        .iter()
                        .map(|c| c.name.len() + c.ty.len() + c.default.len() + 48)
                        .sum::<usize>()
            })
            .sum();
        wrote = true;
        (count, est_bytes)
    } else {
        (*last_objects, 0) // lock poisoned (a reader panicked) — report stale figures, don't crash
    };

    // remember the new fingerprints (rebuilt → also forgets unmonitored schemas) — but ONLY when the
    // store write happened: on a poisoned-lock skip the changed schemas weren't persisted, so leave
    // the fingerprints stale and let the next scan re-detect and re-pull them once the lock recovers.
    if wrote {
        *fingerprints = monitored
            .iter()
            .map(|s| (s.clone(), fps.get(s).cloned().unwrap_or_default()))
            .collect();
    }

    *last_objects = count;
    // back to idle (active, between scans) — clears the `running` flag
    let _ = msg_tx.send(CollectorMsg::Status(CollectorStatus::default()));
    // one heartbeat line per scan (≈ once per cooldown): a changed-schema summary, or a terse
    // "no changes" so the log visibly confirms the scanner is alive on its interval
    let text = if changed.is_empty() {
        format!(
            "scan: no changes ({count} objects, ~{}) in {} ms",
            fmt_size(est_bytes),
            started.elapsed().as_millis()
        )
    } else {
        format!(
            "scan: rescanned {} schema(s) [{}]; {count} objects, ~{} in {} ms",
            changed.len(),
            changed.join(", "),
            fmt_size(est_bytes),
            started.elapsed().as_millis()
        )
    };
    let _ = msg_tx.send(CollectorMsg::Log(LogLine { time: now_hms(), text }));
    Outcome::Done
}

/// Report a failed scan (connect/query error) without panicking: park the scanner (the caller
/// returns [`Outcome::Stopped`]) with the error on the status and the reason as the last
/// activity-log line. The user resumes via the toolbar Enable scan, which rescans immediately.
fn fail(msg_tx: &Sender<CollectorMsg>, e: String) {
    let _ = msg_tx.send(CollectorMsg::Status(CollectorStatus {
        stopped: true,
        last_error: Some(e.clone()),
        ..Default::default()
    }));
    let _ = msg_tx.send(CollectorMsg::Log(LogLine {
        time: now_hms(),
        text: format!("stopped: scan failed: {e}"),
    }));
}

/// Human-readable byte estimate for the activity log: KB up to ~1 MB, then MB.
fn fmt_size(bytes: usize) -> String {
    if bytes < 1024 * 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
