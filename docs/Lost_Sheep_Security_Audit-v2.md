# Lost Sheep — Security, Correctness and Reliability Audit

Scope: `src/` (HTML/CSS/JS frontend) and `src-tauri/` (Rust backend) as
supplied. 74 files, ~8,700 lines of first-party code, plus vendored Leaflet,
pdfmake and the Tauri API shims.

Audit date: 2026-09-11. Independent review — no prior findings, notes or
conversation history were used as input.

---

## Scope correction: the actual threat model

Lost Sheep is a single-user, local-first Tauri 2 desktop application. There is
no server, no HTTP listener, no authentication layer, no user table, no
tenancy, no session, and no client/server trust boundary. The frontend and
backend run in one process on one machine, communicating over Tauri's IPC
channel, which is not reachable from the network. `main.rs` registers 34
commands via `generate_handler!`; every one of them operates on the single
local database belonging to the single local user.

Per your instruction, I modelled against the real threat surface and did not
manufacture findings for the categories that do not apply. 

What I audited instead, as the genuine attack and failure surface:

- **Hostile or malformed file input** — the directory PDF, CSV imports, the
  OSM `.pbf` road extract, and backup archives. These are the only externally
  authored bytes the application consumes.
- **Backup and restore** — file path handling, key derivation, archive
  unpacking, the on-disk swap, and what survives it.
- **Key and secret management** — SQLCipher key lifecycle, OS keychain use,
  Argon2id parameters, passphrase handling.
- **The Tauri IPC surface and CSP** — what the webview can reach, what a
  compromised or buggy frontend could do.
- **Path traversal and unsafe file access** — every command taking a
  filesystem path from the frontend.
- **Resource exhaustion** — unbounded reads, unbounded allocation, quadratic
  algorithms.
- **Privacy and data disclosure** — where congregant PII leaves the encrypted
  store.
- **Correctness**: logic errors, state inconsistency, transaction boundaries,
  concurrency, error propagation, and above all **data-loss paths**.

For a privacy-sensitive local application maintained by one person for one
non-technical end user, data loss is the dominant risk class. That is where the
serious findings are.

---

## Executive Summary

### Overall security rating: 7 / 10

This is a defensively-written codebase. The intrusion-oriented categories are
in good shape and several were clearly hardened deliberately in earlier work.
The rating is held back by privacy leakage through export paths rather than by
any injection or memory-safety weakness.

What is genuinely well done is listed in full under *Positive Findings*, but in
brief: no SQL injection, no XSS, no unsafe Rust, no browser storage, no `eval`,
a CSP with no `unsafe-inline` in `script-src`, canonicalized path checks,
Argon2id with per-backup salts, size-capped archive entries, and a single-use
token gating the destructive half of restore.

### Top five risks

1. **Backup is completely non-functional on any fresh installation** (C-1).
   `strip_road_graph()` still issues `DELETE FROM road_edges` against a schema
   that stopped creating that table at issue #39. The call site uses a hard
   `?`, so `backup_database` returns an error before writing anything. A
   database created before #39 kept the now-empty tables and is unaffected —
   which means the development machine works and every new install does not.
   The user is left with no backups and no explanation.

2. **Restore silently discards every visit and comment recorded since the
   backup was taken** (R-1). `restore_commit` renames the re-keyed backup over
   the live database. `restore_preview` counts households and tags and never
   queries `visits` or compares `households.comments`, so the confirmation
   screen cannot show what is actually at risk. No pre-restore copy is kept.

3. **Retention pruning permanently destroys deleted households and their
   mirrored visit history within 30 days** (C-2). `schema.sql` seeds
   `deletedRetentionDays = '365'`; `ALLOWED_RETENTION_DAYS` is `[1, 7, 14, 30]`;
   `run_prune()` filters the stored value against that array and falls back to
   
   30. The sweep runs unattended in `main.rs`'s `setup()` before the window
       paints. `deleted_visits` cascades away with the parent row.

