//! Diagnostics (M4.6): the log folder, and the plain-text bundle a bug report
//! attaches.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use tauri::{AppHandle, State};

use crate::error::LanBeamError;
use crate::state::AppState;

/// How much of the current log file the diagnostics bundle includes (tail).
const DIAG_LOG_TAIL_MAX: u64 = 500 * 1024;

/// The app's log directory, created if missing.
#[tauri::command]
pub fn get_log_dir(app: AppHandle) -> Result<String, LanBeamError> {
    let dir = crate::paths::log_dir(&app);
    std::fs::create_dir_all(&dir)?;
    Ok(dir.display().to_string())
}

/// Write a plain-text diagnostics bundle into the log dir and return its path.
///
/// Contents: app version, OS, a settings snapshot, network interfaces, and the
/// tail of the current log file. Deliberately NO key material — the identity's
/// private key (the OS credential store; `identity.key` in the DATA folder on
/// macOS) is never read here, and nothing here reads the data folder; the
/// settings snapshot must likewise stay free of secrets if any are ever added.
#[tauri::command]
pub fn export_diagnostics(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, LanBeamError> {
    use std::fmt::Write as _;

    let dir = crate::paths::log_dir(&app);
    std::fs::create_dir_all(&dir)?;

    let unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stamp = compact_utc_stamp(unix_secs);

    // Writing into a String cannot fail, hence the `let _ =` on every line.
    let mut out = String::new();
    let pkg = app.package_info();
    let _ = writeln!(out, "LanBeam diagnostics — {stamp} UTC");
    let _ = writeln!(out, "app version: {}", pkg.version);
    let _ = writeln!(
        out,
        "os: {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(out, "tcp port: {}", state.tcp_port.load(Ordering::Relaxed));
    match state.download_dir.read() {
        Ok(d) => {
            let _ = writeln!(out, "download dir: {}", d.display());
        }
        Err(_) => {
            let _ = writeln!(out, "download dir: <unavailable: lock poisoned>");
        }
    }

    let _ = writeln!(out, "\n[settings]");
    match state.settings.read() {
        Ok(s) => {
            let _ = writeln!(out, "device_name: {}", s.device_name);
            let _ = writeln!(out, "discoverable: {}", s.discoverable);
            let _ = writeln!(out, "auto_open_folder: {}", s.auto_open_folder);
            let _ = writeln!(out, "log_level: {}", s.log_level);
            let _ = writeln!(out, "recv_policy: {}", s.recv_policy);
            let _ = writeln!(out, "download_dir_override: {:?}", s.download_dir_override);
            let _ = writeln!(out, "port: {}", s.port);
            let _ = writeln!(out, "tray_close: {}", s.tray_close);
            let _ = writeln!(out, "notif_system: {}", s.notif_system);
            let _ = writeln!(out, "autostart: {}", s.autostart);
            let _ = writeln!(out, "iface_filter: {:?}", s.iface_filter);
            let _ = writeln!(out, "hotkey_enabled: {}", s.hotkey_enabled);
            let _ = writeln!(out, "hotkey: {}", s.hotkey);
            let _ = writeln!(out, "verify_hash: {}", s.verify_hash);
            let _ = writeln!(out, "conflict: {}", s.conflict);
            let _ = writeln!(out, "organize: {}", s.organize);
            let _ = writeln!(out, "max_concurrent: {}", s.max_concurrent);
            let _ = writeln!(out, "rate_limit: {}", s.rate_limit);
            let _ = writeln!(out, "clip_share: {}", s.clip_share);
            let _ = writeln!(out, "strip_exif: {}", s.strip_exif);
        }
        // A poisoned lock means another task panicked; still produce a bundle —
        // a diagnostics command failing during a failure is the worst outcome.
        Err(_) => {
            let _ = writeln!(out, "<unavailable: settings lock poisoned>");
        }
    }

    let _ = writeln!(out, "\n[network interfaces]");
    match if_addrs::get_if_addrs() {
        Ok(ifaces) => {
            for i in &ifaces {
                let _ = writeln!(
                    out,
                    "{}: {}{}",
                    i.name,
                    i.addr.ip(),
                    if i.is_loopback() { " (loopback)" } else { "" }
                );
            }
            if ifaces.is_empty() {
                let _ = writeln!(out, "<none>");
            }
        }
        Err(e) => {
            let _ = writeln!(out, "<enumeration failed: {e}>");
        }
    }

    let _ = writeln!(out, "\n[log tail (up to {DIAG_LOG_TAIL_MAX} bytes)]");
    match newest_log_file(&dir) {
        Some(log_path) => {
            let _ = writeln!(out, "file: {}", log_path.display());
            match read_tail(&log_path, DIAG_LOG_TAIL_MAX) {
                Ok(tail) => out.push_str(&tail),
                Err(e) => {
                    let _ = writeln!(out, "<read failed: {e}>");
                }
            }
        }
        None => {
            let _ = writeln!(out, "<no log file found>");
        }
    }

    let path = dir.join(format!("lanbeam-diag-{stamp}.txt"));
    std::fs::write(&path, out)?;
    log::info!("diagnostics bundle written to {}", path.display());
    Ok(path.display().to_string())
}

/// Most recently modified `*.log` in `dir` — found by mtime rather than by a
/// hard-coded name so it keeps working if the log plugin's naming changes.
fn newest_log_file(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, p)| p)
}

