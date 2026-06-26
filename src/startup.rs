//!
//! Window startup without a visible "unfolding". The window is created HIDDEN and
//! immediately sized to the monitor work area; maximize + show are sent together after a
//! short warmup (ppp and font atlas stabilization). On Windows winit defers the maximize
//! of a hidden window until it is shown — so without a pre-size the window visibly grows
//! from a small one. This is the generic, project-independent mechanics; the title and the
//! icon (per-project) are added by the calling `main()`.

use eframe::egui;
use egui::ViewportBuilder;

/// Warmup frames before showing the window (measurement showed three is enough — ppp and
/// size are stable from the first frame, only the font atlas/galley still needs warming).
const WARMUP_FRAMES: u8 = 3;

/// Finish building the [`ViewportBuilder`] so the window appears full-screen at once without
/// "unfolding": hidden, no OS frame (custom chrome), shared minimum size, pre-size — the
/// monitor work area (and if it cannot be determined — a plain maximized as fallback). The
/// title and icon — per-project — are attached by the caller BEFORE this.
pub fn full_size_hidden_viewport(builder: ViewportBuilder) -> ViewportBuilder {
    let b = builder
        // min-size is set by the caller (main.rs, ViewportBuilder) — we don't duplicate it here, so
        // there is a single source of truth (modal/island layout counts from 1024×600).
        .with_visible(false) // shown only when fully laid out
        .with_decorations(false); // custom caption bar instead of the OS frame
    match primary_work_area_points() {
        Some((pos, size)) => b.with_position(pos).with_inner_size(size),
        None => b.with_maximized(true), // fallback: the work area could not be determined
    }
}

/// Warm up and show the window frame by frame. Call EVERY frame from `update`; `frame` is
/// the startup counter in the application state (u8, starts at 0). After [`WARMUP_FRAMES`]
/// frames it sends maximize + visible at once (maximize is a visual no-op — the size already
/// matches, only the "maximized" status is needed). While warming up, it keeps repainting.
pub fn reveal_after_warmup(ctx: &egui::Context, frame: &mut u8) {
    if *frame > WARMUP_FRAMES {
        return;
    }
    if *frame == WARMUP_FRAMES {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        #[cfg(windows)]
        {
            apply_rounded_corners(); // Win11 rounds corners in the restored state (maximized stays rectangular)
            install_cursor_fix(); // themed I-beam instead of the system one (see "white cursor" below)
        }
    }
    *frame += 1;
    ctx.request_repaint(); // warmup must proceed even if the application is idle
}

/// Is the window already shown (warmup finished)? For one-off actions after startup.
#[allow(dead_code)] // part of the API: this build does nothing immediately after the show
pub fn revealed(frame: u8) -> bool {
    frame > WARMUP_FRAMES
}

/// Work area of the primary monitor (without the taskbar) in logical egui points:
/// (position, size). Win32: SPI_GETWORKAREA returns physical pixels, divided by the system DPI.
#[cfg(windows)]
pub fn primary_work_area_points() -> Option<(egui::Pos2, egui::Vec2)> {
    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn SystemParametersInfoW(
            action: u32,
            param: u32,
            pv: *mut core::ffi::c_void,
            win_ini: u32,
        ) -> i32;
        fn GetDpiForSystem() -> u32;
    }
    // SAFETY:
    //   - SystemParametersInfoW(SPI_GETWORKAREA): writes work-area rect into a
    //     stack-allocated RECT; the pointer cast is the canonical Win32 pattern.
    //     Called from the UI thread only (egui frame setup).
    //   - GetDpiForSystem(): returns the DPI of the primary monitor; thread-safe
    //     on Windows 10+ (the API itself is not, but the system DPI is stable
    //     during the process lifetime and we call it from one thread).
    const SPI_GETWORKAREA: u32 = 0x0030;
    unsafe {
        let mut r = Rect { left: 0, top: 0, right: 0, bottom: 0 };
        if SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut _ as *mut _, 0) == 0 {
            return None;
        }
        let scale = (GetDpiForSystem() as f32 / 96.0).max(0.5);
        if r.right <= r.left || r.bottom <= r.top {
            return None;
        }
        Some((
            egui::pos2(r.left as f32 / scale, r.top as f32 / scale),
            egui::Vec2::new(
                (r.right - r.left) as f32 / scale,
                (r.bottom - r.top) as f32 / scale,
            ),
        ))
    }
}

