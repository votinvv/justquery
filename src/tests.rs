//! Regression tests — run with `cargo test`.
//!
//! Two layers, no extra crates / no GPU:
//!   * logic tests for the search, tab and helper behaviour;
//!   * headless "smoke" tests that render each screen through `egui::Context::run` so a panic
//!     or layout error (clipping, `min_rect`, sticky-header maths, …) fails the build.

use super::*;
use std::path::Path;

fn app_with_sql(sql: &str) -> JustQueryApp {
    let mut a = JustQueryApp::default();
    a.new_tab();
    a.tabs[0].sql = sql.to_owned();
    a
}

// ---------------------------------------------------------------- find

#[test]
fn find_case_insensitive_ascii() {
    let mut a = app_with_sql("foo Foo FOO bar");
    a.find_query = "foo".into();
    a.caret = 0;
    a.find_match_start = None;
    a.find_run(false);
    assert_eq!(a.find_count, 3);
    assert_eq!(a.find_match_start, Some(0)); // nearest to the caret
}

#[test]
fn find_cyrillic_case_insensitive() {
    // the bug that started this: ASCII-only folding never matched Cyrillic
    let mut a = app_with_sql("Под подвал ПОДарок");
    a.find_query = "под".into();
    a.find_case = false;
    a.caret = 0;
    a.find_match_start = None;
    a.find_run(false);
    assert_eq!(a.find_count, 3);
}

#[test]
fn find_case_sensitive() {
    let mut a = app_with_sql("Под под ПОД");
    a.find_query = "Под".into();
    a.find_case = true;
    a.caret = 0;
    a.find_match_start = None;
    a.find_run(false);
    assert_eq!(a.find_count, 1);
}

#[test]
fn find_whole_word() {
    let mut a = app_with_sql("cat cats category cat");
    a.find_query = "cat".into();
    a.find_whole_word = true;
    a.caret = 0;
    a.find_match_start = None;
    a.find_run(false);
    assert_eq!(a.find_count, 2);
}

#[test]
fn find_nearest_to_caret() {
    let mut a = app_with_sql("ab....ab....ab"); // matches at 0, 6, 12
    a.find_query = "ab".into();
    a.caret = 7;
    a.find_match_start = None;
    a.find_run(false);
    assert_eq!(a.find_match_start, Some(6));
}

#[test]
fn find_navigation_and_wrap() {
    let mut a = app_with_sql("x ab y ab z ab"); // matches at 2, 7, 12
    a.find_query = "ab".into();
    a.find_wrap = true;
    a.caret = 0;
    a.find_match_start = None;
    a.find_run(false);
    assert_eq!(a.find_match_start, Some(2));
    a.find_run(false);
    assert_eq!(a.find_match_start, Some(7));
    a.find_run(false);
    assert_eq!(a.find_match_start, Some(12));
    a.find_run(false); // wrap forward
    assert_eq!(a.find_match_start, Some(2));
    a.find_run(true); // wrap backward
    assert_eq!(a.find_match_start, Some(12));
}

// ---------------------------------------------------------------- tabs / helpers

#[test]
fn new_tabs_are_untitled() {
    let mut a = JustQueryApp::default();
    a.new_tab();
    a.new_tab();
    assert_eq!(a.tabs.len(), 2);
    assert_eq!(a.tabs[0].title, "Untitled");
    assert_eq!(a.tabs[1].title, "Untitled");
    assert_eq!(a.active_tab, 1);
}

#[test]
fn close_tab_activates_previous() {
    let mut a = JustQueryApp::default();
    a.new_tab();
    a.new_tab();
    a.new_tab();
    a.close_tab(2);
    assert_eq!(a.tabs.len(), 2);
    assert_eq!(a.active_tab, 1);
    a.close_tab(0);
    assert_eq!(a.tabs.len(), 1);
    assert_eq!(a.active_tab, 0);
}

#[test]
fn title_from_path_uses_filename() {
    assert_eq!(
        JustQueryApp::title_from_path(Path::new(r"C:\queries\report.sql")),
        "report.sql"
    );
}

#[test]
fn highlighter_keeps_text() {
    let job = crate::highlight::highlight_sql("select 1 from t -- c", 12.0);
    assert!(!job.sections.is_empty());
    assert_eq!(job.text, "select 1 from t -- c");
}

