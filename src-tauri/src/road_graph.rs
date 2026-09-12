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

/// One road_edge — from/to node ids and their coordinates (denormalized
/// from road_nodes at build time — see build_road_graph()'s doc comment,
/// issue #80), real distance (meters, from roads.db's own distance_m
/// column, not recomputed here), and the road's name if it has one
/// (road_names, joined via name_id).
pub struct RoadEdge {
    pub from_id: i64,
    pub to_id: i64,
    pub from_lat: f64,
    pub from_lon: f64,
    pub to_lat: f64,
    pub to_lon: f64,
    pub distance_m: f64,
    pub name: Option<String>,
}

/// In-memory copy of the ingested road graph. Built once per caller
/// invocation (once per diagnostics scan, once per generate_visit_list
/// call) — see build_road_graph() below for the per-call query cost this
/// replaced in both former separate loaders.
pub struct RoadGraph {
    /// node_id -> (lat, lon). visits.rs's node-distance snap and A*
    /// heuristic read this directly. diagnostics.rs's bbox-overlap edge
    /// scan (nearby_scored_edges) does NOT read this per-household —
    /// each edge's own endpoint coords are denormalized onto `edges`
    /// below at build time instead (issue #80) — see that field's doc
    /// comment for why.
    pub coords: HashMap<i64, (f64, f64)>,
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
    if let Ok(mut stmt) = conn.prepare("SELECT id, lat, lon FROM road_nodes") {
        if let Ok(rows) = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?, r.get::<_, f64>(2)?))
        }) {
            for (id, lat, lon) in rows.flatten() {
                coords.insert(id, (lat, lon));
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
                edges.push(RoadEdge { from_id, to_id, from_lat, from_lon, to_lat, to_lon, distance_m, name });
            }
        }
    }

    RoadGraph { coords, edges, adjacency }
}
