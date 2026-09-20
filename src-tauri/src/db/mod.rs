use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;
use std::path::PathBuf;
use uuid::Uuid;

pub type Pool = r2d2::Pool<SqliteConnectionManager>;

const SCHEMA_SQL: &str = include_str!("schema.sql");
const ROADS_SCHEMA_SQL: &str = include_str!("roads_schema.sql");

/// Opens (creating if absent) the encrypted app DB and returns a pooled
/// connection manager. `key_hex` is the SQLCipher key as a 64-char hex
/// string (32 raw bytes) — see crypto::random_key_hex / keychain.rs for
/// where it comes from at app start. `dirty_flag` is issue #68's
/// backup-reminder timer: set on every real data-table write via
/// SQLite's update_hook, cheap in-memory only — SQLite forbids issuing a
/// new statement on the SAME connection from inside its own update_hook
/// callback until the triggering statement finishes, so this cannot
/// write to `settings` itself. A separate background poll (main.rs)
/// reads this flag, persists it, and does the actual 5-minutes-since-
/// last-change reminder check. `with_init` runs once per NEW physical
/// connection the pool opens (up to max_size below), so the hook is
/// installed on every one of them, not just the first.
pub fn open_pool(
    db_path: &PathBuf,
    key_hex: &str,
    dirty_flag: std::sync::Arc<std::sync::Mutex<Option<chrono::DateTime<chrono::Utc>>>>,
) -> anyhow::Result<Pool> {
    let key_hex = key_hex.to_string();
    let manager = SqliteConnectionManager::file(db_path).with_init(move |conn| {
        apply_key(conn, &key_hex)?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        let flag = dirty_flag.clone();
        conn.update_hook(Some(move |_action, _db_name: &str, table_name: &str, _rowid| {
            // logs/settings/schema_meta writes happen on essentially every
            // command (every command logs; settings itself would recurse
            // into "a change was made" forever) — excluded so the timer
            // reflects actual congregation data changing, not the app's
            // own bookkeeping.
            if matches!(table_name, "logs" | "settings" | "schema_meta") {
                return;
            }
            if let Ok(mut guard) = flag.lock() {
                *guard = Some(chrono::Utc::now());
            }
        }));
        Ok(())
    });
    let pool = r2d2::Pool::builder().max_size(8).build(manager)?;

    // Run schema on a fresh connection from the pool so PRAGMA key is set.
    let conn = pool.get()?;
    // Must run BEFORE the schema batch: on an existing database created
    // before system_key existed, schema.sql's own seed INSERT
    // (`INSERT OR IGNORE INTO tags (name, name_norm, system_key) ...`)
    // fails with "no such column: system_key" if the column isn't there
    // yet — SQLite checks the statement's column list before OR IGNORE
    // ever gets a chance to apply (#23).
    migrate_tags_system_key(&conn)?;
    migrate_source_key_seq(&conn)?;
    // Issue #69: must also run before the schema batch below — schema.sql
    // now contains CREATE UNIQUE INDEX statements over household_uid and
    // visit_uid, which fail with "no such column" on a pre-existing
    // database if these migrations haven't added the columns yet.
    migrate_household_uid(&conn)?;
    migrate_visit_uid(&conn)?;
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(pool)
}

/// Opens (creating if absent) the plain, unencrypted road-graph database
/// (issue #39) and returns a pooled connection manager. No SQLCipher key
/// applied — the road graph is public OSM data with no congregant PII,
/// so it doesn't need the keychain/encryption overhead the main DB pays.
/// Kept as a real `r2d2` pool rather than a single `Mutex<Connection>`:
/// routing (#38) will issue many concurrent reads per visit-list
/// generation, and WAL mode (set below) allows those reads to run
/// concurrently against a pool the same way the main DB already does.
pub fn open_roads_pool(db_path: &PathBuf) -> anyhow::Result<Pool> {
    let manager = SqliteConnectionManager::file(db_path).with_init(|conn| {
        conn.execute_batch("PRAGMA journal_mode = WAL;")?;
        Ok(())
    });
    let pool = r2d2::Pool::builder().max_size(8).build(manager)?;

    let conn = pool.get()?;
    conn.execute_batch(ROADS_SCHEMA_SQL)?;
    Ok(pool)
}

