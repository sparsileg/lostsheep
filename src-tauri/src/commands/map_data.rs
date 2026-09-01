use crate::AppState;
use tauri::State;

#[tauri::command]
pub fn get_map_data(state: State<AppState>, tag_id: Option<i64>) -> Result<Vec<super::visits::VisitListEntry>, String> {
    // Reuses generate_visit_list's grouping/shape with an unreachable seed
    // so distance is meaningless here — frontend ignores distance_meters
    // in plain map-display mode and just plots lat/long per group.
    super::visits::generate_visit_list(
        state,
        super::visits::GenerateVisitListParams { seed_household_id: 0, tag_id, count: 100000 },
    )
}
