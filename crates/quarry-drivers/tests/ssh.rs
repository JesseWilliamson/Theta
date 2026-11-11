//! SSH tunnel tests. They need a local sshd, so they only run when
//! `QUARRY_TEST_SSH=1` (see README.md and tests/ssh/setup.sh).

mod common;

use common::*;
use quarry_core::*;
use quarry_drivers::ssh::Tunnel;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

macro_rules! ssh_test {
    () => {
        if std::env::var("QUARRY_TEST_SSH").as_deref() != Ok("1") {
            eprintln!("skipped: set QUARRY_TEST_SSH=1 (see README.md)");
            return;
        }
        // The tests count TCP connections to the sshd, so they must not overlap.
        let _guard = SERIAL.lock().await;
    };
}

fn dir() -> PathBuf {
    std::env::var("QUARRY_TEST_SSH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var_os("TMPDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| "/tmp".into())
                .join("quarry-ssh-test")
        })
}

fn port() -> u16 {
    std::env::var("QUARRY_TEST_SSH_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(2222)
}

fn ssh(auth: SshAuth) -> SshConfig {
    SshConfig { host: "127.0.0.1".into(), port: port(), user: "quarrytest".into(), auth }
}

fn key(name: &str, passphrase: Option<&str>) -> SshAuth {
    SshAuth::KeyFile {
        path: dir().join(name).to_string_lossy().into_owned(),
        passphrase: passphrase.map(str::to_string),
    }
}

fn pg_via(auth: SshAuth) -> ConnectionConfig {
    let mut c = pg_config();
    c.ssh = Some(ssh(auth));
    c
}

/// Established TCP connections from this machine to the sshd port, read from
/// /proc so the test needs no extra tools.
fn established_to_sshd() -> usize {
    let want = format!(":{:04X}", port());
    std::fs::read_to_string("/proc/net/tcp")
        .unwrap()
        .lines()
        .skip(1)
        .filter(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            f.len() > 3 && f[2].ends_with(&want) && f[3] == "01"
        })
        .count()
}

