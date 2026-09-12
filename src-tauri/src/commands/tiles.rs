// commands/tiles.rs — #72: disk-backed cache for OpenStreetMap tile
// images, so a tile already viewed once is never re-requested from
// tile.openstreetmap.org. The actual HTTP fetch happens in the webview
// (map-view.js), which already has CSP permission to reach the tile
// domain — this module only does local disk I/O: read a cached tile,
// write a freshly-fetched one, report when the cache was last touched,
// and wipe it on request. No TTL/expiry: Stan's call — a manual
// "Refresh Map" button in Settings is the only way the cache empties.
//
// Lives outside both SQLite databases on purpose — this is bulk binary
// image data, not household data, and doesn't belong in either
// lost-sheep.db or roads.db.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

fn tile_cache_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("tile-cache"))
}

fn tile_path(cache_dir: &std::path::Path, z: u32, x: u32, y: u32) -> PathBuf {
    cache_dir.join(z.to_string()).join(x.to_string()).join(format!("{y}.png"))
}

fn last_updated_path(cache_dir: &std::path::Path) -> PathBuf {
    cache_dir.join("last-updated.txt")
}

#[tauri::command]
pub fn get_cached_tile(app: AppHandle, z: u32, x: u32, y: u32) -> Result<Option<Vec<u8>>, String> {
    let cache_dir = tile_cache_dir(&app)?;
    match fs::read(tile_path(&cache_dir, z, x, y)) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Called by the frontend right after it fetches a tile the cache didn't
/// have. Also stamps last-updated.txt with the current time — the single
/// value the Settings modal's "Last map update" label reads, so that
/// label never has to scan the whole cache directory to find the most
/// recently written tile.
#[tauri::command]
pub fn save_cached_tile(app: AppHandle, z: u32, x: u32, y: u32, bytes: Vec<u8>) -> Result<(), String> {
    let cache_dir = tile_cache_dir(&app)?;
    let path = tile_path(&cache_dir, z, x, y);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&path, &bytes).map_err(|e| e.to_string())?;

    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_secs();
    let mut f = fs::File::create(last_updated_path(&cache_dir)).map_err(|e| e.to_string())?;
    write!(f, "{now}").map_err(|e| e.to_string())?;
    Ok(())
}

/// Unix seconds of the most recent tile fetch, or None if nothing has
/// ever been cached (including right after clear_tile_cache).
#[tauri::command]
pub fn get_tile_cache_status(app: AppHandle) -> Result<Option<i64>, String> {
    let cache_dir = tile_cache_dir(&app)?;
    match fs::read_to_string(last_updated_path(&cache_dir)) {
        Ok(s) => s.trim().parse::<i64>().map(Some).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// The Settings modal's "Refresh Map" button. Wipes the whole cache
/// (including last-updated.txt) rather than any per-tile or per-area
/// selection — every tile simply re-fetches and re-caches the next time
/// it's actually viewed, same as a first-ever cold cache.
#[tauri::command]
pub fn clear_tile_cache(app: AppHandle) -> Result<(), String> {
    let cache_dir = tile_cache_dir(&app)?;
    match fs::remove_dir_all(&cache_dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
