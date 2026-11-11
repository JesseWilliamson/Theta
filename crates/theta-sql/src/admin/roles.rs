//! Users and roles: listing, privileges and the SQL that changes them.
//!
//! Anything that changes a role returns a [`Change`], which carries two spellings of the same statement: `sql`
//! is what runs, `shown` is what may be displayed, logged or put into history. They differ only when a
//! password is involved, where `shown` has `'***'` in its place. The app must never write `sql` anywhere but
//! to the connection; [`mask_secret`] cleans server error text, because MySQL quotes the offending part of
//! the statement in a syntax error.

use super::{col, number, string_literal, text, truthy};
use theta_core::{DatabaseKind, Family, Feature, ResultSet};

// ---------------------------------------------------------------------------- reading

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Role {
    pub name: String,
    /// MySQL account host; `None` for PostgreSQL and SQL Server, and for MariaDB roles.
    pub host: Option<String>,
    pub login: bool,
    pub superuser: bool,
    pub create_db: bool,
    pub create_role: bool,
    pub replication: bool,
    pub bypass_rls: bool,
    pub inherit: bool,
    /// `None` is unlimited.
    pub conn_limit: Option<i64>,
    pub valid_until: Option<String>,
    pub member_of: Vec<String>,
    pub members: Vec<String>,
    pub comment: Option<String>,
    /// MySQL: authentication plugin.
    pub plugin: Option<String>,
    /// MySQL: `ACCOUNT LOCK`ed.
    pub locked: bool,
    /// MySQL: the password has expired (the account must change it to log in).
    pub password_expired: bool,
    /// MySQL: days between forced password changes; `None` follows the server default.
    pub password_lifetime: Option<String>,
    /// MySQL / SQL Server: this is a role, not a user.
    pub is_role: bool,
}

impl Role {
    /// `name` for PostgreSQL / SQL Server, `name@host` for MySQL accounts: what a list shows.
    pub fn display(&self) -> String {
        match &self.host {
            Some(h) => format!("{}@{h}", self.name),
            None => self.name.clone(),
        }
    }

    /// How a statement names this role.
    pub fn ident(&self, kind: DatabaseKind) -> String {
        ident(kind, &self.name, self.host.as_deref(), self.is_role)
    }
}

pub fn ident(kind: DatabaseKind, name: &str, host: Option<&str>, is_role: bool) -> String {
    match kind.family() {
        Family::MySql if is_role || host.is_none() => quote_my(name),
        Family::MySql => {
            format!("{}@{}", string_literal(name, true), string_literal(host.unwrap_or("%"), true))
        }
        Family::SqlServer => format!("[{}]", name.replace(']', "]]")),
        _ => quote_pg(name),
    }
}

fn quote_pg(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn quote_my(s: &str) -> String {
    format!("`{}`", s.replace('`', "``"))
}

/// The statements that read roles. `members` (MySQL only) is a second query.
#[derive(Debug, Clone, PartialEq)]
pub struct Queries {
    pub roles: String,
    pub members: Option<String>,
}

fn mariadb(banner: &str) -> bool {
    banner.to_ascii_lowercase().contains("mariadb")
}

pub fn queries(kind: DatabaseKind, banner: &str, show_system: bool) -> Result<Queries, &'static str> {
    if let Some(why) = kind.unsupported(Feature::Roles) {
        return Err(why);
    }
    Ok(match kind.family() {
        Family::Postgres => Queries {
            roles: format!(
                "{POSTGRES_ROLES}{}ORDER BY r.rolname",
                if show_system { "" } else { "WHERE r.rolname !~ '^pg_' " }
            ),
            members: None,
        },
        Family::MySql if mariadb(banner) => {
            Queries { roles: MARIADB_USERS.trim().into(), members: Some(MARIADB_MEMBERS.trim().into()) }
        }
        Family::MySql => {
            Queries { roles: MYSQL_USERS.trim().into(), members: Some(MYSQL_MEMBERS.trim().into()) }
        }
        Family::SqlServer => Queries { roles: MSSQL_PRINCIPALS.trim().into(), members: None },
        _ => return Err("this product has no role tool"),
    })
}

const POSTGRES_ROLES: &str = r#"SELECT r.rolname AS name, r.rolcanlogin AS login, r.rolsuper AS superuser,
       r.rolcreatedb AS createdb, r.rolcreaterole AS createrole, r.rolreplication AS replication,
       r.rolbypassrls AS bypassrls, r.rolinherit AS inherit, r.rolconnlimit AS conn_limit,
       r.rolvaliduntil::text AS valid_until,
       coalesce((SELECT string_agg(g.rolname, ', ' ORDER BY g.rolname) FROM pg_auth_members m
                 JOIN pg_roles g ON g.oid = m.roleid WHERE m.member = r.oid), '') AS member_of,
       coalesce((SELECT string_agg(u.rolname, ', ' ORDER BY u.rolname) FROM pg_auth_members m
                 JOIN pg_roles u ON u.oid = m.member WHERE m.roleid = r.oid), '') AS members,
       shobj_description(r.oid, 'pg_authid') AS comment
FROM pg_roles r
"#;

// `mysql.user` is a view over `mysql.global_priv` in MariaDB 10.4+, which keeps lock state and password
// lifetime in a JSON column rather than as columns.
const MARIADB_USERS: &str = r#"
SELECT u.User AS name, u.Host AS host, u.plugin AS plugin, (u.is_role = 'Y') AS is_role,
       (u.Super_priv = 'Y') AS superuser,
       (COALESCE(JSON_VALUE(g.Priv, '$.account_locked'), 'false') = 'true') AS locked,
       (u.password_expired = 'Y') AS expired,
       JSON_VALUE(g.Priv, '$.password_lifetime') AS lifetime,
       u.max_user_connections AS conn_limit
FROM mysql.user u JOIN mysql.global_priv g ON g.User = u.User AND g.Host = u.Host
ORDER BY u.User, u.Host
"#;

