//! Commands of the active tool tab for the command palette.
//!
//! The diagram, the compare tabs, Sessions and Roles are driven by plain keys while they have focus. Every
//! one of those keys is also a palette command while its tab is in front, so a feature that has only a key
//! (Mermaid export, `m`) can be found by name. The keys are listed from the tables the views use.

use super::{AdminTab, ToolTab, Workspace};
use crate::admin::keys::Cmd as AdminCmd;
use crate::er::keys::Cmd as ErCmd;
use gpui_kit::{App, Window};
use std::rc::Rc;

pub struct TabCommand {
    pub label: &'static str,
    /// The key that does the same in the tab, as the key reference spells it.
    pub key: &'static str,
    pub run: Rc<dyn Fn(&mut Window, &mut App)>,
}

impl Workspace {
    /// The commands of the tab in front (none for a console or a table editor).
    pub fn tab_commands(&self, cx: &App) -> Vec<TabCommand> {
        let Some(id) = self.active_tab else { return Vec::new() };
        if let Some(tool) = self.tool_tab(id) {
            return match tool {
                ToolTab::Diagram(v) => {
                    let cmd = |label: &'static str, key: &'static str, c: ErCmd| {
                        let v = v.clone();
                        TabCommand {
                            label,
                            key,
                            run: Rc::new(move |window, cx| v.update(cx, |d, cx| d.run(c, window, cx))),
                        }
                    };
                    vec![
                        cmd("Diagram: Fit to Window", "f", ErCmd::Fit),
                        cmd("Diagram: Zoom In", "+", ErCmd::ZoomIn),
                        cmd("Diagram: Zoom Out", "-", ErCmd::ZoomOut),
                        cmd("Diagram: Reset Zoom", "0", ErCmd::ZoomReset),
                        cmd("Diagram: Next Table", "tab", ErCmd::Next),
                        cmd("Diagram: Previous Table", "shift-tab", ErCmd::Prev),
                        cmd("Diagram: Open Selected Table", "enter", ErCmd::Open),
                        cmd("Diagram: Jump to Table…", "/", ErCmd::Jump),
                        cmd("Diagram: Save as SVG…", "s", ErCmd::SaveSvg),
                        cmd("Diagram: Copy as Mermaid", "m", ErCmd::CopyMermaid),
                        cmd("Diagram: Reload", "r", ErCmd::Reload),
                    ]
                }
                ToolTab::SchemaCompare(v) => {
                    let (a, b, c, d) = (v.clone(), v.clone(), v.clone(), v.clone());
                    vec![
                        TabCommand {
                            label: "Compare: Open Migration Script",
                            key: "enter",
                            run: Rc::new(move |_, cx| a.update(cx, |v, cx| v.open_script(cx))),
                        },
                        TabCommand {
                            label: "Compare: Copy Migration Script",
                            key: "c",
                            run: Rc::new(move |_, cx| b.update(cx, |v, cx| v.copy_script(cx))),
                        },
                        TabCommand {
                            label: "Compare: Include Destructive Statements",
                            key: "d",
                            run: Rc::new(move |_, cx| c.update(cx, |v, cx| v.toggle_destructive(cx))),
                        },
                        TabCommand {
                            label: "Compare: Reload",
                            key: "r",
                            run: Rc::new(move |_, cx| d.update(cx, |v, cx| v.reload(cx))),
                        },
                    ]
                }
                ToolTab::DataCompare(v) => {
                    let (a, b, c) = (v.clone(), v.clone(), v.clone());
                    vec![
                        TabCommand {
                            label: "Compare: Open Sync Script",
                            key: "enter",
                            run: Rc::new(move |_, cx| a.update(cx, |v, cx| v.open_script(cx))),
                        },
                        TabCommand {
                            label: "Compare: Copy Sync Script",
                            key: "c",
                            run: Rc::new(move |_, cx| b.update(cx, |v, cx| v.copy_script(cx))),
                        },
                        TabCommand {
                            label: "Compare: Reload",
                            key: "r",
                            run: Rc::new(move |_, cx| c.update(cx, |v, cx| v.reload(cx))),
                        },
                    ]
                }
            };
        }
        let Some(admin) = self.admin_tab(id) else { return Vec::new() };
        let _ = cx;
        let run = |label: &'static str, key: &'static str, c: AdminCmd| match &admin {
            AdminTab::Sessions(v) => {
                let v = v.clone();
                TabCommand {
                    label,
                    key,
                    run: Rc::new(move |window, cx| v.update(cx, |s, cx| s.command(c, window, cx))),
                }
            }
            AdminTab::Roles(v) => {
                let v = v.clone();
                TabCommand {
                    label,
                    key,
                    run: Rc::new(move |window, cx| v.update(cx, |s, cx| s.command(c, window, cx))),
                }
            }
        };
        match admin {
            AdminTab::Sessions(_) => vec![
                run("Sessions: Refresh", "r", AdminCmd::Refresh),
                run("Sessions: Pause or Resume Refresh", "p", AdminCmd::Pause),
                run("Sessions: Change Refresh Interval", "i", AdminCmd::Interval),
                run("Sessions: Filter…", "/", AdminCmd::Filter),
                run("Sessions: Sort by Next Column", "s", AdminCmd::SortNext),
                run("Sessions: Reverse Sort", "shift-s", AdminCmd::SortReverse),
                run("Sessions: Cancel Query…", "c", AdminCmd::CancelQuery),
                run("Sessions: Terminate Session…", "x", AdminCmd::Terminate),
                run("Sessions: Copy Query", "y", AdminCmd::Copy),
                run("Sessions: Open Query in Console", "o", AdminCmd::OpenInConsole),
            ],
            AdminTab::Roles(_) => vec![
                run("Roles: Refresh", "r", AdminCmd::Refresh),
                run("Roles: Filter…", "/", AdminCmd::Filter),
                run("Roles: New Role…", "n", AdminCmd::New),
                run("Roles: Alter Role…", "e", AdminCmd::Alter),
                run("Roles: Set Password…", "w", AdminCmd::Password),
                run("Roles: Edit Membership…", "m", AdminCmd::Membership),
                run("Roles: Grant Privileges…", "a", AdminCmd::Grant),
                run("Roles: Revoke Privileges…", "v", AdminCmd::Revoke),
                run("Roles: Drop Role…", "shift-d", AdminCmd::Drop),
                run("Roles: Show or Hide System Roles", "t", AdminCmd::ToggleSystem),
            ],
        }
    }
}
