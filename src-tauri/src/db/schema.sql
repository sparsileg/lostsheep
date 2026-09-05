-- Lost Sheep — SQLCipher schema. Applied once on fresh DB creation.
-- All tables use INTEGER PRIMARY KEY (rowid alias) for speed at 10k-row scale.

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('schema_version', '1');

-- One row per household-entry as delivered in the source PDF: a single
-- person, OR a couple sharing one entry (first_name_2/last_name_2/role_2
-- populated). Never more than 2 co-heads per entry — confirmed against
-- real directory data; anyone else at the same address (adult child,
-- grandparent) gets their own separate row. Records that share
-- address_key are grouped together at visit-list time.
CREATE TABLE IF NOT EXISTS households (
    id            INTEGER PRIMARY KEY,
    first_name    TEXT NOT NULL,
    last_name     TEXT NOT NULL,
    role          TEXT NOT NULL CHECK (role IN ('head','husband','wife','minor','grandparent','other')),
    phone_1       TEXT,             -- head 1's phone/email, captured from the directory, display-only
    email_1       TEXT,
    first_name_2  TEXT,              -- second head, if this entry is a couple; NULL otherwise
    last_name_2   TEXT,
    role_2        TEXT CHECK (role_2 IS NULL OR role_2 IN ('head','husband','wife','minor','grandparent','other')),
    phone_2       TEXT,
    email_2       TEXT,
    address_line1 TEXT,               -- nullable: some directory entries have no address on file at all
    address_line2 TEXT,
    city          TEXT,
    state         TEXT,
    zip           TEXT,
    latitude      REAL,              -- nullable: missing geocoords handled gracefully
    longitude     REAL,
    address_key   TEXT NOT NULL,     -- normalized address, used to group same-address records
    source_key    TEXT NOT NULL,     -- normalized name(s)+address, used for import dedupe matching
    has_minors    INTEGER NOT NULL DEFAULT 0,  -- true if the source flagged unparsed minor-children
                                                 -- lines for this entry; names are NEVER stored, only this flag
    comments      TEXT,              -- household-level comments (distinct from visit comments) — free-text
                                       -- notes only now, never auto-populated by the parser
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_households_address_key ON households(address_key);
CREATE INDEX IF NOT EXISTS idx_households_source_key  ON households(source_key);
CREATE INDEX IF NOT EXISTS idx_households_lat_lng      ON households(latitude, longitude);
CREATE INDEX IF NOT EXISTS idx_households_last_name    ON households(last_name);

-- Deleted (soft-delete) mirror. Records land here when an import shows
-- them absent and the user confirms removal, or on manual delete.
CREATE TABLE IF NOT EXISTS deleted_households (
    id               INTEGER PRIMARY KEY,
    original_id      INTEGER NOT NULL,
    first_name       TEXT NOT NULL,
    last_name        TEXT NOT NULL,
    role             TEXT NOT NULL,
    phone_1          TEXT,
    email_1          TEXT,
    first_name_2     TEXT,
    last_name_2      TEXT,
    role_2           TEXT,
    phone_2          TEXT,
    email_2          TEXT,
    address_line1    TEXT,
    address_line2    TEXT,
    city             TEXT,
    state            TEXT,
    zip              TEXT,
    latitude         REAL,
    longitude        REAL,
    address_key      TEXT NOT NULL,
    source_key       TEXT NOT NULL,
    has_minors       INTEGER NOT NULL DEFAULT 0,
    comments         TEXT,
    deletion_reason  TEXT,
    deleted_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE IF NOT EXISTS tags (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,      -- stored trimmed; spaces allowed inside
    name_norm   TEXT NOT NULL UNIQUE,      -- lowercased, collapsed-whitespace, for case-insensitive lookup
    -- Stable identifier for tags the app itself depends on (e.g. the
    -- visit-list "do not contact" exclusion, #23). NULL for ordinary
    -- user tags. Unlike name/name_norm, this never changes on rename,
    -- so app logic keyed off it survives the user relabeling the tag.
    -- Plain column, not UNIQUE inline — SQLite's ALTER TABLE ADD COLUMN
    -- refuses a UNIQUE column outright ("Cannot add a UNIQUE column"),
    -- which matters because db/mod.rs's migrate_tags_system_key() has
    -- to add this same column to existing databases via ALTER. Uniqueness
    -- comes from the partial index below instead, on both fresh and
    -- migrated databases alike.
    system_key  TEXT,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
-- WHERE system_key IS NOT NULL: any number of ordinary tags (system_key
-- NULL) coexist fine; only real system_key values must be unique.
CREATE UNIQUE INDEX IF NOT EXISTS idx_tags_system_key ON tags(system_key) WHERE system_key IS NOT NULL;
INSERT OR IGNORE INTO tags (name, name_norm, system_key) VALUES
    ('Not known', 'not known', NULL),
    ('Known', 'known', NULL),
    ('Do not contact', 'do not contact', 'do_not_contact');

CREATE TABLE IF NOT EXISTS household_tags (
    household_id INTEGER NOT NULL REFERENCES households(id) ON DELETE CASCADE,
    tag_id       INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    tagged_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (household_id, tag_id)
);
CREATE INDEX IF NOT EXISTS idx_household_tags_tag ON household_tags(tag_id);

CREATE TABLE IF NOT EXISTS visits (
    id           INTEGER PRIMARY KEY,
    household_id INTEGER NOT NULL REFERENCES households(id) ON DELETE CASCADE,
    visit_date   TEXT NOT NULL,     -- ISO date
    comments     TEXT,
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_visits_household ON visits(household_id);
CREATE INDEX IF NOT EXISTS idx_visits_date      ON visits(visit_date);

-- Mirrors for soft_delete_household/restore_deleted_household (issue #19).
-- ON DELETE CASCADE FROM deleted_households(id) means retention pruning
-- (settings::prune_old_deleted_and_logs) cleans these up automatically —
-- no separate sweep needed, and no orphaned rows possible.
CREATE TABLE IF NOT EXISTS deleted_visits (
    id                    INTEGER PRIMARY KEY,
    deleted_household_id  INTEGER NOT NULL REFERENCES deleted_households(id) ON DELETE CASCADE,
    visit_date            TEXT NOT NULL,
    comments              TEXT,
    created_at            TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_deleted_visits_household ON deleted_visits(deleted_household_id);

CREATE TABLE IF NOT EXISTS deleted_household_tags (
    deleted_household_id INTEGER NOT NULL REFERENCES deleted_households(id) ON DELETE CASCADE,
    tag_id                INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (deleted_household_id, tag_id)
);

-- One row per PDF/CSV import run.
CREATE TABLE IF NOT EXISTS import_batches (
    id           INTEGER PRIMARY KEY,
    source_type  TEXT NOT NULL CHECK (source_type IN ('pdf','csv')),
    filename     TEXT NOT NULL,
    imported_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    total_rows   INTEGER NOT NULL DEFAULT 0,
    new_count    INTEGER NOT NULL DEFAULT 0,
    review_count INTEGER NOT NULL DEFAULT 0,
    status       TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','committed','discarded'))
);

-- Rows needing user resolution after a diff. incoming_data is the raw
-- parsed JSON for the candidate row; existing_household_id is set when
-- it's a change/removal against a current record.
CREATE TABLE IF NOT EXISTS review_queue (
    id                    INTEGER PRIMARY KEY,
    import_batch_id       INTEGER NOT NULL REFERENCES import_batches(id) ON DELETE CASCADE,
    match_type            TEXT NOT NULL CHECK (match_type IN ('new','changed','removed')),
    incoming_data         TEXT,          -- JSON, null for 'removed'
    existing_household_id INTEGER REFERENCES households(id) ON DELETE SET NULL,
    resolution            TEXT NOT NULL DEFAULT 'pending'
                           CHECK (resolution IN ('pending','replace','merge','add','delete','ignore')),
    resolution_comment    TEXT,
    resolved_at           TEXT
);
CREATE INDEX IF NOT EXISTS idx_review_queue_batch ON review_queue(import_batch_id);

-- Named groups built from a tag + seed household, feeding the map view.
CREATE TABLE IF NOT EXISTS visit_groups (
    id                INTEGER PRIMARY KEY,
    name              TEXT,
    tag_id            INTEGER REFERENCES tags(id) ON DELETE SET NULL,
    seed_household_id INTEGER REFERENCES households(id) ON DELETE SET NULL,
    created_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE IF NOT EXISTS visit_group_members (
    visit_group_id INTEGER NOT NULL REFERENCES visit_groups(id) ON DELETE CASCADE,
    household_id   INTEGER NOT NULL REFERENCES households(id) ON DELETE CASCADE,
    PRIMARY KEY (visit_group_id, household_id)
);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT OR IGNORE INTO settings (key, value) VALUES
    ('theme', 'css/themes/nordic.css'),
    ('fontSize', '16'),
    ('deletedRetentionDays', '365'),
    ('logRetentionDays', '30'),
    ('logLevel', 'info'),
    ('defaultVisitGroupSize', '10'),
    ('pageSize', '25'),
    ('backupFolder', '');

-- Offline map-tile caching was dropped (issue #3) — this runs on every
-- startup, not just a fresh DB, so it also cleans up an existing
-- install's leftover table/setting from before the removal. Must come
-- after the settings table is created above.
DROP TABLE IF EXISTS cache_regions;
DELETE FROM settings WHERE key = 'mapOfflineCacheEnabled';

-- The seeded "Deleted" tag was a manual label that got confused with
-- actual soft-delete (soft_delete_household moves a row out of
-- households entirely, into deleted_households — a different mechanism).
-- Removed as a tag option; household_tags rows referencing it are
-- cleaned up automatically via ON DELETE CASCADE.
DELETE FROM tags WHERE name = 'Deleted';

CREATE TABLE IF NOT EXISTS logs (
    id         INTEGER PRIMARY KEY,
    level      TEXT NOT NULL CHECK (level IN ('error','warning','info','debug')),
    message    TEXT NOT NULL,
    context    TEXT,  -- JSON blob, optional
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_logs_created ON logs(created_at);
CREATE INDEX IF NOT EXISTS idx_logs_level   ON logs(level);

-- Local road graph (issue #7), built from a user-prepared roads-only
-- .pbf via commands::roads::ingest_road_database. Re-ingesting wipes and
-- replaces both tables inside one transaction — no orphaned data.
CREATE TABLE IF NOT EXISTS road_nodes (
    id     INTEGER PRIMARY KEY,
    osm_id INTEGER NOT NULL UNIQUE,
    lat    REAL NOT NULL,
    lon    REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS road_edges (
    id            INTEGER PRIMARY KEY,
    from_node_id  INTEGER NOT NULL REFERENCES road_nodes(id) ON DELETE CASCADE,
    to_node_id    INTEGER NOT NULL REFERENCES road_nodes(id) ON DELETE CASCADE,
    distance_m    REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_road_edges_from ON road_edges(from_node_id);
CREATE INDEX IF NOT EXISTS idx_road_edges_to   ON road_edges(to_node_id);
