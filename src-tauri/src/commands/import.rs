use crate::pdf_parser::{self, ParsedRecord};
use crate::AppState;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

#[derive(Serialize)]
pub struct ImportSummary {
    pub batch_id: i64,
    pub total_rows: usize,
    pub new_count: usize,
    pub changed_count: usize,
    pub removed_count: usize,
    pub unchanged_count: usize,
    pub warnings: Vec<String>,
    /// True when the database was empty at import time — every record was
    /// inserted directly with no review step needed.
    pub auto_accepted: bool,
}

#[derive(Serialize, Clone)]
struct ImportProgress {
    processed: usize,
    total: usize,
}

/// Emits at most once per 10 records (plus always the final one) — enough
/// for a live counter without flooding IPC on a 10k-record import.
fn emit_progress(app: &AppHandle, processed: usize, total: usize) {
    if processed % 10 == 0 || processed == total {
        let _ = app.emit("import-progress", ImportProgress { processed, total });
    }
}

#[tauri::command]
pub async fn import_pdf(app: AppHandle, state: State<'_, AppState>, file_path: String) -> Result<ImportSummary, String> {
    let parsed = pdf_parser::parse_pdf(&app, std::path::Path::new(&file_path)).await.map_err(|e| e.to_string())?;
    run_diff(app, state, "pdf", &file_path, parsed.records, parsed.warnings.iter().map(|w| w.message.clone()).collect())
}

/// CSV import stub — same diff pipeline as PDF, minimal column mapping.
/// Real column layout TBD once a sample CSV export is available.
#[tauri::command]
pub fn import_csv(app: AppHandle, state: State<AppState>, file_path: String) -> Result<ImportSummary, String> {
    let text = std::fs::read_to_string(&file_path).map_err(|e| e.to_string())?;
    let mut records = Vec::new();
    let mut warnings = Vec::new();
    for (i, line) in text.lines().enumerate().skip(1) {
        // first_name,last_name,role,address1,address2,city,state,zip,lat,long
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        if f.len() < 4 {
            warnings.push(format!("row {}: expected at least 4 columns, got {}", i + 1, f.len()));
            continue;
        }
        records.push(ParsedRecord {
            first_name: f[0].to_string(),
            last_name: f[1].to_string(),
            role: if f.len() > 2 { f[2].to_string() } else { "head".to_string() },
            first_name_2: None,
            last_name_2: None,
            role_2: None,
            phone_1: None,
            email_1: None,
            phone_2: None,
            email_2: None,
            address_line1: f.get(3).filter(|s| !s.is_empty()).map(|s| s.to_string()),
            address_line2: f.get(4).filter(|s| !s.is_empty()).map(|s| s.to_string()),
            city: f.get(5).filter(|s| !s.is_empty()).map(|s| s.to_string()),
            state: f.get(6).filter(|s| !s.is_empty()).map(|s| s.to_string()),
            zip: f.get(7).filter(|s| !s.is_empty()).map(|s| s.to_string()),
            latitude: f.get(8).and_then(|s| s.parse().ok()),
            longitude: f.get(9).and_then(|s| s.parse().ok()),
            has_minors: false,
            comments: None,
        });
    }
    run_diff(app, state, "csv", &file_path, records, warnings)
}

