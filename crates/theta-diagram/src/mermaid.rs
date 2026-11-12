//! Mermaid `erDiagram` text for a graph, for pasting into a README, an issue or the Mermaid live editor.
//!
//! Mermaid's grammar is narrower than SQL's, so names are made safe rather than copied: an entity name that is
//! not a plain word is quoted; an attribute type may contain letters, digits, `_`, `-`, parentheses and square
//! brackets but no space or comma, so `numeric(10,2)` becomes `numeric(10_2)` and `double precision`
//! becomes `double_precision`; an attribute name is reduced to word characters. Relationship labels are
//! quoted, and a quote inside one becomes an apostrophe.

use crate::model::Graph;

fn entity(name: &str) -> String {
    let plain = !name.is_empty()
        && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        && name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_');
    if plain { name.to_string() } else { format!("\"{}\"", name.replace('"', "'")) }
}

fn attr_type(t: &str) -> String {
    let mut s: String = t
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '_' | '-' | '(' | ')' | '[' | ']') { c } else { '_' })
        .collect();
    // Must start with a letter.
    if !s.chars().next().is_some_and(|c| c.is_alphabetic()) {
        s.insert_str(0, "t_");
    }
    // `_` runs from spaces and commas read badly.
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    s.trim_end_matches('_').to_string()
}

fn attr_name(n: &str) -> String {
    let s: String = n.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if s.is_empty() { "_".to_string() } else { s }
}

pub fn render(g: &Graph) -> String {
    let mut out = String::from("erDiagram\n");
    for edge in &g.edges {
        // PARENT ||--o{ CHILD : "name". The parent side is "zero or one" when the key may be NULL; the child
        // side is "zero or one" when the key is unique and "zero or more" otherwise.
        let parent = if edge.optional { "|o" } else { "||" };
        let child = if edge.unique { "o|" } else { "o{" };
        let label = if edge.name.is_empty() { edge.from_cols.join(", ") } else { edge.name.clone() };
        out.push_str(&format!(
            "    {} {}--{} {} : \"{}\"\n",
            entity(&g.label(edge.to)),
            parent,
            child,
            entity(&g.label(edge.from)),
            label.replace('"', "'")
        ));
    }
    for (i, n) in g.nodes.iter().enumerate() {
        if n.columns.is_empty() {
            out.push_str(&format!("    {}\n", entity(&g.label(i))));
            continue;
        }
        out.push_str(&format!("    {} {{\n", entity(&g.label(i))));
        for c in &n.columns {
            let mut keys = Vec::new();
            if c.pk {
                keys.push("PK");
            }
            if c.fk {
                keys.push("FK");
            }
            let key = if keys.is_empty() { String::new() } else { format!(" {}", keys.join(", ")) };
            out.push_str(&format!("        {} {}{}\n", attr_type(&c.data_type), attr_name(&c.name), key));
        }
        out.push_str("    }\n");
    }
    out
}
