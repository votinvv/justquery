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
        // min-size задаётся вызывающим (main.rs, ViewportBuilder) — здесь не дублируем, чтобы
        // единственный источник истины (раскладка модалок/островов считает от 1024×600).
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
        {
            apply_rounded_corners(); // Win11 скруглит углы в восстановленном виде (развёрнутое — прямоугольно)
            install_cursor_fix(); // тематический I-beam вместо системного (см. ниже про «белый курсор»)
        }
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

/// Включить скруглённые углы окна в стиле Win11 (DWM). Декорации ОС выключены (свой chrome),
/// поэтому округление задаём вручную через `DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND` —
/// Windows сама держит РАЗВЁРНУТОЕ окно прямоугольным, а ВОССТАНОВЛЕННОЕ — со скруглением.
/// HWND берём перечислением окон GUI-потока: top-level (без владельца) с максимальной площадью —
/// это и есть наше главное окно (так не зависим от заголовка, который меняется при открытии файла).
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
        // SAFETY: hwnd — наше top-level окно (find_main_hwnd); pv указывает на u32 нужного размера.
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

/// HWND нашего главного окна: top-level окно GUI-потока (без владельца) с максимальной площадью —
/// так не зависим от заголовка (он меняется при открытии файла). Общий помощник для оконных
/// твиков (скругление углов, подмена курсора). 0, если окно не найдено.
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
    // SAFETY: EnumThreadWindows вызывает `pick` синхронно для каждого окна нашего потока;
    // lparam — адрес стекового `Best`, живущего до конца вызова.
    unsafe {
        let mut best = Best { hwnd: 0, area: -1 };
        EnumThreadWindows(GetCurrentThreadId(), pick, &mut best as *mut Best as isize);
        best.hwnd
    }
}

// ── Тематический I-beam ──────────────────────────────────────────────────────────────────
//
// Системный курсор «Text Select» на Windows 11 по умолчанию инвертируемый (`beam_i`): цвет он
// берёт, сэмплируя фон под собой, но в GPU-композированном окне (wgpu/DirectComposition + MPO)
// сэмпл не срабатывает и I-beam залипает белым, теряясь на светлом листе и в полях ввода (та же
// беда у Chromium-приложений). egui перекрасить системный курсор не умеет (только enum
// CursorIcon), а winit-овский CustomCursor через eframe недоступен. Поэтому генерируем СВОЙ
// HCURSOR из RGBA в цвете темы (рецепт — winit `WinCursor::new`: 32-bit DDB + 1bpp-маска +
// CreateIconIndirect) и подменяем им системный I-beam в оконной процедуре. При смене темы
// курсор пересоздаётся (см. [`update_ibeam_cursor`], зовётся из update при смене painted-темы).

