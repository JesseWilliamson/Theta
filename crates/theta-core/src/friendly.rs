//! Plain sentences for failures: the one place that turns operating-system and driver error texts
//! ("Connection refused (os error 111)", "password authentication failed for user …") into what the
//! user needs to know and, where there is an obvious next step, what to do about it.
//!
//! The original text is never thrown away: [`Explained::details`] carries it for tooltips, the
//! Messages tab and the log, so nothing is lost for debugging.

use crate::{ConnectionConfig, Error};
use std::io::ErrorKind;
use std::path::Path;

/// A failure in words for the user, plus the original text when it differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explained {
    pub message: String,
    pub details: Option<String>,
}

impl Explained {
    fn new(message: String, raw: &str) -> Self {
        let details = (raw != message && !raw.is_empty()).then(|| raw.to_string());
        Explained { message, details }
    }
}

/// What was being done to a file; decides whether a missing path is "the file" or "the folder".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoOp {
    Read,
    Write,
}

/// A failed file operation on `path`, as a sentence. `e` is whatever `std::io` returned.
pub fn io_error(e: &std::io::Error, path: &Path, op: IoOp) -> Explained {
    let raw = e.to_string();
    let p = path.display();
    let parent_missing = path.parent().is_some_and(|d| !d.as_os_str().is_empty() && !d.exists());
    let message = match e.kind() {
        ErrorKind::NotFound if op == IoOp::Write && parent_missing => {
            format!(
                "The folder {} does not exist.",
                path.parent().map(|d| d.display().to_string()).unwrap_or_default()
            )
        }
        ErrorKind::NotFound => format!("The file {p} does not exist."),
        ErrorKind::PermissionDenied => match op {
            IoOp::Read => format!("You do not have permission to read {p}."),
            IoOp::Write => format!("You do not have permission to write to {p}."),
        },
        ErrorKind::AlreadyExists => format!("{p} already exists."),
        ErrorKind::IsADirectory => format!("{p} is a folder, not a file."),
        ErrorKind::NotADirectory => format!("Part of the path {p} is not a folder."),
        ErrorKind::StorageFull => format!("The disk is full, so {p} could not be written."),
        ErrorKind::ReadOnlyFilesystem => format!("The disk holding {p} is read-only."),
        ErrorKind::QuotaExceeded => format!("The disk quota is used up, so {p} could not be written."),
        ErrorKind::TimedOut => format!("The disk or share holding {p} did not answer in time."),
        ErrorKind::InvalidFilename => format!("{p} is not a valid file name."),
        ErrorKind::InvalidData => format!("{p} could not be read as text in the expected format."),
        ErrorKind::UnexpectedEof => format!("{p} ended unexpectedly."),
        _ => {
            let verb = if op == IoOp::Read { "read" } else { "write" };
            format!("Could not {verb} {p}: {}", sentence(&scrub(&raw)))
        }
    };
    Explained::new(message, &raw)
}

/// [`io_error`] for callers that only need the sentence.
pub fn io_message(e: &std::io::Error, path: &Path, op: IoOp) -> String {
    io_error(e, path, op).message
}

/// Remove operating-system noise from free text: the ` (os error N)` suffix and the doubled
/// "Input/output error:" prefix some drivers add.
pub fn scrub(text: &str) -> String {
    let mut s = text.trim().to_string();
    while let Some(rest) = s.strip_prefix("Input/output error: ") {
        s = rest.to_string();
    }
    // " (os error 111)" at the end or in the middle.
    while let Some(i) = s.find(" (os error ") {
        match s[i..].find(')') {
            Some(j) => s.replace_range(i..i + j + 1, ""),
            None => break,
        }
    }
    s
}

/// Capital first letter and a final full stop, for texts the drivers give in lower case.
fn sentence(text: &str) -> String {
    let t = text.trim().trim_end_matches('.');
    let mut c = t.chars();
    let mut out = match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => return String::new(),
    };
    out.push('.');
    out
}

/// Where a connection points, for messages: `host:port`, or the file for a file database.
fn endpoint(cfg: Option<&ConnectionConfig>, fallback: &str) -> String {
    match cfg {
        Some(c) if c.kind.is_file_based() => c.path.clone(),
        Some(c) if c.port != 0 && !c.host.is_empty() => format!("{}:{}", c.host, c.port),
        Some(c) if !c.host.is_empty() => c.host.clone(),
        _ => fallback.to_string(),
    }
}

