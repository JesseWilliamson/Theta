//! `--demo`: a seeded SQLite database in the config directory.

use crate::runtime;
use std::path::PathBuf;
use theta_core::{ConnectionConfig, DatabaseKind, Error};

pub fn demo_path() -> PathBuf {
    theta_ui::config_dir().join("demo.sqlite")
}

pub const DEMO_NAME: &str = "Demo (SQLite)";

pub fn demo_config(id: &str) -> ConnectionConfig {
    let mut c = ConnectionConfig::new(id, DEMO_NAME, DatabaseKind::Sqlite);
    c.path = demo_path().to_string_lossy().into_owned();
    c
}

/// The saved connection `--demo` should reuse, if any: by name, or by pointing at the demo
/// file (a renamed demo connection must not get a twin).
pub fn find_existing<'a>(
    configs: &'a [ConnectionConfig],
    path: &std::path::Path,
) -> Option<&'a ConnectionConfig> {
    configs.iter().find(|c| {
        c.name == DEMO_NAME || (c.kind == DatabaseKind::Sqlite && std::path::Path::new(&c.path) == path)
    })
}

/// Create the database file and (re)seed it. Runs on the Tokio runtime.
pub async fn create_demo_database() -> Result<(), Error> {
    runtime::spawn_result(async {
        let config = demo_config("demo");
        if let Some(dir) = demo_path().parent() {
            std::fs::create_dir_all(dir).map_err(Error::other)?;
        }
        let conn = theta_drivers::connect(&config).await?;
        let seeded = theta_drivers::demo::seed(&*conn).await;
        let _ = conn.close().await;
        seeded
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_existing_demo_connection_is_reused_by_name_or_by_file() {
        let path = std::path::Path::new("/cfg/demo.sqlite");
        assert!(find_existing(&[], path).is_none());
        let mut by_name = ConnectionConfig::new("a", DEMO_NAME, DatabaseKind::Sqlite);
        by_name.path = "/elsewhere/demo.sqlite".into();
        assert_eq!(find_existing(std::slice::from_ref(&by_name), path).unwrap().id, "a");
        let mut renamed = ConnectionConfig::new("b", "my sandbox", DatabaseKind::Sqlite);
        renamed.path = "/cfg/demo.sqlite".into();
        let other = ConnectionConfig::new("c", "prod", DatabaseKind::Postgres);
        assert_eq!(find_existing(&[other, renamed], path).unwrap().id, "b");
    }
}
