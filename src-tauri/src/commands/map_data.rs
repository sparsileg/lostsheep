use crate::AppState;
use tauri::State;

#[tauri::command]
pub fn get_map_data(state: State<AppState>, tag_id: Option<i64>) -> Result<Vec<super::visits::VisitListEntry>, String> {
    // #31: previously borrowed generate_visit_list wholesale via an
    // unreachable seed (id 0) and count:100000, which meant every
    // Dashboard load and tag-dropdown change ran the full nearest-
    // neighbor route walk (and its seed-distance sort) over every
    // household in the database whenever a route start point was
    // configured — quadratic in household count for data the map
    // never uses (map_data ignores distance_meters entirely; it just
    // plots lat/long per group). fetch_grouped_households is the
    // same grouping with none of that distance/order machinery.
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    super::visits::fetch_grouped_households(&conn, tag_id)
}

// #64 — fetch_grouped_households (and therefore the map/visit lists) drop
// any household with a null latitude or longitude, silently. This gives
// the Dashboard a total it can show alongside the tag cards so the gap is
// visible without running Data Validation. Deliberately NOT tag-scoped
// (unlike fetch_grouped_households) — the Dashboard's stat-card row isn't
// wired to the map's tag filter, so this is a whole-directory count.
// "Do not contact" households are excluded, same as the map/visit-list
// queries — they're never plotted or routed either way, so counting their
// missing coordinates wouldn't explain anything the map shows.
#[tauri::command]
pub fn get_missing_coords_count(state: State<AppState>) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT COUNT(*) FROM households h \
         WHERE (h.latitude IS NULL OR h.longitude IS NULL) \
         AND h.id NOT IN (SELECT ht2.household_id FROM household_tags ht2 JOIN tags t2 ON t2.id = ht2.tag_id WHERE t2.system_key = 'do_not_contact')",
        [],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}