const MARIADB_MEMBERS: &str = r#"
SELECT Role AS role_name, '' AS role_host, User AS member_name, Host AS member_host FROM mysql.roles_mapping
"#;

const MYSQL_USERS: &str = r#"
SELECT User AS name, Host AS host, plugin AS plugin, 0 AS is_role, (Super_priv = 'Y') AS superuser,
       (account_locked = 'Y') AS locked, (password_expired = 'Y') AS expired,
       password_lifetime AS lifetime, max_user_connections AS conn_limit
FROM mysql.user ORDER BY User, Host
"#;

const MYSQL_MEMBERS: &str = r#"
SELECT FROM_USER AS role_name, FROM_HOST AS role_host, TO_USER AS member_name, TO_HOST AS member_host
FROM mysql.role_edges
"#;

// Server-level principals only; STRING_AGG needs SQL Server 2017. Database users and their roles are
// per-database and are not listed here.
const MSSQL_PRINCIPALS: &str = r#"
SELECT p.name AS name,
       CASE WHEN p.is_disabled = 1 THEN 0 ELSE 1 END AS login,
       CASE WHEN EXISTS (SELECT 1 FROM sys.server_role_members m
                         JOIN sys.server_principals r ON r.principal_id = m.role_principal_id
                         WHERE m.member_principal_id = p.principal_id AND r.name = 'sysadmin')
            THEN 1 ELSE 0 END AS superuser,
       CASE WHEN p.type = 'R' THEN 1 ELSE 0 END AS is_role,
       COALESCE((SELECT STRING_AGG(r.name, ', ') FROM sys.server_role_members m
                 JOIN sys.server_principals r ON r.principal_id = m.role_principal_id
                 WHERE m.member_principal_id = p.principal_id), '') AS member_of,
       COALESCE((SELECT STRING_AGG(u.name, ', ') FROM sys.server_role_members m
                 JOIN sys.server_principals u ON u.principal_id = m.member_principal_id
                 WHERE m.role_principal_id = p.principal_id), '') AS members
FROM sys.server_principals p
WHERE p.type IN ('S', 'U', 'G', 'R', 'E', 'X') AND p.name NOT LIKE '##%'
ORDER BY p.name
"#;

fn list(s: &str) -> Vec<String> {
    s.split(',').map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect()
}

pub fn parse_roles(kind: DatabaseKind, rs: &ResultSet) -> Vec<Role> {
    let ix = |n: &str| col(rs, n);
    rs.rows
        .iter()
        .map(|row| {
            let cell = |n: &str| ix(n).and_then(|i| row.get(i));
            let t = |n: &str| cell(n).map(text).unwrap_or_default();
            let b = |n: &str| cell(n).is_some_and(truthy);
            let opt = |n: &str| Some(t(n)).filter(|s| !s.is_empty());
            let limit = cell("conn_limit").and_then(number).map(|n| n as i64).filter(|n| *n > 0);
            let mut r = Role {
                name: t("name"),
                login: b("login"),
                superuser: b("superuser"),
                create_db: b("createdb"),
                create_role: b("createrole"),
                replication: b("replication"),
                bypass_rls: b("bypassrls"),
                inherit: b("inherit"),
                conn_limit: limit,
                valid_until: opt("valid_until"),
                member_of: list(&t("member_of")),
                members: list(&t("members")),
                comment: opt("comment"),
                is_role: b("is_role"),
                ..Default::default()
            };
            if kind.family() == Family::MySql {
                // A MariaDB role has an empty host and is never a login.
                r.host = opt("host");
                r.plugin = opt("plugin");
                r.locked = b("locked");
                r.password_expired = b("expired");
                r.password_lifetime = opt("lifetime").filter(|l| l != "0" && l != "NULL");
                r.login = !r.is_role && !r.locked;
            }
            r
        })
        .collect()
}

/// Fill `member_of` / `members` of MySQL accounts from the membership query.
pub fn apply_members(roles: &mut [Role], rs: &ResultSet) {
    let (Some(rn), Some(rh), Some(mn), Some(mh)) =
        (col(rs, "role_name"), col(rs, "role_host"), col(rs, "member_name"), col(rs, "member_host"))
    else {
        return;
    };
    let label = |n: String, h: String| if h.is_empty() { n } else { format!("{n}@{h}") };
    for row in &rs.rows {
        let role = label(text(&row[rn]), text(&row[rh]));
        let member = label(text(&row[mn]), text(&row[mh]));
        for r in roles.iter_mut() {
            if r.display() == member && !r.member_of.contains(&role) {
                r.member_of.push(role.clone());
            }
            if r.display() == role && !r.members.contains(&member) {
                r.members.push(member.clone());
                // MySQL 8 roles are ordinary accounts; being granted to others is what makes one a role.
                r.is_role = true;
            }
        }
    }
}

// ---------------------------------------------------------------------------- privileges

#[derive(Debug, Clone, PartialEq)]
pub struct Grant {
    /// `global`, `database`, `schema`, `table`, `view`, `sequence`, `column`, `routine`, `owner`.
    pub scope: String,
    pub object: String,
    pub column: Option<String>,
    pub privilege: String,
    pub grantable: bool,
}

/// One row of the privilege pane: an object and what the role may do with it.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectPrivileges {
    pub scope: String,
    pub object: String,
    pub privileges: Vec<String>,
    pub grantable: bool,
}

