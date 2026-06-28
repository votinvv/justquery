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
        apply_rounded_corners(); // Win11 rounds corners in the restored state (maximized stays rectangular)
    }
    *frame += 1;
    ctx.request_repaint(); // warmup must proceed even if the application is idle
}

/// Is the window already shown (warmup finished)? For one-off actions after startup.
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
// input fields (Chromium apps have the same issue). Since egui 0.35 we no longer fight this with a
// hand-rolled HCURSOR + a WNDPROC subclass: egui exposes `Context::set_cursor_image`, and the
// integration uploads the RGBA bitmap to the OS as a real `winit::CustomCursor`. That cursor is a
// straight bitmap — it is NOT sampled/inverted, so it never sticks white. We draw the I-beam in the
// theme color and push it every frame the UI asks for a text cursor (see [`apply_themed_ibeam`]).

/// I-beam side in physical pixels: the system cursor metric (`SM_CYCURSOR`, already at the current
/// DPI), clamped to a sane range. This keeps the bitmap in lock-step with the OS cursor size (incl.
/// the "large cursors" accessibility setting); a fixed-pixel bitmap also stays crisp — winit uploads
/// it at native resolution, with no fractional scaling.
#[cfg(windows)]
fn ibeam_size_px() -> u16 {
    #[link(name = "user32")]
    extern "system" {
        fn GetSystemMetrics(index: i32) -> i32;
    }
    const SM_CYCURSOR: i32 = 14;
    // SAFETY: a pure metric getter, no out-params; called from the UI thread.
    let s = unsafe { GetSystemMetrics(SM_CYCURSOR) };
    s.clamp(16, 128) as u16
}

/// Draw the themed I-beam as a straight-RGBA bitmap cursor (the encoding `egui::CustomCursorImage` /
/// `winit::CustomCursor::from_rgba` expect). The body is the `theme::p().text` color (dark in the
/// light theme, light in the dark one), wrapped in a 1px outline of a contrasting tone so the cursor
/// reads even over glyphs/selection. `size` is the square side in physical pixels, the geometry is
/// classic (stem + serifs top/bottom), and the hotspot is the center.
#[cfg(windows)]
fn build_ibeam_image(size: u16) -> egui::CustomCursorImage {
    use crate::theme;
    let s = size as i32;
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

    // straight (non-premultiplied) RGBA, top-down; the shape is symmetric — a row flip is visually
    // indistinguishable. Pixels are solid (a=255) or transparent (a=0), so premultiplied is a no-op.
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let (col, a) = if bget(x, y) {
                (ink, 255u8)
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
            rgba[i] = col.0;
            rgba[i + 1] = col.1;
            rgba[i + 2] = col.2;
            rgba[i + 3] = a;
        }
    }

    egui::CustomCursorImage {
        rgba: std::sync::Arc::from(rgba),
        size: [size, size],
        hotspot: [cx as u16, cy as u16],
    }
}

/// Hand egui the themed I-beam whenever this frame asked for a text cursor — our custom editor
/// (`codeeditor.rs`) or any standard `TextEdit` / selectable label all funnel through
/// `CursorIcon::Text`, so reading the final `cursor_icon` reproduces the old "beam over text, normal
/// cursor elsewhere" behavior without touching every call site. Anywhere else `cursor_image` stays
/// `None` (egui resets it each frame) and the integration falls back to the normal `cursor_icon`.
///
/// The bitmap is cached and keyed on (theme, size), so the SAME `Arc` is returned across frames —
/// egui-winit dedupes by pointer identity (`Arc::as_ptr`) and never re-uploads it to the OS. A theme
/// change (new key) rebuilds it; so does a DPI change (the metric shifts). Call once per frame, AFTER
/// the whole UI is built, so `cursor_icon` is final.
#[cfg(windows)]
pub fn apply_themed_ibeam(ctx: &egui::Context) {
    use std::cell::RefCell;
    if ctx.output(|o| o.cursor_icon) != egui::CursorIcon::Text {
        return; // not over text — leave cursor_image None; the normal cursor handles the rest
    }
    thread_local! {
        // (theme, size_px) → cached bitmap. Same key → same Arc → no OS re-upload.
        static CACHE: RefCell<Option<((crate::theme::AppTheme, u16), egui::CustomCursorImage)>> =
            const { RefCell::new(None) };
    }
    let key = (crate::theme::current_theme(), ibeam_size_px());
    let image = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((k, img)) = c.as_ref() {
            if *k == key {
                return img.clone(); // cheap: clones the Arc by refcount, same pointer
            }
        }
        let img = build_ibeam_image(key.1);
        *c = Some((key, img.clone()));
        img
    });
    ctx.set_cursor_image(Some(image));
}
