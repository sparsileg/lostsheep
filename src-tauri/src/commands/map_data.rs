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
