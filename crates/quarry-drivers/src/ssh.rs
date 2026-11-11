//! SSH tunnelling: a local TCP listener whose accepted connections are carried
//! to `remote_host:remote_port` through an SSH `direct-tcpip` channel.
//!
//! One SSH session is shared by all forwarded connections. The listener binds to
//! 127.0.0.1 only, on an ephemeral port, so nothing outside this machine can use
//! the tunnel.

use quarry_core::{Error, Result, SshAuth, SshConfig};
use russh::client::{self, Handle};
use russh::keys::agent::client::AgentClient;
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate, ssh_key};
use russh::{ChannelMsg, Disconnect};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// Outcome of comparing the key a server offered with `known_hosts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyCheck {
    /// The recorded key matches.
    Known,
    /// First contact: the key was appended to `file` (trust on first use).
    Added { fingerprint: String, file: PathBuf },
    /// A different key is recorded for this host: refused.
    Changed { fingerprint: String, file: PathBuf, line: usize },
    /// known_hosts could not be read or written; the tunnel is allowed (like `StrictHostKeyChecking=no`
    /// would) because refusing would make the feature unusable on a machine without a home directory.
    Unchecked,
}

fn known_hosts_file() -> Option<PathBuf> {
    std::env::home_dir().map(|h| h.join(".ssh").join("known_hosts"))
}

/// The `known_hosts` name of a host: bare for port 22, `[host]:port` otherwise.
pub fn known_hosts_name(host: &str, port: u16) -> String {
    if port == 22 || port == 0 { host.to_string() } else { format!("[{host}]:{port}") }
}

/// Trust-on-first-use check against `file`. Pure apart from appending an unknown key to it.
pub fn check_host_key(host: &str, port: u16, pk: &ssh_key::PublicKey, file: &Path) -> HostKeyCheck {
    let port = if port == 0 { 22 } else { port };
    let fingerprint = pk.fingerprint(ssh_key::HashAlg::Sha256).to_string();
    match russh::keys::known_hosts::check_known_hosts_path(host, port, pk, file) {
        Ok(true) => HostKeyCheck::Known,
        Ok(false) => match russh::keys::known_hosts::learn_known_hosts_path(host, port, pk, file) {
            Ok(()) => HostKeyCheck::Added { fingerprint, file: file.to_path_buf() },
            Err(_) => HostKeyCheck::Unchecked,
        },
        Err(russh::keys::Error::KeyChanged { line }) => {
            HostKeyCheck::Changed { fingerprint, file: file.to_path_buf(), line }
        }
        Err(_) => HostKeyCheck::Unchecked,
    }
}

/// The message for a refused, changed host key: what was seen, where the old key lives and what
/// to do about it.
pub fn changed_key_message(host: &str, port: u16, fingerprint: &str, file: &Path, line: usize) -> String {
    let name = known_hosts_name(host, port);
    format!(
        "The SSH host key for {name} has changed. The server offered {fingerprint}, but {} (line {line}) \
         records a different key. If you expected this (the server was reinstalled or re-keyed), remove \
         the stale entry with: ssh-keygen -R \"{name}\"  and connect again. If not, someone may be \
         intercepting the connection: do not continue.",
        file.display()
    )
}

/// What the handler learned during the handshake, read back by `Tunnel::open`.
type Report = Arc<StdMutex<Option<HostKeyCheck>>>;

struct Handler {
    host: String,
    port: u16,
    report: Report,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    /// Trust-on-first-use against `~/.ssh/known_hosts`: a host we have never seen
    /// is accepted (and remembered), but a host whose key *changed* is refused,
    /// which is the case that signals a man-in-the-middle.
    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        let pk: &ssh_key::PublicKey = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key,
            // Certificates are validated against a CA we do not manage here.
            PublicKeyOrCertificate::Certificate(_) => return Ok(true),
        };
        let result = match known_hosts_file() {
            Some(file) => check_host_key(&self.host, self.port, pk, &file),
            None => HostKeyCheck::Unchecked,
        };
        let allow = !matches!(result, HostKeyCheck::Changed { .. });
        if let Ok(mut slot) = self.report.lock() {
            *slot = Some(result);
        }
        Ok(allow)
    }
}

pub struct Tunnel {
    notices: Vec<String>,
    local_port: u16,
    session: Arc<Handle<Handler>>,
    accept_task: JoinHandle<()>,
}

fn tunnel_err(e: impl std::fmt::Display) -> Error {
    Error::Tunnel(e.to_string())
}

