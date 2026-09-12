// road_graph.rs — shared in-memory copy of roads.db (road_nodes +
// road_edges + road_names), used by both commands/diagnostics.rs (issue
// #51) and commands/visits.rs (issue #38). Issue #78: each of those files
// previously loaded its own separate struct/query pair, despite both
// doing nearly identical work (one full read of roads.db, held in memory
// for the duration of one command call, to avoid re-querying per
// household / per BFS hop / per route leg). Unified here so a roads.db
// schema change only needs updating in one place, and so both callers
// stay in sync automatically instead of one picking up a fix the other
// misses.

use std::collections::HashMap;

/// Bucket size (degrees) for the coordinate grid index below. Issue #66:
/// visits.rs's snap_to_graph() used to be a full linear scan of `coords`
/// per cache-miss snap — fine for a small hand-built graph, not for a
/// county-sized extract (hundreds of thousands of nodes). 0.01° is
/// roughly 1.1km of latitude per cell at any latitude, and roughly
/// 1.1km*cos(lat) of longitude — small enough that a snap's search
/// radius (a handful of cells around the query point, see
/// visits.rs::snap_to_graph) touches a small, near-constant number of
/// nodes regardless of total graph size, not a divisor tuned against
/// today's specific extract.
pub const GRID_CELL_DEG: f64 = 0.01;

/// Which grid cell a coordinate falls in. Shared by build time (below)
/// and query time (visits.rs::snap_to_graph) so both always agree on
/// bucket boundaries.
pub fn grid_cell(lat: f64, lon: f64) -> (i32, i32) {
    ((lat / GRID_CELL_DEG).floor() as i32, (lon / GRID_CELL_DEG).floor() as i32)
}

/// One road_edge — from/to node ids and their coordinates (denormalized
/// from road_nodes at build time — see build_road_graph()'s doc comment,
/// issue #80) — and the road's name if it has one (road_names, joined via
/// name_id). Segment distance is NOT carried here: diagnostics.rs's only
/// consumer (nearby_scored_edges) computes its own point-to-segment
/// distance from the coords below, and routing's distance figure lives
/// on `adjacency`'s tuples instead, which is the only thing visits.rs
/// reads. A `distance_m` field here would just be a second, unread copy
/// of that same number.
pub struct RoadEdge {
    pub from_id: i64,
    pub to_id: i64,
    pub from_lat: f64,
    pub from_lon: f64,
    pub to_lat: f64,
    pub to_lon: f64,
    pub name: Option<String>,
}

/// In-memory copy of the ingested road graph. Built once per caller
/// invocation (once per diagnostics scan, once per generate_visit_list
/// call) — see build_road_graph() below for the per-call query cost this
/// replaced in both former separate loaders.
#[derive(Default)]
pub struct RoadGraph {
    /// node_id -> (lat, lon). visits.rs's node-distance snap and A*
    /// heuristic read this directly. diagnostics.rs's bbox-overlap edge
    /// scan (nearby_scored_edges) does NOT read this per-household —
    /// each edge's own endpoint coords are denormalized onto `edges`
    /// below at build time instead (issue #80) — see that field's doc
    /// comment for why.
    pub coords: HashMap<i64, (f64, f64)>,
    /// Grid-bucket index over `coords` (issue #66) — grid_cell(lat, lon)
    /// -> every node id whose coords fall in that cell. visits.rs's
    /// snap_to_graph() queries this instead of scanning all of `coords`
    /// on a cache miss. diagnostics.rs doesn't use this — its
    /// nearby_scored_edges() already has its own bbox-overlap scan over
    /// `edges`, unrelated to node snapping.
    pub grid: HashMap<(i32, i32), Vec<i64>>,
    /// Every edge, coords included, for diagnostics.rs's
    /// nearby_scored_edges() full-scan-and-score — see RoadEdge's doc
    /// comment (issue #80) for why coords live here, not looked up from
    /// `coords` per household. visits.rs doesn't use this field — it
    /// only walks `adjacency`.
    pub edges: Vec<RoadEdge>,
    /// node_id -> every (neighbor_node_id, edge_distance_m, edge_name)
    /// reachable by one edge — both directions of each edge are present
    /// as two entries (roads.rs's ingest doesn't track one-way tags, so
    /// every edge is walkable both ways here regardless of which end was
    /// recorded as from/to).
    pub adjacency: HashMap<i64, Vec<(i64, f64, Option<String>)>>,
}

impl RoadGraph {
    /// No nodes loaded at all — no roads.db ingested, or the roads pool
    /// was unreachable. Callers that need "fall back to straight-line"
    /// (visits.rs) or "skip road-name checks" (diagnostics.rs) behavior
    /// should check this rather than re-deriving it from coords/edges
    /// directly.
    pub fn is_empty(&self) -> bool {
        self.coords.is_empty()
    }
}

