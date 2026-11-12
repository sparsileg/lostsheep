#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod crypto;
mod db;
mod geo;
mod keychain;
mod pdf_parser;
mod profiles;
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
    pub road_graph_cache:
        std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<road_graph::RoadGraph>>>>,
    // Issue #68: set by db::open_pool's update_hook on every real write to
    // any non-bookkeeping table (see the exclusion list there). Polled by
    // spawn_backup_reminder_thread below, which persists it to the
    // lastDbChangeAt setting (surviving app restart) and emits a
    // "backup-reminder" toast if 5 minutes pass with no backup since.
    pub last_db_change_at: std::sync::Arc<std::sync::Mutex<Option<chrono::DateTime<chrono::Utc>>>>,
    // Issue #85: the active profile's display name, if any, for the
    // window-title fix below — read once from RunEvent::Ready rather than
    // set directly inside setup(). Setting it inside setup() silently did
    // nothing: the window's own configured title (tauri.conf.json) is
    // applied when the window is actually shown, which happens after
    // setup() returns, and clobbered whatever this code set first.
    pub active_profile_name: Option<String>,
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
            let data_dir = app.path().app_data_dir().expect("no app data dir resolved");
            std::fs::create_dir_all(&data_dir).expect("could not create app data dir");

            // Issue #85: which pair of database files this launch opens
            // depends on the active profile in registry.json, if any. No
            // registry, or no active profile yet, resolves to the exact
            // same flat paths this app has always used — see
            // profiles::resolve_data_paths's own doc comment.
            let registry = profiles::load_registry(&data_dir);
            let (db_path, roads_db_path, active_profile_name) =
                profiles::resolve_data_paths(&data_dir, &registry)
                    .expect("could not resolve profile database paths");

            let key_hex = keychain::get_or_create_db_key().unwrap_or_else(|e| {
                eprintln!("FATAL: {e}");
                std::process::exit(1);
            });

            // Issue #68: shared with db::open_pool's update_hook (fires on
            // every write, sets this) and spawn_backup_reminder_thread
            // (polls it below). Seeded from the persisted lastDbChangeAt
            // setting right after the pool opens, so a change made just
            // before the app closed still starts its 5-minute countdown
            // from the right time rather than resetting to "no change".
            let last_db_change_at: std::sync::Arc<
                std::sync::Mutex<Option<chrono::DateTime<chrono::Utc>>>,
            > = std::sync::Arc::new(std::sync::Mutex::new(None));

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
            // (roads_db_path itself now comes from profiles::resolve_data_paths
            // above, per-profile — issue #85.)
            let roads_pool =
                db::open_roads_pool(&roads_db_path).expect("failed to open roads database");

            // #58: retention pruning no longer runs unattended here. An
            // unattended sweep with no prompt and no visible signal was
            // exactly what silently destroyed deleted households' visit
            // history after as little as a month. The frontend now calls
            // list_prune_candidates on launch, shows the user what would
            // be removed and how long ago it was deleted, and only calls
            // prune_old_deleted_and_logs if they confirm.
            spawn_backup_reminder_thread(
                app.handle().clone(),
                pool.clone(),
                last_db_change_at.clone(),
            );

            app.manage(AppState {
                pool,
                roads_pool,
                db_path,
                live_key_hex: key_hex,
                last_preview: std::sync::Mutex::new(None),
                road_graph_cache: std::sync::Arc::new(std::sync::Mutex::new(None)),
                last_db_change_at,
                active_profile_name,
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
            commands::households::set_contact_plan,
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
            commands::import::get_review_queue,
            commands::import::resolve_review_item,
            commands::import::commit_import_batch,
            commands::import::resolve_all_new_records,
            commands::import::get_pending_import_batch,
            commands::import::discard_import_batch,
            // visits / map
            commands::visits::record_visit,
            commands::visits::update_visit,
            commands::visits::delete_visit,
            commands::visits::get_visits_report,
            commands::visits::get_household_visits,
            commands::visits::get_needs_attention,
            commands::visits::get_coverage,
            commands::visits::get_contact_plans,
            commands::visits::generate_visit_list,
            commands::map_data::get_map_data,
            commands::map_data::get_missing_coords_count,
            // clerk referrals
            commands::referrals::list_referrals,
            commands::referrals::create_referral,
            commands::referrals::update_referral,
            commands::referrals::mark_referrals_reported,
            commands::referrals::close_referral,
            commands::referrals::reopen_referral,
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
            // profiles (#85)
            commands::profiles::list_profiles,
            commands::profiles::get_active_profile,
            commands::profiles::create_profile,
            commands::profiles::switch_profile,
            commands::profiles::restart_app,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, _event| {
            // Issue #85: window title. Confirmed by debug logging that
            // set_title() returns Ok(()) at RunEvent::Ready but the title
            // doesn't actually take — a WebKitGTK/tao timing quirk on this
            // platform (Ready fires before the window's first Resized/
            // Moved/Focused events, i.e. before it's genuinely realized on
            // screen), not a logic bug here. Rather than chase the exact
            // "right" lifecycle moment across platforms, this checks and
            // (re)applies on every event-loop tick — negligible cost (an
            // Option check and a string compare) once nothing needs
            // fixing, and self-corrects the moment the platform is
            // actually ready to accept it.
            //
            // try_state, not state: the very first tick(s) can fire before
            // setup() has finished calling app.manage() — state() panics
            // in that case ("state() called before manage()"), confirmed
            // by hitting that panic in practice. try_state just skips the
            // tick and the loop naturally retries on the next one.
            let Some(state) = app_handle.try_state::<AppState>() else {
                return;
            };
            if let Some(name) = &state.active_profile_name {
                let desired = format!("Lost Sheep - {name}");
                if let Some(window) = app_handle.get_webview_window("main") {
                    if window.title().map(|t| t != desired).unwrap_or(true) {
                        let _ = window.set_title(&desired);
                    }
                }
            }
        });
}

