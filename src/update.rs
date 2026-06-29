//!
//! In-app update check + self-update against GitHub Releases (Windows-only).
//! Repository, file names and User-Agent are per-project, from [`crate::brand`].
//!
//! Flow: a background thread asks the GitHub API for the latest release tag and compares it to the
//! compiled-in version. The status-bar chip shows the verdict. From the About modal the user
//! can download the new exe (published as a flat release asset) and apply it.
//!
//! Applying a running exe on Windows: the file is locked against *opening for write* but can be
//! *renamed* on the same volume while running. So we rename the current exe aside to
//! `<exe>.old`, then copy the freshly downloaded exe into the original path. In a writable
//! location this needs no elevation; in Program Files it fails with PermissionDenied and we redo
//! the swap via an elevated `cmd.exe` (one UAC prompt).
//!
//! Dev/testing override (env): `brand::UPDATE_URL_ENV=<url>` overrides the download URL.

#![allow(non_snake_case)]

use crate::brand;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

pub(crate) const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where the update flow currently is. Lives in the app's `update_status`.
#[derive(Clone, PartialEq)]
pub(crate) enum UpdateStatus {
    NeverChecked,
    Checking,
    Latest,
    /// A newer release was found; the download starts automatically (silent) right after.
    Available { latest: String },
    Downloading { done: u64, total: u64 },
    /// The new exe has been downloaded and staged; waiting for the user to click Install (the only
    /// explicit step — installing may need elevation, so it's a deliberate click, not automatic).
    Downloaded { latest: String },
    Applying,
    PendingRestart,
    /// A step failed. `retry` says which action the clickable retry re-runs.
    Error { msg: String, retry: Retry },
}

/// Which action a failed-update retry should re-run.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Retry {
    /// Re-run the version check (a newer build then auto-downloads). For check / download failures.
    Check,
    /// Re-apply the already-staged exe. For install (elevation) failures — no re-download needed.
    Install,
}

/// Drained in the per-frame poll loop.
pub(crate) enum UpdateMsg {
    CheckDone(Result<CheckResult, String>),
    Progress { done: u64, total: u64 },
    /// Download finished and the exe is staged; carries the version tag (for the `Downloaded` state).
    Downloaded(String),
    Applying,
    Applied,
    /// Check / download failure → the retry re-checks.
    Failed(String),
    /// Install (apply / elevation) failure → the retry re-applies the staged exe.
    InstallFailed(String),
}

#[derive(Clone)]
pub(crate) struct CheckResult {
    pub latest_tag: String, // normalized, e.g. "0.2.0"
    pub is_newer: bool,
}

// ---------------------------------------------------------------------------
// Version comparison
// ---------------------------------------------------------------------------

/// Strip a leading `v`/`V` and any `-prerelease` / `+build` suffix, then parse each `.`-separated
/// component's leading numeric run. Missing/garbage components become 0.
fn parse_semver(tag: &str) -> Vec<u64> {
    let t = tag.trim();
    let t = t.strip_prefix('v').or_else(|| t.strip_prefix('V')).unwrap_or(t);
    // drop pre-release / build metadata
    let core = t.split(['-', '+']).next().unwrap_or(t);
    core.split('.')
        .map(|part| {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u64>().unwrap_or(0)
        })
        .collect()
}

/// True if `latest` is a strictly higher version than `current`.
pub(crate) fn is_newer(current: &str, latest: &str) -> bool {
    let a = parse_semver(current);
    let b = parse_semver(latest);
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if y != x {
            return y > x;
        }
    }
    false
}

/// Display form of a tag: drop a leading `v`.
fn normalize_tag(tag: &str) -> String {
    let t = tag.trim();
    t.strip_prefix('v').or_else(|| t.strip_prefix('V')).unwrap_or(t).to_owned()
}

/// Pull `"tag_name": "..."` out of the releases/latest JSON with a tolerant scan (no serde_json).
fn extract_tag_name(body: &str) -> Option<String> {
    let key = body.find("\"tag_name\"")?;
    let after = &body[key + "\"tag_name\"".len()..];
    let colon = after.find(':')?;
    let rest = &after[colon + 1..];
    let open = rest.find('"')?;
    let val = &rest[open + 1..];
    let close = val.find('"')?;
    let tag = &val[..close];
    if tag.is_empty() {
        None
    } else {
        Some(tag.to_owned())
    }
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

/// A ureq agent using the native-tls stack already in our dependency tree (no second TLS lib).
/// No whole-body timeout on the agent: in ureq 3 `timeout_recv_body` caps the *entire* body
/// transfer, which would abort large exe downloads on slow links (the v2 `timeout_read` was
/// per socket read). The small check request sets its own global timeout instead.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build(),
        )
        .user_agent(brand::USER_AGENT)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .build()
        .new_agent()
}

