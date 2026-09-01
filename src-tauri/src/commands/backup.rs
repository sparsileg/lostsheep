use crate::{crypto, db, AppState};
use serde::Serialize;
use tauri::State;

/// Writes a self-contained encrypted backup file at `dest_path`, re-keyed
/// with a key derived from `passphrase` via Argon2id. The salt is stored
/// in a sidecar `<dest_path>.salt` file (not secret — Argon2id's security
/// comes from the passphrase, salt just prevents rainbow tables) so
/// restore can re-derive the same key later without the OS keychain.
#[tauri::command]
pub fn backup_database(state: State<AppState>, dest_path: String, passphrase: String) -> Result<(), String> {
    let salt = crypto::random_salt_hex();
    let dest_key = crypto::derive_key_hex(&passphrase, &salt).map_err(|e| e.to_string())?;
    let dest = std::path::PathBuf::from(&dest_path);

    db::rekey_copy(&state.db_path, &state.live_key_hex, &dest, &dest_key).map_err(|e| e.to_string())?;
    std::fs::write(format!("{dest_path}.salt"), &salt).map_err(|e| e.to_string())?;

    let conn = state.pool.get().map_err(|e| e.to_string())?;
    super::logs::log(&conn, "info", &format!("backup written to {dest_path}"), None);
    Ok(())
}

#[derive(Serialize)]
pub struct RestoreDiffRow {
    pub kind: String, // "added" | "removed" | "changed"
    pub description: String,
}

#[derive(Serialize)]
pub struct RestorePreview {
    pub rows: Vec<RestoreDiffRow>,
    pub backup_household_count: i64,
    pub current_household_count: i64,
}

fn derive_backup_key(src_path: &str, passphrase: &str) -> Result<String, String> {
    let salt = std::fs::read_to_string(format!("{src_path}.salt"))
        .map_err(|_| "missing .salt sidecar file next to backup — cannot restore".to_string())?;
    crypto::derive_key_hex(passphrase, salt.trim()).map_err(|e| e.to_string())
}

/// Shows a before/after diff without touching the live DB — required by
/// spec so the user can review before committing.
#[tauri::command]
pub fn restore_preview(state: State<AppState>, src_path: String, passphrase: String) -> Result<RestorePreview, String> {
    let key = derive_backup_key(&src_path, &passphrase)?;
    let backup_conn = db::open_with_key(&std::path::PathBuf::from(&src_path), &key).map_err(|e| e.to_string())?;
    let live_conn = state.pool.get().map_err(|e| e.to_string())?;

    let backup_count: i64 = backup_conn.query_row("SELECT count(*) FROM households", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    let live_count: i64 = live_conn.query_row("SELECT count(*) FROM households", [], |r| r.get(0)).map_err(|e| e.to_string())?;

    let mut backup_keys_stmt = backup_conn.prepare("SELECT source_key, first_name, last_name, address_line1 FROM households").map_err(|e| e.to_string())?;
    let backup_rows: Vec<(String, String, String, String)> = backup_keys_stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    let mut live_keys_stmt = live_conn.prepare("SELECT source_key FROM households").map_err(|e| e.to_string())?;
    let live_keys: std::collections::HashSet<String> = live_keys_stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    let mut backup_keyset = std::collections::HashSet::new();
    let mut rows = Vec::new();
    for (key, first, last, addr) in &backup_rows {
        backup_keyset.insert(key.clone());
        if !live_keys.contains(key) {
            rows.push(RestoreDiffRow { kind: "added".into(), description: format!("{first} {last} — {addr}") });
        }
    }

    let mut live_all_stmt = live_conn.prepare("SELECT source_key, first_name, last_name, address_line1 FROM households").map_err(|e| e.to_string())?;
    let live_rows: Vec<(String, String, String, String)> = live_all_stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    for (key, first, last, addr) in &live_rows {
        if !backup_keyset.contains(key) {
            rows.push(RestoreDiffRow { kind: "removed".into(), description: format!("{first} {last} — {addr}") });
        }
    }

    Ok(RestorePreview { rows, backup_household_count: backup_count, current_household_count: live_count })
}

/// Commits the restore: re-keys the backup into the live DB's own
/// SQLCipher key (the one in the OS keychain) and swaps it in atomically.
#[tauri::command]
pub fn restore_commit(state: State<AppState>, src_path: String, passphrase: String) -> Result<(), String> {
    let key = derive_backup_key(&src_path, &passphrase)?;
    let tmp_path = state.db_path.with_extension("restoring");
    db::rekey_copy(&std::path::PathBuf::from(&src_path), &key, &tmp_path, &state.live_key_hex).map_err(|e| e.to_string())?;

    // Swap on disk; caller (frontend) should prompt the user to restart
    // the app afterward so a fresh pool opens against the new file.
    std::fs::rename(&tmp_path, &state.db_path).map_err(|e| e.to_string())?;
    Ok(())
}
