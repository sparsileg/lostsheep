#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod crypto;
mod db;
mod geo;
mod keychain;
mod pdf_parser;
mod road_graph;

use std::path::PathBuf;
use tauri::{Emitter, Manager};

pub struct AppState {
    pub pool: db::Pool,
    pub roads_pool: db::Pool,
    pub db_path: PathBuf,
    pub live_key_hex: String,
    // Issue #26: restore_commit requires a token minted by restore_preview
    // for the exact same file, so the required before/after screen is an
    // enforced step, not just a UI convention. (src_path, token) — cleared
    // (single-use) on every commit attempt, matched or not.
    pub last_preview: std::sync::Mutex<Option<(String, String)>>,
    // Issue #66: generate_visit_list used to rebuild the entire in-memory
    // RoadGraph from roads.db on every single call — the single biggest
    // cost in that command by a wide margin (measured ~1.8s of a ~3.6s
    // total on a real county-sized extract), even though the graph only
    // changes when ingest_road_database() runs. Cached here instead;
    // ingest_road_database() clears it back to None on a successful
    // re-ingest (see roads.rs), so a stale graph can never be served.
    // Arc, not RoadGraph directly, so a cache hit is a cheap refcount
    // bump under the lock rather than cloning every node/edge/adjacency
    // entry — RoadGraph doesn't derive Clone, and shouldn't need to.
    // Outer Arc (issue #66 follow-up): diagnostics.rs's
    // find_potential_problems shares this same cache but runs inside a
    // spawn_blocking closure that can't hold a borrowed State<AppState>
    // (not 'static) — same reason pool/roads_pool are cloned out before
    // that closure today. Cloning the outer Arc is what makes the cache
    // itself movable into that closure the same way.
    pub road_graph_cache: std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<road_graph::RoadGraph>>>>,
    // Issue #68: set by db::open_pool's update_hook on every real write to
    // any non-bookkeeping table (see the exclusion list there). Polled by
    // spawn_backup_reminder_thread below, which persists it to the
    // lastDbChangeAt setting (surviving app restart) and emits a
    // "backup-reminder" toast if 5 minutes pass with no backup since.
    pub last_db_change_at: std::sync::Arc<std::sync::Mutex<Option<chrono::DateTime<chrono::Utc>>>>,
}

const BACKUP_REMINDER_POLL: std::time::Duration = std::time::Duration::from_secs(15);
const BACKUP_REMINDER_THRESHOLD: chrono::Duration = chrono::Duration::minutes(5);

