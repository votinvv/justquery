//! Regression tests — run with `cargo test`.
//!
//! Two layers, no extra crates / no GPU:
//!   * logic tests for the search, tab and helper behaviour;
//!   * headless "smoke" tests that render each screen through `egui::Context::run` so a panic
//!     or layout error (clipping, `min_rect`, sticky-header maths, …) fails the build.

use super::*;
use std::path::Path;

/// Replace the entire active tab's buffer with the text `sql` (document model).
fn set_sql(a: &mut JustQueryApp, sql: &str) {
    let i = a.active_tab;
    if let Some(d) = a.tabs[i].doc_mut() {
        let last = d.line_count() - 1;
        let e = d.line_length(last);
        d.replace_range((0, 0), (last, e), sql);
    }
}

fn app_with_sql(sql: &str) -> JustQueryApp {
    let mut a = JustQueryApp::default();
    a.new_tab();
    set_sql(&mut a, sql);
    a
}

/// Like `app_with_sql`, but the tab is XML — imitates opening a `.xml` file (the kind is decided
/// by extension, never sniffed from the buffer). Assigns the model by matching the registry; a tiny
/// inline test model (root `Document`) is registered so matching/validation can run without fixtures.
fn app_with_xml(text: &str) -> JustQueryApp {
    let mut a = JustQueryApp::default();
    register_test_model(&mut a);
    a.new_tab();
    set_sql(&mut a, text);
    let i = a.active_tab;
    a.tabs[i].kind = TabKind::Xml;
    let head = a.tabs[i]
        .doc_mut()
        .map(|d| String::from_utf8_lossy(&d.read_bytes(0, 8192)).into_owned())
        .unwrap_or_default();
    a.tabs[i].model_id = a.models.match_doc(&head).map(|m| m.manifest.id.clone());
    a
}

/// Register one tiny inline model (root `Document`, matches any `Document`) so the XML helpers can
/// assign a model and exercise validation without external schema files.
fn register_test_model(a: &mut JustQueryApp) {
    const TINY_XSD: &str = r#"<?xml version="1.0"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="Document" type="Doc"/>
  <xs:complexType name="Doc">
    <xs:sequence>
      <xs:element name="Source" type="xs:string"/>
    </xs:sequence>
  </xs:complexType>
</xs:schema>"#;
    // Hermetic: ignore any models the user keeps in AppData — keep only this one.
    a.models = xmlmodel::Registry::load_dir(std::path::Path::new("__no_models_for_test__"));
    a.models.push(xmlmodel::Model {
        manifest: xmlmodel::Manifest {
            id: "test_doc".to_owned(),
            description: String::new(),
            priority: 10,
            r#match: xmlmodel::MatchPred { rules: vec![] },
        },
        xsd: TINY_XSD.to_owned(),
        codes: "{}".to_owned(),
        rules: r#"{"rules": []}"#.to_owned(),
        checksum: String::new(),
        intact: true,
        path: None,
    });
    a.models.sort();
}

// ---------------------------------------------------------------- find

// ---------------------------------------------------------------- search (background → grid)

