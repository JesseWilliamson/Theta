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
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

struct Handler {
    host: String,
    port: u16,
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
        match russh::keys::check_known_hosts(&self.host, self.port, pk) {
            Ok(true) => Ok(true),
            // Not present in known_hosts (or no such file): remember it.
            Ok(false) => {
                let _ = russh::keys::known_hosts::learn_known_hosts(&self.host, self.port, pk);
                Ok(true)
            }
            // A different key is recorded for this host.
            Err(russh::keys::Error::KeyChanged { .. }) => Ok(false),
            // known_hosts unreadable/unparseable: do not make the tunnel unusable.
            Err(_) => Ok(true),
        }
    }
}

pub struct Tunnel {
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
            let agent = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await.map_err(|e| {
                Error::Auth(format!("cannot reach the OpenSSH agent service (is ssh-agent running?): {e}"))
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
        let handler = Handler { host: ssh.host.clone(), port: ssh.port };
        let addr = (ssh.host.as_str(), if ssh.port == 0 { 22 } else { ssh.port });
        let mut session =
            tokio::time::timeout(Duration::from_secs(15), client::connect(config, addr, handler))
                .await
                .map_err(|_| Error::Tunnel(format!("timed out connecting to {}:{}", ssh.host, ssh.port)))?
                .map_err(|e| Error::Tunnel(format!("cannot connect to {}:{}: {e}", ssh.host, ssh.port)))?;
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
        Ok(Tunnel { local_port, session, accept_task })
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
