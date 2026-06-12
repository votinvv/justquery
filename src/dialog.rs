//! SHARED pedant↔justquery — править синхронно (список общих файлов — в README).
//!
//! Native Win32 Open/Save file dialogs via `comdlg32` (no extra crates, Windows-only).
//! Строки диалогов и фильтр форматов — per-project, из [`crate::brand`].

#![allow(non_snake_case)]
#![allow(dead_code)] // буфер обмена/время — библиотечные хелперы, используются по мере нужды

use std::path::PathBuf;

#[repr(C)]
struct OpenFileNameW {
    l_struct_size: u32,
    hwnd_owner: isize,
    h_instance: isize,
    lpstr_filter: *const u16,
    lpstr_custom_filter: *mut u16,
    n_max_cust_filter: u32,
    n_filter_index: u32,
    lpstr_file: *mut u16,
    n_max_file: u32,
    lpstr_file_title: *mut u16,
    n_max_file_title: u32,
    lpstr_initial_dir: *const u16,
    lpstr_title: *const u16,
    flags: u32,
    n_file_offset: u16,
    n_file_extension: u16,
    lpstr_def_ext: *const u16,
    l_cust_data: isize,
    lpfn_hook: isize,
    lp_template_name: *const u16,
    pv_reserved: *mut core::ffi::c_void,
    dw_reserved: u32,
    flags_ex: u32,
}

#[link(name = "comdlg32")]
extern "system" {
    fn GetOpenFileNameW(lpofn: *mut OpenFileNameW) -> i32;
    fn GetSaveFileNameW(lpofn: *mut OpenFileNameW) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn OpenClipboard(hwnd: isize) -> i32;
    fn GetClipboardData(format: u32) -> isize;
    fn CloseClipboard() -> i32;
    fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, utype: u32) -> i32;
}
#[repr(C)]
struct SystemTimeW {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[link(name = "kernel32")]
extern "system" {
    fn GlobalLock(hmem: isize) -> *const u16;
    fn GlobalUnlock(hmem: isize) -> i32;
    fn GetLocalTime(lp: *mut SystemTimeW);
}
const CF_UNICODETEXT: u32 = 13;

/// Local wall-clock time as "HH:MM:SS" (for the execution-log Time column).
pub fn now_hms() -> String {
    let mut st: SystemTimeW = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    format!("{:02}:{:02}:{:02}", st.hour, st.minute, st.second)
}

/// Local wall-clock as "YYYY-MM-DD HH:MM:SS" (timestamp for the startup-error log).
pub fn now_datetime() -> String {
    let mut st: SystemTimeW = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        st.year, st.month, st.day, st.hour, st.minute, st.second
    )
}

/// Native modal message box (MB_OK + error icon). Used to surface a fatal startup failure: a
/// release build has `windows_subsystem = "windows"`, so there is no console for `main` to print
/// to, and `run_native` may fail before any window exists — without this the process would just
/// vanish on launch (the exact "nothing happens" symptom on machines lacking OpenGL).
pub fn message_box(title: &str, text: &str) {
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (text_w, title_w) = (wide(text), wide(title));
    const MB_OK: u32 = 0x0000_0000;
    const MB_ICONERROR: u32 = 0x0000_0010;
    unsafe {
        MessageBoxW(0, text_w.as_ptr(), title_w.as_ptr(), MB_OK | MB_ICONERROR);
    }
}

/// Read UTF-16 text from the Windows clipboard (used by Edit ▸ Paste from the menu).
pub fn clipboard_text() -> Option<String> {
    unsafe {
        if OpenClipboard(0) == 0 {
            return None;
        }
        let h = GetClipboardData(CF_UNICODETEXT);
        if h == 0 {
            CloseClipboard();
            return None;
        }
        let ptr = GlobalLock(h);
        let out = if ptr.is_null() {
            None
        } else {
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            Some(String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len)))
        };
        GlobalUnlock(h);
        CloseClipboard();
        out
    }
}

// Common dialog flags (commdlg.h).
const OFN_HIDEREADONLY: u32 = 0x0000_0004;
const OFN_OVERWRITEPROMPT: u32 = 0x0000_0002;
const OFN_PATHMUSTEXIST: u32 = 0x0000_0800;
const OFN_FILEMUSTEXIST: u32 = 0x0000_1000;
const OFN_EXPLORER: u32 = 0x0008_0000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn base_ofn(buf: &mut [u16], filter: &[u16], title: &[u16]) -> OpenFileNameW {
    let mut ofn: OpenFileNameW = unsafe { std::mem::zeroed() };
    ofn.l_struct_size = std::mem::size_of::<OpenFileNameW>() as u32;
    ofn.lpstr_filter = filter.as_ptr();
    ofn.n_filter_index = 1;
    ofn.lpstr_file = buf.as_mut_ptr();
    ofn.n_max_file = buf.len() as u32;
    ofn.lpstr_title = title.as_ptr();
    ofn
}

/// Show the native "Open" dialog; returns the chosen path, or `None` if cancelled.
pub fn open_file() -> Option<PathBuf> {
    let filter: Vec<u16> = crate::brand::FILE_FILTER.encode_utf16().collect();
    let title = wide(crate::brand::OPEN_TITLE);
    let mut buf = vec![0u16; 4096];
    let mut ofn = base_ofn(&mut buf, &filter, &title);
    ofn.flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY;
    let ok = unsafe { GetOpenFileNameW(&mut ofn) };
    if ok == 0 {
        return None;
    }
    Some(PathBuf::from(from_wide(&buf)))
}

/// Show the native "Save As" dialog (pre-filled with `suggested` file name); returns the
/// chosen path with the project's default extension, or `None` if cancelled.
pub fn save_file(suggested: Option<&str>) -> Option<PathBuf> {
    let filter: Vec<u16> = crate::brand::FILE_FILTER.encode_utf16().collect();
    let title = wide(crate::brand::SAVE_TITLE);
    let def_ext = wide(crate::brand::SAVE_DEF_EXT);
    let mut buf = vec![0u16; 4096];
    if let Some(name) = suggested {
        for (i, c) in name.encode_utf16().enumerate() {
            if i + 1 < buf.len() {
                buf[i] = c;
            }
        }
    }
    let mut ofn = base_ofn(&mut buf, &filter, &title);
    ofn.lpstr_def_ext = def_ext.as_ptr();
    ofn.flags = OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY;
    let ok = unsafe { GetSaveFileNameW(&mut ofn) };
    if ok == 0 {
        return None;
    }
    Some(PathBuf::from(from_wide(&buf)))
}
