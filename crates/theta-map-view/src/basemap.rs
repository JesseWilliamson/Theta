//! The basemap registry: built-in providers plus the user's own, and what the map needs to
//! know about each (URL template, attribution, deepest zoom, dark or light).
//!
//! Only providers that allow keyless use by a desktop application are built in. CARTO's
//! basemaps need a (free) API key since 2026 and so are *not* listed; they work as a
//! custom basemap with the user's own key (see the user guide).

use theta_ui::settings::{CustomBasemap, DEFAULT_TILE_URL, Settings};

/// One basemap. `template` is `None` for "no basemap" (plain background and graticule).
#[derive(Debug, Clone, PartialEq)]
pub struct Basemap {
    pub id: String,
    pub name: String,
    pub template: Option<String>,
    /// Always shown on the map while this basemap is.
    pub attribution: String,
    /// Deepest zoom level the provider serves; deeper zooms scale that level's tiles.
    pub max_zoom: u8,
    /// A dark basemap is not dimmed in dark themes.
    pub dark: bool,
    pub custom: bool,
}

/// Id of the default basemap.
pub const DEFAULT_ID: &str = "osm";

/// The built-in basemaps, in menu order.
pub fn builtins() -> Vec<Basemap> {
    vec![
        Basemap {
            id: DEFAULT_ID.into(),
            name: "OpenStreetMap Standard".into(),
            template: Some(DEFAULT_TILE_URL.into()),
            attribution: "\u{a9} OpenStreetMap contributors".into(),
            max_zoom: 19,
            dark: false,
            custom: false,
        },
        Basemap {
            id: "opentopomap".into(),
            name: "OpenTopoMap".into(),
            template: Some("https://{s}.tile.opentopomap.org/{z}/{x}/{y}.png".into()),
            attribution:
                "Map data: \u{a9} OpenStreetMap contributors, SRTM | Map style: \u{a9} OpenTopoMap (CC-BY-SA)"
                    .into(),
            max_zoom: 17,
            dark: false,
            custom: false,
        },
        Basemap {
            id: "none".into(),
            name: "None (graticule)".into(),
            template: None,
            attribution: String::new(),
            max_zoom: 19,
            dark: false,
            custom: false,
        },
    ]
}

impl From<&CustomBasemap> for Basemap {
    fn from(c: &CustomBasemap) -> Basemap {
        Basemap {
            id: c.id.clone(),
            name: c.name.clone(),
            template: Some(c.url.clone()),
            attribution: c.attribution.clone(),
            max_zoom: c.max_zoom,
            dark: c.dark,
            custom: true,
        }
    }
}

/// Built-ins followed by the custom entries.
pub fn all(settings: &Settings) -> Vec<Basemap> {
    let mut v = builtins();
    v.extend(settings.map_custom_basemaps.iter().map(Basemap::from));
    v
}

/// The basemap in force: the selected one, or OpenStreetMap when the id is unknown (a
/// custom entry that was deleted or a settings file from a newer version).
pub fn selected(settings: &Settings) -> Basemap {
    let list = all(settings);
    list.iter()
        .find(|b| b.id == settings.map_basemap)
        .or_else(|| list.iter().find(|b| b.id == DEFAULT_ID))
        .cloned()
        .expect("the built-in default exists")
}

/// What to show as the attribution: the basemap's own text, or the tile host for a custom
/// entry that has none.
pub fn attribution_text(b: &Basemap) -> String {
    if !b.attribution.trim().is_empty() {
        return b.attribution.clone();
    }
    match &b.template {
        Some(t) => format!("Tiles: {}", crate::tiles::host_of(t)),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_keyless_https_xyz_templates_with_attribution() {
        let b = builtins();
        assert_eq!(b[0].id, DEFAULT_ID, "OpenStreetMap stays the default");
        let mut ids = std::collections::HashSet::new();
        for m in &b {
            assert!(ids.insert(m.id.clone()), "duplicate id {}", m.id);
            if let Some(t) = &m.template {
                assert!(CustomBasemap::url_is_valid(t), "{t}");
                assert!(t.starts_with("https://"));
                assert!(!t.contains("key="), "built-ins never need an API key");
                assert!(!m.attribution.is_empty(), "{} must carry its attribution", m.id);
            }
        }
        let topo = b.iter().find(|m| m.id == "opentopomap").unwrap();
        assert_eq!(topo.max_zoom, 17);
        assert!(topo.attribution.contains("OpenTopoMap") && topo.attribution.contains("SRTM"));
        assert!(b.iter().any(|m| m.template.is_none()), "a None entry exists");
    }

    #[test]
    fn selection_falls_back_to_openstreetmap() {
        let mut s = Settings::default();
        assert_eq!(selected(&s).id, "osm");
        s.map_basemap = "opentopomap".into();
        assert_eq!(selected(&s).id, "opentopomap");
        s.map_basemap = "deleted-custom".into();
        assert_eq!(selected(&s).id, "osm");
        s.map_custom_basemaps.push(CustomBasemap {
            id: "mine".into(),
            name: "Mine".into(),
            url: "https://t.example/{z}/{x}/{y}.png".into(),
            dark: true,
            ..Default::default()
        });
        s.map_basemap = "mine".into();
        let m = selected(&s);
        assert!(m.custom && m.dark);
        assert_eq!(attribution_text(&m), "Tiles: t.example", "no attribution given: name the host");
    }
}
