//! A reader for OpenSSH client configuration (`~/.ssh/config`), enough to turn its `Host` entries into
//! SSH configurations: `HostName`, `User`, `Port`, `IdentityFile` and `ProxyJump`, with `Include`,
//! `Host` patterns and the `%h %u %p %r` tokens.
//!
//! Everything else in the file (`ForwardAgent`, `LocalForward`, `Match` blocks...) is skipped without
//! complaint: the entry is still listed, because a host that cannot be tunnelled the way its author
//! wrote it is still worth offering. The parser is pure; `Include` reaches the disk only through the
//! [`Reader`] it is given, which is how the tests run without files.

use std::path::{Path, PathBuf};

/// One concrete host of the file, after applying every block that matches its alias.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostEntry {
    /// The name after `Host`; what the user types after `ssh`.
    pub alias: String,
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    /// The first `IdentityFile`, with `~` and the tokens expanded.
    pub identity_file: Option<String>,
    /// The raw `ProxyJump` value (`bastion`, `me@jump:2222`, or `none`).
    pub proxy_jump: Option<String>,
}

impl HostEntry {
    /// The address to connect to: `HostName`, or the alias itself when the file gives none.
    pub fn address(&self) -> &str {
        self.host_name.as_deref().unwrap_or(&self.alias)
    }
}

/// What the tokens and `~` expand to.
#[derive(Debug, Clone)]
pub struct Env {
    pub home: PathBuf,
    /// The local account name (`%u`, and the default `User`).
    pub local_user: String,
}

/// Where `Include` finds more text. Given the pattern as written (already with `~` expanded), returns
/// the text of every file it names, in order.
pub type Reader<'a> = &'a mut dyn FnMut(&str) -> Vec<String>;

/// A line of the file, split into a lowercase keyword and its argument.
fn split_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let key_end = line.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(line.len());
    let key = line[..key_end].to_ascii_lowercase();
    let rest = line[key_end..].trim_start();
    // `Key=value`, `Key = value` and `Key value` are all the same line.
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
    Some((key, rest.trim_end().to_string()))
}

/// The words of an argument: split at white space, `"quoted words"` kept together, and a ` #` outside
/// quotes starting a comment.
fn words(arg: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut prev_space = true;
    for c in arg.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '#' if !quoted && prev_space => break,
            c if c.is_whitespace() && !quoted => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
        prev_space = c.is_whitespace() && !quoted;
    }
    if started {
        out.push(cur);
    }
    out
}

/// `*` and `?` wildcards, the way `Host` patterns use them.
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Does a `Host` line's pattern list select `alias`? A matching `!pattern` rules it out whatever else
/// matches, as in OpenSSH.
fn selects(patterns: &[String], alias: &str) -> bool {
    let mut hit = false;
    for p in patterns {
        match p.strip_prefix('!') {
            Some(neg) if glob(neg, alias) => return false,
            Some(_) => {}
            None => hit |= glob(p, alias),
        }
    }
    hit
}

fn is_concrete(pattern: &str) -> bool {
    !pattern.is_empty() && !pattern.contains(['*', '?', '!'])
}

/// One `Host` block (or the lines before the first one, which apply to every host).
#[derive(Debug, Default)]
struct Block {
    /// `None` for the leading lines, which match everything.
    patterns: Option<Vec<String>>,
    /// A `Match` block is skipped: its conditions are not evaluated here.
    skipped: bool,
    host_name: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<String>,
    proxy_jump: Option<String>,
}

const MAX_INCLUDE_DEPTH: usize = 8;