#[test]
fn sample_grid_shape() {
    assert_eq!(crate::sample::GRID_COLS.len(), 20);
    assert_eq!(crate::sample::grid_cell(0, "#"), "1");
    assert!(!crate::sample::grid_cell(5, "email").is_empty());
}

// ---------------------------------------------------------------- headless render smoke tests

fn test_ctx() -> egui::Context {
    let ctx = egui::Context::default();
    theme::setup_fonts(&ctx);
    theme::apply(&ctx, &theme::LIGHT);
    ctx
}

#[test]
fn end_key_lands_at_true_line_end_with_tabs() {
    // A tab is one char but renders several cells wide, so `col * char_w` puts the End caret in the
    // MIDDLE of the line. The caret x must come from the line galley instead — it should sit at the
    // line's real right edge (caret_local_x ≈ line width), not short of it.
    let mut app = JustQueryApp::default();
    app.new_tab();
    app.tabs[0].sql = "a\t".repeat(400); // 800 chars, but far wider once tabs expand
    app.focus_editor = true;
    let ctx = test_ctx();
    for _ in 0..3 {
        let _ = ctx.run_ui(test_input(), |ui| app.main_screen(ui));
    }
    let mut input = test_input();
    input.events = vec![egui::Event::Key {
        key: egui::Key::End,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }];
    let _ = ctx.run_ui(input, |ui| app.main_screen(ui));
    for _ in 0..3 {
        let _ = ctx.run_ui(test_input(), |ui| app.main_screen(ui));
    }
    let (cx, vl, vr, lx, lw): (f32, f32, f32, f32, f32) =
        ctx.data(|d| d.get_temp(egui::Id::new("dbg_caret"))).unwrap();
    // caret is at the line's true end (galley width), not at col*char_w (which would be ~half of it)
    assert!((lx - lw).abs() < 1.0, "End caret x {lx} not at line end {lw}");
    assert!(cx >= vl && cx <= vr, "End caret {cx} not in view [{vl},{vr}]");
}

#[test]
fn end_key_reveals_line_end() {
    for ppp in [1.0_f32, 1.5] {
        let mut app = JustQueryApp::default();
        app.new_tab();
        app.tabs[0].sql = "0".repeat(10000);
        app.focus_editor = true;
        let ctx = test_ctx();
        ctx.set_pixels_per_point(ppp);
        for _ in 0..3 {
            let _ = ctx.run_ui(test_input(), |ui| app.main_screen(ui));
        }
        // a SINGLE End press, then a few idle frames to let the scroll converge
        let mut input = test_input();
        input.events = vec![egui::Event::Key {
            key: egui::Key::End,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];
        let _ = ctx.run_ui(input, |ui| app.main_screen(ui));
        for _ in 0..3 {
            let _ = ctx.run_ui(test_input(), |ui| app.main_screen(ui));
        }
        let (cx, vl, vr, _lx, _lw): (f32, f32, f32, f32, f32) =
            ctx.data(|d| d.get_temp(egui::Id::new("dbg_caret"))).unwrap();
        // the caret at End must sit comfortably inside the viewport, not flush against the right
        // edge (where it overlaps the border and reads as "the second-to-last char").
        assert!(
            cx >= vl && vr - cx >= 4.0,
            "ppp={ppp}: caret {cx} not comfortably in view [{vl},{vr}] (gap {})",
            vr - cx
        );
    }
}

fn test_input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1280.0, 800.0),
        )),
        ..Default::default()
    }
}

/// Render a few frames of the main screen (+ all overlays) — panics fail the test.
fn render_main(app: &mut JustQueryApp, frames: usize) {
    let ctx = test_ctx();
    for _ in 0..frames {
        let _ = ctx.run_ui(test_input(), |ui| {
            // main_screen renders the chrome + manager + editor into the root ui; the floating
            // modals attach to the context (Window/Area), so they take ctx.
            let ctx = &ui.ctx().clone();
            app.main_screen(ui);
            app.about_modal(ctx);
            app.connect_modal(ctx);
            app.no_conn_modal(ctx);
        });
    }
}

