//! On-demand object-attribute fetcher: serves the metadata tab (and, later, editor hints). Runs on
//! its own thread with its OWN connection that opens lazily on the first request and **closes after
//! an idle period** (default 5 min) — so we don't keep a connection on the server when unused.
//! Separate from the collector so a periodic scan never blocks an interactive attribute fetch, and
//! separate from `main_conn` (the kill-switch connection). Always fresh from the DB — no caching.
//!
//! Public contract (frozen): [`start`] → [`DetailsHandle`]; requests via [`DetailsHandle::columns`];
//! replies (matched by `req_id`) via [`DetailsReply`] on `rx`.

use crate::connections::ConnParams;
use crate::metadata::MetaCol;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

/// A request to the details worker.
pub(crate) enum DetailsReq {
    Columns {
        req_id: u64,
        schema: String,
        name: String,
    },
    Shutdown,
}

/// Why a details fetch failed.
pub(crate) enum DetailsErr {
    Deleted,        // the relation no longer exists
    Timeout,        // statement timeout (DB/network slow)
    Other(String),  // any other error
}

/// A reply, correlated to the request by `req_id`.
pub(crate) struct DetailsReply {
    pub req_id: u64,
    pub result: Result<Vec<MetaCol>, DetailsErr>,
}

/// App-side handle to the details worker. Dropping it shuts the worker down.
pub(crate) struct DetailsHandle {
    req: Sender<DetailsReq>,
    pub rx: Receiver<DetailsReply>,
}

impl DetailsHandle {
    /// Request a relation's columns; the reply arrives on `rx` tagged with `req_id`.
    pub fn columns(&self, req_id: u64, schema: String, name: String) {
        let _ = self.req.send(DetailsReq::Columns {
            req_id,
            schema,
            name,
        });
    }
}

impl Drop for DetailsHandle {
    fn drop(&mut self) {
        let _ = self.req.send(DetailsReq::Shutdown);
    }
}

/// Spawn the details worker. `stmt_timeout_ms` bounds each attribute query; the connection idles
/// closed after `idle`.
pub(crate) fn start(params: ConnParams, stmt_timeout_ms: u64, idle: Duration) -> DetailsHandle {
    let (req_tx, req_rx) = std::sync::mpsc::channel::<DetailsReq>();
    let (rep_tx, rep_rx) = std::sync::mpsc::channel::<DetailsReply>();
    std::thread::spawn(move || run(params, stmt_timeout_ms, idle, req_rx, rep_tx));
    DetailsHandle {
        req: req_tx,
        rx: rep_rx,
    }
}

// ============================================================================================
// AGENT B — implement the worker loop below (this is a compiling stub: it replies to every
// request with an Other("not implemented") error so the app runs).
//
// Required behaviour (see module docs + project memory):
//   * Lazy connection: keep `Option<postgres::Client>`. On a request, if None →
//     connections::connect_session(&params); on success run `SET statement_timeout = <ms>` once per
//     freshly opened connection.
//   * Idle close: block on req_rx.recv_timeout(idle). On timeout, drop the client (close the
//     socket) and keep waiting on recv(); reopen lazily on the next request.
//   * Columns: catalog::object_columns(&mut client, &schema, &name):
//       Ok(None)  → DetailsErr::Deleted
//       Ok(Some(rows)) → Ok(rows.map(|(name,ty,nullable,default)| MetaCol{..}))
//       Err(e)    → if e mentions a statement-timeout cancel (SQLSTATE 57014 / "statement timeout")
//                   → DetailsErr::Timeout, else DetailsErr::Other(e). On a hard connection error,
//                   drop the client so the next request reconnects.
//   * Optional: in-flight dedupe (skip a duplicate Columns for the same (schema,name) already
//     queued) — matters more once editor hints arrive; fine to leave simple for v1.
//   * Return on Shutdown / when req_rx is disconnected.
// ============================================================================================
fn run(
    params: ConnParams,
    stmt_timeout_ms: u64,
    idle: Duration,
    req_rx: Receiver<DetailsReq>,
    rep_tx: Sender<DetailsReply>,
) {
    use std::sync::mpsc::RecvTimeoutError;
    let mut client: Option<postgres::Client> = None;
    loop {
        match req_rx.recv_timeout(idle) {
            Ok(DetailsReq::Shutdown) => return,
            Ok(DetailsReq::Columns { req_id, schema, name }) => {
                // ensure a live connection, opening one (with the statement timeout) on demand
                if client.is_none() {
                    match crate::connections::connect_session(&params) {
                        Ok(mut c) => {
                            let _ = c.batch_execute(&format!("SET statement_timeout = {stmt_timeout_ms}"));
                            client = Some(c);
                        }
                        Err(e) => {
                            // leave client None so the next request retries the connect
                            let _ = rep_tx.send(DetailsReply {
                                req_id,
                                result: Err(DetailsErr::Other(e)),
                            });
                            continue;
                        }
                    }
                }
                let c = client.as_mut().expect("client is Some after connect");
                let result = match crate::catalog::object_columns(c, &schema, &name) {
                    Ok(None) => Err(DetailsErr::Deleted),
                    Ok(Some(rows)) => Ok(rows
                        .into_iter()
                        .map(|(name, ty, nullable, default)| MetaCol { name, ty, nullable, default })
                        .collect()),
                    Err(e) => {
                        // connection may be broken or was killed by the timeout — reconnect next time
                        client = None;
                        let lower = e.to_lowercase();
                        if lower.contains("statement timeout")
                            || lower.contains("canceling statement")
                            || lower.contains("57014")
                        {
                            Err(DetailsErr::Timeout)
                        } else {
                            Err(DetailsErr::Other(e))
                        }
                    }
                };
                let _ = rep_tx.send(DetailsReply { req_id, result });
            }
            Err(RecvTimeoutError::Timeout) => client = None, // close the idle connection
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}
