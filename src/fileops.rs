//! Open / Save / Save As for editor tabs, backed by the native Win32 dialogs in [`crate::dialog`].
//! All methods hang off [`crate::JustQueryApp`].

use crate::{dialog, JustQueryApp, Tab};
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
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let id = self.next_tab_id;
                self.next_tab_id += 1;
                let mut tab = Tab::new(id, Self::title_from_path(&path));
                tab.sql = content;
                tab.path = Some(path);
                self.tabs.push(tab);
                self.active_tab = self.tabs.len() - 1;
                self.focus_editor = true;
                self.cursor_ln = 1;
                self.cursor_col = 1;
                self.caret = 0;
            }
            Err(e) => self.error_modal = Some(format!("Open failed: {e}")),
        }
    }

    /// Save the active tab; falls back to "Save As" when it has no backing file yet.
    pub(crate) fn save_active(&mut self) {
        // a connection-settings tab persists to the saved-connections store instead
        if self.cur().is_some_and(|t| t.conn.is_some()) {
            self.save_conn_tab();
            return;
        }
        match self.cur().and_then(|t| t.path.clone()) {
            Some(p) => self.write_active_to(&p),
            None => self.save_active_as(),
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
        self.write_active_to(&path);
        let title = Self::title_from_path(&path);
        if let Some(t) = self.cur_mut() {
            t.path = Some(path);
            t.title = title;
        }
    }

    fn write_active_to(&mut self, path: &Path) {
        let text = match self.cur() {
            Some(t) => t.sql.clone(),
            None => return,
        };
        match std::fs::write(path, text) {
            Ok(()) => {
                if let Some(t) = self.cur_mut() {
                    t.dirty = false;
                }
            }
            Err(e) => self.error_modal = Some(format!("Save failed: {e}")),
        }
    }
}
