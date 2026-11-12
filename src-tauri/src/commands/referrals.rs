// Clerk referrals (docs/Record_Referral_Design.md): record a change the
// ward clerk has to make, export the open ones for the clerk, mark them
// reported, and close them by hand (confirm or cancel).
//
// Stage 2 closes the loop through the weekly directory import: Review
// Updates shows which open referrals a removed or changed household
// matches (review_referrals), resolving the item confirms them
// (confirm_from_import), and a reported referral whose household is still
// in the directory after enough imports is overdue (annotate_overdue).
//
// Each command is a thin wrapper over a plain function taking a
// Connection, so the logic is testable without Tauri.

use crate::pdf_parser::ParsedRecord;
use crate::AppState;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::State;

const KINDS: [&str; 6] = [
    "moved_known",
    "moved_unknown",
    "deceased",
    "contact_info",
    "remove_records",
    "other",
];

#[derive(Serialize, Debug)]
pub struct Referral {
    pub id: i64,
    /// Display reference, "R-0007": what the user and the clerk quote in email.
    pub reference: String,
    pub household_uid: String,
    /// The live household's current id, or None once it has left the
    /// households table (deleted on import, soft-deleted, pruned).
    pub household_id: Option<i64>,
    pub snapshot_name: String,
    pub snapshot_address: Option<String>,
    pub kind: String,
    pub details: Option<String>,
    pub new_address: Option<String>,
    pub learned_on: String,
    pub visit_uid: Option<String>,
    pub status: String,
    pub reported_on: Option<String>,
    pub resolved_on: Option<String>,
    pub resolution_note: Option<String>,
    pub created_at: String,
    /// Completed imports dated after reported_on that still listed the
    /// household. Only counted for `reported` referrals whose household is
    /// still present; 0 otherwise.
    pub imports_since: i64,
    /// Date of the latest of those imports ("still listed as of").
    pub last_import_on: Option<String>,
    /// imports_since has reached the referralOverdueImports setting.
    pub overdue: bool,
}

fn reference(id: i64) -> String {
    format!("R-{id:04}")
}

/// Strict YYYY-MM-DD, re-formatted so stored dates always compare
/// correctly as strings (same rule as visits::parse_iso_date).
fn canonical_date(s: &str) -> Result<String, String> {
    chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map(|d| d.format("%Y-%m-%d").to_string())
        .map_err(|_| format!("'{s}' is not a valid date — use YYYY-MM-DD (e.g. 2026-03-05)"))
}