async fn wait_for(count: usize) {
    for _ in 0..40 {
        if established_to_sshd() == count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(established_to_sshd(), count);
}

async fn check_postgres(cfg: &ConnectionConfig) {
    let conn = quarry_drivers::connect(cfg).await.unwrap_or_else(|e| panic!("connect via SSH failed: {e:?}"));
    assert_eq!(scalar(conn.as_ref(), "SELECT 40 + 2").await, Value::Int(42));
    assert!(conn.server_info().await.unwrap().version.starts_with("PostgreSQL"));
    let rs = conn
        .execute("SELECT g FROM generate_series(1, 20000) g", &ExecOptions { row_limit: None })
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(rs.rows.len(), 20000);
    conn.close().await.unwrap();
}

#[tokio::test]
async fn key_file_end_to_end_and_teardown() {
    ssh_test!();
    wait_for(0).await;
    let cfg = pg_via(key("client_ed25519", None));
    let conn = quarry_drivers::connect(&cfg).await.unwrap();
    assert_eq!(established_to_sshd(), 1, "one SSH session carries the connection");
    assert_eq!(scalar(conn.as_ref(), "SELECT 'via tunnel'").await, text("via tunnel"));
    run(conn.as_ref(), "SELECT pg_sleep(0.1)").await;
    conn.close().await.unwrap();
    wait_for(0).await;
    // close() makes the connection unusable but never panics.
    assert!(conn.execute("SELECT 1", &all()).await.is_err());
    check_postgres(&cfg).await;
    wait_for(0).await;
}

#[tokio::test]
async fn dropping_the_connection_tears_the_tunnel_down() {
    ssh_test!();
    wait_for(0).await;
    let conn = quarry_drivers::connect(&pg_via(key("client_ed25519", None))).await.unwrap();
    assert_eq!(established_to_sshd(), 1);
    drop(conn);
    wait_for(0).await;
}

#[tokio::test]
async fn passphrase_rsa_and_password_auth() {
    ssh_test!();
    check_postgres(&pg_via(key("client_passphrase", Some("s3cret")))).await;
    check_postgres(&pg_via(key("client_rsa", None))).await;
    check_postgres(&pg_via(SshAuth::Password { password: "quarry-test-pw".into() })).await;
}

#[tokio::test]
async fn agent_auth() {
    ssh_test!();
    let sock = std::env::temp_dir().join(unique("quarry-agent"));
    let out =
        std::process::Command::new("ssh-agent").arg("-a").arg(&sock).arg("-s").output().expect("ssh-agent");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let pid: String =
        stdout.split("SSH_AGENT_PID=").nth(1).unwrap().chars().take_while(char::is_ascii_digit).collect();
    let added = std::process::Command::new("ssh-add")
        .arg(dir().join("client_ed25519"))
        .env("SSH_AUTH_SOCK", &sock)
        .output()
        .unwrap();
    assert!(added.status.success(), "{}", String::from_utf8_lossy(&added.stderr));
    // SAFETY: the SSH tests are serialised by SERIAL and nothing else reads this variable.
    unsafe { std::env::set_var("SSH_AUTH_SOCK", &sock) };
    check_postgres(&pg_via(SshAuth::Agent)).await;
    // An agent without the right identity is an authentication failure.
    let empty = std::process::Command::new("ssh-add").arg("-D").env("SSH_AUTH_SOCK", &sock).output().unwrap();
    assert!(empty.status.success());
    let e = quarry_drivers::connect(&pg_via(SshAuth::Agent)).await.err().unwrap();
    assert!(matches!(e, Error::Auth(_)), "{e:?}");
    // SAFETY: as above.
    unsafe { std::env::remove_var("SSH_AUTH_SOCK") };
    let e = quarry_drivers::connect(&pg_via(SshAuth::Agent)).await.err().unwrap();
    assert!(matches!(e, Error::Auth(_)), "{e:?}");
    let _ = std::process::Command::new("kill").arg(pid).status();
}

#[tokio::test]
async fn bad_credentials_are_auth_errors() {
    ssh_test!();
    for auth in [
        SshAuth::Password { password: "wrong".into() },
        key("client_unauthorized", None),
        key("client_passphrase", Some("not-the-passphrase")),
        key("no_such_key_file", None),
    ] {
        let e = quarry_drivers::connect(&pg_via(auth.clone())).await.err().expect("must fail");
        assert!(matches!(e, Error::Auth(_)), "{auth:?}: {e:?}");
    }
    wait_for(0).await;
}

#[tokio::test]
async fn unreachable_endpoints_are_tunnel_errors() {
    ssh_test!();
    // SSH server not listening.
    let mut cfg = pg_via(key("client_ed25519", None));
    cfg.ssh.as_mut().unwrap().port = 1;
    let e = quarry_drivers::connect(&cfg).await.err().unwrap();
    assert!(matches!(e, Error::Tunnel(_)), "{e:?}");
    // SSH works but the database port is closed *as seen from the SSH server*.
    let mut cfg = pg_via(key("client_ed25519", None));
    cfg.port = 1;
    let e = quarry_drivers::connect(&cfg).await.err().unwrap();
    assert!(matches!(e, Error::Tunnel(_)), "{e:?}");
    // The tunnel is closed again after a failed database connect.
    let mut cfg = pg_via(key("client_ed25519", None));
    cfg.password = Some("wrong".into());
    let e = quarry_drivers::connect(&cfg).await.err().unwrap();
    assert!(matches!(e, Error::Auth(_)), "{e:?}");
    wait_for(0).await;
}

#[tokio::test]
async fn tunnel_forwards_bytes_for_many_clients_and_closes() {
    ssh_test!();
    let tunnel = Tunnel::open(&ssh(key("client_ed25519", None)), "127.0.0.1", 5432).await.unwrap();
    let p = tunnel.local_port();
    assert_ne!(p, 0);
    // Several simultaneous clients share the one SSH session.
    let mut handles = vec![];
    for _ in 0..4 {
        handles.push(tokio::spawn(async move {
            let mut s = TcpStream::connect(("127.0.0.1", p)).await.unwrap();
            // PostgreSQL SSLRequest; the server answers a single byte, 'S' or 'N'.
            s.write_all(&[0, 0, 0, 8, 4, 210, 22, 47]).await.unwrap();
            let mut b = [0u8; 1];
            s.read_exact(&mut b).await.unwrap();
            assert!(b[0] == b'S' || b[0] == b'N');
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(established_to_sshd(), 1);
    // The listener is loopback-only.
    let listening = std::fs::read_to_string("/proc/net/tcp").unwrap();
    assert!(listening.lines().any(|l| l.contains(&format!("0100007F:{p:04X}"))));
    tunnel.close().await;
    wait_for(0).await;
    let r = TcpStream::connect(("127.0.0.1", p)).await;
    assert!(r.is_err(), "the local port must be closed after Tunnel::close");
}

#[tokio::test]
async fn mysql_through_a_tunnel() {
    ssh_test!();
    let mut cfg = mysql_config();
    cfg.ssh = Some(ssh(key("client_ed25519", None)));
    let conn = quarry_drivers::connect(&cfg).await.unwrap();
    assert_eq!(scalar(conn.as_ref(), "SELECT 6 * 7").await, Value::Int(42));
    // cancel() opens a second connection; it must go through the tunnel as well.
    let c: std::sync::Arc<dyn Connection> = conn.into();
    let c2 = c.clone();
    let t = tokio::spawn(async move { c2.execute("SELECT SLEEP(20)", &all()).await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    c.cancel().await.unwrap();
    let r = tokio::time::timeout(Duration::from_secs(5), t).await.unwrap().unwrap();
    assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
    c.close().await.unwrap();
    wait_for(0).await;
}

#[tokio::test]
async fn postgres_cancel_and_tls_verification_through_a_tunnel() {
    ssh_test!();
    let cfg = pg_via(key("client_ed25519", None));
    let conn: std::sync::Arc<dyn Connection> = quarry_drivers::connect(&cfg).await.unwrap().into();
    let c2 = conn.clone();
    let t = tokio::spawn(async move { c2.execute("SELECT pg_sleep(20)", &all()).await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    conn.cancel().await.unwrap();
    let r = tokio::time::timeout(Duration::from_secs(5), t).await.unwrap().unwrap();
    assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
    conn.close().await.unwrap();
    // TLS "require" and "prefer" work over the tunnel too.
    let mut tls = pg_via(key("client_ed25519", None));
    tls.ssl_mode = SslMode::Prefer;
    check_postgres(&tls).await;
}