#[cfg(not(windows))]
pub fn primary_work_area_points() -> Option<(egui::Pos2, egui::Vec2)> {
    None
}

/// Enable Win11-style rounded window corners (DWM). OS decorations are disabled (own chrome),
/// so we set the rounding manually via `DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND` —
/// Windows itself keeps a MAXIMIZED window rectangular and a RESTORED one rounded.
/// We obtain the HWND by enumerating the GUI thread's windows: the top-level one (no owner) with
/// the largest area — that is our main window (this way we don't depend on the title, which
/// changes when a file is opened).
#[cfg(windows)]
pub fn apply_rounded_corners() {
    use core::ffi::c_void;
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(hwnd: isize, attr: u32, pv: *const c_void, cb: u32) -> i32;
    }
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: u32 = 2;

    let hwnd = find_main_hwnd();
    if hwnd != 0 {
        let pref = DWMWCP_ROUND;
        // SAFETY: hwnd is our top-level window (find_main_hwnd); pv points to a u32 of the right size.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const u32 as *const c_void,
                4,
            );
        }
    }
}

/// HWND of our main window: the GUI thread's top-level window (no owner) with the largest area —
/// this way we don't depend on the title (it changes when a file is opened). Shared helper for
/// window tweaks (corner rounding, cursor replacement). 0 if the window is not found.
#[cfg(windows)]
fn find_main_hwnd() -> isize {
    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    struct Best {
        hwnd: isize,
        area: i64,
    }
    #[link(name = "user32")]
    extern "system" {
        fn EnumThreadWindows(
            thread_id: u32,
            cb: extern "system" fn(isize, isize) -> i32,
            lparam: isize,
        ) -> i32;
        fn GetWindowRect(hwnd: isize, r: *mut Rect) -> i32;
        fn GetWindow(hwnd: isize, cmd: u32) -> isize;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadId() -> u32;
    }
    const GW_OWNER: u32 = 4;

    extern "system" fn pick(hwnd: isize, lparam: isize) -> i32 {
        unsafe {
            let best = &mut *(lparam as *mut Best);
            if GetWindow(hwnd, GW_OWNER) != 0 {
                return 1; // skip child/owned windows (tooltips, IME)
            }
            let mut r = Rect { left: 0, top: 0, right: 0, bottom: 0 };
            if GetWindowRect(hwnd, &mut r) == 0 {
                return 1;
            }
            let area = (r.right - r.left) as i64 * (r.bottom - r.top) as i64;
            if area > best.area {
                best.area = area;
                best.hwnd = hwnd;
            }
            1
        }
    }
    // SAFETY: EnumThreadWindows calls `pick` synchronously for each window of our thread;
    // lparam is the address of a stack-allocated `Best` that lives until the call returns.
    unsafe {
        let mut best = Best { hwnd: 0, area: -1 };
        EnumThreadWindows(GetCurrentThreadId(), pick, &mut best as *mut Best as isize);
        best.hwnd
    }
}

// ── Themed I-beam ──────────────────────────────────────────────────────────────────
//
// The "Text Select" system cursor on Windows 11 is invertible by default (`beam_i`): it takes its
// color by sampling the background underneath, but in a GPU-composited window (wgpu/DirectComposition
// + MPO) the sampling does not work and the I-beam sticks white, getting lost on a light sheet and in
// input fields (Chromium apps have the same issue). egui cannot recolor the system cursor (only the
// CursorIcon enum), and winit's CustomCursor is not available through eframe. So we generate OUR OWN
// HCURSOR from RGBA in the theme color (recipe — winit `WinCursor::new`: 32-bit DDB + 1bpp mask +
// CreateIconIndirect) and use it to replace the system I-beam in the window procedure. On a theme
// change the cursor is recreated (see [`update_ibeam_cursor`], called from update when the painted
// theme changes).