/// Single full read of road_nodes + road_edges (LEFT JOIN road_names for
/// the name) — replaces every per-household/per-hop/per-route-leg query
/// the two former separate loaders (diagnostics.rs's build_road_graph,
/// visits.rs's load_road_graph) used to issue against roads_conn
/// directly. Returns an empty graph (rather than erroring) if either
/// query fails to prepare or run, matching both former loaders' existing
/// behavior of returning nothing on a prepare error instead of failing
/// the whole call.
pub fn build_road_graph(conn: &rusqlite::Connection) -> RoadGraph {
    let mut coords: HashMap<i64, (f64, f64)> = HashMap::new();
    // Issue #66: built alongside coords, one pass, so there is no separate
    // "index the graph" step after the fact — a node is in the grid the
    // instant it exists in coords.
    let mut grid: HashMap<(i32, i32), Vec<i64>> = HashMap::new();
    if let Ok(mut stmt) = conn.prepare("SELECT id, lat, lon FROM road_nodes") {
        if let Ok(rows) = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?, r.get::<_, f64>(2)?))
        }) {
            for (id, lat, lon) in rows.flatten() {
                coords.insert(id, (lat, lon));
                grid.entry(grid_cell(lat, lon)).or_default().push(id);
            }
        }
    }

    let mut edges = Vec::new();
    let mut adjacency: HashMap<i64, Vec<(i64, f64, Option<String>)>> = HashMap::new();
    let query = "SELECT e.from_node_id, e.to_node_id, e.distance_m, rn.name \
                 FROM road_edges e \
                 LEFT JOIN road_names rn ON rn.id = e.name_id";
    if let Ok(mut stmt) = conn.prepare(query) {
        if let Ok(rows) = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        }) {
            for row in rows.flatten() {
                let (from_id, to_id, distance_m, name) = row;
                adjacency.entry(from_id).or_default().push((to_id, distance_m, name.clone()));
                adjacency.entry(to_id).or_default().push((from_id, distance_m, name.clone()));
                // Issue #80: coords resolved once here, at build time —
                // one HashMap lookup per edge, total, for the whole
                // graph. #78 had dropped these fields from RoadEdge and
                // made diagnostics.rs's nearby_scored_edges() look them
                // up from `coords` itself, once per edge PER HOUSEHOLD —
                // silently reintroducing the per-household/per-item cost
                // #51 existed to eliminate (2 HashMap lookups x every
                // edge x every household, instead of 2 lookups x every
                // edge, once). A dangling from_id/to_id (shouldn't
                // happen with a consistent ingest, but roads.db is a
                // separate, independently re-ingestible file) skips the
                // edge entirely — matches the old pre-#78
                // build_road_graph()'s INNER JOIN behavior of only
                // including edges with valid endpoints.
                let (Some(&(from_lat, from_lon)), Some(&(to_lat, to_lon))) =
                    (coords.get(&from_id), coords.get(&to_id))
                else {
                    continue;
                };
                edges.push(RoadEdge { from_id, to_id, from_lat, from_lon, to_lat, to_lon, name });
            }
        }
    }

    RoadGraph { coords, grid, edges, adjacency }
}

/// Cache-aware graph load, shared by both callers (issue #66 follow-up):
/// visits.rs's generate_visit_list and diagnostics.rs's
/// find_potential_problems each used to run their own full roads.db
/// rebuild on every call — this was the single biggest per-call cost in
/// generate_visit_list (measured ~1.8s of ~3.6s against a real
/// county-sized extract) and diagnostics.rs paid the identical cost
/// independently. One cache, in AppState, shared by both: whichever
/// caller runs first each app session pays the build; the other gets it
/// for free. A successful road-graph re-ingest
/// (roads.rs::ingest_road_database) clears the cache once, and both
/// callers pick that up automatically — neither has (or needs) its own
/// invalidation path.
///
/// Returns None when nothing has been ingested (empty road_nodes) or the
/// roads.db pool is unreachable — deliberately NOT cached, since that
/// query is already cheap and caching "no graph yet" would hide a graph
/// ingested later behind whichever cache-clearing path happened to run.
pub fn load_or_build(
    cache: &std::sync::Mutex<Option<std::sync::Arc<RoadGraph>>>,
    roads_conn: &rusqlite::Connection,
) -> Option<std::sync::Arc<RoadGraph>> {
    {
        let cached = cache.lock().unwrap();
        if let Some(graph) = cached.as_ref() {
            return Some(graph.clone()); // Arc clone — refcount bump, not a data copy
        }
    }

    let graph = build_road_graph(roads_conn);
    if graph.is_empty() {
        return None;
    }
    let graph = std::sync::Arc::new(graph);
    *cache.lock().unwrap() = Some(graph.clone());
    Some(graph)
}