#[test]
fn smoke_editor_renders() {
    let mut app = JustQueryApp::default();
    app.new_tab();
    app.tabs[0].sql = "select id, name\nfrom users\nwhere active = true;".into();
    render_main(&mut app, 3);
}

#[test]
fn completion_columns_after_alias_dot() {
    use crate::metadata::{MetaCol, MetaObjRow, MetaStore};
    let mut app = JustQueryApp::default();
    app.new_tab();
    let tab_id = app.tabs[0].id;
    app.meta_view = MetaStore {
        schemas: vec!["public".to_owned()],
        objects: vec![MetaObjRow {
            schema: "public".to_owned(),
            kind: "Tables".to_owned(),
            name: "users".to_owned(),
            cols: vec![
                MetaCol { name: "id".to_owned(), ty: "int4".to_owned(), nullable: false, default: String::new() },
                MetaCol { name: "name".to_owned(), ty: "text".to_owned(), nullable: true, default: String::new() },
            ],
        }],
    };
    let sql = "select * from public.users u where u.";
    app.ac_build(sql, sql.chars().count(), tab_id);
    assert!(app.ac.open);
    let cols: Vec<_> = app.ac.items.iter().map(|i| i.insert.clone()).collect();
    assert_eq!(cols, vec!["id".to_owned(), "name".to_owned()]);
}

#[test]
fn editor_select_all_copy_keeps_text() {
    // repro guard: a tall buffer, then select-all + copy several times. Must not panic and must
    // not mutate the text (the scroll-clamp path runs here; over-scroll is fixed in `editor`).
    let mut app = JustQueryApp::default();
    app.new_tab();
    app.tabs[0].sql = "x\n".repeat(240);
    app.focus_editor = true;
    let ctx = test_ctx();
    for _ in 0..3 {
        let _ = ctx.run_ui(test_input(), |ui| {
            app.main_screen(ui);
        });
    }
    for _ in 0..4 {
        let mut input = test_input();
        input.events = vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            },
            egui::Event::Copy,
        ];
        let _ = ctx.run_ui(input, |ui| {
            app.main_screen(ui);
        });
    }
    assert_eq!(app.tabs[0].sql.matches('\n').count(), 240);
}

#[test]
fn completion_tables_after_system_schema_dot() {
    // pg_catalog is a system schema: it may carry scanned objects but not appear in `schemas`.
    // `schema.` must still list its relations (regression: it was resolved as a table → empty).
    use crate::metadata::{MetaObjRow, MetaStore};
    let mut app = JustQueryApp::default();
    app.new_tab();
    let tab_id = app.tabs[0].id;
    app.meta_view = MetaStore {
        schemas: vec![], // hidden from the dropdown
        objects: vec![MetaObjRow {
            schema: "pg_catalog".to_owned(),
            kind: "Tables".to_owned(),
            name: "pg_class".to_owned(),
            cols: vec![],
        }],
    };
    let sql = "select * from pg_catalog.";
    app.ac_build(sql, sql.chars().count(), tab_id);
    assert!(app.ac.open);
    assert!(app.ac.items.iter().any(|i| i.insert == "pg_class"));
}

#[test]
fn completion_schemas_on_empty_context() {
    use crate::metadata::MetaStore;
    let mut app = JustQueryApp::default();
    app.new_tab();
    app.meta_view = MetaStore { schemas: vec!["public".to_owned(), "analytics".to_owned()], objects: vec![] };
    app.ac.request = true; // F6
    render_main(&mut app, 2);
    assert!(app.ac.open);
    assert!(app.ac.items.iter().any(|i| i.insert == "public"));
}

#[test]
fn smoke_result_grid_renders() {
    let mut app = JustQueryApp::default();
    app.new_tab();
    if let Some(t) = app.cur_mut() {
        t.executed = true;
        t.result_tab = 1;
        t.results = vec![crate::sample::demo_result(120)]; // a result set → virtualized grid
    }
    render_main(&mut app, 3);
}

#[test]
fn smoke_result_grid_maximized() {
    let mut app = JustQueryApp::default();
    app.new_tab();
    if let Some(t) = app.cur_mut() {
        t.executed = true;
        t.result_tab = 1;
        t.results = vec![crate::sample::demo_result(120)];
        t.result_full = true;
    }
    render_main(&mut app, 2);
}