// Live for the whole session (one window, everything on the UI thread → Relaxed is enough): the
// previous window procedure and our current HCURSOR. OLD_PROC != 0 serves as the "replacement
// installed" flag.
#[cfg(windows)]
static OLD_PROC: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
#[cfg(windows)]
static OUR_CURSOR: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Draw an I-beam in RGBA using the theme color and build an HCURSOR from it. 0 on any WinAPI error.
/// The body is the `theme::p().text` color (dark in the light theme, light in the dark one), surrounded
/// by a 1px outline of a contrasting tone so the cursor reads even over glyphs/selection. The size is
/// the system one (`SM_CXCURSOR`/`SM_CYCURSOR`), the geometry is classic (stem + serifs top/bottom).
#[cfg(windows)]
fn create_ibeam_hcursor() -> isize {
    use crate::theme;
    use core::ffi::c_void;
    #[repr(C)]
    struct IconInfo {
        f_icon: i32,
        x_hotspot: u32,
        y_hotspot: u32,
        hbm_mask: isize,
        hbm_color: isize,
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetSystemMetrics(index: i32) -> i32;
        fn GetDC(hwnd: isize) -> isize;
        fn ReleaseDC(hwnd: isize, dc: isize) -> i32;
        fn CreateIconIndirect(ii: *const IconInfo) -> isize;
    }
    #[link(name = "gdi32")]
    extern "system" {
        fn CreateCompatibleBitmap(dc: isize, w: i32, h: i32) -> isize;
        fn CreateBitmap(w: i32, h: i32, planes: u32, bpp: u32, bits: *const c_void) -> isize;
        fn SetBitmapBits(hbm: isize, cb: u32, bits: *const c_void) -> i32;
        fn DeleteObject(o: isize) -> i32;
    }
    const SM_CYCURSOR: i32 = 14;

    // SAFETY: pure metric getters; we clamp the result to a sane range.
    let s = unsafe { GetSystemMetrics(SM_CYCURSOR) }.clamp(16, 128);
    let (w, h) = (s, s);

    // The geometry is taken from the system beam_r (32px) and scaled: a 1px stem + serifs as two
    // segments on the sides (a gap on the axis), height ≈ half the cursor. The body of the serif rows
    // themselves does not run through the center — there is the outline there; at the stem ends the
    // outline is cut out (a "bevel" of the joint). Crisp pixels, no anti-aliasing. The center (cx,cy)
    // is the hotspot.
    let (cx, cy) = (s / 2, s / 2);
    let t = (s / 32).max(1); // line thickness (exactly 1px at a 32px cursor)
    let total_h = (s / 2).max(8); // I-beam height from serif to serif
    let serif_half = (s / 8).max(2); // half-width of a serif (≈4px at 32)
    let serif_top = cy - total_h / 2;
    let serif_bot = serif_top + total_h - 1;
    let stem_x0 = cx - (t - 1) / 2;

    let is_body = |x: i32, y: i32| -> bool {
        let in_stem = x >= stem_x0 && x < stem_x0 + t;
        // the stem — between the serifs (the serif rows themselves go into the outline at the center)
        if in_stem && y >= serif_top + t && y <= serif_bot - t {
            return true;
        }
        // the serifs — segments on the sides of the stem (with a gap on the axis)
        let serif_row =
            (y >= serif_top && y < serif_top + t) || (y > serif_bot - t && y <= serif_bot);
        serif_row && !in_stem && x > cx - serif_half && x < cx + serif_half
    };
    let body_mask: Vec<bool> = (0..w * h).map(|i| is_body(i % w, i / w)).collect();
    let bget = |x: i32, y: i32| -> bool {
        x >= 0 && x < w && y >= 0 && y < h && body_mask[(y * w + x) as usize]
    };

    // Theme colors (CHANGE WITH THEME): ink — body = text color (dark in light, light in dark),
    // halo — an outline of a contrasting tone so the cursor reads on any background.
    let pal = theme::p();
    let dark = matches!(theme::current_theme(), theme::AppTheme::Dark);
    let ink = (pal.text.r(), pal.text.g(), pal.text.b());
    let halo: (u8, u8, u8) = if dark { (0, 0, 0) } else { (255, 255, 255) };

    // BGRA (CreateCompatibleBitmap order), top-down; the shape is symmetric — a row flip is visually
    // indistinguishable. Pixels are solid (a=255) or transparent (a=0) — premultiplied is not needed.
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let (col, a) = if bget(x, y) {
                (ink, 255)
            } else {
                // the outline = 1px around the body, EXCEPT the stem ends (the "bevel" cutout)
                let in_stem = x >= stem_x0 && x < stem_x0 + t;
                let notch = in_stem && (y == serif_top - 1 || y == serif_bot + 1);
                let touch = !notch
                    && (-1..=1)
                        .any(|dy| (-1..=1).any(|dx| (dx != 0 || dy != 0) && bget(x + dx, y + dy)));
                if touch {
                    (halo, 255)
                } else {
                    ((0, 0, 0), 0)
                }
            };
            let i = ((y * w + x) * 4) as usize;
            bgra[i] = col.2;
            bgra[i + 1] = col.1;
            bgra[i + 2] = col.0;
            bgra[i + 3] = a;
        }
    }

    // SAFETY: the standard build of a color DDB + a 1bpp mask and CreateIconIndirect (winit recipe);
    // all handles are released right here, hcursor owns its own copies.
    unsafe {
        let dc = GetDC(0);
        if dc == 0 {
            return 0;
        }
        let hbm_color = CreateCompatibleBitmap(dc, w, h);
        ReleaseDC(0, dc);
        if hbm_color == 0 {
            return 0;
        }
        if SetBitmapBits(hbm_color, bgra.len() as u32, bgra.as_ptr() as *const c_void) == 0 {
            DeleteObject(hbm_color);
            return 0;
        }
        // 1bpp mask, rows WORD-aligned; all 0xFF — transparency is set by the color layer's alpha.
        let mask = vec![0xffu8; ((((w + 15) >> 4) << 1) * h) as usize];
        let hbm_mask = CreateBitmap(w, h, 1, 1, mask.as_ptr() as *const c_void);
        if hbm_mask == 0 {
            DeleteObject(hbm_color);
            return 0;
        }
        let ii = IconInfo {
            f_icon: 0, // cursor (not an icon)
            x_hotspot: cx as u32,
            y_hotspot: cy as u32,
            hbm_mask,
            hbm_color,
        };
        let hcursor = CreateIconIndirect(&ii);
        DeleteObject(hbm_color);
        DeleteObject(hbm_mask);
        hcursor
    }
}

