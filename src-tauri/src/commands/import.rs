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
    // Issue #32: resolved path is what actually gets opened / handed to
    // the pdftotext sidecar; file_path (raw, unvalidated) is kept around
    // only for the display filename in run_diff below.
    let resolved = crate::commands::paths::resolve_read_path(&file_path)?;
    let parsed = pdf_parser::parse_pdf(&app, &resolved).await.map_err(|e| e.to_string())?;
    run_diff(app, state, "pdf", &file_path, parsed.records, parsed.warnings.iter().map(|w| w.message.clone()).collect())
}

/// One of the six values `households.role`/`role_2` CHECK constraints permit
/// (schema.sql). Anything else is rejected here rather than carried forward —
/// an out-of-vocabulary role previously survived the whole review pipeline
/// and only failed at INSERT with a raw SQLite constraint error, and an
/// unescaped copy of it was also the vector for a stored-XSS finding in the
/// Review view (issue #18). Case-insensitive match against the schema's own
/// lowercase vocabulary; anything else defaults to "head" with a warning.
const VALID_ROLES: [&str; 6] = ["head", "husband", "wife", "minor", "grandparent", "other"];

fn normalize_role(raw: &str, row_num: usize, warnings: &mut Vec<String>) -> String {
    let trimmed = raw.trim();
    let lower = trimmed.to_lowercase();
    if VALID_ROLES.contains(&lower.as_str()) {
        return lower;
    }
    warnings.push(format!(
        "row {row_num}: role '{trimmed}' is not one of {VALID_ROLES:?} — defaulted to 'head'"
    ));
    "head".to_string()
}

