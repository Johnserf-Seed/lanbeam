//! Paste-to-send: what is on the clipboard that could go to a device.

use std::path::PathBuf;
use std::time::SystemTime;

use tauri::{AppHandle, Manager};

use crate::clipboard::{self, ClipboardPayload};
use crate::error::LanBeamError;

/// Where clipboard images are written so they can be sent as files.
pub(crate) fn clipboard_root(app: &AppHandle) -> PathBuf {
    app.path()
        .app_cache_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("lanbeam")
        .join("clipboard")
}

/// Read the system clipboard for something to send: copied files, an image
/// (returned as the path of a PNG written for it, with a small preview), or
/// text. Files exclude the other two; an image and text can both be there.
/// Reads only — nothing is sent, and the clipboard is left as it was.
#[tauri::command]
pub async fn read_clipboard(app: AppHandle) -> Result<ClipboardPayload, LanBeamError> {
    let root = clipboard_root(&app);
    // Encoding a large screenshot takes a moment; keep it off the async runtime.
    tauri::async_runtime::spawn_blocking(move || {
        let payload = clipboard::read(&root)?;
        // After the write, so the image just pasted is the newest one kept.
        clipboard::sweep(
            &root,
            SystemTime::now(),
            clipboard::MAX_AGE,
            clipboard::KEEP,
        );
        Ok(payload)
    })
    .await
    .map_err(|e| LanBeamError::Io(format!("clipboard read task failed: {e}")))?
}
