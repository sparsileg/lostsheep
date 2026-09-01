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
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(pool)
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
