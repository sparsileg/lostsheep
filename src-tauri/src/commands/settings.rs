use crate::AppState;
use rusqlite::params;
use std::collections::HashMap;
use tauri::State;

#[tauri::command]
pub fn get_settings(state: State<AppState>) -> Result<HashMap<String, String>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT key, value FROM settings").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// Route start point (#13) is either fully set (label + both coords) or
/// fully unset — a label with no coordinates, or coordinates with no
/// label, is rejected the same way a malformed lat/lon pair would be.
/// Single validation path for this setting; save_settings is the only
/// place values ever get written, so this is the only place it needs to
/// live.
fn validate_route_start(values: &HashMap<String, String>) -> Result<(), String> {
    let label = values.get("routeStartLabel").map(|s| s.trim()).unwrap_or("");
    let lat_raw = values.get("routeStartLat").map(|s| s.trim()).unwrap_or("");
    let lon_raw = values.get("routeStartLon").map(|s| s.trim()).unwrap_or("");

    let filled_count = [!label.is_empty(), !lat_raw.is_empty(), !lon_raw.is_empty()]
        .iter()
        .filter(|&&b| b)
        .count();

    if filled_count == 0 {
        return Ok(());
    }
    if filled_count != 3 {
        return Err("Route start point needs a label and both coordinates, or leave all three blank".to_string());
    }

    let lat: f64 = lat_raw.parse().map_err(|_| "Route start latitude must be a number".to_string())?;
    let lon: f64 = lon_raw.parse().map_err(|_| "Route start longitude must be a number".to_string())?;
    if !(-90.0..=90.0).contains(&lat) {
        return Err("Route start latitude must be between -90 and 90".to_string());
    }
    if !(-180.0..=180.0).contains(&lon) {
        return Err("Route start longitude must be between -180 and 180".to_string());
    }
    Ok(())
}

#[tauri::command]
pub fn save_settings(state: State<AppState>, values: HashMap<String, String>) -> Result<(), String> {
    validate_route_start(&values)?;
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    for (k, v) in values {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![k, v],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn prune_old_deleted_and_logs(state: State<AppState>) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let settings: HashMap<String, String> = get_settings(State::clone(&state))?;
    let deleted_days: i64 = settings.get("deletedRetentionDays").and_then(|v| v.parse().ok()).unwrap_or(365);
    let log_days: i64 = settings.get("logRetentionDays").and_then(|v| v.parse().ok()).unwrap_or(30);

    conn.execute(
        &format!("DELETE FROM deleted_households WHERE deleted_at < datetime('now', '-{deleted_days} days')"),
        [],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        &format!("DELETE FROM logs WHERE created_at < datetime('now', '-{log_days} days')"),
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