fn today() -> String {
    chrono::Local::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

fn blank_to_none(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn check_kind(kind: &str) -> Result<(), String> {
    if KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(format!("unknown referral kind '{kind}'"))
    }
}

/// "Smith, John & Mary", or "Smith, John & Mary Jones" when the second
/// head's surname differs.
fn household_display_name(
    first: &str,
    last: &str,
    first_2: Option<&str>,
    last_2: Option<&str>,
) -> String {
    let mut name = format!("{last}, {first}");
    if let Some(f2) = first_2.filter(|s| !s.is_empty()) {
        name.push_str(" & ");
        name.push_str(f2);
        if let Some(l2) = last_2.filter(|s| !s.is_empty() && *s != last) {
            name.push(' ');
            name.push_str(l2);
        }
    }
    name
}

/// "123 Oak St, Apt 2, Winchester VA 22601", or None with no address.
fn one_line_address(
    line1: Option<&str>,
    line2: Option<&str>,
    city: Option<&str>,
    state: Option<&str>,
    zip: Option<&str>,
) -> Option<String> {
    let csz = [city, state, zip]
        .into_iter()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let parts: Vec<&str> = [line1, line2]
        .into_iter()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .chain(std::iter::once(csz.as_str()).filter(|s| !s.is_empty()))
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

const SELECT_REFERRAL: &str =
    "SELECT r.id, r.household_uid, h.id, r.snapshot_name, r.snapshot_address, r.kind, \
     r.details, r.new_address, r.learned_on, r.visit_uid, r.status, r.reported_on, r.resolved_on, \
     r.resolution_note, r.created_at \
     FROM referrals r LEFT JOIN households h ON h.household_uid = r.household_uid";

fn row_to_referral(row: &rusqlite::Row) -> rusqlite::Result<Referral> {
    let id: i64 = row.get(0)?;
    Ok(Referral {
        id,
        reference: reference(id),
        household_uid: row.get(1)?,
        household_id: row.get(2)?,
        snapshot_name: row.get(3)?,
        snapshot_address: row.get(4)?,
        kind: row.get(5)?,
        details: row.get(6)?,
        new_address: row.get(7)?,
        learned_on: row.get(8)?,
        visit_uid: row.get(9)?,
        status: row.get(10)?,
        reported_on: row.get(11)?,
        resolved_on: row.get(12)?,
        resolution_note: row.get(13)?,
        created_at: row.get(14)?,
        imports_since: 0,
        last_import_on: None,
        overdue: false,
    })
}

/// The referralOverdueImports setting: how many completed imports after
/// the reported date may still show the household before the referral is
/// overdue. Default 2 (about two weeks at weekly imports), at least 1.
pub fn overdue_threshold(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT value FROM settings WHERE key = 'referralOverdueImports'",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| v.trim().parse::<i64>().ok())
    .unwrap_or(2)
    .max(1)
}

/// Fills imports_since / last_import_on / overdue (design §2.2).
///
/// An import counts once it is finished: not discarded, and with no
/// review item still pending. Counting imports rather than days means a
/// stretch with no import never makes anything overdue: overdue says the
/// directory was checked and the change was still not there. A referral
/// whose household has left the directory is not overdue; the change it
/// asked for has happened, or Review Updates is asking about it.
pub fn annotate_overdue(conn: &Connection, referrals: &mut [Referral]) -> Result<(), String> {
    let threshold = overdue_threshold(conn);
    let mut stmt = conn
        .prepare_cached(
            "SELECT count(*), max(date(b.imported_at, 'localtime')) FROM import_batches b \
             WHERE b.status <> 'discarded' AND date(b.imported_at, 'localtime') > ?1 \
               AND NOT EXISTS (SELECT 1 FROM review_queue q \
                               WHERE q.import_batch_id = b.id AND q.resolution = 'pending')",
        )
        .map_err(|e| e.to_string())?;
    for r in referrals.iter_mut() {
        let (Some(reported_on), Some(_)) = (r.reported_on.as_deref(), r.household_id) else {
            continue;
        };
        if r.status != "reported" {
            continue;
        }
        let (n, last): (i64, Option<String>) = stmt
            .query_row(params![reported_on], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| e.to_string())?;
        r.imports_since = n;
        r.last_import_on = last;
        r.overdue = n >= threshold;
    }
    Ok(())
}

fn get_referral(conn: &Connection, id: i64) -> Result<Referral, String> {
    conn.query_row(
        &format!("{SELECT_REFERRAL} WHERE r.id = ?1"),
        params![id],
        row_to_referral,
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("referral {id} not found"))
    .and_then(|r| {
        let mut one = [r];
        annotate_overdue(conn, &mut one)?;
        let [r] = one;
        Ok(r)
    })
}

/// Every referral, or only one household's (by its current id, matched
/// through household_uid). Newest first. The table stays small (dozens a
/// year), so the frontend filters by status itself.
pub fn list_referrals_query(
    conn: &Connection,
    household_id: Option<i64>,
) -> Result<Vec<Referral>, String> {
    let sql = match household_id {
        Some(_) => format!(
            "{SELECT_REFERRAL} WHERE r.household_uid = \
             (SELECT household_uid FROM households WHERE id = ?1) ORDER BY r.id DESC"
        ),
        None => format!("{SELECT_REFERRAL} ORDER BY r.id DESC"),
    };
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = match household_id {
        Some(hid) => stmt.query_map(params![hid], row_to_referral),
        None => stmt.query_map([], row_to_referral),
    }
    .map_err(|e| e.to_string())?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| e.to_string())?;
    let mut rows = rows;
    annotate_overdue(conn, &mut rows)?;
    Ok(rows)
}

pub struct NewReferral {
    pub household_id: i64,
    pub kind: String,
    pub details: Option<String>,
    pub new_address: Option<String>,
    pub learned_on: String,
    pub visit_uid: Option<String>,
}

pub fn create_referral_query(conn: &Connection, n: NewReferral) -> Result<Referral, String> {
    check_kind(&n.kind)?;
    let learned_on = canonical_date(&n.learned_on)?;
    let new_address = if n.kind == "moved_known" {
        blank_to_none(n.new_address)
    } else {
        None
    };

    type Snapshot = (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let hh: Option<Snapshot> = conn
        .query_row(
            "SELECT household_uid, first_name, last_name, first_name_2, last_name_2, \
             address_line1, address_line2, city, state, zip FROM households WHERE id = ?1",
            params![n.household_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let (uid, first, last, first_2, last_2, a1, a2, city, st, zip) =
        hh.ok_or_else(|| format!("household {} not found", n.household_id))?;

    let name = household_display_name(&first, &last, first_2.as_deref(), last_2.as_deref());
    let address = one_line_address(
        a1.as_deref(),
        a2.as_deref(),
        city.as_deref(),
        st.as_deref(),
        zip.as_deref(),
    );

    conn.execute(
        "INSERT INTO referrals (household_uid, snapshot_name, snapshot_address, kind, details, \
         new_address, learned_on, visit_uid) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            uid,
            name,
            address,
            n.kind,
            blank_to_none(n.details),
            new_address,
            learned_on,
            n.visit_uid
        ],
    )
    .map_err(|e| e.to_string())?;
    get_referral(conn, conn.last_insert_rowid())
}

/// Edits the user-entered fields of an open referral. Closed ones are
/// history and stay as they were.
pub fn update_referral_query(
    conn: &Connection,
    id: i64,
    kind: &str,
    details: Option<String>,
    new_address: Option<String>,
    learned_on: &str,
) -> Result<Referral, String> {
    check_kind(kind)?;
    let learned_on = canonical_date(learned_on)?;
    let new_address = if kind == "moved_known" {
        blank_to_none(new_address)
    } else {
        None
    };
    let n = conn
        .execute(
            "UPDATE referrals SET kind = ?1, details = ?2, new_address = ?3, learned_on = ?4 \
             WHERE id = ?5 AND status IN ('to_report', 'reported')",
            params![kind, blank_to_none(details), new_address, learned_on, id],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("{} is closed or does not exist", reference(id)));
    }
    get_referral(conn, id)
}

/// Marks exactly these referrals reported on `reported_on`. Only ones
/// still `to_report` change, so a referral already reported keeps its
/// first-sent date. Returns how many changed.
pub fn mark_reported_query(
    conn: &mut Connection,
    ids: &[i64],
    reported_on: &str,
) -> Result<usize, String> {
    let reported_on = canonical_date(reported_on)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut changed = 0;
    for id in ids {
        changed += tx
            .execute(
                "UPDATE referrals SET status = 'reported', reported_on = ?1 \
                 WHERE id = ?2 AND status = 'to_report'",
                params![reported_on, id],
            )
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(changed)
}

/// Closes an open referral as confirmed (the record was updated) or
/// cancelled (withdrawn), with the date and an optional note.
pub fn close_referral_query(
    conn: &Connection,
    id: i64,
    status: &str,
    note: Option<String>,
    on: &str,
) -> Result<Referral, String> {
    if status != "confirmed" && status != "cancelled" {
        return Err(format!("cannot close a referral as '{status}'"));
    }
    let n = conn
        .execute(
            "UPDATE referrals SET status = ?1, resolved_on = ?2, resolution_note = ?3 \
             WHERE id = ?4 AND status IN ('to_report', 'reported')",
            params![status, on, blank_to_none(note), id],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!(
            "{} is already closed or does not exist",
            reference(id)
        ));
    }
    get_referral(conn, id)
}

/// Reopens a closed referral, e.g. one confirmed or cancelled by mistake.
/// It goes back to `reported` if it had been sent, else `to_report`.
pub fn reopen_referral_query(conn: &Connection, id: i64) -> Result<Referral, String> {
    let n = conn
        .execute(
            "UPDATE referrals SET \
               status = CASE WHEN reported_on IS NULL THEN 'to_report' ELSE 'reported' END, \
               resolved_on = NULL, resolution_note = NULL, import_batch_id = NULL \
             WHERE id = ?1 AND status IN ('confirmed', 'cancelled')",
            params![id],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("{} is not closed or does not exist", reference(id)));
    }
    get_referral(conn, id)
}

// ── Import confirmation (design §3.3) ───────────────────────────────────

/// One open referral as it relates to a Review Updates item.
#[derive(Serialize, Debug, Clone)]
pub struct ReviewReferral {
    pub id: i64,
    pub reference: String,
    pub kind: String,
    pub status: String,
    pub reported_on: Option<String>,
    pub new_address: Option<String>,
    /// What resolving the item (Delete, or Replace/Merge/Link) does to it:
    /// "confirm" — confirmed automatically;
    /// "ask" — the directory changed, but not clearly as reported; the
    ///         user chooses Confirm or Leave open;
    /// "none" — this change says nothing about it; it stays open.
    pub effect: String,
    /// Why, in a few words, for Review Updates and the resolution note.
    pub reason: String,
}

/// Short kind label for the deletion reason ("Moved — clerk referral R-0012").
fn kind_short_label(kind: &str) -> &'static str {
    match kind {
        "moved_known" | "moved_unknown" => "Moved",
        "deceased" => "Deceased",
        "contact_info" => "Contact info",
        "remove_records" => "Records removal requested",
        _ => "Clerk referral",
    }
}