async fn authenticate(session: &mut Handle<Handler>, ssh: &SshConfig) -> Result<()> {
    let user = ssh.user.clone();
    let result = match &ssh.auth {
        SshAuth::Password { password } => {
            session.authenticate_password(user, password.clone()).await.map_err(tunnel_err)?
        }
        SshAuth::KeyFile { path, passphrase } => {
            let expanded = expand_home(path);
            let key = russh::keys::load_secret_key(&expanded, passphrase.as_deref())
                .map_err(|e| Error::Auth(format!("cannot load SSH key {path}: {e}")))?;
            let hash = session.best_supported_rsa_hash().await.map_err(tunnel_err)?.flatten();
            session
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
                .map_err(tunnel_err)?
        }
        SshAuth::Agent => {
            // The agent speaks the same protocol everywhere; only the transport
            // differs: a Unix socket named by SSH_AUTH_SOCK, or on Windows the
            // OpenSSH agent service's well-known named pipe.
            #[cfg(unix)]
            let agent = AgentClient::connect_env()
                .await
                .map_err(|e| Error::Auth(format!("cannot reach ssh-agent (is SSH_AUTH_SOCK set?): {e}")))?;
            #[cfg(windows)]
            let agent =
                AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await.map_err(|e| {
                    Error::Auth(format!(
                        "cannot reach the OpenSSH agent service (is ssh-agent running?): {e}"
                    ))
                })?;
            authenticate_with_agent(session, user, agent).await?
        }
    };
    if result.success() { Ok(()) } else { Err(Error::Auth("SSH server rejected the credentials".into())) }
}

async fn authenticate_with_agent<S>(
    session: &mut Handle<Handler>,
    user: String,
    mut agent: AgentClient<S>,
) -> Result<russh::client::AuthResult>
where
    S: russh::keys::agent::client::AgentStream + Send + Unpin + 'static,
{
    let identities = agent.request_identities().await.map_err(tunnel_err)?;
    if identities.is_empty() {
        return Err(Error::Auth("ssh-agent holds no identities".into()));
    }
    let hash = session.best_supported_rsa_hash().await.map_err(tunnel_err)?.flatten();
    for id in identities {
        let russh::keys::agent::AgentIdentity::PublicKey { key, .. } = id else { continue };
        let r = session
            .authenticate_publickey_with(user.clone(), key, hash, &mut agent)
            .await
            .map_err(tunnel_err)?;
        if r.success() {
            return Ok(r);
        }
    }
    Err(Error::Auth("none of the ssh-agent identities were accepted".into()))
}

fn expand_home(path: &str) -> String {
    match path.strip_prefix("~/") {
        // USERPROFILE is the Windows spelling of HOME.
        Some(rest) => match std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
            Ok(h) => format!("{h}/{rest}"),
            Err(_) => path.to_string(),
        },
        None => path.to_string(),
    }
}

impl Tunnel {
    pub async fn open(ssh: &SshConfig, remote_host: &str, remote_port: u16) -> Result<Tunnel> {
        let config = Arc::new(client::Config {
            nodelay: true,
            inactivity_timeout: None,
            keepalive_interval: Some(Duration::from_secs(30)),
            ..Default::default()
        });
        let report: Report = Arc::default();
        let handler = Handler { host: ssh.host.clone(), port: ssh.port, report: report.clone() };
        let addr = (ssh.host.as_str(), if ssh.port == 0 { 22 } else { ssh.port });
        let connected = tokio::time::timeout(Duration::from_secs(15), client::connect(config, addr, handler))
            .await
            .map_err(|_| Error::Tunnel(format!("timed out connecting to {}:{}", ssh.host, ssh.port)))?;
        let seen = report.lock().ok().and_then(|r| r.clone());
        let mut session = match connected {
            Ok(session) => session,
            Err(_) if matches!(seen, Some(HostKeyCheck::Changed { .. })) => {
                let Some(HostKeyCheck::Changed { fingerprint, file, line }) = seen else { unreachable!() };
                return Err(Error::Tunnel(changed_key_message(
                    &ssh.host,
                    ssh.port,
                    &fingerprint,
                    &file,
                    line,
                )));
            }
            Err(e) => {
                return Err(Error::Tunnel(format!("cannot connect to {}:{}: {e}", ssh.host, ssh.port)));
            }
        };
        let mut notices = Vec::new();
        if let Some(HostKeyCheck::Added { fingerprint, file }) = seen {
            notices.push(format!(
                "Added host key for {} ({fingerprint}) to {}",
                known_hosts_name(&ssh.host, ssh.port),
                file.display()
            ));
        }
        authenticate(&mut session, ssh).await?;

        // Prove the remote endpoint is reachable now, so a wrong host/port is
        // reported when connecting instead of as a mysterious DB timeout later.
        let probe = session
            .channel_open_direct_tcpip(remote_host.to_string(), remote_port as u32, "127.0.0.1", 0)
            .await
            .map_err(|e| {
                Error::Tunnel(format!("SSH server cannot reach {remote_host}:{remote_port}: {e}"))
            })?;
        drop(probe);

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(tunnel_err)?;
        let local_port = listener.local_addr().map_err(tunnel_err)?.port();
        let session = Arc::new(session);
        let accept_session = session.clone();
        let host = remote_host.to_string();
        let accept_task = tokio::spawn(async move {
            loop {
                let Ok((stream, peer)) = listener.accept().await else { break };
                let session = accept_session.clone();
                let host = host.clone();
                tokio::spawn(async move {
                    let _ = forward(session, stream, peer, &host, remote_port).await;
                });
            }
        });
        Ok(Tunnel { notices, local_port, session, accept_task })
    }

