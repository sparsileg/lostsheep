// commands/paths.rs — issue #32: every path-taking command (backup,
// restore, CSV/PDF import, road .pbf ingest) previously trusted a plain
// frontend-supplied String with no server-side check at all. Policy
// (confirmed with Stan): writes are allowed only inside the configured
// backupFolder setting, enforced by resolve_write_dest below. This module
// is the one place that boundary is enforced — every command routes
// through it rather than five separate copies of the same check.
//
// Canonicalize-then-check, not string-prefix-check: `std::fs::canonicalize`
// resolves `..` and symlinks, so a naive string-prefix test is not used
// anywhere here.
//
// Issue #87 Fix 2: resolve_read_path used to also require the resolved
// path fall under dirs::home_dir() — broke pCloudSync on Windows (`P:\`
// drive, outside home) though the identical setup worked on Linux (pCloud
// there is a home subfolder). Dropped (Option B, confirmed with Stan):
// every read caller already routes the raw path through Tauri's native
// file-picker dialog before it ever reaches here, so the dialog is the
// real boundary on what a user can hand this function, not a directory
// check on top of it.

use crate::AppState;
use std::path::{Path, PathBuf};
use tauri::State;

/// Resolves a read-only path (restore source, CSV/PDF import, road .pbf
/// ingest). The file must exist for this to succeed, which is fine —
/// every caller here is about to open it anyway.
pub fn resolve_read_path(raw: &str) -> Result<PathBuf, String> {
    std::fs::canonicalize(raw).map_err(|e| format!("could not open {raw}: {e}"))
}

/// Resolves a backup destination and confirms its parent directory is
/// exactly the configured `backupFolder` setting — reading straight from
/// the database, not trusting anything the frontend claims that folder
/// to be. Also refuses to overwrite an existing file (previously
/// `File::create` truncated silently; two backups on the same day used
/// to destroy each other with no warning).
pub fn resolve_write_dest(state: &State<AppState>, raw: &str) -> Result<PathBuf, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let folder: String = conn
        .query_row("SELECT value FROM settings WHERE key = 'backupFolder'", [], |r| r.get(0))
        .map_err(|_| "backup folder is not set — set it in Settings before backing up".to_string())?;
    if folder.trim().is_empty() {
        return Err("backup folder is not set — set it in Settings before backing up".to_string());
    }

    let allowed_root = std::fs::canonicalize(&folder)
        .map_err(|e| format!("configured backup folder is missing or inaccessible: {e}"))?;

    let raw_path = Path::new(raw);
    let parent = raw_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| format!("{raw} has no parent directory"))?;
    let parent = std::fs::canonicalize(parent)
        .map_err(|e| format!("could not resolve destination folder for {raw}: {e}"))?;

    if parent != allowed_root {
        return Err(format!(
            "backups may only be written to the configured backup folder ({})",
            allowed_root.display()
        ));
    }

    let file_name = raw_path
        .file_name()
        .ok_or_else(|| format!("{raw} has no filename"))?;
    let dest = parent.join(file_name);

    if dest.exists() {
        // Issue #73: "choose a different name" was written for an API
        // caller — the Backup dialog has no filename field, so it told
        // the user to do something the UI didn't let them do. Minute-
        // granularity timestamps (backup-restore.js's backupTimestamp())
        // make an actual same-day collision rare now; when one still
        // happens (two backups within the same minute, or a leftover
        // file from a previous run), point at something doable from
        // where the user is instead.
        return Err(format!(
            "a backup already exists at {} — delete or move it, or try again in a minute",
            dest.display()
        ));
    }

    Ok(dest)
}