/// Lower-case alphanumeric words, for comparing a typed address with the
/// directory's (the same normalisation as pdf_parser::address_key).
fn address_words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Whether the directory's new address is the one the user reported:
/// the reported address starts with the street line (and unit line, if
/// any), and names the city if the directory has one. A miss falls to
/// "ask", which is safe because the user decides.
pub fn address_matches(reported: &str, incoming: &ParsedRecord) -> bool {
    let reported = address_words(reported);
    let mut street = address_words(incoming.address_line1.as_deref().unwrap_or(""));
    if street.is_empty() {
        return false;
    }
    street.extend(address_words(
        incoming.address_line2.as_deref().unwrap_or(""),
    ));
    if !reported.starts_with(&street) {
        return false;
    }
    match incoming.city.as_deref().map(address_words) {
        Some(city) if !city.is_empty() => {
            reported.windows(city.len()).any(|w| w == city.as_slice())
        }
        _ => true,
    }
}

/// The open referrals on `household_uid` and what resolving a Review
/// Updates item of `match_type` would do to each.
///
/// `changed` is the item's coarse changed-field list (import::changed_fields:
/// "Name", "Address", "Coordinates", "Phone/Email", "Comments") and
/// `incoming` its incoming record; both only matter for "changed".
pub fn review_referrals(
    conn: &Connection,
    household_uid: &str,
    match_type: &str,
    incoming: Option<&ParsedRecord>,
    changed: &[String],
) -> Result<Vec<ReviewReferral>, String> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, kind, status, reported_on, new_address FROM referrals \
             WHERE household_uid = ?1 AND status IN ('to_report', 'reported') ORDER BY id",
        )
        .map_err(|e| e.to_string())?;
    // id, kind, status, reported_on, new_address
    type OpenRow = (i64, String, String, Option<String>, Option<String>);
    let open: Vec<OpenRow> = stmt
        .query_map(params![household_uid], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;

    let has = |f: &str| changed.iter().any(|c| c == f);
    let address = has("Address");
    let name = has("Name");
    let contact = has("Phone/Email");

    Ok(open
        .into_iter()
        .map(|(id, kind, status, reported_on, new_address)| {
            let (effect, reason) = if match_type == "removed" {
                ("confirm", "Removed from directory".to_string())
            } else {
                match kind.as_str() {
                    "moved_known" if address => {
                        let matched = match (new_address.as_deref(), incoming) {
                            (Some(a), Some(rec)) => address_matches(a, rec),
                            _ => false,
                        };
                        if matched {
                            ("confirm", "Address updated".to_string())
                        } else {
                            (
                                "ask",
                                "Address changed, but not to the reported one".to_string(),
                            )
                        }
                    }
                    "moved_unknown" if address => (
                        "ask",
                        "Address changed; no new address was reported".to_string(),
                    ),
                    "contact_info" if address || contact => {
                        ("confirm", "Contact info updated".to_string())
                    }
                    "deceased" | "remove_records" if name => {
                        ("ask", "Names on the record changed".to_string())
                    }
                    "other" if name || address || contact => {
                        ("ask", "The record changed".to_string())
                    }
                    _ => ("none", "This change does not settle it".to_string()),
                }
            };
            ReviewReferral {
                id,
                reference: reference(id),
                kind,
                status,
                reported_on,
                new_address,
                effect: effect.to_string(),
                reason,
            }
        })
        .collect())
}