#[test]
fn smoke_find_and_about() {
    let mut app = JustQueryApp::default();
    app.new_tab();
    app.open_find();
    app.about_open = true;
    render_main(&mut app, 3);
}

#[test]
fn smoke_connection_dialogs() {
    let mut app = JustQueryApp::default();
    app.connections.push(Connection {
        id: 1,
        name: "local".into(),
        host: "localhost".into(),
        port: "5432".into(),
        db: "app".into(),
        user: "me".into(),
        password: String::new(),
        created: 0,
        ..Default::default()
    });
    app.left_panel = Some(LeftPanel::Database); // manager side panel
    app.connect_open = true;
    app.no_conn_open = true;
    render_main(&mut app, 3);
}

#[test]
fn smoke_connection_tab() {
    // a connection tab renders its settings form instead of the SQL editor
    let mut app = JustQueryApp::default();
    app.left_panel = Some(LeftPanel::Database);
    app.open_conn_tab(Connection {
        port: "5432".into(),
        ..Default::default()
    });
    assert!(app.cur().map_or(false, |t| t.conn.is_some()));
    render_main(&mut app, 3);
}

#[test]
fn connection_tab_save_commits() {
    let mut app = JustQueryApp::default();
    app.open_conn_tab(Connection {
        name: "newconn".into(),
        ..Default::default()
    });
    assert!(app.cur().map_or(false, |t| t.dirty)); // new connection starts dirty
    app.commit_conn_tab(); // in-memory only (no disk write in tests)
    assert_eq!(app.connections.len(), 1);
    assert_eq!(app.connections[0].name, "newconn");
    assert!(app.connections[0].id != 0);
    assert!(app.cur().map_or(false, |t| !t.dirty)); // saved → clean
}

// ---------------------------------------------------------------- statement splitter

#[test]
fn split_basic_and_trailing() {
    let s = crate::connections::split_statements("select 1; select 2");
    assert_eq!(s, vec!["select 1".to_owned(), "select 2".to_owned()]);
}

#[test]
fn split_keeps_semicolon_in_string() {
    let s = crate::connections::split_statements("select 'a;b'; select 2");
    assert_eq!(s, vec!["select 'a;b'".to_owned(), "select 2".to_owned()]);
}

#[test]
fn split_keeps_semicolon_in_dollar_quote() {
    // a function body / DO block must not be torn apart on its inner semicolons
    let sql = "do $$ begin perform 1; perform 2; end $$; select 9";
    let s = crate::connections::split_statements(sql);
    assert_eq!(s.len(), 2);
    assert_eq!(s[1], "select 9");
    assert!(s[0].contains("perform 1; perform 2"));
}

#[test]
fn split_keeps_semicolon_in_comments() {
    let s = crate::connections::split_statements("select 1 -- a; b\n; /* c; d */ select 2");
    assert_eq!(s.len(), 2);
    assert!(s[0].starts_with("select 1"));
    assert_eq!(s[1], "/* c; d */ select 2");
}

#[test]
fn split_dollar_param_is_not_a_tag() {
    // $1 is a parameter placeholder, not a dollar-quote opener — the ; still splits
    let s = crate::connections::split_statements("select $1; select 2");
    assert_eq!(s.len(), 2);
}

// ---------------------------------------------------------------- metadata

#[test]
fn smoke_metadata_panel_renders() {
    use crate::metadata::{MetaObjRow, MetaStore};
    let mut app = JustQueryApp::default();
    app.left_panel = Some(LeftPanel::Metadata);
    app.connected = true;
    let store = MetaStore {
        schemas: vec!["public".to_owned()],
        objects: vec![
            MetaObjRow { schema: "public".to_owned(), kind: "Tables".to_owned(), name: "users".to_owned(), cols: vec![] },
            MetaObjRow { schema: "public".to_owned(), kind: "Views".to_owned(), name: "v_users".to_owned(), cols: vec![] },
        ],
    };
    app.meta_store = std::sync::Arc::new(crate::metadata::SharedStore {
        store: std::sync::RwLock::new(store.clone()),
        generation: std::sync::atomic::AtomicU64::new(1),
    });
    app.meta_view = store;
    app.meta_view_gen = 1;
    app.meta_schema_sel = Some("public".to_owned());
    app.meta_folders_open.insert("public/Tables".to_owned());
    render_main(&mut app, 2);
}

