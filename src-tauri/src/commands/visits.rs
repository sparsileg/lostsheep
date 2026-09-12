use crate::road_graph::{grid_cell, load_or_build, RoadGraph, GRID_CELL_DEG};
use crate::AppState;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{BinaryHeap, HashMap, HashSet};
use tauri::State;

/// Parses a date string strictly as a real calendar date in YYYY-MM-DD
/// form. chrono is lenient about zero-padding — "2026-6-5" parses to a
/// valid NaiveDate — so callers must store the value returned by
/// formatting this NaiveDate back out (see record_visit), not the raw
/// input string. Otherwise a non-padded-but-real date still corrupts
/// get_visits_report's lexicographic BETWEEN comparison, which is the
/// core failure this issue is about (#35).
fn parse_iso_date(s: &str) -> Result<chrono::NaiveDate, String> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| format!("'{s}' is not a valid date — use YYYY-MM-DD (e.g. 2026-03-05)"))
}

#[tauri::command]
pub fn record_visit(
    state: State<AppState>,
    household_id: i64,
    visit_date: String,
    comments: Option<String>,
) -> Result<i64, String> {
    let parsed = parse_iso_date(&visit_date)?;
    // Store the canonical zero-padded form chrono formats back out, not
    // whatever the caller sent — this is the backend, load-bearing check
    // (the frontend's is a convenience, not a control, per the issue).
    let normalized = parsed.format("%Y-%m-%d").to_string();
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO visits (household_id, visit_date, comments) VALUES (?1, ?2, ?3)",
        params![household_id, normalized, comments],
    )
    .map_err(|e| e.to_string())?;
    let visit_id = conn.last_insert_rowid();
    super::logs::log(&conn, "info", &format!("visit recorded for household {household_id} on {normalized}"), None);
    Ok(visit_id)
}

#[derive(Serialize)]
pub struct VisitRecord {
    pub id: i64,
    pub household_id: i64,
    pub household_name: String,
    pub visit_date: String,
    pub comments: Option<String>,
}

