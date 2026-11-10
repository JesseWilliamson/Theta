//! One `AssetSource` serving Quarry's own SVGs first and gpui-kit's complete
//! Lucide set second, so views can mix `IconName::Database` with our
//! database-kind glyphs without caring where each comes from.

use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

const OWN: &[(&str, &[u8])] = &[
    ("icons/db-postgres.svg", include_bytes!("../assets/icons/db-postgres.svg")),
    ("icons/db-mysql.svg", include_bytes!("../assets/icons/db-mysql.svg")),
    ("icons/db-sqlite.svg", include_bytes!("../assets/icons/db-sqlite.svg")),
];

pub struct QuarryAssets;

impl AssetSource for QuarryAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = OWN.iter().find(|(p, _)| *p == path) {
            return Ok(Some(Cow::Borrowed(*bytes)));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = gpui_kit::assets::AllAssets.list(path)?;
        all.extend(OWN.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)));
        Ok(all)
    }
}
