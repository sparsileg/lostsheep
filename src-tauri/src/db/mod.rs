use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
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
        conn.update_hook(Some(
            move |_action, _db_name: &str, table_name: &str, _rowid| {
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
            },
        ));
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
    backfill_null_visit_uids(&conn)?;
    // Issue #70: must also run before the schema batch — schema.sql's own
    // CREATE TABLE IF NOT EXISTS is a no-op on an existing review_queue
    // table, so a pre-existing database would otherwise keep the old,
    // narrower CHECK constraint forever and reject 'link' resolutions
    // with a raw CHECK-constraint error at write time.
    migrate_review_queue_link_resolution(&conn)?;
    migrate_stage2(&conn)?;
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

    conn.execute_batch(
        "ALTER TABLE households ADD COLUMN source_key_seq INTEGER NOT NULL DEFAULT 0;",
    )?;

    // Assign sequential seq per duplicate source_key group. Done in Rust
    // rather than a single SQL window-function statement — simpler to
    // reason about correctness here, and this runs at most once per
    // install, never on a hot path.
    let mut rows_stmt =
        conn.prepare("SELECT id, source_key FROM households ORDER BY source_key, id")?;
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
            conn.execute(
                "UPDATE households SET source_key_seq = ?1 WHERE id = ?2",
                rusqlite::params![*seq, id],
            )?;
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
    let ids: Vec<i64> = ids_stmt
        .query_map([], |r| r.get(0))?
        .filter_map(Result::ok)
        .collect();
    drop(ids_stmt);
    for id in ids {
        let uid = Uuid::new_v4().to_string();
        conn.execute(
            "UPDATE households SET household_uid = ?1 WHERE id = ?2",
            rusqlite::params![uid, id],
        )?;
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
    let ids: Vec<i64> = ids_stmt
        .query_map([], |r| r.get(0))?
        .filter_map(Result::ok)
        .collect();
    drop(ids_stmt);
    for id in ids {
        let uid = Uuid::new_v4().to_string();
        conn.execute(
            "UPDATE visits SET visit_uid = ?1 WHERE id = ?2",
            rusqlite::params![uid, id],
        )?;
    }

    // deleted_visits: nullable mirror column, no backfill possible — same
    // reasoning as deleted_households.household_uid above.
    conn.execute_batch(
        "ALTER TABLE deleted_visits ADD COLUMN visit_uid TEXT; \
         CREATE UNIQUE INDEX IF NOT EXISTS idx_visits_visit_uid ON visits(visit_uid);",
    )
}

/// Repairs visits stored with a NULL visit_uid. record_visit used to omit
/// visit_uid from its INSERT; on a database whose column was added by
/// migrate_visit_uid (nullable — ALTER TABLE can't add NOT NULL without a
/// default) that silently stored NULL for every visit recorded since.
/// The unique index permits any number of NULLs, so nothing flagged it.
/// Runs on every open; after the first repair it finds no rows and is a
/// single indexed lookup. A fresh database's NOT NULL column can never
/// hold NULL, so this is a no-op there.
fn backfill_null_visit_uids(conn: &Connection) -> rusqlite::Result<()> {
    let table_exists: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='visits'",
        [],
        |r| r.get(0),
    )?;
    if table_exists == 0 {
        return Ok(());
    }
    let mut stmt = conn.prepare("SELECT id FROM visits WHERE visit_uid IS NULL")?;
    let ids: Vec<i64> = stmt
        .query_map([], |r| r.get(0))?
        .filter_map(Result::ok)
        .collect();
    drop(stmt);
    for id in ids {
        conn.execute(
            "UPDATE visits SET visit_uid = ?1 WHERE id = ?2",
            rusqlite::params![Uuid::new_v4().to_string(), id],
        )?;
    }
    Ok(())
}