/// One-time column migration for databases created before system_key
/// existed on tags (#23) — the "Do not contact" exclusion used to key
/// off the user-editable name_norm, which silently broke on rename.
/// Checked via PRAGMA table_info rather than schema_meta.schema_version:
/// nothing in this codebase reads schema_version yet, and a
/// check-then-act probe here is self-contained and safe to run on every
/// launch regardless of what that column says. No-op on a fresh DB
/// (schema.sql's own CREATE TABLE already includes the column) and a
/// no-op on a DB that's already been migrated.
fn migrate_tags_system_key(conn: &Connection) -> rusqlite::Result<()> {
    let table_exists: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='tags'",
        [],
        |r| r.get(0),
    )?;
    if table_exists == 0 {
        return Ok(()); // fresh DB — schema.sql's CREATE TABLE handles it
    }

    let mut stmt = conn.prepare("PRAGMA table_info(tags)")?;
    let has_column = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == "system_key");
    if has_column {
        return Ok(()); // already migrated
    }

    // SQLite refuses ALTER TABLE ADD COLUMN with a UNIQUE constraint
    // directly ("Cannot add a UNIQUE column") — add it plain, then get
    // uniqueness via the same partial index schema.sql defines for
    // fresh databases, so migrated and fresh DBs end up structurally
    // identical.
    conn.execute_batch(
        "ALTER TABLE tags ADD COLUMN system_key TEXT; \
         CREATE UNIQUE INDEX IF NOT EXISTS idx_tags_system_key ON tags(system_key) WHERE system_key IS NOT NULL; \
         UPDATE tags SET system_key = 'do_not_contact' WHERE name_norm = 'do not contact' AND system_key IS NULL;",
    )
}

/// One-time migration for databases created before source_key_seq existed
/// (dup-key restore preview issue): source_key alone was never unique —
/// two distinct households with identical name+address (father/son,
/// same role) can compute the same key with no field left to
/// disambiguate them. Adds the column, then assigns 0/1/2/... within
/// each duplicate group ordered by id (the oldest record keeps 0, so
/// existing tags/visits/comments stay associated with the household a
/// user would expect), before schema.sql's UNIQUE(source_key,
/// source_key_seq) index is created. No-op on a fresh DB (schema.sql's
/// CREATE TABLE already includes the column) and a no-op once migrated.
fn migrate_source_key_seq(conn: &Connection) -> rusqlite::Result<()> {
    let table_exists: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='households'",
        [],
        |r| r.get(0),
    )?;
    if table_exists == 0 {
        return Ok(()); // fresh DB — schema.sql's CREATE TABLE handles it
    }

    let mut stmt = conn.prepare("PRAGMA table_info(households)")?;
    let has_column = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == "source_key_seq");
    if has_column {
        return Ok(()); // already migrated
    }

    conn.execute_batch("ALTER TABLE households ADD COLUMN source_key_seq INTEGER NOT NULL DEFAULT 0;")?;

    // Assign sequential seq per duplicate source_key group. Done in Rust
    // rather than a single SQL window-function statement — simpler to
    // reason about correctness here, and this runs at most once per
    // install, never on a hot path.
    let mut rows_stmt = conn.prepare("SELECT id, source_key FROM households ORDER BY source_key, id")?;
    let all_rows: Vec<(i64, String)> = rows_stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(Result::ok)
        .collect();
    drop(rows_stmt);

    let mut next_seq: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for (id, key) in all_rows {
        let seq = next_seq.entry(key).or_insert(-1);
        *seq += 1;
        if *seq != 0 {
            // Only rows past the first in a group actually need writing —
            // the column's own DEFAULT 0 already covers the first.
            conn.execute("UPDATE households SET source_key_seq = ?1 WHERE id = ?2", rusqlite::params![*seq, id])?;
        }
    }

    Ok(())
}

