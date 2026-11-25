//! Saved SSH configurations: named tunnels that connections refer to by id.
//!
//! A configuration is stored in `<config_dir>/ssh_configs.json` without secrets; its password or key
//! passphrase lives in the keychain under its own key (`ssh:<id>`, see [`secret_key`]), in the same
//! [`SecretStore`] connections use. A connection holds only the id (`ConnectionConfig::ssh_profile`).
//! Just before connecting, [`resolve`] turns that id into the plain `theta_core::SshConfig` the driver
//! layer takes, so `theta-drivers` knows nothing about saved configurations.
//!
//! Older files kept the tunnel inline in each connection. [`migrate_inline`] turns those into
//! configurations at load: identical tunnels become one shared configuration.

use super::secrets::{Secret, SecretStore};
use serde::{Deserialize, Serialize};
use theta_core::{ConnectionConfig, Error, SshAuth, SshConfig};

/// A saved SSH configuration. Called a "profile" in code to keep it apart from the driver's
/// `SshConfig`; the user sees "SSH configuration".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshProfile {
    /// Stable unique id (UUID text). Never shown.
    pub id: String,
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
    /// Seconds between keep-alive packets; 0 turns them off.
    #[serde(default = "default_keepalive")]
    pub keepalive_secs: u32,
    /// The id of another configuration this one is reached through (a jump host).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

fn default_port() -> u16 {
    22
}

fn default_keepalive() -> u32 {
    SshConfig::DEFAULT_KEEPALIVE_SECS
}

/// The file `ssh_configs.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StoredSsh {
    #[serde(default)]
    pub configs: Vec<SshProfile>,
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The keychain key of a configuration's secrets. Connection ids are bare UUIDs, so the prefix keeps
/// the two apart in one store.
pub fn secret_key(id: &str) -> String {
    format!("ssh:{id}")
}

impl SshProfile {
    pub fn new(name: impl Into<String>, host: impl Into<String>, user: impl Into<String>, auth: SshAuth) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            host: host.into(),
            port: 22,
            user: user.into(),
            auth,
            keepalive_secs: default_keepalive(),
            via: None,
        }
    }

    /// `user@host:port`, the line that identifies a server in lists.
    pub fn address(&self) -> String {
        format!("{}@{}:{}", self.user, self.host, self.port)
    }

    /// "Key file", "Password" or "SSH agent".
    pub fn auth_label(&self) -> &'static str {
        auth_label(&self.auth)
    }

    /// The configuration as it is written to disk: secrets removed.
    pub fn stripped(&self) -> SshProfile {
        let mut p = self.clone();
        match &mut p.auth {
            SshAuth::Password { password } => password.clear(),
            SshAuth::KeyFile { passphrase, .. } => *passphrase = None,
            SshAuth::Agent => {}
        }
        p
    }

    /// The secrets this configuration carries, for the keychain.
    pub fn secret(&self) -> Secret {
        let mut s = Secret::default();
        match &self.auth {
            SshAuth::Password { password } if !password.is_empty() => s.ssh_password = Some(password.clone()),
            SshAuth::KeyFile { passphrase: Some(p), .. } if !p.is_empty() => s.ssh_passphrase = Some(p.clone()),
            _ => {}
        }
        s
    }

    /// Fill the secrets back in from the keychain.
    pub fn with_secret(mut self, secret: &Secret) -> SshProfile {
        match &mut self.auth {
            SshAuth::Password { password } => {
                if let Some(p) = &secret.ssh_password {
                    *password = p.clone();
                }
            }
            SshAuth::KeyFile { passphrase, .. } => {
                if secret.ssh_passphrase.is_some() {
                    *passphrase = secret.ssh_passphrase.clone();
                }
            }
            SshAuth::Agent => {}
        }
        self
    }
}

