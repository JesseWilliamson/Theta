//! Command-line flags (small enough that a hand-rolled parser beats a dependency).
//!
//! [`USAGE`] is the single description of every flag; `--help` prints it and a parse error
//! repeats it, so the two cannot drift apart. A test checks that every flag the parser
//! accepts appears in it.

/// The full usage text.
pub const USAGE: &str = "\
usage: theta [OPTIONS]

Options:
  --connect URL          Add a temporary connection and connect to it. Repeatable.
                         URL: postgres://user:pass@host:port/db, mysql://..., sqlite:///path
  --ssh USER@HOST[:PORT] Tunnel the --connect connections through this SSH server (port 22
                         by default). Without --ssh-key the running ssh-agent
                         (SSH_AUTH_SOCK) authenticates.
  --ssh-key PATH         Authenticate the --ssh tunnel with this private key file.
  --run SQL              Run SQL once the first start-up connection is ready.
  --script FILE          Run the SQL in FILE once the first start-up connection is ready.
  --import FILE          Open the Import Data dialog on FILE once the first start-up
                         connection is ready (a path to a .csv/.tsv file).
  --into [SCHEMA.]TABLE  With --import: import into this existing table instead of a new one.
  --modify [SCHEMA.]TABLE  Open the Modify Table editor on TABLE once the first start-up
                         connection is ready.
  --new-table [SCHEMA]   Open the New Table editor once the first start-up connection is ready.
  --demo                 Create (or reuse) and connect the \"Demo (SQLite)\" sample database.
                         Also enabled by THETA_DEMO=1.
  --theme NAME           Start with this theme (for example \"Tokyo Night\").
  --stress-catalog[=N]   Debug: add a synthetic connection with N tables (default 5000)
                         to exercise the explorer, completion and pickers.
  --version, -V          Print the version and exit.
  --help, -h             Print this help and exit.

Environment:
  THETA_CONFIG_DIR      Use this directory for settings, connections and logs.
  THETA_DEMO=1          Same as --demo.
";

/// Every flag the parser accepts (for the usage test).
pub const FLAGS: [&str; 15] = [
    "--connect",
    "--ssh",
    "--ssh-key",
    "--run",
    "--script",
    "--import",
    "--into",
    "--modify",
    "--new-table",
    "--demo",
    "--theme",
    "--stress-catalog",
    "--version",
    "--help",
    "-h",
];

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
    /// File to open the Import Data dialog on, once connected.
    pub import: Option<String>,
    /// Existing table (`[schema.]table`) for `--import`.
    pub into: Option<String>,
    /// `--modify [schema.]table`: open the table editor on it once connected.
    pub modify: Option<String>,
    /// `--new-table [schema]`: open the editor on a new table once connected (inner `None`: default schema).
    pub new_table: Option<Option<String>>,
    /// SSH tunnel for `--connect`: `user@host:port`.
    pub ssh: Option<String>,
    /// Private key for the tunnel; without it the ssh-agent authenticates.
    pub ssh_key: Option<String>,
    /// `--help` was given: print [`USAGE`] and exit.
    pub help: bool,
    /// `--version` was given: print the version and exit.
    pub version: bool,
}

