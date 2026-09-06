// commands/roads.rs — issue #7: ingest an already-prepared, roads-only
// .pbf (clipped + tag-filtered to highway=* ways by Stan's own external
// script — see the issue) into a local node/edge road graph stored in
// the main SQLCipher DB. No acquire/clip/filter here — that step happens
// outside this app, on purpose (Functional_Requirements.md: local-first,
// no network calls). This command only parses, builds, and stores.
//
// Re-ingesting replaces the graph cleanly: the whole parse happens in
// memory first, and only a fully-built graph is written, inside one
// transaction that wipes both tables before inserting. A malformed file
// fails before any of that runs, so the existing graph is untouched.

use crate::AppState;
use osmpbf::{Element, ElementReader};
use std::collections::{HashMap, HashSet};
use tauri::{AppHandle, Emitter, State};

#[derive(Clone, serde::Serialize)]
struct IngestProgress {
    stage: String,
}

// Issue #40 — road-overlay/route-snap support. Started at 1,000,000
// ("effectively no cap") to confirm the mechanism worked; in practice
// that many edges rendered unusably on the map, so cut down to 5,000 —
// still just a constant, adjust again if this is too low/high in
// practice.
const MAX_ROAD_EDGES_PER_QUERY: usize = 5_000;

// Placeholder pre-#38: real routing will decide its own snap tolerance
// when it lands (Dijkstra/A* snapping). This is only for the Road
// Management "show route" overlay's snap-line display, and reuses the
// same 250m figure discussed for #38 so the two don't disagree visually
// once #38 ships.
//
// #38 landed: this is now also the real routing snap tolerance
// (commands::visits::generate_visit_list reuses this same constant via
// super::roads::SNAP_TOLERANCE_M) — kept as one number so the overlay's
// snap lines and the actual route distance never disagree about what
// counts as "close enough to a road."
pub(crate) const SNAP_TOLERANCE_M: f64 = 250.0;

#[derive(serde::Serialize)]
pub struct RoadEdgeSegment {
    pub lat1: f64,
    pub lon1: f64,
    pub lat2: f64,
    pub lon2: f64,
}

#[derive(serde::Serialize)]
pub struct RoadsInBounds {
    pub edges: Vec<RoadEdgeSegment>,
    // true means the query hit MAX_ROAD_EDGES_PER_QUERY and edges was
    // deliberately left empty — caller should show a "zoom in" message
    // rather than render a silently-partial road layer.
    pub truncated: bool,
}

#[derive(serde::Serialize)]
pub struct NearestRoadNode {
    pub lat: f64,
    pub lon: f64,
    pub distance_m: f64,
}

fn emit_progress(app: &AppHandle, stage: &str) {
    // Progress is best-effort UI feedback, not load-bearing — a failed
    // emit (e.g. no listener attached yet) must never abort the ingest.
    let _ = app.emit("road-ingest-progress", IngestProgress { stage: stage.to_string() });
}

struct ParsedWay {
    node_refs: Vec<i64>,
}

fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const EARTH_RADIUS_M: f64 = 6_371_000.0;
    let lat1r = lat1.to_radians();
    let lat2r = lat2.to_radians();
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2) + lat1r.cos() * lat2r.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().asin()
}