pub fn auth_label(auth: &SshAuth) -> &'static str {
    match auth {
        SshAuth::KeyFile { .. } => "Key file",
        SshAuth::Password { .. } => "Password",
        SshAuth::Agent => "SSH agent",
    }
}

/// The configurations `id` reaches through, nearest first, ending with `id` itself; `Err` with the
/// reason when a reference is missing or the chain loops.
fn chain<'a>(profiles: &'a [SshProfile], id: &str) -> Result<Vec<&'a SshProfile>, String> {
    let mut out: Vec<&SshProfile> = Vec::new();
    let mut next = Some(id.to_string());
    while let Some(id) = next {
        let p = profiles
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| "The SSH configuration this connection uses no longer exists.".to_string())?;
        if out.iter().any(|q| q.id == p.id) {
            return Err(format!("The SSH jump hosts of “{}” form a loop.", p.name));
        }
        out.push(p);
        next = p.via.clone();
    }
    Ok(out)
}

/// Whether `id` is `target` or reaches it through jump hosts: the connections that depend on a
/// configuration include those that use one that jumps through it.
pub fn depends_on(profiles: &[SshProfile], id: &str, target: &str) -> bool {
    chain(profiles, id).is_ok_and(|c| c.iter().any(|p| p.id == target))
}

/// The driver's `SshConfig` for configuration `id`, with secrets from `secrets` and the jump hosts
/// nested in `via`.
pub fn resolve(profiles: &[SshProfile], secrets: &SecretStore, id: &str) -> Result<SshConfig, Error> {
    let links = chain(profiles, id).map_err(Error::Other)?;
    // `links` runs from the target outwards to the first hop; build from the outermost inwards.
    let mut via: Option<Box<SshConfig>> = None;
    for p in links.into_iter().rev() {
        let p = p.clone().with_secret(&secrets.load(&secret_key(&p.id)));
        via = Some(Box::new(SshConfig {
            host: p.host,
            port: p.port,
            user: p.user,
            auth: p.auth,
            keepalive_secs: p.keepalive_secs,
            via: via.take(),
        }));
    }
    via.map(|b| *b).ok_or_else(|| Error::Other("no SSH configuration".into()))
}

/// The connections a configuration would affect if it changed or went away: those that name it, or
/// a configuration that jumps through it. Names, in list order.
pub fn users_of<'a>(profiles: &[SshProfile], connections: &'a [ConnectionConfig], id: &str) -> Vec<&'a ConnectionConfig> {
    connections
        .iter()
        .filter(|c| c.ssh_profile.as_deref().is_some_and(|p| depends_on(profiles, p, id)))
        .collect()
}

/// A name for a new configuration that no other one has.
pub fn unique_name(profiles: &[SshProfile], wanted: &str) -> String {
    let taken = |n: &str| profiles.iter().any(|p| p.name.eq_ignore_ascii_case(n));
    if !taken(wanted) {
        return wanted.to_string();
    }
    (2..).map(|n| format!("{wanted} ({n})")).find(|n| !taken(n)).expect("an unbounded range")
}

/// What decides whether two inline tunnels are the same configuration: the server, the user and
/// the way it authenticates, secrets included (two passwords for one account are two configurations).
fn same_tunnel(a: &SshConfig, b: &SshConfig) -> bool {
    (&a.host, a.port, &a.user, &a.auth) == (&b.host, b.port, &b.user, &b.auth)
}