#[test]
fn smoke_metadata_tab_renders() {
    use crate::metadata::{MetaCol, MetaObject, MetaState};
    let mut app = JustQueryApp::default();
    app.new_tab();
    if let Some(t) = app.cur_mut() {
        t.meta = Some(MetaObject {
            schema: "public".to_owned(),
            name: "users".to_owned(),
            kind: "Tables".to_owned(),
            state: MetaState::Loaded(vec![MetaCol {
                name: "id".to_owned(),
                ty: "integer".to_owned(),
                nullable: false,
                default: "nextval('users_id_seq')".to_owned(),
            }]),
        });
    }
    render_main(&mut app, 2);
}

#[test]
fn smoke_scan_modal_renders() {
    use crate::metadata::{LogLine, MetaStore};
    let mut app = JustQueryApp::default();
    app.connected = true;
    app.meta_mgr_open = true;
    let store = MetaStore {
        schemas: vec!["public".to_owned(), "app".to_owned(), "audit".to_owned()],
        objects: vec![],
    };
    app.meta_store = std::sync::Arc::new(crate::metadata::SharedStore {
        store: std::sync::RwLock::new(store),
        generation: std::sync::atomic::AtomicU64::new(1),
    });
    app.edit_schemas = Some(vec!["public".to_owned()]); // public monitored; app/audit available
    app.meta_sel_avail = vec!["app".to_owned()]; // a highlighted row → the "›" transfer is enabled
    app.collector_log.push_back(LogLine { time: "12:00:00".to_owned(), text: "scan ok".to_owned() });
    // render across a few of the lifecycle states the header word reflects
    let ctx = test_ctx();
    for st in [
        crate::metadata::CollectorStatus::default(),
        crate::metadata::CollectorStatus { asleep: true, ..Default::default() },
        crate::metadata::CollectorStatus { paused: true, ..Default::default() },
        crate::metadata::CollectorStatus { over_budget: true, last_error: Some("budget".into()), ..Default::default() },
    ] {
        app.collector_status = st;
        let _ = ctx.run_ui(test_input(), |ui| {
            let ctx = &ui.ctx().clone();
            app.meta_manager_modal(ctx);
        });
    }
}

// ---------------------------------------------------------------- live metadata (needs a DB)

// Exercises the catalog helpers against a real PostgreSQL (the local `postrust-pg` dev container:
// postgres/postgres @ localhost:5432). Ignored by default — run with:
//   cargo test --release live_metadata -- --ignored --nocapture
#[test]
#[ignore]
fn live_metadata_smoke() {
    use crate::connections::{
        connect_session, list_objects_in_schema, list_schemas, object_columns, ConnParams,
    };
    let p = ConnParams {
        host: "localhost".into(),
        port: "5432".into(),
        db: "postgres".into(),
        user: "postgres".into(),
        password: "postgres".into(),
    };
    let mut c = connect_session(&p).expect("connect");
    c.batch_execute(
        "DROP TABLE IF EXISTS jq_meta_test; \
         CREATE TABLE jq_meta_test(id int primary key, name text not null, note text);",
    )
    .expect("create table");

    let schemas = list_schemas(&mut c).expect("schemas");
    assert!(schemas.iter().any(|s| s == "public"), "schemas: {schemas:?}");
    // system schemas are now monitored too (lots of objects to browse)
    assert!(schemas.iter().any(|s| s == "pg_catalog"), "pg_catalog missing: {schemas:?}");
    assert!(schemas.iter().any(|s| s == "information_schema"), "info_schema missing");
    let pg_objs = list_objects_in_schema(&mut c, "pg_catalog").expect("pg_catalog objects");
    assert!(pg_objs.len() > 100, "pg_catalog should have many objects, got {}", pg_objs.len());

    let objs = list_objects_in_schema(&mut c, "public").expect("objects");
    assert!(
        objs.iter().any(|(k, n)| k == "Tables" && n == "jq_meta_test"),
        "objects: {objs:?}"
    );

    let cols = object_columns(&mut c, "public", "jq_meta_test")
        .expect("columns query")
        .expect("table exists");
    assert_eq!(cols.len(), 3);
    let id = cols.iter().find(|(n, ..)| n == "id").unwrap();
    assert_eq!(id.1, "integer");
    assert!(!id.2, "id should be NOT NULL"); // nullable == false
    let note = cols.iter().find(|(n, ..)| n == "note").unwrap();
    assert!(note.2, "note should be nullable");

    // deleted-object detection
    let gone = object_columns(&mut c, "public", "jq_no_such_obj_zzz").expect("missing query");
    assert!(gone.is_none(), "missing relation should be None");

    c.batch_execute("DROP TABLE jq_meta_test").expect("drop");
    println!("live_metadata_smoke OK: {} schemas, {} objects in public", schemas.len(), objs.len());
}