pub fn parse(args: impl IntoIterator<Item = String>, env_demo: bool) -> Result<Args, String> {
    let mut out = Args { demo: env_demo, ..Default::default() };
    let mut it = args.into_iter().peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--help" | "-h" => out.help = true,
            "--version" | "-V" => out.version = true,
            "--demo" => out.demo = true,
            "--connect" => out.connect.push(it.next().ok_or("--connect needs a URL")?),
            "--run" => out.run = Some(it.next().ok_or("--run needs SQL")?),
            "--script" => out.script = Some(it.next().ok_or("--script needs a file")?),
            "--import" => out.import = Some(it.next().ok_or("--import needs a file")?),
            "--into" => out.into = Some(it.next().ok_or("--into needs a table name")?),
            "--modify" => out.modify = Some(it.next().ok_or("--modify needs a table name")?),
            // The schema is optional: it is taken only when the next word is not another flag.
            "--new-table" => out.new_table = Some(it.next_if(|a| !a.starts_with('-'))),
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
            a if a.starts_with("--run=") => out.run = Some(a["--run=".len()..].to_string()),
            a if a.starts_with("--script=") => out.script = Some(a["--script=".len()..].to_string()),
            a if a.starts_with("--import=") => out.import = Some(a["--import=".len()..].to_string()),
            a if a.starts_with("--into=") => out.into = Some(a["--into=".len()..].to_string()),
            a if a.starts_with("--modify=") => out.modify = Some(a["--modify=".len()..].to_string()),
            a if a.starts_with("--new-table=") => {
                out.new_table = Some(Some(a["--new-table=".len()..].to_string()))
            }
            a if a.starts_with("--ssh=") => out.ssh = Some(a["--ssh=".len()..].to_string()),
            a if a.starts_with("--ssh-key=") => out.ssh_key = Some(a["--ssh-key=".len()..].to_string()),
            a if a.starts_with("--theme=") => out.theme = Some(a["--theme=".len()..].to_string()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if out.ssh.is_some() && out.connect.is_empty() {
        return Err("--ssh needs at least one --connect URL to tunnel".into());
    }
    if out.ssh_key.is_some() && out.ssh.is_none() {
        return Err("--ssh-key needs --ssh user@host[:port]".into());
    }
    if out.into.is_some() && out.import.is_none() {
        return Err("--into needs --import FILE".into());
    }
    if out.import.is_some() && (out.run.is_some() || out.script.is_some()) {
        return Err("use --import on its own, without --run or --script".into());
    }
    if out.modify.is_some() && out.new_table.is_some() {
        return Err("use either --modify or --new-table, not both".into());
    }
    if (out.modify.is_some() || out.new_table.is_some())
        && (out.import.is_some() || out.run.is_some() || out.script.is_some())
    {
        return Err("use --modify / --new-table on their own, without --import, --run or --script".into());
    }
    if out.run.is_some() && out.script.is_some() {
        return Err("use either --run or --script, not both".into());
    }
    Ok(out)
}

/// The one line that says how the `--ssh` tunnel authenticates.
pub fn ssh_auth_note(args: &Args) -> Option<String> {
    let ssh = args.ssh.as_ref()?;
    Some(match &args.ssh_key {
        Some(key) => format!("theta: SSH tunnel via {ssh}, key file {key}"),
        None => format!("theta: SSH tunnel via {ssh}, no --ssh-key given: authenticating with ssh-agent"),
    })
}

/// `theta 0.1.0-alpha.6`.
pub fn version_line() -> String {
    format!("theta {}", env!("CARGO_PKG_VERSION"))
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
    fn table_editor_flags() {
        let a = p(&["--connect", "x", "--modify", "public.customers"], false).unwrap();
        assert_eq!(a.modify.as_deref(), Some("public.customers"));
        assert_eq!(p(&["--new-table"], false).unwrap().new_table, Some(None));
        assert_eq!(p(&["--new-table", "sales"], false).unwrap().new_table, Some(Some("sales".into())));
        // A following flag is not taken for the schema.
        let a = p(&["--new-table", "--demo"], false).unwrap();
        assert_eq!(a.new_table, Some(None));
        assert!(a.demo);
        assert!(p(&["--modify"], false).is_err());
        assert!(p(&["--modify", "t", "--new-table"], false).is_err());
        assert!(p(&["--modify", "t", "--run", "select 1"], false).is_err());
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
        assert!(p(&["--stress-catalog=x"], false).is_err());
        assert!(p(&["--run"], false).is_err());
    }

    #[test]
    fn unknown_flags_are_errors_naming_the_flag() {
        let e = p(&["--bogus"], false).unwrap_err();
        assert!(e.contains("--bogus"), "{e}");
        assert!(p(&["--demo", "-x"], false).is_err());
        assert!(p(&["stray"], false).is_err());
    }

    #[test]
    fn help_and_version() {
        assert!(p(&["--help"], false).unwrap().help);
        assert!(p(&["-h"], false).unwrap().help);
        assert!(p(&["--version"], false).unwrap().version);
        assert!(p(&["-V"], false).unwrap().version);
        assert!(version_line().starts_with("theta 0.1.0"));
    }

    #[test]
    fn every_flag_is_in_the_usage_text() {
        for f in FLAGS {
            assert!(USAGE.contains(f), "{f} missing from USAGE");
        }
        // And the other way round: every `--long` flag in the usage text is accepted.
        for word in USAGE.split_whitespace().filter(|w| w.starts_with("--")) {
            let flag = word.trim_end_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'));
            let flag = flag.split(['[', '=']).next().unwrap();
            assert!(FLAGS.contains(&flag), "USAGE mentions {flag}, which the parser does not know");
        }
    }

    #[test]
    fn ssh_without_a_key_uses_the_agent_and_says_so() {
        let a = p(&["--connect", "postgres://h/d", "--ssh", "bob@box:2222"], false).unwrap();
        assert!(ssh_auth_note(&a).unwrap().contains("ssh-agent"));
        let a = p(&["--connect", "postgres://h/d", "--ssh=bob@box", "--ssh-key", "/k"], false).unwrap();
        assert!(ssh_auth_note(&a).unwrap().contains("/k"));
        assert!(ssh_auth_note(&p(&[], false).unwrap()).is_none());
    }

    #[test]
    fn inconsistent_ssh_flags_are_rejected() {
        assert!(p(&["--ssh", "bob@box"], false).is_err(), "nothing to tunnel");
        assert!(p(&["--connect", "x", "--ssh-key", "/k"], false).is_err(), "key without --ssh");
        assert!(p(&["--run", "select 1", "--script", "f.sql"], false).is_err());
    }
}