/// The statement that lists a role's explicit privileges. PostgreSQL answers with `(scope, object, col,
/// privilege, grantable)` rows from the ACL columns (`aclexplode`); MySQL's `SHOW GRANTS` is parsed by
/// [`parse_show_grants`]. Default privileges (PUBLIC's) are not shown: they belong to no role.
pub fn privileges_query(kind: DatabaseKind, role: &Role) -> Result<String, &'static str> {
    if let Some(why) = kind.unsupported(Feature::Roles) {
        return Err(why);
    }
    match kind.family() {
        Family::Postgres => {
            let r = string_literal(&role.name, false);
            Ok(format!(
                r#"WITH r AS (SELECT oid FROM pg_roles WHERE rolname = {r})
SELECT 'database' AS scope, d.datname::text AS object, '' AS col, a.privilege_type AS privilege, a.is_grantable AS grantable
  FROM pg_database d, aclexplode(d.datacl) a, r WHERE a.grantee = r.oid
UNION ALL
SELECT 'schema', n.nspname::text, '', a.privilege_type, a.is_grantable
  FROM pg_namespace n, aclexplode(n.nspacl) a, r WHERE a.grantee = r.oid
UNION ALL
SELECT CASE c.relkind WHEN 'S' THEN 'sequence' WHEN 'v' THEN 'view' WHEN 'm' THEN 'view' ELSE 'table' END,
       n.nspname || '.' || c.relname, '', a.privilege_type, a.is_grantable
  FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace, aclexplode(c.relacl) a, r
  WHERE a.grantee = r.oid AND c.relkind IN ('r', 'p', 'v', 'm', 'f', 'S')
UNION ALL
SELECT 'column', n.nspname || '.' || c.relname, at.attname::text, a.privilege_type, a.is_grantable
  FROM pg_attribute at JOIN pg_class c ON c.oid = at.attrelid JOIN pg_namespace n ON n.oid = c.relnamespace,
       aclexplode(at.attacl) a, r WHERE a.grantee = r.oid AND NOT at.attisdropped
UNION ALL
SELECT 'owner', n.nspname || '.' || c.relname, '', 'OWNER', false
  FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace, r
  WHERE c.relowner = r.oid AND c.relkind IN ('r', 'p', 'v', 'm', 'f', 'S') AND n.nspname NOT IN ('pg_catalog', 'information_schema') AND n.nspname !~ '^pg_toast'
ORDER BY 1, 2, 3, 4"#
            ))
        }
        Family::MySql => Ok(format!("SHOW GRANTS FOR {}", role.ident(kind))),
        _ => Err("privileges are not listed for this product"),
    }
}

pub fn parse_pg_grants(rs: &ResultSet) -> Vec<Grant> {
    let (Some(s), Some(o), Some(c), Some(p), Some(g)) =
        (col(rs, "scope"), col(rs, "object"), col(rs, "col"), col(rs, "privilege"), col(rs, "grantable"))
    else {
        return Vec::new();
    };
    rs.rows
        .iter()
        .map(|r| Grant {
            scope: text(&r[s]),
            object: text(&r[o]),
            column: Some(text(&r[c])).filter(|c| !c.is_empty()),
            privilege: text(&r[p]),
            grantable: truthy(&r[g]),
        })
        .collect()
}

/// Group grants by object; column grants become `SELECT (a, b)` entries on their table. Order: global,
/// database, schema, then relations, in the order the server returned them.
pub fn group(grants: &[Grant]) -> Vec<ObjectPrivileges> {
    let mut out: Vec<ObjectPrivileges> = Vec::new();
    // (scope, object) -> index; column grants are merged per table and privilege.
    let mut cols: Vec<(usize, String, Vec<String>)> = Vec::new();
    for g in grants {
        let scope = if g.scope == "column" { "table" } else { g.scope.as_str() };
        let at = match out.iter().position(|o| o.scope == scope && o.object == g.object) {
            Some(i) => i,
            None => {
                out.push(ObjectPrivileges {
                    scope: scope.into(),
                    object: g.object.clone(),
                    privileges: Vec::new(),
                    grantable: false,
                });
                out.len() - 1
            }
        };
        if let Some(c) = &g.column {
            match cols.iter_mut().find(|(i, p, _)| *i == at && *p == g.privilege) {
                Some((_, _, v)) => v.push(c.clone()),
                None => cols.push((at, g.privilege.clone(), vec![c.clone()])),
            }
        } else if !out[at].privileges.contains(&g.privilege) {
            out[at].privileges.push(g.privilege.clone());
        }
        out[at].grantable |= g.grantable;
    }
    for (at, p, c) in cols {
        out[at].privileges.push(format!("{p} ({})", c.join(", ")));
    }
    let rank = |s: &str| match s {
        "global" => 0,
        "database" => 1,
        "schema" => 2,
        "owner" => 9,
        _ => 5,
    };
    out.sort_by_key(|o| rank(&o.scope));
    out
}

/// Parse the lines of MySQL / MariaDB `SHOW GRANTS`: `GRANT SELECT, INSERT (a, b) ON `db`.`t` TO ...`.
/// Role grants (`GRANT role TO user`) have no `ON` and are skipped; they are memberships.
pub fn parse_show_grants(lines: &[String]) -> Vec<Grant> {
    let mut out = Vec::new();
    for line in lines {
        let Some(rest) = line.trim().strip_prefix("GRANT ") else { continue };
        let Some(on) = find_top_level(rest, " ON ") else { continue };
        let (privs, after) = (&rest[..on], &rest[on + 4..]);
        let Some(to) = after.rfind(" TO ") else { continue };
        let object = after[..to].trim();
        let grantable = after.ends_with(" WITH GRANT OPTION");
        let (scope, label) = match object.split_once('.') {
            Some(("*", "*")) => ("global", "*.*".to_string()),
            Some((db, "*")) => ("database", unquote(db)),
            Some((db, t)) => ("table", format!("{}.{}", unquote(db), unquote(t))),
            None => ("table", unquote(object)),
        };
        for p in split_privs(privs) {
            let (name, cols) = match p.split_once('(') {
                Some((n, c)) => (n.trim().to_string(), Some(c.trim_end_matches(')').to_string())),
                None => (p.trim().to_string(), None),
            };
            match cols {
                Some(c) => {
                    for column in c.split(',').map(|c| unquote(c.trim())) {
                        out.push(Grant {
                            scope: "column".into(),
                            object: label.clone(),
                            column: Some(column),
                            privilege: name.clone(),
                            grantable,
                        });
                    }
                }
                None => out.push(Grant {
                    scope: scope.into(),
                    object: label.clone(),
                    column: None,
                    privilege: name,
                    grantable,
                }),
            }
        }
    }
    out
}