/// A window procedure on top of winit: when winit set the system `IDC_IBEAM` in the client
/// area, we replace it with our themed one. Other cursors (arrow, resize, hand) are left unchanged.
#[cfg(windows)]
extern "system" fn cursor_subclass(hwnd: isize, msg: u32, w: usize, l: isize) -> isize {
    use std::sync::atomic::Ordering::Relaxed;
    #[link(name = "user32")]
    extern "system" {
        fn CallWindowProcW(prev: isize, hwnd: isize, msg: u32, w: usize, l: isize) -> isize;
        fn GetCursor() -> isize;
        fn SetCursor(h: isize) -> isize;
        fn LoadCursorW(hinst: isize, name: *const u16) -> isize;
    }
    const WM_SETCURSOR: u32 = 0x0020;
    const HTCLIENT: u32 = 1;
    const IDC_IBEAM: usize = 32513;
    // SAFETY: called by Windows as a window procedure; OLD_PROC is the valid previous WNDPROC.
    unsafe {
        let r = CallWindowProcW(OLD_PROC.load(Relaxed), hwnd, msg, w, l);
        if msg == WM_SETCURSOR && (l as u32 & 0xffff) == HTCLIENT {
            let our = OUR_CURSOR.load(Relaxed);
            if our != 0 && GetCursor() == LoadCursorW(0, IDC_IBEAM as *const u16) {
                SetCursor(our);
            }
        }
        r
    }
}

