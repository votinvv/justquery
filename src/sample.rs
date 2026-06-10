//! Demo data for the result-grid smoke tests (`#[cfg(test)]`-only; real runs fill the grid from
//! the live connection).

/// Build a demo result set (`rows` synthesised rows).
pub fn demo_result(rows: usize) -> crate::ResultSet {
    // skip the synthetic "#" column (GRID_COLS[0]); the grid adds its own row numbers
    let columns: Vec<String> = GRID_COLS.iter().skip(1).map(|c| c.0.to_owned()).collect();
    let data: Vec<Vec<String>> = (0..rows)
        .map(|i| GRID_COLS.iter().skip(1).map(|c| grid_cell(i, c.0)).collect())
        .collect();
    crate::ResultSet::new(columns, data)
}

/// Company names used to synthesise demo rows.
const NAMES: [&str; 18] = [
    "Acme Corp", "Globex", "Initech", "Umbrella", "Soylent", "Stark Industries",
    "Wayne Ent", "Wonka Inc", "Cyberdyne", "Tyrell Corp", "Hooli", "Pied Piper",
    "Aperture", "Black Mesa", "Massive Dynamic", "Oscorp", "Gringotts", "Nakatomi",
];

/// Result grid columns: `(title, width, numeric)`. The total width overflows the panel
/// on purpose, so the horizontal scrollbar can be exercised without resizing the window.
pub const GRID_COLS: [(&str, f32, bool); 20] = [
    ("#", 48.0, true),
    ("id", 70.0, true),
    ("full_name", 170.0, false),
    ("email", 240.0, false),
    ("phone", 150.0, false),
    ("country", 90.0, false),
    ("city", 140.0, false),
    ("address", 300.0, false),
    ("segment", 120.0, false),
    ("manager", 170.0, false),
    ("status", 100.0, false),
    ("revenue", 120.0, true),
    ("orders", 90.0, true),
    ("avg_check", 110.0, true),
    ("last_order", 130.0, false),
    ("created_at", 160.0, false),
    ("tags", 160.0, false),
    ("notes", 300.0, false),
    ("description", 320.0, false),
    ("rnk", 70.0, true),
];

/// Synthesise a demo value for grid cell `(row i, column title)`.
pub fn grid_cell(i: usize, title: &str) -> String {
    let name = NAMES[(i * 7) % NAMES.len()];
    match title {
        "#" => format!("{}", i + 1),
        "id" => format!("{}", 1000 + i + 1),
        "full_name" => name.to_owned(),
        "email" => {
            if (i + 1).is_multiple_of(9) {
                "(null)".to_owned()
            } else {
                format!("{}@example.com", name.to_lowercase().replace(' ', ""))
            }
        }
        "phone" => format!("+1 555 {:04}", (i * 37) % 10000),
        "country" => ["US", "UK", "DE", "FR", "JP", "BR"][i % 6].to_owned(),
        "city" => ["New York", "London", "Berlin", "Paris", "Tokyo", "Rio"][i % 6].to_owned(),
        "address" => format!("{} Main St, suite {}", 100 + (i * 13) % 900, (i % 50) + 1),
        "segment" => ["Enterprise", "SMB", "Startup", "Gov"][i % 4].to_owned(),
        "manager" => NAMES[(i * 3) % NAMES.len()].to_owned(),
        "status" => ["active", "paused", "churned"][i % 3].to_owned(),
        "revenue" => format!("{:.2}", 987654.0 / ((i + 1) as f64)),
        "orders" => format!("{}", ((i * 13) % 47) + 3),
        "avg_check" => format!("{:.2}", 120.0 + (i % 50) as f64 * 3.5),
        "last_order" => format!("2026-{:02}-{:02}", (i % 12) + 1, (i % 27) + 1),
        "created_at" => format!("2023-{:02}-{:02}", (i % 12) + 1, (i % 27) + 1),
        "tags" => "vip, eu, b2b".to_owned(),
        "notes" => format!("note #{} - follow up on renewal", i + 1),
        "description" => format!("Account {} - quarterly review pending", 1000 + i + 1),
        "rnk" => format!("{}", i + 1),
        _ => String::new(),
    }
}
