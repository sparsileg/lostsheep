use crate::AppState;
use rusqlite::params;
use serde::Serialize;
use std::collections::HashMap;
use tauri::State;

/// Retention is now a fixed dropdown in the UI (30/90/180/365 days) rather
/// than free-text — but save_settings has no whitelist otherwise and any
/// value can still reach it over IPC, so this is validated here too, not
/// just constrained in the frontend. Closes the "0 means delete
/// everything" and "negative value silently no-ops" cases (#28) by
/// construction: neither is a member of this set.
pub const ALLOWED_RETENTION_DAYS: [i64; 4] = [1, 7, 14, 30];

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

fn validate_retention(values: &HashMap<String, String>) -> Result<(), String> {
    for key in ["deletedRetentionDays", "logRetentionDays"] {
        if let Some(raw) = values.get(key) {
            let n: i64 = raw.trim().parse().map_err(|_| format!("{key} must be a number"))?;
            if !ALLOWED_RETENTION_DAYS.contains(&n) {
                return Err(format!("{key} must be one of 1, 7, 14, or 30 days"));
            }
        }
    }
    Ok(())
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
    validate_retention(&values)?;
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut keys: Vec<String> = values.keys().cloned().collect();
    keys.sort();
    for (k, v) in values {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![k, v],
        )
        .map_err(|e| e.to_string())?;
    }
    super::logs::log(&conn, "info", &format!("settings saved: {}", keys.join(", ")), None);
    Ok(())
}

#[derive(Serialize)]
pub struct PruneImpact {
    pub deleted_households: i64,
    pub logs: i64,
}

/// Counts what prune_old_deleted_and_logs *would* remove under the given
/// retention values, without deleting anything. settings-modal.js calls
/// this with the pending (not-yet-saved) values before Save commits, so
/// the confirmation the person sees reflects the choice they're about to
/// make, not the retention that's currently in effect (#28).
#[tauri::command]
pub fn preview_prune_impact(state: State<AppState>, deleted_days: i64, log_days: i64) -> Result<PruneImpact, String> {
    if !ALLOWED_RETENTION_DAYS.contains(&deleted_days) || !ALLOWED_RETENTION_DAYS.contains(&log_days) {
        return Err("retention values must be one of 1, 7, 14, or 30 days".to_string());
    }
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let deleted_modifier = format!("-{deleted_days} days");
    let log_modifier = format!("-{log_days} days");
    let deleted_households: i64 = conn
        .query_row(
            "SELECT count(*) FROM deleted_households WHERE deleted_at < datetime('now', ?1)",
            params![deleted_modifier],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let logs: i64 = conn
        .query_row(
            "SELECT count(*) FROM logs WHERE created_at < datetime('now', ?1)",
            params![log_modifier],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(PruneImpact { deleted_households, logs })
}

#[derive(Serialize)]
pub struct PruneResult {
    pub deleted_households: i64,
    pub logs: i64,
}

/// No longer called from Save (#28) — retention is a policy setting, not
/// a trigger. This needs to be called at application startup instead,
/// where an unattended sweep is expected housekeeping rather than a
/// side effect of an unrelated button. NOT YET WIRED UP: that call needs
/// to go in main.rs's setup, after db::open_pool() — main.rs wasn't part
/// of this patch. Left as a #[tauri::command] (not made private) so it
/// stays available for issue #28's suggested on-demand "Prune Now"
/// hamburger-menu entry later, with its own confirmation.
/// The actual sweep, over a plain connection — extracted so main.rs can
/// call this directly at startup without needing a managed `State`
/// (which doesn't exist yet that early in setup()). The #[tauri::command]
/// below is now a thin wrapper over this, kept for the on-demand "Prune
/// Now" hamburger-menu entry (#28) that still goes through IPC.
pub fn run_prune(conn: &rusqlite::Connection) -> Result<PruneResult, String> {
    let settings: HashMap<String, String> = {
        let mut stmt = conn.prepare("SELECT key, value FROM settings").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    // Falls back to the default if the stored value somehow isn't one of
    // the allowed values (e.g. a database from before this fix that still
    // has a stray "0" in it) — defensive, since this runs unattended.
    let deleted_days: i64 = settings
        .get("deletedRetentionDays")
        .and_then(|v| v.parse().ok())
        .filter(|n| ALLOWED_RETENTION_DAYS.contains(n))
        .unwrap_or(30);
    let log_days: i64 = settings
        .get("logRetentionDays")
        .and_then(|v| v.parse().ok())
        .filter(|n| ALLOWED_RETENTION_DAYS.contains(n))
        .unwrap_or(30);

    // Bound parameters (#28) — the modifier string is still built in Rust,
    // but it now reaches SQLite as a parameter, not spliced into the SQL
    // text. The previous version's safety depended entirely on the
    // .parse::<i64>() two lines above the format!; this doesn't depend on
    // that at all.
    let deleted_modifier = format!("-{deleted_days} days");
    let log_modifier = format!("-{log_days} days");

    let deleted_households = conn
        .execute(
            "DELETE FROM deleted_households WHERE deleted_at < datetime('now', ?1)",
            params![deleted_modifier],
        )
        .map_err(|e| e.to_string())? as i64;
    let logs = conn
        .execute(
            "DELETE FROM logs WHERE created_at < datetime('now', ?1)",
            params![log_modifier],
        )
        .map_err(|e| e.to_string())? as i64;
    Ok(PruneResult { deleted_households, logs })
}

#[tauri::command]
pub fn prune_old_deleted_and_logs(state: State<AppState>) -> Result<PruneResult, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    run_prune(&conn)
}