// Issue #69: exercises the household_uid/visit_uid plumbing end to end —
// Replace, soft-delete/restore, and the review-driven delete/restore path
// (Review Updates' "removed" -> Delete, the one actually reachable from the
// UI) — against a real, throwaway SQLCipher database rather than the app's
// own, so this never touches production data. Each test builds its own temp
// db/roads pair and deletes them (including WAL/SHM sidecars) on the way
// out. Needs tauri's "test" feature (Cargo.toml, [dev-dependencies]) to
// obtain a State<AppState> outside of a running app — these command fns all
// take State by value, so there's no lighter-weight way to call them
// directly.
#[cfg(test)]
mod uid_lifecycle_tests {
    use super::*;
    use crate::commands::households;
    use crate::commands::import;
    use rusqlite::OptionalExtension;
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "lostsheep_test_{}_{}",
            Uuid::new_v4().simple(),
            name
        ))
    }

    fn random_key_hex() -> String {
        format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
    }

    fn build_state() -> (AppState, PathBuf, PathBuf) {
        let db_path = temp_path("main.db");
        let roads_path = temp_path("roads.db");
        let key_hex = random_key_hex();
        let last_db_change_at = Arc::new(Mutex::new(None));
        let pool =
            db::open_pool(&db_path, &key_hex, last_db_change_at.clone()).expect("open_pool failed");
        let roads_pool = db::open_roads_pool(&roads_path).expect("open_roads_pool failed");
        let state = AppState {
            pool,
            roads_pool,
            db_path: db_path.clone(),
            live_key_hex: key_hex,
            last_preview: Mutex::new(None),
            road_graph_cache: Arc::new(Mutex::new(None)),
            last_db_change_at,
            active_profile_name: None,
        };
        (state, db_path, roads_path)
    }

    fn cleanup(paths: &[PathBuf]) {
        for p in paths {
            let _ = std::fs::remove_file(p);
            let _ = std::fs::remove_file(format!("{}-wal", p.display()));
            let _ = std::fs::remove_file(format!("{}-shm", p.display()));
        }
    }

    // Inserts one bare-minimum household directly — bypasses the PDF
    // parser/import diff entirely. This harness tests uid plumbing, not
    // import parsing.
    fn insert_test_household(pool: &db::Pool, source_key: &str) -> (i64, String) {
        let conn = pool.get().unwrap();
        let uid = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO households (household_uid, first_name, last_name, role, address_key, source_key, source_key_seq, has_minors) \
             VALUES (?1, 'Test', 'Household', 'head', 'test-addr', ?2, 0, 0)",
            rusqlite::params![uid, source_key],
        ).unwrap();
        (conn.last_insert_rowid(), uid)
    }

    fn insert_test_visit(pool: &db::Pool, household_id: i64) -> (i64, String) {
        let conn = pool.get().unwrap();
        let uid = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO visits (visit_uid, household_id, visit_date, comments) VALUES (?1, ?2, '2026-01-01', 'test visit')",
            rusqlite::params![uid, household_id],
        ).unwrap();
        (conn.last_insert_rowid(), uid)
    }

    fn get_household_uid(pool: &db::Pool, id: i64) -> String {
        pool.get()
            .unwrap()
            .query_row(
                "SELECT household_uid FROM households WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn soft_delete_then_restore_preserves_uids() {
        let (state, db_path, roads_path) = build_state();
        let (hh_id, hh_uid) = insert_test_household(&state.pool, "test-key-1");
        let (_, visit_uid) = insert_test_visit(&state.pool, hh_id);

        let app = tauri::test::mock_app();
        app.manage(state);
        let st: tauri::State<AppState> = app.state();

        households::soft_delete_household(st.clone(), hh_id, Some("test".into())).unwrap();

        {
            let conn = st.pool.get().unwrap();
            let gone: Option<i64> = conn
                .query_row(
                    "SELECT id FROM households WHERE id = ?1",
                    rusqlite::params![hh_id],
                    |r| r.get(0),
                )
                .optional()
                .unwrap();
            assert!(gone.is_none(), "household still present after soft delete");
        }

        let deleted_id: i64 = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT id FROM deleted_households WHERE original_id = ?1",
                rusqlite::params![hh_id],
                |r| r.get(0),
            )
            .unwrap();
        let mirrored_uid: Option<String> = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT household_uid FROM deleted_households WHERE id = ?1",
                rusqlite::params![deleted_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            mirrored_uid.as_deref(),
            Some(hh_uid.as_str()),
            "household_uid not mirrored on delete"
        );

        let mirrored_visit_uid: Option<String> = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT visit_uid FROM deleted_visits WHERE deleted_household_id = ?1",
                rusqlite::params![deleted_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            mirrored_visit_uid.as_deref(),
            Some(visit_uid.as_str()),
            "visit_uid not mirrored on delete"
        );

        households::restore_deleted_household(st.clone(), deleted_id).unwrap();

        let new_id: i64 = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT id FROM households WHERE source_key = 'test-key-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            get_household_uid(&st.pool, new_id),
            hh_uid,
            "household_uid changed across delete/restore"
        );

        let restored_visit_uid: String = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT visit_uid FROM visits WHERE household_id = ?1",
                rusqlite::params![new_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            restored_visit_uid, visit_uid,
            "visit_uid changed across delete/restore"
        );

        cleanup(&[db_path, roads_path]);
    }

    #[test]
    fn replace_carries_uid_forward() {
        let (state, db_path, roads_path) = build_state();
        let (hh_id, hh_uid) = insert_test_household(&state.pool, "test-key-2");

        let app = tauri::test::mock_app();
        app.manage(state);
        let st: tauri::State<AppState> = app.state();

        let incoming = serde_json::json!({
            "first_name": "Test", "last_name": "Household", "role": "head",
            "first_name_2": null, "last_name_2": null, "role_2": null,
            "phone_1": "555-0000", "email_1": null, "phone_2": null, "email_2": null,
            "address_line1": "123 New St", "address_line2": null, "city": null,
            "state": null, "zip": null, "latitude": null, "longitude": null,
            "has_minors": false, "comments": null
        });
        let batch_id: i64 = {
            let conn = st.pool.get().unwrap();
            conn.execute(
                "INSERT INTO import_batches (source_type, filename) VALUES ('csv', 'test.csv')",
                [],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let item_id: i64 = {
            let conn = st.pool.get().unwrap();
            conn.execute(
                "INSERT INTO review_queue (import_batch_id, match_type, incoming_data, existing_household_id) VALUES (?1, 'changed', ?2, ?3)",
                rusqlite::params![batch_id, incoming.to_string(), hh_id],
            ).unwrap();
            conn.last_insert_rowid()
        };

        import::resolve_review_item(st.clone(), item_id, "replace".to_string(), None, None, None)
            .unwrap();

        let new_id: i64 = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT id FROM households WHERE address_line1 = '123 New St'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            get_household_uid(&st.pool, new_id),
            hh_uid,
            "household_uid changed across Replace"
        );

        cleanup(&[db_path, roads_path]);
    }

    #[test]
    fn review_delete_then_restore_preserves_uids() {
        let (state, db_path, roads_path) = build_state();
        let (hh_id, hh_uid) = insert_test_household(&state.pool, "test-key-3");
        let (_, visit_uid) = insert_test_visit(&state.pool, hh_id);

        let app = tauri::test::mock_app();
        app.manage(state);
        let st: tauri::State<AppState> = app.state();

        // The path you actually use — Review Updates' "removed" -> Delete —
        // not households::soft_delete_household directly.
        let batch_id: i64 = {
            let conn = st.pool.get().unwrap();
            conn.execute(
                "INSERT INTO import_batches (source_type, filename) VALUES ('csv', 'test.csv')",
                [],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let item_id: i64 = {
            let conn = st.pool.get().unwrap();
            conn.execute(
                "INSERT INTO review_queue (import_batch_id, match_type, existing_household_id) VALUES (?1, 'removed', ?2)",
                rusqlite::params![batch_id, hh_id],
            ).unwrap();
            conn.last_insert_rowid()
        };

        import::resolve_review_item(
            st.clone(),
            item_id,
            "delete".to_string(),
            Some("test".into()),
            None,
            None,
        )
        .unwrap();

        let deleted_id: i64 = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT id FROM deleted_households WHERE original_id = ?1",
                rusqlite::params![hh_id],
                |r| r.get(0),
            )
            .unwrap();
        let mirrored_uid: Option<String> = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT household_uid FROM deleted_households WHERE id = ?1",
                rusqlite::params![deleted_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            mirrored_uid.as_deref(),
            Some(hh_uid.as_str()),
            "household_uid not mirrored on review-delete"
        );

        let mirrored_visit_uid: Option<String> = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT visit_uid FROM deleted_visits WHERE deleted_household_id = ?1",
                rusqlite::params![deleted_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            mirrored_visit_uid.as_deref(),
            Some(visit_uid.as_str()),
            "visit_uid not mirrored on review-delete"
        );

        households::restore_deleted_household(st.clone(), deleted_id).unwrap();

        let new_id: i64 = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT id FROM households WHERE source_key = 'test-key-3'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            get_household_uid(&st.pool, new_id),
            hh_uid,
            "household_uid changed across review-delete/restore"
        );

        cleanup(&[db_path, roads_path]);
    }

    // Issue #70: exercises the new "link" action — a 'new' item (auto-
    // matching failed at import time) manually linked by the user to a
    // same-batch 'removed' item. Confirms link runs the identical
    // merge-machinery guarantees (uid/comment/tag/visit carried forward,
    // old household gone) and that the paired 'removed' review_queue row
    // is auto-resolved rather than left dangling.
    #[test]
    fn link_carries_uid_and_history_forward() {
        let (state, db_path, roads_path) = build_state();
        let (hh_id, hh_uid) = insert_test_household(&state.pool, "test-key-4");
        let (_, visit_uid) = insert_test_visit(&state.pool, hh_id);

        let app = tauri::test::mock_app();
        app.manage(state);
        let st: tauri::State<AppState> = app.state();

        // Tag and comment on the outgoing household — both must carry
        // forward through link, same guarantee as replace/merge (#19/#20).
        let tag_id = {
            let conn = st.pool.get().unwrap();
            crate::commands::tags::get_or_create_tag_id(&conn, "LinkTestTag").unwrap()
        };
        {
            let conn = st.pool.get().unwrap();
            conn.execute(
                "INSERT INTO household_tags (household_id, tag_id) VALUES (?1, ?2)",
                rusqlite::params![hh_id, tag_id],
            )
            .unwrap();
            conn.execute(
                "UPDATE households SET comments = 'pre-link comment' WHERE id = ?1",
                rusqlite::params![hh_id],
            )
            .unwrap();
        }

        let incoming = serde_json::json!({
            "first_name": "Test", "last_name": "Household", "role": "head",
            "first_name_2": null, "last_name_2": null, "role_2": null,
            "phone_1": null, "email_1": null, "phone_2": null, "email_2": null,
            "address_line1": "456 Linked Ave", "address_line2": null, "city": null,
            "state": null, "zip": null, "latitude": null, "longitude": null,
            "has_minors": false, "comments": null
        });
        let batch_id: i64 = {
            let conn = st.pool.get().unwrap();
            conn.execute(
                "INSERT INTO import_batches (source_type, filename) VALUES ('csv', 'test.csv')",
                [],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        // The 'new' item — auto-matching failed at import time, which is
        // exactly the scenario #70 covers.
        let new_item_id: i64 = {
            let conn = st.pool.get().unwrap();
            conn.execute(
                "INSERT INTO review_queue (import_batch_id, match_type, incoming_data, existing_household_id) VALUES (?1, 'new', ?2, NULL)",
                rusqlite::params![batch_id, incoming.to_string()],
            ).unwrap();
            conn.last_insert_rowid()
        };
        // The paired 'removed' item pointing at the household this test
        // links against.
        let removed_item_id: i64 = {
            let conn = st.pool.get().unwrap();
            conn.execute(
                "INSERT INTO review_queue (import_batch_id, match_type, existing_household_id) VALUES (?1, 'removed', ?2)",
                rusqlite::params![batch_id, hh_id],
            ).unwrap();
            conn.last_insert_rowid()
        };

        import::resolve_review_item(
            st.clone(),
            new_item_id,
            "link".to_string(),
            None,
            Some(hh_id),
            None,
        )
        .unwrap();

        let new_id: i64 = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT id FROM households WHERE address_line1 = '456 Linked Ave'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            get_household_uid(&st.pool, new_id),
            hh_uid,
            "household_uid did not carry forward across link"
        );

        let carried_comment: Option<String> = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT comments FROM households WHERE id = ?1",
                rusqlite::params![new_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            carried_comment.as_deref(),
            Some("pre-link comment"),
            "comment not carried forward across link"
        );

        let carried_visit_uid: String = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT visit_uid FROM visits WHERE household_id = ?1",
                rusqlite::params![new_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            carried_visit_uid, visit_uid,
            "visit not re-pointed across link"
        );

        let carried_tag_count: i64 = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM household_tags WHERE household_id = ?1 AND tag_id = ?2",
                rusqlite::params![new_id, tag_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(carried_tag_count, 1, "tag not carried forward across link");

        let old_gone: Option<i64> = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT id FROM households WHERE id = ?1",
                rusqlite::params![hh_id],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert!(old_gone.is_none(), "old household still present after link");

        let removed_resolution: String = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT resolution FROM review_queue WHERE id = ?1",
                rusqlite::params![removed_item_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(
            removed_resolution, "pending",
            "paired 'removed' review item was not auto-resolved by link"
        );

        cleanup(&[db_path, roads_path]);
    }

    // ── Clerk referrals Stage 2 / Visit Reporting Stage 2 ───────────────

    fn new_batch(pool: &db::Pool, imported_at: &str, status: &str) -> i64 {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO import_batches (source_type, filename, imported_at, status) VALUES ('pdf', 't.pdf', ?1, ?2)",
            rusqlite::params![imported_at, status],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn queue_item(
        pool: &db::Pool,
        batch: i64,
        match_type: &str,
        incoming: Option<String>,
        hh: i64,
    ) -> i64 {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO review_queue (import_batch_id, match_type, incoming_data, existing_household_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![batch, match_type, incoming, hh],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn referral(
        pool: &db::Pool,
        hh: i64,
        kind: &str,
        new_address: Option<&str>,
        reported_on: Option<&str>,
    ) -> i64 {
        let mut conn = pool.get().unwrap();
        let r = commands::referrals::create_referral_query(
            &conn,
            commands::referrals::NewReferral {
                household_id: hh,
                kind: kind.into(),
                details: None,
                new_address: new_address.map(str::to_string),
                learned_on: "2026-01-01".into(),
                visit_uid: None,
            },
        )
        .unwrap();
        if let Some(on) = reported_on {
            commands::referrals::mark_reported_query(&mut conn, &[r.id], on).unwrap();
        }
        r.id
    }

    fn referral_row(pool: &db::Pool, id: i64) -> (String, Option<String>, Option<i64>) {
        pool.get()
            .unwrap()
            .query_row(
                "SELECT status, resolution_note, import_batch_id FROM referrals WHERE id = ?1",
                rusqlite::params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    }

    fn incoming_at(address: &str, city: &str) -> String {
        serde_json::json!({
            "first_name": "Test", "last_name": "Household", "role": "head",
            "first_name_2": null, "last_name_2": null, "role_2": null,
            "phone_1": null, "email_1": null, "phone_2": null, "email_2": null,
            "address_line1": address, "address_line2": null, "city": city,
            "state": "VA", "zip": null, "latitude": null, "longitude": null,
            "has_minors": false, "comments": null
        })
        .to_string()
    }

    #[test]
    fn review_delete_confirms_referrals_and_keeps_stage2_fields() {
        let (state, db_path, roads_path) = build_state();
        let (hh_id, _) = insert_test_household(&state.pool, "ref-key-1");
        {
            let conn = state.pool.get().unwrap();
            conn.execute(
                "UPDATE households SET contact_plan = 'monthly' WHERE id = ?1",
                rusqlite::params![hh_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO visits (visit_uid, household_id, visit_date, outcome, method, invited) \
                 VALUES ('v-card', ?1, '2026-02-01', 'sent', 'card', 1)",
                rusqlite::params![hh_id],
            )
            .unwrap();
        }
        let moved = referral(
            &state.pool,
            hh_id,
            "moved_unknown",
            None,
            Some("2026-02-01"),
        );
        let unsent = referral(&state.pool, hh_id, "contact_info", None, None);

        let app = tauri::test::mock_app();
        app.manage(state);
        let st: tauri::State<AppState> = app.state();

        let batch = new_batch(&st.pool, "2026-03-01T12:00:00.000Z", "pending");
        let item = queue_item(&st.pool, batch, "removed", None, hh_id);

        let queue = import::get_review_queue(st.clone(), batch).unwrap();
        let effects: Vec<&str> = queue[0]
            .referrals
            .iter()
            .map(|r| r.effect.as_str())
            .collect();
        assert_eq!(effects, ["confirm", "confirm"]);

        let confirmed =
            import::resolve_review_item(st.clone(), item, "delete".into(), None, None, None)
                .unwrap();
        assert_eq!(confirmed.len(), 2);

        let (status, note, batch_id) = referral_row(&st.pool, moved);
        assert_eq!(status, "confirmed");
        assert_eq!(
            note.as_deref(),
            Some("Removed from directory, import of 2026-03-01")
        );
        assert_eq!(batch_id, Some(batch));
        assert_eq!(
            referral_row(&st.pool, unsent).0,
            "confirmed",
            "to_report may be confirmed too"
        );

        let conn = st.pool.get().unwrap();
        let (reason, plan, deleted_id): (Option<String>, Option<String>, i64) = conn
            .query_row(
                "SELECT deletion_reason, contact_plan, id FROM deleted_households WHERE original_id = ?1",
                rusqlite::params![hh_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            reason.as_deref(),
            Some(format!("Moved — clerk referral R-{moved:04}").as_str())
        );
        assert_eq!(plan.as_deref(), Some("monthly"));
        let (outcome, method, invited): (String, String, bool) = conn
            .query_row(
                "SELECT outcome, method, invited FROM deleted_visits WHERE deleted_household_id = ?1",
                rusqlite::params![deleted_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (outcome.as_str(), method.as_str(), invited),
            ("sent", "card", true)
        );
        drop(conn);

        let summary = import::commit_import_batch(st.clone(), batch).unwrap();
        assert_eq!(summary.referrals_confirmed, 2);

        // Restore brings the plan and the visit's kind back.
        households::restore_deleted_household(st.clone(), deleted_id).unwrap();
        let conn = st.pool.get().unwrap();
        let (plan, method): (Option<String>, String) = conn
            .query_row(
                "SELECT h.contact_plan, v.method FROM households h JOIN visits v ON v.household_id = h.id \
                 WHERE h.source_key = 'ref-key-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (plan.as_deref(), method.as_str()),
            (Some("monthly"), "card")
        );
        drop(conn);

        cleanup(&[db_path, roads_path]);
    }

    #[test]
    fn replace_confirms_matching_move_and_asks_on_mismatch() {
        let (state, db_path, roads_path) = build_state();
        let (a_id, _) = insert_test_household(&state.pool, "ref-key-a");
        let (b_id, _) = insert_test_household(&state.pool, "ref-key-b");
        {
            let conn = state.pool.get().unwrap();
            conn.execute(
                "UPDATE households SET contact_plan = 'quarterly' WHERE id = ?1",
                rusqlite::params![a_id],
            )
            .unwrap();
        }
        let a_ref = referral(
            &state.pool,
            a_id,
            "moved_known",
            Some("45 Elm Rd, Front Royal VA 22630"),
            Some("2026-02-01"),
        );
        let b_ref = referral(
            &state.pool,
            b_id,
            "moved_known",
            Some("45 Elm Rd, Front Royal VA"),
            Some("2026-02-01"),
        );
        let b_other = referral(&state.pool, b_id, "deceased", None, Some("2026-02-01"));

        let app = tauri::test::mock_app();
        app.manage(state);
        let st: tauri::State<AppState> = app.state();
        let batch = new_batch(&st.pool, "2026-03-01T12:00:00.000Z", "pending");
        let a_item = queue_item(
            &st.pool,
            batch,
            "changed",
            Some(incoming_at("45 Elm Road", "Front Royal")),
            a_id,
        );
        let b_item = queue_item(
            &st.pool,
            batch,
            "changed",
            Some(incoming_at("9 Pine Ct", "Winchester")),
            b_id,
        );

        let queue = import::get_review_queue(st.clone(), batch).unwrap();
        let effect = |item: i64, id: i64| -> String {
            queue
                .iter()
                .find(|q| q.id == item)
                .unwrap()
                .referrals
                .iter()
                .find(|r| r.id == id)
                .unwrap()
                .effect
                .clone()
        };
        // "Road" vs "Rd" is a miss: asked, not auto-confirmed.
        assert_eq!(effect(a_item, a_ref), "ask");
        assert_eq!(effect(b_item, b_ref), "ask");
        assert_eq!(effect(b_item, b_other), "none");

        // The user confirms A's anyway; B is left open.
        let done = import::resolve_review_item(
            st.clone(),
            a_item,
            "replace".into(),
            None,
            None,
            Some(vec![a_ref]),
        )
        .unwrap();
        assert_eq!(done, vec![format!("R-{a_ref:04}")]);
        assert_eq!(referral_row(&st.pool, a_ref).0, "confirmed");
        let done =
            import::resolve_review_item(st.clone(), b_item, "replace".into(), None, None, None)
                .unwrap();
        assert!(done.is_empty());
        assert_eq!(referral_row(&st.pool, b_ref).0, "reported");
        assert_eq!(referral_row(&st.pool, b_other).0, "reported");

        // The contact plan rode through Replace.
        let plan: Option<String> = st
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT contact_plan FROM households WHERE address_line1 = '45 Elm Road'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(plan.as_deref(), Some("quarterly"));

        cleanup(&[db_path, roads_path]);
    }

    #[test]
    fn exact_new_address_auto_confirms() {
        let (state, db_path, roads_path) = build_state();
        let (id, _) = insert_test_household(&state.pool, "ref-key-c");
        let r = referral(
            &state.pool,
            id,
            "moved_known",
            Some("45 Elm Rd, Front Royal VA 22630"),
            Some("2026-02-01"),
        );
        let app = tauri::test::mock_app();
        app.manage(state);
        let st: tauri::State<AppState> = app.state();
        let batch = new_batch(&st.pool, "2026-03-01T12:00:00.000Z", "pending");
        let item = queue_item(
            &st.pool,
            batch,
            "changed",
            Some(incoming_at("45 Elm Rd", "Front Royal")),
            id,
        );
        let done =
            import::resolve_review_item(st.clone(), item, "replace".into(), None, None, None)
                .unwrap();
        assert_eq!(done.len(), 1);
        let (status, note, _) = referral_row(&st.pool, r);
        assert_eq!(status, "confirmed");
        assert_eq!(
            note.as_deref(),
            Some("Address updated, import of 2026-03-01")
        );
        cleanup(&[db_path, roads_path]);
    }

    #[test]
    fn overdue_counts_only_completed_imports_after_reporting() {
        let (state, db_path, roads_path) = build_state();
        let (id, _) = insert_test_household(&state.pool, "ref-key-d");
        let r = referral(&state.pool, id, "moved_unknown", None, Some("2026-02-01"));
        let pool = &state.pool;
        new_batch(pool, "2026-01-20T12:00:00.000Z", "committed"); // before reporting
        new_batch(pool, "2026-02-08T12:00:00.000Z", "committed");
        new_batch(pool, "2026-02-10T12:00:00.000Z", "discarded");
        let open = new_batch(pool, "2026-02-12T12:00:00.000Z", "pending");
        queue_item(pool, open, "new", Some(incoming_at("1 A St", "X")), id);

        let get = || {
            commands::referrals::list_referrals_query(&pool.get().unwrap(), None)
                .unwrap()
                .into_iter()
                .find(|x| x.id == r)
                .unwrap()
        };
        let x = get();
        assert_eq!((x.imports_since, x.overdue), (1, false));

        // An import with nothing to review, finished: the second one.
        new_batch(pool, "2026-02-15T12:00:00.000Z", "pending");
        let x = get();
        assert_eq!((x.imports_since, x.overdue), (2, true));
        assert_eq!(x.last_import_on.as_deref(), Some("2026-02-15"));

        // The threshold is a setting.
        pool.get()
            .unwrap()
            .execute(
                "UPDATE settings SET value = '3' WHERE key = 'referralOverdueImports'",
                [],
            )
            .unwrap();
        assert!(!get().overdue);
        cleanup(&[db_path, roads_path]);
    }
}
