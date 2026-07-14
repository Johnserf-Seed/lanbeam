//! Where LanBeam keeps its files — named after the PRODUCT, not the bundle id.
//!
//! Tauri derives its app directories from the bundle identifier, so `app_data_dir()`
//! and `app_log_dir()` land in `…/app.lanbeam.desktop/…`. The identifier itself is
//! right and STAYS: macOS needs a reverse-DNS `CFBundleIdentifier`, the installer's
//! upgrade path keys off it, and so does `lanbeam://` registration. It is simply the
//! wrong thing to name a folder a human has to open — `%LOCALAPPDATA%\app.lanbeam.desktop\logs`
//! reads like a machine artifact rather than a product, and the app walks users
//! straight to it (设置 → 打开日志目录, 导出诊断).
//!
//! So the folder is `LanBeam`, on the platform's own conventions:
//!
//! | | data (settings · trust · resume state) | logs |
//! |---|---|---|
//! | Windows | `%APPDATA%\LanBeam` | `%LOCALAPPDATA%\LanBeam\logs` |
//! | macOS | `~/Library/Application Support/LanBeam` | `~/Library/Logs/LanBeam` |
//! | Linux | `~/.local/share/LanBeam` | `~/.config/LanBeam/logs` |
//!
//! The DEVICE IDENTITY is not here and never was — the X25519 private key lives in
//! the OS keychain under the service name `LanBeam` (see [`crate::identity`]), so
//! none of this can lose it. What IS here is the trust store, which is why
//! [`migrate_from_identifier_dir`] exists rather than a rename and a shrug.

use std::path::PathBuf;

use tauri::{AppHandle, Manager, Runtime};

/// The one folder name. Not the bundle identifier.
pub const APP_DIR: &str = "LanBeam";

/// The JSON files this app owns. Used by the migration; a file not on this list is
/// not ours and is left alone.
const OWNED: &[&str] = &["settings", "trusted", "partials"];

/// Settings, trust store, resume state. Falls back to the temp dir exactly as the
/// call sites did before, so an unresolvable OS dir degrades instead of panicking.
pub fn data_dir<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    app.path()
        .data_dir()
        .map(|d| d.join(APP_DIR))
        .unwrap_or_else(|_| std::env::temp_dir().join(APP_DIR))
}

/// Rotating log files. Mirrors what Tauri's own `app_log_dir()` does per platform —
/// only the folder name differs.
pub fn log_dir<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    let p = app.path();

    #[cfg(target_os = "macos")]
    let dir = p.home_dir().map(|h| h.join("Library/Logs").join(APP_DIR));

    #[cfg(target_os = "linux")]
    let dir = p.config_dir().map(|c| c.join(APP_DIR).join("logs"));

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let dir = p.local_data_dir().map(|d| d.join(APP_DIR).join("logs"));

    dir.unwrap_or_else(|_| std::env::temp_dir().join(APP_DIR).join("logs"))
}

/// Move an existing install's files out of Tauri's identifier-named folder.
///
/// Without this, renaming the folder would silently abandon the user's TRUST STORE
/// — their whole trust circle, every device they ever verified — plus their settings
/// and any resume state. Losing that to a cosmetic change would be exactly the kind
/// of quiet data loss this codebase has spent a long time hunting down.
///
/// Runs BEFORE the logger exists (settings are read before the log plugin can be
/// configured, and the log path is one of the things that moved), so it cannot log.
/// It returns a line for the caller to log once logging is up — same shape as
/// `settings::load`'s deferred diagnostic.
///
/// Idempotent: a file already present at the destination is never overwritten, so a
/// second run — or a downgrade-then-upgrade — cannot clobber newer data with older.
pub fn migrate_from_identifier_dir<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    let old = app.path().app_data_dir().ok()?;
    let new = data_dir(app);
    if old == new || !old.is_dir() {
        return None;
    }

    move_owned_json(&old, &new)
}

