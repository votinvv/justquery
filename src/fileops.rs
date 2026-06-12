//! Open / Save / Save As for editor tabs, backed by the native Win32 dialogs in [`crate::dialog`].
//! All methods hang off [`crate::JustQueryApp`].
//!
//! Файл НЕ читается целиком: документ маппится через mmap (см. [`crate::doc`]); файлы
//! крупнее порога открываются в фоне с прогрессом на листе редактора.

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
                Ok(d) => tab.doc = TabDoc::Ready(Box::new(d)),
                Err(e) => {
                    self.error_modal = Some(format!("Open failed: {e}"));
                    return;
                }
            }
        } else {
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
        if self.cur().is_some_and(|t| t.conn.is_some()) {
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
        let mut err = None;
        if let Some(t) = self.cur_mut() {
            if let Some(d) = t.doc_mut() {
                match d.save(Some(&path)) {
                    Ok(()) => {
                        t.path = Some(path);
                        t.title = title;
                    }
                    Err(e) => err = Some(format!("Save failed: {e}")),
                }
            }
        }
        if let Some(e) = err {
            self.error_modal = Some(e);
        }
    }
}