/// Parses `file_path`, builds an in-memory node/edge graph, and replaces
/// the stored road graph with it. Runs on Tauri's command thread pool
/// (not the UI thread) same as import_pdf — no explicit spawn needed.
#[tauri::command]
pub fn ingest_road_database(state: State<AppState>, app: AppHandle, file_path: String) -> Result<String, String> {
    let result = (|| -> Result<String, String> {
    // Issue #32: resolved, home-dir-checked path used for both passes
    // below. file_path (raw) is kept only for log messages/error text.
    let resolved = super::paths::resolve_read_path(&file_path)?;
    let file_path = resolved.to_string_lossy().to_string();

    emit_progress(&app, "reading ways");

    let ways_reader = ElementReader::from_path(&file_path)
        .map_err(|e| format!("could not open {file_path}: {e}"))?;

    let mut ways: Vec<ParsedWay> = Vec::new();
    let mut needed_nodes: HashSet<i64> = HashSet::new();

    ways_reader
        .for_each(|el| {
            if let Element::Way(way) = el {
                // File is assumed pre-filtered to roads only (per the
                // issue), but a stray non-road way costs nothing to skip
                // defensively rather than trust blindly.
                if way.tags().any(|(k, _)| k == "highway") {
                    let refs: Vec<i64> = way.refs().collect();
                    if refs.len() >= 2 {
                        for r in &refs {
                            needed_nodes.insert(*r);
                        }
                        ways.push(ParsedWay { node_refs: refs });
                    }
                }
            }
        })
        .map_err(|e| format!("failed reading ways from {file_path}: {e}"))?;

    if ways.is_empty() {
        return Err("no road ways found in file — is it filtered to highway=* ways?".to_string());
    }

    emit_progress(&app, "reading node coordinates");

    // Second pass: osmpbf's ElementReader is a streaming, single-purpose
    // reader, so node coordinates are collected in a fresh pass over the
    // same file rather than trying to interleave with the ways pass.
    let nodes_reader = ElementReader::from_path(&file_path)
        .map_err(|e| format!("could not reopen {file_path}: {e}"))?;

    let mut coords: HashMap<i64, (f64, f64)> = HashMap::with_capacity(needed_nodes.len());
    nodes_reader
        .for_each(|el| match el {
            Element::Node(n) => {
                if needed_nodes.contains(&n.id()) {
                    coords.insert(n.id(), (n.lat(), n.lon()));
                }
            }
            Element::DenseNode(n) => {
                if needed_nodes.contains(&n.id()) {
                    coords.insert(n.id(), (n.lat(), n.lon()));
                }
            }
            _ => {}
        })
        .map_err(|e| format!("failed reading nodes from {file_path}: {e}"))?;

    emit_progress(&app, "building graph");

    let mut node_local_id: HashMap<i64, i64> = HashMap::new();
    let mut node_rows: Vec<(i64, f64, f64)> = Vec::new(); // (osm_id, lat, lon)
    let mut edge_rows: Vec<(i64, i64, f64)> = Vec::new(); // (from_osm_id, to_osm_id, distance_m)

    for way in &ways {
        let mut prev: Option<i64> = None;
        for &node_id in &way.node_refs {
            let Some(&(lat, lon)) = coords.get(&node_id) else {
                // Way references a node this file didn't carry coordinates
                // for — skip just that segment rather than failing the
                // whole ingest.
                prev = None;
                continue;
            };
            if !node_local_id.contains_key(&node_id) {
                node_local_id.insert(node_id, node_rows.len() as i64);
                node_rows.push((node_id, lat, lon));
            }
            if let Some(prev_id) = prev {
                if prev_id != node_id {
                    let (plat, plon) = coords[&prev_id];
                    edge_rows.push((prev_id, node_id, haversine_m(plat, plon, lat, lon)));
                }
            }
            prev = Some(node_id);
        }
    }

    if node_rows.is_empty() || edge_rows.is_empty() {
        return Err("could not resolve any road segments — file may be missing node data".to_string());
    }

    emit_progress(&app, "storing graph");

    // Issue #39: road graph lives in its own plain SQLite file now, not
    // the main SQLCipher DB — write the graph there. `logs` still lives
    // in the main DB, so that write below goes through state.pool as before.
    let mut roads_conn = state.roads_pool.get().map_err(|e| e.to_string())?;
    let tx = roads_conn.transaction().map_err(|e| e.to_string())?;

    tx.execute("DELETE FROM road_edges", []).map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM road_nodes", []).map_err(|e| e.to_string())?;

    {
        let mut insert_node = tx
            .prepare("INSERT INTO road_nodes (osm_id, lat, lon) VALUES (?1, ?2, ?3)")
            .map_err(|e| e.to_string())?;
        for (osm_id, lat, lon) in &node_rows {
            insert_node.execute(rusqlite::params![osm_id, lat, lon]).map_err(|e| e.to_string())?;
        }
    }

    // road_nodes was just wiped, so its rowids restart at 1 in insertion
    // order — read them back to map osm_id -> the row id road_edges needs.
    let mut osm_to_row: HashMap<i64, i64> = HashMap::with_capacity(node_rows.len());
    {
        let mut stmt = tx.prepare("SELECT id, osm_id FROM road_nodes").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (id, osm_id) = row.map_err(|e| e.to_string())?;
            osm_to_row.insert(osm_id, id);
        }
    }

    {
        let mut insert_edge = tx
            .prepare("INSERT INTO road_edges (from_node_id, to_node_id, distance_m) VALUES (?1, ?2, ?3)")
            .map_err(|e| e.to_string())?;
        for (from_osm, to_osm, dist) in &edge_rows {
            let (Some(&from_id), Some(&to_id)) = (osm_to_row.get(from_osm), osm_to_row.get(to_osm)) else {
                continue;
            };
            insert_edge.execute(rusqlite::params![from_id, to_id, dist]).map_err(|e| e.to_string())?;
        }
    }

    tx.commit().map_err(|e| e.to_string())?;

    let log_conn = state.pool.get().map_err(|e| e.to_string())?;
    super::logs::log(
        &log_conn,
        "info",
        &format!("road graph ingested: {} nodes, {} edges from {file_path}", node_rows.len(), edge_rows.len()),
        None,
    );

    emit_progress(&app, "done");
    Ok(format!("{} nodes, {} edges", node_rows.len(), edge_rows.len()))
    })();

    // Issue #27: a failed road ingest previously vanished with no trace.
    if let Err(e) = &result {
        if let Ok(conn) = state.pool.get() {
            super::logs::log(&conn, "error", &format!("road graph ingest failed ({file_path}): {e}"), None);
        }
    }
    result
}