/// Turn the inline `ssh` of every connection into a reference to a saved configuration. Connections
/// with the same tunnel share one configuration (an existing one is reused when it matches). Secrets
/// move from the connection's keychain entry to the configuration's. Returns whether anything changed.
pub fn migrate_inline(
    connections: &mut [ConnectionConfig],
    profiles: &mut Vec<SshProfile>,
    secrets: &SecretStore,
) -> bool {
    let mut changed = false;
    for c in connections.iter_mut() {
        let Some(inline) = c.ssh.take() else { continue };
        changed = true;
        // The inline form on disk has no secrets; they are in the connection's keychain entry.
        let mut conn_secret = secrets.load(&c.id);
        let full = {
            let mut probe = ConnectionConfig::new("", "", c.kind);
            probe.ssh = Some(inline.clone());
            conn_secret.apply_to(&mut probe);
            probe.ssh.expect("just set")
        };
        let existing = profiles.iter().find(|p| {
            let have = SshProfile::clone(p).with_secret(&secrets.load(&secret_key(&p.id)));
            p.via.is_none()
                && p.keepalive_secs == full.keepalive_secs
                && same_tunnel(
                    &SshConfig::new(have.host, have.port, have.user, have.auth),
                    &SshConfig::new(full.host.clone(), full.port, full.user.clone(), full.auth.clone()),
                )
        });
        let id = match existing {
            Some(p) => p.id.clone(),
            None => {
                let name = unique_name(profiles, &format!("{}@{}:{}", full.user, full.host, full.port));
                let mut p = SshProfile::new(name, full.host.clone(), full.user.clone(), full.auth.clone());
                p.port = full.port;
                p.keepalive_secs = full.keepalive_secs;
                secrets.save(&secret_key(&p.id), &p.secret());
                let id = p.id.clone();
                profiles.push(p.stripped());
                id
            }
        };
        c.ssh_profile = Some(id);
        // The connection keeps only its own password.
        conn_secret.ssh_password = None;
        conn_secret.ssh_passphrase = None;
        secrets.save(&c.id, &conn_secret);
    }
    changed
}

/// What one `Host` entry of `~/.ssh/config` becomes: key file when it names an `IdentityFile`, else the
/// ssh-agent, the way `ssh` itself would try its keys.
pub fn from_host_entry(entry: &theta_drivers::ssh_config::HostEntry, local_user: &str) -> SshProfile {
    let auth = match &entry.identity_file {
        Some(path) => SshAuth::KeyFile { path: path.clone(), passphrase: None },
        None => SshAuth::Agent,
    };
    let user = entry.user.clone().unwrap_or_else(|| local_user.to_string());
    let mut p = SshProfile::new(entry.alias.clone(), entry.address(), user, auth);
    p.port = entry.port.unwrap_or(22);
    p
}

/// Whether `profiles` already holds this server, as the same user, on the same port: importing it
/// again would add a duplicate.
pub fn already_imported(
    profiles: &[SshProfile],
    entry: &theta_drivers::ssh_config::HostEntry,
    local_user: &str,
) -> bool {
    let want = from_host_entry(entry, local_user);
    profiles.iter().any(|p| (&p.host, p.port, &p.user) == (&want.host, want.port, &want.user))
}

/// The configurations to add for the ticked entries. A `ProxyJump` that names another ticked entry (or
/// an existing configuration of that name) becomes its jump host; one that names anything else is left
/// out, and the entry connects directly. Names are the aliases, made unique.
pub fn plan_import(
    entries: &[&theta_drivers::ssh_config::HostEntry],
    existing: &[SshProfile],
    local_user: &str,
) -> Vec<SshProfile> {
    let mut made: Vec<SshProfile> = Vec::new();
    let mut all: Vec<SshProfile> = existing.to_vec();
    for e in entries {
        let mut p = from_host_entry(e, local_user);
        p.name = unique_name(&all, &p.name);
        all.push(p.clone());
        made.push(p);
    }
    for (p, e) in made.iter_mut().zip(entries) {
        let jump = e.proxy_jump.as_deref().filter(|j| !j.contains(',') && !j.eq_ignore_ascii_case("none"));
        let Some(alias) = jump else { continue };
        // `user@host:port` forms are not matched: only an alias can name another entry.
        let target = entries
            .iter()
            .position(|o| o.alias == alias)
            .map(|i| all[existing.len() + i].id.clone())
            .or_else(|| existing.iter().find(|x| x.name == alias).map(|x| x.id.clone()));
        p.via = target.filter(|id| *id != p.id);
    }
    made
}

