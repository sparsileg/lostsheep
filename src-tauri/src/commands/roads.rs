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

    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;

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

    super::logs::log(
        &conn,
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
