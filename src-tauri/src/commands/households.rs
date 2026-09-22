use crate::AppState;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

#[derive(Serialize)]
pub struct Household {
    pub id: i64,
    pub first_name: String,
    pub last_name: String,
    pub role: String,
    pub phone_1: Option<String>,
    pub email_1: Option<String>,
    pub first_name_2: Option<String>,
    pub last_name_2: Option<String>,
    pub role_2: Option<String>,
    pub phone_2: Option<String>,
    pub email_2: Option<String>,
    pub address_line1: Option<String>,
    pub address_line2: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub zip: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub has_minors: bool,
    pub comments: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
pub struct SearchParams {
    pub query: Option<String>,
    pub tag_names: Vec<String>, // implicit AND across these
    pub page: u32,
    pub page_size: u32,
}

#[derive(Serialize)]
pub struct SearchResult {
    pub households: Vec<Household>,
    pub total: i64,
}

fn row_to_household(conn: &rusqlite::Connection, row: &rusqlite::Row) -> rusqlite::Result<Household> {
    let id: i64 = row.get("id")?;
    let mut stmt = conn
        .prepare_cached("SELECT t.name FROM tags t JOIN household_tags ht ON ht.tag_id = t.id WHERE ht.household_id = ?1 ORDER BY t.name")?;
    let tags = stmt
        .query_map(params![id], |r| r.get::<_, String>(0))?
        .filter_map(Result::ok)
        .collect();
    Ok(Household {
        id,
        first_name: row.get("first_name")?,
        last_name: row.get("last_name")?,
        role: row.get("role")?,
        phone_1: row.get("phone_1")?,
        email_1: row.get("email_1")?,
        first_name_2: row.get("first_name_2")?,
        last_name_2: row.get("last_name_2")?,
        role_2: row.get("role_2")?,
        phone_2: row.get("phone_2")?,
        email_2: row.get("email_2")?,
        address_line1: row.get("address_line1")?,
        address_line2: row.get("address_line2")?,
        city: row.get("city")?,
        state: row.get("state")?,
        zip: row.get("zip")?,
        latitude: row.get("latitude")?,
        longitude: row.get("longitude")?,
        has_minors: row.get::<_, i64>("has_minors")? != 0,
        comments: row.get("comments")?,
        tags,
    })
}

/// Builds the shared WHERE clause + bound params for both the paginated
/// on-screen search and the ids-only bulk-tag path below — the two must
/// never drift into separately-maintained copies of the same filter
/// logic (#22).
// Issue #29: LIKE metacharacters (% and _) are not otherwise special to
// SQLite outside a LIKE pattern, so escaping them here and pairing with
// ESCAPE '\' in the query makes a literal % or _ in the search box match
// literally instead of acting as a wildcard.
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn build_where(params: &SearchParams) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let mut where_clauses = vec!["1=1".to_string()];
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(q) = params.query.as_ref().filter(|q| !q.trim().is_empty()) {
        // Issue #29: "Search is always an implicit AND between
        // keywords" (Functional_Requirements.md) — previously the whole
        // input was one contiguous substring, so "Smith Winchester"
        // never matched unless those two words were adjacent in that
        // exact order. Now each whitespace-separated token gets its own
        // LIKE clause, ANDed together, so token order and position don't
        // matter. Tags are deliberately NOT included here — capped at
        // one per household and already covered by the dedicated tag
        // filter dropdown; Functional_Requirements.md has been amended
        // to record that as the intended mechanism.
        //
        // Per Stan: a token also matches if it's found in any of this
        // household's VISIT comments (visits.comments), not just its own
        // household-level comments (h.comments, already covered above) —
        // an EXISTS subquery per token, OR'd alongside the household-field
        // match rather than folded into the same concatenated string
        // (visits is a one-to-many child table, so it can't join into a
        // single household row without either duplicating rows per visit
        // or aggregating comments across visits; EXISTS avoids both).
        for token in q.split_whitespace() {
            where_clauses.push(
                "((h.first_name || ' ' || h.last_name || ' ' || coalesce(h.first_name_2,'') || ' ' || coalesce(h.last_name_2,'') || ' ' || \
                  coalesce(h.address_line1,'') || ' ' || coalesce(h.address_line2,'') || ' ' || \
                  coalesce(h.city,'') || ' ' || coalesce(h.state,'') || ' ' || coalesce(h.zip,'') || ' ' || \
                  coalesce(h.phone_1,'') || ' ' || coalesce(h.phone_2,'') || ' ' || \
                  coalesce(h.email_1,'') || ' ' || coalesce(h.email_2,'') || ' ' || \
                  coalesce(h.comments,'') \
                 ) LIKE ? ESCAPE '\\' \
                 OR EXISTS (SELECT 1 FROM visits v WHERE v.household_id = h.id AND v.comments LIKE ? ESCAPE '\\'))".to_string(),
            );
            let pattern = format!("%{}%", escape_like(token));
            binds.push(Box::new(pattern.clone()));
            binds.push(Box::new(pattern));
        }
    }
    for tag in &params.tag_names {
        where_clauses.push(
            "h.id IN (SELECT ht.household_id FROM household_tags ht JOIN tags t ON t.id=ht.tag_id WHERE t.name_norm = ?)".to_string(),
        );
        binds.push(Box::new(normalize_tag(tag)));
    }

