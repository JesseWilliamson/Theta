//! Application bootstrap: platform setup, store creation, window, start-up
//! connections from the command line.

use crate::cli::Args;
use crate::connections::secrets::SecretStore;
use crate::connections::url::parse_connection_url;
use crate::connections::{ConnectionStore, connections_path, new_connection_id};
use crate::workspace::Workspace;
use crate::{demo, keymap, runtime};
use gpui_kit::component::TitleBar;
use gpui_kit::{AppContext as _, Bounds, Entity, WindowBounds, WindowOptions, px, size};
use std::sync::Arc;
use std::time::Duration;
use theta_core::{Catalog, ConnectionConfig, DatabaseKind, Schema, Table, TableKind};

/// The 128 px application icon, decoded once at start-up.
fn window_icon() -> Option<Arc<image::RgbaImage>> {
    const PNG: &[u8] = include_bytes!("../../../packaging/icons/hicolor/128x128/apps/theta.png");
    image::load_from_memory_with_format(PNG, image::ImageFormat::Png).ok().map(|i| Arc::new(i.to_rgba8()))
}

pub fn run(args: Args) {
    // Start the I/O runtime eagerly so the first connect does not pay for it.
    let _ = runtime::runtime();
    theta_ui::application().run(move |cx| {
        theta_ui::init(cx);
        cx.set_app_identity("theta", "Theta");
        keymap::bind(cx);
        keymap::set_menus(cx);
        if let Some(theme) = &args.theme {
            theta_ui::themes::set_theme(theme, cx);
        }

        let (store, warning) =
            ConnectionStore::load(Some(connections_path()), Arc::new(SecretStore::system()));
        let store: Entity<ConnectionStore> = cx.new(|_| store);

        let (w, h) = theta_ui::window_size_from_env().unwrap_or((1440., 900.));
        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(800.), px(500.))),
            // Matches StartupWMClass in packaging/linux/theta.desktop so the shell groups the
            // window with the launcher; the icon is used where the platform takes one per window (X11).
            app_id: Some("theta".into()),
            icon: window_icon(),
            #[cfg(target_os = "linux")]
            window_decorations: Some(gpui_kit::WindowDecorations::Client),
            #[cfg(target_os = "linux")]
            window_background: gpui_kit::WindowBackgroundAppearance::Opaque,
            ..TitleBar::window_options()
        };
        let store_for_window = store.clone();
        let opened = gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| Workspace::new(store_for_window, window, cx))
        });
        let (handle, workspace) = match opened {
            Ok(v) => v,
            Err(e) => {
                eprintln!("theta: could not open a window: {e}");
                cx.quit();
                return;
            }
        };
        handle
            .update(cx, |_, window, cx| {
                window.set_window_title("Theta");
                workspace.update(cx, |ws, cx| {
                    ws.focus_default(window, cx);
                    ws.show_startup_warnings(window, cx);
                });
                if let Some(w) = warning {
                    workspace
                        .update(cx, |ws, cx| ws.notify(crate::connections::NoticeKind::Error, w, window, cx));
                }
            })
            .ok();
        cx.activate(true);
        // Panics on background tasks do not end the app; tell the user and name the log.
        let reporter = workspace.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                for report in crate::panic::take_reports() {
                    let text = crate::panic::notice_text(&report);
                    let reporter = reporter.clone();
                    let _ = handle.update(cx, move |_, window, cx| {
                        reporter.update(cx, |ws, cx| {
                            ws.notify(crate::connections::NoticeKind::Error, text, window, cx)
                        })
                    });
                }
            }
        })
        .detach();
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let startup_sql = match (&args.run, &args.script) {
            (Some(sql), _) => Some(sql.clone()),
            (None, Some(path)) => match std::fs::read_to_string(path) {
                Ok(t) => Some(t),
                Err(e) => {
                    eprintln!("theta: --script {path}: {e}");
                    None
                }
            },
            _ => None,
        };
        if let Some(sql) = startup_sql {
            workspace.update(cx, |ws, _| ws.run_startup_sql(sql));
        }
        if let Some(file) = args.import.clone() {
            workspace.update(cx, |ws, _| ws.import_at_startup(file, args.into.clone()));
        }
        let startup_table = match (&args.modify, &args.new_table) {
            (Some(name), _) => Some(crate::workspace::StartupTable::Modify(name.clone())),
            (None, Some(schema)) => Some(crate::workspace::StartupTable::New(schema.clone())),
            _ => None,
        };
        if let Some(target) = startup_table {
            workspace.update(cx, |ws, _| ws.table_editor_at_startup(target));
        }
        startup_connections(&args, &store, cx);
    });
}

