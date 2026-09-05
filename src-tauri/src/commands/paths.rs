// commands/paths.rs — issue #32: every path-taking command (backup,
// restore, CSV/PDF import, road .pbf ingest) previously trusted a plain
// frontend-supplied String with no server-side check at all. Policy
// (confirmed with Stan): reads are allowed anywhere under the user's
// home directory; writes are allowed only inside the configured
// backupFolder setting. This module is the one place that boundary is
// enforced — every command routes through it rather than five separate
// copies of the same check.
//
// Canonicalize-then-check, not string-prefix-check: `std::fs::canonicalize`
// resolves `..` and symlinks, so a naive `raw.starts_with(home)` string
// test (which `~/backups/../../etc/x` would pass) is not used anywhere
// here.

use crate::AppState;
use std::path::{Path, PathBuf};
use tauri::State;

fn home_dir() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "could not determine the user's home directory".to_string())
}

/// Resolves a read-only path (restore source, CSV/PDF import, road .pbf
/// ingest) and confirms it falls under the user's home directory. The
/// file must exist for this to succeed, which is fine — every caller
/// here is about to open it anyway.
pub fn resolve_read_path(raw: &str) -> Result<PathBuf, String> {
    let home = home_dir()?;
    let home = std::fs::canonicalize(&home)
        .map_err(|e| format!("could not resolve home directory: {e}"))?;
    let resolved = std::fs::canonicalize(raw)
        .map_err(|e| format!("could not open {raw}: {e}"))?;
    if !resolved.starts_with(&home) {
        return Err(format!("{raw} is outside the allowed home directory"));
    }
    Ok(resolved)
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
        return Err(format!("a file already exists at {}: choose a different name", dest.display()));
    }

    Ok(dest)
}
