//! The editor key tables, read from the editor crate's README so the in-app
//! reference cannot drift from the documentation.

use quarry_ui::settings::KeymapMode;

const README: &str = include_str!("../../../quarry-editor/README.md");

#[derive(Debug, Clone, PartialEq)]
pub struct KeyRef {
    pub section: String,
    pub keys: String,
    pub action: String,
}

/// Rows of the Helix or Standard key tables.
pub fn editor_keys(mode: KeymapMode) -> Vec<KeyRef> {
    parse(README, mode)
}

fn strip_ticks(s: &str) -> String {
    s.replace('`', "").replace("\\|", "|")
}

pub fn parse(readme: &str, mode: KeymapMode) -> Vec<KeyRef> {
    let want = match mode {
        KeymapMode::Helix => "Helix keymap",
        KeymapMode::Vim => "Vim keymap",
        KeymapMode::Standard => "Standard keymap",
    };
    let mut in_part = false;
    let mut section = String::new();
    let mut out = Vec::new();
    for line in readme.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            in_part = h.trim() == want;
            section = h.trim().to_string();
            continue;
        }
        if !in_part {
            continue;
        }
        if let Some(h) = line.strip_prefix("### ") {
            section = h.trim().to_string();
            continue;
        }
        if !line.starts_with('|') {
            continue;
        }
        // Split on unescaped pipes.
        let mut cells: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut chars = line.trim().trim_start_matches('|').chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' && chars.peek() == Some(&'|') {
                cur.push('|');
                chars.next();
            } else if c == '|' {
                cells.push(cur.trim().to_string());
                cur.clear();
            } else {
                cur.push(c);
            }
        }
        if !cur.trim().is_empty() {
            cells.push(cur.trim().to_string());
        }
        if cells.len() < 2 || cells[0].starts_with("---") || cells[0] == "keys" || cells[0] == "key" {
            continue;
        }
        out.push(KeyRef {
            section: section.clone(),
            keys: strip_ticks(&cells[0]),
            action: strip_ticks(&cells[1]),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helix_table_is_read() {
        let rows = editor_keys(KeymapMode::Helix);
        assert!(rows.len() > 50, "{}", rows.len());
        let w = rows.iter().find(|r| r.keys.starts_with("w b e")).unwrap();
        assert!(w.action.contains("next word start"));
        assert!(rows.iter().all(|r| !r.keys.contains('`')));
    }

    #[test]
    fn standard_table_is_separate() {
        let std = editor_keys(KeymapMode::Standard);
        let helix = editor_keys(KeymapMode::Helix);
        assert!(!std.is_empty());
        assert_ne!(std.first(), helix.first());
    }

    #[test]
    fn vim_table_is_read_from_its_own_section() {
        let rows = editor_keys(KeymapMode::Vim);
        assert!(rows.len() > 80, "{}", rows.len());
        assert!(rows.iter().any(|r| r.section.starts_with("Operators")), "sections: {:?}", rows.first());
        let dd = rows.iter().find(|r| r.keys.contains("dd")).expect("an operator row mentioning dd");
        assert!(!dd.action.is_empty());
        let sections: std::collections::HashSet<_> = rows.iter().map(|r| r.section.as_str()).collect();
        assert!(!sections.contains("Standard keymap") && !sections.contains("Helix keymap"));
        // `|` is escaped as `\|` in table cells and must come out as a pipe.
        assert!(rows.iter().all(|r| !r.keys.contains("\\|")));
        assert_ne!(rows.first(), editor_keys(KeymapMode::Helix).first());
    }

    #[test]
    fn parses_escaped_pipes() {
        let t = "## Helix keymap\n\n| keys | action |\n| --- | --- |\n| `g\\|` | goto column |\n";
        let r = parse(t, KeymapMode::Helix);
        assert_eq!(
            r,
            vec![KeyRef { section: "Helix keymap".into(), keys: "g|".into(), action: "goto column".into() }]
        );
    }
}
