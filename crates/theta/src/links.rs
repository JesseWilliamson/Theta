//! Where the documentation lives, for the Help menu and Settings > About.

pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
pub const WEBSITE: &str = "https://jessewilliamson.github.io/Theta/";
pub const USER_GUIDE: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/blob/main/docs/USER_GUIDE.md");
pub const KEY_REFERENCE: &str = "https://jessewilliamson.github.io/Theta/keybindings.html";
pub const CHANGELOG: &str = "https://jessewilliamson.github.io/Theta/changelog.html";
pub const ISSUES: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/issues");
pub const THIRD_PARTY: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/blob/main/docs/THIRD_PARTY.md");

/// "0.1.0-alpha.9", plus the commit and date when the build recorded them: "0.1.0-alpha.9 (abc123def, 2026-11-12)".
pub fn version_line() -> String {
    let (commit, date) = (env!("THETA_BUILD_COMMIT"), env!("THETA_BUILD_DATE"));
    let version = env!("CARGO_PKG_VERSION");
    match (commit.is_empty(), date.is_empty()) {
        (true, true) => version.to_string(),
        (false, true) => format!("{version} ({commit})"),
        (true, false) => format!("{version} ({date})"),
        (false, false) => format!("{version} ({commit}, {date})"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn links_are_https_and_the_version_line_starts_with_the_version() {
        for l in [super::REPOSITORY, super::WEBSITE, super::USER_GUIDE, super::KEY_REFERENCE, super::CHANGELOG, super::ISSUES, super::THIRD_PARTY] {
            assert!(l.starts_with("https://"), "{l}");
        }
        assert!(super::version_line().starts_with(env!("CARGO_PKG_VERSION")));
    }
}