/// CSV import stub — same diff pipeline as PDF, minimal column mapping.
/// Real column layout TBD once a sample CSV export is available.
#[tauri::command]
pub fn import_csv(app: AppHandle, state: State<AppState>, file_path: String) -> Result<ImportSummary, String> {
    // Issue #32: resolved path is what's actually read.
    let resolved = super::paths::resolve_read_path(&file_path)?;
    let text = std::fs::read_to_string(&resolved).map_err(|e| e.to_string())?;
    let mut records = Vec::new();
    let mut warnings = Vec::new();
    for (i, line) in text.lines().enumerate().skip(1) {
        // first_name,last_name,role,address1,address2,city,state,zip,lat,long
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        if f.len() < 4 {
            warnings.push(format!("row {}: expected at least 4 columns, got {}", i + 1, f.len()));
            continue;
        }
        let role = if f.len() > 2 && !f[2].is_empty() {
            normalize_role(f[2], i + 1, &mut warnings)
        } else {
            "head".to_string()
        };
        records.push(ParsedRecord {
            first_name: f[0].to_string(),
            last_name: f[1].to_string(),
            role,
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

        let matching_ids: Vec<i64> = {
            let mut stmt = tx.prepare("SELECT id FROM households WHERE source_key = ?1").map_err(|e| e.to_string())?;
            let ids: Vec<i64> = stmt
                .query_map(params![source_key], |r| r.get(0))
                .map_err(|e| e.to_string())?
                .filter_map(Result::ok)
                .collect();
            ids
        };

        // A source_key matching more than one existing household is a
        // known collision (father/son, same name+address, no field left
        // to tell them apart) — no way to auto-decide which one this row
        // corresponds to, so don't guess. Falls through to the same
        // name-match/new path used when there's no match at all.
        if matching_ids.len() == 1 {
            // Exact source_key match = unchanged, discarded per spec.
            unchanged_count += 1;
        } else {
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
        // A single PDF can itself contain two distinct households that
        // collide on source_key (father/son at the same address) — even
        // starting from an empty DB. Pick the next free seq for this key
        // rather than assuming 0.
        let source_key_seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(source_key_seq), -1) + 1 FROM households WHERE source_key = ?1",
                params![source_key],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO households (first_name, last_name, role, phone_1, email_1, first_name_2, last_name_2, role_2, phone_2, email_2, \
             address_line1, address_line2, city, state, zip, latitude, longitude, address_key, source_key, source_key_seq, has_minors, comments) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",
            params![
                rec.first_name, rec.last_name, rec.role, rec.phone_1, rec.email_1, rec.first_name_2, rec.last_name_2, rec.role_2,
                rec.phone_2, rec.email_2, rec.address_line1, rec.address_line2, rec.city, rec.state, rec.zip, rec.latitude, rec.longitude,
                address_key, source_key, source_key_seq, rec.has_minors, rec.comments
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
///
/// Fully transactional (issue #21) — a failure at any point leaves the
/// review item and every table it touches exactly as they were before the
/// call.
///
/// replace/merge no longer destroy the outgoing household's visit history
/// or comments (issue #19/#20): visits are re-pointed at the new row's id
/// BEFORE the old row is deleted, so ON DELETE CASCADE has nothing left to
/// remove by the time the DELETE runs; comments are preserved the same way
/// tags already were, concatenated with the incoming value on the rare
/// occasion both are present (the 3+-heads warning case) rather than
/// discarding one. Merge and Replace are treated identically for this
/// purpose — making them behave differently for other fields (address,
/// phone, etc.) is a separate product decision the issue explicitly left
/// open for discussion and is not implemented here.
#[tauri::command]
pub fn resolve_review_item(state: State<AppState>, item_id: i64, action: String, comment: Option<String>) -> Result<(), String> {
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;

    let (match_type, incoming_json, existing_id): (String, Option<String>, Option<i64>) = tx
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

            // replace/merge preserve the outgoing household's tag(s) and
            // comments before the old row is touched — ON DELETE CASCADE
            // would otherwise wipe the tag along with it (the original #19
            // finding), and the parser's own comments value is almost
            // always None, so an unconditional overwrite silently erased
            // whatever the user had typed (#20). Only a GENUINELY new
            // household should ever get auto-tagged/reset.
            let mut preserved_tag_ids: Vec<i64> = Vec::new();
            let mut existing_comments: Option<String> = None;
            if action == "replace" || action == "merge" {
                if let Some(old_id) = existing_id {
                    {
                        let mut stmt = tx
                            .prepare("SELECT tag_id FROM household_tags WHERE household_id = ?1")
                            .map_err(|e| e.to_string())?;
                        preserved_tag_ids = stmt
                            .query_map(params![old_id], |r| r.get(0))
                            .map_err(|e| e.to_string())?
                            .collect::<Result<_, _>>()
                            .map_err(|e| e.to_string())?;
                    }
                    existing_comments = tx
                        .query_row("SELECT comments FROM households WHERE id = ?1", params![old_id], |r| r.get::<_, Option<String>>(0))
                        .optional()
                        .map_err(|e| e.to_string())?
                        .flatten();
                    // The new row is inserted before the old one is
                    // deleted (visits must be re-pointed to the new id
                    // first), so if the incoming record's source_key is
                    // unchanged, the old row is still occupying the exact
                    // (source_key, source_key_seq) slot the new row would
                    // naturally reclaim. Bump the outgoing row onto a
                    // guaranteed-free sentinel first — -id is always
                    // negative and therefore never collides with a real
                    // slot — so the insert below never trips the
                    // UNIQUE(source_key, source_key_seq) constraint. The
                    // row is deleted a few lines down regardless.
                    tx.execute("UPDATE households SET source_key_seq = -id WHERE id = ?1", params![old_id])
                        .map_err(|e| e.to_string())?;
                }
            }

            let final_comments = match (rec.comments.as_deref(), existing_comments.as_deref()) {
                (None, None) => None,
                (Some(incoming), None) => Some(incoming.to_string()),
                (None, Some(existing)) => Some(existing.to_string()),
                (Some(incoming), Some(existing)) if incoming == existing => Some(existing.to_string()),
                (Some(incoming), Some(existing)) => Some(format!("{incoming}\n\n{existing}")),
            };

            let source_key_seq: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(source_key_seq), -1) + 1 FROM households WHERE source_key = ?1",
                    params![source_key],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;

            tx.execute(
                "INSERT INTO households (first_name, last_name, role, phone_1, email_1, first_name_2, last_name_2, role_2, phone_2, email_2, \
                 address_line1, address_line2, city, state, zip, latitude, longitude, address_key, source_key, source_key_seq, has_minors, comments) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",
                params![
                    rec.first_name, rec.last_name, rec.role, rec.phone_1, rec.email_1, rec.first_name_2, rec.last_name_2, rec.role_2,
                    rec.phone_2, rec.email_2, rec.address_line1, rec.address_line2, rec.city, rec.state, rec.zip, rec.latitude, rec.longitude,
                    address_key, source_key, source_key_seq, rec.has_minors, final_comments
                ],
            )
            .map_err(|e| e.to_string())?;
            let new_id = tx.last_insert_rowid();

            if action == "add" {
                let not_known_id = crate::commands::tags::get_or_create_tag_id(&tx, "Not known").map_err(|e| e.to_string())?;
                tx.execute("INSERT OR IGNORE INTO household_tags (household_id, tag_id) VALUES (?1, ?2)", params![new_id, not_known_id])
                    .map_err(|e| e.to_string())?;
            } else {
                for tag_id in preserved_tag_ids {
                    tx.execute("INSERT OR IGNORE INTO household_tags (household_id, tag_id) VALUES (?1, ?2)", params![new_id, tag_id])
                        .map_err(|e| e.to_string())?;
                }
                if let Some(old_id) = existing_id {
                    // Re-point visits at the new row BEFORE the old one is
                    // deleted, so the cascade below has nothing left to
                    // remove (#19) — order matters: this must run before
                    // the DELETE, not after.
                    tx.execute("UPDATE visits SET household_id = ?1 WHERE household_id = ?2", params![new_id, old_id])
                        .map_err(|e| e.to_string())?;
                    tx.execute("DELETE FROM households WHERE id = ?1", params![old_id]).map_err(|e| e.to_string())?;
                }
            }
        }
        "delete" => {
            if let Some(id) = existing_id {
                let affected = tx
                    .execute(
                        "INSERT INTO deleted_households (original_id, first_name, last_name, role, phone_1, email_1, first_name_2, last_name_2, role_2, phone_2, email_2, \
                         address_line1, address_line2, city, state, zip, latitude, longitude, address_key, source_key, has_minors, comments, deletion_reason) \
                         SELECT id, first_name, last_name, role, phone_1, email_1, first_name_2, last_name_2, role_2, phone_2, email_2, address_line1, address_line2, city, state, zip, \
                         latitude, longitude, address_key, source_key, has_minors, comments, ?2 FROM households WHERE id = ?1",
                        params![id, comment],
                    )
                    .map_err(|e| e.to_string())?;
                if affected == 0 {
                    return Err(format!("no household with id {id}"));
                }
                let deleted_id = tx.last_insert_rowid();

                // Mirror visits and tags before the DELETE cascades them
                // away — same treatment as soft_delete_household (#19).
                {
                    let mut stmt = tx
                        .prepare("SELECT visit_date, comments FROM visits WHERE household_id = ?1")
                        .map_err(|e| e.to_string())?;
                    let visits: Vec<(String, Option<String>)> = stmt
                        .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))
                        .map_err(|e| e.to_string())?
                        .collect::<Result<_, _>>()
                        .map_err(|e| e.to_string())?;
                    for (visit_date, v_comments) in visits {
                        tx.execute(
                            "INSERT INTO deleted_visits (deleted_household_id, visit_date, comments) VALUES (?1, ?2, ?3)",
                            params![deleted_id, visit_date, v_comments],
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
            }
        }
        "ignore" => {}
        other => return Err(format!("unknown action '{other}'")),
    }

    let _ = match_type;
    tx.execute(
        "UPDATE review_queue SET resolution = ?1, resolution_comment = ?2, resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?3",
        params![action, comment, item_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
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
