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

CREATE TABLE IF NOT EXISTS road_edges (
    id            INTEGER PRIMARY KEY,
    from_node_id  INTEGER NOT NULL REFERENCES road_nodes(id) ON DELETE CASCADE,
    to_node_id    INTEGER NOT NULL REFERENCES road_nodes(id) ON DELETE CASCADE,
    distance_m    REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_road_edges_from ON road_edges(from_node_id);
CREATE INDEX IF NOT EXISTS idx_road_edges_to   ON road_edges(to_node_id);
