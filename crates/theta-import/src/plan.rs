//! Deciding where file columns go: names for a new table, auto-mapping onto an existing one, and the
//! `CREATE TABLE` text. Pure functions so the dialog stays a thin layer over them.

use crate::convert::Target;
use theta_core::{ColumnInfo, DatabaseKind, TypeKind};
use theta_sql::keywords::{qualified_name, quote_if_needed};

/// Lowercase letters and digits only: `First Name`, `first_name` and `FIRST-NAME` all map to `firstname`.
pub fn normalise(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// For each source column, the index of the target column it maps to, if any. A target column is used
/// at most once, and an exact (case-insensitive) match beats a normalised one.
pub fn auto_map(source: &[String], targets: &[ColumnInfo]) -> Vec<Option<usize>> {
    let mut used = vec![false; targets.len()];
    let mut out: Vec<Option<usize>> = vec![None; source.len()];
    for pass in 0..2 {
        for (i, s) in source.iter().enumerate() {
            if out[i].is_some() {
                continue;
            }
            let hit = targets.iter().enumerate().find(|(j, t)| {
                !used[*j]
                    && if pass == 0 {
                        t.name.eq_ignore_ascii_case(s.trim())
                    } else {
                        let n = normalise(s);
                        !n.is_empty() && normalise(&t.name) == n
                    }
            });
            if let Some((j, _)) = hit {
                used[j] = true;
                out[i] = Some(j);
            }
        }
    }
    out
}

/// A column name that is easy to type afterwards: lowercase snake case, never empty, never starting with a
/// digit. `First Name` becomes `first_name`, an empty header cell `column_3`.
pub fn column_name_from_header(raw: &str, position: usize) -> String {
    let mut out = String::new();
    let mut pending_sep = false;
    for c in raw.trim().chars() {
        if c.is_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('_');
            }
            pending_sep = false;
            out.extend(c.to_lowercase());
        } else {
            pending_sep = true;
        }
    }
    if out.is_empty() {
        return format!("column_{}", position + 1);
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert_str(0, "c_");
    }
    out
}

/// Make names unique by suffixing `_2`, `_3`, ... (case-insensitively).
pub fn dedupe(names: &mut [String]) {
    let mut seen: Vec<String> = Vec::new();
    for n in names.iter_mut() {
        let mut candidate = n.clone();
        let mut k = 2;
        while seen.iter().any(|s| s.eq_ignore_ascii_case(&candidate)) {
            candidate = format!("{n}_{k}");
            k += 1;
        }
        seen.push(candidate.clone());
        *n = candidate;
    }
}

/// Table name suggestion from a file name: `Sales 2024.csv` becomes `sales_2024`.
pub fn table_name_from_file(path: &str) -> String {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    column_name_from_header(stem, 0).replace("column_1", "imported_data")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewColumn {
    pub name: String,
    /// The type text written into `CREATE TABLE`; free text so the user can change it.
    pub sql_type: String,
    pub nullable: bool,
}

pub fn create_table_sql(
    kind: DatabaseKind,
    schema: Option<&str>,
    table: &str,
    columns: &[NewColumn],
) -> String {
    let name = qualified_name(schema, table, kind);
    let cols: Vec<String> = columns
        .iter()
        .map(|c| {
            format!(
                "    {} {}{}",
                quote_if_needed(&c.name, kind),
                c.sql_type,
                if c.nullable { "" } else { " NOT NULL" }
            )
        })
        .collect();
    format!("CREATE TABLE {name} (\n{}\n)", cols.join(",\n"))
}

/// Classify a free-text SQL type; see [`theta_sql::ddl::kind_of_type_name`].
pub fn kind_of_type_name(sql_type: &str) -> TypeKind {
    theta_sql::ddl::kind_of_type_name(sql_type)
}

impl Target {
    /// A target for a column that does not exist yet, from the type text the user chose.
    pub fn from_type_name(sql_type: &str, db: DatabaseKind, nullable: bool) -> Target {
        let info = ColumnInfo {
            name: String::new(),
            data_type: sql_type.to_string(),
            kind: kind_of_type_name(sql_type),
            nullable,
            default: None,
            comment: None,
            geometry: None,
            ..Default::default()
        };
        Target::from_column(&info, db)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: "text".into(),
            kind: TypeKind::Text,
            nullable: true,
            default: None,
            comment: None,
            geometry: None,
            ..Default::default()
        }
    }

    #[test]
    fn auto_map_prefers_exact_then_normalised() {
        let targets = vec![col("id"), col("first_name"), col("Email"), col("note")];
        let source: Vec<String> = ["ID", "First Name", "e-mail", "email", "extra"].map(String::from).to_vec();
        // "email" matches Email exactly, so "e-mail" (normalised "email") finds it taken and stays unmapped.
        assert_eq!(auto_map(&source, &targets), vec![Some(0), Some(1), None, Some(2), None]);
    }

    #[test]
    fn auto_map_uses_each_target_once() {
        let targets = vec![col("a")];
        let source: Vec<String> = ["a", "A"].map(String::from).to_vec();
        assert_eq!(auto_map(&source, &targets), vec![Some(0), None]);
    }

    #[test]
    fn header_names_become_identifiers() {
        assert_eq!(column_name_from_header("First Name", 0), "first_name");
        assert_eq!(column_name_from_header("  Ünïcode-Col!  ", 0), "ünïcode_col");
        assert_eq!(column_name_from_header("", 2), "column_3");
        assert_eq!(column_name_from_header("2024 total", 0), "c_2024_total");
        assert_eq!(column_name_from_header("$$$", 4), "column_5");
    }

    #[test]
    fn duplicates_get_suffixes() {
        let mut n = ["a", "A", "b", "a"].map(String::from).to_vec();
        dedupe(&mut n);
        assert_eq!(n, ["a", "A_2", "b", "a_3"]);
    }

    #[test]
    fn table_name_from_path() {
        assert_eq!(table_name_from_file("/tmp/Sales 2024.csv"), "sales_2024");
        assert_eq!(table_name_from_file("C:\\data\\Orders.TSV"), "orders");
        assert_eq!(table_name_from_file("/x/.csv"), "imported_data");
    }

    #[test]
    fn create_table_quotes_only_when_needed() {
        let cols = vec![
            NewColumn { name: "id".into(), sql_type: "integer".into(), nullable: false },
            NewColumn { name: "order".into(), sql_type: "text".into(), nullable: true },
        ];
        assert_eq!(
            create_table_sql(DatabaseKind::Postgres, Some("public"), "t", &cols),
            "CREATE TABLE public.t (\n    id integer NOT NULL,\n    \"order\" text\n)"
        );
        assert!(create_table_sql(DatabaseKind::MySql, None, "t", &cols).contains("`order` text"));
    }

    #[test]
    fn type_names_classify() {
        for (t, k) in [
            ("character varying(40)", TypeKind::Text),
            ("INT", TypeKind::Integer),
            ("TINYINT(1)", TypeKind::Bool),
            ("DECIMAL(10,2)", TypeKind::Decimal),
            ("double precision", TypeKind::Float),
            ("timestamptz", TypeKind::Timestamp),
            ("DATETIME(6)", TypeKind::Timestamp),
            ("geometry(Point,4326)", TypeKind::Other),
        ] {
            assert_eq!(kind_of_type_name(t), k, "{t}");
        }
        let t = Target::from_type_name("VARCHAR(20)", DatabaseKind::MySql, true);
        assert_eq!(t.max_len, Some(20));
    }
}