fn startup_connections(args: &Args, store: &Entity<ConnectionStore>, cx: &mut gpui_kit::App) {
    for url in &args.connect {
        match parse_connection_url(url) {
            Ok(parsed) => {
                let mut config = parsed.into_config(new_connection_id(), None);
                if let Some(ssh) = &args.ssh {
                    config.ssh = parse_ssh(ssh, args.ssh_key.as_deref());
                }
                store.update(cx, |s, cx| {
                    let id = s.add_temporary(config, cx);
                    s.connect(&id, cx);
                });
            }
            Err(e) => eprintln!("theta: --connect {url}: {e}"),
        }
    }
    if args.demo {
        let store = store.clone();
        cx.spawn(async move |cx| {
            let result = demo::create_demo_database().await;
            cx.update(|cx| match result {
                Ok(()) => store.update(cx, |s, cx| {
                    let existing = demo::find_existing(s.configs(), &demo::demo_path()).map(|c| c.id.clone());
                    let id = existing.unwrap_or_else(|| s.add(demo::demo_config(&new_connection_id()), cx));
                    s.connect(&id, cx);
                }),
                Err(e) => eprintln!("theta: could not create the demo database: {e}"),
            })
        })
        .detach();
    }
    if let Some(n) = args.stress_catalog {
        store.update(cx, |s, cx| {
            let config =
                ConnectionConfig::new("stress", format!("Stress ({n} tables)"), DatabaseKind::Postgres);
            let id = s.add_temporary(config, cx);
            s.set_catalog_synthetic(
                &id,
                synthetic_catalog(n),
                Arc::new(crate::connections::NullConnection),
                cx,
            );
        });
    }
}

/// A large synthetic catalog for `--stress-catalog`.
pub fn synthetic_catalog(tables: usize) -> Catalog {
    let tables = (0..tables)
        .map(|i| Table {
            name: format!("table_{i:05}"),
            kind: if i % 9 == 0 { TableKind::View } else { TableKind::Table },
            columns: vec![],
            primary_key: vec![],
            foreign_keys: vec![],
            indexes: vec![],
            row_estimate: Some((i as u64 * 7919) % 2_000_000),
            comment: None,
            ..Default::default()
        })
        .collect();
    Catalog {
        schemas: vec![Schema { name: "public".into(), tables, routines: vec![], sequences: vec![] }],
        default_schema: Some("public".into()),
    }
}

/// `user@host[:port]` plus an optional key file.
pub fn parse_ssh(spec: &str, key: Option<&str>) -> Option<theta_core::SshConfig> {
    let (user, rest) = spec.split_once('@')?;
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().ok()?),
        None => (rest, 22),
    };
    // Without a key file the running ssh-agent authenticates (the CLI says so at start-up).
    let auth = match key {
        Some(path) => theta_core::SshAuth::KeyFile { path: path.to_string(), passphrase: None },
        None => theta_core::SshAuth::Agent,
    };
    Some(theta_core::SshConfig { host: host.to_string(), port, user: user.to_string(), auth })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_spec_parses() {
        let c = parse_ssh("bob@box.example:2222", Some("/k")).unwrap();
        assert_eq!((c.user.as_str(), c.host.as_str(), c.port), ("bob", "box.example", 2222));
        assert_eq!(parse_ssh("bob@box", Some("/k")).unwrap().port, 22);
        assert!(parse_ssh("nohost", Some("/k")).is_none());
        assert_eq!(parse_ssh("bob@box", None).unwrap().auth, theta_core::SshAuth::Agent);
    }
}
