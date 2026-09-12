#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod crypto;
mod db;
mod geo;
mod keychain;
mod pdf_parser;
mod road_graph;

use std::path::PathBuf;
use tauri::Manager;

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

            let pool = db::open_pool(&db_path, &key_hex).expect("failed to open encrypted database");

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
            app.manage(AppState {
                pool,
                roads_pool,
                db_path,
                live_key_hex: key_hex,
                last_preview: std::sync::Mutex::new(None),
                road_graph_cache: std::sync::Arc::new(std::sync::Mutex::new(None)),
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
