use crate::AppState;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tauri::State;

#[tauri::command]
pub fn record_visit(
    state: State<AppState>,
    household_id: i64,
    visit_date: String,
    comments: Option<String>,
) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO visits (household_id, visit_date, comments) VALUES (?1, ?2, ?3)",
        params![household_id, visit_date, comments],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
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

#[derive(Serialize)]
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
}

/// Nearest-N generation, grouped by address so multi-head households are
/// never split across the boundary. "Count" caps distinct addresses, not
/// raw rows — matches the "all records sharing an address are included
/// together" rule.
#[tauri::command]
pub fn generate_visit_list(state: State<AppState>, params: GenerateVisitListParams) -> Result<Vec<VisitListEntry>, String> {
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

    let sql = match params.tag_id {
        Some(_) => {
            "SELECT h.id, h.address_key, h.address_line1, h.city, h.state, h.zip, h.latitude, h.longitude, \
             h.first_name, h.last_name, h.first_name_2, h.last_name_2, h.phone_1, h.phone_2 \
             FROM households h JOIN household_tags ht ON ht.household_id = h.id \
             WHERE ht.tag_id = ?1 AND h.latitude IS NOT NULL AND h.longitude IS NOT NULL \
             AND h.id NOT IN (SELECT ht2.household_id FROM household_tags ht2 JOIN tags t2 ON t2.id = ht2.tag_id WHERE t2.name_norm = 'do not contact')"
        }
        None => {
            "SELECT h.id, h.address_key, h.address_line1, h.city, h.state, h.zip, h.latitude, h.longitude, \
             h.first_name, h.last_name, h.first_name_2, h.last_name_2, h.phone_1, h.phone_2 \
             FROM households h WHERE h.latitude IS NOT NULL AND h.longitude IS NOT NULL \
             AND h.id NOT IN (SELECT ht2.household_id FROM household_tags ht2 JOIN tags t2 ON t2.id = ht2.tag_id WHERE t2.name_norm = 'do not contact')"
        }
    };
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;

    struct Row {
        id: i64, address_key: String, address_line1: Option<String>,
        city: Option<String>, state: Option<String>, zip: Option<String>,
        lat: f64, lon: f64, name: String, phones: Vec<String>,
    }
    let rows: Vec<Row> = if let Some(tag_id) = params.tag_id {
        stmt.query_map(rusqlite::params![tag_id], map_row).map_err(|e| e.to_string())?
    } else {
        stmt.query_map([], map_row).map_err(|e| e.to_string())?
    }
    .filter_map(Result::ok)
    .collect();

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

    // Group rows by address, take the min distance per group, sort groups
    // by that distance, then take the first `count` groups.
    use std::collections::HashMap;
    let mut groups: HashMap<String, Vec<&Row>> = HashMap::new();
    for r in &rows {
        groups.entry(r.address_key.clone()).or_default().push(r);
    }

    let mut entries: Vec<VisitListEntry> = groups
        .into_iter()
        .map(|(key, members)| {
            let first = members[0];
            let dist = crate::geo::haversine_meters(seed.0, seed.1, first.lat, first.lon);
            VisitListEntry {
                address_key: key,
                address_line1: first.address_line1.clone(),
                city: first.city.clone(),
                state: first.state.clone(),
                zip: first.zip.clone(),
                latitude: first.lat,
                longitude: first.lon,
                distance_meters: dist,
                household_ids: members.iter().map(|m| m.id).collect(),
                names: members.iter().map(|m| m.name.clone()).collect(),
                phones: members.iter().flat_map(|m| m.phones.clone()).collect(),
                distance_context: "seed".to_string(),
            }
        })
        .collect();

    // Selection stays seed-distance-based regardless of route mode (#13):
    // this sort+truncate picks which N addresses are included, unchanged.
    entries.sort_by(|a, b| a.distance_meters.partial_cmp(&b.distance_meters).unwrap());
    entries.truncate(params.count as usize);

    // Ordering, though, uses the configured route start point when one
    // exists — nearest-neighbor walk over the already-selected N
    // addresses, recomputing distance_meters as per-leg distance rather
    // than distance-from-seed. Falls back to today's seed-sorted order
    // (already produced above) when the setting is unconfigured.
    let route_start: Option<(f64, f64)> = {
        let get = |key: &str| -> Option<String> {
            conn.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                rusqlite::params![key],
                |r| r.get::<_, String>(0),
            )
            .ok()
        };
        match (
            get("routeStartLat").and_then(|s| s.parse::<f64>().ok()),
            get("routeStartLon").and_then(|s| s.parse::<f64>().ok()),
        ) {
            (Some(lat), Some(lon)) => Some((lat, lon)),
            _ => None,
        }
    };

    if let Some((start_lat, start_lon)) = route_start {
        let mut remaining = entries;
        let mut ordered: Vec<VisitListEntry> = Vec::with_capacity(remaining.len());
        let mut cur = (start_lat, start_lon);
        while !remaining.is_empty() {
            let mut best_idx = 0;
            let mut best_dist = f64::MAX;
            for (i, e) in remaining.iter().enumerate() {
                let d = crate::geo::haversine_meters(cur.0, cur.1, e.latitude, e.longitude);
                if d < best_dist {
                    best_dist = d;
                    best_idx = i;
                }
            }
            let mut next = remaining.remove(best_idx);
            next.distance_meters = best_dist;
            next.distance_context = "route".to_string();
            cur = (next.latitude, next.longitude);
            ordered.push(next);
        }
        entries = ordered;
    }

    let _dedupe_guard: HashSet<String> = HashSet::new(); // reserved: cross-group id collision guard if needed later
    Ok(entries)
}