fn find_top_level(s: &str, pat: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ if depth == 0 && s[i..].starts_with(pat) => return Some(i),
            _ => {}
        }
    }
    None
}

fn split_privs(s: &str) -> Vec<String> {
    let (mut out, mut depth, mut cur) = (Vec::new(), 0, String::new());
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c)
            }
            ')' => {
                depth -= 1;
                cur.push(c)
            }
            ',' if depth == 0 => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out.into_iter().map(|p| p.trim().to_string()).collect()
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('`').replace("``", "`")
}

// ---------------------------------------------------------------------------- changing

/// A statement to run, and the same statement as it may be shown. See the module comment.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub sql: String,
    pub shown: String,
    /// One line for the confirmation's header.
    pub summary: String,
    /// Dropping or revoking: the confirmation says so.
    pub destructive: bool,
}

impl Change {
    fn plain(sql: String, summary: impl Into<String>, destructive: bool) -> Change {
        Change { shown: sql.clone(), sql, summary: summary.into(), destructive }
    }

    /// `build` is given the text to use where the password goes: the real literal, then `'***'`.
    fn secret(
        kind: DatabaseKind,
        password: &str,
        summary: impl Into<String>,
        build: impl Fn(&str) -> String,
    ) -> Change {
        let real = string_literal(password, kind.family() == Family::MySql);
        Change { sql: build(&real), shown: build("'***'"), summary: summary.into(), destructive: false }
    }
}

/// Remove every trace of `secret` from `text`: the raw password, and its quoted spelling.
pub fn mask_secret(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    for spelling in [string_literal(secret, true), string_literal(secret, false), secret.to_string()] {
        out = out.replace(&spelling, "'***'");
    }
    out
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NewRole {
    pub name: String,
    /// MySQL account host (`%` when empty).
    pub host: String,
    pub is_role: bool,
    pub login: bool,
    pub superuser: bool,
    pub create_db: bool,
    pub create_role: bool,
    pub replication: bool,
    pub inherit: bool,
    pub conn_limit: Option<i64>,
    pub valid_until: Option<String>,
    pub password: Option<String>,
    /// MySQL.
    pub locked: bool,
    pub password_expired: bool,
}

fn need_edit(kind: DatabaseKind) -> Result<(), String> {
    match kind.unsupported(Feature::RoleEdit) {
        Some(why) => Err(why.to_string()),
        None => Ok(()),
    }
}

fn check_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("A name is required".into());
    }
    if name.contains('\0') {
        return Err("A name cannot contain a NUL character".into());
    }
    Ok(())
}

pub fn create_role(kind: DatabaseKind, r: &NewRole) -> Result<Change, String> {
    need_edit(kind)?;
    check_name(&r.name)?;
    match kind.family() {
        Family::Postgres => {
            let mut opts: Vec<String> = vec![
                if r.login { "LOGIN" } else { "NOLOGIN" }.into(),
                if r.superuser { "SUPERUSER" } else { "NOSUPERUSER" }.into(),
                if r.create_db { "CREATEDB" } else { "NOCREATEDB" }.into(),
                if r.create_role { "CREATEROLE" } else { "NOCREATEROLE" }.into(),
                if r.replication { "REPLICATION" } else { "NOREPLICATION" }.into(),
                if r.inherit { "INHERIT" } else { "NOINHERIT" }.into(),
            ];
            if let Some(n) = r.conn_limit {
                opts.push(format!("CONNECTION LIMIT {n}"));
            }
            if let Some(v) = r.valid_until.as_deref().filter(|v| !v.trim().is_empty()) {
                opts.push(format!("VALID UNTIL {}", string_literal(v, false)));
            }
            let name = quote_pg(&r.name);
            let head = format!("CREATE ROLE {name} WITH {}", opts.join(" "));
            let summary = format!("Create role {}", r.name);
            Ok(match r.password.as_deref().filter(|p| !p.is_empty()) {
                Some(p) => Change::secret(kind, p, summary, |lit| format!("{head} PASSWORD {lit}")),
                None => Change::plain(head, summary, false),
            })
        }
        Family::MySql => {
            let host = if r.host.trim().is_empty() { "%" } else { r.host.trim() };
            if r.is_role {
                let sql = format!("CREATE ROLE {}", quote_my(&r.name));
                return Ok(Change::plain(sql, format!("Create role {}", r.name), false));
            }
            let who = ident(kind, &r.name, Some(host), false);
            let mut tail = String::new();
            if r.password_expired {
                tail.push_str(" PASSWORD EXPIRE");
            }
            if r.locked {
                tail.push_str(" ACCOUNT LOCK");
            }
            if let Some(n) = r.conn_limit {
                tail.push_str(&format!(" WITH MAX_USER_CONNECTIONS {n}"));
            }
            let summary = format!("Create user {}@{host}", r.name);
            Ok(match r.password.as_deref().filter(|p| !p.is_empty()) {
                Some(p) => Change::secret(kind, p, summary, |lit| {
                    format!("CREATE USER {who} IDENTIFIED BY {lit}{tail}")
                }),
                None => Change::plain(format!("CREATE USER {who}{tail}"), summary, false),
            })
        }
        _ => Err("this product cannot change roles here".into()),
    }
}

/// Attributes to change; `None` leaves one alone.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AlterRole {
    pub login: Option<bool>,
    pub superuser: Option<bool>,
    pub create_db: Option<bool>,
    pub create_role: Option<bool>,
    pub replication: Option<bool>,
    pub inherit: Option<bool>,
    /// `Some(None)` removes the limit.
    pub conn_limit: Option<Option<i64>>,
    pub valid_until: Option<Option<String>>,
    pub locked: Option<bool>,
    pub expire_password: Option<bool>,
}

impl AlterRole {
    pub fn is_empty(&self) -> bool {
        *self == AlterRole::default()
    }
}