fn name_between(text: &str, open: char, close: char) -> Option<String> {
    let a = text.find(open)? + open.len_utf8();
    let b = text[a..].find(close)? + a;
    Some(text[a..b].to_string())
}

/// Why a connection (or an SSH tunnel) failed, in words. `cfg` supplies the host, port and user the
/// driver text leaves out. The result never starts with "could not connect": every place that shows
/// it already says what was being connected.
pub fn connect_error(e: &Error, cfg: Option<&ConnectionConfig>) -> Explained {
    let raw = e.to_string();
    let text = match e {
        Error::Connect(t) | Error::Auth(t) | Error::Tunnel(t) | Error::Other(t) => scrub(t),
        other => scrub(&other.to_string()),
    };
    let lower = text.to_ascii_lowercase();
    let at = endpoint(cfg, "the server");
    let host = cfg.map(|c| c.host.as_str()).filter(|h| !h.is_empty()).unwrap_or("the server");
    let user = cfg.map(|c| c.user.as_str()).filter(|u| !u.is_empty());
    let user_text = |from_driver: Option<String>| -> String {
        match (user, from_driver) {
            (Some(u), _) => u.to_string(),
            (None, Some(d)) => d,
            (None, None) => "this user".to_string(),
        }
    };
    let tunnel = matches!(e, Error::Tunnel(_));
    let ssh_host = cfg
        .and_then(|c| c.ssh.as_ref())
        .map(|s| format!("{}:{}", s.host, s.port))
        .unwrap_or_else(|| "the SSH server".into());
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));

    let message = if tunnel {
        tunnel_message(&text, &lower, &ssh_host)
    } else if matches!(e, Error::Auth(_)) && has(&["ssh", "ssh-agent"]) {
        sentence(&text)
    } else if has(&[
        "password authentication failed",
        "access denied for user",
        "authentication failed",
        "login failed",
    ]) || matches!(e, Error::Auth(_)) && has(&["password"])
    {
        let driver_user = name_between(&text, '"', '"').or_else(|| name_between(&text, '\'', '\''));
        format!("The server rejected the password for user {}.", user_text(driver_user))
    } else if matches!(e, Error::Auth(_)) && has(&["asked for a password"]) {
        "The server asks for a password, but none is saved for this connection.".to_string()
    } else if has(&["connection refused"]) {
        format!("Nothing is listening on {at}. Is the server running, and is the port right?")
    } else if has(&[
        "failed to lookup address",
        "name or service not known",
        "nodename nor servname",
        "no such host",
        "temporary failure in name resolution",
        "no address associated",
        "dns error",
    ]) {
        format!("The host {host} could not be found. Check the name or your VPN.")
    } else if has(&["network is unreachable", "no route to host", "host is unreachable"]) {
        format!("{at} cannot be reached from this computer. Check your network or VPN.")
    } else if has(&["timed out", "timeout", "deadline has elapsed"]) {
        format!("The server at {at} did not answer in time. Check the address, the firewall or your VPN.")
    } else if has(&["connection reset", "broken pipe", "unexpected eof", "closed the connection"]) {
        format!("The server at {at} closed the connection while connecting. Check the port and the SSL mode.")
    } else if has(&["unknown issuer", "self signed", "self-signed", "unknownissuer"]) {
        format!(
            "The certificate of {at} is not trusted. Use another SSL mode, or add the signing certificate to this computer."
        )
    } else if has(&["certificate", "tls", "ssl", "handshake"]) {
        format!(
            "The secure connection to {at} could not be set up ({}). Check the SSL mode of the connection.",
            text.trim_end_matches('.')
        )
    } else if let Some(db) = missing_database(&text, &lower) {
        format!(
            "There is no database named {db} on {at}. Check the name, or leave it empty for the server's default."
        )
    } else if has(&["unable to open database file"]) {
        sqlite_open_message(&text, cfg)
    } else if has(&["file is not a database", "not a database"]) {
        let path = cfg.map(|c| c.path.as_str()).unwrap_or("the file");
        format!("{path} is not a database file of this kind.")
    } else if has(&["the folder "]) {
        sentence(&text)
    } else {
        sentence(text.strip_prefix("error connecting to server: ").unwrap_or(&text))
    };
    Explained::new(message, &raw)
}

fn missing_database(text: &str, lower: &str) -> Option<String> {
    if lower.starts_with("unknown database") {
        return name_between(text, '\'', '\'');
    }
    if lower.contains("database \"") && lower.contains("does not exist") {
        return name_between(text, '"', '"');
    }
    if lower.contains("cannot open database") {
        return name_between(text, '"', '"');
    }
    None
}

