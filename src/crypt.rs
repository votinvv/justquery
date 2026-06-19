//! Per-user secret encryption via the Windows Data Protection API (DPAPI). Used to store saved
//! connection passwords so only the same Windows user can read them back. No extra crates.

#![allow(non_snake_case)]

use core::ffi::c_void;
use std::ptr;

#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[link(name = "crypt32")]
extern "system" {
    fn CryptProtectData(
        p_data_in: *const DataBlob,
        sz_descr: *const u16,
        p_entropy: *const DataBlob,
        reserved: *mut c_void,
        p_prompt: *mut c_void,
        flags: u32,
        p_data_out: *mut DataBlob,
    ) -> i32;
    fn CryptUnprotectData(
        p_data_in: *const DataBlob,
        pp_descr: *mut *mut u16,
        p_entropy: *const DataBlob,
        reserved: *mut c_void,
        p_prompt: *mut c_void,
        flags: u32,
        p_data_out: *mut DataBlob,
    ) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(h_mem: *mut c_void) -> *mut c_void;
}

const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

/// Encrypt `data` for the current Windows user. Returns `None` on failure.
pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
    let input = DataBlob {
        cb_data: data.len() as u32,
        pb_data: data.as_ptr() as *mut u8,
    };
    let mut out = DataBlob {
        cb_data: 0,
        pb_data: ptr::null_mut(),
    };
    // SAFETY: DPAPI calls are safe to make from any thread. We pass:
    //   - input: a valid DataBlob pointing to our byte slice
    //   - p_entropy: null (no entropy, machine-local scope)
    //   - flags: UI_FORBIDDEN (no interactive prompt)
    //   - p_data_out: a zeroed DataBlob that DPAPI fills on success
    // DPAPI allocates pb_data via LocalAlloc; we free with LocalFree below.
    let ok = unsafe {
        CryptProtectData(
            &input,
            ptr::null(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 || out.pb_data.is_null() {
        return None;
    }
    // SAFETY: on success, out.pb_data points to cb_data bytes allocated by DPAPI.
    // We copy the bytes into an owned Vec, then immediately free the DPAPI allocation —
    // before any operation that could panic — so no leak even on OOM.
    let v = unsafe {
        let slice = std::slice::from_raw_parts(out.pb_data, out.cb_data as usize);
        let owned = slice.to_vec();
        LocalFree(out.pb_data as *mut c_void);
        owned
    };
    Some(v)
}

/// Decrypt data produced by [`protect`]. Returns `None` on failure (e.g. another user / corrupt).
pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    let input = DataBlob {
        cb_data: data.len() as u32,
        pb_data: data.as_ptr() as *mut u8,
    };
    let mut out = DataBlob {
        cb_data: 0,
        pb_data: ptr::null_mut(),
    };
    // SAFETY: same contract as protect() — DPAPI allocates output via LocalAlloc.
    // Entropy and description pointers are null; UI is suppressed.
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            ptr::null_mut(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 || out.pb_data.is_null() {
        return None;
    }
    // SAFETY: copy-then-free, same pattern as protect() — no leak if to_vec panics.
    let v = unsafe {
        let slice = std::slice::from_raw_parts(out.pb_data, out.cb_data as usize);
        let owned = slice.to_vec();
        LocalFree(out.pb_data as *mut c_void);
        owned
    };
    Some(v)
}

// ---- hex helpers (config files store bytes as lowercase hex) ----

pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(2) {
        return None;
    }
    let val = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    let mut v = Vec::with_capacity(b.len() / 2);
    let mut i = 0;
    while i < b.len() {
        v.push((val(b[i])? << 4) | val(b[i + 1])?);
        i += 2;
    }
    Some(v)
}