/// Run the background search to completion (bounded poll loop).
fn run_search(a: &mut JustQueryApp, query: &str) {
    a.start_search(query.to_owned());
    let ctx = test_ctx();
    for _ in 0..500 {
        a.poll_procs(&ctx);
        if a.tabs[a.active_tab].proc.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(a.tabs[a.active_tab].proc.is_none(), "search did not finish");
}

fn match_count(a: &JustQueryApp) -> usize {
    a.tabs[a.active_tab]
        .panel
        .iter()
        .filter_map(|s| match s {
            crate::ResultTab::Probe { res, .. } => Some(res.len()),
            _ => None,
        })
        .sum()
}

#[test]
fn search_case_insensitive_ascii() {
    let mut a = app_with_sql("foo Foo FOO bar");
    run_search(&mut a, "foo");
    assert_eq!(match_count(&a), 3);
}

#[test]
fn search_cyrillic_case_insensitive() {
    // regression: an ASCII-only fold never matched Cyrillic — the background search folds the Unicode way
    let mut a = app_with_sql("Под подвал ПОДарок");
    run_search(&mut a, "под");
    assert_eq!(match_count(&a), 3);
}

#[test]
fn search_multiline_highlights_editor() {
    let mut a = app_with_sql("ab\n..ab\nx"); // matches at (0,0) and (1,2)
    run_search(&mut a, "ab");
    assert_eq!(match_count(&a), 2);
    // matches are highlighted in the editor per line (search_hl)
    assert_eq!(a.tabs[0].search_hl.get(&1), Some(&vec![(2usize, 2usize)]));
}

#[test]
fn search_works_on_xml_tab() {
    let mut a = app_with_xml("<?xml version=\"1.0\"?>\n<a>foo</a>\n<b>foo</b>");
    assert!(a.tabs[a.active_tab].is_xml());
    run_search(&mut a, "foo");
    assert_eq!(match_count(&a), 2);
}

#[test]
fn search_blocks_other_processes_while_running() {
    // while a process runs the tab is busy — a repeat start is ignored (one process per tab)
    let mut a = app_with_sql("aaaa");
    a.start_search("a".into());
    assert!(a.tab_busy(), "the tab should be busy with the search");
    a.start_search("a".into()); // does not spawn a second process
    let ctx = test_ctx();
    for _ in 0..200 {
        a.poll_procs(&ctx);
        if a.tabs[a.active_tab].proc.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(a.tabs[a.active_tab].proc.is_none());
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
    let (job, st) = crate::highlight::highlight_sql(
        "select 1 from t -- c",
        crate::highlight::LineState::Text,
        12.0,
    );
    assert!(!job.sections.is_empty());
    assert_eq!(job.text, "select 1 from t -- c");
    assert_eq!(st, crate::highlight::LineState::Text);
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
    set_sql(&mut app, &"a\t".repeat(400)); // 800 chars, but far wider once tabs expand
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
        set_sql(&mut app, &"0".repeat(10000));
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
            app.main_screen(ui);
            app.connect_modal(ui.ctx());
            app.no_conn_modal(ui.ctx());
        });
    }
}

#[test]
fn smoke_editor_renders() {
    let mut app = JustQueryApp::default();
    app.new_tab();
    set_sql(&mut app, "select id, name\nfrom users\nwhere active = true;");
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
    set_sql(&mut app, sql);
    let mut d = app.tabs[0].take_doc().unwrap();
    app.ac_build(&mut d, (0, sql.chars().count()), tab_id);
    app.tabs[0].put_doc(d);
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
    set_sql(&mut app, &"x\n".repeat(240));
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
    assert_eq!(app.tabs[0].full_sql().unwrap().matches('\n').count(), 240);
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
    set_sql(&mut app, sql);
    let mut d = app.tabs[0].take_doc().unwrap();
    app.ac_build(&mut d, (0, sql.chars().count()), tab_id);
    app.tabs[0].put_doc(d);
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
        // a data sheet in the unified result panel → virtualized grid
        t.panel = vec![crate::ResultTab::Data(crate::sample::demo_result(120))];
        t.panel_active = 0;
    }
    render_main(&mut app, 3);
}

#[test]
fn smoke_result_grid_maximized() {
    let mut app = JustQueryApp::default();
    app.new_tab();
    if let Some(t) = app.cur_mut() {
        t.panel = vec![crate::ResultTab::Data(crate::sample::demo_result(120))];
        t.panel_active = 0;
        t.result_full = true;
    }
    render_main(&mut app, 2);
}

#[test]
fn smoke_unified_panel_mixed_sheets() {
    // unified panel: a data grid + a single-line status-OK + status-error + findings — all in one
    // tab strip; switching the active sheet goes through a real frame without panicking
    let mut app = JustQueryApp::default();
    app.new_tab();
    if let Some(t) = app.cur_mut() {
        t.panel = vec![
            crate::ResultTab::Data(crate::sample::demo_result(30)),
            crate::ResultTab::Data(crate::ResultSet::status(true, "CREATE", 1, "CREATE TABLE")),
            crate::ResultTab::Data(crate::ResultSet::status(false, "INSERT", 4, "relation \"x\" does not exist")),
            crate::ResultTab::Probe { title: "Validation".to_owned(), res: proc::Results::new_validation() },
        ];
        t.panel_active = 0;
    }
    for active in 0..4 {
        if let Some(t) = app.cur_mut() {
            t.panel_active = active;
        }
        render_main(&mut app, 1);
    }
}