fn collect(text: &str, env: &Env, read: Reader, depth: usize, blocks: &mut Vec<Block>) {
    for line in text.lines() {
        let Some((key, arg)) = split_line(line) else { continue };
        let args = words(&arg);
        match key.as_str() {
            "host" => blocks.push(Block { patterns: Some(args), ..Default::default() }),
            // A Match block's conditions are not evaluated; nothing in it may leak into other hosts.
            "match" => blocks.push(Block { skipped: true, ..Default::default() }),
            "include" => {
                if depth >= MAX_INCLUDE_DEPTH {
                    continue;
                }
                for pattern in args {
                    let expanded = expand_tilde(&pattern, &env.home);
                    for included in read(&expanded) {
                        // An Include inside a Host block continues in that block; one at the top
                        // level is its own, so the included file's Host lines start new blocks.
                        collect(&included, env, read, depth + 1, blocks);
                    }
                }
            }
            _ => {
                if blocks.is_empty() {
                    blocks.push(Block::default());
                }
                let block = blocks.last_mut().expect("a block was just pushed");
                let first = args.first().cloned();
                match key.as_str() {
                    "hostname" => block.host_name = block.host_name.take().or(first),
                    "user" => block.user = block.user.take().or(first),
                    "port" => block.port = block.port.or(first.and_then(|p| p.parse().ok())),
                    "identityfile" => block.identity_file = block.identity_file.take().or(first),
                    "proxyjump" => block.proxy_jump = block.proxy_jump.take().or(first),
                    _ => {}
                }
            }
        }
    }
}

fn expand_tilde(path: &str, home: &Path) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest).to_string_lossy().into_owned(),
        None if path == "~" => home.to_string_lossy().into_owned(),
        None => path.to_string(),
    }
}

/// Replace `%h %u %p %r %d %%` in `text`; a token it does not know stays as written.
fn expand_tokens(text: &str, host: &str, user: &str, port: u16, env: &Env) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('h') => out.push_str(host),
            Some('r') => out.push_str(user),
            Some('u') => out.push_str(&env.local_user),
            Some('p') => out.push_str(&port.to_string()),
            Some('d') => out.push_str(&env.home.to_string_lossy()),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// The hosts of `text`, in the order their names first appear. Relative `Include`s are the
/// caller's business: [`Reader`] receives the pattern with `~` expanded and decides where a
/// relative one starts.
pub fn parse(text: &str, env: &Env, read: Reader) -> Vec<HostEntry> {
    let mut blocks = Vec::new();
    collect(text, env, read, 0, &mut blocks);
    let mut aliases: Vec<String> = Vec::new();
    for b in &blocks {
        for p in b.patterns.iter().flatten() {
            if is_concrete(p) && !aliases.contains(p) {
                aliases.push(p.clone());
            }
        }
    }
    aliases
        .into_iter()
        .map(|alias| {
            let mut e = HostEntry { alias: alias.clone(), ..Default::default() };
            // First value wins, over every block that applies, as OpenSSH reads the file.
            for b in blocks.iter().filter(|b| !b.skipped && b.patterns.as_ref().is_none_or(|p| selects(p, &alias)))
            {
                e.host_name = e.host_name.take().or_else(|| b.host_name.clone());
                e.user = e.user.take().or_else(|| b.user.clone());
                e.port = e.port.or(b.port);
                e.identity_file = e.identity_file.take().or_else(|| b.identity_file.clone());
                e.proxy_jump = e.proxy_jump.take().or_else(|| b.proxy_jump.clone());
            }
            let host = e.host_name.as_deref().map(|h| h.replace("%h", &alias)).unwrap_or_else(|| alias.clone());
            let user = e.user.clone().unwrap_or_else(|| env.local_user.clone());
            let port = e.port.unwrap_or(22);
            e.identity_file = e
                .identity_file
                .take()
                .map(|f| expand_tilde(&expand_tokens(&f, &host, &user, port, env), &env.home));
            e.host_name = e.host_name.take().map(|_| host);
            e
        })
        .collect()
}

/// Read `path` (normally `~/.ssh/config`) from disk. A missing file is `None`; `Include` patterns may
/// use `*` and `?` in their last path component and, when relative, start at `~/.ssh`.
pub fn read_file(path: &Path, env: &Env) -> Option<Vec<HostEntry>> {
    let text = std::fs::read_to_string(path).ok()?;
    let ssh_dir = env.home.join(".ssh");
    let mut reader = |pattern: &str| include_texts(pattern, &ssh_dir);
    Some(parse(&text, env, &mut reader))
}