// Живут всю сессию (одно окно, всё в UI-потоке → Relaxed достаточно): прежняя оконная процедура
// и наш текущий HCURSOR. OLD_PROC != 0 служит признаком «подмена установлена».
#[cfg(windows)]
static OLD_PROC: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
#[cfg(windows)]
static OUR_CURSOR: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Нарисовать I-beam в RGBA цветом темы и собрать из него HCURSOR. 0 при любой ошибке WinAPI.
/// Тело — цвет `theme::p().text` (тёмный в светлой теме, светлый в тёмной), вокруг — 1px обводка
/// контрастного тона, чтобы курсор читался и поверх глифов/выделения. Размер — системный
/// (`SM_CXCURSOR`/`SM_CYCURSOR`), геометрия классическая (стержень + засечки сверху/снизу).
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

    // SAFETY: чистые геттеры метрик; результат зажимаем в разумный диапазон.
    let s = unsafe { GetSystemMetrics(SM_CYCURSOR) }.clamp(16, 128);
    let (w, h) = (s, s);

    // Геометрия снята с системного beam_r (32px) и масштабируется: стержень 1px + засечки двумя
    // сегментами по бокам (разрыв на оси), высота ≈ половина курсора. Тело самих рядов засечки по
    // центру не идёт — там обводка; на торцах стержня обводка вырезается («скос» стыка). Чёткие
    // пиксели, без сглаживания. Центр (cx,cy) — горячая точка.
    let (cx, cy) = (s / 2, s / 2);
    let t = (s / 32).max(1); // толщина линий (ровно 1px при курсоре 32px)
    let total_h = (s / 2).max(8); // высота I-beam от засечки до засечки
    let serif_half = (s / 8).max(2); // полуширина засечки (≈4px при 32)
    let serif_top = cy - total_h / 2;
    let serif_bot = serif_top + total_h - 1;
    let stem_x0 = cx - (t - 1) / 2;

    let is_body = |x: i32, y: i32| -> bool {
        let in_stem = x >= stem_x0 && x < stem_x0 + t;
        // стержень — между засечками (ряды самих засечек по центру уходят в обводку)
        if in_stem && y >= serif_top + t && y <= serif_bot - t {
            return true;
        }
        // засечки — сегменты по бокам от стержня (с разрывом на оси)
        let serif_row =
            (y >= serif_top && y < serif_top + t) || (y > serif_bot - t && y <= serif_bot);
        serif_row && !in_stem && x > cx - serif_half && x < cx + serif_half
    };
    let body_mask: Vec<bool> = (0..w * h).map(|i| is_body(i % w, i / w)).collect();
    let bget = |x: i32, y: i32| -> bool {
        x >= 0 && x < w && y >= 0 && y < h && body_mask[(y * w + x) as usize]
    };

    // Цвета темы (СМЕНА ПО ТЕМЕ): ink — тело = цвет текста (тёмный в светлой, светлый в тёмной),
    // halo — обводка контрастного тона, чтобы курсор читался на любом фоне.
    let pal = theme::p();
    let dark = matches!(theme::current_theme(), theme::AppTheme::Dark);
    let ink = (pal.text.r(), pal.text.g(), pal.text.b());
    let halo: (u8, u8, u8) = if dark { (0, 0, 0) } else { (255, 255, 255) };

    // BGRA (порядок CreateCompatibleBitmap), top-down; форма симметрична — флип строк визуально
    // неотличим. Пиксели сплошные (a=255) либо прозрачные (a=0) — premultiplied не требуется.
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let (col, a) = if bget(x, y) {
                (ink, 255)
            } else {
                // обводка = 1px вокруг тела, КРОМЕ торцов стержня (вырез-«скос»)
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

    // SAFETY: стандартная сборка цветного DDB + 1bpp-маски и CreateIconIndirect (рецепт winit);
    // все хэндлы освобождаются здесь же, hcursor владеет своими копиями.
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
        // 1bpp-маска, строки выровнены по WORD; всё 0xFF — прозрачность задаёт альфа цветного слоя.
        let mask = vec![0xffu8; ((((w + 15) >> 4) << 1) * h) as usize];
        let hbm_mask = CreateBitmap(w, h, 1, 1, mask.as_ptr() as *const c_void);
        if hbm_mask == 0 {
            DeleteObject(hbm_color);
            return 0;
        }
        let ii = IconInfo {
            f_icon: 0, // курсор (не иконка)
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

/// Оконная процедура поверх winit: когда winit поставил системный `IDC_IBEAM` в клиентской
/// области, заменяем его нашим тематическим. Прочие курсоры (стрелка, ресайз, рука) — без изменений.
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
    // SAFETY: вызывается Windows как оконная процедура; OLD_PROC — валидная прежняя WNDPROC.
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

/// Установить подмену I-beam: собрать тематический курсор и подменить оконную процедуру. Разово,
/// после показа окна (см. [`reveal_after_warmup`]). При неудаче — тихий no-op (системный курсор).
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
    // SAFETY: одноразовая установка из UI-потока; cursor_subclass честно зовёт прежнюю WNDPROC.
    unsafe {
        let old = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, cursor_subclass as *const () as isize);
        OLD_PROC.store(old, Relaxed);
    }
}

/// Пересобрать тематический I-beam (после смены темы). No-op, пока подмена не установлена.
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
        return; // подмена ещё не стоит — обновлять нечего
    }
    let new = create_ibeam_hcursor();
    if new == 0 {
        return; // старый курсор оставляем — лучше прежний, чем никакого
    }
    let old = OUR_CURSOR.swap(new, Relaxed);
    // SAFETY: всё в UI-потоке. Если сейчас на экране I-beam (наш старый ИЛИ системный) — сразу
    // ставим новый, затем уничтожаем старый (уже не активный).
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

/// Покадровая страховка: winit ставит курсор не только в `WM_SETCURSOR`, но и напрямую при смене
/// `CursorIcon` (см. winit `Window::set_cursor`) — тогда после смены темы/без движения мыши на
/// экране успевает мелькнуть системный I-beam. Здесь ловим этот случай: если показан системный
/// `IDC_IBEAM`, заменяем нашим. Дёшево (пара вызовов) и срабатывает только когда кадры идут.
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
    // SAFETY: UI-поток; читаем текущий курсор и при системном I-beam подменяем нашим.
    unsafe {
        if GetCursor() == LoadCursorW(0, IDC_IBEAM as *const u16) {
            SetCursor(our);
        }
    }
}
