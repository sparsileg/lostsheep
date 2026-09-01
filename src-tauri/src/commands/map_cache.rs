use crate::AppState;
use rusqlite::params;
use serde::Serialize;
use tauri::State;

#[derive(Serialize)]
pub struct CacheRegion {
    pub id: i64,
    pub name: String,
    pub polygon_geojson: String,
    pub tile_count: i64,
    pub bytes_on_disk: i64,
}

#[tauri::command]
pub fn list_cache_regions(state: State<AppState>) -> Result<Vec<CacheRegion>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT id, name, polygon_geojson, tile_count, bytes_on_disk FROM cache_regions").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(CacheRegion { id: r.get(0)?, name: r.get(1)?, polygon_geojson: r.get(2)?, tile_count: r.get(3)?, bytes_on_disk: r.get(4)? })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// Frontend (map-view.js) computes the tile x/y/z set covering the drawn
/// polygon and fetches+writes them under the app data dir's tile-cache
/// folder directly via Tauri fs APIs; this command just records the
/// region's bookkeeping row once that's done.
#[tauri::command]
pub fn save_cache_region(state: State<AppState>, name: String, polygon_geojson: String, tile_count: i64, bytes_on_disk: i64) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO cache_regions (name, polygon_geojson, tile_count, bytes_on_disk) VALUES (?1,?2,?3,?4)",
        params![name, polygon_geojson, tile_count, bytes_on_disk],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

#[tauri::command]
pub fn delete_cache_region(state: State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM cache_regions WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_map_data(state: State<AppState>, tag_id: Option<i64>) -> Result<Vec<super::visits::VisitListEntry>, String> {
    // Reuses generate_visit_list's grouping/shape with an unreachable seed
    // so distance is meaningless here — frontend ignores distance_meters
    // in plain map-display mode and just plots lat/long per group.
    super::visits::generate_visit_list(
        state,
        super::visits::GenerateVisitListParams { seed_household_id: 0, tag_id, count: 100000 },
    )
}
