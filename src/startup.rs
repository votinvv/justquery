//!
//! Запуск окна без видимого «распахивания». Окно создаётся СКРЫТЫМ и сразу размером
//! с рабочую область монитора; maximize + показ шлются вместе после короткого прогрева
//! (стабилизация ppp и шрифтового атласа). На Windows winit откладывает maximize
//! скрытого окна до показа — поэтому без предразмера окно видимо разворачивается из
//! маленького. Здесь — общая, не зависящая от проекта механика; заголовок и иконку
//! (per-project) добавляет вызывающий `main()`.

use eframe::egui;
use egui::ViewportBuilder;

/// Кадров прогрева перед показом окна (замер показал: трёх достаточно — ppp и размер
/// стабильны с первого кадра, остаётся прогреть атлас шрифтов/galley).
const WARMUP_FRAMES: u8 = 3;

/// Достроить [`ViewportBuilder`] так, чтобы окно появилось сразу во весь экран без
/// «распахивания»: скрытое, без рамки ОС (кастомный chrome), общий минимальный размер,
/// предразмер — рабочая область монитора (а если её не определить — обычный maximized
/// как fallback). Заголовок и иконку — per-project — навешивает вызывающий ДО этого.
pub fn full_size_hidden_viewport(builder: ViewportBuilder) -> ViewportBuilder {
    let b = builder
        .with_min_inner_size([760.0, 480.0])
        .with_visible(false) // показываемся только полностью свёрстанными
        .with_decorations(false); // кастомный caption bar вместо рамки ОС
    match primary_work_area_points() {
        Some((pos, size)) => b.with_position(pos).with_inner_size(size),
        None => b.with_maximized(true), // fallback: рабочую область не определили
    }
}

/// Покадрово прогреть и показать окно. Вызывать КАЖДЫЙ кадр из `update`; `frame` —
/// счётчик старта в состоянии приложения (u8, начинается с 0). После [`WARMUP_FRAMES`]
/// кадров шлёт maximize + visible одновременно (maximize визуально no-op — размер уже
/// совпал, нужен лишь статус «развёрнуто»). Пока идёт прогрев, держит перерисовку.
pub fn reveal_after_warmup(ctx: &egui::Context, frame: &mut u8) {
    if *frame > WARMUP_FRAMES {
        return;
    }
    if *frame == WARMUP_FRAMES {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        #[cfg(windows)]
        apply_rounded_corners(); // Win11 скруглит углы в восстановленном виде (развёрнутое — прямоугольно)
    }
    *frame += 1;
    ctx.request_repaint(); // прогрев должен идти даже если приложение простаивает
}

/// Окно уже показано (прогрев завершён)? Для разовых действий после старта.
#[allow(dead_code)] // часть API: эта сборка ничего не делает сразу после показа
pub fn revealed(frame: u8) -> bool {
    frame > WARMUP_FRAMES
}

/// Рабочая область основного монитора (без таскбара) в логических пунктах egui:
/// (позиция, размер). Win32: SPI_GETWORKAREA — физические пиксели, делим на системный DPI.
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

/// Включить скруглённые углы окна в стиле Win11 (DWM). Декорации ОС выключены (свой chrome),
/// поэтому округление задаём вручную через `DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND` —
/// Windows сама держит РАЗВЁРНУТОЕ окно прямоугольным, а ВОССТАНОВЛЕННОЕ — со скруглением.
/// HWND берём перечислением окон GUI-потока: top-level (без владельца) с максимальной площадью —
/// это и есть наше главное окно (так не зависим от заголовка, который меняется при открытии файла).
#[cfg(windows)]
pub fn apply_rounded_corners() {
    use core::ffi::c_void;
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
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(hwnd: isize, attr: u32, pv: *const c_void, cb: u32) -> i32;
    }
    const GW_OWNER: u32 = 4;
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: u32 = 2;

    extern "system" fn pick(hwnd: isize, lparam: isize) -> i32 {
        unsafe {
            let best = &mut *(lparam as *mut Best);
            if GetWindow(hwnd, GW_OWNER) != 0 {
                return 1; // пропускаем дочерние/принадлежащие окна (тултипы, IME)
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
    unsafe {
        let mut best = Best { hwnd: 0, area: -1 };
        EnumThreadWindows(GetCurrentThreadId(), pick, &mut best as *mut Best as isize);
        if best.hwnd != 0 {
            let pref = DWMWCP_ROUND;
            DwmSetWindowAttribute(
                best.hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const u32 as *const c_void,
                4,
            );
        }
    }
}