// Exercises the incremental-scan helpers against the same local PostgreSQL: a baseline scan, then
// an ALTER, then asserts the per-schema fingerprint moved and the new column was picked up. Ignored
// by default — run with:
//   cargo test --release live_fingerprint -- --ignored --nocapture
#[test]
#[ignore]
fn live_fingerprint_diff() {
    use crate::connections::{
        connect_session, count_meta_rows, scan_schema, schema_fingerprints, ConnParams,
    };
    let p = ConnParams {
        host: "localhost".into(),
        port: "5432".into(),
        db: "postgres".into(),
        user: "postgres".into(),
        password: "postgres".into(),
    };
    let mut c = connect_session(&p).expect("connect");
    // isolate in a dedicated schema so the assertions are deterministic
    c.batch_execute(
        "DROP SCHEMA IF EXISTS jq_fp_test CASCADE; \
         CREATE SCHEMA jq_fp_test; \
         CREATE TABLE jq_fp_test.t(id int primary key, name text not null);",
    )
    .expect("setup");
    let schemas = vec!["jq_fp_test".to_owned()];

    // --- baseline: fingerprint, count, full scan ---
    let d1 = schema_fingerprints(&mut c, &schemas)
        .expect("fp1")
        .get("jq_fp_test")
        .cloned()
        .unwrap_or_default();
    assert!(!d1.is_empty(), "schema should have a fingerprint");
    let cnt1 = count_meta_rows(&mut c, &schemas).expect("cnt1");
    let scan1 = scan_schema(&mut c, "jq_fp_test").expect("scan1");
    let t1 = scan1.iter().find(|(_, n, _)| n == "t").expect("table t");
    assert_eq!(t1.2.len(), 2, "two columns initially");

    // --- ALTER: add a column AND change a column's type (the latter only touches pg_attribute) ---
    c.batch_execute(
        "ALTER TABLE jq_fp_test.t ADD COLUMN note text; \
         ALTER TABLE jq_fp_test.t ALTER COLUMN name TYPE varchar(64);",
    )
    .expect("alter");

    let d2 = schema_fingerprints(&mut c, &schemas)
        .expect("fp2")
        .get("jq_fp_test")
        .cloned()
        .unwrap_or_default();
    assert_ne!(d1, d2, "fingerprint must change after ALTER");
    let cnt2 = count_meta_rows(&mut c, &schemas).expect("cnt2");
    assert!(cnt2 > cnt1, "attr count must grow after ADD COLUMN ({cnt1} -> {cnt2})");
    let scan2 = scan_schema(&mut c, "jq_fp_test").expect("scan2");
    let t2 = scan2.iter().find(|(_, n, _)| n == "t").expect("table t after alter");
    assert_eq!(t2.2.len(), 3, "three columns after ADD COLUMN");
    let name_col = t2.2.iter().find(|(n, ..)| n == "name").expect("name col");
    assert!(
        name_col.1.starts_with("character varying"),
        "ALTER COLUMN TYPE should be reflected, got {}",
        name_col.1
    );

    // --- no-op: a re-read with nothing changed keeps the same fingerprint (no false positives) ---
    let d3 = schema_fingerprints(&mut c, &schemas)
        .expect("fp3")
        .get("jq_fp_test")
        .cloned()
        .unwrap_or_default();
    assert_eq!(d2, d3, "fingerprint must be stable when nothing changed");

    c.batch_execute("DROP SCHEMA jq_fp_test CASCADE").expect("cleanup");
    println!("live_fingerprint_diff OK: count {cnt1} -> {cnt2}, fp {d1} -> {d2}");
}