fn include_texts(pattern: &str, ssh_dir: &Path) -> Vec<String> {
    let path = if Path::new(pattern).is_absolute() { PathBuf::from(pattern) } else { ssh_dir.join(pattern) };
    let (Some(dir), Some(file)) = (path.parent(), path.file_name().and_then(|f| f.to_str())) else {
        return Vec::new();
    };
    if !file.contains(['*', '?']) {
        return std::fs::read_to_string(&path).into_iter().collect();
    }
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| glob(file, n)))
        .map(|e| e.path())
        .collect();
    names.sort();
    names.iter().filter_map(|p| std::fs::read_to_string(p).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env { home: PathBuf::from("/home/me"), local_user: "me".into() }
    }

    fn hosts(text: &str) -> Vec<HostEntry> {
        parse(text, &env(), &mut |_| Vec::new())
    }

    #[test]
    fn reads_the_five_directives_in_both_spellings() {
        let e = hosts(
            "Host db\n  HostName db.example.com\n  User admin\n  Port 2222\n  IdentityFile ~/.ssh/db_key\n\
             Host lax\n  hostname=lax.example.com\n  USER = bob\n  port=2200\n",
        );
        assert_eq!(e.len(), 2);
        assert_eq!(
            e[0],
            HostEntry {
                alias: "db".into(),
                host_name: Some("db.example.com".into()),
                user: Some("admin".into()),
                port: Some(2222),
                identity_file: Some("/home/me/.ssh/db_key".into()),
                proxy_jump: None,
            }
        );
        assert_eq!(e[1].host_name.as_deref(), Some("lax.example.com"));
        assert_eq!((e[1].user.as_deref(), e[1].port), (Some("bob"), Some(2200)));
    }

    #[test]
    fn comments_blank_lines_and_trailing_comments_are_ignored() {
        let e = hosts("# top\n\nHost a # the first\n  # HostName nope\n  HostName a.example # trailing\n");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].alias, "a");
        assert_eq!(e[0].host_name.as_deref(), Some("a.example"));
    }

    #[test]
    fn several_patterns_on_one_line_give_several_entries_and_wildcards_are_skipped() {
        let e = hosts("Host web1 web2 *.internal !skip\n  User deploy\n");
        let names: Vec<_> = e.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(names, ["web1", "web2"]);
        assert!(e.iter().all(|h| h.user.as_deref() == Some("deploy")));
        assert!(hosts("Host *\n  User x\nHost ?\n  User y\n").is_empty());
    }

    #[test]
    fn star_blocks_supply_defaults_and_the_first_value_wins() {
        let e = hosts(
            "Host *\n  User fallback\n  Port 2022\nHost a\n  User mine\n  HostName a.example\nHost *.example\n  Port 9\n",
        );
        // `Host *` comes first in the file, so its User wins over the later, more specific one,
        // exactly as in OpenSSH ("the first obtained value will be used").
        assert_eq!(e[0].user.as_deref(), Some("fallback"));
        assert_eq!(e[0].port, Some(2022));
        let e = hosts("Host a\n  User mine\nHost *\n  User fallback\n  Port 2022\n");
        assert_eq!((e[0].user.as_deref(), e[0].port), (Some("mine"), Some(2022)));
    }

    #[test]
    fn negated_patterns_exclude() {
        let e = hosts("Host a b\n  HostName one\nHost * !b\n  User star\n");
        assert_eq!(e[0].user.as_deref(), Some("star"));
        assert_eq!(e[1].user, None);
    }

    #[test]
    fn identity_file_tokens_and_tilde_expand() {
        let e = hosts(
            "Host db\n  HostName db.example.com\n  User admin\n  Port 2222\n  \
             IdentityFile ~/.ssh/%h_%r_%p_%u_%%\n",
        );
        assert_eq!(e[0].identity_file.as_deref(), Some("/home/me/.ssh/db.example.com_admin_2222_me_%"));
        let e = hosts("Host x\n  IdentityFile \"/keys/my key\"\n  IdentityFile /second\n");
        assert_eq!(e[0].identity_file.as_deref(), Some("/keys/my key"), "quotes keep the space; the first file wins");
        // Without HostName, %h is the alias; an unknown token stays as written.
        let e = hosts("Host box\n  IdentityFile /k/%h-%z\n");
        assert_eq!(e[0].identity_file.as_deref(), Some("/k/box-%z"));
    }

    #[test]
    fn include_is_followed_with_a_reader_and_match_blocks_and_unknown_keys_are_ignored() {
        let main = "Include conf.d/*.conf\nHost own\n  HostName own.example\n  ForwardAgent yes\n  LocalForward 1 2\n\
                    Match host own\n  User never\n";
        let mut asked = Vec::new();
        let e = parse(main, &env(), &mut |p| {
            asked.push(p.to_string());
            vec!["Host inc\n  HostName inc.example\n  User viainc\n".to_string()]
        });
        assert_eq!(asked, ["conf.d/*.conf"]);
        let names: Vec<_> = e.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(names, ["inc", "own"]);
        assert_eq!(e[0].user.as_deref(), Some("viainc"));
        assert_eq!(e[1].user, None, "Match blocks are not evaluated");
        assert_eq!(e[1].host_name.as_deref(), Some("own.example"));
    }

    #[test]
    fn include_with_tilde_and_a_cycle_terminates() {
        let mut asked = Vec::new();
        let e = parse("Include ~/more\nHost a\n", &env(), &mut |p| {
            asked.push(p.to_string());
            vec!["Include ~/more\nHost b\n".to_string()]
        });
        assert_eq!(asked[0], "/home/me/more");
        assert!(asked.len() <= MAX_INCLUDE_DEPTH + 1);
        assert!(e.iter().any(|h| h.alias == "a") && e.iter().any(|h| h.alias == "b"));
    }

    #[test]
    fn proxy_jump_and_default_address() {
        let e = hosts("Host inner\n  ProxyJump me@bastion:2222\nHost bastion\n  HostName 10.0.0.1\n");
        assert_eq!(e[0].proxy_jump.as_deref(), Some("me@bastion:2222"));
        assert_eq!(e[0].address(), "inner", "no HostName: the alias is the address");
        assert_eq!(e[1].address(), "10.0.0.1");
    }

    #[test]
    fn a_repeated_alias_is_listed_once_and_a_bad_port_is_dropped() {
        let e = hosts("Host a\n  Port nope\nHost a\n  Port 22\n");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].port, Some(22), "an unreadable value does not block a later one");
    }

    #[test]
    fn glob_matches_like_ssh() {
        assert!(glob("*.example.com", "db.example.com"));
        assert!(glob("db?", "db1"));
        assert!(!glob("db?", "db"));
        assert!(glob("*", ""));
        assert!(!glob("a*b", "acd"));
    }

    #[test]
    fn read_file_follows_includes_on_disk() {
        let dir = std::env::temp_dir().join(format!("theta-sshcfg-{}", std::process::id()));
        let ssh = dir.join(".ssh");
        std::fs::create_dir_all(ssh.join("conf.d")).unwrap();
        std::fs::write(ssh.join("config"), "Include conf.d/*.conf\nHost top\n  User t\n").unwrap();
        std::fs::write(ssh.join("conf.d/a.conf"), "Host fromfile\n  Port 2200\n").unwrap();
        std::fs::write(ssh.join("conf.d/ignored.txt"), "Host nope\n").unwrap();
        let env = Env { home: dir.clone(), local_user: "me".into() };
        let e = read_file(&ssh.join("config"), &env).unwrap();
        let names: Vec<_> = e.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(names, ["fromfile", "top"]);
        assert_eq!(e[0].port, Some(2200));
        assert!(read_file(&ssh.join("missing"), &env).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