/// One-time migration for databases created before household_uid existed
/// (issue #69): households.id is a rowid, regenerated by Replace/Merge
/// and by restore_deleted_household(); source_key is derived from
/// mutable fields and isn't even unique alone. household_uid is the
/// first value that is both stable and unique. Assigns a fresh UUIDv4 to
/// every existing household, and adds the (nullable, no-backfill —
/// there's nothing to backfill from) mirror columns on
/// deleted_households so a delete/restore round trip has somewhere to
/// carry the value through. No-op on a fresh DB (schema.sql's own CREATE
/// TABLE already includes household_uid) and a no-op once migrated.
fn migrate_household_uid(conn: &Connection) -> rusqlite::Result<()> {
    let table_exists: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='households'",
        [],
        |r| r.get(0),
    )?;
    if table_exists == 0 {
        return Ok(()); // fresh DB — schema.sql's CREATE TABLE handles it
    }

    let mut stmt = conn.prepare("PRAGMA table_info(households)")?;
    let has_column = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == "household_uid");
    if has_column {
        return Ok(()); // already migrated
    }

    conn.execute_batch("ALTER TABLE households ADD COLUMN household_uid TEXT;")?;

    // One UUID per row, same one-at-a-time approach migrate_source_key_seq
    // uses above — simpler to reason about than a bulk SQL expression,
    // and this runs at most once per install, never on a hot path.
    let mut ids_stmt = conn.prepare("SELECT id FROM households")?;
    let ids: Vec<i64> = ids_stmt.query_map([], |r| r.get(0))?.filter_map(Result::ok).collect();
    drop(ids_stmt);
    for id in ids {
        let uid = Uuid::new_v4().to_string();
        conn.execute("UPDATE households SET household_uid = ?1 WHERE id = ?2", rusqlite::params![uid, id])?;
    }

    // deleted_households: nullable mirror columns, no backfill possible —
    // a row soft-deleted before this migration has no household_uid or
    // original source_key_seq to carry forward (see each column's own
    // comment in schema.sql). Guarded the same way as the households
    // columns above rather than a separate table_info probe, since both
    // land together in this one migration.
    conn.execute_batch(
        "ALTER TABLE deleted_households ADD COLUMN household_uid TEXT; \
         ALTER TABLE deleted_households ADD COLUMN source_key_seq INTEGER; \
         CREATE UNIQUE INDEX IF NOT EXISTS idx_households_household_uid ON households(household_uid);",
    )
}

/// One-time migration for databases created before visit_uid existed
/// (issue #69/#70): #70's recovery journal needs a stable per-visit id to
/// dedupe replayed events. Bundled into the same release as
/// household_uid per #69's own open-question decision ("doing both in
/// one migration is cheaper than two"), but kept as its own function —
/// visits and households are unrelated tables, and this stays independently
/// no-op-safe the same way every other migration here does.
fn migrate_visit_uid(conn: &Connection) -> rusqlite::Result<()> {
    let table_exists: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='visits'",
        [],
        |r| r.get(0),
    )?;
    if table_exists == 0 {
        return Ok(()); // fresh DB — schema.sql's CREATE TABLE handles it
    }

    let mut stmt = conn.prepare("PRAGMA table_info(visits)")?;
    let has_column = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == "visit_uid");
    if has_column {
        return Ok(()); // already migrated
    }

    conn.execute_batch("ALTER TABLE visits ADD COLUMN visit_uid TEXT;")?;

    let mut ids_stmt = conn.prepare("SELECT id FROM visits")?;
    let ids: Vec<i64> = ids_stmt.query_map([], |r| r.get(0))?.filter_map(Result::ok).collect();
    drop(ids_stmt);
    for id in ids {
        let uid = Uuid::new_v4().to_string();
        conn.execute("UPDATE visits SET visit_uid = ?1 WHERE id = ?2", rusqlite::params![uid, id])?;
    }

    // deleted_visits: nullable mirror column, no backfill possible — same
    // reasoning as deleted_households.household_uid above.
    conn.execute_batch(
        "ALTER TABLE deleted_visits ADD COLUMN visit_uid TEXT; \
         CREATE UNIQUE INDEX IF NOT EXISTS idx_visits_visit_uid ON visits(visit_uid);",
    )
}

fn apply_key(conn: &Connection, key_hex: &str) -> rusqlite::Result<()> {
    conn.pragma_update(None, "key", &format!("x'{}'", key_hex))?;
    // Cheap sanity read — throws SQLITE_NOTADB if the key is wrong, which
    // callers surface to the user as "could not unlock database".
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))?;
    Ok(())
}

/// Re-keys a *copy* of the live DB with a brand-new key derived from a
/// user passphrase, producing a self-contained portable backup file that
/// does not depend on this machine's OS keychain. Used by commands::backup.
pub fn rekey_copy(src_path: &PathBuf, src_key_hex: &str, dest_path: &PathBuf, dest_key_hex: &str) -> anyhow::Result<()> {
    let conn = Connection::open(src_path)?;
    apply_key(&conn, src_key_hex)?;
    conn.execute(
        "ATTACH DATABASE ?1 AS backup_db KEY ?2",
        rusqlite::params![dest_path.to_string_lossy(), format!("x'{}'", dest_key_hex)],
    )?;
    conn.query_row("SELECT sqlcipher_export('backup_db')", [], |_| Ok(()))?;
    conn.execute("DETACH DATABASE backup_db", [])?;
    Ok(())
}

/// Opens an arbitrary SQLCipher file with the given key — used by restore
/// preview/commit against a backup file, independent of the live pool.
pub fn open_with_key(path: &PathBuf, key_hex: &str) -> anyhow::Result<Connection> {
    let conn = Connection::open(path)?;
    apply_key(&conn, key_hex)?;
    Ok(conn)
}
