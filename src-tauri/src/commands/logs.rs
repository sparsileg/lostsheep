use crate::AppState;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::atomic::{AtomicU8, Ordering};
use tauri::State;

/// Severity ordinal — lower is more severe. Matches schema.sql's CHECK
/// constraint values (error/warning/info/debug). An unrecognized string
/// (shouldn't happen; every call site is a hardcoded literal) is treated
/// as "info" — neither silently dropped nor allowed to bypass the
/// configured minimum.
fn level_ordinal(level: &str) -> u8 {
    match level {
        "error" => 0,
        "warning" => 1,
        "info" => 2,
        "debug" => 3,
        _ => 2,
    }
}

/// Issue #76: the "Minimum log level" Settings control used to have no
/// effect on what got written — every caller wrote unconditionally, and
/// the setting only changed which Log Viewer checkboxes started ticked.
/// This is the write-time filter the label actually promises.
///
/// A process-wide atomic, not a per-call settings query: log() runs
/// inside open transactions and hot loops (generate_visit_list's #66
/// timing diagnostic, import's per-item logging) where a settings lookup
/// per call would reintroduce exactly the kind of hidden per-call cost
/// #51/#66 spent real effort eliminating elsewhere. It also has to work
/// when log() is called before AppState exists, which rules out
/// anything backed by AppState. Starts at "debug" (log everything) so
/// nothing is silently lost between process start and the first
/// refresh_min_level_cache() call.
static MIN_LOG_LEVEL_ORDINAL: AtomicU8 = AtomicU8::new(3);

/// Loads the configured minimum level into the cache above. Called once
/// at startup (main.rs, right after the main pool opens) and again every
/// time Settings saves (save_settings, settings.rs) so a live change
/// takes effect immediately rather than waiting for the next launch. A
/// missing key (fresh database, never configured) or a read error leaves
/// the cached value as whatever it already was — defaults to "log
/// everything" until a real value is ever saved, which is the same
/// direction of failure as this issue's own "errors must always be
/// written" acceptance criterion.
pub fn refresh_min_level_cache(conn: &Connection) {
    if let Ok(level) = conn.query_row(
        "SELECT value FROM settings WHERE key = 'logLevel'",
        [],
        |r| r.get::<_, String>(0),
    ) {
        MIN_LOG_LEVEL_ORDINAL.store(level_ordinal(&level), Ordering::Relaxed);
    }
}

/// Internal write helper — called from other command modules on
/// significant operations (import, delete, backup, restore, etc).
pub fn log(conn: &Connection, level: &str, message: &str, context: Option<&str>) {
    // Errors are always written regardless of the configured minimum
    // (issue #76 acceptance criterion) — a write-time filter must never
    // be able to hide the one thing an operator most needs to see.
    if level != "error" && level_ordinal(level) > MIN_LOG_LEVEL_ORDINAL.load(Ordering::Relaxed) {
        return;
    }
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