/// Viewport-bounded road overlay query for the Road Management modal's
/// "show roads on map" toggle (issue #40). Returns every edge with at
/// least one endpoint inside the given lat/lon box, capped at
/// MAX_ROAD_EDGES_PER_QUERY — past the cap, `edges` is left empty and
/// `truncated: true` is returned so the caller can show a "zoom in"
/// message instead of rendering a silently-partial layer.
#[tauri::command]
pub fn get_roads_in_bounds(
    state: State<AppState>,
    min_lat: f64,
    max_lat: f64,
    min_lon: f64,
    max_lon: f64,
) -> Result<RoadsInBounds, String> {
    let conn = state.roads_pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT n1.lat, n1.lon, n2.lat, n2.lon
             FROM road_edges e
             JOIN road_nodes n1 ON e.from_node_id = n1.id
             JOIN road_nodes n2 ON e.to_node_id = n2.id
             WHERE (n1.lat BETWEEN ?1 AND ?2 AND n1.lon BETWEEN ?3 AND ?4)
                OR (n2.lat BETWEEN ?1 AND ?2 AND n2.lon BETWEEN ?3 AND ?4)
             LIMIT ?5",
        )
        .map_err(|e| e.to_string())?;

    // Ask for one more than the cap so a truncated result is
    // distinguishable from a result that just happens to land exactly
    // on the cap.
    let query_limit = (MAX_ROAD_EDGES_PER_QUERY + 1) as i64;
    let rows = stmt
        .query_map(
            rusqlite::params![min_lat, max_lat, min_lon, max_lon, query_limit],
            |r| {
                Ok(RoadEdgeSegment {
                    lat1: r.get(0)?,
                    lon1: r.get(1)?,
                    lat2: r.get(2)?,
                    lon2: r.get(3)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;

    let mut edges = Vec::new();
    for row in rows {
        edges.push(row.map_err(|e| e.to_string())?);
        if edges.len() > MAX_ROAD_EDGES_PER_QUERY {
            return Ok(RoadsInBounds { edges: Vec::new(), truncated: true });
        }
    }
    Ok(RoadsInBounds { edges, truncated: false })
}

/// Nearest-road-node lookup for the Road Management modal's "show route"
/// overlay (issue #40) — draws a snap line from a household to whichever
/// road node it's closest to. One call per household, per Stan's
/// decision (simpler code over fewer round trips at this scale).
/// Placeholder ahead of #38's real routing snap logic — same
/// SNAP_TOLERANCE_M so the two don't visually disagree once #38 lands.
/// Returns None when nothing is within tolerance (no ingested road
/// nearby, or no road graph ingested at all).
#[tauri::command]
pub fn get_nearest_road_node(state: State<AppState>, lat: f64, lon: f64) -> Result<Option<NearestRoadNode>, String> {
    let conn = state.roads_pool.get().map_err(|e| e.to_string())?;

    // Rough meters->degrees conversion, padded by 1.5x, to keep this a
    // cheap indexed box lookup rather than scanning every node — exact
    // ranking below is real haversine, this box is only a candidate
    // prefilter.
    let deg_margin = (SNAP_TOLERANCE_M / 111_000.0) * 1.5;
    let mut stmt = conn
        .prepare("SELECT lat, lon FROM road_nodes WHERE lat BETWEEN ?1 AND ?2 AND lon BETWEEN ?3 AND ?4")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(
            rusqlite::params![lat - deg_margin, lat + deg_margin, lon - deg_margin, lon + deg_margin],
            |r| Ok((r.get::<_, f64>(0)?, r.get::<_, f64>(1)?)),
        )
        .map_err(|e| e.to_string())?;

    let mut best: Option<(f64, f64, f64)> = None; // (lat, lon, distance_m)
    for row in rows {
        let (nlat, nlon) = row.map_err(|e| e.to_string())?;
        let d = haversine_m(lat, lon, nlat, nlon);
        if best.as_ref().map_or(true, |b| d < b.2) {
            best = Some((nlat, nlon, d));
        }
    }

    Ok(best.and_then(|(nlat, nlon, d)| {
        if d <= SNAP_TOLERANCE_M {
            Some(NearestRoadNode { lat: nlat, lon: nlon, distance_m: d })
        } else {
            None
        }
    }))
}
