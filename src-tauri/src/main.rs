#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod crypto;
mod db;
mod geo;
mod keychain;
mod pdf_parser;

use std::path::PathBuf;
use tauri::Manager;

pub struct AppState {
    pub pool: db::Pool,
    pub db_path: PathBuf,
    pub live_key_hex: String,
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

            // #28: retention pruning runs unattended at startup now, not
            // as a side effect of Settings Save. run_prune() takes a
            // plain connection rather than State<AppState> specifically
            // so it can be called here, before app.manage() below exists.
            // Best-effort — a failure here must not block the app from
            // opening.
            {
                let conn = pool.get().expect("failed to get db connection for startup prune");
                match commands::settings::run_prune(&conn) {
                    Ok(result) => {
                        commands::logs::log(
                            &conn,
                            "info",
                            &format!(
                                "startup prune: removed {} deleted household(s), {} log row(s)",
                                result.deleted_households, result.logs
                            ),
                            None,
                        );
                    }
                    Err(e) => eprintln!("startup prune failed (non-fatal): {e}"),
                }
            }

            app.manage(AppState { pool, db_path, live_key_hex: key_hex });
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
            // settings / logs
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::prune_old_deleted_and_logs,
            commands::settings::preview_prune_impact,
            commands::logs::get_logs,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
