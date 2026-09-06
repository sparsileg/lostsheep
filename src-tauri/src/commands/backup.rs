use crate::{crypto, db, AppState};
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use tauri::State;

const DB_ENTRY: &str = "lost-sheep.db";
const SALT_ENTRY: &str = "lost-sheep.salt";

/// Deletes its wrapped path on drop — used for the scratch DB files this
/// module writes to disk (SQLCipher's sqlcipher_export needs a real path,
/// not an in-memory buffer) so a cleanup step can't be forgotten on any
/// early-return `?` path.
struct TmpFile(std::path::PathBuf);
impl Drop for TmpFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn tmp_path(prefix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("{prefix}-{}.tmp", uuid::Uuid::new_v4()))
}

// Issue #26: not a security hash — just enough to bind a preview to "this
// exact file, unchanged since preview ran". Path alone would let a
// different file dropped at the same path slip through; size+mtime
// catches that without re-reading (and re-capping) the whole archive.
fn preview_token(src_path: &str, meta: &std::fs::Metadata) -> String {
    let mtime_secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut hasher = DefaultHasher::new();
    src_path.hash(&mut hasher);
    meta.len().hash(&mut hasher);
    mtime_secs.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Writes a self-contained, single-file backup at `dest_path`: a zip
/// archive (uncompressed — the DB is already SQLCipher-encrypted, so
/// there's nothing to gain from a compression codec) containing the
/// re-keyed database and the Argon2id salt needed to re-derive its key
/// from the passphrase later. Previously these were two separate files
/// (a `.db` and a sidecar `.salt`); bundling them removes a way for the
/// two to get separated or for only one to actually land on disk.
#[tauri::command]
pub fn backup_database(state: State<AppState>, dest_path: String, passphrase: String) -> Result<String, String> {
    let result = (|| -> Result<String, String> {
    // Issue #32: dest_path is a plain frontend-supplied String — never
    // trusted as-is. Must resolve to a not-yet-existing file directly
    // inside the configured backupFolder setting, read straight from the
    // DB here (not from anything the caller claims).
    let dest_path = super::paths::resolve_write_dest(&state, &dest_path)?
        .to_string_lossy()
        .to_string();

    let salt = crypto::random_salt_hex();
    let dest_key = crypto::derive_key_hex(&passphrase, &salt).map_err(|e| e.to_string())?;

    let tmp_db = TmpFile(tmp_path("lost-sheep-backup"));
    db::rekey_copy(&state.db_path, &state.live_key_hex, &tmp_db.0, &dest_key).map_err(|e| e.to_string())?;

    // The road graph (issue #7) is ingested from a user-held, easily
    // re-fetched .pbf and can run 10x+ larger than the rest of the DB —
    // no reason to bloat every backup with data that isn't backup-worthy
    // in the first place. Stripped from this copy only, after the export,
    // so the live DB itself is untouched.
    strip_road_graph(&tmp_db.0, &dest_key)?;
    strip_display_only_settings(&tmp_db.0, &dest_key)?;

    write_backup_zip(&dest_path, &tmp_db.0, &salt)?;

    // Confirm the file actually landed before telling the user it
    // succeeded — this directly addresses backups that appeared to
    // complete but weren't found at the destination afterward.
    let meta = std::fs::metadata(&dest_path).map_err(|e| {
        format!("backup appears to have failed — no file found at {dest_path} after writing: {e}")
    })?;
    if meta.len() == 0 {
        return Err(format!("backup file at {dest_path} was created but is empty"));
    }

    let conn = state.pool.get().map_err(|e| e.to_string())?;
    super::logs::log(&conn, "info", &format!("backup written to {dest_path} ({} bytes)", meta.len()), None);
    Ok(dest_path)
    })();

    // Issue #27: failed backups previously left no trace at all — the
    // error string went to the frontend's 4-second message bar and
    // nowhere else. Every early-return `?` above is now covered without
    // needing to touch each one individually.
    if let Err(e) = &result {
        if let Ok(conn) = state.pool.get() {
            super::logs::log(&conn, "error", &format!("backup failed: {e}"), None);
        }
    }
    result
}

/// Deletes the road graph from a backup-bound *copy* of the DB and
/// reclaims the freed space — the copy is a scratch file the caller
/// deletes when done (TmpFile), never the live DB, so this never touches
/// what's actually ingested via ingest_road_database.
fn strip_road_graph(path: &std::path::Path, key_hex: &str) -> Result<(), String> {
    let conn = db::open_with_key(&path.to_path_buf(), key_hex).map_err(|e| e.to_string())?;
    conn.execute_batch("DELETE FROM road_edges; DELETE FROM road_nodes; VACUUM;")
        .map_err(|e| e.to_string())?;
    Ok(())
}

// Issue #40: showRoadsOverlay/showRouteOverlay are this-machine display
// preferences, not congregation data — stripped from the backup copy
// only, same pattern as strip_road_graph above and how backupFolder is
// already excluded (paths.rs).
const DISPLAY_ONLY_SETTINGS_KEYS: [&str; 2] = ["showRoadsOverlay", "showRouteOverlay"];

fn strip_display_only_settings(path: &std::path::Path, key_hex: &str) -> Result<(), String> {
    let conn = db::open_with_key(&path.to_path_buf(), key_hex).map_err(|e| e.to_string())?;
    for key in DISPLAY_ONLY_SETTINGS_KEYS {
        conn.execute("DELETE FROM settings WHERE key = ?1", rusqlite::params![key])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn write_backup_zip(dest_path: &str, tmp_db: &std::path::Path, salt: &str) -> Result<(), String> {
    // Issue #32: no create_dir_all here — resolve_write_dest already
    // requires the configured backupFolder to exist and requires
    // dest_path to sit directly inside it, so there is no legitimate
    // case where a directory still needs creating at this point.
    let file = std::fs::File::create(dest_path).map_err(|e| format!("could not create {dest_path}: {e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);

    let mut db_bytes = Vec::new();
    std::fs::File::open(tmp_db)
        .and_then(|mut f| f.read_to_end(&mut db_bytes))
        .map_err(|e| e.to_string())?;
    zip.start_file(DB_ENTRY, options).map_err(|e| e.to_string())?;
    zip.write_all(&db_bytes).map_err(|e| e.to_string())?;

    zip.start_file(SALT_ENTRY, options).map_err(|e| e.to_string())?;
    zip.write_all(salt.as_bytes()).map_err(|e| e.to_string())?;

    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

// Issue #25: caps on what a hostile/corrupted archive can make this
// process read into memory before the passphrase is even checked. Well
// above any legitimate size here (10,000 households is a few MB).
const MAX_DB_ENTRY_BYTES: u64 = 200 * 1024 * 1024;
const MAX_SALT_ENTRY_BYTES: u64 = 1024;

/// Unpacks a backup zip's DB entry into a scratch file (SQLCipher needs a
/// real path) and returns it alongside the salt entry's contents.
fn extract_backup_zip(src_path: &str) -> Result<(TmpFile, String), String> {
    let file = std::fs::File::open(src_path).map_err(|e| format!("could not open {src_path}: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("{src_path} is not a valid backup file: {e}"))?;

    // A genuine backup always has exactly these two entries. Any other
    // count means the file isn't a Lost Sheep backup — reject by shape
    // before trusting anything else about it.
    if archive.len() != 2 {
        return Err(format!(
            "backup file has {} entries, expected 2 — not a valid Lost Sheep backup",
            archive.len()
        ));
    }

    let db_bytes = {
        let mut entry = archive
            .by_name(DB_ENTRY)
            .map_err(|_| format!("backup file is missing its {DB_ENTRY} entry — not a valid Lost Sheep backup"))?;
        if entry.size() > MAX_DB_ENTRY_BYTES {
            return Err(format!(
                "backup's {DB_ENTRY} entry is {} bytes, over the {MAX_DB_ENTRY_BYTES}-byte limit — refusing to read",
                entry.size()
            ));
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        buf
    };

    let salt = {
        let mut entry = archive
            .by_name(SALT_ENTRY)
            .map_err(|_| format!("backup file is missing its {SALT_ENTRY} entry — not a valid Lost Sheep backup"))?;
        if entry.size() > MAX_SALT_ENTRY_BYTES {
            return Err(format!("backup's {SALT_ENTRY} entry is too large — not a valid Lost Sheep backup"));
        }
        let mut s = String::new();
        entry.read_to_string(&mut s).map_err(|e| e.to_string())?;
        s
    };

    let tmp_db = TmpFile(tmp_path("lost-sheep-restore"));
    std::fs::write(&tmp_db.0, &db_bytes).map_err(|e| e.to_string())?;
    Ok((tmp_db, salt.trim().to_string()))
}

#[derive(Serialize)]
pub struct RestoreDiffRow {
    pub kind: String, // "added" | "removed" | "changed"
    pub description: String,
}

#[derive(Serialize)]
pub struct TagCountRow {
    pub name: String,
    pub current_count: i64,
    pub backup_count: i64,
}

#[derive(Serialize)]
pub struct RestorePreview {
    pub rows: Vec<RestoreDiffRow>,
    pub backup_household_count: i64,
    pub current_household_count: i64,
    pub tag_counts: Vec<TagCountRow>,
    // Issue #26: must be echoed back to restore_commit unchanged, or the
    // commit is refused. Proves a preview ran against this exact file.
    pub token: String,
}

fn tag_counts(conn: &rusqlite::Connection) -> Result<std::collections::HashMap<String, i64>, String> {
    let mut stmt = conn
        .prepare("SELECT t.name, (SELECT count(*) FROM household_tags ht WHERE ht.tag_id = t.id) FROM tags t")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows.into_iter().collect())
}

/// Shows a before/after diff without touching the live DB — required by
/// spec so the user can review before committing. Includes a per-tag
/// current-vs-backup count breakdown alongside the household add/remove
/// rows, so "how many end up in each tag" is visible before committing,
/// not just the raw add/remove list.
#[tauri::command]
pub fn restore_preview(state: State<AppState>, src_path: String, passphrase: String) -> Result<RestorePreview, String> {
    let result = (|| -> Result<RestorePreview, String> {
    // Issue #32: confirm src_path is under the user's home directory
    // before ever opening it.
    let src_path = super::paths::resolve_read_path(&src_path)?.to_string_lossy().to_string();
    let meta = std::fs::metadata(&src_path).map_err(|e| format!("could not read {src_path}: {e}"))?;
    let token = preview_token(&src_path, &meta);
    let (tmp_db, salt) = extract_backup_zip(&src_path)?;
    let key = crypto::derive_key_hex(&passphrase, &salt).map_err(|e| e.to_string())?;
    let backup_conn = db::open_with_key(&tmp_db.0, &key).map_err(|e| e.to_string())?;
    let live_conn = state.pool.get().map_err(|e| e.to_string())?;

    let backup_count: i64 = backup_conn.query_row("SELECT count(*) FROM households", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    let live_count: i64 = live_conn.query_row("SELECT count(*) FROM households", [], |r| r.get(0)).map_err(|e| e.to_string())?;

    let mut backup_keys_stmt = backup_conn.prepare("SELECT source_key, first_name, last_name, address_line1 FROM households").map_err(|e| e.to_string())?;
    let backup_rows: Vec<(String, String, String, String)> = backup_keys_stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    let mut live_all_stmt = live_conn.prepare("SELECT source_key, first_name, last_name, address_line1 FROM households").map_err(|e| e.to_string())?;
    let live_rows_all: Vec<(String, String, String, String)> = live_all_stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    // Issue #37: source_key has no UNIQUE constraint (schema.sql) and
    // pdf_parser::source_key() can collide for two distinct households
    // (same names + same address_line1, e.g. an adult child at a parent's
    // address). The previous HashSet-membership check treated "key
    // present on both sides" as "no change" even when the counts on each
    // side differed — a real added household and a real removed
    // household could cancel out invisibly whenever they shared a key.
    // Grouping by key and comparing counts per key instead makes the
    // added/removed tally algebraically equal to the raw count delta,
    // and surfaces the collision itself as its own row instead of hiding
    // it.
    let mut backup_by_key: std::collections::HashMap<&str, Vec<&(String, String, String, String)>> = std::collections::HashMap::new();
    for row in &backup_rows {
        backup_by_key.entry(row.0.as_str()).or_default().push(row);
    }
    let mut live_by_key: std::collections::HashMap<&str, Vec<&(String, String, String, String)>> = std::collections::HashMap::new();
    for row in &live_rows_all {
        live_by_key.entry(row.0.as_str()).or_default().push(row);
    }

    let mut all_keys: Vec<&str> = backup_by_key.keys().chain(live_by_key.keys()).copied().collect();
    all_keys.sort();
    all_keys.dedup();

    let mut rows = Vec::new();
    let mut added_count = 0i64;
    let mut removed_count = 0i64;
    for key in all_keys {
        let b = backup_by_key.get(key).map(|v| v.as_slice()).unwrap_or(&[]);
        let l = live_by_key.get(key).map(|v| v.as_slice()).unwrap_or(&[]);

        if b.len() > 1 || l.len() > 1 {
            rows.push(RestoreDiffRow {
                kind: "duplicate_key".into(),
                description: format!(
                    "source_key \"{key}\" appears {} time(s) in backup, {} time(s) in current — names/address collide, diff below may be approximate for this key",
                    b.len(),
                    l.len()
                ),
            });
        }

        if b.len() > l.len() {
            for (_, first, last, addr) in b.iter().take(b.len() - l.len()) {
                rows.push(RestoreDiffRow { kind: "added".into(), description: format!("{first} {last} — {addr}") });
                added_count += 1;
            }
        } else if l.len() > b.len() {
            for (_, first, last, addr) in l.iter().take(l.len() - b.len()) {
                rows.push(RestoreDiffRow { kind: "removed".into(), description: format!("{first} {last} — {addr}") });
                removed_count += 1;
            }
        }
    }

    // Issue #37: added/removed counted this way is constructed to always
    // satisfy this equation, but check it explicitly rather than trust
    // the arithmetic silently — a preview the user can't trust is worse
    // than a blocked restore.
    if live_count + added_count - removed_count != backup_count {
        return Err(format!(
            "restore preview is internally inconsistent (current {live_count} + added {added_count} - removed {removed_count} != backup {backup_count}) — refusing to show an untrustworthy diff"
        ));
    }

    let current_tag_counts = tag_counts(&live_conn)?;
    let backup_tag_counts = tag_counts(&backup_conn)?;
    let mut names: Vec<String> = current_tag_counts.keys().chain(backup_tag_counts.keys()).cloned().collect();
    names.sort();
    names.dedup();
    let tag_counts_out: Vec<TagCountRow> = names
        .into_iter()
        .map(|name| {
            let current_count = *current_tag_counts.get(&name).unwrap_or(&0);
            let backup_count = *backup_tag_counts.get(&name).unwrap_or(&0);
            TagCountRow { name, current_count, backup_count }
        })
        .collect();

    *state.last_preview.lock().map_err(|_| "internal error: preview lock poisoned".to_string())? =
        Some((src_path.clone(), token.clone()));

    Ok(RestorePreview { rows, backup_household_count: backup_count, current_household_count: live_count, tag_counts: tag_counts_out, token })
    })();

    if let Err(e) = &result {
        if let Ok(conn) = state.pool.get() {
            super::logs::log(&conn, "error", &format!("restore preview failed: {e}"), None);
        }
    }
    result
}

/// Commits the restore: re-keys the backup into the live DB's own
/// SQLCipher key (the one in the OS keychain) and swaps it in atomically.
/// Requires a token minted by a prior `restore_preview` call against this
/// same file (issue #26) — the before/after screen is enforced, not just
/// a UI convention. Restarts the app on success (issue #26 Option B) so
/// no stale pool connection can read the pre-restore file afterward.
#[tauri::command]
pub fn restore_commit(state: State<AppState>, app: tauri::AppHandle, src_path: String, passphrase: String, token: String) -> Result<(), String> {
    let result = (|| -> Result<(), String> {
    // Issue #32: same check as restore_preview — this is the destructive
    // half, so it gets no less scrutiny just because preview already ran.
    let src_path = super::paths::resolve_read_path(&src_path)?.to_string_lossy().to_string();

    // Single-use: consumed on every commit attempt, matched or not, so a
    // token can't be replayed and a failed attempt always forces a fresh
    // preview before the next try.
    let previewed = state
        .last_preview
        .lock()
        .map_err(|_| "internal error: preview lock poisoned".to_string())?
        .take();
    match previewed {
        Some((p, t)) if p == src_path && t == token => {}
        _ => return Err("restore was not previewed for this exact file — run Preview changes again before committing".to_string()),
    }

    let (tmp_db, salt) = extract_backup_zip(&src_path)?;
    let key = crypto::derive_key_hex(&passphrase, &salt).map_err(|e| e.to_string())?;
    // Scratch file for the re-keyed database.
    // Must live on the SAME filesystem as state.db_path — std::fs::rename
    // fails with EXDEV ("Invalid cross-device link") if the source and
    // destination are on different mounts, which system temp dir often
    // is relative to the app data directory. Same-directory placement
    // guarantees the rename below is a same-filesystem atomic rename,
    // not a cross-device one. Trades away the tidiness of keeping scratch
    // files out of the app data dir (a stray file here on a hard crash
    // is possible again) for a restore that actually works.
    let rekeyed = TmpFile(state.db_path.with_extension(format!("restoring-{}.tmp", uuid::Uuid::new_v4())));
    db::rekey_copy(&tmp_db.0, &key, &rekeyed.0, &state.live_key_hex).map_err(|e| e.to_string())?;

    // Swap on disk.
    std::fs::rename(&rekeyed.0, &state.db_path).map_err(|e| e.to_string())?;

    // The live pool runs in WAL mode (db::open_pool), so the PRE-restore
    // database may have left `-wal`/`-shm` sidecar files sitting next to
    // db_path. Those belong to the OLD database's content — the rename
    // above only swaps the main .db file, not its sidecars. Left in
    // place, SQLite would try to recover those old WAL frames against the
    // newly-swapped-in file at next startup, which is exactly why a
    // restore could "complete" on disk yet show no data after restart:
    // the mismatched WAL recovery silently wins over the restored file's
    // actual content. Clearing them makes the restored file start clean.
    for suffix in ["-wal", "-shm"] {
        if let Some(name) = state.db_path.file_name().and_then(|n| n.to_str()) {
            let _ = std::fs::remove_file(state.db_path.with_file_name(format!("{name}{suffix}")));
        }
    }

    // Issue #26: the connection pool in AppState still holds the
    // pre-restore file open — renaming over it doesn't close those
    // handles, so stale reads/writes are possible until the process
    // reopens the database fresh. AppHandle::restart() exits this
    // process and relaunches the same binary, closing that window.
    //
    // Gated to release builds: `cargo tauri dev` runs a separate
    // frontend dev server on 127.0.0.1 and tears it down when it sees
    // this process exit, even to relaunch — the restarted webview then
    // has nothing to reload and shows a permanent white screen /
    // connection-refused. A packaged build has no dev server (frontend
    // is bundled into the binary), so this only affects dev workflows.
    #[cfg(debug_assertions)]
    {
        let _ = app; // unused in debug builds — silence the warning
        return Err(
            "Restore complete, but the app could not restart itself automatically \
             in `cargo tauri dev` (it would tear down the frontend dev server). \
             Please stop and re-run `cargo tauri dev` now."
                .to_string(),
        );
    }

    #[cfg(not(debug_assertions))]
    {
        app.restart();
    }

    // Reached only in a release build, and only if restart() somehow
    // returns instead of exiting the process — kept rather than relied
    // on divergence, since that exact signature isn't verified here.
    #[cfg(not(debug_assertions))]
    Ok(())
    })();

    if let Err(e) = &result {
        if let Ok(conn) = state.pool.get() {
            // The debug-build manual-restart message is Err-shaped so the
            // frontend surfaces it prominently, but the restore itself
            // already succeeded by the time it's returned — log it as
            // info, not error, so it doesn't read as a failed restore.
            let level = if e.starts_with("Restore complete") { "info" } else { "error" };
            super::logs::log(&conn, level, &format!("restore commit: {e}"), None);
        }
    }
    result
}
