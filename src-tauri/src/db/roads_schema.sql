-- Lost Sheep — roads.db schema (issue #39). Plain SQLite, no SQLCipher,
-- no keychain key: public OSM data, no congregant PII. Lives alongside
-- the main DB file. Applied once on first open via db::open_roads_pool.
--
-- Built/replaced by commands::roads::ingest_road_database. Re-ingesting
-- wipes and replaces both tables inside one transaction — no orphaned
-- data.

CREATE TABLE IF NOT EXISTS road_nodes (
    id     INTEGER PRIMARY KEY,
    osm_id INTEGER NOT NULL UNIQUE,
    lat    REAL NOT NULL,
    lon    REAL NOT NULL
);

-- Issue #45: distinct road name strings, normalized out of road_edges
-- (many edges share the same way/name — storing the string per-edge
-- would duplicate it hundreds of times for zero benefit).
CREATE TABLE IF NOT EXISTS road_names (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

-- FTS5 index for substring/word search on road names (#45) — a plain
-- index on road_names.name only helps exact/prefix match, not
-- "contains" or word search. Content table design: road_names stays
-- the source of truth, this stores only the search index. Full rebuild
-- happens every ingest anyway (roads.db is wiped and reingested
-- wholesale), so the triggers below exist for correctness on any
-- future direct edit, not because ingest itself depends on them.
CREATE VIRTUAL TABLE IF NOT EXISTS road_names_fts USING fts5(
    name,
    content='road_names',
    content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS road_names_ai AFTER INSERT ON road_names BEGIN
    INSERT INTO road_names_fts(rowid, name) VALUES (new.id, new.name);
END;

CREATE TRIGGER IF NOT EXISTS road_names_ad AFTER DELETE ON road_names BEGIN
    INSERT INTO road_names_fts(road_names_fts, rowid, name) VALUES('delete', old.id, old.name);
END;

CREATE TRIGGER IF NOT EXISTS road_names_au AFTER UPDATE ON road_names BEGIN
    INSERT INTO road_names_fts(road_names_fts, rowid, name) VALUES('delete', old.id, old.name);
    INSERT INTO road_names_fts(rowid, name) VALUES (new.id, new.name);
END;

CREATE TABLE IF NOT EXISTS road_edges (
    id            INTEGER PRIMARY KEY,
    from_node_id  INTEGER NOT NULL REFERENCES road_nodes(id) ON DELETE CASCADE,
    to_node_id    INTEGER NOT NULL REFERENCES road_nodes(id) ON DELETE CASCADE,
    distance_m    REAL NOT NULL,
    name_id       INTEGER REFERENCES road_names(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_road_edges_from ON road_edges(from_node_id);
CREATE INDEX IF NOT EXISTS idx_road_edges_to   ON road_edges(to_node_id);
-- Issue #40: bounds overlay query and nearest-node snap lookup both
-- filter road_nodes by a lat/lon box before ranking exactly by
-- haversine distance.
CREATE INDEX IF NOT EXISTS idx_road_nodes_lat_lon ON road_nodes(lat, lon);
