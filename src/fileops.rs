//! Open / Save / Save As for editor tabs, backed by the native Win32 dialogs in [`crate::dialog`].
//! All methods hang off [`crate::JustQueryApp`].
//!
//! Файл НЕ читается целиком: документ маппится через mmap (см. [`crate::doc`]); файлы
//! крупнее порога открываются в фоне с прогрессом на листе редактора.

use crate::doc::{Document, ASYNC_THRESHOLD};
use crate::{codeeditor, dialog, JustQueryApp, Tab, TabDoc, TabKind};
use std::path::Path;

impl JustQueryApp {
    pub(crate) fn title_from_path(path: &Path) -> String {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled.sql".to_owned())
    }

    /// Open a file in a new tab via the native dialog.
    pub(crate) fn open_file(&mut self) {
        let Some(path) = dialog::open_file() else {
            return;
        };
        // already open? just switch to that tab instead of opening a duplicate
        if let Some(i) = self.tabs.iter().position(|t| t.path.as_deref() == Some(path.as_path())) {
            self.active_tab = i;
            self.focus_editor = true;
            return;
        }
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        // SQL vs XML is decided here, by extension — a `.xml` file opens as an XML tab.
        let is_xml = Self::is_xml_path(&path);
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = Tab::new(id, Self::title_from_path(&path));
        tab.path = Some(path.clone());
        if is_xml {
            tab.kind = TabKind::Xml;
        }
        if size <= ASYNC_THRESHOLD {
            match Document::open_sync(&path, None) {
                Ok(mut d) => {
                    // .xml → auto-detect the schema version from the head now content is available
                    if is_xml {
                        let head = d.read_bytes(0, 4096);
                        if let Some(si) = Self::detect_schema_idx(&String::from_utf8_lossy(&head)) {
                            tab.schema_idx = si;
                        }
                    }
                    tab.doc = TabDoc::Ready(Box::new(d));
                }
                Err(e) => {
                    self.error_modal = Some(format!("Open failed: {e}"));
                    return;
                }
            }
        } else {
            // large file → schema auto-detect is deferred to poll_loading once the doc is ready
            tab.doc = TabDoc::Loading { rx: Document::spawn_open(path), progress: 0 };
        }
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        self.focus_editor = true;
        self.cursor_ln = 1;
        self.cursor_col = 1;
    }

    /// Save the active tab; falls back to "Save As" when it has no backing file yet.
    pub(crate) fn save_active(&mut self) {
        // a connection-settings tab persists to the saved-connections store instead
        if self.is_connection_tab() {
            self.save_conn_tab();
            return;
        }
        let has_path = self.cur().and_then(|t| t.path.as_ref()).is_some();
        if !has_path {
            self.save_active_as();
            return;
        }
        let mut err = None;
        if let Some(t) = self.cur_mut() {
            if let Some(d) = t.doc_mut() {
                if let Err(e) = d.save(None) {
                    err = Some(format!("Save failed: {e}"));
                }
            }
        }
        if let Some(e) = err {
            self.error_modal = Some(e);
        }
    }

    /// Save the active tab under a new path chosen in the native dialog.
    pub(crate) fn save_active_as(&mut self) {
        if self.cur().is_none() {
            return;
        }
        let suggested = self.cur().map(|t| t.title.clone());
        let Some(path) = dialog::save_file(suggested.as_deref()) else {
            return;
        };
        let title = Self::title_from_path(&path);
        let want_xml = Self::is_xml_path(&path);
        let mut err = None;
        let mut flipped = false; // SQL↔XML changed → drop the galley cache (highlighter differs)
        if let Some(t) = self.cur_mut() {
            match t.doc_mut().map(|d| d.save(Some(&path))) {
                Some(Ok(())) => {
                    t.path = Some(path);
                    t.title = title;
                    // re-evaluate SQL/XML by the new extension — the only kind signal
                    let changed = matches!(
                        (&t.kind, want_xml),
                        (TabKind::Sql, true) | (TabKind::Xml, false)
                    );
                    if changed {
                        if want_xml {
                            let head = t.doc_mut().map(|d| d.read_bytes(0, 4096)).unwrap_or_default();
                            if let Some(si) =
                                Self::detect_schema_idx(&String::from_utf8_lossy(&head))
                            {
                                t.schema_idx = si;
                            }
                        }
                        t.kind = if want_xml { TabKind::Xml } else { TabKind::Sql };
                        t.lex = codeeditor::LexCache::default();
                        flipped = true;
                    }
                }
                Some(Err(e)) => err = Some(format!("Save failed: {e}")),
                None => {}
            }
        }
        if flipped {
            self.line_cache.clear();
        }
        if let Some(e) = err {
            self.error_modal = Some(e);
        }
    }
}