/// The file-moving core, split out so it is testable without a Tauri app.
fn move_owned_json(old: &std::path::Path, new: &std::path::Path) -> Option<String> {
    // Instance-suffixed variants too (`settings-b.json`), or a LANBEAM_INSTANCE test
    // profile would be left behind by a migration that only knew the bare names.
    let entries = std::fs::read_dir(old).ok()?;
    let mut moved = Vec::new();
    let mut failed = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let stem = name.strip_suffix(".json").unwrap_or("");
        let ours = OWNED
            .iter()
            .any(|o| stem == *o || stem.starts_with(&format!("{o}-")));
        if !ours {
            continue;
        }
        let dest = new.join(name);
        if dest.exists() {
            continue; // never overwrite: the destination is the newer truth
        }
        if std::fs::create_dir_all(new).is_err() {
            failed.push(name.to_string());
            continue;
        }
        // Same volume in every real case, so this is a move. A cross-device rename
        // fails rather than half-copying, hence the explicit copy fallback — and if
        // the copy works but the delete doesn't, the file still LIVES at the
        // destination, which is the outcome that matters.
        let ok = std::fs::rename(entry.path(), &dest).is_ok()
            || (std::fs::copy(entry.path(), &dest).is_ok() && {
                let _ = std::fs::remove_file(entry.path());
                true
            });
        if ok {
            moved.push(name.to_string());
        } else {
            failed.push(name.to_string());
        }
    }

    if moved.is_empty() && failed.is_empty() {
        return None;
    }
    let mut note = format!(
        "migrated app data out of the bundle-id folder: {} -> {} ({})",
        old.display(),
        new.display(),
        moved.join(", ")
    );
    if !failed.is_empty() {
        note.push_str(&format!("; COULD NOT MOVE: {}", failed.join(", ")));
    }
    Some(note)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("lanbeam-paths-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write(dir: &std::path::Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    /// The whole point: the TRUST STORE comes along. Renaming the folder without a
    /// migration would quietly abandon the user's trust circle — every device they
    /// ever verified — to make a path prettier.
    #[test]
    fn migration_carries_the_trust_store_and_settings_across() {
        let root = tmp("move");
        let (old, new) = (root.join("app.lanbeam.desktop"), root.join("LanBeam"));
        write(&old, "trusted.json", "TRUST-CIRCLE");
        write(&old, "settings.json", "SETTINGS");
        write(&old, "partials.json", "PARTIALS");
        // A per-instance profile (LANBEAM_INSTANCE=b) must not be left behind.
        write(&old, "trusted-b.json", "INSTANCE-B");
        // Something that is not ours: left strictly alone.
        write(&old, "some-other-app.json", "NOT OURS");

        let note = move_owned_json(&old, &new).expect("something moved");

        assert_eq!(
            std::fs::read_to_string(new.join("trusted.json")).unwrap(),
            "TRUST-CIRCLE",
            "the trust circle arrived intact"
        );
        assert!(new.join("settings.json").exists());
        assert!(new.join("partials.json").exists());
        assert!(new.join("trusted-b.json").exists(), "instance profiles too");
        assert!(!old.join("trusted.json").exists(), "and it was a MOVE");
        assert!(
            old.join("some-other-app.json").exists(),
            "a file that is not ours is not ours to move"
        );
        assert!(note.contains("trusted.json"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Never overwrite the destination. A second run — or a downgrade that wrote to
    /// the old folder, then an upgrade — must not clobber newer data with older.
    #[test]
    fn migration_never_overwrites_what_is_already_there() {
        let root = tmp("keep");
        let (old, new) = (root.join("app.lanbeam.desktop"), root.join("LanBeam"));
        write(&old, "trusted.json", "OLD");
        write(&new, "trusted.json", "NEW");

        move_owned_json(&old, &new);

        assert_eq!(
            std::fs::read_to_string(new.join("trusted.json")).unwrap(),
            "NEW",
            "the destination is the newer truth, and it stays"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A fresh install has nothing to move, and says nothing about it.
    #[test]
    fn migration_is_silent_when_there_is_nothing_to_do() {
        let root = tmp("empty");
        let (old, new) = (root.join("app.lanbeam.desktop"), root.join("LanBeam"));
        std::fs::create_dir_all(&old).unwrap();
        assert!(move_owned_json(&old, &new).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
