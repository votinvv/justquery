//! Бренд проекта — всё, чем JustQuery отличается внутри SHARED-файлов (имена, URL-ы,
//! строки диалогов, лого). Общие с Pedant файлы параметризованы отсюда и больше
//! не содержат упоминаний конкретного проекта.

use eframe::egui;
use egui::{Color32, CornerRadius, Stroke, Vec2};

/// Базовое имя exe: файлы обновления `<base>-new.exe` / `<base>.old`, temp-каталог.
pub const EXE_BASE: &str = "justquery";
/// GitHub API «последний релиз».
pub const REPO_API_LATEST: &str =
    "https://api.github.com/repos/votinvv/justquery/releases/latest";
/// Прямая ссылка на exe последнего релиза.
pub const EXE_DOWNLOAD_URL: &str =
    "https://github.com/votinvv/justquery/releases/latest/download/justquery.exe";
/// Env-переменная, переопределяющая URL обновления (dev/тест).
pub const UPDATE_URL_ENV: &str = "JUSTQUERY_UPDATE_URL";
/// User-Agent запросов к GitHub.
pub const USER_AGENT: &str = concat!("JustQuery/", env!("CARGO_PKG_VERSION"));

/// Фильтр файловых диалогов: пары «подпись\0маска», двойной NUL в конце.
/// "All files" первым, чтобы Open не ограничивался *.sql по умолчанию.
pub const FILE_FILTER: &str = "All files (*.*)\0*.*\0SQL files (*.sql)\0*.sql\0\0";
pub const OPEN_TITLE: &str = "Open SQL File";
pub const SAVE_TITLE: &str = "Save SQL File";
/// Расширение по умолчанию в «Сохранить как».
pub const SAVE_DEF_EXT: &str = "sql";

/// Глиняный квадрат лого (общий цвет обоих проектов).
const CLAY: Color32 = Color32::from_rgb(0xC9, 0x64, 0x42);

/// Лого приложения в потоке UI, со стороной `size` px.
pub fn logo(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(size, size), egui::Sense::hover());
    paint_logo(ui.painter(), rect, 1.0);
}

/// Нарисовать лого painter'ом в `rect` с прозрачностью `alpha`.
pub fn paint_logo(pt: &egui::Painter, rect: egui::Rect, alpha: f32) {
    // Монограмма «JQ» — та же геометрия, что в иконке приложения
    // (tools/generate_icon.py): глиняный скруглённый квадрат, белая «J»-полилиния
    // и «Q» — кольцо с диагональным хвостом (лупа). Координаты нормированы к [0,1].
    let clay = CLAY.gamma_multiply(alpha);
    let white = Color32::WHITE.gamma_multiply(alpha);
    let size = rect.width();
    let at = |nx: f32, ny: f32| rect.min + Vec2::new(nx * size, ny * size);

    // clay rounded square (6% margin, 22% corner radius — matches the .ico)
    let sq = egui::Rect::from_min_max(at(0.06, 0.06), at(0.94, 0.94));
    let corner = (sq.width() * 0.22) as u8;
    pt.rect_filled(sq, CornerRadius::same(corner), clay);

    // "J": a single polyline (top bar -> stem -> bottom hook), rounded stroke
    let j_pts: Vec<egui::Pos2> = [
        (0.27, 0.30), (0.46, 0.30),
        (0.43, 0.30), (0.43, 0.60),
        (0.43, 0.635), (0.415, 0.685), (0.375, 0.715),
        (0.325, 0.722), (0.275, 0.700), (0.255, 0.655),
    ]
    .iter()
    .map(|&(x, y)| at(x, y))
    .collect();
    pt.add(egui::Shape::line(j_pts, Stroke::new(0.092 * size, white)));

    // "Q": ring (lens) + short diagonal tail (handle)
    pt.circle_stroke(at(0.66, 0.50), 0.12 * size, Stroke::new(0.09 * size, white));
    pt.line_segment([at(0.685, 0.585), at(0.795, 0.715)], Stroke::new(0.092 * size, white));
}
