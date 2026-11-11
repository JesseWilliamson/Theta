//! The native dump tools (`pg_dump`, `mysqldump` / `mariadb-dump`) as an alternative to Theta's own dump.
//!
//! They produce the complete, native format (routines, triggers, extensions, ownership ...), so when one is
//! installed it is offered next to the built-in dump. This module only builds the command; the app runs it.
//! The password is never an argument (it would show in `ps` and shell history): it goes in the environment
//! variable the tool reads, and the command line shown to the user names the variable without its value.

use std::path::{Path, PathBuf};
use theta_core::{DatabaseKind, Family};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCommand {
    pub program: String,
    pub args: Vec<String>,
    /// Environment variable that carries the password, if there is one.
    pub password_env: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub struct NativeRequest<'a> {
    pub kind: DatabaseKind,
    pub host: &'a str,
    pub port: u16,
    pub user: &'a str,
    pub database: &'a str,
    pub has_password: bool,
    /// `(schema, table)`; empty means everything.
    pub tables: Vec<(String, String)>,
    pub structure: bool,
    pub data: bool,
    pub output: &'a Path,
}

/// The executable names to look for, in order of preference.
pub fn program_names(kind: DatabaseKind) -> &'static [&'static str] {
    match kind.family() {
        Family::Postgres => &["pg_dump"],
        Family::MySql => &["mysqldump", "mariadb-dump"],
        _ => &[],
    }
}

