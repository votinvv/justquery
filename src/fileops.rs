//! Open / Save / Save As for editor tabs, backed by the native Win32 dialogs in [`crate::dialog`].
//! All methods hang off [`crate::JustQueryApp`].
//!
//! The file is NOT read in full: the document is mapped via mmap (see [`crate::doc`]); files
//! larger than the threshold open in the background with progress shown on the editor sheet.

use crate::doc::{Document, ASYNC_THRESHOLD};
use crate::{dialog, JustQueryApp, Tab, TabDoc};
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
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = Tab::new(id, Self::title_from_path(&path));
        tab.path = Some(path.clone());
        if size <= ASYNC_THRESHOLD {
            match Document::open_sync(&path, None) {
                Ok(d) => {
                    tab.doc = TabDoc::Ready(Box::new(d));
                }
                Err(e) => {
                    self.error_modal = Some(format!("Open failed: {e}"));
                    return;
                }
            }
        } else {
            // large file → opened in the background with progress on the editor sheet
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
        // the Scan tab's Save IS Apply: push the staged scan settings to the live collector + disk
        if self.is_scan_tab() {
            self.apply_meta_edits();
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

    /// Save the active tab under a new path chosen in the native dialog. Pages without a text
    /// document (connection form, About, …) have no Save-As target and are a no-op.
    pub(crate) fn save_active_as(&mut self) {
        // a connection tab's "Save As" exports the connection to a chosen `.conn` file
        if self.is_connection_tab() {
            self.export_active_conn();
            return;
        }
        if !self.cur().is_some_and(|t| t.is_editor()) {
            return; // non-document pages: nothing to "Save As"
        }
        let suggested = self.cur().map(|t| t.title.clone());
        let Some(path) = dialog::save_file(suggested.as_deref()) else {
            return;
        };
        let title = Self::title_from_path(&path);
        let mut err = None;
        if let Some(t) = self.cur_mut() {
            match t.doc_mut().map(|d| d.save(Some(&path))) {
                Some(Ok(())) => {
                    t.path = Some(path);
                    t.title = title;
                }
                Some(Err(e)) => {
                    err = Some(format!("Save failed: {e}"));
                }
                None => {}
            }
        }
        if let Some(e) = err {
            self.error_modal = Some(e);
        }
    }
}
