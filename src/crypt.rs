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
    let v = unsafe { std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec() };
    unsafe { LocalFree(out.pb_data as *mut c_void) };
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
    let v = unsafe { std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec() };
    unsafe { LocalFree(out.pb_data as *mut c_void) };
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
    if b.len() % 2 != 0 {
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