/// Find the first of `names` in `path_var` (the value of `PATH`).
pub fn find_in_path(names: &[&str], path_var: &str) -> Option<PathBuf> {
    let exe = |n: &str| if cfg!(windows) { format!("{n}.exe") } else { n.to_string() };
    for dir in std::env::split_paths(path_var) {
        for n in names {
            let p = dir.join(exe(n));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Build the command line, or the reason the tool cannot do this request.
pub fn command(program: &str, r: &NativeRequest) -> Result<NativeCommand, String> {
    if !r.structure && !r.data {
        return Err("Choose structure, data or both".into());
    }
    let mut args: Vec<String> = Vec::new();
    match r.kind.family() {
        Family::Postgres => {
            if !r.host.is_empty() {
                args.extend(["--host".into(), r.host.into()]);
            }
            if r.port != 0 {
                args.extend(["--port".into(), r.port.to_string()]);
            }
            if !r.user.is_empty() {
                args.extend(["--username".into(), r.user.into()]);
            }
            // Never prompt: the app has no terminal to answer.
            args.push("--no-password".into());
            args.extend(["--file".into(), r.output.display().to_string()]);
            if !r.structure {
                args.push("--data-only".into());
            }
            if !r.data {
                args.push("--schema-only".into());
            }
            for (s, t) in &r.tables {
                args.extend(["--table".into(), if s.is_empty() { t.clone() } else { format!("{s}.{t}") }]);
            }
            args.push(r.database.into());
            Ok(NativeCommand {
                program: program.into(),
                args,
                password_env: r.has_password.then_some("PGPASSWORD"),
            })
        }
        Family::MySql => {
            if !r.host.is_empty() {
                args.extend(["--host".into(), r.host.into()]);
            }
            if r.port != 0 {
                args.extend(["--port".into(), r.port.to_string()]);
            }
            if !r.user.is_empty() {
                args.extend(["--user".into(), r.user.into()]);
            }
            args.push(format!("--result-file={}", r.output.display()));
            if !r.data {
                args.push("--no-data".into());
            }
            if !r.structure {
                args.push("--no-create-info".into());
            }
            args.push(r.database.into());
            args.extend(r.tables.iter().map(|(_, t)| t.clone()));
            Ok(NativeCommand {
                program: program.into(),
                args,
                password_env: r.has_password.then_some("MYSQL_PWD"),
            })
        }
        _ => Err("this product has no native dump tool Theta can drive".into()),
    }
}

impl NativeCommand {
    /// The command as shown to the user: arguments quoted for a shell, and the password as the variable
    /// that carries it (`PGPASSWORD=…`), never its value.
    pub fn display(&self) -> String {
        let quote = |a: &str| {
            if !a.is_empty() && a.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@,+".contains(c)) {
                a.to_string()
            } else {
                format!("'{}'", a.replace('\'', "'\\''"))
            }
        };
        let mut out = String::new();
        if let Some(v) = self.password_env {
            out.push_str(&format!("{v}=… "));
        }
        out.push_str(&quote(&self.program));
        for a in &self.args {
            out.push(' ');
            out.push_str(&quote(a));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(kind: DatabaseKind) -> NativeRequest<'static> {
        NativeRequest {
            kind,
            host: "db.example.com",
            port: 5432,
            user: "ann",
            database: "shop",
            has_password: true,
            tables: vec![("public".into(), "orders".into()), ("public".into(), "it's".into())],
            structure: true,
            data: true,
            output: Path::new("/tmp/out dir/shop.sql"),
        }
    }

    #[test]
    fn pg_dump_command_keeps_the_password_out_of_argv() {
        let c = command("pg_dump", &req(DatabaseKind::Postgres)).unwrap();
        assert_eq!(c.password_env, Some("PGPASSWORD"));
        assert!(c.args.iter().all(|a| !a.to_lowercase().contains("password") || a == "--no-password"));
        assert_eq!(c.args.last().unwrap(), "shop");
        let shown = c.display();
        assert!(shown.starts_with("PGPASSWORD=… pg_dump --host db.example.com --port 5432 --username ann --no-password --file '/tmp/out dir/shop.sql'"), "{shown}");
        assert!(shown.contains("--table public.orders") && shown.contains("'public.it'\\''s'"), "{shown}");
    }

    #[test]
    fn structure_and_data_switches() {
        let mut r = req(DatabaseKind::Postgres);
        r.data = false;
        assert!(command("pg_dump", &r).unwrap().args.contains(&"--schema-only".to_string()));
        r.data = true;
        r.structure = false;
        assert!(command("pg_dump", &r).unwrap().args.contains(&"--data-only".to_string()));
        r.data = false;
        assert!(command("pg_dump", &r).is_err());
        let mut m = req(DatabaseKind::MySql);
        m.data = false;
        assert!(command("mysqldump", &m).unwrap().args.contains(&"--no-data".to_string()));
    }

    #[test]
    fn mysqldump_command_uses_the_environment_for_the_password() {
        let c = command("mysqldump", &req(DatabaseKind::MySql)).unwrap();
        assert_eq!(c.password_env, Some("MYSQL_PWD"));
        assert!(c.args.iter().any(|a| a.starts_with("--result-file=")));
        assert!(!c.args.iter().any(|a| a.starts_with("--password") || a.starts_with("-p")));
        let pos = c.args.iter().position(|a| a == "shop").unwrap();
        assert_eq!(&c.args[pos + 1..], ["orders", "it's"]);
        let mut r = req(DatabaseKind::MySql);
        r.has_password = false;
        assert_eq!(command("mysqldump", &r).unwrap().password_env, None);
    }

    #[test]
    fn other_products_have_no_native_tool() {
        assert!(command("x", &req(DatabaseKind::Sqlite)).is_err());
        assert!(program_names(DatabaseKind::DuckDb).is_empty());
        assert_eq!(program_names(DatabaseKind::TimescaleDb), ["pg_dump"]);
    }

    #[test]
    fn finds_a_program_on_the_path() {
        let dir = std::env::temp_dir().join(format!("theta-native-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = if cfg!(windows) { "mariadb-dump.exe" } else { "mariadb-dump" };
        std::fs::write(dir.join(name), "").unwrap();
        let path = std::env::join_paths([PathBuf::from("/nonexistent"), dir.clone()]).unwrap();
        let found = find_in_path(&["mysqldump", "mariadb-dump"], path.to_str().unwrap()).unwrap();
        assert_eq!(found, dir.join(name));
        assert!(find_in_path(&["pg_dump"], path.to_str().unwrap()).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
