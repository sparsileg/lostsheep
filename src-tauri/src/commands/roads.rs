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

use crate::geo;
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
pub(crate) const SNAP_TOLERANCE_M: f64 = 300.0;

// Issue #53: an unclipped regional/national .pbf (a full Geofabrik
// state/country download, sitting right next to the properly clipped
// extract, same .pbf extension, no way for the file picker to tell them
// apart) blows the whole-parse-in-memory ingest past available RAM —
// OOM-killed on Linux, allocation failure on Windows, no error, no log
// entry, just the app vanishing mid-ingest. Calibrated against this
// project's own known-good extract: a 6.9 MiB roads-only .pbf produced a
// ~38 MB roads.db (~5.5x expansion — pbf's varint/delta/zlib encoding is
// far denser than the flat node/edge rows this app stores). 100 MiB
// roads-only input would extrapolate to roughly half a gigabyte in
// memory — generous headroom over any real clipped county/region extract
// while stopping well short of a multi-gigabyte accidental ingest. A
// single named constant per the issue's constraint — retune here if a
// legitimately larger clipped extract ever needs more room.
const MAX_INGEST_PBF_BYTES: u64 = 100 * 1024 * 1024;

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

// Straight-line-only routing investigation: a .pbf clipped without a
// buffer at its extract boundary can cut the road network into several
// disconnected pieces even though every individual segment still has
// valid geometry — invisible on a rendered map overlay (every edge still
// draws correctly), but fatal to A* routing (visits.rs::astar_distance
// returns None whenever start/goal fall in different pieces, and every
// such leg silently falls back to straight-line — see
// RouteDistanceSource::StraightLineNoSnap). This summary runs at ingest
// time, on the exact same osm-id adjacency the routing graph will use
// once written and loaded, so a bad extract is visible immediately in
// the ingest log instead of only showing up later as "routes look wrong."
struct ConnectivitySummary {
    total_nodes: usize,
    component_count: usize,
    largest_component_size: usize,
    // Components below this size are almost certainly clipping
    // artifacts (a road stub cut at the extract boundary), not a real,
    // separate road network — counted separately from component_count
    // so the log line can distinguish "one big network plus boundary
    // debris" from "genuinely fragmented."
    small_component_count: usize,
}

const SMALL_COMPONENT_THRESHOLD: usize = 5;

/// Union-find over the osm node ids actually present in `node_rows`,
/// unioned by every edge in `edge_rows` (both already guaranteed to
/// reference resolvable nodes — see the ways-parsing loop above). Cheap
/// (near-linear with path compression) even at hundreds of thousands of
/// edges, so this runs unconditionally on every ingest rather than being
/// gated behind a flag.
fn summarize_connectivity(
    node_rows: &[(i64, f64, f64)],
    edge_rows: &[(i64, i64, f64, Option<String>)],
) -> ConnectivitySummary {
    let mut parent: HashMap<i64, i64> = HashMap::with_capacity(node_rows.len());
    for (osm_id, _, _) in node_rows {
        parent.insert(*osm_id, *osm_id);
    }

    fn find(parent: &mut HashMap<i64, i64>, x: i64) -> i64 {
        let mut root = x;
        while parent[&root] != root {
            root = parent[&root];
        }
        let mut cur = x;
        while parent[&cur] != root {
            let next = parent[&cur];
            parent.insert(cur, root);
            cur = next;
        }
        root
    }

    for (from_osm, to_osm, _, _) in edge_rows {
        let ra = find(&mut parent, *from_osm);
        let rb = find(&mut parent, *to_osm);
        if ra != rb {
            parent.insert(ra, rb);
        }
    }

    let mut sizes: HashMap<i64, usize> = HashMap::new();
    for (osm_id, _, _) in node_rows {
        let root = find(&mut parent, *osm_id);
        *sizes.entry(root).or_insert(0) += 1;
    }

    ConnectivitySummary {
        total_nodes: node_rows.len(),
        component_count: sizes.len(),
        largest_component_size: sizes.values().copied().max().unwrap_or(0),
        small_component_count: sizes.values().filter(|&&s| s < SMALL_COMPONENT_THRESHOLD).count(),
    }
}

struct ParsedWay {
    node_refs: Vec<i64>,
    // Issue #45: OSM's `name` tag, when present, ends up on every edge
    // this way produces (via edge_rows below), normalized into
    // road_names at DB-write time. None means the way has no `name`
    // tag — legitimate, not an error.
    name: Option<String>,
}

