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
            app.manage(AppState { pool, roads_pool, db_path, live_key_hex: key_hex, last_preview: std::sync::Mutex::new(None) });
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