pub fn alter_role(kind: DatabaseKind, role: &Role, a: &AlterRole) -> Result<Change, String> {
    need_edit(kind)?;
    if a.is_empty() {
        return Err("Nothing to change".into());
    }
    let who = role.ident(kind);
    match kind.family() {
        Family::Postgres => {
            let flag = |v: Option<bool>, yes: &str, no: &str| v.map(|b| if b { yes } else { no }.to_string());
            let mut opts: Vec<String> = [
                flag(a.login, "LOGIN", "NOLOGIN"),
                flag(a.superuser, "SUPERUSER", "NOSUPERUSER"),
                flag(a.create_db, "CREATEDB", "NOCREATEDB"),
                flag(a.create_role, "CREATEROLE", "NOCREATEROLE"),
                flag(a.replication, "REPLICATION", "NOREPLICATION"),
                flag(a.inherit, "INHERIT", "NOINHERIT"),
            ]
            .into_iter()
            .flatten()
            .collect();
            if let Some(l) = a.conn_limit {
                opts.push(format!("CONNECTION LIMIT {}", l.unwrap_or(-1)));
            }
            if let Some(v) = &a.valid_until {
                opts.push(match v {
                    Some(v) => format!("VALID UNTIL {}", string_literal(v, false)),
                    None => "VALID UNTIL 'infinity'".into(),
                });
            }
            if opts.is_empty() {
                return Err("Nothing to change".into());
            }
            Ok(Change::plain(
                format!("ALTER ROLE {who} WITH {}", opts.join(" ")),
                format!("Alter role {}", role.name),
                false,
            ))
        }
        Family::MySql => {
            if role.is_role {
                return Err("A role has no account options to change".into());
            }
            let mut opts = Vec::new();
            match a.locked {
                Some(true) => opts.push("ACCOUNT LOCK".to_string()),
                Some(false) => opts.push("ACCOUNT UNLOCK".to_string()),
                None => {}
            }
            if a.expire_password == Some(true) {
                opts.push("PASSWORD EXPIRE".into());
            }
            if let Some(l) = a.conn_limit {
                opts.push(format!("WITH MAX_USER_CONNECTIONS {}", l.unwrap_or(0)));
            }
            if opts.is_empty() {
                return Err(
                    "MySQL accounts can change lock state, password expiry and the connection limit".into()
                );
            }
            Ok(Change::plain(
                format!("ALTER USER {who} {}", opts.join(" ")),
                format!("Alter user {}", role.display()),
                false,
            ))
        }
        _ => Err("this product cannot change roles here".into()),
    }
}

pub fn set_password(kind: DatabaseKind, role: &Role, password: &str) -> Result<Change, String> {
    need_edit(kind)?;
    if password.is_empty() {
        return Err("A password is required".into());
    }
    let who = role.ident(kind);
    let summary = format!("Set the password of {}", role.display());
    match kind.family() {
        Family::Postgres => {
            Ok(Change::secret(kind, password, summary, |lit| format!("ALTER ROLE {who} PASSWORD {lit}")))
        }
        Family::MySql if role.is_role => Err("A role has no password".into()),
        Family::MySql => {
            Ok(Change::secret(kind, password, summary, |lit| format!("ALTER USER {who} IDENTIFIED BY {lit}")))
        }
        _ => Err("this product cannot change roles here".into()),
    }
}

pub fn grant_membership(kind: DatabaseKind, group: &Role, member: &Role) -> Result<Change, String> {
    need_edit(kind)?;
    Ok(Change::plain(
        format!("GRANT {} TO {}", group.ident(kind), member.ident(kind)),
        format!("Add {} to {}", member.display(), group.display()),
        false,
    ))
}

pub fn revoke_membership(kind: DatabaseKind, group: &Role, member: &Role) -> Result<Change, String> {
    need_edit(kind)?;
    Ok(Change::plain(
        format!("REVOKE {} FROM {}", group.ident(kind), member.ident(kind)),
        format!("Remove {} from {}", member.display(), group.display()),
        true,
    ))
}

/// What a privilege can be granted on.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// MySQL `*.*`.
    Global,
    Database(String),
    Schema(String),
    AllTablesInSchema(String),
    Table {
        schema: Option<String>,
        name: String,
    },
    Sequence {
        schema: Option<String>,
        name: String,
    },
}

/// The privileges that make sense on a target, which is also the whitelist `grant` / `revoke` check.
pub fn grantable_privileges(kind: DatabaseKind, target: &Target) -> &'static [&'static str] {
    match (kind.family(), target) {
        (Family::Postgres, Target::Database(_)) => &["CONNECT", "CREATE", "TEMPORARY", "ALL"],
        (Family::Postgres, Target::Schema(_)) => &["USAGE", "CREATE", "ALL"],
        (Family::Postgres, Target::Sequence { .. }) => &["USAGE", "SELECT", "UPDATE", "ALL"],
        (Family::Postgres, _) => {
            &["SELECT", "INSERT", "UPDATE", "DELETE", "TRUNCATE", "REFERENCES", "TRIGGER", "ALL"]
        }
        (Family::MySql, Target::Table { .. }) => &[
            "SELECT",
            "INSERT",
            "UPDATE",
            "DELETE",
            "CREATE",
            "DROP",
            "ALTER",
            "INDEX",
            "REFERENCES",
            "TRIGGER",
            "ALL PRIVILEGES",
        ],
        (Family::MySql, _) => &[
            "SELECT",
            "INSERT",
            "UPDATE",
            "DELETE",
            "CREATE",
            "DROP",
            "ALTER",
            "INDEX",
            "REFERENCES",
            "TRIGGER",
            "EXECUTE",
            "CREATE VIEW",
            "SHOW VIEW",
            "CREATE ROUTINE",
            "ALTER ROUTINE",
            "EVENT",
            "LOCK TABLES",
            "CREATE TEMPORARY TABLES",
            "ALL PRIVILEGES",
        ],
        _ => &[],
    }
}

