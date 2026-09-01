# Lost Sheep — Architecture & Build Doc

## 1. Architecture

Tauri 1.x app. Rust backend, vanilla JS/HTML/CSS frontend. No web server,
no browser runtime — single native binary per OS.

```
Frontend (webview)  <-- invoke() -->  Rust backend  <-->  SQLCipher SQLite file
        |                                  |                      ^
        |                                  |                      |
    Leaflet map                    OS keychain (DB key)    argon2-derived key
   (OSM tiles + cached                                      (backup files only)
    polygon regions)
```

- Frontend never touches the filesystem or DB directly — every read/write
  goes through a Tauri command (see §4). This keeps SQLCipher key handling,
  validation, and logging in one trusted place.
- DB key lives in the OS keychain, fetched once at startup
  (`keychain::get_or_create_db_key`), silently, no unlock password shown
  to the user. If the keychain is unavailable, the app fails fast with a
  message pointing at Restore (see Help modal).
- Backups are a *separate* re-keyed copy of the DB (SQLCipher
  `sqlcipher_export`), keyed from a user passphrase via Argon2id — fully
  portable, independent of this machine's keychain.

## 2. File Structure

```
lost-sheep/
  src-tauri/                     Rust backend
    Cargo.toml
    tauri.conf.json
    src/
      main.rs                    AppState, command registration, bootstrap
      db/
        mod.rs                   pool, key application, rekey_copy
        schema.sql                11-table schema, applied on first run
      commands/
        households.rs            search/get/update/soft-delete
        tags.rs                  CRUD + tag/untag + bulk-tag-search-results
        import.rs                PDF/CSV parse -> diff -> review_queue -> commit
        visits.rs                record_visit, report, generate_visit_list (haversine)
        backup.rs                backup_database, restore_preview, restore_commit
        map_cache.rs             cache_regions CRUD, get_map_data
        settings.rs              get/save settings, retention pruning
        logs.rs                  write helper + get_logs
      pdf_parser.rs               PDF -> ParsedRecord, address/source key normalization
      keychain.rs                 OS keychain wrapper
      crypto.rs                   random key gen, Argon2id derivation
      geo.rs                      haversine distance

  src/                           Frontend (Tauri distDir)
    index.html                   shell: sidebar + view containers, script load order
    css/
      base.css                   structure only — reads CSS vars
      themes/{nordic,dark,light,matrix,flat}.css   tokens only (from existing project files)
    js/
      api.js                     ONE wrapper around every invoke() call
      core.js                    router (showView), message toasts, view registry
      sidebar.js                 theme/font-size/hamburger chrome
      tag-chip-input.js          reusable tag-entry widget w/ type-ahead
      views/
        dashboard-view.js/.css
        import-view.js/.css
        review-view.js/.css
        households-view.js/.css
        tags-view.js/.css
        map-view.js/.css         Leaflet, seed selection, visit-list gen, cache draw
        settings-view.js/.css
        log-viewer.js/.css       native rewrite of the reference Svelte component
        backup-restore.js/.css
```

Each view is a self-registering module (`registerView(name, {init, onShow})`)
plus its own CSS file — matches the "each major view/operation has its own
CSS file, no inline CSS" requirement.

## 3. Database Schema

See `src-tauri/src/db/schema.sql` for full DDL. Summary:

| Table | Purpose |
|---|---|
| `households` | One row per person-record (multi-head = multiple rows, same `address_key`) |
| `deleted_households` | Soft-delete mirror, retained per `settings.deletedRetentionDays` |
| `tags`, `household_tags` | Tag catalog + many-to-many |
| `visits` | Visit/attempt records, date + comments |
| `import_batches`, `review_queue` | Import diff staging, user-resolved before commit |
| `visit_groups`, `visit_group_members` | Named tag+seed groups (optional persistence layer over ad-hoc generation) |
| `cache_regions` | User-drawn polygon regions for offline map tiles |
| `settings` | Key/value app settings |
| `logs` | Multi-level app log, read by the Log Viewer |

Key design points:
- `address_key` (normalized address) is how same-address multi-head
  records are grouped for visit-list generation — never a foreign key to
  a separate "household" table, per the settled decision to keep
  multi-head people as flat, independent rows.
- `source_key` (normalized name+address) is the import dedupe key — exact
  matches are skipped; everything else becomes a `review_queue` row.

## 4. API (Tauri Commands)

All defined in `src-tauri/src/commands/*.rs`, registered in `main.rs`,
called from `src/js/api.js`.

**Households** — `search_households`, `get_household`, `update_household`, `soft_delete_household`
**Tags** — `list_tags`, `create_tag`, `rename_tag`, `delete_tag`, `tag_households`, `untag_household`, `bulk_tag_search_results`
**Import** — `import_pdf`, `import_csv`, `get_review_queue`, `resolve_review_item`, `commit_import_batch`
**Visits/Map** — `record_visit`, `get_visits_report`, `generate_visit_list`, `get_map_data`, `list_cache_regions`, `save_cache_region`, `delete_cache_region`
**Backup/Restore** — `backup_database`, `restore_preview`, `restore_commit`
**Settings/Logs** — `get_settings`, `save_settings`, `prune_old_deleted_and_logs`, `get_logs`

## 5. UI Architecture

- Single-page shell (`index.html`): fixed sidebar (nav, theme, font size,
  hamburger menu) + one `<main>` with one `<section class="view">` per
  screen, toggled by `core.js`'s `showView()`.
- Each view module owns its own DOM subtree, its own CSS file, and talks
  to the backend only through `api.js`.
- Modals (household edit, backup, restore, help) are plain DOM overlays
  appended to `<body>`, no framework.
- Map view holds live Leaflet state (`MapView.map`, `.markersLayer`) for
  seed selection and polygon-drawing for offline cache regions.

## 6. What's Scaffolded vs. What Needs Real Input

This is a working **shape** of the full app — every screen, every table,
every command is wired end-to-end — but two things are still stubs
pending real reference material, exactly as flagged in the original
requirements doc:

1. **`pdf_parser.rs`'s block grammar** — built against a reasonable guess
   at directory-PDF structure (name lines + address line + "Lat/Long"
   trailer per blank-line-separated block). Swap in real parsing logic
   once a real sample PDF is available; the diff/review/commit pipeline
   downstream of it doesn't change.
2. **Offline tile caching** — `map-view.js`'s polygon draw records the
   region in the DB (`cache_regions`) but doesn't yet compute the tile
   x/y/z set and write files to disk; that's a `window.__TAURI__.fs` call
   to add against the app data dir once tile math is wired in.

Everything else (schema, commands, all 9 views, theming, backup/restore
with before/after diff, tagging, log viewer, nearest-N visit-list
generation grouped by address) is real, runnable code, not placeholders.

## 7. Build Order (for local setup)

```
cd lost-sheep/src-tauri
cargo tauri dev        # first run: creates DB, generates SQLCipher key in OS keychain
```

Then, in order of dependency:
1. Confirm schema against a real sample PDF's actual layout — adjust
   `pdf_parser.rs` if the block grammar doesn't match.
2. Import a real directory, walk Review Updates end-to-end.
3. Tag a handful of households, generate a visit list on the Map view.
4. Run a Backup, then a Restore against a copy of the DB to confirm the
   before/after diff looks right.
5. `cargo tauri build` for installers (deb/rpm/dmg/msi/nsis already
   configured in `tauri.conf.json`).
