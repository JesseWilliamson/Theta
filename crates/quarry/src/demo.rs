//! `--demo`: a seeded SQLite database in the config directory.

use crate::runtime;
use quarry_core::{ConnectionConfig, DatabaseKind, Error};
use std::path::PathBuf;

pub fn demo_path() -> PathBuf {
    quarry_ui::config_dir().join("demo.sqlite")
}

pub const DEMO_NAME: &str = "Demo (SQLite)";

pub fn demo_config(id: &str) -> ConnectionConfig {
    let mut c = ConnectionConfig::new(id, DEMO_NAME, DatabaseKind::Sqlite);
    c.path = demo_path().to_string_lossy().into_owned();
    c
}

/// Create the database file and (re)seed it. Runs on the Tokio runtime.
pub async fn create_demo_database() -> Result<(), Error> {
    runtime::spawn_result(async {
        let config = demo_config("demo");
        if let Some(dir) = demo_path().parent() {
            std::fs::create_dir_all(dir).map_err(Error::other)?;
        }
        let conn = quarry_drivers::connect(&config).await?;
        let seeded = quarry_drivers::demo::seed(&*conn).await;
        let _ = conn.close().await;
        seeded
    })
    .await
}