#[cfg(test)]
mod tests {
    use super::*;
    use theta_core::DatabaseKind;

    fn entry(alias: &str) -> theta_drivers::ssh_config::HostEntry {
        theta_drivers::ssh_config::HostEntry { alias: alias.into(), ..Default::default() }
    }

    #[test]
    fn a_host_entry_becomes_a_configuration() {
        let mut e = entry("db");
        e.host_name = Some("db.example.com".into());
        e.user = Some("admin".into());
        e.port = Some(2222);
        e.identity_file = Some("/home/me/.ssh/db".into());
        let p = from_host_entry(&e, "me");
        assert_eq!(
            (p.name.as_str(), p.host.as_str(), p.user.as_str(), p.port),
            ("db", "db.example.com", "admin", 2222)
        );
        assert_eq!(p.auth, SshAuth::KeyFile { path: "/home/me/.ssh/db".into(), passphrase: None });
        // Without an IdentityFile the agent; without User the local account; without HostName the alias.
        let p = from_host_entry(&entry("box"), "me");
        assert_eq!((p.host.as_str(), p.user.as_str(), p.port, &p.auth), ("box", "me", 22, &SshAuth::Agent));
    }

    #[test]
    fn importing_names_uniquely_and_links_jump_hosts() {
        let mut inner = entry("inner");
        inner.proxy_jump = Some("bastion".into());
        let bastion = entry("bastion");
        let mut other = entry("other");
        other.proxy_jump = Some("me@elsewhere:2222".into());
        let mut multi = entry("multi");
        multi.proxy_jump = Some("bastion,inner".into());
        let existing = vec![key_profile("old", "bastion")];
        let made = plan_import(&[&inner, &bastion, &other, &multi], &existing, "me");
        let names: Vec<_> = made.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["inner", "bastion (2)", "other", "multi"], "an existing name is not reused");
        assert_eq!(made[0].via.as_deref(), Some(made[1].id.as_str()), "the ticked bastion, not the old one");
        assert_eq!(made[2].via, None, "user@host:port is not an alias of an entry");
        assert_eq!(made[3].via, None, "only a single hop is supported");
        // With the bastion not ticked, the existing configuration of that name is the jump host.
        let made = plan_import(&[&inner], &[key_profile("old", "bastion")], "me");
        assert_eq!(made[0].via.as_deref(), Some("old"));
        let made = plan_import(&[&inner], &[], "me");
        assert_eq!(made[0].via, None);
    }

    #[test]
    fn imported_servers_are_recognised() {
        let mut e = entry("jump");
        e.host_name = Some("jump.example".into());
        e.user = Some("me".into());
        assert!(already_imported(&[key_profile("a", "Bastion")], &e, "me"));
        e.port = Some(2222);
        assert!(!already_imported(&[key_profile("a", "Bastion")], &e, "me"));
    }

    fn key_profile(id: &str, name: &str) -> SshProfile {
        let mut p = SshProfile::new(
            name,
            "jump.example",
            "me",
            SshAuth::KeyFile { path: "/k".into(), passphrase: Some("kp-secret-9".into()) },
        );
        p.id = id.into();
        p
    }

    #[test]
    fn address_and_labels() {
        let p = key_profile("a", "Bastion");
        assert_eq!(p.address(), "me@jump.example:22");
        assert_eq!(p.auth_label(), "Key file");
        assert_eq!(auth_label(&SshAuth::Agent), "SSH agent");
        assert_eq!(auth_label(&SshAuth::Password { password: String::new() }), "Password");
    }

    #[test]
    fn the_stored_form_has_no_secret_and_the_secret_round_trips() {
        let p = key_profile("a", "Bastion");
        let json = serde_json::to_string(&p.stripped()).unwrap();
        assert!(!json.contains("kp-secret-9"), "{json}");
        let back = p.stripped().with_secret(&p.secret());
        assert_eq!(back, p);
        let mut pw = key_profile("b", "pw");
        pw.auth = SshAuth::Password { password: "hunter2".into() };
        assert!(!serde_json::to_string(&pw.stripped()).unwrap().contains("hunter2"));
        assert_eq!(pw.stripped().with_secret(&pw.secret()), pw);
        assert!(key_profile("c", "x").stripped().secret().is_empty());
    }

    #[test]
    fn a_file_without_the_newer_fields_reads_with_defaults() {
        let p: SshProfile = serde_json::from_str(
            r#"{"id":"1","name":"n","host":"h","user":"u","auth":{"method":"agent"}}"#,
        )
        .unwrap();
        assert_eq!((p.port, p.keepalive_secs, p.via), (22, 30, None));
    }

    #[test]
    fn resolve_fills_secrets_and_nests_jump_hosts() {
        let secrets = SecretStore::memory_only();
        let outer = key_profile("outer", "Outer");
        secrets.save(&secret_key("outer"), &outer.secret());
        let mut inner = SshProfile::new("Inner", "10.0.0.5", "dba", SshAuth::Password { password: "pw".into() });
        inner.id = "inner".into();
        inner.via = Some("outer".into());
        inner.keepalive_secs = 0;
        secrets.save(&secret_key("inner"), &inner.secret());
        let profiles = vec![outer.stripped(), inner.stripped()];

        let cfg = resolve(&profiles, &secrets, "inner").unwrap();
        assert_eq!((cfg.host.as_str(), cfg.user.as_str(), cfg.keepalive_secs), ("10.0.0.5", "dba", 0));
        assert_eq!(cfg.auth, SshAuth::Password { password: "pw".into() });
        let hop = cfg.via.as_deref().unwrap();
        assert_eq!(hop.host, "jump.example");
        assert_eq!(hop.auth, SshAuth::KeyFile { path: "/k".into(), passphrase: Some("kp-secret-9".into()) });
        assert!(hop.via.is_none());
        assert!(resolve(&profiles, &secrets, "outer").unwrap().via.is_none());
    }

    #[test]
    fn resolve_reports_a_missing_configuration_and_a_loop() {
        let secrets = SecretStore::memory_only();
        let mut a = key_profile("a", "A");
        a.via = Some("b".into());
        let mut b = key_profile("b", "B");
        b.via = Some("a".into());
        let profiles = vec![a, b];
        assert!(resolve(&profiles, &secrets, "a").unwrap_err().to_string().contains("loop"));
        assert!(resolve(&profiles, &secrets, "gone").unwrap_err().to_string().contains("no longer exists"));
        let mut c = key_profile("c", "C");
        c.via = Some("ghost".into());
        assert!(resolve(&[c], &secrets, "c").is_err());
    }

    #[test]
    fn users_include_connections_that_reach_a_configuration_through_a_jump_host() {
        let outer = key_profile("outer", "Outer");
        let mut inner = key_profile("inner", "Inner");
        inner.via = Some("outer".into());
        let profiles = vec![outer, inner];
        let mut direct = ConnectionConfig::new("c1", "direct", DatabaseKind::Postgres);
        direct.ssh_profile = Some("outer".into());
        let mut jumped = ConnectionConfig::new("c2", "jumped", DatabaseKind::Postgres);
        jumped.ssh_profile = Some("inner".into());
        let plain = ConnectionConfig::new("c3", "plain", DatabaseKind::Postgres);
        let conns = vec![direct, jumped, plain];
        let names = |id: &str| users_of(&profiles, &conns, id).iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        assert_eq!(names("outer"), ["direct", "jumped"]);
        assert_eq!(names("inner"), ["jumped"]);
    }

    #[test]
    fn unique_names_count_up() {
        let profiles = vec![key_profile("a", "me@h:22"), key_profile("b", "me@h:22 (2)")];
        assert_eq!(unique_name(&profiles, "other"), "other");
        assert_eq!(unique_name(&profiles, "ME@h:22"), "ME@h:22 (3)");
    }

    #[test]
    fn migration_shares_identical_tunnels_and_moves_secrets() {
        let secrets = SecretStore::memory_only();
        let inline = |auth: SshAuth| SshConfig::new("jump.example", 22, "me", auth);
        let key = || SshAuth::KeyFile { path: "/k".into(), passphrase: None };
        let mut a = ConnectionConfig::new("a", "A", DatabaseKind::Postgres);
        a.password = Some("dbpw".into());
        a.ssh = Some(inline(key()));
        let mut b = ConnectionConfig::new("b", "B", DatabaseKind::MySql);
        b.ssh = Some(inline(key()));
        let mut c = ConnectionConfig::new("c", "C", DatabaseKind::Postgres);
        c.ssh = Some(inline(SshAuth::Password { password: String::new() }));
        let d = ConnectionConfig::new("d", "D", DatabaseKind::Postgres);
        // A and B share the passphrase, C has a password; the file holds none of them.
        for id in ["a", "b"] {
            secrets.save(id, &Secret { ssh_passphrase: Some("kp-secret-9".into()), ..Default::default() });
        }
        secrets.save("a", &Secret {
            password: Some("dbpw".into()),
            ssh_passphrase: Some("kp-secret-9".into()),
            ..Default::default()
        });
        secrets.save("c", &Secret { ssh_password: Some("sshpw".into()), ..Default::default() });
        let mut conns = vec![a, b, c, d];
        let mut profiles = Vec::new();

        assert!(migrate_inline(&mut conns, &mut profiles, &secrets));
        assert_eq!(profiles.len(), 2, "A and B are one configuration, C another");
        assert!(conns.iter().all(|c| c.ssh.is_none()));
        assert_eq!(conns[0].ssh_profile, conns[1].ssh_profile);
        assert_ne!(conns[0].ssh_profile, conns[2].ssh_profile);
        assert_eq!(conns[3].ssh_profile, None);
        assert_eq!(profiles[0].name, "me@jump.example:22");
        assert_eq!(profiles[1].name, "me@jump.example:22 (2)", "same address, different credentials");

        let key_id = conns[0].ssh_profile.clone().unwrap();
        assert_eq!(secrets.load(&secret_key(&key_id)).ssh_passphrase.as_deref(), Some("kp-secret-9"));
        let pw_id = conns[2].ssh_profile.clone().unwrap();
        assert_eq!(secrets.load(&secret_key(&pw_id)).ssh_password.as_deref(), Some("sshpw"));
        // The connection keeps its own password and nothing of the tunnel's.
        assert_eq!(secrets.load("a"), Secret { password: Some("dbpw".into()), ..Default::default() });
        assert!(secrets.load("b").is_empty() && secrets.load("c").is_empty());
        assert!(!serde_json::to_string(&profiles).unwrap().contains("kp-secret-9"));

        // Running it again changes nothing.
        assert!(!migrate_inline(&mut conns, &mut profiles, &secrets));
        assert_eq!(profiles.len(), 2);
    }

    #[test]
    fn migration_reuses_a_configuration_that_already_matches() {
        let secrets = SecretStore::memory_only();
        let mut existing = key_profile("keep", "Mine");
        existing.auth = SshAuth::Agent;
        let mut profiles = vec![existing];
        let mut c = ConnectionConfig::new("a", "A", DatabaseKind::Postgres);
        c.ssh = Some(SshConfig::new("jump.example", 22, "me", SshAuth::Agent));
        let mut conns = vec![c];
        assert!(migrate_inline(&mut conns, &mut profiles, &secrets));
        assert_eq!(profiles.len(), 1);
        assert_eq!(conns[0].ssh_profile.as_deref(), Some("keep"));
    }
}