/// One-time migration for databases created before 'link' joined
/// review_queue.resolution's vocabulary (issue #70). SQLite can't ALTER a
/// CHECK constraint in place, so this rebuilds the table under the
/// standard SQLite "12-step" pattern: FK enforcement off for this
/// connection, swap the table inside a transaction, integrity-check,
/// FK enforcement back on. Detected by inspecting the table's own stored
/// CREATE TABLE text (sqlite_master.sql) for the literal 'link' — PRAGMA
/// table_info doesn't expose CHECK constraint contents the way it exposes
/// columns, so the column-existence probe the other migrations in this
/// file use doesn't apply here. No-op on a fresh DB (schema.sql's own
/// CREATE TABLE already includes 'link') and a no-op once migrated.
fn migrate_review_queue_link_resolution(conn: &Connection) -> rusqlite::Result<()> {
    let table_exists: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='review_queue'",
        [],
        |r| r.get(0),
    )?;
    if table_exists == 0 {
        return Ok(()); // fresh DB — schema.sql's CREATE TABLE handles it
    }

    let create_sql: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='review_queue'",
        [],
        |r| r.get(0),
    )?;
    if create_sql.contains("'link'") {
        return Ok(()); // already migrated
    }

    conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    let result = conn.execute_batch(
        "BEGIN;
         CREATE TABLE review_queue_new (
             id                    INTEGER PRIMARY KEY,
             import_batch_id       INTEGER NOT NULL REFERENCES import_batches(id) ON DELETE CASCADE,
             match_type            TEXT NOT NULL CHECK (match_type IN ('new','changed','removed')),
             incoming_data         TEXT,
             existing_household_id INTEGER REFERENCES households(id) ON DELETE SET NULL,
             resolution            TEXT NOT NULL DEFAULT 'pending'
                                    CHECK (resolution IN ('pending','replace','merge','add','delete','link','ignore')),
             resolution_comment    TEXT,
             resolved_at           TEXT
         );
         INSERT INTO review_queue_new SELECT * FROM review_queue;
         DROP TABLE review_queue;
         ALTER TABLE review_queue_new RENAME TO review_queue;
         CREATE INDEX IF NOT EXISTS idx_review_queue_batch ON review_queue(import_batch_id);
         COMMIT;",
    );
    // Re-enable FK enforcement regardless of outcome — never leave a
    // connection with it silently off if the rebuild above failed partway
    // (the BEGIN/COMMIT block above means it either fully applied or
    // fully rolled back, but this connection's PRAGMA is a separate,
    // connection-local setting that doesn't participate in that transaction).
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    result?;
    conn.query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
        .ok();
    Ok(())
}

/// Adds `column` to `table` unless it is already there. Returns whether
/// it was added. A missing table is left alone: schema.sql creates it,
/// with the column, right after the migrations run.
fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    decl: &str,
) -> rusqlite::Result<bool> {
    let table_exists: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |r| r.get(0),
    )?;
    if table_exists == 0 {
        return Ok(false);
    }
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let has_column = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == column);
    if has_column {
        return Ok(false);
    }
    conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl};"))?;
    Ok(true)
}