fn target_sql(kind: DatabaseKind, t: &Target) -> Result<String, String> {
    let qual = |schema: &Option<String>, name: &str| match schema {
        Some(s) => format!("{}.{}", quote_pg(s), quote_pg(name)),
        None => quote_pg(name),
    };
    Ok(match (kind.family(), t) {
        (Family::Postgres, Target::Database(d)) => format!("DATABASE {}", quote_pg(d)),
        (Family::Postgres, Target::Schema(s)) => format!("SCHEMA {}", quote_pg(s)),
        (Family::Postgres, Target::AllTablesInSchema(s)) => format!("ALL TABLES IN SCHEMA {}", quote_pg(s)),
        (Family::Postgres, Target::Table { schema, name }) => format!("TABLE {}", qual(schema, name)),
        (Family::Postgres, Target::Sequence { schema, name }) => format!("SEQUENCE {}", qual(schema, name)),
        (Family::Postgres, Target::Global) => return Err("PostgreSQL has no global privileges".into()),
        (Family::MySql, Target::Global) => "*.*".into(),
        (Family::MySql, Target::Database(d) | Target::Schema(d) | Target::AllTablesInSchema(d)) => {
            format!("{}.*", quote_my(d))
        }
        (Family::MySql, Target::Table { schema, name }) => match schema {
            Some(s) => format!("{}.{}", quote_my(s), quote_my(name)),
            None => quote_my(name),
        },
        (Family::MySql, Target::Sequence { .. }) => return Err("MySQL has no sequences".into()),
        _ => return Err("this product cannot change privileges here".into()),
    })
}

fn priv_list(kind: DatabaseKind, target: &Target, privs: &[String]) -> Result<String, String> {
    if privs.is_empty() {
        return Err("Choose at least one privilege".into());
    }
    let allowed = grantable_privileges(kind, target);
    let mut out = Vec::new();
    for p in privs {
        let up = p.trim().to_ascii_uppercase();
        let Some(ok) = allowed.iter().find(|a| **a == up) else {
            return Err(format!("{p} cannot be granted on this object"));
        };
        out.push(*ok);
    }
    Ok(out.join(", "))
}

pub fn grant_privileges(
    kind: DatabaseKind,
    role: &Role,
    privs: &[String],
    target: &Target,
    with_grant_option: bool,
) -> Result<Change, String> {
    need_edit(kind)?;
    let (p, t) = (priv_list(kind, target, privs)?, target_sql(kind, target)?);
    let option = if with_grant_option { " WITH GRANT OPTION" } else { "" };
    Ok(Change::plain(
        format!("GRANT {p} ON {t} TO {}{option}", role.ident(kind)),
        format!("Grant {p} to {}", role.display()),
        false,
    ))
}

pub fn revoke_privileges(
    kind: DatabaseKind,
    role: &Role,
    privs: &[String],
    target: &Target,
) -> Result<Change, String> {
    need_edit(kind)?;
    let (p, t) = (priv_list(kind, target, privs)?, target_sql(kind, target)?);
    Ok(Change::plain(
        format!("REVOKE {p} ON {t} FROM {}", role.ident(kind)),
        format!("Revoke {p} from {}", role.display()),
        true,
    ))
}

pub fn drop_role(kind: DatabaseKind, role: &Role) -> Result<Change, String> {
    need_edit(kind)?;
    let sql = match kind.family() {
        Family::Postgres => format!("DROP ROLE {}", role.ident(kind)),
        Family::MySql if role.is_role => format!("DROP ROLE {}", role.ident(kind)),
        Family::MySql => format!("DROP USER {}", role.ident(kind)),
        _ => return Err("this product cannot change roles here".into()),
    };
    Ok(Change::plain(sql, format!("Drop {}", role.display()), true))
}

/// PostgreSQL refuses `DROP ROLE` while the role owns objects or holds privileges in any database. This
/// counts what is visible from the current database so the confirmation can warn; `None` for MySQL, which
/// has no ownership.
pub fn owned_objects_query(kind: DatabaseKind, role: &Role) -> Option<String> {
    (kind.family() == Family::Postgres).then(|| {
        format!(
            "SELECT count(*) FILTER (WHERE d.deptype = 'o') AS owned, count(*) FILTER (WHERE d.deptype = 'a') AS granted \
             FROM pg_shdepend d JOIN pg_roles r ON r.oid = d.refobjid \
             WHERE r.rolname = {} AND d.refclassid = 'pg_authid'::regclass \
             AND (d.dbid = (SELECT oid FROM pg_database WHERE datname = current_database()) OR d.dbid = 0)",
            string_literal(&role.name, false)
        )
    })
}