fn sqlite_open_message(text: &str, cfg: Option<&ConnectionConfig>) -> String {
    let path = cfg
        .map(|c| c.path.clone())
        .filter(|p| !p.is_empty())
        .or_else(|| text.split_once(": ").map(|(_, p)| p.trim().to_string()));
    let Some(path) = path else {
        return "The database file could not be opened.".into();
    };
    let p = Path::new(&path);
    if p.is_dir() {
        format!("{path} is a folder, not a database file.")
    } else if p.parent().is_some_and(|d| !d.as_os_str().is_empty() && !d.exists()) {
        format!(
            "The folder {} does not exist.",
            p.parent().map(|d| d.display().to_string()).unwrap_or_default()
        )
    } else if p.exists() {
        format!(
            "{path} could not be opened. Check that you may read and write it, and that no other program has locked it."
        )
    } else {
        format!("The file {path} could not be created. Check that you may write to its folder.")
    }
}

fn tunnel_message(text: &str, lower: &str, ssh_host: &str) -> String {
    if lower.starts_with("timed out connecting to") {
        return format!(
            "The SSH server {ssh_host} did not answer in time. Check the address, the firewall or your VPN."
        );
    }
    if let Some(rest) = text.strip_prefix("cannot connect to ") {
        // "host:port: <io error>"
        let reason = rest.splitn(3, ':').nth(2).map(str::trim).unwrap_or("");
        let l = reason.to_ascii_lowercase();
        return if l.contains("connection refused") {
            format!(
                "Nothing is listening for SSH on {ssh_host}. Is the SSH server running, and is the port right?"
            )
        } else if l.contains("lookup") || l.contains("not known") || l.contains("no such host") {
            format!("The SSH host {ssh_host} could not be found. Check the name or your VPN.")
        } else {
            format!("The SSH server {ssh_host} could not be reached: {}", sentence(reason))
        };
    }
    if let Some(rest) = text.strip_prefix("SSH server cannot reach ") {
        let target = rest.split(": ").next().unwrap_or(rest);
        return format!(
            "The SSH server {ssh_host} could not reach the database at {target}. Check the host and port as the SSH server sees them."
        );
    }
    sentence(&format!("SSH tunnel: {text}")).replace("SSH tunnel: ", "The SSH tunnel failed: ")
}