#[tauri::command]
pub fn get_visits_report(state: State<AppState>, date_from: String, date_to: String) -> Result<Vec<VisitRecord>, String> {
    // A malformed range used to just return an empty table — indistinguishable
    // from "no visits in this period" (#35). Reject it explicitly instead.
    // Normalizing both bounds also means a non-zero-padded but otherwise valid
    // range (e.g. "2026-6-1") compares correctly against the zero-padded
    // dates record_visit now guarantees are in the table.
    let date_from = parse_iso_date(&date_from)?.format("%Y-%m-%d").to_string();
    let date_to = parse_iso_date(&date_to)?.format("%Y-%m-%d").to_string();
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT v.id, v.household_id, \
             h.first_name || ' ' || h.last_name || coalesce(' & ' || h.first_name_2 || ' ' || h.last_name_2, ''), \
             v.visit_date, v.comments \
             FROM visits v JOIN households h ON h.id = v.household_id \
             WHERE v.visit_date BETWEEN ?1 AND ?2 ORDER BY v.visit_date DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![date_from, date_to], |row| {
            Ok(VisitRecord {
                id: row.get(0)?,
                household_id: row.get(1)?,
                household_name: row.get(2)?,
                visit_date: row.get(3)?,
                comments: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// Visit history for a single household — what the household detail
/// modal shows. Separate from get_visits_report, which spans all
/// households over a date range for the reporting use case.
#[derive(Serialize)]
pub struct HouseholdVisit {
    pub id: i64,
    pub visit_date: String,
    pub comments: Option<String>,
}

#[tauri::command]
pub fn get_household_visits(state: State<AppState>, household_id: i64) -> Result<Vec<HouseholdVisit>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, visit_date, comments FROM visits WHERE household_id = ?1 ORDER BY visit_date DESC, id DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![household_id], |row| {
            Ok(HouseholdVisit { id: row.get(0)?, visit_date: row.get(1)?, comments: row.get(2)? })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

#[derive(Deserialize)]
pub struct GenerateVisitListParams {
    pub seed_household_id: i64,
    pub tag_id: Option<i64>, // restrict candidate pool to this tag's group; None = whole DB
    pub count: u32,          // number of DISTINCT ADDRESSES to include
}

#[derive(Serialize, Clone)]
pub struct RoutePathPoint {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Serialize, Clone)]
pub struct VisitListEntry {
    pub address_key: String,
    pub address_line1: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub zip: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
    pub distance_meters: f64,
    pub household_ids: Vec<i64>, // all records sharing this address, included as a unit
    pub names: Vec<String>,
    pub phones: Vec<String>,
    /// "seed" — distance_meters is straight-line distance from the
    /// clicked seed household (today's behavior). "route" — the
    /// routeStartLat/routeStartLon setting is configured, and
    /// distance_meters is the leg distance from the previous stop in a
    /// nearest-neighbor walk (first entry's leg is from the configured
    /// start point). Selection (#13's N addresses) is always seed-based
    /// either way — this only changes ordering/labeling.
    pub distance_context: String,
    /// Issue #38 — only meaningful when distance_context == "route".
    /// "road": distance_meters is real road distance (household → snap
    /// node, A* across road_edges, snap node → household). "straight_line_no_snap":
    /// a road graph exists but one or both endpoints of this leg
    /// couldn't be snapped within tolerance, or no path was found
    /// between them — distance_meters fell back to straight-line for
    /// this leg only. "straight_line_no_graph": no road graph is
    /// ingested at all — every route leg falls back to straight-line,
    /// same as before #38. None for non-route entries (distance_context
    /// "seed"/"none"), where this distinction doesn't apply.
    pub route_distance_source: Option<String>,
    /// Issue #38 follow-up (per Stan): the actual geometry for this leg,
    /// so the map can draw the route following real roads instead of a
    /// straight line household-to-household. Only present when
    /// distance_context == "route". When route_distance_source == "road",
    /// this is [household] → [snap node] → [A* path nodes...] →
    /// [snap node] → [household] — the house-to-road offset is included
    /// as real geometry, not just folded into the distance number.
    /// Otherwise (either straight-line fallback case) this is just
    /// [household, household] — a straight line, same as what the map
    /// already drew before this existed.
    pub route_path: Option<Vec<RoutePathPoint>>,
}

/// Fetches every eligible household row (respecting the optional tag
/// filter and always excluding "do not contact"), then groups by address
/// so multi-head households at the same address are never split. This is
/// the shared, distance-free half of the old generate_visit_list — no
/// seed lookup, no haversine, no sort/truncate, no route walk. Two
/// callers want genuinely different things done with this same grouped
/// data: generate_visit_list orders/limits it by distance for an actual
/// visit list, and get_map_data (map_data.rs) just wants every group
/// plotted with no distance math at all (#31 — that walk was previously
/// borrowed wholesale via a count:100000/seed:0 call, making the map load
/// quadratic in household count once a route start point was configured).
pub fn fetch_grouped_households(
    conn: &rusqlite::Connection,
    tag_id: Option<i64>,
) -> Result<Vec<VisitListEntry>, String> {
    let sql = match tag_id {
        Some(_) => {
            "SELECT h.id, h.address_key, h.address_line1, h.city, h.state, h.zip, h.latitude, h.longitude, \
             h.first_name, h.last_name, h.first_name_2, h.last_name_2, h.phone_1, h.phone_2 \
             FROM households h JOIN household_tags ht ON ht.household_id = h.id \
             WHERE ht.tag_id = ?1 AND h.latitude IS NOT NULL AND h.longitude IS NOT NULL \
             AND h.id NOT IN (SELECT ht2.household_id FROM household_tags ht2 JOIN tags t2 ON t2.id = ht2.tag_id WHERE t2.system_key = 'do_not_contact')"
        }
        None => {
            "SELECT h.id, h.address_key, h.address_line1, h.city, h.state, h.zip, h.latitude, h.longitude, \
             h.first_name, h.last_name, h.first_name_2, h.last_name_2, h.phone_1, h.phone_2 \
             FROM households h WHERE h.latitude IS NOT NULL AND h.longitude IS NOT NULL \
             AND h.id NOT IN (SELECT ht2.household_id FROM household_tags ht2 JOIN tags t2 ON t2.id = ht2.tag_id WHERE t2.system_key = 'do_not_contact')"
        }
    };
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;

    struct Row {
        id: i64, address_key: String, address_line1: Option<String>,
        city: Option<String>, state: Option<String>, zip: Option<String>,
        lat: f64, lon: f64, name: String, phones: Vec<String>,
    }
    fn map_row(r: &rusqlite::Row) -> rusqlite::Result<Row> {
        let name = match r.get::<_, Option<String>>(10)? {
            Some(first2) => format!("{} {} & {} {}", r.get::<_, String>(8)?, r.get::<_, String>(9)?, first2, r.get::<_, Option<String>>(11)?.unwrap_or_default()),
            None => format!("{} {}", r.get::<_, String>(8)?, r.get::<_, String>(9)?),
        };
        let phones: Vec<String> = [r.get::<_, Option<String>>(12)?, r.get::<_, Option<String>>(13)?]
            .into_iter().flatten().collect();
        Ok(Row {
            id: r.get(0)?, address_key: r.get(1)?, address_line1: r.get(2)?,
            city: r.get(3)?, state: r.get(4)?, zip: r.get(5)?,
            lat: r.get(6)?, lon: r.get(7)?, name, phones,
        })
    }
    let rows: Vec<Row> = if let Some(tag_id) = tag_id {
        stmt.query_map(rusqlite::params![tag_id], map_row).map_err(|e| e.to_string())?
    } else {
        stmt.query_map([], map_row).map_err(|e| e.to_string())?
    }
    .filter_map(Result::ok)
    .collect();

    use std::collections::HashMap;
    let mut groups: HashMap<String, Vec<&Row>> = HashMap::new();
    for r in &rows {
        groups.entry(r.address_key.clone()).or_default().push(r);
    }

    let entries: Vec<VisitListEntry> = groups
        .into_iter()
        .map(|(key, members)| {
            let first = members[0];
            VisitListEntry {
                address_key: key,
                address_line1: first.address_line1.clone(),
                city: first.city.clone(),
                state: first.state.clone(),
                zip: first.zip.clone(),
                latitude: first.lat,
                longitude: first.lon,
                distance_meters: 0.0,
                household_ids: members.iter().map(|m| m.id).collect(),
                names: members.iter().map(|m| m.name.clone()).collect(),
                phones: members.iter().flat_map(|m| m.phones.clone()).collect(),
                distance_context: "none".to_string(),
                route_distance_source: None,
                route_path: None,
            }
        })
        .collect();

    Ok(entries)
}

/// Loads the full road graph from roads.db via the shared road_graph
/// module (issue #78), or None if nothing has been ingested (empty
/// road_nodes) or the roads.db pool is unreachable for any reason. None
/// is the "no graph" case throughout this file — never an error, per
/// #38's acceptance criteria: a missing/partial graph must fall back to
/// straight-line, not block generating a visit list.
///
/// Issue #66: this used to run the full roads.db read on every call —
/// measured at ~1.8s of a ~3.6s generate_visit_list call against a real
/// county-sized extract, the single biggest cost in that command. Now a
/// thin wrapper over road_graph::load_or_build(), which also serves
/// diagnostics.rs's find_potential_problems from this same cache — see
/// that function's doc comment for the cache/invalidation contract.
fn load_road_graph(state: &State<AppState>) -> Option<std::sync::Arc<RoadGraph>> {
    let conn = state.roads_pool.get().ok()?;
    load_or_build(&state.road_graph_cache, &conn)
}

/// Cache for snap_to_graph() results within a single generate_visit_list
/// call (issue #79). Keyed on quantized (lat, lon) rather than raw f64
/// (not hashable directly) — multiplied by 1e7 and rounded, giving
/// ~1.1cm precision at the equator, comfortably tighter than geocoding
/// accuracy, so two calls for "the same point" always land on the same
/// key. The route's actual point set (selected stops + seed/route-start)
/// is small and heavily repeated across legs — every 2-opt matrix cell
/// and every shortlist candidate re-snaps the same handful of household
/// coordinates from scratch today; caching means each distinct point is
/// scanned against the full node set at most once per call instead of
/// once per leg that references it.
type SnapCache = HashMap<(i64, i64), Option<(i64, f64)>>;

fn snap_cache_key(lat: f64, lon: f64) -> (i64, i64) {
    ((lat * 1e7).round() as i64, (lon * 1e7).round() as i64)
}

// Meters per degree of latitude — near-constant at any latitude, used
// below to size the grid search radius. Longitude's meters-per-degree
// shrinks with cos(latitude), so that axis is sized separately in
// snap_to_graph rather than reusing this same figure for both.
const METERS_PER_DEGREE_LAT: f64 = 111_320.0;

/// Nearest road node to (lat, lon) within tolerance_m, or None. Issue
/// #66: used to be a full linear scan of every loaded node on a cache
/// miss — fine for a handful of distinct points against a small graph,
/// not against a county-sized extract (hundreds of thousands of nodes).
/// Now queries RoadGraph's grid index (road_graph.rs, built once per
/// call in load_road_graph) instead: only the cells within tolerance_m
/// of (lat, lon) are visited, so cost scales with local node density,
/// not total graph size — matching the same principle roads.rs's
/// get_nearest_road_node already applies via its SQL bounding-box
/// prefilter, just as an in-memory grid instead of a SQL query. Cache is
/// keyed on (lat, lon) only, not tolerance_m — every call site in this
/// file passes the same SNAP_TOLERANCE_M constant, so a single cache per
/// call is correct; if a caller ever needs two different tolerances
/// against the same graph in one invocation, this cache would need
/// splitting by tolerance too.
fn snap_to_graph(graph: &RoadGraph, lat: f64, lon: f64, tolerance_m: f64, cache: &mut SnapCache) -> Option<(i64, f64)> {
    let key = snap_cache_key(lat, lon);
    if let Some(cached) = cache.get(&key) {
        return *cached;
    }

    // Ring radius in cells, computed per axis since a degree of longitude
    // covers fewer meters than a degree of latitude away from the
    // equator. +1 cell of buffer on each so a query point sitting right
    // at a cell edge still sees every node within tolerance_m, even one
    // just across that boundary.
    let lat_rings = (tolerance_m / METERS_PER_DEGREE_LAT / GRID_CELL_DEG).ceil() as i32 + 1;
    let meters_per_degree_lon = (METERS_PER_DEGREE_LAT * lat.to_radians().cos().abs()).max(1.0);
    let lon_rings = (tolerance_m / meters_per_degree_lon / GRID_CELL_DEG).ceil() as i32 + 1;

    let (center_row, center_col) = grid_cell(lat, lon);
    let mut best: Option<(i64, f64)> = None;
    for row in (center_row - lat_rings)..=(center_row + lat_rings) {
        for col in (center_col - lon_rings)..=(center_col + lon_rings) {
            let Some(ids) = graph.grid.get(&(row, col)) else { continue };
            for &id in ids {
                let Some(&(nlat, nlon)) = graph.coords.get(&id) else { continue };
                let d = crate::geo::haversine_meters(lat, lon, nlat, nlon);
                if d <= tolerance_m && best.map_or(true, |(_, bd)| d < bd) {
                    best = Some((id, d));
                }
            }
        }
    }
    cache.insert(key, best);
    best
}

// Ordered by est_total ascending — BinaryHeap is a max-heap by default,
// so Ord is reversed here to make it behave as the min-heap A* needs.
struct AStarFrontierNode {
    cost: f64,
    est_total: f64,
    node: i64,
}
impl PartialEq for AStarFrontierNode {
    fn eq(&self, other: &Self) -> bool {
        self.est_total == other.est_total
    }
}
impl Eq for AStarFrontierNode {}
impl PartialOrd for AStarFrontierNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for AStarFrontierNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.est_total.total_cmp(&self.est_total)
    }
}

// Issue #66 follow-up: astar_distance/astar_path used to allocate a fresh
// HashMap (best_cost, and came_from for the path variant) and BinaryHeap
// on every single call — with ~150 A* calls in a typical generate_
// visit_list run (selection re-rank + nearest-neighbor walk + two_opt's
// O(n^2) matrix), that's ~150 sets of heap allocations for buffers that
// are the same shape every time. Reused here instead: one scratch
// allocation per generate_visit_list call, cleared (not reallocated)
// between A* runs. clear() drops entries but keeps the underlying
// capacity, so after the first few calls warm it up, subsequent calls
// allocate nothing for these three structures at all.
struct AStarScratch {
    best_cost: HashMap<i64, f64>,
    came_from: HashMap<i64, i64>, // only populated/used by astar_path
    open: BinaryHeap<AStarFrontierNode>,
}

impl AStarScratch {
    fn new() -> Self {
        Self { best_cost: HashMap::new(), came_from: HashMap::new(), open: BinaryHeap::new() }
    }
}

/// A* shortest-path distance between two already-snapped nodes, using
/// haversine to the goal as the admissible heuristic (never overestimates
/// true road distance, since road distance is always >= straight-line).
/// None if the goal is unreachable from start within the loaded graph
/// (e.g. two disconnected road-network islands in the ingested extract).
fn astar_distance(graph: &RoadGraph, start: i64, goal: i64, scratch: &mut AStarScratch) -> Option<f64> {
    if start == goal {
        return Some(0.0);
    }
    let goal_coord = *graph.coords.get(&goal)?;

    scratch.best_cost.clear();
    scratch.open.clear();
    scratch.best_cost.insert(start, 0.0);
    scratch.open.push(AStarFrontierNode { cost: 0.0, est_total: 0.0, node: start });

    while let Some(AStarFrontierNode { cost, node, .. }) = scratch.open.pop() {
        if node == goal {
            return Some(cost);
        }
        // Stale heap entry from before a cheaper path to this node was
        // found — skip rather than re-expand.
        if let Some(&known) = scratch.best_cost.get(&node) {
            if cost > known {
                continue;
            }
        }
        if let Some(neighbors) = graph.adjacency.get(&node) {
            for &(next, edge_dist, _) in neighbors {
                let new_cost = cost + edge_dist;
                let is_better = scratch.best_cost.get(&next).map_or(true, |&c| new_cost < c);
                if is_better {
                    scratch.best_cost.insert(next, new_cost);
                    if let Some(&(nlat, nlon)) = graph.coords.get(&next) {
                        let h = crate::geo::haversine_meters(nlat, nlon, goal_coord.0, goal_coord.1);
                        scratch.open.push(AStarFrontierNode { cost: new_cost, est_total: new_cost + h, node: next });
                    }
                }
            }
        }
    }
    None
}

/// Same search as astar_distance, but also reconstructs the actual node
/// path via a predecessor map — only called once per chosen stop (not
/// once per candidate evaluated), since the scan loop above only needs
/// the distance, not the geometry, to pick a winner.
fn astar_path(graph: &RoadGraph, start: i64, goal: i64, scratch: &mut AStarScratch) -> Option<(f64, Vec<i64>)> {
    if start == goal {
        return Some((0.0, vec![start]));
    }
    let goal_coord = *graph.coords.get(&goal)?;

    scratch.best_cost.clear();
    scratch.came_from.clear();
    scratch.open.clear();
    scratch.best_cost.insert(start, 0.0);
    scratch.open.push(AStarFrontierNode { cost: 0.0, est_total: 0.0, node: start });

    while let Some(AStarFrontierNode { cost, node, .. }) = scratch.open.pop() {
        if node == goal {
            let mut path = vec![goal];
            let mut cur = goal;
            while let Some(&prev) = scratch.came_from.get(&cur) {
                path.push(prev);
                cur = prev;
            }
            path.reverse();
            return Some((cost, path));
        }
        if let Some(&known) = scratch.best_cost.get(&node) {
            if cost > known {
                continue;
            }
        }
        if let Some(neighbors) = graph.adjacency.get(&node) {
            for &(next, edge_dist, _) in neighbors {
                let new_cost = cost + edge_dist;
                let is_better = scratch.best_cost.get(&next).map_or(true, |&c| new_cost < c);
                if is_better {
                    scratch.best_cost.insert(next, new_cost);
                    scratch.came_from.insert(next, node);
                    if let Some(&(nlat, nlon)) = graph.coords.get(&next) {
                        let h = crate::geo::haversine_meters(nlat, nlon, goal_coord.0, goal_coord.1);
                        scratch.open.push(AStarFrontierNode { cost: new_cost, est_total: new_cost + h, node: next });
                    }
                }
            }
        }
    }
    None
}

/// Distance-source tag for a single route leg — see VisitListEntry's
/// route_distance_source doc comment for what each value means to the
/// frontend.
enum RouteDistanceSource {
    Road,
    StraightLineNoSnap,
    StraightLineNoGraph,
}
impl RouteDistanceSource {
    fn as_str(&self) -> &'static str {
        match self {
            RouteDistanceSource::Road => "road",
            RouteDistanceSource::StraightLineNoSnap => "straight_line_no_snap",
            RouteDistanceSource::StraightLineNoGraph => "straight_line_no_graph",
        }
    }
}

/// One route leg's distance: household → its snap node (straight-line),
/// A* across road_edges snap node → snap node, then snap node →
/// household (straight-line) — per Stan, the house-to-road offset is
/// real distance and must not be dropped just because the graph itself
/// only has node-to-node edges. Falls back to plain straight-line,
/// flagged via the returned RouteDistanceSource, whenever no graph is
/// loaded, either endpoint can't snap within tolerance, or no path
/// exists between the two snap nodes in the loaded graph.
fn route_leg_distance(
    graph: Option<&RoadGraph>,
    from_lat: f64,
    from_lon: f64,
    to_lat: f64,
    to_lon: f64,
    cache: &mut SnapCache,
    scratch: &mut AStarScratch,
) -> (f64, RouteDistanceSource) {
    let Some(graph) = graph else {
        return (
            crate::geo::haversine_meters(from_lat, from_lon, to_lat, to_lon),
            RouteDistanceSource::StraightLineNoGraph,
        );
    };

    let from_snap = snap_to_graph(graph, from_lat, from_lon, super::roads::SNAP_TOLERANCE_M, cache);
    let to_snap = snap_to_graph(graph, to_lat, to_lon, super::roads::SNAP_TOLERANCE_M, cache);

    match (from_snap, to_snap) {
        (Some((from_node, from_snap_dist)), Some((to_node, to_snap_dist))) => {
            match astar_distance(graph, from_node, to_node, scratch) {
                Some(road_dist) => (from_snap_dist + road_dist + to_snap_dist, RouteDistanceSource::Road),
                None => (
                    crate::geo::haversine_meters(from_lat, from_lon, to_lat, to_lon),
                    RouteDistanceSource::StraightLineNoSnap,
                ),
            }
        }
        _ => (
            crate::geo::haversine_meters(from_lat, from_lon, to_lat, to_lon),
            RouteDistanceSource::StraightLineNoSnap,
        ),
    }
}

/// Geometry for one already-decided leg — called once per chosen stop
/// (after route_leg_distance has already picked the winner), not per
/// candidate. `source` is whatever route_leg_distance already returned
/// for this same (from, to) pair; re-snapping here is cheap and avoids
/// threading extra state through the scan loop just to skip it.
fn route_leg_path(
    graph: Option<&RoadGraph>,
    from_lat: f64,
    from_lon: f64,
    to_lat: f64,
    to_lon: f64,
    source: &RouteDistanceSource,
    cache: &mut SnapCache,
    scratch: &mut AStarScratch,
) -> Vec<RoutePathPoint> {
    let straight = || {
        vec![
            RoutePathPoint { lat: from_lat, lon: from_lon },
            RoutePathPoint { lat: to_lat, lon: to_lon },
        ]
    };

    let is_road = matches!(source, RouteDistanceSource::Road);
    if !is_road {
        return straight();
    }
    let Some(graph) = graph else { return straight() };

    let from_snap = snap_to_graph(graph, from_lat, from_lon, super::roads::SNAP_TOLERANCE_M, cache);
    let to_snap = snap_to_graph(graph, to_lat, to_lon, super::roads::SNAP_TOLERANCE_M, cache);
    let (Some((from_node, _)), Some((to_node, _))) = (from_snap, to_snap) else {
        return straight();
    };
    let Some((_, node_path)) = astar_path(graph, from_node, to_node, scratch) else {
        return straight();
    };

    let mut points = Vec::with_capacity(node_path.len() + 2);
    points.push(RoutePathPoint { lat: from_lat, lon: from_lon });
    for id in &node_path {
        if let Some(&(lat, lon)) = graph.coords.get(id) {
            points.push(RoutePathPoint { lat, lon });
        }
    }
    points.push(RoutePathPoint { lat: to_lat, lon: to_lon });
    points
}

// Safety cap on the 2-opt pass below — the pairwise distance matrix is
// O(n^2) A*/snap calls, computed once up front. At the sizes this app
// actually generates (typically 10-20 stops per list) this is a
// non-issue; the cap exists purely so an unusually large `count` doesn't
// turn into a very long-running command. Past the cap, the
// nearest-neighbor order from the walk above is kept as-is — worse
// ordering, not a hang or an error.
const TWO_OPT_MAX_STOPS: usize = 100;

// Outer-loop safety net — 2-opt on this few stops converges in a
// handful of passes in practice; this cap only exists to guarantee
// termination even in a pathological floating-point edge case, not
// because it's expected to be hit.
const TWO_OPT_MAX_PASSES: usize = 50;

// Same role as TWO_OPT_MAX_PASSES, for or_opt_pass()'s outer loop below.
const OR_OPT_MAX_PASSES: usize = 50;

/// Or-opt pass (#44): relocates a single stop to a different position in
/// the tour, which 2-opt (segment *reversal* only) can never do. Fixes
/// the case 2-opt structurally can't: a stop that needs to move, not
/// flip. Operates on the same `perm`/`dist_start`/`dist_matrix` 2-opt
/// already built — no new distance calls. For every stop, tries removing
/// it and reinserting at every other position (including back at the
/// route start), keeps the best strictly-improving relocation found, and
/// repeats until a full pass finds no improvement or OR_OPT_MAX_PASSES is
/// hit. Chain relocation (moving a run of 2+ consecutive stops together)
/// is out of scope here — single-stop relocation is what the reported
/// stranded-stop symptom needs. Issue #81: the tail position (last stop)
/// is priced against the return trip to start, so a stop far from start
/// is no longer a free place to end the tour.
fn or_opt_pass(perm: &mut Vec<usize>, dist_start: &[f64], dist_matrix: &[Vec<f64>]) {
    let n = perm.len();
    if n < 3 {
        return; // nothing to relocate relative to
    }

    // (None, None) never occurs in practice (n >= 3 here). (Some, None) is
    // "last stop of the route" — issue #81: priced as the closing leg back
    // to the configured start point (dist_start is symmetric start<->stop
    // distance, same reuse 2-opt's leg_dist already makes for the first
    // leg), not free. This is what gives or-opt a reason to relocate a
    // stop that's far from start away from the tail position.
    let edge_cost = |from: Option<usize>, to: Option<usize>| -> f64 {
        match (from, to) {
            (None, Some(t)) => dist_start[t],
            (Some(f), Some(t)) => dist_matrix[f][t],
            (Some(f), None) => dist_start[f],
            (None, None) => 0.0,
        }
    };

    let mut improved = true;
    let mut passes = 0;
    while improved && passes < OR_OPT_MAX_PASSES {
        improved = false;
        passes += 1;
        for i in 0..perm.len() {
            let node = perm[i];
            let prev = if i == 0 { None } else { Some(perm[i - 1]) };
            let next = if i == perm.len() - 1 { None } else { Some(perm[i + 1]) };

            // Cost of closing the gap left by removing `node` from position i.
            let removed_cost = edge_cost(prev, Some(node)) + edge_cost(Some(node), next);
            let gap_cost = edge_cost(prev, next);
            let remove_delta = gap_cost - removed_cost;

            let mut without: Vec<usize> = perm.clone();
            without.remove(i);

            // Best strictly-improving reinsertion point, if any. Threshold
            // matches 2-opt's epsilon so float noise can't cause a
            // no-op swap to look like an improvement.
            let mut best_delta = -1e-9;
            let mut best_pos: Option<usize> = None;
            for k in 0..=without.len() {
                let ins_prev = if k == 0 { None } else { Some(without[k - 1]) };
                let ins_next = if k == without.len() { None } else { Some(without[k]) };
                let insert_cost = edge_cost(ins_prev, Some(node)) + edge_cost(Some(node), ins_next)
                    - edge_cost(ins_prev, ins_next);
                let total_delta = remove_delta + insert_cost;
                if total_delta < best_delta {
                    best_delta = total_delta;
                    best_pos = Some(k);
                }
            }

            if let Some(k) = best_pos {
                without.insert(k, node);
                *perm = without;
                improved = true;
            }
        }
    }
}

/// 2-opt improvement pass over the nearest-neighbor tour built above,
/// followed by an or-opt pass (#44). Nearest-neighbor is a greedy,
/// one-step-at-a-time heuristic — it can leave an expensive stop stranded
/// for last just because something else was marginally closer at every
/// earlier step. 2-opt fixes the worst of that cheaply: repeatedly test
/// whether reversing a segment of the tour shortens the total distance,
/// keep the swap if so, stop when no swap helps. But 2-opt can only
/// reverse contiguous segments — it can never relocate a single stop to a
/// different position — so or_opt_pass() runs afterward to catch exactly
/// that case. Neither guarantees the globally optimal tour, but together
/// they reliably improve on plain nearest-neighbor.
///
/// Distances between every pair of stops (and from the configured start
/// point to every stop) are computed once into a matrix up front and
/// reused for every swap test — the stop *set* doesn't change during
/// 2-opt, only the order, so this avoids re-running A* for the same pair
/// on every pass. Each entry's distance_meters/route_distance_source/
/// route_path is recomputed fresh afterward against the final order,
/// since which pairs end up adjacent changes.
///
/// Issue #81: both this pass and or_opt_pass() price the last stop in the
/// tour against dist_start (the closing leg back to the configured start
/// point), not as a free ending — otherwise neither pass has any reason
/// to avoid stranding a far-from-start stop at the tail. This only
/// changes the *search* cost function; the displayed route still ends at
/// a real stop with its own real leg distance/path, no synthetic return
/// leg is added to the output.
// Issue #66 diagnostic: per-phase timing inside two_opt_improve, logged
// by generate_visit_list's caller so we can see from real usage (Log
// Viewer) which phase actually dominates wall time, instead of guessing.
// dist_start/dist_matrix are where every route_leg_distance() call (snap
// + A*) happens; search/finalize do no distance-graph work of their own
// (search is pure array lookups against the matrix already built;
// finalize re-runs route_leg_distance/route_leg_path once per final
// stop, O(n) not O(n²)).
#[derive(Default)]
struct TwoOptTiming {
    dist_start_ms: f64,
    dist_matrix_ms: f64,
    search_ms: f64,
    finalize_ms: f64,
}

fn two_opt_improve(
    graph: Option<&RoadGraph>,
    start_lat: f64,
    start_lon: f64,
    stops: Vec<VisitListEntry>,
    cache: &mut SnapCache,
    scratch: &mut AStarScratch,
) -> (Vec<VisitListEntry>, TwoOptTiming) {
    let n = stops.len();
    if n < 2 || n > TWO_OPT_MAX_STOPS {
        return (stops, TwoOptTiming::default());
    }

    let t_dist_start = std::time::Instant::now();
    let mut dist_start: Vec<f64> = Vec::with_capacity(n);
    for e in &stops {
        dist_start.push(route_leg_distance(graph, start_lat, start_lon, e.latitude, e.longitude, cache, scratch).0);
    }
    let dist_start_ms = t_dist_start.elapsed().as_secs_f64() * 1000.0;

    let t_dist_matrix = std::time::Instant::now();
    let mut dist_matrix = vec![vec![0.0_f64; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let d = route_leg_distance(graph, stops[i].latitude, stops[i].longitude, stops[j].latitude, stops[j].longitude, cache, scratch).0;
            dist_matrix[i][j] = d;
            dist_matrix[j][i] = d;
        }
    }
    let dist_matrix_ms = t_dist_matrix.elapsed().as_secs_f64() * 1000.0;

    let t_search = std::time::Instant::now();
    let leg_dist = |from: Option<usize>, to: usize| -> f64 {
        match from {
            None => dist_start[to],
            Some(from) => dist_matrix[from][to],
        }
    };

    let mut perm: Vec<usize> = (0..n).collect();
    let mut improved = true;
    let mut passes = 0;
    while improved && passes < TWO_OPT_MAX_PASSES {
        improved = false;
        passes += 1;
        for i in 0..n {
            for j in (i + 1)..n {
                let prev = if i == 0 { None } else { Some(perm[i - 1]) };
                let next = if j == n - 1 { None } else { Some(perm[j + 1]) };
                // next == None means the segment endpoint (perm[j] before
                // the swap, perm[i] after) becomes the last stop of the
                // tour. Issue #81: priced as the closing leg back to the
                // configured start point (dist_start), not free — matches
                // or_opt_pass()'s edge_cost so both passes agree on what a
                // tour costs and neither can undo the other's fix.
                let old_cost = leg_dist(prev, perm[i]) + next.map_or(dist_start[perm[j]], |nx| dist_matrix[perm[j]][nx]);
                let new_cost = leg_dist(prev, perm[j]) + next.map_or(dist_start[perm[i]], |nx| dist_matrix[perm[i]][nx]);
                if new_cost + 1e-9 < old_cost {
                    perm[i..=j].reverse();
                    improved = true;
                }
            }
        }
    }

    // Or-opt pass (#44) — 2-opt above only reverses contiguous segments,
    // so a stop that needs to move to a different spot (not flip) stays
    // stranded no matter how many 2-opt passes run. Reuses this same
    // perm/dist_start/dist_matrix, no new distance calls.
    or_opt_pass(&mut perm, &dist_start, &dist_matrix);
    let search_ms = t_search.elapsed().as_secs_f64() * 1000.0;

    // Materialize the final order, then recompute each leg's
    // distance/source/path fresh — 2-opt/or-opt only tracked bare
    // distances during the search above, not which RouteDistanceSource or
    // path geometry go with each pair.
    let t_finalize = std::time::Instant::now();
    let mut result: Vec<VisitListEntry> = perm.into_iter().map(|i| stops[i].clone()).collect();
    let mut cur = (start_lat, start_lon);
    for entry in result.iter_mut() {
        let (d, source) = route_leg_distance(graph, cur.0, cur.1, entry.latitude, entry.longitude, cache, scratch);
        let path = route_leg_path(graph, cur.0, cur.1, entry.latitude, entry.longitude, &source, cache, scratch);
        entry.distance_meters = d;
        entry.distance_context = "route".to_string();
        entry.route_distance_source = Some(source.as_str().to_string());
        entry.route_path = Some(path);
        cur = (entry.latitude, entry.longitude);
    }
    let finalize_ms = t_finalize.elapsed().as_secs_f64() * 1000.0;

    (result, TwoOptTiming { dist_start_ms, dist_matrix_ms, search_ms, finalize_ms })
}

// How much bigger the haversine-sorted shortlist is than the final N,
// before road-distance re-ranks it down (experiment, gated behind the
// "useRoadDistanceSelection" setting — see generate_visit_list). 3x is a
// starting guess, not tuned against real data yet.
const ROAD_SELECTION_SHORTLIST_MULTIPLIER: usize = 3;

/// Nearest-N generation, grouped by address so multi-head households are
/// never split across the boundary. "Count" caps distinct addresses, not
/// raw rows — matches the "all records sharing an address are included
/// together" rule.
#[tauri::command]
pub fn generate_visit_list(state: State<AppState>, params: GenerateVisitListParams) -> Result<Vec<VisitListEntry>, String> {
    // Issue #66 diagnostic: total wall time plus a per-phase breakdown,
    // logged once at the end via commands::logs::log (DEBUG level, Log
    // Viewer). Grid-indexed snapping (this issue's earlier patch) sped
    // up snap_to_graph itself but didn't change overall wall time in
    // testing — this instrumentation exists to show, from a real call
    // against the real roads.db, which phase actually costs the time
    // instead of guessing from code inspection alone.
    let t_total = std::time::Instant::now();
    let conn = state.pool.get().map_err(|e| e.to_string())?;

    // seed_household_id = 0 is used by map_data::get_map_data for
    // plain "show everything" mode, where distance is unused — falls
    // back to (0,0) instead of erroring on a not-found id.
    let seed: (f64, f64) = conn
        .query_row(
            "SELECT latitude, longitude FROM households WHERE id = ?1",
            rusqlite::params![params.seed_household_id],
            |r| Ok((r.get::<_, Option<f64>>(0)?.unwrap_or(0.0), r.get::<_, Option<f64>>(1)?.unwrap_or(0.0))),
        )
        .unwrap_or((0.0, 0.0));

    let mut entries = fetch_grouped_households(&conn, params.tag_id)?;
    for e in &mut entries {
        e.distance_meters = crate::geo::haversine_meters(seed.0, seed.1, e.latitude, e.longitude);
        e.distance_context = "seed".to_string();
    }

    // total_cmp is total by construction (NaN sorts consistently instead
    // of panicking) — replaces the partial_cmp().unwrap() that crashed on
    // a NaN distance_meters (#24). address_key tiebreak makes ordering
    // reproducible across runs when HashMap iteration puts two groups at
    // the same distance in a different relative order each time.
    entries.sort_by(|a, b| {
        a.distance_meters
            .total_cmp(&b.distance_meters)
            .then_with(|| a.address_key.cmp(&b.address_key))
    });

    // Settings lookups shared by selection and ordering below — read once
    // upfront rather than the single-purpose closure previously scoped
    // inside the route_start block only.
    let get_setting = |key: &str| -> Option<String> {
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            rusqlite::params![key],
            |r| r.get::<_, String>(0),
        )
        .ok()
    };

    // Ordering uses the configured route start point when one exists —
    // nearest-neighbor walk over the selected N addresses, recomputing
    // distance_meters as per-leg distance rather than distance-from-seed.
    // Falls back to today's seed-sorted order when the setting is
    // unconfigured. Read here (before selection) so the road graph below
    // can be loaded once and shared by both selection and ordering.
    let route_start: Option<(f64, f64)> = match (
        get_setting("routeStartLat").and_then(|s| s.parse::<f64>().ok()),
        get_setting("routeStartLon").and_then(|s| s.parse::<f64>().ok()),
    ) {
        (Some(lat), Some(lon)) => Some((lat, lon)),
        _ => None,
    };

    // Re-ranks a haversine-sorted shortlist by real road distance from the
    // seed before truncating to the final N, instead of truncating on
    // straight-line distance alone (#13's original behavior). Default is
    // ON as of tonight's testing — set settings key
    // "useRoadDistanceSelection" to "false" to fall back to plain
    // haversine selection. That fallback path (and the haversine sort
    // above) is kept intact and untouched, so flipping the setting is a
    // full, instant revert with no code removed.
    let use_road_selection = get_setting("useRoadDistanceSelection").as_deref() != Some("false");
    // Issue #38: real road distance per leg, when a graph is ingested and
    // both ends of a leg snap within tolerance. Loaded once here — shared
    // by the road-distance selection re-rank below (if enabled) and the
    // route-ordering walk further down (if a start point is configured) —
    // rather than querying roads.db twice for the same call.
    let t_load_graph = std::time::Instant::now();
    let graph = if use_road_selection || route_start.is_some() {
        load_road_graph(&state)
    } else {
        None
    };
    let load_graph_ms = t_load_graph.elapsed().as_secs_f64() * 1000.0;
    // Issue #79: one snap-result cache shared across the shortlist
    // re-rank below, the nearest-neighbor walk, and two_opt_improve —
    // all three re-snap the same small set of household coordinates
    // (every leg's endpoints are drawn from the selected stops plus the
    // seed/route-start point) repeatedly; caching means each distinct
    // point is scanned against the full node set at most once for this
    // whole call instead of once per leg that references it.
    let mut snap_cache: SnapCache = HashMap::new();
    // Issue #66 follow-up: one reusable A* scratch buffer for this whole
    // call — see AStarScratch's doc comment for why (avoids ~150
    // allocate-per-call HashMap/BinaryHeap pairs across the selection
    // re-rank, the nearest-neighbor walk, and two_opt_improve's matrix).
    let mut astar_scratch = AStarScratch::new();

    let want = params.count as usize;
    let mut selection_ms = 0.0;
    if use_road_selection {
        let t_selection = std::time::Instant::now();
        let shortlist_len = want.saturating_mul(ROAD_SELECTION_SHORTLIST_MULTIPLIER).min(entries.len());
        // Road distance from the seed decides *which* N addresses make
        // the list — distance_meters/distance_context on the kept entries
        // stay the haversine "seed" values already set above, matching
        // VisitListEntry's documented contract for non-route lists.
        // Ordering-for-display still comes from the route-start walk
        // below when one is configured, same as before.
        let mut ranked: Vec<(usize, f64)> = Vec::with_capacity(shortlist_len);
        for (i, e) in entries[..shortlist_len].iter().enumerate() {
            let d = route_leg_distance(graph.as_deref(), seed.0, seed.1, e.latitude, e.longitude, &mut snap_cache, &mut astar_scratch).0;
            ranked.push((i, d));
        }
        ranked.sort_by(|a, b| a.1.total_cmp(&b.1));
        ranked.truncate(want);
        let mut keep_indexes: Vec<usize> = ranked.into_iter().map(|(i, _)| i).collect();
        keep_indexes.sort_unstable();
        entries = keep_indexes.into_iter().map(|i| entries[i].clone()).collect();
        selection_ms = t_selection.elapsed().as_secs_f64() * 1000.0;
    } else {
        entries.truncate(want);
    }

    let mut walk_ms = 0.0;
    let mut two_opt_timing: Option<TwoOptTiming> = None;
    if let Some((start_lat, start_lon)) = route_start {
        let t_walk = std::time::Instant::now();
        let mut remaining = entries;
        let mut ordered: Vec<VisitListEntry> = Vec::with_capacity(remaining.len());
        let mut cur = (start_lat, start_lon);
        while !remaining.is_empty() {
            let mut best_idx = 0;
            let mut best_dist = f64::MAX;
            let mut best_source = RouteDistanceSource::StraightLineNoGraph;
            for (i, e) in remaining.iter().enumerate() {
                let (d, source) = route_leg_distance(graph.as_deref(), cur.0, cur.1, e.latitude, e.longitude, &mut snap_cache, &mut astar_scratch);
                if d < best_dist {
                    best_dist = d;
                    best_idx = i;
                    best_source = source;
                }
            }
            let mut next = remaining.remove(best_idx);
            let path = route_leg_path(graph.as_deref(), cur.0, cur.1, next.latitude, next.longitude, &best_source, &mut snap_cache, &mut astar_scratch);
            next.distance_meters = best_dist;
            next.distance_context = "route".to_string();
            next.route_distance_source = Some(best_source.as_str().to_string());
            next.route_path = Some(path);
            cur = (next.latitude, next.longitude);
            ordered.push(next);
        }
        walk_ms = t_walk.elapsed().as_secs_f64() * 1000.0;
        // 2-opt tour-ordering pass — reuses the same in-memory graph
        // this walk already loaded. Nearest-neighbor above only ever
        // decides "closest unvisited stop next"; this cleans up the
        // worst of that heuristic's mistakes afterward.
        let (improved, timing) = two_opt_improve(graph.as_deref(), start_lat, start_lon, ordered, &mut snap_cache, &mut astar_scratch);
        entries = improved;
        two_opt_timing = Some(timing);
    }

    // Issue #66 diagnostic: one summary row per call, DEBUG level — cheap
    // enough to leave running rather than gating behind a settings flag.
    // route_walk/two_opt fields are only present when a route start point
    // is configured, since that's the only path that runs either.
    let mut timing_msg = format!(
        "stops={} graph_loaded={} load_graph={load_graph_ms:.1}ms",
        entries.len(),
        graph.is_some(),
    );
    if use_road_selection {
        timing_msg.push_str(&format!(" selection_rerank={selection_ms:.1}ms"));
    }
    if let Some(t) = &two_opt_timing {
        timing_msg.push_str(&format!(
            " route_walk={walk_ms:.1}ms two_opt_dist_start={:.1}ms two_opt_dist_matrix={:.1}ms two_opt_search={:.1}ms two_opt_finalize={:.1}ms",
            t.dist_start_ms, t.dist_matrix_ms, t.search_ms, t.finalize_ms
        ));
    }
    timing_msg.push_str(&format!(" total={:.1}ms", t_total.elapsed().as_secs_f64() * 1000.0));
    super::logs::log(&conn, "debug", &timing_msg, Some("visits::generate_visit_list#66"));

    let _dedupe_guard: HashSet<String> = HashSet::new(); // reserved: cross-group id collision guard if needed later
    Ok(entries)
}