pub fn drop_warning(owned: i64, granted: i64) -> Option<String> {
    if owned == 0 && granted == 0 {
        return None;
    }
    Some(format!(
        "This role owns {owned} object(s) and holds {granted} privilege(s) in this database. PostgreSQL refuses to drop it until they are reassigned (REASSIGN OWNED BY … TO …) or removed (DROP OWNED BY …); objects in other databases are not counted here."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use theta_core::{Column, TypeKind, Value};

    const PG: DatabaseKind = DatabaseKind::Postgres;
    const MY: DatabaseKind = DatabaseKind::MySql;

    fn rs(cols: &[&str], rows: Vec<Vec<Value>>) -> ResultSet {
        ResultSet {
            columns: cols
                .iter()
                .map(|n| Column {
                    name: n.to_string(),
                    type_name: "text".into(),
                    kind: TypeKind::Text,
                    origin: None,
                })
                .collect(),
            rows,
            truncated: false,
        }
    }

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    fn pg_role(name: &str) -> Role {
        Role { name: name.into(), ..Default::default() }
    }

    fn my_user(name: &str, host: &str) -> Role {
        Role { name: name.into(), host: Some(host.into()), ..Default::default() }
    }

    #[test]
    fn unsupported_products_say_why() {
        assert!(queries(DatabaseKind::Sqlite, "", false).unwrap_err().contains("embedded"));
        assert!(queries(DatabaseKind::ClickHouse, "", false).is_err());
        assert!(queries(DatabaseKind::SqlServer, "", false).is_ok(), "listing works on SQL Server");
        // ... but editing does not, and says so.
        assert!(
            create_role(DatabaseKind::SqlServer, &NewRole { name: "x".into(), ..Default::default() })
                .is_err()
        );
        assert!(drop_role(DatabaseKind::Sqlite, &pg_role("x")).is_err());
    }

    #[test]
    fn mysql_flavour_follows_the_banner() {
        assert!(queries(MY, "10.11.14-MariaDB", false).unwrap().roles.contains("global_priv"));
        assert!(queries(MY, "10.11.14-MariaDB", false).unwrap().members.unwrap().contains("roles_mapping"));
        assert!(queries(MY, "8.0.36", false).unwrap().members.unwrap().contains("role_edges"));
        assert!(queries(PG, "", false).unwrap().roles.contains("!~ '^pg_'"));
        assert!(!queries(PG, "", true).unwrap().roles.contains("!~ '^pg_'"));
    }

    #[test]
    fn parses_postgres_roles() {
        let r = rs(
            &[
                "name",
                "login",
                "superuser",
                "createdb",
                "createrole",
                "replication",
                "bypassrls",
                "inherit",
                "conn_limit",
                "valid_until",
                "member_of",
                "members",
                "comment",
            ],
            vec![vec![
                t("app"),
                Value::Bool(true),
                Value::Bool(false),
                Value::Bool(true),
                Value::Bool(false),
                Value::Bool(false),
                Value::Bool(false),
                Value::Bool(true),
                Value::Int(-1),
                Value::Null,
                t("readers, writers"),
                t(""),
                Value::Null,
            ]],
        );
        let roles = parse_roles(PG, &r);
        assert_eq!(roles[0].member_of, ["readers", "writers"]);
        assert!(roles[0].login && roles[0].create_db && roles[0].conn_limit.is_none());
    }

    #[test]
    fn parses_mysql_accounts_and_memberships() {
        let r = rs(
            &[
                "name",
                "host",
                "plugin",
                "is_role",
                "superuser",
                "locked",
                "expired",
                "lifetime",
                "conn_limit",
            ],
            vec![
                vec![
                    t("ann"),
                    t("%"),
                    t("mysql_native_password"),
                    Value::Int(0),
                    Value::Int(0),
                    Value::Int(1),
                    Value::Int(0),
                    Value::Null,
                    Value::Int(5),
                ],
                vec![
                    t("staff"),
                    t(""),
                    t(""),
                    Value::Int(1),
                    Value::Int(0),
                    Value::Int(0),
                    Value::Int(0),
                    Value::Null,
                    Value::Int(0),
                ],
            ],
        );
        let mut roles = parse_roles(MY, &r);
        assert!(roles[0].locked && !roles[0].login && roles[0].conn_limit == Some(5));
        assert!(roles[1].is_role && roles[1].host.is_none());
        let m = rs(
            &["role_name", "role_host", "member_name", "member_host"],
            vec![vec![t("staff"), t(""), t("ann"), t("%")]],
        );
        apply_members(&mut roles, &m);
        assert_eq!(roles[0].member_of, ["staff"]);
        assert_eq!(roles[1].members, ["ann@%"]);
    }

    #[test]
    fn creates_a_postgres_role_and_masks_the_password() {
        let c = create_role(
            PG,
            &NewRole {
                name: "app user".into(),
                login: true,
                inherit: true,
                conn_limit: Some(5),
                password: Some("it's secret".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            c.sql,
            "CREATE ROLE \"app user\" WITH LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION INHERIT CONNECTION LIMIT 5 PASSWORD 'it''s secret'"
        );
        assert!(c.shown.ends_with("PASSWORD '***'"));
        assert!(!c.shown.contains("secret") && !c.summary.contains("secret"));
    }

    #[test]
    fn mysql_account_statements() {
        let c = create_role(
            MY,
            &NewRole {
                name: "ann".into(),
                host: "10.%".into(),
                password: Some("p\\w'd".into()),
                locked: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(c.sql, "CREATE USER 'ann'@'10.%' IDENTIFIED BY 'p\\\\w''d' ACCOUNT LOCK");
        assert_eq!(c.shown, "CREATE USER 'ann'@'10.%' IDENTIFIED BY '***' ACCOUNT LOCK");
        let u = my_user("ann", "%");
        let a = alter_role(
            MY,
            &u,
            &AlterRole { locked: Some(false), conn_limit: Some(Some(3)), ..Default::default() },
        )
        .unwrap();
        assert_eq!(a.sql, "ALTER USER 'ann'@'%' ACCOUNT UNLOCK WITH MAX_USER_CONNECTIONS 3");
        let p = set_password(MY, &u, "pw").unwrap();
        assert_eq!(
            (p.sql.as_str(), p.shown.as_str()),
            ("ALTER USER 'ann'@'%' IDENTIFIED BY 'pw'", "ALTER USER 'ann'@'%' IDENTIFIED BY '***'")
        );
        assert_eq!(drop_role(MY, &u).unwrap().sql, "DROP USER 'ann'@'%'");
        let role = Role { name: "staff".into(), is_role: true, ..Default::default() };
        assert_eq!(drop_role(MY, &role).unwrap().sql, "DROP ROLE `staff`");
        assert_eq!(grant_membership(MY, &role, &u).unwrap().sql, "GRANT `staff` TO 'ann'@'%'");
    }

    #[test]
    fn postgres_alter_password_membership_and_drop() {
        let r = pg_role("app");
        let a = alter_role(
            PG,
            &r,
            &AlterRole {
                login: Some(false),
                create_db: Some(true),
                conn_limit: Some(None),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(a.sql, "ALTER ROLE \"app\" WITH NOLOGIN CREATEDB CONNECTION LIMIT -1");
        assert!(alter_role(PG, &r, &AlterRole::default()).is_err());
        let p = set_password(PG, &r, "hunter2").unwrap();
        assert_eq!(p.sql, "ALTER ROLE \"app\" PASSWORD 'hunter2'");
        assert_eq!(p.shown, "ALTER ROLE \"app\" PASSWORD '***'");
        assert!(set_password(PG, &r, "").is_err());
        assert_eq!(
            grant_membership(PG, &pg_role("readers"), &r).unwrap().sql,
            "GRANT \"readers\" TO \"app\""
        );
        let rv = revoke_membership(PG, &pg_role("readers"), &r).unwrap();
        assert_eq!(rv.sql, "REVOKE \"readers\" FROM \"app\"");
        assert!(rv.destructive);
        assert_eq!(drop_role(PG, &r).unwrap().sql, "DROP ROLE \"app\"");
    }

    #[test]
    fn privilege_statements_are_whitelisted() {
        let r = pg_role("app");
        let tbl = Target::Table { schema: Some("public".into()), name: "orders".into() };
        let g = grant_privileges(PG, &r, &["select".into(), "UPDATE".into()], &tbl, true).unwrap();
        assert_eq!(g.sql, "GRANT SELECT, UPDATE ON TABLE \"public\".\"orders\" TO \"app\" WITH GRANT OPTION");
        let v =
            revoke_privileges(PG, &r, &["ALL".into()], &Target::AllTablesInSchema("public".into())).unwrap();
        assert_eq!(v.sql, "REVOKE ALL ON ALL TABLES IN SCHEMA \"public\" FROM \"app\"");
        assert!(grant_privileges(PG, &r, &["SELECT; DROP TABLE x".into()], &tbl, false).is_err());
        assert!(
            grant_privileges(PG, &r, &["CONNECT".into()], &tbl, false).is_err(),
            "CONNECT is for databases"
        );
        assert!(grant_privileges(PG, &r, &[], &tbl, false).is_err());
        let u = my_user("ann", "%");
        let g =
            grant_privileges(MY, &u, &["SELECT".into()], &Target::Database("shop".into()), false).unwrap();
        assert_eq!(g.sql, "GRANT SELECT ON `shop`.* TO 'ann'@'%'");
        let g = grant_privileges(MY, &u, &["ALL PRIVILEGES".into()], &Target::Global, false).unwrap();
        assert_eq!(g.sql, "GRANT ALL PRIVILEGES ON *.* TO 'ann'@'%'");
    }

    #[test]
    fn quoting_defeats_injection_in_names() {
        let r = pg_role("a\"; DROP ROLE x; --");
        assert_eq!(drop_role(PG, &r).unwrap().sql, "DROP ROLE \"a\"\"; DROP ROLE x; --\"");
        let u = my_user("a'; DROP USER x; --", "h'");
        assert_eq!(drop_role(MY, &u).unwrap().sql, "DROP USER 'a''; DROP USER x; --'@'h'''");
    }

    #[test]
    fn mask_secret_removes_every_spelling() {
        let msg = "syntax error near 'it''s secret' (it's secret)";
        let masked = mask_secret(msg, "it's secret");
        assert!(!masked.contains("secret"), "{masked}");
        assert_eq!(mask_secret("plain", ""), "plain");
    }

    #[test]
    fn parses_show_grants() {
        let lines: Vec<String> = [
            "GRANT ALL PRIVILEGES ON *.* TO `quarry`@`localhost` IDENTIFIED BY PASSWORD '*13'",
            "GRANT SELECT, INSERT ON `shop`.* TO `ann`@`%`",
            "GRANT SELECT (id, name), UPDATE (name) ON `shop`.`stores` TO `ann`@`%` WITH GRANT OPTION",
            "GRANT `staff` TO `ann`@`%`",
        ]
        .map(String::from)
        .into();
        let g = parse_show_grants(&lines);
        let grouped = group(&g);
        assert_eq!(grouped[0].scope, "global");
        assert_eq!(grouped[0].privileges, ["ALL PRIVILEGES"]);
        assert_eq!(grouped[1].object, "shop");
        assert_eq!(grouped[1].privileges, ["SELECT", "INSERT"]);
        assert_eq!(grouped[2].object, "shop.stores");
        assert_eq!(grouped[2].privileges, ["SELECT (id, name)", "UPDATE (name)"]);
        assert!(grouped[2].grantable && !grouped[1].grantable);
        assert_eq!(grouped.len(), 3, "the role grant is a membership, not a privilege");
    }

    #[test]
    fn groups_postgres_grants_by_object() {
        let r = rs(
            &["scope", "object", "col", "privilege", "grantable"],
            vec![
                vec![t("table"), t("public.orders"), t(""), t("SELECT"), Value::Bool(false)],
                vec![t("table"), t("public.orders"), t(""), t("INSERT"), Value::Bool(false)],
                vec![t("column"), t("public.orders"), t("total"), t("UPDATE"), Value::Bool(false)],
                vec![t("database"), t("shop"), t(""), t("CONNECT"), Value::Bool(true)],
                vec![t("owner"), t("public.stores"), t(""), t("OWNER"), Value::Bool(false)],
            ],
        );
        let g = group(&parse_pg_grants(&r));
        assert_eq!(g[0].scope, "database");
        let orders = g.iter().find(|o| o.object == "public.orders").unwrap();
        assert_eq!(orders.privileges, ["SELECT", "INSERT", "UPDATE (total)"]);
        assert_eq!(g.last().unwrap().scope, "owner");
    }

    #[test]
    fn drop_warning_only_when_something_is_owned() {
        assert!(drop_warning(0, 0).is_none());
        assert!(drop_warning(3, 0).unwrap().contains("REASSIGN OWNED"));
        assert!(owned_objects_query(PG, &pg_role("x")).unwrap().contains("pg_shdepend"));
        assert!(owned_objects_query(MY, &pg_role("x")).is_none());
    }
}
