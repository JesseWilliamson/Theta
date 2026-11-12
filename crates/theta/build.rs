//! Records the commit and date this binary was built from, for Settings > About. Done here, at build time,
//! so the application never runs `git` itself; without a git checkout (a source tarball) both stay empty
//! and About simply leaves them out.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

fn main() {
    let commit = git(&["rev-parse", "--short=9", "HEAD"]).unwrap_or_default();
    let date = git(&["log", "-1", "--format=%cs"]).unwrap_or_default();
    println!("cargo:rustc-env=THETA_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=THETA_BUILD_DATE={date}");
    // A new commit or checkout changes HEAD; that is when the values go stale.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=build.rs");
}
