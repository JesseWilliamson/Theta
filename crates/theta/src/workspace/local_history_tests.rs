//! Local history through the workspace: a run snapshots what ran, a restore is one undo step and keeps what
//! it replaced, and the snapshots are real files that another process (the next start) reads.

use super::shortcut_tests::{editor_of, editor_text, setup};
use super::*;
use crate::exec::local_history::{Kind, LocalHistory};
use gpui_kit::TestAppContext;

fn key_and_id(f: &super::shortcut_tests::Fixture, vcx: &mut gpui_kit::VisualTestContext) -> (String, u64) {
    f.ws.read_with(vcx, |ws, _| {
        let id = ws.active_tab.expect("an active console");
        (ws.consoles[&id].key.clone(), id)
    })
}

#[gpui_kit::test]
fn running_snapshots_the_text_that_ran(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    editor_of(&f, vcx).update_in(vcx, |v, _, cx| v.update_editor(cx, |e| e.set_text("select 1 as one")));
    f.ws.update_in(vcx, |ws, window, cx| ws.run_active(run::RunReq::All, window, cx));
    let (key, _) = key_and_id(&f, vcx);
    let list = f.ws.read_with(vcx, |ws, _| {
        let h = ws.local_history();
        h.list(&key).into_iter().map(|m| (m.kind, h.read(&key, &m.id).unwrap())).collect::<Vec<_>>()
    });
    assert_eq!(list, vec![(Kind::Run, "select 1 as one".to_string())]);

    // The same text run again is not a second snapshot.
    f.ws.update_in(vcx, |ws, window, cx| ws.run_active(run::RunReq::All, window, cx));
    assert_eq!(f.ws.read_with(vcx, |ws, _| ws.local_history().list(&key).len()), 1);
}

#[gpui_kit::test]
fn a_restore_is_one_undo_step_and_the_replaced_text_is_kept(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    let editor = editor_of(&f, vcx);
    editor.update_in(vcx, |v, _, cx| v.update_editor(cx, |e| e.set_text("select 2 as two\nfrom t")));
    let (key, id) = key_and_id(&f, vcx);
    f.ws.update_in(vcx, |ws, window, cx| ws.restore_snapshot(id, "select 1 as one".into(), window, cx));
    assert_eq!(editor_text(&f, vcx), "select 1 as one");

    // What was in the editor is now a "before restore" snapshot.
    let (kind, text) = f.ws.read_with(vcx, |ws, _| {
        let h = ws.local_history();
        let m = h.list(&key).into_iter().next().unwrap();
        (m.kind, h.read(&key, &m.id).unwrap())
    });
    assert_eq!((kind, text.as_str()), (Kind::Restore, "select 2 as two\nfrom t"));

    // One undo brings the text back, not half of it.
    editor.update_in(vcx, |v, _, cx| v.update_editor(cx, |e| e.undo()));
    assert_eq!(editor_text(&f, vcx), "select 2 as two\nfrom t");
}

#[gpui_kit::test]
fn the_snapshots_are_files_another_process_can_read(cx: &mut TestAppContext) {
    let (f, vcx) = setup(cx);
    editor_of(&f, vcx).update_in(vcx, |v, _, cx| v.update_editor(cx, |e| e.set_text("select 7 as seven")));
    f.ws.update_in(vcx, |ws, window, cx| ws.run_active(run::RunReq::All, window, cx));
    let (key, dir) = f.ws.read_with(vcx, |ws, _| {
        let id = ws.active_tab.unwrap();
        (ws.consoles[&id].key.clone(), ws.data_dir.join("local-history"))
    });
    // A fresh store over the same directory, as the next start would build, sees the snapshot on disk.
    let reopened = LocalHistory::new(dir);
    let snap = reopened.list(&key);
    assert_eq!(snap.len(), 1);
    assert_eq!(reopened.read(&key, &snap[0].id).unwrap(), "select 7 as seven");
}