/// Install the I-beam replacement: build the themed cursor and replace the window procedure. Once,
/// after the window is shown (see [`reveal_after_warmup`]). On failure — a silent no-op (system cursor).
#[cfg(windows)]
pub fn install_cursor_fix() {
    use std::sync::atomic::Ordering::Relaxed;
    #[link(name = "user32")]
    extern "system" {
        fn SetWindowLongPtrW(hwnd: isize, index: i32, val: isize) -> isize;
    }
    const GWLP_WNDPROC: i32 = -4;

    let hwnd = find_main_hwnd();
    if hwnd == 0 {
        return;
    }
    let cur = create_ibeam_hcursor();
    if cur == 0 {
        return;
    }
    OUR_CURSOR.store(cur, Relaxed);
    // SAFETY: a one-time install from the UI thread; cursor_subclass honestly calls the previous WNDPROC.
    unsafe {
        let old = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, cursor_subclass as *const () as isize);
        OLD_PROC.store(old, Relaxed);
    }
}

/// Rebuild the themed I-beam (after a theme change). A no-op until the replacement is installed.
#[cfg(windows)]
pub fn update_ibeam_cursor() {
    use std::sync::atomic::Ordering::Relaxed;
    #[link(name = "user32")]
    extern "system" {
        fn GetCursor() -> isize;
        fn SetCursor(h: isize) -> isize;
        fn LoadCursorW(hinst: isize, name: *const u16) -> isize;
        fn DestroyCursor(h: isize) -> i32;
    }
    const IDC_IBEAM: usize = 32513;
    if OLD_PROC.load(Relaxed) == 0 {
        return; // the replacement is not installed yet — nothing to update
    }
    let new = create_ibeam_hcursor();
    if new == 0 {
        return; // keep the old cursor — the previous one is better than none
    }
    let old = OUR_CURSOR.swap(new, Relaxed);
    // SAFETY: everything on the UI thread. If an I-beam is currently on screen (our old OR the system
    // one) — set the new one right away, then destroy the old one (already inactive).
    unsafe {
        let cur = GetCursor();
        if cur == old || cur == LoadCursorW(0, IDC_IBEAM as *const u16) {
            SetCursor(new);
        }
        if old != 0 {
            DestroyCursor(old);
        }
    }
}

/// A per-frame safeguard: winit sets the cursor not only in `WM_SETCURSOR` but also directly on a
/// `CursorIcon` change (see winit `Window::set_cursor`) — then after a theme change / without mouse
/// movement the system I-beam manages to flash on screen. Here we catch that case: if the system
/// `IDC_IBEAM` is shown, we replace it with ours. Cheap (a couple of calls) and only fires while
/// frames are running.
#[cfg(windows)]
pub fn tick_ibeam() {
    use std::sync::atomic::Ordering::Relaxed;
    #[link(name = "user32")]
    extern "system" {
        fn GetCursor() -> isize;
        fn SetCursor(h: isize) -> isize;
        fn LoadCursorW(hinst: isize, name: *const u16) -> isize;
    }
    const IDC_IBEAM: usize = 32513;
    let our = OUR_CURSOR.load(Relaxed);
    if our == 0 {
        return;
    }
    // SAFETY: UI thread; we read the current cursor and replace it with ours on a system I-beam.
    unsafe {
        if GetCursor() == LoadCursorW(0, IDC_IBEAM as *const u16) {
            SetCursor(our);
        }
    }
}