/// The deletion reason Review Updates' Delete records when the user gives
/// none: "Moved — clerk referral R-0012" (design §3.3), from the first
/// referral the deletion confirms.
pub fn deletion_reason(referrals: &[ReviewReferral]) -> Option<String> {
    referrals.iter().find(|r| r.effect == "confirm").map(|r| {
        format!(
            "{} — clerk referral {}",
            kind_short_label(&r.kind),
            r.reference
        )
    })
}

/// Confirms an open referral from a Review Updates resolution, inside the
/// caller's transaction. A `to_report` referral may be confirmed too (the
/// clerk heard from someone else first); its reported_on stays NULL.
pub fn confirm_from_import(
    conn: &Connection,
    id: i64,
    note: &str,
    batch_id: i64,
    on: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE referrals SET status = 'confirmed', resolved_on = ?1, resolution_note = ?2, \
         import_batch_id = ?3 WHERE id = ?4 AND status IN ('to_report', 'reported')",
        params![on, note, batch_id, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// For the post-import summary: referrals the batch confirmed, and how
/// many are overdue now.
pub fn import_summary_counts(conn: &Connection, batch_id: i64) -> Result<(i64, i64), String> {
    let confirmed: i64 = conn
        .query_row(
            "SELECT count(*) FROM referrals WHERE import_batch_id = ?1",
            params![batch_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let overdue = list_referrals_query(conn, None)?
        .iter()
        .filter(|r| r.overdue)
        .count() as i64;
    Ok((confirmed, overdue))
}

pub fn today_local() -> String {
    today()
}

fn log(conn: &Connection, message: &str) {
    crate::commands::logs::log(conn, "info", message, None);
}

#[tauri::command]
pub fn list_referrals(
    state: State<AppState>,
    household_id: Option<i64>,
) -> Result<Vec<Referral>, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    list_referrals_query(&conn, household_id)
}

#[tauri::command]
pub fn create_referral(
    state: State<AppState>,
    household_id: i64,
    kind: String,
    details: Option<String>,
    new_address: Option<String>,
    learned_on: String,
    visit_uid: Option<String>,
) -> Result<Referral, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let r = create_referral_query(
        &conn,
        NewReferral {
            household_id,
            kind,
            details,
            new_address,
            learned_on,
            visit_uid: blank_to_none(visit_uid),
        },
    )?;
    log(
        &conn,
        &format!("clerk referral {} created ({})", r.reference, r.kind),
    );
    Ok(r)
}

#[tauri::command]
pub fn update_referral(
    state: State<AppState>,
    id: i64,
    kind: String,
    details: Option<String>,
    new_address: Option<String>,
    learned_on: String,
) -> Result<Referral, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let r = update_referral_query(&conn, id, &kind, details, new_address, &learned_on)?;
    log(&conn, &format!("clerk referral {} edited", r.reference));
    Ok(r)
}

#[tauri::command]
pub fn mark_referrals_reported(
    state: State<AppState>,
    ids: Vec<i64>,
    reported_on: Option<String>,
) -> Result<usize, String> {
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let on = reported_on.unwrap_or_else(today);
    let n = mark_reported_query(&mut conn, &ids, &on)?;
    log(
        &conn,
        &format!("{n} clerk referral(s) marked reported on {on}"),
    );
    Ok(n)
}

#[tauri::command]
pub fn close_referral(
    state: State<AppState>,
    id: i64,
    status: String,
    note: Option<String>,
) -> Result<Referral, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let r = close_referral_query(&conn, id, &status, note, &today())?;
    log(
        &conn,
        &format!("clerk referral {} {}", r.reference, r.status),
    );
    Ok(r)
}

