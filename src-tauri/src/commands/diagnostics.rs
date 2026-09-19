// commands/diagnostics.rs — issue #48: "Potential problems" diagnostic
// scan, reachable from the hamburger menu. Read-only: runs a fixed set of
// checks against every household and returns a combined report. Nothing
// here writes to households, visits, or tags.
//
// Address parsing: split address_line1 on whitespace, drop every leading
// word that starts with a digit (house number "328"; fraction/dash tokens
// like "1/2" that follow it; a number-letter unit suffix glued on with no
// space, like "26A"), then drop the trailing word (assumed to be the road
// type — St/Ave/Ct/Dr/etc), leaving the household's road name (H).
//
// Matching H against a road database name (R) — four rules (#48
// follow-up), because neither H nor R reliably carries a real trailing
// type word (H always has one dropped already; R sometimes has one to
// drop, sometimes doesn't — "Back Creek" in roads.db has no separate type
// word at all, while "Chestnut Grove Road" does):
//
//   Rule 1 — try three word-count alignments before giving up: drop the
//   last word from both H and R, drop it from H only, or drop it from R
//   only. A match on any of the three counts as agreement.
//
//   Rule 2 — expand single-letter directionals (N/E/S/W) to their full
//   word (North/East/South/West) on both sides before comparing, so
//   "N Main" and "North Main" aren't treated as different roads.
//
//   Rule 3 — if nothing within snap tolerance is named at all, walk
//   outward from the nearest edge's endpoints (driveways are often
//   several unnamed road_edges long, not just one hop) until a named road
//   is found, and compare that name against H instead of giving up
//   immediately.
//
//   Rule 4 — if a nearby name doesn't match H, also walk outward the same
//   way before concluding it's a real mismatch — the household may be
//   snapping to a short unnamed or differently-named stub right next to
//   the road it's actually on.
//
// Candidate selection (#48 follow-up, round 3): every named road_edge
// within SNAP_TOLERANCE_M is checked against H directly — not just the
// single nearest edge. A driveway or trail can legitimately be a few
// meters closer than the household's actual (named) street; matching
// only the nearest edge, then walking outward from *that specific edge's*
// endpoints if it didn't match, could miss a real match sitting a few
// meters further out in the same candidate list, if the nearest edge's
// own graph component never reached it within the hop limit. See
// road_name_problem()'s doc comment for the household that exposed this.
//
// Snap approach: road **segments**, found by edge bounding-box overlap
// against a search box around the household — not nearest road node, and
// not anchored on nearby nodes either (#48 follow-up, round 2). Round 1:
// node-only snap could pick a distant node on a road that happens to have
// one nearby over a visually closer road with none in range. Round 2:
// anchoring edge selection on the 3 nearest *nodes* still missed a road
// whenever the household sat near the middle of one of its long edges,
// far from either endpoint node, while a shorter nearby road's endpoints
// happened to be closer. Fixed by dropping node proximity from edge
// selection entirely — every edge whose own bbox overlaps the search box
// is scored by point-to-segment distance, regardless of how far its
// endpoints are. See nearby_scored_edges().
//
// If roads.db has no ingested road names at all, both checks are skipped
// for every household — an empty/un-ingested roads database would
// otherwise flag every single address as "not found", which isn't a
// useful report, just an unconfigured database. The no-address and
// no-geocoords checks below still run regardless.
//
// Per Stan (#48 design decision): a record with no address_line1 is
// flagged "No address on file" and neither check runs against it. A
// record with an address but no geocoords still runs check 1, plus gets
// an explicit "No geocoordinates on file" flag (check 2 can't run without
// coordinates to snap).

use crate::road_graph::{self, RoadGraph};
use crate::AppState;
use serde::Serialize;
use std::collections::HashMap;
use std::collections::HashSet;

/// Two coordinates at the same address within this distance are treated
/// as "the same place" for the shared-address geocoordinate check below
/// — small re-geocode jitter (different geocoder run, rounding) shouldn't
/// flag every multi-resident address. Change this one value to retune.
const ADDRESS_COORD_TOLERANCE_METERS: f64 = 33.0;
use tauri::{AppHandle, Emitter, State};

