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
    // i64 throughout: page arrives as u32 with no upper bound from the
    // IPC caller, and (page - 1) * page_size in u32 could overflow for a
    // page above ~4.3 million, either panicking (debug) or wrapping to a
    // silently wrong offset (release). i64 has no realistic overflow risk
    // at these magnitudes (#33).
    let page_size: i64 = (page_size.clamp(1, 1000)) as i64;
    let offset: i64 = (page.max(1) as i64 - 1) * page_size;
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