#[test]
fn smoke_find_and_about() {
    let mut app = JustQueryApp::default();
    app.new_tab();
    app.open_find();
    app.open_about();
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
    let mut app = JustQueryApp { left_panel: Some(LeftPanel::Database), ..Default::default() };
    app.open_conn_tab(Connection {
        port: "5432".into(),
        ..Default::default()
    });
    assert!(app.is_connection_tab());
    render_main(&mut app, 3);
}

#[test]
fn connection_tab_save_commits() {
    let mut app = JustQueryApp::default();
    app.open_conn_tab(Connection {
        name: "newconn".into(),
        ..Default::default()
    });
    assert!(app.cur().is_some_and(|t| t.dirty())); // new connection starts dirty
    app.commit_conn_tab(); // in-memory only (no disk write in tests)
    assert_eq!(app.connections.len(), 1);
    assert_eq!(app.connections[0].name, "newconn");
    assert!(app.connections[0].id != 0);
    assert!(app.cur().is_some_and(|t| !t.dirty())); // saved → clean
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

#[test]
fn split_with_lines_maps_statements_to_source_lines() {
    // lines are 1-based; blank/comment lines at the start are accounted for by the newlines
    let sql = "select 1;\n\nselect 2;\nselect\n  3";
    let s = crate::connections::split_statements_lines(sql);
    assert_eq!(s.len(), 3);
    assert_eq!(s[0], ("select 1".to_owned(), 1));
    assert_eq!(s[1], ("select 2".to_owned(), 3));
    assert_eq!(s[2].1, 4); // the third statement starts on line 4
}

// ---------------------------------------------------------------- metadata

#[test]
fn smoke_metadata_panel_renders() {
    use crate::metadata::{MetaObjRow, MetaStore};
    let mut app = JustQueryApp {
        left_panel: Some(LeftPanel::Metadata),
        connected: true,
        ..Default::default()
    };
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
        t.kind = TabKind::Meta(MetaObject {
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
fn smoke_session_tab_renders() {
    use crate::metadata::{LogLine, MetaStore};
    let mut app = JustQueryApp { connected: true, ..Default::default() };
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
    app.open_session();
    // render across a few of the lifecycle states the header colour reflects
    for st in [
        crate::metadata::CollectorStatus::default(),
        crate::metadata::CollectorStatus { asleep: true, ..Default::default() },
        crate::metadata::CollectorStatus { paused: true, ..Default::default() },
        crate::metadata::CollectorStatus { over_budget: true, last_error: Some("budget".into()), ..Default::default() },
    ] {
        app.collector_status = st;
        render_main(&mut app, 1);
    }
}

// The Session tab's broken branch: a dropped control connection shows the failure reason +
// Reconnect button in the Connection block, and disables the Scan block. `conn_broken` is never
// set by the app today (no live drop detector), so this exercises the UI path directly.
#[test]
fn smoke_session_tab_broken() {
    use crate::connections::ConnParams;
    let mut app = JustQueryApp {
        connected: false,
        conn_broken: true,
        active_label: "admin@shop".to_owned(),
        conn_params: Some(ConnParams {
            host: "localhost".to_owned(),
            port: "5432".to_owned(),
            db: "shop".to_owned(),
            user: "admin".to_owned(),
            password: String::new(),
        }),
        main_pid: Some(18_432),
        main_ssl: Some(true),
        main_conn_since: Some("14:02:11".to_owned()),
        last_error: Some("server closed the connection".to_owned()),
        ..Default::default()
    };
    app.open_session();
    render_main(&mut app, 1);
    // pid/since are retained from the last live session, ssl cleared to "—"
    assert_eq!(app.main_pid, Some(18_432));
}

// The connection chip in the status bar opens the Session tab when clicked, and renders nothing
// when never connected / deliberately disconnected.
#[test]
fn smoke_conn_chip_opens_session() {
    let mut app = JustQueryApp {
        connected: true,
        active_label: "admin@shop".to_owned(),
        ..Default::default()
    };
    render_main(&mut app, 1);
    assert!(!app.tabs.iter().any(|t| matches!(t.kind, crate::TabKind::Session)),
        "Session tab is not opened until the chip is clicked");
    app.open_session();
    assert!(app.tabs.iter().any(|t| matches!(t.kind, crate::TabKind::Session)));
    // opening again re-selects the singleton, not a duplicate
    let n = app.tabs.len();
    app.open_session();
    assert_eq!(app.tabs.len(), n);
}

// ---------------------------------------------------------------- live metadata (needs a DB)

// Exercises the catalog helpers against a real PostgreSQL (the local `postrust-pg` dev container:
// postgres/postgres @ localhost:5432). Ignored by default — run with:
//   cargo test --release live_metadata -- --ignored --nocapture
#[test]
#[ignore]
fn live_metadata_smoke() {
    use crate::catalog::{list_objects_in_schema, list_schemas, object_columns};
    use crate::connections::{connect_session, ConnParams};
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
    use crate::catalog::{count_meta_rows, scan_schema, schema_fingerprints};
    use crate::connections::{connect_session, ConnParams};
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
    // timer; upper bound proves it respects the cooldown (no nonstop loop).
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
    assert_eq!(app.tabs[0].full_sql().unwrap().matches('\n').count(), 150);
}

#[test]
fn editor_renders_at_fractional_dpi() {
    // gutter/active-line/caret position from the galley's real per-row rects, so they stay aligned
    // with the text even when the row pitch isn't an integer number of pixels (Windows scaling).
    let mut app = JustQueryApp::default();
    app.new_tab();
    set_sql(&mut app, &"select a\nfrom t\n".repeat(120));
    app.focus_editor = true;
    let ctx = test_ctx();
    ctx.set_pixels_per_point(1.5);
    render_main(&mut app, 3);
}

// ---------------------------------------------------------------- tab kind (SQL vs XML by extension)

#[test]
fn new_tab_is_sql() {
    let mut a = JustQueryApp::default();
    a.new_tab();
    assert!(a.is_sql_tab());
}

#[test]
fn new_tab_stays_sql_even_with_xml_content() {
    // a fresh buffer is SQL regardless of content — XML is decided by file extension, never sniffed
    let a = app_with_sql("<?xml version=\"1.0\"?>\n<a/>");
    assert!(a.is_sql_tab(), "a buffer with <?xml in a new tab stays SQL until saved as .xml");
    assert!(!a.is_xml_tab());
}

#[test]
fn xml_extension_decides_kind() {
    assert!(JustQueryApp::is_xml_path(Path::new("report.xml")));
    assert!(JustQueryApp::is_xml_path(Path::new("REPORT.XML"))); // case-insensitive
    assert!(!JustQueryApp::is_xml_path(Path::new("query.sql")));
    assert!(!JustQueryApp::is_xml_path(Path::new("noext")));
}

#[test]
fn xml_tab_helper_reports_xml() {
    let a = app_with_xml("<?xml version=\"1.0\"?>\n<Document/>");
    assert!(a.is_xml_tab());
    assert!(!a.is_sql_tab());
}

#[test]
fn xml_tab_renders_without_panic() {
    // headless: the XML tab renders the XML toolbar + XML highlighting in a frame without panicking
    let mut a = app_with_xml("<?xml version=\"1.0\"?>\n<Document><a x=\"1\">t</a></Document>");
    a.focus_editor = true;
    render_main(&mut a, 3);
    assert!(a.tabs[a.active_tab].is_xml());
}

#[test]
fn xml_format_reformats_buffer_end_to_end() {
    // start_xml_format → background thread → poll_procs → swap_origin (as in a real frame)
    let mut a = app_with_xml("<?xml version=\"1.0\"?><a><b>x</b></a>");
    assert!(a.tabs[a.active_tab].is_xml());
    a.start_xml_format();
    assert!(a.tabs[a.active_tab].proc.is_some(), "the process should start");
    let ctx = test_ctx();
    for _ in 0..300 {
        a.poll_procs(&ctx);
        if a.tabs[a.active_tab].proc.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(a.tabs[a.active_tab].proc.is_none(), "formatting did not finish");
    let out = a.tabs[a.active_tab].doc_mut().unwrap().full_text();
    assert!(out.starts_with("<?xml"), "declaration on the first line: {out:?}");
    assert!(out.contains("  <b>x</b>"), "child element with indentation: {out:?}");
}

#[test]
fn xml_validate_reports_findings_end_to_end() {
    // schemaVersion="5.1" → version auto-detect; invalid XML → ≥1 finding in the panel
    let mut a = app_with_xml(
        "<?xml version=\"1.0\"?>\n<Document schemaVersion=\"5.1\">\n  <oops>\n",
    );
    assert!(a.tabs[a.active_tab].is_xml());
    assert_eq!(a.tabs[a.active_tab].model_id.as_deref(), Some("test_doc"), "test model auto-detected");
    a.start_xml_validate();
    assert!(a.tabs[a.active_tab].proc.is_some(), "validation should start");
    let ctx = test_ctx();
    for _ in 0..500 {
        a.poll_procs(&ctx);
        if a.tabs[a.active_tab].proc.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(a.tabs[a.active_tab].proc.is_none(), "validation did not finish");
    assert!(match_count(&a) >= 1, "invalid XML should yield ≥1 finding");
}

#[test]
fn xml_findings_panel_renders() {
    // the findings panel goes through a real frame (a grid with Type/Line/Code/Message columns)
    let mut a = app_with_xml("<?xml version=\"1.0\"?>\n<Document schemaVersion=\"5.1\"/>");
    a.tabs[a.active_tab]
        .panel
        .push(crate::ResultTab::Probe { title: "Validation".to_owned(), res: proc::Results::new_validation() });
    render_main(&mut a, 2);
}

// End-to-end test of the lazy COPY stream worker against the local `postrust-pg` dev container
// (postgres/postgres @ localhost:5432). Validates: first screenful + More + All page through the
// whole result via COPY, values come back as text, the total is exact, and the session connection
// is reusable after abandoning the stream mid-way (drain-to-end resync). Run with:
//   cargo test --release live_lazy_copy -- --ignored --nocapture
#[test]
#[ignore]
fn live_lazy_copy_stream() {
    use crate::connections::{run_statements_worker, ConnParams, ExecMsg, FetchCmd};
    use std::sync::atomic::AtomicBool;
    use std::sync::{mpsc, Arc};

    let p = ConnParams {
        host: "localhost".into(),
        port: "5432".into(),
        db: "postgres".into(),
        user: "postgres".into(),
        password: "postgres".into(),
    };

    // ---- page through 1000 rows via the lazy stream: first page → More → All → end ----
    let (tx, rx) = mpsc::channel();
    let (cmd_tx, cmd_rx) = mpsc::channel::<FetchCmd>();
    let stop = Arc::new(AtomicBool::new(false));
    let stmts = vec![(
        "SELECT g AS n, 'v' || g AS label FROM generate_series(1, 1000) g".to_owned(),
        1usize,
    )];
    let h = std::thread::spawn(move || {
        run_statements_worker(None, p.clone(), stmts, tx, cmd_rx, stop, 50)
    });

    let mut cols: Vec<String> = Vec::new();
    let mut rows = 0usize;
    let mut more_seen = 0usize;
    let mut first_cell = String::new();
    let reclaimed = loop {
        match rx.recv().expect("worker channel") {
            ExecMsg::Ready(_) => {}
            ExecMsg::LazyBegin { cols: c, .. } => cols = c,
            ExecMsg::LazyRows(batch) => {
                if first_cell.is_empty() {
                    if let Some(r) = batch.first() {
                        first_cell = r[0].clone();
                    }
                }
                rows += batch.len();
            }
            ExecMsg::LazyMore => {
                more_seen += 1;
                // first pause → fetch one more page; second → fetch the rest
                let _ = cmd_tx.send(if more_seen == 1 { FetchCmd::More(100) } else { FetchCmd::All });
            }
            ExecMsg::LazyEnd { error } => assert!(error.is_none(), "clean stream, got error: {error:?}"),
            ExecMsg::Done(c) => break c,
            ExecMsg::Result(_) | ExecMsg::Status { .. } => {
                panic!("unexpected buffered output for a single lazy SELECT")
            }
        }
    };
    h.join().unwrap();
    assert_eq!(cols, vec!["n".to_owned(), "label".to_owned()], "columns from prepare");
    assert_eq!(rows, 1000, "all rows paged in via COPY");
    assert_eq!(first_cell, "1", "first cell is the text rendering of n=1");
    let client = *reclaimed.expect("connection handed back");

    // ---- abandon a stream mid-way (Close), then reuse the SAME connection for a buffered query ----
    let (tx2, rx2) = mpsc::channel();
    let (cmd_tx2, cmd_rx2) = mpsc::channel::<FetchCmd>();
    let stop2 = Arc::new(AtomicBool::new(false));
    let stmts2 = vec![(
        "SELECT g FROM generate_series(1, 100000) g".to_owned(),
        1usize,
    )];
    let p2 = ConnParams {
        host: "localhost".into(),
        port: "5432".into(),
        db: "postgres".into(),
        user: "postgres".into(),
        password: "postgres".into(),
    };
    let h2 = std::thread::spawn(move || {
        run_statements_worker(Some(client), p2, stmts2, tx2, cmd_rx2, stop2, 20)
    });
    let client2 = loop {
        match rx2.recv().expect("worker2 channel") {
            ExecMsg::LazyMore => {
                let _ = cmd_tx2.send(FetchCmd::Close); // abandon after the first screenful
            }
            ExecMsg::Done(c) => break c,
            _ => {}
        }
    };
    h2.join().unwrap();
    let mut client2 = *client2.expect("connection survived an abandoned stream");

    // the reclaimed connection must be clean (resynced past the aborted COPY)
    let n: i32 = client2.query_one("SELECT 42", &[]).expect("reuse after abandon").get(0);
    assert_eq!(n, 42, "session usable after an abandoned lazy stream");
}

// A non-last SELECT in a multi-statement batch must show its first page, cap + resync (not buffer the
// whole thing) and flag the grid truncated; the LAST SELECT still streams lazily. Run with:
//   cargo test --release live_lazy_copy_intermediate -- --ignored --nocapture
#[test]
#[ignore]
fn live_lazy_copy_intermediate() {
    use crate::connections::{run_statements_worker, ConnParams, ExecMsg, FetchCmd};
    use std::sync::atomic::AtomicBool;
    use std::sync::{mpsc, Arc};

    let p = ConnParams {
        host: "localhost".into(),
        port: "5432".into(),
        db: "postgres".into(),
        user: "postgres".into(),
        password: "postgres".into(),
    };
    let (tx, rx) = mpsc::channel();
    let (cmd_tx, cmd_rx) = mpsc::channel::<FetchCmd>();
    let stop = Arc::new(AtomicBool::new(false));
    let stmts = vec![
        ("SELECT g FROM generate_series(1, 100000) g".to_owned(), 1usize), // intermediate → capped to a page
        ("SELECT 99 AS v".to_owned(), 2usize),                             // last → lazy
    ];
    let h = std::thread::spawn(move || run_statements_worker(None, p, stmts, tx, cmd_rx, stop, 20));

    let mut head_rows = None;
    let mut head_truncated = false;
    let mut last_rows = 0usize;
    loop {
        match rx.recv().expect("worker channel") {
            ExecMsg::Result(rs) => {
                head_rows = Some(rs.rows.len());
                head_truncated = rs.truncated;
            }
            ExecMsg::LazyRows(b) => last_rows += b.len(),
            ExecMsg::LazyMore => {
                let _ = cmd_tx.send(FetchCmd::All);
            }
            ExecMsg::Done(_) => break,
            _ => {}
        }
    }
    h.join().unwrap();
    assert_eq!(head_rows, Some(20), "intermediate SELECT capped to the first page");
    assert!(head_truncated, "intermediate grid flagged truncated");
    assert_eq!(last_rows, 1, "the last SELECT streamed lazily (1 row)");
}

// Build a 10M-row, 10-column test table (mixed types incl. arrays / jsonb / a tab in text, to
// exercise COPY escaping) on the local dev DB for manually testing the lazy доскролл. Run with:
//   cargo test --release live_make_bigtest -- --ignored --nocapture
#[test]
#[ignore]
fn live_make_bigtest() {
    use crate::connections::{connect_session, ConnParams};
    let p = ConnParams {
        host: "localhost".into(),
        port: "5432".into(),
        db: "postgres".into(),
        user: "postgres".into(),
        password: "postgres".into(),
    };
    let mut c = connect_session(&p).expect("connect");
    let _ = c.batch_execute("SET statement_timeout = 0; SET maintenance_work_mem = '512MB';");
    c.batch_execute(
        "DROP TABLE IF EXISTS jq_bigtest;
         CREATE TABLE jq_bigtest (
             id         bigint PRIMARY KEY,
             name       text,
             amount     numeric(12,2),
             created_at timestamptz,
             active     boolean,
             score      double precision,
             tags       text[],
             data       jsonb,
             uid        uuid,
             note       text
         );
         INSERT INTO jq_bigtest
         SELECT
             g,
             'name_' || g,
             (g % 100000)::numeric / 100,
             timestamptz '2020-01-01 00:00:00+00' + (g % 1000000) * interval '1 minute',
             (g % 2 = 0),
             (g % 1000)::float8 + 0.5,
             ARRAY['tag' || (g % 10), 'grp' || (g % 3)],
             jsonb_build_object('k', g, 'even', g % 2 = 0),
             md5(g::text)::uuid,
             'row ' || g || chr(9) || 'has a tab'
         FROM generate_series(1, 10000000) AS g;",
    )
    .expect("create + fill jq_bigtest");
    let n: i64 = c.query_one("SELECT count(*) FROM jq_bigtest", &[]).unwrap().get(0);
    let size: String = c
        .query_one("SELECT pg_size_pretty(pg_total_relation_size('jq_bigtest'))", &[])
        .unwrap()
        .get(0);
    println!("jq_bigtest: {n} rows, {size} on disk");
    assert_eq!(n, 10_000_000);
}

// ── result-grid client-side sort ──────────────────────────────────────────────────────

use std::cmp::Ordering;

#[test]
fn cmp_cell_numeric_string_and_null() {
    // numbers compare numerically, not lexicographically
    assert_eq!(cmp_cell("2", "10"), Ordering::Less);
    assert_eq!(cmp_cell("10", "2"), Ordering::Greater);
    assert_eq!(cmp_cell("7", "7"), Ordering::Equal);
    // mixed int/float falls back to f64
    assert_eq!(cmp_cell("2.5", "10"), Ordering::Less);
    assert_eq!(cmp_cell("-3", "0.5"), Ordering::Less);
    // exact past f64's mantissa (i128 path)
    assert_eq!(cmp_cell("9223372036854775807", "9223372036854775806"), Ordering::Greater);
    // non-numeric → byte-wise
    assert_eq!(cmp_cell("apple", "banana"), Ordering::Less);
    // NULL ("—") sorts greatest (lands last on ascending), regardless of the other operand
    assert_eq!(cmp_cell("—", "5"), Ordering::Greater);
    assert_eq!(cmp_cell("5", "—"), Ordering::Less);
    assert_eq!(cmp_cell("—", "zzz"), Ordering::Greater);
    assert_eq!(cmp_cell("—", "—"), Ordering::Equal);
}

#[test]
fn cmp_rows_multi_key() {
    let a = vec!["1".to_owned(), "b".to_owned()];
    let b = vec!["1".to_owned(), "a".to_owned()];
    // first key ties (both "1") → second key (asc) breaks it: "a" < "b"
    assert_eq!(cmp_rows(&a, &b, &[(0, false), (1, false)]), Ordering::Greater);
    // second key descending flips the tiebreak
    assert_eq!(cmp_rows(&a, &b, &[(0, false), (1, true)]), Ordering::Less);
    // first key descending dominates
    let c = vec!["2".to_owned(), "a".to_owned()];
    assert_eq!(cmp_rows(&a, &c, &[(0, true)]), Ordering::Greater);
}

#[test]
fn toggle_sort_single_column_cycle() {
    let mut rs = ResultSet::new(vec!["c0".to_owned()], Vec::new());
    rs.toggle_sort(0, false); // asc
    assert_eq!(rs.sort, vec![(0, false)]);
    rs.toggle_sort(0, false); // desc
    assert_eq!(rs.sort, vec![(0, true)]);
    rs.toggle_sort(0, false); // reset
    assert!(rs.sort.is_empty());
}

#[test]
fn toggle_sort_plain_click_replaces() {
    let mut rs = ResultSet::new(vec!["c0".to_owned(), "c1".to_owned()], Vec::new());
    rs.toggle_sort(0, false);
    rs.toggle_sort(1, true); // additive → two keys
    assert_eq!(rs.sort, vec![(0, false), (1, false)]);
    // a plain (non-additive) click on a different column clears everything and starts fresh (asc)
    rs.toggle_sort(0, false);
    assert_eq!(rs.sort, vec![(0, false)]);
}

#[test]
fn toggle_sort_additive_multi() {
    let mut rs = ResultSet::new(vec!["c0".to_owned(), "c1".to_owned(), "c2".to_owned()], Vec::new());
    rs.toggle_sort(0, false); // [c0 asc]
    rs.toggle_sort(2, true); // [c0 asc, c2 asc] — new column joins at lowest priority
    assert_eq!(rs.sort, vec![(0, false), (2, false)]);
    rs.toggle_sort(2, true); // c2 asc → desc, c0 kept
    assert_eq!(rs.sort, vec![(0, false), (2, true)]);
    rs.toggle_sort(2, true); // c2 desc → removed, c0 kept
    assert_eq!(rs.sort, vec![(0, false)]);
    rs.toggle_sort(0, true); // additive on the lone key: asc → desc
    assert_eq!(rs.sort, vec![(0, true)]);
}

#[test]
fn rebuild_view_sorts_and_re_sorts_on_growth() {
    let mk = |a: &str, b: &str| vec![a.to_owned(), b.to_owned()];
    let mut rs = ResultSet::new(
        vec!["n".to_owned(), "s".to_owned()],
        vec![mk("3", "b"), mk("10", "a"), mk("2", "c"), mk("—", "d")],
    );
    // no sort → identity (empty view, data_row is identity)
    assert!(rs.view.is_empty());
    assert_eq!(rs.data_row(1), 1);

    rs.toggle_sort(0, false); // sort by the numeric column ascending
    rs.rebuild_view();
    // numeric order 2,3,10 then NULL last → row indices [2,0,1,3]
    assert_eq!(rs.view, vec![2, 0, 1, 3]);
    let visible: Vec<&str> = (0..rs.rows.len()).map(|r| rs.rows[rs.data_row(r)][0].as_str()).collect();
    assert_eq!(visible, vec!["2", "3", "10", "—"]);

    // a lazy batch arrives → mark dirty, rebuild, and the new row slots into place
    rs.rows.push(mk("5", "e"));
    rs.view_dirty = true;
    rs.rebuild_view();
    let visible: Vec<&str> = (0..rs.rows.len()).map(|r| rs.rows[rs.data_row(r)][0].as_str()).collect();
    assert_eq!(visible, vec!["2", "3", "5", "10", "—"]);

    // clearing the sort restores identity
    rs.toggle_sort(0, false); // asc → desc
    rs.toggle_sort(0, false); // desc → cleared
    rs.rebuild_view();
    assert!(rs.view.is_empty());
}

#[test]
fn doscroll_drops_sort_markers_keeps_order_appends_at_end() {
    let mk = |a: &str| vec![a.to_owned()];
    let mut rs = ResultSet::new(vec!["n".to_owned()], vec![mk("3"), mk("1"), mk("2")]);
    rs.toggle_sort(0, false);
    rs.rebuild_view();
    assert_eq!(rs.view, vec![1, 2, 0]); // sorted 1,2,3

    // a lazy batch arrives — emulate the LazyRows handler: append + drop the sort markers, keep view
    rs.rows.push(mk("0"));
    rs.rows.push(mk("5"));
    rs.sort.clear(); // markers gone; we do NOT rebuild_view

    assert!(rs.sort.is_empty(), "sort markers cleared on doscroll");
    // already-shown rows keep their sorted order; the new rows land at the end in natural order
    let visible: Vec<&str> = (0..rs.rows.len()).map(|r| rs.rows[rs.data_row(r)][0].as_str()).collect();
    assert_eq!(visible, vec!["1", "2", "3", "0", "5"]);
}

#[test]
fn row_selection_click_ctrl_alt() {
    let mut a = JustQueryApp::default();
    let as_vec = |a: &JustQueryApp| a.grid_rows.iter().copied().collect::<Vec<_>>();

    a.update_row_sel(5, false, false); // plain click → single row
    assert_eq!(as_vec(&a), vec![5]);
    a.update_row_sel(8, true, false); // Ctrl → add
    assert_eq!(as_vec(&a), vec![5, 8]);
    a.update_row_sel(5, true, false); // Ctrl on a selected row → remove (anchor → 5)
    assert_eq!(as_vec(&a), vec![8]);
    a.update_row_sel(2, false, true); // Alt → range from anchor 5 down to 2 (replaces)
    assert_eq!(as_vec(&a), vec![2, 3, 4, 5]);
    a.update_row_sel(10, false, false); // plain click resets to single
    assert_eq!(as_vec(&a), vec![10]);
    a.update_row_sel(12, true, false); // Ctrl add (anchor → 12)
    a.update_row_sel(14, true, true); // Ctrl+Alt → additive range from anchor 12
    assert_eq!(as_vec(&a), vec![10, 12, 13, 14]);
}
