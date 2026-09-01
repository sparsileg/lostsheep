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

#[tauri::command]
pub fn save_settings(state: State<AppState>, values: HashMap<String, String>) -> Result<(), String> {
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
