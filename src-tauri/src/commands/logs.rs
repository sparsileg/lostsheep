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
pub fn get_logs(state: State<AppState>, levels: Vec<String>, page: u32, page_size: u32) -> Result<Vec<LogEntry>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let page_size: i64 = (page_size.clamp(1, 1000)) as i64;
    let offset: i64 = (page.max(1) as i64 - 1) * page_size;

    // Issue #27 (#4): the old single-`level: Option<String>` version had
    // a real bug in its no-filter branch — SQL referenced ?2/?3 but only
    // ?1/?2 were ever bound, so an unfiltered call always errored (latent
    // only because the frontend never actually called it that way).
    // Issue #27 (#5): the Log Viewer used to call this once per checked
    // level and merge client-side, so "page 2" meant the second page of
    // each level independently — not the second page of anything
    // coherent. A single query over every requested level, ordered and
    // paginated once, fixes both: empty `levels` means no filter at all.
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let where_clause = if levels.is_empty() {
        String::new()
    } else {
        let placeholders: Vec<String> = levels.iter().enumerate().map(|(i, _)| format!("?{}", i + 1)).collect();
        for lvl in &levels {
            binds.push(Box::new(lvl.clone()));
        }
        format!("WHERE level IN ({})", placeholders.join(","))
    };

    let limit_idx = binds.len() + 1;
    let offset_idx = binds.len() + 2;
    let sql = format!(
        "SELECT id, level, message, context, created_at FROM logs {where_clause} \
         ORDER BY id DESC LIMIT ?{limit_idx} OFFSET ?{offset_idx}"
    );
    binds.push(Box::new(page_size));
    binds.push(Box::new(offset));

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    let rows = stmt
        .query_map(param_refs.as_slice(), |r| {
            Ok(LogEntry { id: r.get(0)?, level: r.get(1)?, message: r.get(2)?, context: r.get(3)?, created_at: r.get(4)? })
        })
        .map_err(|e| e.to_string())?;
    let out: Vec<LogEntry> = rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    Ok(out)
}