/// Version 1.5.0: visit outcome / method / invitation and household
/// contact plans (Visit_Reporting_Design.md Stage 2), and the import batch
/// that confirmed a clerk referral (Record_Referral_Design.md Stage 2).
///
/// Existing visits default to visit / contacted: the only thing the app
/// could record before was a visit, and "contacted" is the conservative
/// reading (it never makes a household look neglected that wasn't). The
/// time of the migration is kept in schema_meta so those defaulted rows
/// can be told apart from observed ones later (created_at earlier than it).
fn migrate_stage2(conn: &Connection) -> rusqlite::Result<()> {
    let outcome_added = add_column_if_missing(
        conn,
        "visits",
        "outcome",
        "TEXT NOT NULL DEFAULT 'contacted' CHECK (outcome IN ('contacted','no_answer','not_home',\
         'refused','moved','deceased','sent','other'))",
    )?;
    add_column_if_missing(
        conn,
        "visits",
        "method",
        "TEXT NOT NULL DEFAULT 'visit' CHECK (method IN ('visit','phone','card'))",
    )?;
    add_column_if_missing(conn, "visits", "invited", "INTEGER NOT NULL DEFAULT 0")?;
    add_column_if_missing(
        conn,
        "deleted_visits",
        "outcome",
        "TEXT NOT NULL DEFAULT 'contacted'",
    )?;
    add_column_if_missing(
        conn,
        "deleted_visits",
        "method",
        "TEXT NOT NULL DEFAULT 'visit'",
    )?;
    add_column_if_missing(
        conn,
        "deleted_visits",
        "invited",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(
        conn,
        "households",
        "contact_plan",
        "TEXT CHECK (contact_plan IS NULL OR contact_plan IN ('monthly','quarterly','as_needed'))",
    )?;
    add_column_if_missing(conn, "deleted_households", "contact_plan", "TEXT")?;
    add_column_if_missing(conn, "referrals", "import_batch_id", "INTEGER")?;
    if outcome_added {
        conn.execute(
            "INSERT OR IGNORE INTO schema_meta (key, value) \
             VALUES ('visit_outcome_migrated_at', strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            [],
        )?;
    }
    Ok(())
}

fn apply_key(conn: &Connection, key_hex: &str) -> rusqlite::Result<()> {
    conn.pragma_update(None, "key", format!("x'{}'", key_hex))?;
    // Cheap sanity read — throws SQLITE_NOTADB if the key is wrong, which
    // callers surface to the user as "could not unlock database".
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))?;
    Ok(())
}