    (where_clauses.join(" AND "), binds)
}

/// Every id matching the search/filter, with no page cap and no per-row
/// tag subquery — for callers that need the complete result set rather
/// than a page of full records (issue #22). bulk_tag_search_results used
/// to ask search_households for page_size: 100000, which the 500-row
/// clamp below silently truncated with no indication that anything was
/// dropped. This isn't just "the same query minus the cap" — skipping
/// row_to_household's tag subquery per row also makes it far cheaper at
/// 10k rows, where that would otherwise be 10k extra queries just to
/// read an id.
pub(crate) fn matching_household_ids(conn: &rusqlite::Connection, params: &SearchParams) -> Result<Vec<i64>, String> {
    let (where_sql, binds) = build_where(params);
    let sql = format!("SELECT h.id FROM households h WHERE {}", where_sql);
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let ids = stmt
        .query_map(rusqlite::params_from_iter(binds.iter()), |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    Ok(ids)
}

/// Free-text search across name/address/comments (deliberately NOT tags —
/// use the tag filter dropdown for that). AND-combined with tag filters.
/// bulk-tagging (tags::bulk_tag_search_results) matches the same
/// households via matching_household_ids/build_where above, not this
/// function directly — that path needs every match, not one capped page.
#[tauri::command]
pub fn search_households(state: State<AppState>, params: SearchParams) -> Result<SearchResult, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;

    let (where_sql, mut binds) = build_where(&params);
    let count_sql = format!("SELECT count(*) FROM households h WHERE {}", where_sql);
    let mut count_stmt = conn.prepare(&count_sql).map_err(|e| e.to_string())?;
    let total: i64 = count_stmt
        .query_row(rusqlite::params_from_iter(binds.iter()), |r| r.get(0))
        .map_err(|e| e.to_string())?;

    let page_size = params.page_size.clamp(1, 500) as i64;
    // Derive the real last page from `total` (just computed above) and
    // clamp the requested page to it — an absurd page number returns the
    // last real page instead of an empty result. All arithmetic here is
    // i64: page arrives as u32 with no upper bound from the IPC caller,
    // and (page - 1) * page_size in u32 could overflow for a page above
    // ~8.6 million, either panicking (debug) or wrapping to a silently
    // wrong offset (release) (#33).
    let max_page = if total > 0 { (total - 1) / page_size + 1 } else { 1 };
    let page = (params.page.max(1) as i64).min(max_page);
    let offset = (page - 1) * page_size;

    let list_sql = format!(
        "SELECT h.* FROM households h WHERE {} ORDER BY h.last_name, h.first_name LIMIT ?{} OFFSET ?{}",
        where_sql,
        binds.len() + 1,
        binds.len() + 2
    );
    let mut list_stmt = conn.prepare(&list_sql).map_err(|e| e.to_string())?;
    binds.push(Box::new(page_size));
    binds.push(Box::new(offset));
    let households = list_stmt
        .query_map(rusqlite::params_from_iter(binds.iter()), |row| row_to_household(&conn, row))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    Ok(SearchResult { households, total })
}

#[tauri::command]
pub fn get_household(state: State<AppState>, id: i64) -> Result<Household, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.query_row("SELECT * FROM households WHERE id = ?1", params![id], |row| {
        row_to_household(&conn, row)
    })
    .map_err(|e| e.to_string())
}

/// The household detail modal is otherwise read-only (name/address/phone
/// come from the imported directory, corrected via re-import + Review —
/// not edited by hand here). Comments are the one free-text field users
/// add to directly.
#[tauri::command]
pub fn update_household_comments(state: State<AppState>, id: i64, comments: Option<String>) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE households SET comments = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?2",
        params![comments, id],
    )
    .map_err(|e| e.to_string())?;
    crate::commands::logs::log(&conn, "info", &format!("household {id} comments updated"), None);
    Ok(())
}