/// Read at most the last `max` bytes of a file (lossy UTF-8: a tail can start
/// mid-multibyte-sequence and log content is diagnostic, not canonical).
fn read_tail(path: &Path, max: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    if len > max {
        f.seek(SeekFrom::Start(len - max))?;
    }
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Format unix seconds as a compact UTC stamp `YYYYMMDD-HHMMSS` (diagnostics
/// file names). Hand-rolled via [`civil_from_days`] to avoid pulling in a date
/// crate for a single call site.
fn compact_utc_stamp(unix_secs: u64) -> String {
    let (y, m, d) = civil_from_days((unix_secs / 86_400) as i64);
    let secs = unix_secs % 86_400;
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// Days since 1970-01-01 → (year, month, day) in the proleptic Gregorian
/// calendar — Howard Hinnant's `civil_from_days` algorithm, exact for the
/// whole i64 day range we can ever see.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known calendar anchors: the epoch itself, a famous round number
    /// (1e9 = 2001-09-09 01:46:40 UTC), and a leap day.
    #[test]
    fn civil_from_days_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_723 + 31 + 28), (2024, 2, 29)); // leap day
        assert_eq!(civil_from_days(-1), (1969, 12, 31)); // pre-epoch stays exact
    }

    #[test]
    fn compact_utc_stamp_formats_and_pads() {
        assert_eq!(compact_utc_stamp(0), "19700101-000000");
        assert_eq!(compact_utc_stamp(1_000_000_000), "20010909-014640");
    }

    /// The tail must honor the byte cap and keep the END of the file.
    #[test]
    fn read_tail_caps_and_keeps_the_end() {
        let dir = std::env::temp_dir().join(format!("lanbeam-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.log");
        std::fs::write(&p, "AAAAAAAAAAtail-marker").unwrap();
        let tail = read_tail(&p, 11).unwrap();
        assert_eq!(tail, "tail-marker");
        // a cap larger than the file returns everything
        let all = read_tail(&p, 10_000).unwrap();
        assert_eq!(all, "AAAAAAAAAAtail-marker");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only `*.log` files count, and the most recently modified one wins.
    #[test]
    fn newest_log_file_picks_latest_log() {
        let dir = std::env::temp_dir().join(format!("lanbeam-logs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.log"), "old").unwrap();
        std::fs::write(dir.join("ignore.txt"), "not a log").unwrap();
        // ensure a strictly later mtime on filesystems with coarse timestamps
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(dir.join("new.log"), "new").unwrap();
        let got = newest_log_file(&dir).expect("a log file exists");
        assert_eq!(got.file_name().unwrap(), "new.log");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
