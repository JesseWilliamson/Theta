//! Connection folders: the dialogs behind the explorer's folder commands.

use super::Workspace;
use crate::connections::NoticeKind;
use crate::dialogs::name;
use crate::explorer::tree::NodeKey;
use crate::palette;
use gpui_kit::component::WindowExt as _;
use gpui_kit::{Context, Window};
use std::rc::Rc;

impl Workspace {
    pub(super) fn new_connection_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let for_validate = store.clone();
        name::open(
            name::Prompt { title: "New Folder", label: "Folder name", initial: "", submit: "Create" },
            Rc::new(move |text, cx| {
                if for_validate.read(cx).layout().name_is_free(text, None) {
                    Ok(())
                } else if text.trim().is_empty() {
                    Err("A folder needs a name".into())
                } else {
                    Err(format!("A folder named “{}” already exists", text.trim()))
                }
            }),
            Rc::new(move |text, _, cx| {
                let _ = store.update(cx, |s, cx| s.add_folder(&text, cx));
            }),
            window,
            cx,
        );
    }

    pub(super) fn rename_connection_folder(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = self.store.read(cx).layout().folder(&id).map(|f| f.name.clone()) else { return };
        let (store, for_validate, fid) = (self.store.clone(), self.store.clone(), id.clone());
        name::open(
            name::Prompt {
                title: "Rename Folder",
                label: "Folder name",
                initial: &current,
                submit: "Rename",
            },
            Rc::new(move |text, cx| {
                if for_validate.read(cx).layout().name_is_free(text, Some(&id)) {
                    Ok(())
                } else if text.trim().is_empty() {
                    Err("A folder needs a name".into())
                } else {
                    Err(format!("A folder named “{}” already exists", text.trim()))
                }
            }),
            Rc::new(move |text, _, cx| {
                let _ = store.update(cx, |s, cx| s.rename_folder(&fid, &text, cx));
            }),
            window,
            cx,
        );
    }

    pub(super) fn confirm_delete_folder(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::button::ButtonVariant;
        let store = self.store.clone();
        let (folder, members) = {
            let s = store.read(cx);
            let l = s.layout();
            (
                l.folder(&id).map(|f| f.name.clone()).unwrap_or_default(),
                l.folder_of.values().filter(|f| **f == id).count(),
            )
        };
        let what = match members {
            0 => "The folder is empty.".to_string(),
            1 => "Its connection moves to the top level; it is not deleted.".to_string(),
            n => format!("Its {n} connections move to the top level; none of them is deleted."),
        };
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (store, id) = (store.clone(), id.clone());
            alert
                .title(format!("Delete folder “{folder}”?"))
                .description(what.clone())
                .confirm()
                .ok_text("Delete Folder")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    store.update(cx, |s, cx| s.delete_folder(&id, cx));
                    true
                })
        });
    }

    pub(super) fn move_to_folder_picker(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let delegate = palette::FolderPicker::new(self.store.clone(), id);
        self.open_picker(delegate, window, cx);
    }

    /// The palette command: the connection selected in the explorer, else the active one.
    pub(super) fn move_active_connection_to_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = match self.explorer.read(cx).selected_key() {
            Some(NodeKey::Connection(id)) => Some(id.clone()),
            _ => None,
        };
        match selected.or_else(|| self.active_connection_id(cx)) {
            Some(id) => self.move_to_folder_picker(id, window, cx),
            None => self.notify(NoticeKind::Info, "Add a connection first", window, cx),
        }
    }
}