    /// Things worth telling the user once, e.g. that a new host key was added to known_hosts.
    pub fn notices(&self) -> &[String] {
        &self.notices
    }

    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    pub async fn close(self) {
        self.accept_task.abort();
        let _ = self.session.disconnect(Disconnect::ByApplication, "", "en").await;
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        // Stops accepting; in-flight forwards end when the SSH session is dropped.
        self.accept_task.abort();
    }
}

async fn forward(
    session: Arc<Handle<Handler>>,
    mut stream: TcpStream,
    peer: SocketAddr,
    host: &str,
    port: u16,
) -> std::result::Result<(), russh::Error> {
    let _ = stream.set_nodelay(true);
    let mut channel = session
        .channel_open_direct_tcpip(host.to_string(), port as u32, peer.ip().to_string(), peer.port() as u32)
        .await?;
    let (mut rd, mut wr) = stream.split();
    let mut buf = vec![0u8; 32 * 1024];
    let mut local_open = true;
    loop {
        tokio::select! {
            n = tokio::io::AsyncReadExt::read(&mut rd, &mut buf), if local_open => {
                match n {
                    Ok(0) | Err(_) => { local_open = false; channel.eof().await?; }
                    Ok(n) => channel.data(&buf[..n]).await?,
                }
            }
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        if tokio::io::AsyncWriteExt::write_all(&mut wr, &data).await.is_err() { break; }
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                    _ => {}
                }
            }
        }
    }
    let _ = tokio::io::AsyncWriteExt::shutdown(&mut wr).await;
    let _ = channel.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(which: u8) -> ssh_key::PublicKey {
        let line = match which {
            1 => "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIO2MA82KOmGvKezpIYN3YI/iUdPv3DfnVbQqwXkBO/JZ",
            _ => "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFhVQStMpBTWFwWs9wx9NUxjKQlSUbE4d4BNyrv5TZk8",
        };
        ssh_key::PublicKey::from_openssh(line).unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quarry-ssh-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("known_hosts")
    }

    #[test]
    fn first_contact_is_remembered_then_known_then_changed_is_refused() {
        let file = scratch("tofu");
        let a = key(1);
        let HostKeyCheck::Added { fingerprint, .. } = check_host_key("db.example", 2222, &a, &file) else {
            panic!("expected Added");
        };
        assert!(fingerprint.starts_with("SHA256:"));
        assert_eq!(check_host_key("db.example", 2222, &a, &file), HostKeyCheck::Known);
        match check_host_key("db.example", 2222, &key(2), &file) {
            HostKeyCheck::Changed { line, file: f, fingerprint } => {
                let text = std::fs::read_to_string(&file).unwrap();
                assert!(
                    text.lines().nth(line - 1).is_some_and(|l| l.starts_with("[db.example]:2222")),
                    "{line}"
                );
                assert_eq!(f, file);
                assert!(fingerprint.starts_with("SHA256:"));
            }
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    #[test]
    fn changed_key_message_names_host_fingerprint_file_line_and_remedy() {
        let m = changed_key_message("db.example", 2222, "SHA256:abc", Path::new("/h/.ssh/known_hosts"), 7);
        for needle in [
            "[db.example]:2222",
            "SHA256:abc",
            "/h/.ssh/known_hosts",
            "line 7",
            "ssh-keygen -R \"[db.example]:2222\"",
        ] {
            assert!(m.contains(needle), "{needle} in {m}");
        }
        assert!(changed_key_message("h", 22, "x", Path::new("f"), 1).contains("ssh-keygen -R \"h\""));
    }
}