// #65: private copy removed — this used to omit the .min(1.0) clamp
// crate::geo::haversine_meters carries for near-antipodal-point rounding
// (#24), so it could return NaN on a malformed .pbf's coordinates where
// the canonical one couldn't. Both call sites below now call the
// canonical function directly instead.

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

    // Issue #53: refuse before ever opening the file — an unclipped
    // regional/national extract has the same .pbf extension as a properly
    // clipped one, and the whole-parse-in-memory design (see this file's
    // header comment) has no other chance to bail out before accumulating
    // enough to be OOM-killed.
    let file_size = std::fs::metadata(&resolved)
        .map_err(|e| format!("could not read {file_path}: {e}"))?
        .len();
    if file_size > MAX_INGEST_PBF_BYTES {
        return Err(format!(
            "{file_path} is {:.0} MB, over the {} MB ingest limit — this looks like a full regional \
             extract rather than a roads-only clip. Clip it to your area first (see the Road Management \
             instructions), then try again.",
            file_size as f64 / (1024.0 * 1024.0),
            MAX_INGEST_PBF_BYTES / (1024 * 1024)
        ));
    }

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
                        let name = way
                            .tags()
                            .find(|(k, _)| *k == "name")
                            .map(|(_, v)| v.to_string());
                        ways.push(ParsedWay { node_refs: refs, name });
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
    // (from_osm_id, to_osm_id, distance_m, way_name) — way_name cloned
    // per edge here (many edges share one way's name); deduped back down
    // to one road_names row per distinct string at DB-write time below.
    let mut edge_rows: Vec<(i64, i64, f64, Option<String>)> = Vec::new();

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
                    edge_rows.push((prev_id, node_id, geo::haversine_meters(plat, plon, lat, lon), way.name.clone()));
                }
            }
            prev = Some(node_id);
        }
    }

    // Issue #45: distinct road names across this ingest, in first-seen
    // order — inserted into road_names below, then mapped back to real
    // row ids the same way node osm_ids are mapped back after insert.
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut name_rows: Vec<String> = Vec::new();
    for (_, _, _, name) in &edge_rows {
        if let Some(n) = name {
            if seen_names.insert(n.clone()) {
                name_rows.push(n.clone());
            }
        }
    }

    if node_rows.is_empty() || edge_rows.is_empty() {
        return Err("could not resolve any road segments — file may be missing node data".to_string());
    }

    emit_progress(&app, "checking connectivity");
    let connectivity = summarize_connectivity(&node_rows, &edge_rows);

    emit_progress(&app, "storing graph");

    // Issue #39: road graph lives in its own plain SQLite file now, not
    // the main SQLCipher DB — write the graph there. `logs` still lives
    // in the main DB, so that write below goes through state.pool as before.
    let mut roads_conn = state.roads_pool.get().map_err(|e| e.to_string())?;
    let tx = roads_conn.transaction().map_err(|e| e.to_string())?;

    tx.execute("DELETE FROM road_edges", []).map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM road_nodes", []).map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM road_names", []).map_err(|e| e.to_string())?;

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
        let mut insert_name = tx
            .prepare("INSERT INTO road_names (name) VALUES (?1)")
            .map_err(|e| e.to_string())?;
        for name in &name_rows {
            insert_name.execute(rusqlite::params![name]).map_err(|e| e.to_string())?;
        }
    }

    // road_names was just wiped too — read back the same way osm_to_row
    // is built above, rather than assuming rowid order.
    let mut name_to_id: HashMap<String, i64> = HashMap::with_capacity(name_rows.len());
    {
        let mut stmt = tx.prepare("SELECT id, name FROM road_names").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (id, name) = row.map_err(|e| e.to_string())?;
            name_to_id.insert(name, id);
        }
    }

    {
        let mut insert_edge = tx
            .prepare("INSERT INTO road_edges (from_node_id, to_node_id, distance_m, name_id) VALUES (?1, ?2, ?3, ?4)")
            .map_err(|e| e.to_string())?;
        for (from_osm, to_osm, dist, name) in &edge_rows {
            let (Some(&from_id), Some(&to_id)) = (osm_to_row.get(from_osm), osm_to_row.get(to_osm)) else {
                continue;
            };
            let name_id: Option<i64> = name.as_ref().and_then(|n| name_to_id.get(n)).copied();
            insert_edge
                .execute(rusqlite::params![from_id, to_id, dist, name_id])
                .map_err(|e| e.to_string())?;
        }
    }

    tx.commit().map_err(|e| e.to_string())?;

    // Issue #66: the just-committed graph is now stale in memory (if
    // anything had loaded it yet) — clear the cache so the next
    // generate_visit_list call rebuilds from the freshly-ingested data
    // instead of serving the old graph indefinitely.
    *state.road_graph_cache.lock().unwrap() = None;

    let largest_pct = if connectivity.total_nodes > 0 {
        connectivity.largest_component_size as f64 / connectivity.total_nodes as f64 * 100.0
    } else {
        0.0
    };
    let log_conn = state.pool.get().map_err(|e| e.to_string())?;
    super::logs::log(
        &log_conn,
        "info",
        &format!(
            "road graph ingested: {} nodes, {} edges, {} road names from {file_path} — \
             connectivity: {} component(s), largest holds {} node(s) ({largest_pct:.1}% of total), \
             {} small (<{SMALL_COMPONENT_THRESHOLD}-node) component(s)",
            node_rows.len(),
            edge_rows.len(),
            name_rows.len(),
            connectivity.component_count,
            connectivity.largest_component_size,
            connectivity.small_component_count,
        ),
        None,
    );

    emit_progress(&app, "done");
    Ok(format!(
        "{} nodes, {} edges, {} road names — {} connected component(s), largest covers {largest_pct:.1}% of the network",
        node_rows.len(),
        edge_rows.len(),
        name_rows.len(),
        connectivity.component_count,
    ))
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
        let d = geo::haversine_meters(lat, lon, nlat, nlon);
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
