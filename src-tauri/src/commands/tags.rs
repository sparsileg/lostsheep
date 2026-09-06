use crate::AppState;
use rusqlite::params;
use serde::Serialize;
use tauri::State;

fn norm(s: &str) -> String {
    s.trim().to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Serialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub household_count: i64,
    /// Set for tags the app itself depends on (currently just "Do not
    /// contact") — lets callers like the Dashboard map's tag filter
    /// exclude them from user-facing pickers without hardcoding names.
    pub system_key: Option<String>,
}

#[tauri::command]
pub fn list_tags(state: State<AppState>) -> Result<Vec<Tag>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT t.id, t.name, (SELECT count(*) FROM household_tags ht WHERE ht.tag_id = t.id) AS cnt, t.system_key \
             FROM tags t ORDER BY t.name",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Tag { id: row.get(0)?, name: row.get(1)?, household_count: row.get(2)?, system_key: row.get(3)? })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}
pub(crate) fn get_or_create_tag_id(conn: &rusqlite::Connection, name: &str) -> rusqlite::Result<i64> {
    let name = name.trim();
    let name_norm = norm(name);
    conn.execute(
        "INSERT INTO tags (name, name_norm) VALUES (?1, ?2) ON CONFLICT(name_norm) DO NOTHING",
        params![name, name_norm],
    )?;
    conn.query_row("SELECT id FROM tags WHERE name_norm = ?1", params![name_norm], |r| r.get(0))
}

#[tauri::command]
pub fn create_tag(state: State<AppState>, name: String) -> Result<i64, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    get_or_create_tag_id(&conn, &name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rename_tag(state: State<AppState>, id: i64, new_name: String) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE tags SET name = ?1, name_norm = ?2 WHERE id = ?3",
        params![new_name.trim(), norm(&new_name), id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// substitute_tag_id: households carrying the deleted tag are re-pointed
/// to this tag instead of being silently un-categorised. None is only
/// accepted when the tag isn't applied to anyone. Returns the number of
/// households affected — tags-modal.js's TagDeleteModal.confirm() already
/// expects this shape (`const affected = await DBManager.deleteTag(...)`).
#[tauri::command]
pub fn delete_tag(state: State<AppState>, id: i64, substitute_tag_id: Option<i64>) -> Result<i64, String> {
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;

    // System tags (system_key set — e.g. "Do not contact") guard a
    // safety exclusion with no meaningful substitute. Refuse outright
    // rather than let generate_visit_list's exclusion silently stop
    // matching anything (#23).
    let system_key: Option<String> = tx
        .query_row("SELECT system_key FROM tags WHERE id = ?1", params![id], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if system_key.is_some() {
        return Err("this tag is required by the app and cannot be deleted".to_string());
    }

    let affected: i64 = tx
        .query_row("SELECT count(*) FROM household_tags WHERE tag_id = ?1", params![id], |r| r.get(0))
        .map_err(|e| e.to_string())?;

    if affected > 0 {
        match substitute_tag_id {
            Some(sub_id) => {
                if sub_id == id {
                    return Err("substitute tag must be different from the tag being deleted".to_string());
                }
                // OR IGNORE: a household that already carries the
                // substitute tag would otherwise hit household_tags'
                // (household_id, tag_id) primary key.
                tx.execute(
                    "INSERT OR IGNORE INTO household_tags (household_id, tag_id) \
                     SELECT household_id, ?1 FROM household_tags WHERE tag_id = ?2",
                    params![sub_id, id],
                )
                .map_err(|e| e.to_string())?;
                tx.execute("DELETE FROM household_tags WHERE tag_id = ?1", params![id]).map_err(|e| e.to_string())?;
            }
            None => {
                return Err(format!(
                    "tag is applied to {affected} household(s) — choose a substitute tag or remove it from those households first"
                ));
            }
        }
    }

    tx.execute("DELETE FROM tags WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(affected)
}

/// Tags are capped at one per household — functionally more like a
/// category than a tag right now, by design. Applying a tag always
/// clears whatever was there before, on every household in the list.
#[tauri::command]
pub fn tag_households(state: State<AppState>, household_ids: Vec<i64>, tag_name: String) -> Result<(), String> {
    let count = household_ids.len();
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let tag_id = get_or_create_tag_id(&tx, &tag_name).map_err(|e| e.to_string())?;
    for hid in household_ids {
        tx.execute("DELETE FROM household_tags WHERE household_id = ?1", params![hid]).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR IGNORE INTO household_tags (household_id, tag_id) VALUES (?1, ?2)",
            params![hid, tag_id],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    super::logs::log(&conn, "info", &format!("tagged {count} household(s) with \"{tag_name}\""), None);
    Ok(())
}

#[tauri::command]
pub fn untag_household(state: State<AppState>, household_id: i64, tag_id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM household_tags WHERE household_id = ?1 AND tag_id = ?2",
        params![household_id, tag_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Tags every household matching the given search, not just a page of
/// it. Goes through matching_household_ids/build_where
/// (commands::households) rather than search_households itself — that
/// path applies the 500-row display cap, which this used to try to opt
/// out of with page_size: 100000 and get silently clamped back down
/// (issue #22). The returned count is exactly how many ids were tagged,
/// not an assumed page size.
#[tauri::command]
pub fn bulk_tag_search_results(
    state: State<AppState>,
    search: super::households::SearchParams,
    tag_name: String,
) -> Result<i64, String> {
    let ids = {
        let conn = state.pool.get().map_err(|e| e.to_string())?;
        super::households::matching_household_ids(&conn, &search)?
    };
    let count = ids.len() as i64;
    tag_households(state, ids, tag_name)?;
    Ok(count)
}
