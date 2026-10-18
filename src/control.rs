//! The control connection: the app's one long-lived session to the active server, owned by its own
//! thread (never the UI's, so the frame can't block on it). It does two jobs:
//!   * **liveness** — a cheap ping every [`PING_EVERY`]; when the server is gone the thread reports
//!     [`ControlMsg::Lost`] once and exits, which turns the status-bar identity chip red;
//!   * **the kill switch** — a session that ignores its CancelRequest is ended with
//!     `pg_terminate_backend` over this always-free connection ([`ControlHandle::terminate`]).
//!
//! The query tabs, the metadata scanner and the details fetcher each use their own connections.

use crate::connections::err_chain;
use eframe::egui;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

/// How often the control connection proves the server is still there.
const PING_EVERY: Duration = Duration::from_secs(15);

enum ControlCmd {
    Terminate(i32),
}

/// What the control thread reports back (polled in the update loop).
pub(crate) enum ControlMsg {
    /// The control connection failed: the server is unreachable / restarted / killed the session.
    Lost(String),
}

/// App-side handle to the control thread. Dropping it closes the control connection.
pub(crate) struct ControlHandle {
    cmd: Sender<ControlCmd>,
    pub rx: Receiver<ControlMsg>,
}

impl ControlHandle {
    /// End backend `pid` with `pg_terminate_backend` (best effort; the caller's escalation
    /// abandons the session if even this doesn't settle it).
    pub fn terminate(&self, pid: i32) {
        if pid > 0 {
            let _ = self.cmd.send(ControlCmd::Terminate(pid));
        }
    }
}

/// Hand the freshly opened control connection to its own thread. `ctx` wakes the (possibly idle)
/// UI when the connection is lost, so the red chip shows without waiting for input.
pub(crate) fn start(client: postgres::Client, ctx: egui::Context) -> ControlHandle {
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let (msg_tx, msg_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || run(client, cmd_rx, msg_tx, ctx));
    ControlHandle {
        cmd: cmd_tx,
        rx: msg_rx,
    }
}

fn run(
    mut client: postgres::Client,
    cmd_rx: Receiver<ControlCmd>,
    msg_tx: Sender<ControlMsg>,
    ctx: egui::Context,
) {
    loop {
        match cmd_rx.recv_timeout(PING_EVERY) {
            Ok(ControlCmd::Terminate(pid)) => {
                let _ = client.execute("SELECT pg_terminate_backend($1)", &[&pid]);
            }
            Err(RecvTimeoutError::Timeout) => {}
            // the handle was dropped (disconnect / reconnect): the client closes with the thread
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if let Err(e) = client.simple_query("SELECT 1") {
            let _ = msg_tx.send(ControlMsg::Lost(err_chain(&e)));
            ctx.request_repaint();
            return;
        }
    }
}
