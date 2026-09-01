use crate::AppState;
use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::State;

/// Internal write helper — called from other command modules on
/// significant operations (import, delete, backup, restore, etc).
pub fn log(conn: &Connection, level: &str, message: &str, context: Option<&str>) {
    let _ = conn.execute(
        "INSERT INTO logs (level, message, context) VALUES (?1, ?2, ?3)",
        params![level, message, context],
    );
}

#[derive(Serialize)]
pub struct LogEntry {
    pub id: i64,
    pub level: String,
    pub message: String,
    pub context: Option<String>,
    pub created_at: String,
}

#[tauri::command]
pub fn get_logs(state: State<AppState>, level: Option<String>, page: u32, page_size: u32) -> Result<Vec<LogEntry>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let offset = (page.max(1) - 1) * page_size.clamp(1, 1000);
    let sql = match &level {
        Some(_) => "SELECT id, level, message, context, created_at FROM logs WHERE level = ?1 ORDER BY id DESC LIMIT ?2 OFFSET ?3",
        None => "SELECT id, level, message, context, created_at FROM logs ORDER BY id DESC LIMIT ?2 OFFSET ?3",
    };
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let mapper = |r: &rusqlite::Row| {
        Ok(LogEntry { id: r.get(0)?, level: r.get(1)?, message: r.get(2)?, context: r.get(3)?, created_at: r.get(4)? })
    };
    let rows = match &level {
        Some(lvl) => stmt.query_map(params![lvl, page_size, offset], mapper),
        None => stmt.query_map(params![page_size, offset], mapper),
    }
    .map_err(|e| e.to_string())?;
    let out: Vec<LogEntry> = rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    Ok(out)
}
