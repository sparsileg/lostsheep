use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;
use std::path::PathBuf;

pub type Pool = r2d2::Pool<SqliteConnectionManager>;

const SCHEMA_SQL: &str = include_str!("schema.sql");

/// Opens (creating if absent) the encrypted app DB and returns a pooled
/// connection manager. `key_hex` is the SQLCipher key as a 64-char hex
/// string (32 raw bytes) — see crypto::random_key_hex / keychain.rs for
/// where it comes from at app start.
pub fn open_pool(db_path: &PathBuf, key_hex: &str) -> anyhow::Result<Pool> {
    let key_hex = key_hex.to_string();
    let manager = SqliteConnectionManager::file(db_path).with_init(move |conn| {
        apply_key(conn, &key_hex)?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
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
    conn.execute_batch(SCHEMA_SQL)?;
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