/// Re-keys a *copy* of the live DB with a brand-new key derived from a
/// user passphrase, producing a self-contained portable backup file that
/// does not depend on this machine's OS keychain. Used by commands::backup.
pub fn rekey_copy(
    src_path: &Path,
    src_key_hex: &str,
    dest_path: &Path,
    dest_key_hex: &str,
) -> anyhow::Result<()> {
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

/// Issue #87 Fix 1: commits a restore by copying pages directly from the
/// already-rekeyed scratch file (`rekeyed_path`, same key as the live DB —
/// see `rekey_copy`/`restore_commit`) into a live connection pulled from
/// the pool, via SQLite's Online Backup API. Replaces the previous
/// `std::fs::rename(rekeyed_path, db_path)`, which worked on Linux
/// (rename-over-open-file is POSIX-legal) but failed on Windows with
/// "Access is denied. (os error 5)" because the pool's connections still
/// hold db_path open with no share-delete permission on that handle. This
/// approach never touches the file at the OS level at all, so there's
/// nothing for Windows' file-locking rules to block.
///
/// Requires the "backup" feature enabled on the rusqlite dependency in
/// Cargo.toml.
pub fn restore_via_online_backup(
    pool: &Pool,
    rekeyed_path: &PathBuf,
    key_hex: &str,
) -> anyhow::Result<()> {
    let src = Connection::open(rekeyed_path)?;
    apply_key(&src, key_hex)?;
    let mut dst = pool.get()?;
    {
        let backup = rusqlite::backup::Backup::new(&src, &mut dst)?;
        // rusqlite panics if pages_per_step isn't positive (-1, the usual
        // "all pages in one sqlite3_backup_step call" sentinel at the C
        // API level, isn't accepted here) — i32::MAX stands in for it, so
        // this still completes in a single internal step. Stan's DBs are
        // a few MB at documented scale (10,000 households), so one step is
        // fast and simplest. No progress callback: restore_commit has no
        // progress UI (unlike import's emit_progress), so there's nothing
        // to report to.
        backup.run_to_completion(i32::MAX, std::time::Duration::from_millis(0), None)?;
    }
    Ok(())
}

#[cfg(test)]
mod stage2_migration_tests {
    use super::*;

    /// A 1.4.0-shaped database: the tables Stage 2 alters, without the new
    /// columns, holding one visit and one household.
    fn v140() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL); \
             CREATE TABLE households (id INTEGER PRIMARY KEY, household_uid TEXT NOT NULL, \
                first_name TEXT NOT NULL); \
             CREATE TABLE deleted_households (id INTEGER PRIMARY KEY, first_name TEXT NOT NULL); \
             CREATE TABLE visits (id INTEGER PRIMARY KEY, visit_uid TEXT NOT NULL, \
                household_id INTEGER NOT NULL, visit_date TEXT NOT NULL, comments TEXT); \
             CREATE TABLE deleted_visits (id INTEGER PRIMARY KEY, visit_date TEXT NOT NULL); \
             CREATE TABLE referrals (id INTEGER PRIMARY KEY, household_uid TEXT NOT NULL); \
             INSERT INTO households (household_uid, first_name) VALUES ('u1', 'Ann'); \
             INSERT INTO visits (visit_uid, household_id, visit_date) VALUES ('v1', 1, '2026-01-01');",
        )
        .unwrap();
        c
    }

    #[test]
    fn adds_columns_with_defaults_and_records_the_date() {
        let c = v140();
        migrate_stage2(&c).unwrap();
        let (outcome, method, invited): (String, String, i64) = c
            .query_row("SELECT outcome, method, invited FROM visits", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(
            (outcome.as_str(), method.as_str(), invited),
            ("contacted", "visit", 0)
        );
        let plan: Option<String> = c
            .query_row("SELECT contact_plan FROM households", [], |r| r.get(0))
            .unwrap();
        assert_eq!(plan, None);
        let marked: i64 = c
            .query_row(
                "SELECT count(*) FROM schema_meta WHERE key = 'visit_outcome_migrated_at'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(marked, 1);
        // The CHECKs came along with the columns.
        assert!(c
            .execute("UPDATE visits SET outcome = 'bogus'", [])
            .is_err());
        assert!(c
            .execute("UPDATE households SET contact_plan = 'weekly'", [])
            .is_err());
        c.prepare("SELECT import_batch_id FROM referrals").unwrap();
    }

    #[test]
    fn is_idempotent_and_schema_sql_applies_after_it() {
        let c = v140();
        migrate_stage2(&c).unwrap();
        migrate_stage2(&c).unwrap();
        // A fresh database: nothing to migrate, schema.sql creates it all.
        let fresh = Connection::open_in_memory().unwrap();
        migrate_stage2(&fresh).unwrap();
        fresh.execute_batch(SCHEMA_SQL).unwrap();
        migrate_stage2(&fresh).unwrap();
        fresh.execute_batch(SCHEMA_SQL).unwrap();
    }
}

#[cfg(test)]
mod visit_uid_backfill_tests {
    use super::*;

    /// Shape of a database whose visit_uid column was added by
    /// migrate_visit_uid: nullable, with NULLs left by the old record_visit.
    #[test]
    fn backfills_null_uids_and_leaves_existing_ones() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE visits (id INTEGER PRIMARY KEY, household_id INTEGER, visit_date TEXT, visit_uid TEXT); \
             CREATE UNIQUE INDEX idx_visits_visit_uid ON visits(visit_uid); \
             INSERT INTO visits (household_id, visit_date, visit_uid) VALUES \
                (1, '2026-01-01', 'keep-me'), (1, '2026-02-01', NULL), (2, '2026-03-01', NULL);",
        )
        .unwrap();

        backfill_null_visit_uids(&c).unwrap();

        let nulls: i64 = c
            .query_row(
                "SELECT count(*) FROM visits WHERE visit_uid IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nulls, 0);
        let kept: String = c
            .query_row("SELECT visit_uid FROM visits WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kept, "keep-me");
        let distinct: i64 = c
            .query_row("SELECT count(DISTINCT visit_uid) FROM visits", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(distinct, 3);

        // Second run is a no-op.
        backfill_null_visit_uids(&c).unwrap();
    }

    #[test]
    fn no_visits_table_is_a_no_op() {
        let c = Connection::open_in_memory().unwrap();
        backfill_null_visit_uids(&c).unwrap();
    }
}