#[tauri::command]
pub fn reopen_referral(state: State<AppState>, id: i64) -> Result<Referral, String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    let r = reopen_referral_query(&conn, id)?;
    log(&conn, &format!("clerk referral {} reopened", r.reference));
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../db/schema.sql")).unwrap();
        c
    }

    fn household(c: &Connection, uid: &str) -> i64 {
        c.execute(
            "INSERT INTO households (household_uid, first_name, last_name, role, first_name_2, last_name_2, \
             address_line1, city, state, zip, address_key, source_key) \
             VALUES (?1, 'John', 'Smith', 'head', 'Mary', 'Smith', '123 Oak St', 'Winchester', 'VA', '22601', ?1, ?1)",
            params![uid],
        )
        .unwrap();
        c.last_insert_rowid()
    }

    fn new(hh: i64, kind: &str) -> NewReferral {
        NewReferral {
            household_id: hh,
            kind: kind.into(),
            details: Some("  neighbor said  ".into()),
            new_address: Some("45 Elm Rd".into()),
            learned_on: "2026-11-8".into(),
            visit_uid: None,
        }
    }

    #[test]
    fn create_snapshots_household_and_normalizes() {
        let c = conn();
        let hh = household(&c, "uid-1");
        let r = create_referral_query(&c, new(hh, "moved_known")).unwrap();
        assert_eq!(r.reference, format!("R-{:04}", r.id));
        assert_eq!(r.household_uid, "uid-1");
        assert_eq!(r.household_id, Some(hh));
        assert_eq!(r.snapshot_name, "Smith, John & Mary");
        assert_eq!(
            r.snapshot_address.as_deref(),
            Some("123 Oak St, Winchester VA 22601")
        );
        assert_eq!(r.learned_on, "2026-11-08");
        assert_eq!(r.details.as_deref(), Some("neighbor said"));
        assert_eq!(r.new_address.as_deref(), Some("45 Elm Rd"));
        assert_eq!(r.status, "to_report");
    }

    #[test]
    fn new_address_kept_only_for_moved_known() {
        let c = conn();
        let hh = household(&c, "uid-1");
        let r = create_referral_query(&c, new(hh, "deceased")).unwrap();
        assert_eq!(r.new_address, None);
    }

    #[test]
    fn create_rejects_bad_input() {
        let c = conn();
        let hh = household(&c, "uid-1");
        assert!(create_referral_query(&c, new(hh, "lost")).is_err());
        assert!(create_referral_query(&c, new(hh + 99, "other")).is_err());
        let mut bad_date = new(hh, "other");
        bad_date.learned_on = "11/08/2026".into();
        assert!(create_referral_query(&c, bad_date).is_err());
    }

    #[test]
    fn referral_outlives_household_and_follows_uid() {
        let c = conn();
        let hh = household(&c, "uid-1");
        let r = create_referral_query(&c, new(hh, "moved_unknown")).unwrap();

        // Replace/Merge regenerate households.id but carry household_uid.
        c.execute("DELETE FROM households WHERE id = ?1", params![hh])
            .unwrap();
        let gone = list_referrals_query(&c, None).unwrap();
        assert_eq!(gone[0].household_id, None);
        assert_eq!(gone[0].snapshot_name, "Smith, John & Mary");

        let hh2 = household(&c, "uid-1");
        let back = list_referrals_query(&c, Some(hh2)).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].id, r.id);
        assert_eq!(back[0].household_id, Some(hh2));
    }

    #[test]
    fn list_by_household_filters() {
        let c = conn();
        let a = household(&c, "uid-a");
        let b = household(&c, "uid-b");
        create_referral_query(&c, new(a, "other")).unwrap();
        create_referral_query(&c, new(b, "other")).unwrap();
        create_referral_query(&c, new(b, "deceased")).unwrap();
        assert_eq!(list_referrals_query(&c, Some(a)).unwrap().len(), 1);
        assert_eq!(list_referrals_query(&c, Some(b)).unwrap().len(), 2);
        assert_eq!(list_referrals_query(&c, None).unwrap().len(), 3);
    }

    #[test]
    fn mark_reported_only_touches_to_report() {
        let mut c = conn();
        let hh = household(&c, "uid-1");
        let r1 = create_referral_query(&c, new(hh, "other")).unwrap();
        let r2 = create_referral_query(&c, new(hh, "deceased")).unwrap();
        assert_eq!(
            mark_reported_query(&mut c, &[r1.id], "2026-11-01").unwrap(),
            1
        );
        // r1 again keeps its first-sent date; r2 is new.
        assert_eq!(
            mark_reported_query(&mut c, &[r1.id, r2.id], "2026-11-11").unwrap(),
            1
        );
        assert_eq!(
            get_referral(&c, r1.id).unwrap().reported_on.as_deref(),
            Some("2026-11-01")
        );
        assert_eq!(
            get_referral(&c, r2.id).unwrap().reported_on.as_deref(),
            Some("2026-11-11")
        );
        assert_eq!(get_referral(&c, r2.id).unwrap().status, "reported");
    }

    #[test]
    fn close_and_reopen() {
        let mut c = conn();
        let hh = household(&c, "uid-1");
        let sent = create_referral_query(&c, new(hh, "other")).unwrap();
        let unsent = create_referral_query(&c, new(hh, "deceased")).unwrap();
        mark_reported_query(&mut c, &[sent.id], "2026-11-01").unwrap();

        let done = close_referral_query(
            &c,
            sent.id,
            "confirmed",
            Some("clerk did it".into()),
            "2026-11-20",
        )
        .unwrap();
        assert_eq!(done.status, "confirmed");
        assert_eq!(done.resolved_on.as_deref(), Some("2026-11-20"));
        assert!(close_referral_query(&c, sent.id, "cancelled", None, "2026-11-21").is_err());
        assert!(update_referral_query(&c, sent.id, "other", None, None, "2026-11-01").is_err());
        assert!(close_referral_query(&c, unsent.id, "reported", None, "2026-11-21").is_err());

        // A to_report referral can be confirmed directly (the clerk heard first).
        let c2 = close_referral_query(&c, unsent.id, "cancelled", None, "2026-11-21").unwrap();
        assert_eq!(c2.status, "cancelled");

        assert_eq!(
            reopen_referral_query(&c, sent.id).unwrap().status,
            "reported"
        );
        let reopened = reopen_referral_query(&c, unsent.id).unwrap();
        assert_eq!(reopened.status, "to_report");
        assert_eq!(reopened.resolved_on, None);
        assert!(reopen_referral_query(&c, unsent.id).is_err());
    }

    #[test]
    fn update_edits_open_referral() {
        let c = conn();
        let hh = household(&c, "uid-1");
        let r = create_referral_query(&c, new(hh, "moved_known")).unwrap();
        let u = update_referral_query(
            &c,
            r.id,
            "contact_info",
            Some("phone disconnected".into()),
            Some("x".into()),
            "2026-11-09",
        )
        .unwrap();
        assert_eq!(u.kind, "contact_info");
        assert_eq!(u.new_address, None);
        assert_eq!(u.details.as_deref(), Some("phone disconnected"));
        assert_eq!(u.learned_on, "2026-11-09");
    }

    #[test]
    fn display_name_and_address_helpers() {
        assert_eq!(household_display_name("Ann", "Lee", None, None), "Lee, Ann");
        assert_eq!(
            household_display_name("Ann", "Lee", Some("Bo"), Some("Kim")),
            "Lee, Ann & Bo Kim"
        );
        assert_eq!(one_line_address(None, None, None, None, None), None);
        assert_eq!(
            one_line_address(Some("1 A St"), Some(""), Some("Town"), None, Some("12345"))
                .as_deref(),
            Some("1 A St, Town 12345")
        );
    }

    fn rec(line1: &str, city: Option<&str>) -> ParsedRecord {
        serde_json::from_value(serde_json::json!({
            "first_name": "John", "last_name": "Smith", "role": "head",
            "first_name_2": null, "last_name_2": null, "role_2": null,
            "phone_1": null, "email_1": null, "phone_2": null, "email_2": null,
            "address_line1": line1, "address_line2": null, "city": city,
            "state": "VA", "zip": null, "latitude": null, "longitude": null,
            "has_minors": false, "comments": null
        }))
        .unwrap()
    }

    #[test]
    fn address_matching() {
        let r = rec("45 Elm Rd", Some("Front Royal"));
        assert!(address_matches("45 Elm Rd, Front Royal VA 22630", &r));
        assert!(address_matches("45 elm rd. front royal", &r));
        assert!(!address_matches("45 Elm Road, Front Royal", &r));
        assert!(!address_matches("45 Elm Rd, Winchester VA", &r));
        assert!(!address_matches("145 Elm Rd, Front Royal", &r));
        assert!(address_matches("45 Elm Rd", &rec("45 Elm Rd", None)));
        assert!(!address_matches("45 Elm Rd", &rec("", None)));
    }

    #[test]
    fn review_effects_by_kind() {
        let c = conn();
        let hh = household(&c, "u-eff");
        for kind in ["contact_info", "deceased", "other", "moved_unknown"] {
            create_referral_query(&c, new(hh, kind)).unwrap();
        }
        let effects = |changed: &[&str]| -> Vec<String> {
            let changed: Vec<String> = changed.iter().map(|s| s.to_string()).collect();
            review_referrals(&c, "u-eff", "changed", None, &changed)
                .unwrap()
                .into_iter()
                .map(|r| format!("{}={}", r.kind, r.effect))
                .collect()
        };
        assert_eq!(
            effects(&["Phone/Email"]),
            [
                "contact_info=confirm",
                "deceased=none",
                "other=ask",
                "moved_unknown=none"
            ]
        );
        assert_eq!(
            effects(&["Name"]),
            [
                "contact_info=none",
                "deceased=ask",
                "other=ask",
                "moved_unknown=none"
            ]
        );
        assert_eq!(
            effects(&["Coordinates"]),
            [
                "contact_info=none",
                "deceased=none",
                "other=none",
                "moved_unknown=none"
            ]
        );
        let removed = review_referrals(&c, "u-eff", "removed", None, &[]).unwrap();
        assert!(removed.iter().all(|r| r.effect == "confirm"));
        assert_eq!(
            deletion_reason(&removed).as_deref(),
            Some(format!("Contact info — clerk referral {}", removed[0].reference).as_str())
        );
    }
}
