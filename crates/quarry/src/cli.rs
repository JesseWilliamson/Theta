//! Command-line flags (small enough that a hand-rolled parser beats a dependency).
//!
//! ```text
//! quarry [--demo] [--connect URL]... [--run SQL | --script FILE] [--ssh user@host:port --ssh-key PATH]
//!        [--stress-catalog[=N]] [--theme NAME]
//! ```

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Args {
    /// Create and connect the SQLite demo database.
    pub demo: bool,
    /// Temporary connections to add and connect (URLs).
    pub connect: Vec<String>,
    /// Debug: add a synthetic connection with this many tables.
    pub stress_catalog: Option<usize>,
    pub theme: Option<String>,
    /// SQL to run once the first start-up connection is ready.
    pub run: Option<String>,
    pub script: Option<String>,
    /// SSH tunnel for `--connect`: `user@host:port`.
    pub ssh: Option<String>,
    pub ssh_key: Option<String>,
}

pub fn parse(args: impl IntoIterator<Item = String>, env_demo: bool) -> Result<Args, String> {
    let mut out = Args { demo: env_demo, ..Default::default() };
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--demo" => out.demo = true,
            "--connect" => out.connect.push(it.next().ok_or("--connect needs a URL")?),
            "--run" => out.run = Some(it.next().ok_or("--run needs SQL")?),
            "--script" => out.script = Some(it.next().ok_or("--script needs a file")?),
            "--ssh" => out.ssh = Some(it.next().ok_or("--ssh needs user@host:port")?),
            "--ssh-key" => out.ssh_key = Some(it.next().ok_or("--ssh-key needs a path")?),
            "--theme" => out.theme = Some(it.next().ok_or("--theme needs a theme name")?),
            "--stress-catalog" => out.stress_catalog = Some(5000),
            a if a.starts_with("--stress-catalog=") => {
                let n = a["--stress-catalog=".len()..]
                    .parse()
                    .map_err(|_| "--stress-catalog=N needs a number")?;
                out.stress_catalog = Some(n);
            }
            a if a.starts_with("--connect=") => out.connect.push(a["--connect=".len()..].to_string()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str], env: bool) -> Result<Args, String> {
        parse(args.iter().map(|s| s.to_string()), env)
    }

    #[test]
    fn flags() {
        let a = p(
            &["--demo", "--connect", "sqlite:///x.db", "--connect=mysql://h/d", "--theme", "Tokyo Night"],
            false,
        )
        .unwrap();
        assert!(a.demo);
        assert_eq!(a.connect, vec!["sqlite:///x.db", "mysql://h/d"]);
        assert_eq!(a.theme.as_deref(), Some("Tokyo Night"));
    }

    #[test]
    fn env_enables_demo_and_stress_has_default() {
        assert!(p(&[], true).unwrap().demo);
        assert_eq!(p(&["--stress-catalog"], false).unwrap().stress_catalog, Some(5000));
        assert_eq!(p(&["--stress-catalog=12"], false).unwrap().stress_catalog, Some(12));
    }

    #[test]
    fn errors() {
        assert!(p(&["--connect"], false).is_err());
        assert!(p(&["--bogus"], false).is_err());
        assert!(p(&["--stress-catalog=x"], false).is_err());
    }
}