// Drives the live collector worker against the local PostgreSQL with a 1s interval and asserts the
// cadence: it scans on its own timer (several heartbeats over ~3.5s) without runaway nonstop scans.
// Ignored by default — run with:
//   cargo test --release live_collector -- --ignored --nocapture
#[test]
#[ignore]
fn live_collector_cadence() {
    use crate::connections::ConnParams;
    use crate::meta_collector::{start, CollectorMsg};
    use crate::metadata::{CollectorSettings, SharedStore};
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::{Duration, Instant};

    let p = ConnParams {
        host: "localhost".into(),
        port: "5432".into(),
        db: "postgres".into(),
        user: "postgres".into(),
        password: "postgres".into(),
    };
    // 5s is the smallest interval the worker honours (`interval.max(5)`, matching the UI's 5..=3600)
    let settings = CollectorSettings {
        enabled: true,
        interval: 5,
        budget: 1_000_000,
        idle: 300,                                // stay in the active window for the whole test
        schemas: Some(vec!["public".to_owned()]), // small scan
    };
    let shared = std::sync::Arc::new(SharedStore::default());
    let handle = start(p, settings, shared);

    let mut scans = 0usize;
    let deadline = Instant::now() + Duration::from_millis(12_500);
    while Instant::now() < deadline {
        match handle.rx.recv_timeout(Duration::from_millis(250)) {
            Ok(CollectorMsg::Log(l)) if l.text.starts_with("scan:") => scans += 1,
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(handle); // Drop sends Shutdown

    // ~12.5s at a 5s interval → scans at ≈0/5/10 (3). Lower bound proves it keeps scanning on its
    // timer; upper bound proves it respects the перекур (no nonstop loop).
    assert!(scans >= 2, "expected the scanner to keep ticking, got {scans} scans");
    assert!(scans <= 6, "scanner ran far too often (nonstop?), got {scans} scans");
    println!("live_collector_cadence OK: {scans} scans in ~12.5s at 5s interval");
}

// ---------------------------------------------------------------- crypt helpers

#[test]
fn hex_roundtrip() {
    let bytes = [0u8, 1, 15, 16, 127, 128, 255];
    let hex = crate::crypt::to_hex(&bytes);
    assert_eq!(hex, "00010f107f80ff");
    assert_eq!(crate::crypt::from_hex(&hex).unwrap(), bytes);
}

#[test]
fn dpapi_roundtrip() {
    let secret = "пароль p@ss 123".as_bytes();
    let enc = crate::crypt::protect(secret).expect("DPAPI protect");
    assert_ne!(enc, secret); // actually encrypted
    let dec = crate::crypt::unprotect(&enc).expect("DPAPI unprotect");
    assert_eq!(dec, secret);
}

#[test]
fn editor_many_enters_render() {
    // Smart Enter path: pressing Enter past the viewport must keep rendering (the gutter/active-
    // line/caret use the galley's real row height now, so no drift / scroll overshoot) and grow
    // the buffer by one line per press.
    let mut app = JustQueryApp::default();
    app.new_tab();
    app.focus_editor = true;
    let ctx = test_ctx();
    for _ in 0..2 {
        let _ = ctx.run_ui(test_input(), |ui| app.main_screen(ui));
    }
    for _ in 0..150 {
        let mut input = test_input();
        input.events = vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];
        let _ = ctx.run_ui(input, |ui| app.main_screen(ui));
    }
    assert_eq!(app.tabs[0].sql.matches('\n').count(), 150);
}

#[test]
fn editor_renders_at_fractional_dpi() {
    // gutter/active-line/caret position from the galley's real per-row rects, so they stay aligned
    // with the text even when the row pitch isn't an integer number of pixels (Windows scaling).
    let mut app = JustQueryApp::default();
    app.new_tab();
    app.tabs[0].sql = "select a\nfrom t\n".repeat(120);
    app.focus_editor = true;
    let ctx = test_ctx();
    ctx.set_pixels_per_point(1.5);
    render_main(&mut app, 3);
}