// Issue #68: background reminder loop. Plain OS thread (not async — nothing
// else in this codebase runs a Tokio runtime), polling every 15s.
//   - Persists the in-memory dirty timestamp to the lastDbChangeAt setting
//     so the 5-minute window survives an app restart (main() reseeds
//     last_db_change_at from this same setting on startup).
//   - Emits "backup-reminder" once per dirty-timestamp value, only if no
//     backup has been recorded (lastBackupAt) since that change.
fn spawn_backup_reminder_thread(
    app_handle: tauri::AppHandle,
    pool: db::Pool,
    last_db_change_at: std::sync::Arc<std::sync::Mutex<Option<chrono::DateTime<chrono::Utc>>>>,
) {
    std::thread::spawn(move || {
        let mut last_persisted: Option<chrono::DateTime<chrono::Utc>> = None;
        let mut toasted_for: Option<chrono::DateTime<chrono::Utc>> = None;
        loop {
            std::thread::sleep(BACKUP_REMINDER_POLL);

            let dirty_at = match last_db_change_at.lock() {
                Ok(guard) => *guard,
                Err(_) => None,
            };
            let Some(dirty_at) = dirty_at else { continue };

            if last_persisted != Some(dirty_at) {
                if let Ok(conn) = pool.get() {
                    let iso = dirty_at.to_rfc3339();
                    let _ = conn.execute(
                        "INSERT INTO settings (key, value) VALUES ('lastDbChangeAt', ?1) \
                         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                        rusqlite::params![iso],
                    );
                }
                last_persisted = Some(dirty_at);
            }

            if toasted_for == Some(dirty_at) {
                continue;
            }

            let last_backup_at: Option<String> = pool.get().ok().and_then(|conn| {
                conn.query_row(
                    "SELECT value FROM settings WHERE key = 'lastBackupAt'",
                    [],
                    |r| r.get(0),
                )
                .ok()
            });
            let backed_up_since_change = last_backup_at
                .as_deref()
                .filter(|s| !s.is_empty())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.with_timezone(&chrono::Utc) >= dirty_at)
                .unwrap_or(false);
            if backed_up_since_change {
                continue;
            }

            if chrono::Utc::now() - dirty_at >= BACKUP_REMINDER_THRESHOLD {
                let _ = app_handle.emit("backup-reminder", ());
                toasted_for = Some(dirty_at);
            }
        }
    });
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("no app data dir resolved");
            std::fs::create_dir_all(&data_dir).expect("could not create app data dir");
            let db_path = data_dir.join("lost-sheep.db");

            let key_hex = keychain::get_or_create_db_key()
                .unwrap_or_else(|e| {
                    eprintln!("FATAL: {e}");
                    std::process::exit(1);
                });

            // Issue #68: shared with db::open_pool's update_hook (fires on
            // every write, sets this) and spawn_backup_reminder_thread
            // (polls it below). Seeded from the persisted lastDbChangeAt
            // setting right after the pool opens, so a change made just
            // before the app closed still starts its 5-minute countdown
            // from the right time rather than resetting to "no change".
            let last_db_change_at: std::sync::Arc<std::sync::Mutex<Option<chrono::DateTime<chrono::Utc>>>> =
                std::sync::Arc::new(std::sync::Mutex::new(None));

            let pool = db::open_pool(&db_path, &key_hex, last_db_change_at.clone())
                .expect("failed to open encrypted database");

            if let Ok(conn) = pool.get() {
                let saved: Option<String> = conn
                    .query_row(
                        "SELECT value FROM settings WHERE key = 'lastDbChangeAt'",
                        [],
                        |r| r.get(0),
                    )
                    .ok();
                if let Some(iso) = saved.filter(|s| !s.is_empty()) {
                    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(&iso) {
                        if let Ok(mut guard) = last_db_change_at.lock() {
                            *guard = Some(t.with_timezone(&chrono::Utc));
                        }
                    }
                }
            }

            // Issue #76: prime the write-time log-level filter from
            // whatever's already stored, before any other command (or
            // this same setup() function, further down) can call
            // logs::log(). Best-effort — if this fails (fresh database,
            // no logLevel saved yet) the cache keeps its "log everything"
            // default, same as the crate-level doc comment on
            // MIN_LOG_LEVEL_ORDINAL describes.
            if let Ok(conn) = pool.get() {
                commands::logs::refresh_min_level_cache(&conn);
            }

            // Issue #39: road graph lives in its own plain (unencrypted)
            // SQLite file, alongside the main DB. Never touched by
            // restore (#25/#26) — that's the whole point of the split.
            let roads_db_path = data_dir.join("roads.db");
            let roads_pool = db::open_roads_pool(&roads_db_path).expect("failed to open roads database");

            // #58: retention pruning no longer runs unattended here. An
            // unattended sweep with no prompt and no visible signal was
            // exactly what silently destroyed deleted households' visit
            // history after as little as a month. The frontend now calls
            // list_prune_candidates on launch, shows the user what would
            // be removed and how long ago it was deleted, and only calls
            // prune_old_deleted_and_logs if they confirm.
            spawn_backup_reminder_thread(app.handle().clone(), pool.clone(), last_db_change_at.clone());

            app.manage(AppState {
                pool,
                roads_pool,
                db_path,
                live_key_hex: key_hex,
                last_preview: std::sync::Mutex::new(None),
                road_graph_cache: std::sync::Arc::new(std::sync::Mutex::new(None)),
                last_db_change_at,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // households
            commands::households::search_households,
            commands::households::get_household,
            commands::households::update_household_comments,
            commands::households::soft_delete_household,
            commands::households::list_deleted_households,
            commands::households::restore_deleted_household,
            // tags
            commands::tags::list_tags,
            commands::tags::create_tag,
            commands::tags::rename_tag,
            commands::tags::delete_tag,
            commands::tags::tag_households,
            commands::tags::untag_household,
            commands::tags::bulk_tag_search_results,
            // import
            commands::import::import_pdf,
            commands::import::import_csv,
            commands::import::get_review_queue,
            commands::import::resolve_review_item,
            commands::import::commit_import_batch,
            commands::import::resolve_all_new_records,
            commands::import::get_pending_import_batch,
            commands::import::discard_import_batch,
            // visits / map
            commands::visits::record_visit,
            commands::visits::get_visits_report,
            commands::visits::get_household_visits,
            commands::visits::generate_visit_list,
            commands::map_data::get_map_data,
            commands::map_data::get_missing_coords_count,
            // backup / restore
            commands::backup::backup_database,
            commands::backup::restore_preview,
            commands::backup::restore_commit,
            // roads
            commands::roads::ingest_road_database,
            commands::roads::get_roads_in_bounds,
            commands::roads::get_nearest_road_node,
            // diagnostics
            commands::diagnostics::find_potential_problems,
            // map tiles (#72)
            commands::tiles::get_cached_tile,
            commands::tiles::save_cached_tile,
            commands::tiles::get_tile_cache_status,
            commands::tiles::clear_tile_cache,
            // settings / logs
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::prune_old_deleted_and_logs,
            commands::settings::preview_prune_impact,
            commands::settings::list_prune_candidates,
            commands::logs::get_logs,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