#[derive(Serialize)]
pub struct PotentialProblem {
    pub household_id: i64,
    pub household_name: String,
    /// Present only on a shared-address group entry (see the address-key
    /// coordinate-mismatch check below) — every household_id sharing that
    /// address, household_id/household_name above being the first of
    /// them. Absent (omitted from JSON) for an ordinary single-household
    /// entry, which frontend code should treat as its signal to render
    /// household_id/household_name alone instead of a member list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub household_ids: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub household_names: Option<Vec<String>>,
    /// Present only on a shared-address group entry, parallel to
    /// household_ids/household_names (same index = same household). Each
    /// entry is that member's own tag (None if untagged) — the top-level
    /// `tag` field stays None for a group since members may differ; this
    /// is how the PDF shows each resident's tag individually instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub household_tags: Option<Vec<Option<String>>>,
    pub address_line1: Option<String>,
    pub address_line2: Option<String>,
    /// The household's tag (Known / Not known / Do Not Contact), if any —
    /// households are capped at one tag apiece. None when untagged.
    pub tag: Option<String>,
    pub reasons: Vec<String>,
    /// Populated for only the first DEBUG_TRACE_LIMIT households that
    /// actually get flagged during the road-name/snap check (#48
    /// follow-up — "no name on file" investigation). Every candidate node
    /// considered, every edge scored, the winning segment, and the full
    /// hop-by-hop walk, so it can be read straight off the printed report.
    /// None (omitted from JSON) for every other household, and always
    /// None once DEBUG_TRACE_LIMIT captures have been made. Temporary
    /// instrumentation — safe to strip once the underlying pattern is
    /// found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_trace: Option<Vec<String>>,
}

/// Issue #80: an empty/un-ingested roads.db and a fully-ingested one with
/// no findings used to produce the identical response — a bare
/// Vec<PotentialProblem>, empty either way. roads_checked distinguishes
/// them: false means the road-name/snap checks below never ran at all
/// for any household (see roads_available), not that they ran and found
/// nothing wrong. sidebar.js's showPotentialProblemsModal() and
/// data-validation-pdf.js both key off this to show a distinct warning
/// instead of "no potential problems found."
#[derive(Serialize)]
pub struct DiagnosticsReport {
    pub problems: Vec<PotentialProblem>,
    pub roads_checked: bool,
}

/// Debug-trace budget for the road-name/snap check below — only the first
/// this many *flagged* households get a trace attached, not the first
/// this many households overall. Keeps the report from ballooning when
/// the flagged reason (currently "no name on file") is the common case.
const DEBUG_TRACE_LIMIT: usize = 10;

#[derive(Serialize, Clone)]
struct DiagnosticsProgress {
    processed: usize,
    total: usize,
}

/// Emits at most once per 10 households (plus always the final one) —
/// same throttle as import.rs's emit_progress, enough for a live
/// percentage without flooding IPC at this app's up-to-10,000-household
/// scale.
fn emit_diagnostics_progress(app: &AppHandle, processed: usize, total: usize) {
    if processed % 10 == 0 || processed == total {
        let _ = app.emit("diagnostics-progress", DiagnosticsProgress { processed, total });
    }
}

/// Hard cap on trace lines per household — a dense road grid could in
/// theory make the hop-by-hop walk (walk_to_named_roads) fan out into
/// hundreds of edges per hop. Truncates rather than silently growing the
/// report unbounded; the walk itself still runs to completion regardless.
const TRACE_LINE_CAP: usize = 200;

fn trace_push(trace: &mut Option<&mut Vec<String>>, line: String) {
    if let Some(t) = trace.as_mut() {
        if t.len() < TRACE_LINE_CAP {
            t.push(line);
        } else if t.len() == TRACE_LINE_CAP {
            t.push("... trace truncated (line cap reached) ...".to_string());
        }
    }
}

/// Drops the last word of a name if there are 2+ words; returns it as-is
/// (trimmed) otherwise. Used both on the household's parsed road name and
/// on raw road_names.name values — never assume which one actually has a
/// type-word suffix to drop (see module doc, Rule 1).
fn drop_last(s: &str) -> String {
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.len() < 2 {
        return s.trim().to_string();
    }
    words[..words.len() - 1].join(" ")
}

