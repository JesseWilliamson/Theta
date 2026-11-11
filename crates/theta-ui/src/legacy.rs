//! One-time migrations from the product's former name, Quarry.
//!
//! Users upgrading from 0.1.0-alpha.3 or earlier have their configuration under
//! `…/quarry/` and a settings file that names the "Quarry Dark" / "Quarry Light"
//! themes. Both are carried over silently so an upgrade loses nothing; nothing here
//! runs for a fresh install.

use std::io;
use std::path::Path;

/// The current name of a theme that was renamed from Quarry, `None` for any other theme.
/// Case-insensitive because `settings.json` is hand-edited.
pub fn renamed_theme(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "quarry dark" => Some(crate::themes::THETA_DARK),
        "quarry light" => Some(crate::themes::THETA_LIGHT),
        _ => None,
    }
}

/// What [`migrate_dir`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Migrated {
    /// Nothing to do: no old directory, or the new one already exists.
    No,
    Moved,
    /// A rename across filesystems is impossible, so the tree was copied and the old one
    /// removed best-effort.
    Copied,
}

/// Move `old` to `new` if `old` exists and `new` does not. The new directory winning
/// whenever it exists means a second run (or a user who already started the new version)
/// can never overwrite anything.
pub fn migrate_dir(old: &Path, new: &Path) -> io::Result<Migrated> {
    if !old.is_dir() || new.exists() {
        return Ok(Migrated::No);
    }
    if let Some(parent) = new.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(old, new).is_ok() {
        return Ok(Migrated::Moved);
    }
    // Copy into a sibling first so a failure half-way never leaves a partial `new`
    // that would make the next start think the migration was done.
    let staging = new.with_extension("migrating");
    let _ = std::fs::remove_dir_all(&staging);
    copy_tree(old, &staging).inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&staging);
    })?;
    std::fs::rename(&staging, new)?;
    let _ = std::fs::remove_dir_all(old);
    Ok(Migrated::Copied)
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Move the pre-rename config and tile-cache directories to their new places and print a
/// notice for each. Skipped for a location the user overrode with `THETA_CONFIG_DIR` /
/// `THETA_CACHE_DIR`: an explicit path is theirs to manage.
pub fn migrate_app_dirs() {
    if std::env::var_os("THETA_CONFIG_DIR").is_none_or(|d| d.is_empty())
        && let Some(base) = dirs::config_dir()
    {
        report("configuration", &base.join("quarry"), &base.join("theta"));
    }
    if std::env::var_os("THETA_CACHE_DIR").is_none_or(|d| d.is_empty())
        && let Some(base) = dirs::cache_dir()
    {
        report("tile cache", &base.join("quarry"), &base.join("theta"));
    }
}

fn report(what: &str, old: &Path, new: &Path) {
    match migrate_dir(old, new) {
        Ok(Migrated::No) => {}
        Ok(m) => eprintln!(
            "theta: {} the {what} from {} to {} (Theta was called Quarry before 0.1.0-alpha.4)",
            if m == Migrated::Moved { "moved" } else { "copied" },
            old.display(),
            new.display()
        ),
        Err(e) => eprintln!("theta: could not migrate the {what} from {}: {e}", old.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_theme_names_map_to_the_new_ones() {
        assert_eq!(renamed_theme("Quarry Dark"), Some("Theta Dark"));
        assert_eq!(renamed_theme("quarry light"), Some("Theta Light"));
        assert_eq!(renamed_theme("Theta Dark"), None);
        assert_eq!(renamed_theme("Tokyo Night"), None);
    }

    fn seed(dir: &Path) {
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        std::fs::write(dir.join("settings.json"), r#"{"theme":"Quarry Dark"}"#).unwrap();
        std::fs::write(dir.join("themes").join("mine.json"), "{}").unwrap();
    }

    #[test]
    fn moves_the_old_directory_when_the_new_one_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let (old, new) = (tmp.path().join("quarry"), tmp.path().join("theta"));
        seed(&old);
        assert_eq!(migrate_dir(&old, &new).unwrap(), Migrated::Moved);
        assert!(!old.exists());
        assert!(new.join("settings.json").is_file() && new.join("themes/mine.json").is_file());
    }

    #[test]
    fn never_touches_an_existing_new_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let (old, new) = (tmp.path().join("quarry"), tmp.path().join("theta"));
        seed(&old);
        std::fs::create_dir_all(&new).unwrap();
        assert_eq!(migrate_dir(&old, &new).unwrap(), Migrated::No);
        assert!(old.join("settings.json").is_file());
        assert!(!new.join("settings.json").exists());
    }

    #[test]
    fn nothing_to_do_without_an_old_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let (old, new) = (tmp.path().join("quarry"), tmp.path().join("theta"));
        assert_eq!(migrate_dir(&old, &new).unwrap(), Migrated::No);
        assert!(!new.exists());
    }

    #[test]
    fn copy_fallback_reproduces_the_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let (old, staged) = (tmp.path().join("quarry"), tmp.path().join("copy"));
        seed(&old);
        copy_tree(&old, &staged).unwrap();
        assert_eq!(std::fs::read_to_string(staged.join("themes/mine.json")).unwrap(), "{}");
    }
}
