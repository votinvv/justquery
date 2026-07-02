//! Project branding: names, URLs, dialog strings, logo. These parameters are kept here so
//! the rest of the modules contain no references to a specific project.

use eframe::egui;
use egui::{Color32, CornerRadius, Stroke, Vec2};

/// Base exe name: update files `<base>-new.exe` / `<base>.old`, temp directory.
pub const EXE_BASE: &str = "justquery";
/// GitHub API "latest release".
pub const REPO_API_LATEST: &str =
    "https://api.github.com/repos/votinvv/justquery/releases/latest";
/// Direct link to the exe of the latest release.
pub const EXE_DOWNLOAD_URL: &str =
    "https://github.com/votinvv/justquery/releases/latest/download/justquery.exe";
/// Env variable that overrides the update URL (dev/test).
pub const UPDATE_URL_ENV: &str = "JUSTQUERY_UPDATE_URL";
/// User-Agent for requests to GitHub.
pub const USER_AGENT: &str = concat!("JustQuery/", env!("CARGO_PKG_VERSION"));

/// File dialog filter: "label\0mask" pairs, double NUL at the end.
/// "All files" comes first so Open is not restricted to a specific type by default.
pub const FILE_FILTER: &str =
    "All files (*.*)\0*.*\0SQL files (*.sql)\0*.sql\0\0";
pub const OPEN_TITLE: &str = "Open file";
pub const SAVE_TITLE: &str = "Save file";
/// Default extension in "Save as".
pub const SAVE_DEF_EXT: &str = "sql";

/// Clay logo square (shared color of both projects).
const CLAY: Color32 = Color32::from_rgb(0xC9, 0x64, 0x42);

/// Application logo in the UI flow, with side `size` px.
pub fn logo(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(size, size), egui::Sense::hover());
    paint_logo(ui.painter(), rect, 1.0);
}

/// Paint the logo with `pt` into `rect` with opacity `alpha`.
pub fn paint_logo(pt: &egui::Painter, rect: egui::Rect, alpha: f32) {
    // "JQ" monogram — the same geometry as in the application icon
    // (tools/generate_icon.py): a clay rounded square, a white "J" polyline
    // and a "Q" — a ring with a diagonal tail (a magnifier). Coordinates normalized to [0,1].
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