/// Rule 2: single-letter directionals, matched whole-word and
/// case-insensitively, expand to their full word. A trailing period
/// ("N.", "S.") is stripped before matching and dropped from the
/// result — "N." expands to "North", not "North.". Every other word
/// (including one whose only non-letter content is elsewhere, e.g.
/// "N/A") is left untouched.
fn expand_directional(word: &str) -> String {
    let trimmed = word.strip_suffix('.').unwrap_or(word);
    match trimmed.to_uppercase().as_str() {
        "N" => "North".to_string(),
        "E" => "East".to_string(),
        "S" => "South".to_string(),
        "W" => "West".to_string(),
        _ => word.to_string(),
    }
}

fn normalize_name(s: &str) -> String {
    s.split_whitespace().map(expand_directional).collect::<Vec<_>>().join(" ")
}

/// Rules 1+2 together: normalize both sides (directional expansion), then
/// try all three drop-alignments. Used for check 2's small per-household
/// candidate lists (nearest-node names, connected-road names), where
/// iterating candidates directly is cheap.
fn names_match(candidates: &[String], household_road_name: &str) -> bool {
    let h = normalize_name(household_road_name);
    let h_lower = h.to_lowercase();
    let h_dropped_lower = drop_last(&h).to_lowercase();

    candidates.iter().any(|c| {
        let r = normalize_name(c);
        let r_lower = r.to_lowercase();
        let r_dropped_lower = drop_last(&r).to_lowercase();

        h_dropped_lower == r_dropped_lower // drop both sides
            || h_dropped_lower == r_lower // drop household side only
            || h_lower == r_dropped_lower // drop road-db side only
    })
}

/// True if `word` is "Apt" (any case), with or without a trailing period
/// ("Apt", "APT.", "apt").
fn is_apt_word(word: &str) -> bool {
    word.strip_suffix('.').unwrap_or(word).eq_ignore_ascii_case("Apt")
}

/// Drops the assumed road-type word (the last one) and every leading word
/// that starts with a digit — a house number ("328"), a fraction/dash
/// token that follows it ("1/2"), or a number-letter unit suffix glued on
/// with no space ("26A"). A trailing "Apt <word>" pair (unit number or
/// alphanumeric unit, e.g. "Apt 421") is stripped first, before any of
/// the above — "400 Clocktower Ridge Drive Apt 421" drops "Apt 421", then
/// "400" and "Drive" per the existing rules, leaving "Clocktower Ridge".
/// Returns whatever's left joined back into one string. None when there's
/// nothing left to check (empty, only leading number-like tokens + type
/// word with nothing between, or a single word with no separate type word
/// to drop).
fn road_name_from_address(address_line1: &str) -> Option<String> {
    let mut words: Vec<&str> = address_line1.split_whitespace().collect();
    if words.len() >= 3 && is_apt_word(words[words.len() - 2]) {
        words.truncate(words.len() - 2);
    }
    if words.len() < 2 {
        return None;
    }
    let start = words
        .iter()
        .position(|w| !w.starts_with(|c: char| c.is_ascii_digit()))
        .unwrap_or(words.len());
    if start >= words.len() - 1 {
        return None;
    }
    Some(words[start..words.len() - 1].join(" "))
}

/// Every neighbor node reachable via a single road_edge from `node_id`,
/// paired with that edge's name (None if the edge is unnamed). In-memory
/// lookup (issue #51) against the shared RoadGraph (issue #78) — was a
/// fresh SQL query per call. Distance is dropped here — this file only
/// ever needs the name for the hop-by-hop walk below.
fn edges_at_node(graph: &RoadGraph, node_id: i64) -> Vec<(i64, Option<String>)> {
    graph
        .adjacency
        .get(&node_id)
        .map(|neighbors| neighbors.iter().map(|(n, _dist, name)| (*n, name.clone())).collect())
        .unwrap_or_default()
}

/// Rules 3+4 fallback, generalized (#48 follow-up): many "no name on
/// file" flags turned out to be driveways — a short chain of *several*
/// unnamed road_edges before reaching a real named road, not just one hop
/// away. A single-hop check kept missing those. This does a breadth-first
/// walk outward from `start_nodes`, one shell at a time, stopping at the
/// first shell that has any named edge and returning every distinct name
/// found in that shell (not deeper — a driveway two hops from Elm St and
/// three hops from Oak Rd should match Elm, not both). Bounded by
/// MAX_UNNAMED_HOPS so a long unnamed rural stretch can't walk the whole
/// graph. Cycle-safe via `visited`.
const MAX_UNNAMED_HOPS: usize = 100;