fn run_diff(
    app: AppHandle,
    state: State<AppState>,
    source_type: &str,
    file_path: &str,
    incoming: Vec<ParsedRecord>,
    warnings: Vec<String>,
) -> Result<ImportSummary, String> {
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;

    let existing_household_count: i64 =
        conn.query_row("SELECT count(*) FROM households", [], |r| r.get(0)).map_err(|e| e.to_string())?;

    let filename = std::path::Path::new(file_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.to_string());

    if existing_household_count == 0 {
        return auto_accept_all(app, &mut conn, source_type, &filename, incoming, warnings);
    }

    let tx = conn.transaction().map_err(|e| e.to_string())?;

    tx.execute(
        "INSERT INTO import_batches (source_type, filename, total_rows) VALUES (?1, ?2, ?3)",
        params![source_type, filename, incoming.len() as i64],
    )
    .map_err(|e| e.to_string())?;
    let batch_id = tx.last_insert_rowid();

    let mut new_count = 0;
    let mut changed_count = 0;
    let mut unchanged_count = 0;
    let mut seen_source_keys: Vec<String> = Vec::new();
    let total = incoming.len();

    for (i, rec) in incoming.iter().enumerate() {
        let source_key = pdf_parser::source_key(&rec.first_name, &rec.last_name, &rec.first_name_2, &rec.last_name_2, &rec.address_line1);
        seen_source_keys.push(source_key.clone());

        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM households WHERE source_key = ?1",
                params![source_key],
                |r| r.get(0),
            )
            .ok();

        match existing {
            Some(_id) => {
                // Exact source_key match = unchanged, discarded per spec.
                unchanged_count += 1;
            }
            None => {
                // Could still be the SAME household with a changed address
                // — best-effort match on either head's name, else new.
                let name_match: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM households WHERE \
                         (first_name = ?1 AND last_name = ?2) OR (first_name_2 = ?1 AND last_name_2 = ?2) \
                         OR (?3 IS NOT NULL AND first_name = ?3 AND last_name = ?4) \
                         OR (?3 IS NOT NULL AND first_name_2 = ?3 AND last_name_2 = ?4)",
                        params![rec.first_name, rec.last_name, rec.first_name_2, rec.last_name_2],
                        |r| r.get(0),
                    )
                    .ok();
                let match_type = if name_match.is_some() { "changed" } else { "new" };
                if match_type == "changed" { changed_count += 1 } else { new_count += 1 };

                tx.execute(
                    "INSERT INTO review_queue (import_batch_id, match_type, incoming_data, existing_household_id) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        batch_id,
                        match_type,
                        serde_json::to_string(rec).map_err(|e| e.to_string())?,
                        name_match
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        emit_progress(&app, i + 1, total);
    }

    // Anything currently in the DB whose source_key wasn't seen in this
    // batch at all is a candidate removal — flagged for review, never
    // auto-deleted.
    {
        let mut stmt = tx.prepare("SELECT id, source_key FROM households").map_err(|e| e.to_string())?;
        let existing_keys: Vec<(i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        for (id, key) in existing_keys {
            if !seen_source_keys.contains(&key) {
                tx.execute(
                    "INSERT INTO review_queue (import_batch_id, match_type, existing_household_id) VALUES (?1, 'removed', ?2)",
                    params![batch_id, id],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }

    let review_count: i64 = tx
        .query_row("SELECT count(*) FROM review_queue WHERE import_batch_id = ?1", params![batch_id], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE import_batches SET new_count = ?1, review_count = ?2 WHERE id = ?3",
        params![new_count, review_count, batch_id],
    )
    .map_err(|e| e.to_string())?;

    tx.commit().map_err(|e| e.to_string())?;

    Ok(ImportSummary {
        batch_id,
        total_rows: incoming.len(),
        new_count,
        changed_count,
        removed_count: (review_count as usize).saturating_sub(new_count + changed_count),
        unchanged_count,
        warnings,
        auto_accepted: false,
    })
}

/// Empty-database fast path: nothing to diff against, so every parsed
/// record is inserted straight into `households` — no review_queue rows,
/// no manual per-item resolution. The batch is recorded as already
/// committed so it still shows up in import history.
fn auto_accept_all(
    app: AppHandle,
    conn: &mut rusqlite::Connection,
    source_type: &str,
    filename: &str,
    incoming: Vec<ParsedRecord>,
    warnings: Vec<String>,
) -> Result<ImportSummary, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;

    tx.execute(
        "INSERT INTO import_batches (source_type, filename, total_rows, new_count, review_count, status) \
         VALUES (?1, ?2, ?3, ?3, 0, 'committed')",
        params![source_type, filename, incoming.len() as i64],
    )
    .map_err(|e| e.to_string())?;
    let batch_id = tx.last_insert_rowid();
    let not_known_tag_id = crate::commands::tags::get_or_create_tag_id(&tx, "Not known").map_err(|e| e.to_string())?;

    let total = incoming.len();
    for (i, rec) in incoming.iter().enumerate() {
        let address_key = pdf_parser::address_key(&rec.address_line1, &rec.address_line2, &rec.city);
        let source_key = pdf_parser::source_key(&rec.first_name, &rec.last_name, &rec.first_name_2, &rec.last_name_2, &rec.address_line1);
        tx.execute(
            "INSERT INTO households (first_name, last_name, role, phone_1, email_1, first_name_2, last_name_2, role_2, phone_2, email_2, \
             address_line1, address_line2, city, state, zip, latitude, longitude, address_key, source_key, has_minors, comments) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
            params![
                rec.first_name, rec.last_name, rec.role, rec.phone_1, rec.email_1, rec.first_name_2, rec.last_name_2, rec.role_2,
                rec.phone_2, rec.email_2, rec.address_line1, rec.address_line2, rec.city, rec.state, rec.zip, rec.latitude, rec.longitude,
                address_key, source_key, rec.has_minors, rec.comments
            ],
        )
        .map_err(|e| e.to_string())?;
        let new_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT OR IGNORE INTO household_tags (household_id, tag_id) VALUES (?1, ?2)",
            params![new_id, not_known_tag_id],
        )
        .map_err(|e| e.to_string())?;
        emit_progress(&app, i + 1, total);
    }

    tx.commit().map_err(|e| e.to_string())?;
    super::logs::log(conn, "info", &format!("empty-database import: {total} households auto-accepted"), None);

    Ok(ImportSummary {
        batch_id,
        total_rows: total,
        new_count: total,
        changed_count: 0,
        removed_count: 0,
        unchanged_count: 0,
        warnings,
        auto_accepted: true,
    })
}

#[derive(Serialize)]
pub struct ReviewItem {
    pub id: i64,
    pub match_type: String,
    pub incoming_data: Option<String>, // JSON, frontend parses
    pub existing_household_id: Option<i64>,
    pub existing_summary: Option<String>,
}

#[tauri::command]
pub fn get_review_queue(state: State<AppState>, batch_id: i64) -> Result<Vec<ReviewItem>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT rq.id, rq.match_type, rq.incoming_data, rq.existing_household_id, \
             (SELECT first_name || ' ' || last_name || \
                    coalesce(' & ' || first_name_2 || ' ' || last_name_2, '') || \
                    ' — ' || coalesce(address_line1,'(no address)') \
              FROM households WHERE id = rq.existing_household_id) \
             FROM review_queue rq WHERE rq.import_batch_id = ?1 AND rq.resolution = 'pending'",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![batch_id], |r| {
            Ok(ReviewItem {
                id: r.get(0)?, match_type: r.get(1)?, incoming_data: r.get(2)?,
                existing_household_id: r.get(3)?, existing_summary: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// User-driven resolution of one review item: replace|merge|add|delete|ignore.
#[tauri::command]
pub fn resolve_review_item(state: State<AppState>, item_id: i64, action: String, comment: Option<String>) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let (match_type, incoming_json, existing_id): (String, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT match_type, incoming_data, existing_household_id FROM review_queue WHERE id = ?1",
            params![item_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|e| e.to_string())?;

    match action.as_str() {
        "add" | "replace" | "merge" => {
            let rec: ParsedRecord = serde_json::from_str(
                incoming_json.as_deref().ok_or("no incoming data for this action")?,
            )
            .map_err(|e| e.to_string())?;
            let address_key = pdf_parser::address_key(&rec.address_line1, &rec.address_line2, &rec.city);
            let source_key = pdf_parser::source_key(&rec.first_name, &rec.last_name, &rec.first_name_2, &rec.last_name_2, &rec.address_line1);

            // replace/merge delete the existing row, and ON DELETE CASCADE
            // would silently wipe its tag along with it — preserve it here
            // and restore it after the reinsert, since only a GENUINELY
            // new household should ever get auto-tagged/reset.
            let mut preserved_tag_ids: Vec<i64> = Vec::new();
            if action == "replace" || action == "merge" {
                if let Some(id) = existing_id {
                    let mut stmt = conn.prepare("SELECT tag_id FROM household_tags WHERE household_id = ?1").map_err(|e| e.to_string())?;
                    preserved_tag_ids = stmt
                        .query_map(params![id], |r| r.get(0))
                        .map_err(|e| e.to_string())?
                        .filter_map(Result::ok)
                        .collect();
                    conn.execute("DELETE FROM households WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
                }
            }
            conn.execute(
                "INSERT INTO households (first_name, last_name, role, phone_1, email_1, first_name_2, last_name_2, role_2, phone_2, email_2, \
                 address_line1, address_line2, city, state, zip, latitude, longitude, address_key, source_key, has_minors, comments) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
                params![
                    rec.first_name, rec.last_name, rec.role, rec.phone_1, rec.email_1, rec.first_name_2, rec.last_name_2, rec.role_2,
                    rec.phone_2, rec.email_2, rec.address_line1, rec.address_line2, rec.city, rec.state, rec.zip, rec.latitude, rec.longitude,
                    address_key, source_key, rec.has_minors, rec.comments
                ],
            )
            .map_err(|e| e.to_string())?;
            let new_id = conn.last_insert_rowid();

            if action == "add" {
                let not_known_id = crate::commands::tags::get_or_create_tag_id(&conn, "Not known").map_err(|e| e.to_string())?;
                conn.execute("INSERT OR IGNORE INTO household_tags (household_id, tag_id) VALUES (?1, ?2)", params![new_id, not_known_id])
                    .map_err(|e| e.to_string())?;
            } else {
                for tag_id in preserved_tag_ids {
                    conn.execute("INSERT OR IGNORE INTO household_tags (household_id, tag_id) VALUES (?1, ?2)", params![new_id, tag_id])
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        "delete" => {
            if let Some(id) = existing_id {
                conn.execute(
                    "INSERT INTO deleted_households (original_id, first_name, last_name, role, phone_1, email_1, first_name_2, last_name_2, role_2, phone_2, email_2, \
                     address_line1, address_line2, city, state, zip, latitude, longitude, address_key, source_key, has_minors, comments, deletion_reason) \
                     SELECT id, first_name, last_name, role, phone_1, email_1, first_name_2, last_name_2, role_2, phone_2, email_2, address_line1, address_line2, city, state, zip, \
                     latitude, longitude, address_key, source_key, has_minors, comments, ?2 FROM households WHERE id = ?1",
                    params![id, comment],
                )
                .map_err(|e| e.to_string())?;
                conn.execute("DELETE FROM households WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
            }
        }
        "ignore" => {}
        other => return Err(format!("unknown action '{other}'")),
    }

    let _ = match_type;
    conn.execute(
        "UPDATE review_queue SET resolution = ?1, resolution_comment = ?2, resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?3",
        params![action, comment, item_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn commit_import_batch(state: State<AppState>, batch_id: i64) -> Result<(), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let pending: i64 = conn
        .query_row(
            "SELECT count(*) FROM review_queue WHERE import_batch_id = ?1 AND resolution = 'pending'",
            params![batch_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if pending > 0 {
        return Err(format!("{pending} review item(s) still pending — resolve before committing"));
    }
    conn.execute("UPDATE import_batches SET status = 'committed' WHERE id = ?1", params![batch_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Bulk-resolves every still-pending "new" item in a batch as "add" — one
/// click instead of clicking Add on each one individually. Deliberately
/// leaves "changed" and "removed" items alone; those need a real decision
/// (replace vs merge vs add-as-new, confirm a removal), not a rubber stamp.
#[tauri::command]
pub fn resolve_all_new_records(state: State<AppState>, batch_id: i64) -> Result<i64, String> {
    let ids: Vec<i64> = {
        let conn = state.pool.get().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id FROM review_queue WHERE import_batch_id = ?1 AND match_type = 'new' AND resolution = 'pending'")
            .map_err(|e| e.to_string())?;
        let rows: Vec<i64> = stmt
            .query_map(params![batch_id], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        rows
    };
    let count = ids.len() as i64;
    for id in ids {
        resolve_review_item(state.clone(), id, "add".to_string(), None)?;
    }
    Ok(count)
}

/// The frontend used to track "which batch still needs review" purely in
/// a JS variable, which meant an unfinished review vanished across app
/// restarts even though the review_queue rows were still sitting there in
/// the database. This makes it discoverable again on load.
#[tauri::command]
pub fn get_pending_import_batch(state: State<AppState>) -> Result<Option<i64>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT import_batch_id FROM review_queue WHERE resolution = 'pending' ORDER BY import_batch_id DESC LIMIT 1",
        [],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}