fn check() -> Result<CheckResult, String> {
    let body = agent()
        .get(brand::REPO_API_LATEST)
        .config()
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("could not reach GitHub: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let tag = extract_tag_name(&body).ok_or("could not read the latest release")?;
    let is_newer = is_newer(CURRENT_VERSION, &tag);
    Ok(CheckResult { latest_tag: normalize_tag(&tag), is_newer })
}

/// Spawn the background version check. Sends exactly one `CheckDone`.
pub(crate) fn spawn_check(tx: Sender<UpdateMsg>) {
    std::thread::spawn(move || {
        let _ = tx.send(UpdateMsg::CheckDone(check()));
    });
}

/// Spawn the background download (silent — no install). Re-checks first (the cached "available" may
/// be stale: the app sat open for days, or the release was pulled / re-tagged); if we're current now
/// it flips back to LATEST. Otherwise streams `Progress`, then finishes with `Downloaded(tag)` or
/// `Failed`. The (possibly elevated) install is a separate, user-triggered step — see [`spawn_install`].
pub(crate) fn spawn_download(tx: Sender<UpdateMsg>) {
    std::thread::spawn(move || {
        let tag = match check() {
            Ok(r) if !r.is_newer => {
                let _ = tx.send(UpdateMsg::CheckDone(Ok(r)));
                return;
            }
            Ok(r) => r.latest_tag, // still newer → proceed to download
            Err(e) => {
                let _ = tx.send(UpdateMsg::Failed(e));
                return;
            }
        };
        let _ = match download(&tx) {
            Ok(()) => tx.send(UpdateMsg::Downloaded(tag)),
            Err(e) => tx.send(UpdateMsg::Failed(e)),
        };
    });
}

/// Spawn the install of the already-staged exe (the swap; one UAC prompt if it lands in Program
/// Files). Sends `Applying`, then `Applied` / `InstallFailed`. No re-download — the staged file from
/// [`spawn_download`] is applied as-is.
pub(crate) fn spawn_install(tx: Sender<UpdateMsg>) {
    std::thread::spawn(move || {
        let staged = match staged_path() {
            Some(p) if p.exists() => p,
            _ => {
                // The staged file is gone (temp cleaner / manual delete / a restart between download
                // and install). Report it as a download-class failure so the retry re-CHECKS (and
                // re-downloads) rather than looping on "retry install" against a missing file.
                let _ = tx.send(UpdateMsg::Failed(
                    "the downloaded update is missing — re-checking".to_owned(),
                ));
                return;
            }
        };
        let _ = tx.send(UpdateMsg::Applying);
        let _ = match apply_update(&staged) {
            Ok(()) => tx.send(UpdateMsg::Applied),
            Err(e) => tx.send(UpdateMsg::InstallFailed(e)),
        };
    });
}

/// Download the latest exe to the staging area (no install). Streams `Progress`; on success the
/// completed file sits at [`staged_path`].
fn download(tx: &Sender<UpdateMsg>) -> Result<(), String> {
    let dir = staging_dir().ok_or("no %APPDATA% directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let part = dir.join(format!("{}-new.exe.part", brand::EXE_BASE));
    let staged = staged_path().ok_or("no %APPDATA% directory")?;
    let _ = std::fs::remove_file(&part);
    let _ = std::fs::remove_file(&staged);

    let url = std::env::var(brand::UPDATE_URL_ENV)
        .unwrap_or_else(|_| brand::EXE_DOWNLOAD_URL.to_owned());
    let resp = agent().get(&url).call().map_err(|e| format!("download failed: {e}"))?;
    let total: u64 = resp
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // `into_reader()` is unlimited (no 10 MB body cap) — required for the exe download.
    let mut reader = resp.into_body().into_reader();
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 64 * 1024];
    let mut done: u64 = 0;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        let _ = tx.send(UpdateMsg::Progress { done, total });
    }
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    if done == 0 {
        return Err("downloaded an empty file".to_owned());
    }
    std::fs::rename(&part, &staged).map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Filesystem / swap
// ---------------------------------------------------------------------------

/// `%APPDATA%\<App>\update\` — staging area for the downloaded exe.
fn staging_dir() -> Option<PathBuf> {
    Some(crate::appdata_dir()?.join("update"))
}

/// Path of the staged downloaded exe (`…\update\<exe>-new.exe`). Present between a finished
/// download and the install.
fn staged_path() -> Option<PathBuf> {
    Some(staging_dir()?.join(format!("{}-new.exe", brand::EXE_BASE)))
}

/// Replace the running exe with `staged`. Tries a plain rename+copy (works in a writable install or
/// the dev tree); on PermissionDenied falls back to an elevated `cmd.exe` doing the same move+copy.
fn apply_update(staged: &Path) -> Result<(), String> {
    let cur = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = cur.parent().ok_or("cannot locate the install directory")?;
    let old = dir.join(format!("{}.old", brand::EXE_BASE));
    let _ = std::fs::remove_file(&old); // clear any leftover from a prior update

    match std::fs::rename(&cur, &old) {
        Ok(()) => match std::fs::copy(staged, &cur) {
            Ok(_) => Ok(()),
            Err(e) => {
                // put the running exe back so the app stays launchable
                let _ = std::fs::rename(&old, &cur);
                Err(format!("could not write the new exe: {e}"))
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            elevated_swap(&cur, &old, staged)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Do the move+copy from an elevated `cmd.exe` (single UAC prompt). ShellExecuteW gives no child
/// exit code, so success is confirmed by polling the filesystem.
fn elevated_swap(cur: &Path, old: &Path, staged: &Path) -> Result<(), String> {
    let params = format!(
        "/c move /y \"{}\" \"{}\" & copy /y \"{}\" \"{}\"",
        cur.display(),
        old.display(),
        staged.display(),
        cur.display()
    );
    let ret = shell_execute_runas("cmd.exe", &params);
    if ret <= 32 {
        return Err(format!("could not start the elevated updater (code {ret})"));
    }
    let want = std::fs::metadata(staged).map(|m| m.len()).unwrap_or(0);
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(200));
        if let Ok(m) = std::fs::metadata(cur) {
            if want > 0 && m.len() == want {
                return Ok(());
            }
        }
    }
    Err("update was not applied — the elevation prompt may have been declined".to_owned())
}

/// Best-effort removal of the leftover `<exe>.old` next to the running exe. Called once at
/// startup; in Program Files this may fail without elevation, which is harmless.
pub(crate) fn startup_cleanup() {
    if let Ok(cur) = std::env::current_exe() {
        if let Some(dir) = cur.parent() {
            let _ = std::fs::remove_file(dir.join(format!("{}.old", brand::EXE_BASE)));
        }
    }
}

// ---------------------------------------------------------------------------
// Win32 FFI (ShellExecuteW with the "runas" verb, same style as src/dialog.rs)
// ---------------------------------------------------------------------------

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: isize,
        lp_operation: *const u16,
        lp_file: *const u16,
        lp_parameters: *const u16,
        lp_directory: *const u16,
        n_show_cmd: i32,
    ) -> isize;
}
const SW_HIDE: i32 = 0;

fn to_wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// Launch `file params` elevated. Returns the raw ShellExecuteW result (>32 = launch ok).
fn shell_execute_runas(file: &str, params: &str) -> isize {
    let op = to_wide("runas");
    let file_w = to_wide(file);
    let params_w = to_wide(params);
    // SAFETY: ShellExecuteW with "runas" verb launches an elevated process.
    // All string args are null-terminated wide strings; nShow=SW_HIDE suppresses
    // the elevation prompt's console window. The return value >32 means success.
    unsafe {
        ShellExecuteW(
            0,
            op.as_ptr(),
            file_w.as_ptr(),
            params_w.as_ptr(),
            std::ptr::null(),
            SW_HIDE,
        )
    }
}

#[cfg(test)]
mod update_tests {
    use super::*;

    #[test]
    fn semver_compare() {
        assert!(is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("0.1.0", "v0.1.1"));
        assert!(is_newer("0.2.0", "0.10.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.2.0", "v0.2.0"));
        assert!(!is_newer("0.2.0", "0.1.9"));
        // missing trailing components are zero
        assert!(!is_newer("0.2.0", "0.2"));
        assert!(is_newer("0.2", "0.2.1"));
        // pre-release suffix is stripped → treated as the final
        assert!(!is_newer("0.2.0", "0.2.0-rc1"));
    }

    #[test]
    fn parse_handles_garbage() {
        assert_eq!(parse_semver("v1.2.3"), vec![1, 2, 3]);
        assert_eq!(parse_semver("1.2.3-beta.4"), vec![1, 2, 3]);
        assert_eq!(parse_semver("garbage"), vec![0]);
    }

    #[test]
    fn tag_name_extraction() {
        let json = r#"{"url":"x","tag_name":"v0.2.0","name":"0.2.0"}"#;
        assert_eq!(extract_tag_name(json).as_deref(), Some("v0.2.0"));
        let spaced = r#"{ "tag_name" :  "0.3.1" }"#;
        assert_eq!(extract_tag_name(spaced).as_deref(), Some("0.3.1"));
        assert_eq!(extract_tag_name(r#"{"name":"x"}"#), None);
    }
}