/// Deletes are transactional (issue #21) and mirror both the household's
/// visit history and its tag(s) into deleted_visits/deleted_household_tags
/// before the DELETE cascades those away (issue #19) — restore_deleted_household
/// below reverses all three.
#[tauri::command]
pub fn soft_delete_household(state: State<AppState>, id: i64, reason: Option<String>) -> Result<(), String> {
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;

    let affected = tx
        .execute(
            "INSERT INTO deleted_households (original_id, household_uid, first_name, last_name, role, phone_1, email_1, first_name_2, last_name_2, role_2, phone_2, email_2, \
             address_line1, address_line2, city, state, zip, latitude, longitude, address_key, source_key, has_minors, comments, deletion_reason) \
             SELECT id, household_uid, first_name, last_name, role, phone_1, email_1, first_name_2, last_name_2, role_2, phone_2, email_2, address_line1, address_line2, city, state, zip, \
             latitude, longitude, address_key, source_key, has_minors, comments, ?2 FROM households WHERE id = ?1",
            params![id, reason],
        )
        .map_err(|e| e.to_string())?;
    if affected == 0 {
        return Err(format!("no household with id {id}"));
    }
    let deleted_id = tx.last_insert_rowid();

    // Mirror visits and tags before the DELETE below cascades them away.
    // Scoped blocks so each prepared statement (and its borrow of tx) is
    // dropped before the next tx.execute() call — the E0597 shape noted in
    // issue #21's constraints.
    {
        // Issue #69: carry visit_uid forward into the mirror too, so a
        // delete/restore round trip preserves per-visit identity the same
        // way household_uid is preserved above.
        let mut stmt = tx
            .prepare("SELECT visit_uid, visit_date, comments FROM visits WHERE household_id = ?1")
            .map_err(|e| e.to_string())?;
        let visits: Vec<(String, String, Option<String>)> = stmt
            .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        for (visit_uid, visit_date, comments) in visits {
            tx.execute(
                "INSERT INTO deleted_visits (deleted_household_id, visit_uid, visit_date, comments) VALUES (?1, ?2, ?3, ?4)",
                params![deleted_id, visit_uid, visit_date, comments],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    {
        let mut stmt = tx
            .prepare("SELECT tag_id FROM household_tags WHERE household_id = ?1")
            .map_err(|e| e.to_string())?;
        let tag_ids: Vec<i64> = stmt
            .query_map(params![id], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        for tag_id in tag_ids {
            tx.execute(
                "INSERT INTO deleted_household_tags (deleted_household_id, tag_id) VALUES (?1, ?2)",
                params![deleted_id, tag_id],
            )
            .map_err(|e| e.to_string())?;
        }
    }

    tx.execute("DELETE FROM households WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
    crate::commands::logs::log(&tx, "info", &format!("household {id} soft-deleted"), None);
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

fn normalize_tag(s: &str) -> String {
    s.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Serialize)]
pub struct DeletedHousehold {
    pub id: i64,
    pub original_id: i64,
    pub first_name: String,
    pub last_name: String,
    pub first_name_2: Option<String>,
    pub last_name_2: Option<String>,
    pub address_line1: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub zip: Option<String>,
    pub deletion_reason: Option<String>,
    pub deleted_at: String,
}

#[tauri::command]
pub fn list_deleted_households(state: State<AppState>) -> Result<Vec<DeletedHousehold>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, original_id, first_name, last_name, first_name_2, last_name_2, \
             address_line1, city, state, zip, deletion_reason, deleted_at \
             FROM deleted_households ORDER BY deleted_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(DeletedHousehold {
                id: row.get(0)?,
                original_id: row.get(1)?,
                first_name: row.get(2)?,
                last_name: row.get(3)?,
                first_name_2: row.get(4)?,
                last_name_2: row.get(5)?,
                address_line1: row.get(6)?,
                city: row.get(7)?,
                state: row.get(8)?,
                zip: row.get(9)?,
                deletion_reason: row.get(10)?,
                deleted_at: row.get(11)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// Moves a deleted_households row back into households — the reverse of
/// soft_delete_household. role/role_2/latitude/longitude/address_key/
/// source_key/has_minors/comments all carry over unchanged; only the id
/// changes (a fresh households.id, not the original one — a household
/// re-imported or re-tagged since deletion may already occupy that
/// address_key/source_key pairing, so reusing the old id isn't safe).
#[tauri::command]
pub fn restore_deleted_household(state: State<AppState>, id: i64) -> Result<(), String> {
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;

    // Issue #61: deleted_households doesn't carry source_key_seq (that's
    // Piece 2 of the issue, folded into the R-3 schema work instead of
    // done here) — the INSERT below can't just copy it forward the way
    // every other column is. Look up this row's source_key first, using
    // .optional() rather than a bare query_row so a bad id produces the
    // same clean "no deleted household with id {id}" error as before,
    // instead of a raw "query returned no rows" from rusqlite.
    let row: Option<(String, Option<String>)> = tx
        .query_row(
            "SELECT source_key, household_uid FROM deleted_households WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((source_key, household_uid)) = row else {
        return Err(format!("no deleted household with id {id}"));
    };
    // Issue #69: carry the uid forward, same as replace/merge does. A row
    // soft-deleted before household_uid existed has nothing to carry — mint
    // one now rather than leave the NOT NULL insert below with nothing to
    // write; landing on a different uid than a pre-migration household
    // "originally" had is unavoidable, there is no original value to recover.
    let household_uid = household_uid.unwrap_or_else(|| Uuid::new_v4().to_string());

    // Same expression import.rs's auto_accept_all()/resolve_review_item()
    // already use to assign a fresh seq on a new household — restoring
    // into whichever slot is actually free for this source_key, rather
    // than taking households.source_key_seq's DEFAULT 0 and colliding
    // with whatever a later import may have already placed there (the
    // father-and-adult-son case source_key_seq exists for in the first
    // place). Landing on a different seq than the household originally
    // had is fine — the seq is a disambiguator, not an identity.
    let next_seq: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(source_key_seq), -1) + 1 FROM households WHERE source_key = ?1",
            params![source_key],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    let affected = tx
        .execute(
            "INSERT INTO households (household_uid, first_name, last_name, role, phone_1, email_1, first_name_2, last_name_2, role_2, phone_2, email_2, \
             address_line1, address_line2, city, state, zip, latitude, longitude, address_key, source_key, source_key_seq, has_minors, comments) \
             SELECT ?2, first_name, last_name, role, phone_1, email_1, first_name_2, last_name_2, role_2, phone_2, email_2, \
             address_line1, address_line2, city, state, zip, latitude, longitude, address_key, source_key, ?3, has_minors, comments \
             FROM deleted_households WHERE id = ?1",
            params![id, household_uid, next_seq],
        )
        .map_err(|e| e.to_string())?;
    if affected == 0 {
        return Err(format!("no deleted household with id {id}"));
    }
    let new_id = tx.last_insert_rowid();

    // Restore tags and visits from their mirrors before the DELETE below
    // cascades the mirror rows away (both reference deleted_households(id)
    // ON DELETE CASCADE).
    {
        let mut stmt = tx
            .prepare("SELECT tag_id FROM deleted_household_tags WHERE deleted_household_id = ?1")
            .map_err(|e| e.to_string())?;
        let tag_ids: Vec<i64> = stmt
            .query_map(params![id], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        for tag_id in tag_ids {
            tx.execute(
                "INSERT OR IGNORE INTO household_tags (household_id, tag_id) VALUES (?1, ?2)",
                params![new_id, tag_id],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    {
        // Issue #69/#70: visits.visit_uid is NOT NULL — a deleted_visits row
        // from before visit_uid existed has none to carry forward, so mint
        // a fresh one per row here rather than fail the INSERT. Same
        // reasoning as household_uid's fallback above; no original value
        // to recover for a pre-migration row.
        let mut stmt = tx
            .prepare("SELECT visit_uid, visit_date, comments FROM deleted_visits WHERE deleted_household_id = ?1")
            .map_err(|e| e.to_string())?;
        let visits: Vec<(Option<String>, String, Option<String>)> = stmt
            .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        for (visit_uid, visit_date, comments) in visits {
            let visit_uid = visit_uid.unwrap_or_else(|| Uuid::new_v4().to_string());
            tx.execute(
                "INSERT INTO visits (household_id, visit_uid, visit_date, comments) VALUES (?1, ?2, ?3, ?4)",
                params![new_id, visit_uid, visit_date, comments],
            )
            .map_err(|e| e.to_string())?;
        }
    }

    tx.execute("DELETE FROM deleted_households WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
    crate::commands::logs::log(&tx, "info", &format!("deleted_households {id} restored to households"), None);
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}