fn walk_to_named_roads(
    graph: &RoadGraph,
    start_nodes: &[i64],
    trace: &mut Option<&mut Vec<String>>,
) -> Vec<String> {
    let mut visited: HashSet<i64> = start_nodes.iter().copied().collect();
    let mut frontier: Vec<i64> = start_nodes.to_vec();

    trace_push(trace, format!("walk start node(s): {start_nodes:?}"));

    for hop in 1..=MAX_UNNAMED_HOPS {
        let mut names_found: HashSet<String> = HashSet::new();
        let mut next_frontier: Vec<i64> = Vec::new();

        for &node in &frontier {
            for (neighbor, name) in edges_at_node(graph, node) {
                match name {
                    Some(n) => {
                        trace_push(
                            trace,
                            format!("  hop {hop}: node {node} -> neighbor {neighbor} named \"{n}\""),
                        );
                        names_found.insert(n);
                    }
                    None => {
                        trace_push(
                            trace,
                            format!("  hop {hop}: node {node} -> neighbor {neighbor} unnamed"),
                        );
                        if visited.insert(neighbor) {
                            next_frontier.push(neighbor);
                        }
                    }
                }
            }
        }

        if !names_found.is_empty() {
            trace_push(trace, format!("hop {hop}: named road(s) found: {names_found:?}"));
            return names_found.into_iter().collect();
        }
        if next_frontier.is_empty() {
            trace_push(trace, format!("hop {hop}: no unnamed neighbors left to expand — stopping"));
            break;
        }
        frontier = next_frontier;
    }

    trace_push(trace, format!("no named road found within {MAX_UNNAMED_HOPS} hop(s)"));
    Vec::new()
}

/// Rules 3+4 entry point from a matched segment's two endpoints — walks
/// outward from both until a named road is found (see walk_to_named_roads).
fn connected_road_names_both(
    graph: &RoadGraph,
    node_a: i64,
    node_b: i64,
    trace: &mut Option<&mut Vec<String>>,
) -> Vec<String> {
    walk_to_named_roads(graph, &[node_a, node_b], trace)
}