/// Any other error text a dialog shows (Import, Export, script runs): operating-system noise
/// removed, first letter capitalised. Use [`io_error`] when the path is known.
pub fn plain(text: &str) -> String {
    sentence(&scrub(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DatabaseKind;
    use std::io;

    fn pg(port: u16, host: &str) -> ConnectionConfig {
        let mut c = ConnectionConfig::new("i", "n", DatabaseKind::Postgres);
        c.host = host.into();
        c.port = port;
        c.user = "postgres".into();
        c
    }

    fn msg(e: Error, c: &ConnectionConfig) -> String {
        connect_error(&e, Some(c)).message
    }

    #[test]
    fn postgres_driver_texts() {
        let c = pg(5999, "127.0.0.1");
        assert_eq!(
            msg(Error::Connect("error connecting to server: Connection refused (os error 111)".into()), &c),
            "Nothing is listening on 127.0.0.1:5999. Is the server running, and is the port right?"
        );
        let c = pg(5432, "db.internal");
        assert_eq!(
            msg(
                Error::Connect(
                    "error connecting to server: failed to lookup address information: Name or service not known"
                        .into()
                ),
                &c
            ),
            "The host db.internal could not be found. Check the name or your VPN."
        );
        assert_eq!(
            msg(Error::Auth("password authentication failed for user \"postgres\"".into()), &c),
            "The server rejected the password for user postgres."
        );
        assert_eq!(
            msg(Error::Connect("database \"nodb_x\" does not exist".into()), &c),
            "There is no database named nodb_x on db.internal:5432. Check the name, or leave it empty for the server's default."
        );
    }

    #[test]
    fn mysql_driver_texts() {
        let mut c = pg(3306, "127.0.0.1");
        c.kind = DatabaseKind::MySql;
        c.user = "quarry".into();
        assert_eq!(
            msg(
                Error::Connect(
                    "Input/output error: Input/output error: Connection refused (os error 111)".into()
                ),
                &c
            ),
            "Nothing is listening on 127.0.0.1:3306. Is the server running, and is the port right?"
        );
        assert_eq!(
            msg(Error::Auth("Access denied for user 'quarry'@'localhost' (using password: YES)".into()), &c),
            "The server rejected the password for user quarry."
        );
        assert!(
            msg(Error::Connect("Unknown database 'nodb_x'".into()), &c)
                .starts_with("There is no database named nodb_x on")
        );
    }

    #[test]
    fn sqlite_driver_texts() {
        // The message depends on what is really on disk, so the paths are built from the
        // host's temporary folder rather than spelled the Unix way.
        let tmp = std::env::temp_dir();
        let missing = tmp.join("theta-no-such-folder-7f3a");
        let file = missing.join("x.db").display().to_string();
        let mut c = ConnectionConfig::new("i", "n", DatabaseKind::Sqlite);
        c.path = file.clone();
        assert_eq!(
            msg(Error::Connect(format!("unable to open database file: {file}")), &c),
            format!("The folder {} does not exist.", missing.display())
        );
        let dir = tmp.display().to_string();
        c.path = dir.clone();
        assert_eq!(
            msg(Error::Connect(format!("unable to open database file: {dir}")), &c),
            format!("{dir} is a folder, not a database file.")
        );
    }

    #[test]
    fn transport_and_tls_failures() {
        let c = pg(5432, "db.internal");
        assert!(
            msg(Error::Connect("timed out connecting to db.internal:5432".into()), &c)
                .contains("did not answer in time")
        );
        assert!(
            msg(Error::Connect("Network is unreachable (os error 101)".into()), &c)
                .contains("cannot be reached")
        );
        assert!(
            msg(Error::Connect("invalid peer certificate: UnknownIssuer".into()), &c).contains("not trusted")
        );
        assert!(msg(Error::Connect("TLS setup failed: bad".into()), &c).contains("secure connection"));
        let mut s = pg(5432, "db.internal");
        s.ssh = Some(crate::SshConfig {
            host: "bastion".into(),
            port: 22,
            user: "u".into(),
            auth: crate::SshAuth::Password { password: String::new() },
        });
        assert_eq!(
            msg(Error::Tunnel("timed out connecting to bastion:22".into()), &s),
            "The SSH server bastion:22 did not answer in time. Check the address, the firewall or your VPN."
        );
        assert!(
            msg(Error::Tunnel("cannot connect to bastion:22: Connection refused (os error 111)".into()), &s)
                .starts_with("Nothing is listening for SSH on bastion:22")
        );
    }

    #[test]
    fn the_original_text_is_kept() {
        let c = pg(5999, "127.0.0.1");
        let x = connect_error(&Error::Connect("Connection refused (os error 111)".into()), Some(&c));
        assert_eq!(x.details.as_deref(), Some("could not connect: Connection refused (os error 111)"));
        assert!(!x.message.contains("os error"));
    }

    #[test]
    fn io_kinds() {
        let p = Path::new("/x/y.csv");
        let e = |k| io::Error::from(k);
        assert_eq!(io_message(&e(ErrorKind::NotFound), p, IoOp::Read), "The file /x/y.csv does not exist.");
        assert_eq!(
            io_message(&e(ErrorKind::NotFound), Path::new("/nonexistent/dir/x.csv"), IoOp::Write),
            "The folder /nonexistent/dir does not exist."
        );
        assert_eq!(
            io_message(&e(ErrorKind::PermissionDenied), p, IoOp::Read),
            "You do not have permission to read /x/y.csv."
        );
        assert_eq!(
            io_message(&e(ErrorKind::PermissionDenied), p, IoOp::Write),
            "You do not have permission to write to /x/y.csv."
        );
        assert_eq!(
            io_message(&e(ErrorKind::IsADirectory), p, IoOp::Read),
            "/x/y.csv is a folder, not a file."
        );
        assert!(io_message(&e(ErrorKind::StorageFull), p, IoOp::Write).contains("disk is full"));
        let real = io::Error::from_raw_os_error(2);
        let x = io_error(&real, p, IoOp::Read);
        assert!(x.details.unwrap().contains("os error 2"));
        let other = io::Error::other("something odd (os error 5)");
        assert_eq!(io_message(&other, p, IoOp::Read), "Could not read /x/y.csv: Something odd.");
    }

    #[test]
    fn scrub_removes_os_noise() {
        assert_eq!(scrub("No such file or directory (os error 2)"), "No such file or directory");
        assert_eq!(plain("broken pipe (os error 32)"), "Broken pipe.");
    }
}