4. **The Known / Not Known button silently removes a "Do not contact" tag**
   (C-3). `tag_households()` opens with an unconditional `DELETE FROM
   household_tags WHERE household_id = ?1` and has no exemption for
   `system_key` tags — the one tag `delete_tag()` refuses to delete and
   `fetch_grouped_households()` filters on. One click on a list row re-exposes
   that household to visit lists.

5. **Household comments and visit records cannot survive the loss of the
   database** (R-3, R-4). They exist nowhere but inside `lost-sheep.db` and
   encrypted copies of it, there is no data-level export of either, and no
   value in the schema is both stable and unique enough to reattach them to a
   rebuilt database. Covered in full below.

### Most likely attack vectors

Ranked by realistic probability for this deployment, not by theoretical
severity:

1. **A wrong or oversized file chosen in a picker.** The `.pbf` ingest reads an
   entire OSM extract into memory with no size check (C-10); the file the user
   downloads before clipping sits next to the clipped one with the same
   extension. Result is an OOM kill with no message and no log entry.
2. **A malformed or unexpected directory PDF.** The parser drops any household
   whose surname begins with an accented character, silently, and corrupts its
   neighbour's `has_minors` flag while doing so (C-13).
3. **An untrusted backup archive.** Well defended — entry count, entry names
   and entry sizes are all validated before anything is read (#25) — but the
   re-keyed file is renamed directly over the live database with no retained
   copy of what it replaced.
4. **Local disclosure through export artifacts.** PDF exports write the full
   congregation directory unencrypted to the download folder (S-1); map tile
   requests disclose the congregation's geographic footprint to a third party
   on every pan and zoom (S-2).
5. **Physical or local access.** The SQLCipher key lives in the OS keychain,
   unlocked whenever the desktop session is. This is the documented, accepted
   architecture — noted for completeness, not as a finding.

### Most severe vulnerabilities

There are no remote-exploitable vulnerabilities, no injection vectors and no
memory-safety defects. The severe findings are all availability and integrity
of the user's own data:

- **C-1** — backup unavailable on fresh installs (High).
- **R-1** — irreversible silent loss of visits and comments on restore (High).
- **C-2** — irreversible silent loss of deleted households' history (High).
- **C-3** — safety-exclusion bypass via an ordinary UI control (High).
- **R-3 / R-4** — no recovery path for the only user-authored data (High,
  structural).

---

## Detailed Findings

Full write-ups are in the accompanying issue files, one per finding, in
`TEMPLATE_ISSUE.md` format with severity, location, trace, attack or failure
scenario, impact, and recommended approach. Designations are local to this
report — assign GitHub numbers on insertion.

Findings are Medium and above, per your instruction. Confidence is High for all
listed items unless marked otherwise; every one was traced to specific lines
rather than inferred.

### High severity

| ID      | Finding                                                                  | Location                                                                  |
| ------- | ------------------------------------------------------------------------ | ------------------------------------------------------------------------- |
| ~~C-1~~ | Backup fails outright on any database created after issue #39            | `commands/backup.rs::strip_road_g~~~~raph`                                |
| R-1     | Restore silently discards every visit and comment since the backup       | `commands/backup.rs::restore_preview` / `restore_commit`                  |
| C-2     | Retention prune destroys deleted households and their visits in ≤30 days | `commands/settings.rs::run_prune`, `db/schema.sql`                        |
| C-3     | Known / Not Known button silently clears "Do not contact"                | `commands/tags.rs::tag_households`, `views/households-view.js::markKnown` |
| R-3     | No durable household identity                                            | `db/schema.sql`, `commands/import.rs::resolve_review_item`                |
| R-4     | No recovery for comments and visits if the database is lost              | architectural                                                             |

### Medium severity

| ID      | Finding                                                                                               | Location                                                          |
| ------- | ----------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| C-4     | Re-import silently discards changed phone, email, city, state, ZIP, address line 2                    | `commands/import.rs::run_diff`                                    |
| C-5     | Restoring a deleted household can fail on a raw UNIQUE constraint                                     | `commands/households.rs::restore_deleted_household`               |
| C-6     | Resolving a review item detaches siblings; Confirm Delete becomes a silent no-op that reports success | `commands/import.rs::resolve_review_item`                         |
| C-7     | "Add all new records" is not atomic and reports no partial progress                                   | `commands/import.rs::resolve_all_new_records`                     |
| C-8     | Households without coordinates are invisible on the map and in every visit list                       | `commands/visits.rs::fetch_grouped_households`                    |
| C-9     | `roads.rs` carries an unclamped duplicate of the haversine fixed in #24                               | `commands/roads.rs::haversine_m`                                  |
| C-10    | `.pbf` ingest has no size cap and can exhaust memory with no error                                    | `commands/roads.rs::ingest_road_database`                         |
| C-11    | Log Viewer auto-refresh never fires — checks `.active` on the wrong element                           | `views/log-viewer.js::startLogTail`                               |
| C-12    | PDF import blocks the async runtime for its entire duration                                           | `commands/import.rs::import_pdf`, `pdf_parser.rs::parse_pdf`      |
| C-13    | Parser silently drops households whose surname starts with a non-ASCII capital                        | `pdf_parser.rs::name_line_re`                                     |
| C-14    | CSV import corrupts quoted fields and reads the file uncapped                                         | `commands/import.rs::import_csv`                                  |
| P-1     | Visit-list snapping is quadratic over the whole road node table                                       | `commands/visits.rs::snap_to_graph`, `two_opt_improve`            |
| S-1     | PDF exports write the full directory unencrypted outside the app                                      | `views/directory-pdf.js`, `map-view.js`, `data-validation-pdf.js` |
| S-2     | Map tiles disclose the congregation's location to a third party                                       | `views/map-view.js`, `tauri.conf.json` CSP                        |
| ~~U-1~~ | A second backup on the same calendar day always fails, with no way to proceed                         | `views/backup-restore.js`, `commands/paths.rs`                    |
| U-2     | Household modal discards unsaved comments and visit notes with no prompt                              | `views/households-view.js::openHouseholdModal`                    |
| U-3     | Review items claim "Changed: Comments" on every annotated household                                   | `commands/import.rs::changed_fields`                              |
| U-4     | The "Minimum log level" setting has no effect on what is written                                      | `commands/logs.rs::log`                                           |
| U-5     | Visits and comments cannot be exported in any machine-readable form                                   | `views/visits-report-view.js`                                     |
| R-2     | Nothing indicates whether a backup has ever been taken or how old it is                               | `views/backup-restore.js`, `sidebar.js`                           |

### Requires verification

Two items I could not confirm from source alone. Both should be tested on real
target platforms before being relied on, and neither is filed as a finding:

1. **`pdfMake` blob downloads under WebKitGTK in a packaged build.** All three
   PDF exports terminate in `pdfMake.createPdf(...).download(filename)`. Whether
   a Tauri webview on Linux honours that, prompts, or silently does nothing was
   not determinable from the code. `sidebar.js` asserts "Report saved to your
   Downloads folder" while its own comment admits the path is a guess. If the
   download silently fails on any platform, the Data Validation feature appears
   to work and produces nothing.

2. **Inline script hashing in the release CSP.** `index.html` ends with
   `<script>window.onload = () => Core.init();</script>`, and the configured CSP
   is `script-src 'self'` with no nonce and no hash. Tauri 2 is understood to
   inject hashes for inline scripts at build time, which would make this work —
   but if it does not, or if the behaviour changes, the release build never
   boots while `cargo tauri dev` continues to work. Worth confirming against an
   actual packaged artifact rather than assuming.

### Below threshold — noted, not filed

Recorded so they are not lost, but not worth issue tracker entries at this
stage:

- **`roads.db` observations.** `road_names_fts` is created with full
  insert/delete/update triggers, but nothing in the codebase ever queries it —
  the FTS5 index is built and maintained on every ingest for no consumer.
  `ingest_road_database` deletes and reinserts both tables without a subsequent
  `VACUUM`, so the file grows across re-ingests. `open_roads_pool` does not set
  `PRAGMA foreign_keys = ON` (unlike `open_pool`), so `road_edges`'s foreign
  keys to `road_nodes` and `road_names` are declared but not enforced.
  `get_roads_in_bounds` does not validate that `min_lat < max_lat`. None of
  these is currently harmful.
- **`node_local_id` in `ingest_road_database` is effectively dead** — it is
  populated and used only for a `contains_key` dedupe check; its values are
  never read, because `osm_to_row` is rebuilt from the database afterward.
- **`_dedupe_guard` in `generate_visit_list`** is an unused `HashSet` with a
  "reserved" comment.
- **Modal element lookups are global, not scoped.** `openHouseholdModal` and
  several others use `document.getElementById` for their own controls rather
  than querying within their overlay. Not reachable today because modals do not
  stack, but fragile.
- **Three haversine implementations exist** with three different numerical
  behaviours: `geo.rs` (clamped, correct), `roads.rs` (unclamped — C-9), and
  `map-view.js` (`atan2` form, numerically the best of the three).
- **`debug` and `warning` log levels are offered in Settings and in the Log
  Viewer but no code path ever writes them.** Every `logs::log` call site uses
  `"info"` or `"error"`.
- **`src-tauri/capabilities/` was not included in the supplied archive**, so the
  Tauri 2 permission set could not be audited. Worth a look — it governs what
  the `shell` and `dialog` plugins are actually allowed to do, and
  `tauri-plugin-shell` in particular should be scoped to the `pdftotext`
  sidecar only. Same for `tauri.linux.conf.json`, referenced by
  `pdf_parser.rs` but not supplied.

---

## Preserving comments and visits

Your second question, treated as a design problem rather than a finding. Two
issues cover it: **R-3** (durable identity) and **R-4** (the journal and the
matching cascade). This section is the reasoning behind them.

### Why this is hard

Everything in Lost Sheep is reconstructible except two fields. Households come
from the source PDF. The road graph comes from a `.pbf` you keep. Tags are a
handful of labels. **`households.comments` and the `visits` table are the only
data the user creates**, and they exist nowhere but inside `lost-sheep.db` and
encrypted copies of it. A backup is a copy of the same single point of failure,
not an independent one — and it needs a passphrase that is deliberately stored
nowhere.

The hard part, as you said, is identity. I traced every candidate:

- **`households.id`** is a rowid, and it is regenerated by ordinary use.
  `resolve_review_item` implements Replace and Merge as insert-new-then-delete-
  old and takes a fresh `last_insert_rowid()`. `restore_deleted_household` does
  the same and documents it. Accepting a routine directory update changes a
  household's id.
- **`source_key`** is `normalize(first + last + first2 + last2 +
  address_line1)`. A marriage, a surname correction or a move changes it. It is
  also explicitly not unique — the `source_key_seq` column comment states
  "source_key alone is NOT unique" and describes the father-and-adult-son case.
- **`source_key_seq`** is positional: assigned as `COALESCE(MAX(...), -1) + 1`
  at insert time, or by insertion order in the migration. Two databases built
  from the same source in a different order assign it differently. It is also
  discarded entirely at soft-delete (which is separately a bug — C-5).
- **`address_key`** identifies a place, not a household, by design — it is what
  groups co-residents for visit lists.

So nothing in the schema is both stable and unique. `restore_preview` already
trips over this and says so, emitting a `"duplicate_key"` diff row reading
"names/address collide, diff below may be approximate for this key".

### The proposal, in two parts

**Part one (R-3): add a durable identity.** A `household_uid` (UUIDv4) minted
once when a household first enters the database and never rewritten — critically,
**carried forward rather than re-minted** through Replace, Merge, soft-delete
and restore. Backfilled for existing rows by a migration following the
`migrate_source_key_seq` pattern already in `db/mod.rs`. A `visit_uid` should
come in the same migration.

This makes identity durable within a database and its descendants: across
replace/merge, across delete/restore, across backup and restore. It does **not**
by itself solve the total-loss case, because a database rebuilt from the PDF
mints fresh uids that no external record can match. The uid is the fast path,
not the whole answer.

**Part two (R-4): an append-only recovery journal.** On every comment save and
every visit record, append one event to a separate file. Append-only, so a
crash mid-write costs one line rather than the file. Each event carries three
things:

- a **stable event identity** (`visit_uid`), so replaying into a database that
  already holds some of those visits does not duplicate them;
- the **`household_uid`** — exact match, no user involvement, whenever the
  target database descends from the source one;
- a **full identity snapshot at write time** — first/last, second head, both
  address lines, city, state, ZIP, `address_key`, `source_key`,
  `source_key_seq`, lat/lon, phone, email. This is what makes the disaster case
  solvable: a brand-new database has none of the old uids, but it does have all
  of these fields.

The snapshot must be **refreshed, not left stale**. Whenever Replace or Merge
changes a household's identifying fields, emit an identity-refresh event so the
journal tracks who they are now, not who they were the first time someone wrote
a note about them.

### The matching cascade

Reattaching a journal to a database it did not come from, tier by tier. Nothing
is ever auto-discarded.

| Tier | Match                                       | Handling                         |
| ---- | ------------------------------------------- | -------------------------------- |
| 1    | `household_uid` exact                       | auto-accept                      |
| 2    | `(source_key, source_key_seq)` exact        | auto-accept                      |
| 3    | `source_key` exact, seq ambiguous           | show candidates, user picks      |
| 4    | normalized full name + `address_key`        | auto-accept                      |
| 5    | normalized full name only (household moved) | user confirms                    |
| 6    | `address_key` only (name changed)           | user confirms                    |
| 7    | no match                                    | park as unmatched, never discard |

Tier 7 matters more than it looks. An unmatched note stays parked indefinitely
and is re-run against the database after every future import, so a household
who reappears in a later directory picks their history back up without anyone
having to remember they were missing.

The review interface for tiers 3, 5, 6 and 7 should reuse the Review Updates
pattern rather than inventing a second one — same shape, same buttons, a
workflow the user already knows.

### Four decisions I did not make for you

1. **Where the journal lives.** The configured backup folder means a user who
   points it at a synced directory gets off-machine copies for free. The app
   data directory is tidier but co-located with the thing it insures against.
2. **How it is protected.** It holds names, addresses, phones, emails and visit
   comments — the most sensitive content in the app, in the clearest form it
   has ever existed in. Plaintext is not acceptable. A keychain-encrypted
   journal is automatic and prompt-free but useless in exactly the
   machine-died-with-the-keychain scenario it exists for. A passphrase-derived
   key survives that but costs a prompt per session. My recommendation is both:
   keychain encryption for the continuous local journal, plus a manual
   passphrase-protected export for off-machine storage, with the app showing
   how stale that export is.
3. **JSONL or SQLite.** JSONL is append-only by nature and readable by a future
   person with no copy of this app. SQLite is easier to query and harder to
   corrupt, but reintroduces a binary dependency at the moment everything else
   has been lost. I lean JSONL.
4. **Replay granularity** — final state only, or the full edit history.
   Recommend retaining history in the file and replaying final state by
   default.

### Sequencing

R-4 depends on R-3. Do not start the journal before the uid exists and has been
verified to survive Replace, Merge, delete/restore and a backup round trip — a
journal keyed on an unstable identifier is worse than none, because it looks
like insurance.

Worth splitting delivery into three: (i) schema plus journal writing, (ii)
manual export and import of the journal file, (iii) the matching cascade and
its review UI. Stage (i) alone already removes the total-loss scenario. Stages
(ii) and (iii) make recovery practical rather than merely theoretically
possible.

Three cheaper mitigations are worth landing regardless, and much sooner: **U-5**
(a plain CSV export of the visit log — the single cheapest thing that gets this
data out of the app), **R-1 Piece 2** (a pre-restore safety copy), and **R-2**
(a visible backup-age indicator).

---

## Positive Findings

Places where the code is doing the right thing, with the specifics, because
these are load-bearing and should not be lost in a future refactor.

**Every SQL query is parameterized.** Across 11 command modules I found no
string-interpolated user data in any statement. The two places that build SQL
dynamically do it correctly: `households.rs::build_where` generates `?`
placeholders and pushes values into a `Vec<Box<dyn ToSql>>`, and
`logs.rs::get_logs` generates numbered placeholders and binds each level. Even
`run_prune`'s date modifier, which is built with `format!`, is passed as a bound
parameter rather than spliced — and the comment explains exactly why: "The
previous version's safety depended entirely on the `.parse::<i64>()` two lines
above the format!; this doesn't depend on that at all."

**LIKE metacharacters are escaped.** `escape_like()` handles `\`, `%` and `_`,
paired with `ESCAPE '\\'` in the query, so a literal `%` in the search box
matches literally rather than acting as a wildcard.

**Output encoding is consistent.** `escapeHtml` in `core.js` covers `&`, `<`,
`>`, `"` and `'`, and is applied at every `innerHTML` interpolation of
non-constant data I traced — including inside attribute values, which is where
this is usually missed. `map-view.js` additionally uses `CSS.escape` for the
`data-select-seed` selector. I found no XSS.

**No browser storage, no `eval`, no dynamic script loading.** State lives in
module-scope objects and in the database.

**The CSP is tight.** `script-src 'self'` with no `unsafe-inline`, no
`unsafe-eval`, `default-src 'self'`, and network access narrowed to a single
host pattern. `withGlobalTauri: false`, so the API is not sprayed onto
`window`.

**Path handling canonicalizes rather than prefix-matches.** `paths.rs` is the
single enforcement point for every path-taking command, and its header explains
the reasoning: "`std::fs::canonicalize` resolves `..` and symlinks, so a naive
`raw.starts_with(home)` string test (which `~/backups/../../etc/x` would pass)
is not used anywhere here." Writes are additionally constrained to the
configured backup folder, read from the database rather than trusted from the
caller, with the parent directory compared for exact equality rather than
prefix.

**Backup archive parsing is defensive in the right order.** `extract_backup_zip`
validates entry count, then entry names, then entry sizes against
`MAX_DB_ENTRY_BYTES` / `MAX_SALT_ENTRY_BYTES` — all before reading anything and
before the passphrase is checked. That ordering is correct and deliberate.

**Key derivation is sound.** Argon2id, `V0x13`, 19 MiB / 2 passes / 32-byte
output, with a fresh 16-byte random salt per backup bundled alongside the
database in the same archive so the two cannot be separated. The live key is 32
bytes from `thread_rng`, hex-encoded, held in the OS keychain. `apply_key`
performs a sanity read after setting the pragma so a wrong key surfaces as an
error rather than as corrupt reads.

**The hand-rolled hex decoder validates before it assumes.** It checks even
length and `is_ascii_hexdigit` on every byte before chunking, and the `unwrap`
on `from_utf8` carries a comment justifying why it cannot fail. This is the
right way to write that.

**`TmpFile`'s `Drop` impl removes scratch files on every early-return path**,
so a `?` in the middle of backup or restore cannot leak a decrypted database to
the temp directory.

**Restore requires a preview, and the token is single-use.** `last_preview` is
`.take()`n on every commit attempt whether it matches or not, so a token cannot
be replayed and a failed attempt forces a fresh preview. The token binds path,
size and mtime — not a security hash, and the comment says so honestly.

**The WAL sidecar cleanup in `restore_commit` is a subtle bug that was found
and fixed**, with a comment explaining the failure it caused: stale `-wal`
frames recovering over the newly-swapped file, making a restore appear to
succeed and then show no data.

**Transactions are used correctly where they matter.**
`soft_delete_household`, `restore_deleted_household`, `resolve_review_item`,
`delete_tag`, `tag_households` and `discard_import_batch` each wrap their
multi-statement work in one transaction. The mirror-then-delete ordering in the
delete paths is correct — visits and tags are copied out before the cascade
removes them — and `resolve_review_item` re-points `visits` to the new row
*before* deleting the old one, with a comment noting that the order is
load-bearing.

**Prepared statements are scoped to drop their borrows.** The blocks in
`soft_delete_household` are explicitly structured "so each prepared statement
(and its borrow of tx) is dropped before the next tx.execute()" — the E0597
shape, handled deliberately rather than discovered repeatedly.

**Integer overflow was considered.** `search_households` computes pagination in
`i64` with a comment explaining that `(page - 1) * page_size` in `u32` could
overflow above ~8.6 million and either panic or silently wrap.

**Floating-point total functions.** `geo::haversine_meters` clamps before
`asin` to avoid `NaN` at near-antipodal inputs; `generate_visit_list` sorts
with `total_cmp` rather than `partial_cmp().unwrap()`, replacing a real crash;
`pdf_parser::valid_coord` rejects non-finite and out-of-range coordinates
before they reach the database. All three came from the same fix and all three
are the right shape.

**`find_potential_problems` uses `spawn_blocking` correctly**, cloning the
pools out of `State` first because `State<'_, AppState>` is not `'static` — and
the comment explains both the symptom and the mechanism. This is the pattern
`import_pdf` should adopt (C-12).

**A/A* is implemented properly.** Admissible haversine heuristic, `Ord`
deliberately reversed to make `BinaryHeap` behave as a min-heap, and stale heap
entries skipped rather than re-expanded.

**No `unsafe`, anywhere.** No `unwrap()` or `expect()` on fallible runtime
paths; the only `expect()` calls are in `main.rs`'s startup, where failure
genuinely should abort, and the keychain failure path exits with a message that
points the user at the documented recovery route.

**Errors are logged, not just returned.** The closure-wrapping pattern in
`backup_database`, `restore_preview`, `restore_commit`, `import_pdf`,
`import_csv`, `ingest_road_database`, `resolve_review_item` and
`discard_import_batch` covers every early-return `?` with one log call, instead
of needing a log line at each one.

**Minors' names are never stored.** `pdf_parser` keeps only a `has_minors`
boolean, the fixture has a regression test asserting the names do not appear in
comments, and the UI renders the flag as a placeholder. This is the most
careful piece of data handling in the codebase and it is worth saying so.

**Comments and tags are preserved across Replace and Merge**, with the code
carrying the history of why: the parser's comments value "is almost always
None, so an unconditional overwrite silently erased whatever the user had
typed". The behaviour is correct — only the review UI's label about it is wrong
(U-3).

**The codebase documents its own reasoning unusually well.** A large number of
the comments record not just what the code does but what went wrong before,
which alternative was rejected and why. That is what made this audit possible
to do properly in one pass, and several findings above were found by noticing
that a comment's stated invariant no longer matched the code beside it.

---

## Second pass

Re-reviewed on the assumption that the first pass missed subtle issues,
specifically looking for privilege escalation, authentication and authorization
bypass, race conditions, business-logic flaws, data leakage, missing
validation, TOCTOU, concurrency issues, and inconsistent assumptions between
frontend and backend. Only items not already listed above are reported here.

**Privilege escalation, auth bypass, authz bypass** — no new findings, and
none possible: the application has no privilege model to escalate within.

**TOCTOU.** Two windows exist, both minor and both worth knowing about:

- `paths::resolve_write_dest` checks `dest.exists()` and returns a path;
  `write_backup_zip` then calls `File::create` on it. A file appearing in
  between would be truncated. Requires local write access to the backup folder
  during the gap; not worth a fix, but it means the no-overwrite guarantee is
  best-effort rather than atomic.
- `restore_preview` stashes `(src_path, token)` where the token hashes path,
  size and mtime; `restore_commit` re-reads the file. A replacement with
  identical size and mtime would pass. The comment already scopes this
  correctly — "not a security hash" — and the threat model does not include an
  attacker with write access to the user's home directory.

**Concurrency.** `last_preview: Mutex<Option<(String, String)>>` is the only
shared mutable state and is used correctly — `.take()` under lock, poisoning
mapped to an error rather than unwrapped. The r2d2 pools are `max_size: 8` in
WAL mode. Because Tauri runs synchronous commands on a thread pool, two
commands genuinely can run concurrently; I looked for read-modify-write races
across separate pooled connections and found the assignments that matter
(`source_key_seq` via `COALESCE(MAX(...), -1) + 1`) are inside the same
transaction as their insert, which serializes them under SQLite's write lock.
No deadlock path found: no code holds two connections from the same pool
simultaneously, and no lock is held across an `.await`.

**Frontend/backend assumption mismatches.** Several found; all are already
captured above as findings, but the pattern is worth naming because it recurs:
the Settings retention dropdown offers values that disagree with the schema
seed (C-2); the Log Viewer's level setting means something different from what
its label says (U-4); the review UI's "Changed: Comments" label implies
behaviour the backend deliberately does not have (U-3); the backup modal offers
no filename field while the backend's error tells the user to choose a
different name (U-1); the Data Validation modal asserts a save location the
backend never returns (S-1). In every case the backend is correct and the
frontend describes something else.

**Business-logic flaws.** Two new observations, neither filed:

- `run_diff`'s empty-database fast path triggers on `existing_household_count
  == 0` and bypasses review entirely, inserting every parsed record directly.
  That is correct for a genuinely first import — but "the households table is
  empty" is also the state after a restore from a backup that happened to be
  empty, or after deleting everything. An import at that moment silently
  bulk-inserts with no review step. Low likelihood; noting the assumption.
- A household whose street address changed generates *two* review rows in one
  batch — a `'changed'` row matched by name, and a `'removed'` row because the
  old `source_key` was not seen. This is the mechanism behind C-6 and is
  separately a source of review-queue noise. Deduplicating it in `run_diff`
  would reduce both.

**Missing validation.** `rename_tag` trims and normalizes but does not check
for an empty result or for collision with an existing `name_norm`, so a
duplicate or blank rename surfaces a raw SQLite `UNIQUE constraint failed`
message. Same class as C-5 but lower impact — filed nowhere; mentioning it here
because the fix is two lines and could ride along with any tags work.

**Data leakage.** Beyond S-1 and S-2, I checked whether anything sensitive
reaches the `logs` table, since logs are readable in-app and copied to the
clipboard wholesale by the Log Viewer's Copy button. Log messages carry
household ids, batch ids, counts and file paths — no names, addresses or visit
comments. That is the right line and it is held consistently across all call
sites.

**Dependencies.** `Cargo.toml` pins are current-major and unremarkable:
`rusqlite 0.31` with `bundled-sqlcipher` (no system OpenSSL dependency),
`argon2 0.5`, `zip 0.6` with default features off, `osmpbf 0.3`, `keyring 2`.
Vendored frontend code is better documented than is typical:
`src/include/tauri-api/VENDORED.md` records exact upstream versions
(`@tauri-apps/api@2.11.1`, `@tauri-apps/plugin-dialog@2.4.0`), why they were
vendored rather than CDN-loaded, the single line that was edited in
`dialog.js`, and how to upgrade. Leaflet and pdfmake carry their versions only
in their own file banners (Leaflet 1.9.4, pdfmake v0.2.7) with no equivalent
provenance note and no integrity hash recorded anywhere. Extending
`VENDORED.md` to cover those two — version, source, and a checksum — would
close the remaining supply-chain gap cheaply and is the only thing missing
here.