/// Perpendicular distance (meters, local flat-plane approximation — fine
/// at SNAP_TOLERANCE_M scale) from point (px, py) to the segment (ax, ay)
/// – (bx, by), clamped to the segment (not the infinite line).
fn point_to_segment_dist(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let abx = bx - ax;
    let aby = by - ay;
    let ab_len_sq = abx * abx + aby * aby;
    let t = if ab_len_sq > 0.0 {
        (((px - ax) * abx + (py - ay) * aby) / ab_len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let cx = ax + t * abx;
    let cy = ay + t * aby;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Every road_edge whose own bounding box overlaps the search box around
/// (lat, lon), scored by point-to-segment distance and sorted ascending —
/// not just the single nearest one (#48 follow-up, round 2: anchoring on
/// nearby *nodes* missed a road whose own nodes were both far from where
/// the household sits along its length; see road_name_problem's doc for
/// round 3, why "nearest only" isn't enough either). Household lat/lon is
/// the local flat-plane projection origin — fine at SNAP_TOLERANCE_M
/// scale. Empty if nothing overlaps the box at all.
///
/// In-memory scan of the precomputed RoadGraph (issue #51) — was a fresh
/// per-household SQL bbox query; SQLite couldn't index the per-row
/// max(fn.lat, tn.lat) comparison it used, forcing a full table scan +
/// two joins every time. Same bbox-overlap filter, same scoring, just
/// against graph.edges instead of a rusqlite statement.
fn nearby_scored_edges(
    graph: &RoadGraph,
    lat: f64,
    lon: f64,
    trace: &mut Option<&mut Vec<String>>,
) -> Vec<(f64, i64, i64, Option<String>)> {
    let deg_margin = (super::roads::SNAP_TOLERANCE_M / 111_000.0) * 3.0;
    let min_lat = lat - deg_margin;
    let max_lat = lat + deg_margin;
    let min_lon = lon - deg_margin;
    let max_lon = lon + deg_margin;

    trace_push(
        trace,
        format!("search box: lat [{min_lat:.6}, {max_lat:.6}] lon [{min_lon:.6}, {max_lon:.6}]"),
    );

    let meters_per_deg_lat = 111_320.0_f64;
    let meters_per_deg_lon = 111_320.0_f64 * lat.to_radians().cos();

    let mut scored: Vec<(f64, i64, i64, Option<String>)> = Vec::new();
    for e in &graph.edges {
        // Issue #80: RoadEdge (shared road_graph.rs) carries its own
        // endpoint coords again, resolved once per edge at graph-build
        // time — not looked up from graph.coords here per household.
        // #78 had dropped these fields and made this loop do 2 HashMap
        // lookups per edge per household, silently reintroducing the
        // per-item overhead #51 existed to eliminate (confirmed: #51's
        // 50x speedup measured zero after #78 landed).
        //
        // An edge's bbox overlaps the search box unless it's entirely
        // above/below/left/right of it — same test the old SQL WHERE
        // clause ran, just as plain scalar comparisons here.
        let edge_max_lat = e.from_lat.max(e.to_lat);
        let edge_min_lat = e.from_lat.min(e.to_lat);
        let edge_max_lon = e.from_lon.max(e.to_lon);
        let edge_min_lon = e.from_lon.min(e.to_lon);
        if edge_max_lat < min_lat || edge_min_lat > max_lat || edge_max_lon < min_lon || edge_min_lon > max_lon {
            continue;
        }

        let ax = (e.from_lon - lon) * meters_per_deg_lon;
        let ay = (e.from_lat - lat) * meters_per_deg_lat;
        let bx = (e.to_lon - lon) * meters_per_deg_lon;
        let by = (e.to_lat - lat) * meters_per_deg_lat;
        let d = point_to_segment_dist(0.0, 0.0, ax, ay, bx, by);
        scored.push((d, e.from_id, e.to_id, e.name.clone()));
    }
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    trace_push(trace, format!("{} road_edge(s) overlapping search box", scored.len()));
    for (d, a, b, name) in scored.iter().take(10) {
        trace_push(
            trace,
            format!(
                "  edge {a}-{b} name={} dist={d:.1}m",
                name.clone().unwrap_or_else(|| "NONE".to_string())
            ),
        );
    }

    scored
}

/// The road-name check itself (checks 3/4 combined) — returns Some(reason)
/// if this household should be flagged, None if the address's road name
/// checks out against something nearby.
///
/// #48 follow-up, round 3: matching only against the single *nearest*
/// edge's name (falling back to a graph walk from its endpoints if
/// unnamed) missed real matches that were sitting in the candidate list
/// the whole time. Confirmed on household #4: the household's actual
/// street, East Street, was 20.6m away and present among the scored
/// edges — but an unnamed trail 6.2m closer was picked as "the" segment,
/// and the trail's own graph component never reached East Street within
/// MAX_UNNAMED_HOPS, so the real match was thrown away before it was ever
/// compared against H.
///
/// Fixed by checking every named edge within SNAP_TOLERANCE_M against H
/// directly — not just the nearest one — before falling back to walking
/// outward from the nearest edge's endpoints (still needed for the
/// genuine-driveway case: nothing named within tolerance at all).
fn road_name_problem(
    graph: &RoadGraph,
    lat: f64,
    lon: f64,
    address_road_name: &str,
    trace: &mut Option<&mut Vec<String>>,
) -> Option<String> {
    let scored = nearby_scored_edges(graph, lat, lon, trace);
    if scored.is_empty() {
        return Some("No road found near household within snap tolerance".to_string());
    }

    let within_tol: Vec<&(f64, i64, i64, Option<String>)> =
        scored.iter().filter(|(d, ..)| *d <= super::roads::SNAP_TOLERANCE_M).collect();

    if within_tol.is_empty() {
        trace_push(
            trace,
            format!(
                "nearest edge is {:.1}m — outside tolerance ({:.1}m)",
                scored[0].0,
                super::roads::SNAP_TOLERANCE_M
            ),
        );
        return Some("No road found near household within snap tolerance".to_string());
    }

    let named_within: Vec<String> = within_tol.iter().filter_map(|(_, _, _, n)| n.clone()).collect();
    trace_push(
        trace,
        format!("{} named edge(s) within tolerance: {named_within:?}", named_within.len()),
    );

    if !named_within.is_empty() && names_match(&named_within, address_road_name) {
        trace_push(trace, "matched directly against a named edge within tolerance".to_string());
        return None;
    }

    // No direct match among nearby named edges (or none within tolerance
    // are named at all) — widen the search by walking outward from the
    // single nearest edge's endpoints (handles the driveway/trail case).
    let (_, node_a, node_b, _) = within_tol[0];
    let walked = connected_road_names_both(graph, *node_a, *node_b, trace);

    let mut candidates = named_within;
    for n in walked {
        if !candidates.contains(&n) {
            candidates.push(n);
        }
    }
    // Multiple edges — separate segments of the same physical road — can
    // carry the identical name, which otherwise showed up as repeated
    // entries in both the on-screen modal and the PDF's "Snapped road
    // name(s)" list. Order-preserving dedup, not a sort — first-seen wins.
    let mut seen = HashSet::new();
    candidates.retain(|n| seen.insert(n.clone()));

    if names_match(&candidates, address_road_name) {
        trace_push(trace, "matched after walking outward from nearest edge".to_string());
        return None;
    }

    if candidates.is_empty() {
        Some("Nearest road has no name on file — cannot verify against address".to_string())
    } else {
        Some(format!(
            "Snapped road name(s) [{}] differ from address road name \"{address_road_name}\"",
            candidates.join(", ")
        ))
    }
}


#[tauri::command]
pub async fn find_potential_problems(app: AppHandle, state: State<'_, AppState>) -> Result<DiagnosticsReport, String> {
    // #48 follow-up: this used to be a plain sync fn. At ~390 households
    // the scan takes a minute or two, and a non-async command with no
    // spawn_blocking runs on the same thread as the webview's own event
    // loop — nothing could paint (no modal, no progress ring, no repaint
    // of anything else in the app) until it returned. spawn_blocking
    // moves the scan to a background thread; the pools are cloned out
    // first, same reason/pattern import_pdf already uses elsewhere in
    // this codebase — State<'_, AppState> isn't 'static, but r2d2::Pool
    // is just an Arc-backed handle, cheap to clone — so the closure below
    // can be 'static + Send. Internal indentation is left as-is rather
    // than fully reflowed, to keep this diff to the actual change.
    let pool = state.pool.clone();
    let roads_pool = state.roads_pool.clone();
    // Issue #66 follow-up: cheap outer-Arc clone, same reason
    // pool/roads_pool are cloned here rather than moving `state` itself —
    // State<'_, AppState> isn't 'static, this is.
    let road_graph_cache = state.road_graph_cache.clone();

    tauri::async_runtime::spawn_blocking(move || -> Result<DiagnosticsReport, String> {
    let conn = pool.get().map_err(|e| e.to_string())?;
    let roads_conn = roads_pool.get().map_err(|e| e.to_string())?;

    // Precomputed once, not per household: every road_names.name goes into
    // two sets — normalized-and-full, and normalized-and-last-word-dropped
    // — so check 1 below is a couple of set lookups instead of N separate
    // SQL round trips or an O(households x road_names) scan. Two sets
    // because Rule 1 (module doc) needs to try road-db names both with and
    // without their last word dropped.
    let (road_full_set, road_dropped_set): (HashSet<String>, HashSet<String>) = {
        let mut stmt = roads_conn.prepare("SELECT name FROM road_names").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        let mut full = HashSet::new();
        let mut dropped = HashSet::new();
        for raw in rows.flatten() {
            let norm = normalize_name(&raw);
            full.insert(norm.to_lowercase());
            dropped.insert(drop_last(&norm).to_lowercase());
        }
        (full, dropped)
    };
    let roads_available = !road_full_set.is_empty();

    // Issue #51 / #78: one full read of road_edges/road_nodes/road_names
    // via the shared road_graph module, replacing every
    // per-household/per-BFS-hop query nearby_scored_edges() and
    // edges_at_node() used to issue against roads_conn directly.
    // Issue #66 follow-up: now served from the same cache
    // generate_visit_list() (visits.rs) populates — a data-validation
    // scan no longer pays its own full roads.db rebuild if a route was
    // already generated this session (or vice versa), and a re-ingest
    // (roads.rs::ingest_road_database) invalidates it for both callers
    // at once. None means nothing has been ingested (not cached, per
    // load_or_build's doc comment) — falls back to an empty RoadGraph
    // here rather than propagating an Option, since every downstream
    // function in this file (edges_at_node, road_name_problem, etc.)
    // already expects a plain &RoadGraph and already handles "empty" via
    // roads_available below, same as before this change.
    let cached_road_graph = road_graph::load_or_build(&road_graph_cache, &roads_conn);
    let empty_road_graph_fallback;
    let road_graph: &RoadGraph = match &cached_road_graph {
        Some(g) => g.as_ref(),
        None => {
            empty_road_graph_fallback = RoadGraph::default();
            &empty_road_graph_fallback
        }
    };

    struct Row {
        id: i64,
        household_name: String,
        address_line1: Option<String>,
        address_line2: Option<String>,
        address_key: String,
        lat: Option<f64>,
        lon: Option<f64>,
        tag_name: Option<String>,
    }
    let mut stmt = conn
        .prepare(
            "SELECT h.id, h.first_name, h.last_name, h.first_name_2, h.last_name_2, h.address_line1, \
             h.address_line2, h.latitude, h.longitude, h.address_key, \
             (SELECT t.name FROM household_tags ht JOIN tags t ON t.id = ht.tag_id \
              WHERE ht.household_id = h.id LIMIT 1) AS tag_name \
             FROM households h",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<Row> = stmt
        .query_map([], |r| {
            let first_name_2: Option<String> = r.get(3)?;
            let household_name = match first_name_2 {
                // Matches the naming convention used elsewhere (e.g.
                // visits::fetch_grouped_households) for a two-head entry.
                Some(f2) => format!(
                    "{} {} & {} {}",
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    f2,
                    r.get::<_, Option<String>>(4)?.unwrap_or_default()
                ),
                None => format!("{} {}", r.get::<_, String>(1)?, r.get::<_, String>(2)?),
            };
            Ok(Row {
                id: r.get(0)?,
                household_name,
                address_line1: r.get(5)?,
                address_line2: r.get(6)?,
                lat: r.get(7)?,
                lon: r.get(8)?,
                address_key: r.get(9)?,
                tag_name: r.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;

    // #XX follow-up: households sharing an address (same address_key)
    // should also share geocoordinates — nothing enforces that today, and
    // a Replace/Merge that only touches one resident's row can leave the
    // others silently stale (confirmed live: two residents at one
    // address, one freshly re-geocoded, one not — the map's per-address
    // marker picked whichever row happened to come back first). Flagged
    // here rather than fixed by restructuring how coordinates are stored,
    // per Stan's call: this exists to surface the problem at its source
    // (the import/replace step that didn't sync everyone), not to paper
    // over it with a shared-coordinate schema.
    //
    // Reported as ONE group entry per mismatched address (Stan's call —
    // three residents at one address previously produced three separate,
    // identical-looking rows with no indication they were even related;
    // a single group entry listing everyone at the address makes the
    // actual source problem — one import/replace step that missed some
    // residents — visible at a glance). This does NOT replace a
    // household's own other reasons (street name not found, no address on
    // file, etc.) — those still produce that household's normal
    // individual entry alongside the group entry; a household can
    // legitimately appear in both.
    let mut addr_groups: HashMap<&str, Vec<&Row>> = HashMap::new();
    for row in &rows {
        if !row.address_key.trim().is_empty() {
            addr_groups.entry(row.address_key.as_str()).or_default().push(row);
        }
    }
    // A missing coordinate counts as distinct from any real coordinate
    // pair, not as "no opinion" — one resident geocoded and another not,
    // at the same address, is exactly the kind of split this exists to
    // catch, same as an outright numeric disagreement. Two present
    // coordinates within ADDRESS_COORD_TOLERANCE_METERS of each other are
    // NOT a mismatch — ordinary re-geocode jitter, not a real problem.
    fn coords_close_enough(
        a: (Option<f64>, Option<f64>),
        b: (Option<f64>, Option<f64>),
        tolerance_m: f64,
    ) -> bool {
        match (a, b) {
            (a, b) if a == b => true, // covers both-None and exact match
            ((Some(alat), Some(alon)), (Some(blat), Some(blon))) => {
                crate::geo::haversine_meters(alat, alon, blat, blon) <= tolerance_m
            }
            _ => false, // one side missing, the other present
        }
    }

    let mut group_problems: Vec<PotentialProblem> = Vec::new();
    for members in addr_groups.values() {
        if members.len() < 2 {
            continue;
        }
        let first_coords = (members[0].lat, members[0].lon);
        let mismatched = members
            .iter()
            .any(|m| !coords_close_enough((m.lat, m.lon), first_coords, ADDRESS_COORD_TOLERANCE_METERS));
        if !mismatched {
            continue;
        }
        group_problems.push(PotentialProblem {
            household_id: members[0].id,
            household_name: members[0].household_name.clone(),
            household_ids: Some(members.iter().map(|m| m.id).collect()),
            household_names: Some(members.iter().map(|m| m.household_name.clone()).collect()),
            household_tags: Some(members.iter().map(|m| m.tag_name.clone()).collect()),
            address_line1: members[0].address_line1.clone(),
            address_line2: members[0].address_line2.clone(),
            tag: None, // members may hold different tags — not meaningful to pick one
            reasons: vec![format!(
                "Shared address — {} households with differing or missing geocoordinates",
                members.len()
            )],
            debug_trace: None,
        });
    }

    let mut problems = Vec::new();
    let mut debug_captures: usize = 0;
    let total = rows.len();

    for (i, row) in rows.into_iter().enumerate() {
        emit_diagnostics_progress(&app, i + 1, total);

        let address = match row.address_line1.as_deref().map(str::trim) {
            Some(a) if !a.is_empty() => a,
            _ => {
                problems.push(PotentialProblem {
                    household_id: row.id,
                    household_name: row.household_name.clone(),
                    household_ids: None,
                    household_names: None,
                    household_tags: None,
                    address_line1: row.address_line1.clone(),
                    address_line2: row.address_line2.clone(),
                    tag: row.tag_name.clone(),
                    reasons: vec!["No address on file".to_string()],
                    debug_trace: None,
                });
                continue;
            }
        };

        let mut reasons: Vec<String> = Vec::new();
        let mut debug_trace: Option<Vec<String>> = None;
        let road_name = road_name_from_address(address);

        if roads_available {
            if let Some(name) = &road_name {
                let hn = normalize_name(name);
                let h_full = hn.to_lowercase();
                let h_dropped = drop_last(&hn).to_lowercase();
                // Rule 1: try all three alignments — drop both, drop
                // household side only, drop road-db side only.
                let found = road_dropped_set.contains(&h_dropped)
                    || road_full_set.contains(&h_dropped)
                    || road_dropped_set.contains(&h_full);
                if !found {
                    reasons.push(format!("Street name \"{name}\" not found in roads database"));
                }
            }
        }

        match (row.lat, row.lon) {
            (Some(lat), Some(lon)) => {
                if roads_available {
                    if let Some(name) = &road_name {
                        // Debug trace budget is spent only on households
                        // that actually end up flagged by this check —
                        // see DEBUG_TRACE_LIMIT doc comment (#48 "no name
                        // on file" investigation).
                        let want_trace = debug_captures < DEBUG_TRACE_LIMIT;
                        let mut local_trace: Vec<String> = Vec::new();
                        if want_trace {
                            local_trace.push(format!("Household #{}: \"{}\"", row.id, row.household_name));
                            local_trace.push(format!("Address: \"{address}\""));
                            local_trace.push(format!("Address road name (H): \"{name}\""));
                            local_trace.push(format!("Coordinates: ({lat:.6}, {lon:.6})"));
                        }
                        let mut trace_opt: Option<&mut Vec<String>> =
                            if want_trace { Some(&mut local_trace) } else { None };

                        if let Some(reason) = road_name_problem(road_graph, lat, lon, name, &mut trace_opt) {
                            if want_trace {
                                local_trace.push(format!("RESULT: flagged — {reason}"));
                                debug_trace = Some(local_trace);
                                debug_captures += 1;
                            }
                            reasons.push(reason);
                        }
                    }
                }
            }
            _ => {
                reasons.push("No geocoordinates on file".to_string());
            }
        }

        if !reasons.is_empty() {
            problems.push(PotentialProblem {
                household_id: row.id,
                household_name: row.household_name.clone(),
                household_ids: None,
                household_names: None,
                household_tags: None,
                address_line1: row.address_line1.clone(),
                address_line2: row.address_line2.clone(),
                tag: row.tag_name.clone(),
                reasons,
                debug_trace,
            });
        }
    }

    problems.extend(group_problems);

    Ok(DiagnosticsReport { problems, roads_checked: roads_available })
    })
    .await
    .map_err(|e| e.to_string())?
}
