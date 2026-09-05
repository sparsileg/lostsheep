# Lost Sheep — Security, Correctness and Reliability Audit

Scope: full review of `src/` and `src-tauri/` as uploaded (53 files, ~4,700 lines
of first-party code excluding vendored `pdfmake`).
Method: manual read of every source file, cross-referenced against
`Functional_Requirements.md` and the schema.

---

## 0. Threat model correction

The audit brief described a multi-user REST API with per-user API keys exposed on
the public Internet. **Lost Sheep is none of those things.** It is a single-user
Tauri 2 desktop application with no HTTP server, no network listener, no
authentication, and no multi-tenancy. Roughly half the requested categories —
authentication, authorization, BOLA, horizontal and vertical privilege
escalation, API key enumeration, CORS, CSRF, session handling, rate limiting,
replay — have no attack surface here and are marked N/A below with reasoning
rather than padded with speculation.

The audit was conducted against the real threat model, agreed in advance:

**Assets.** Names, addresses, phone numbers, email addresses, household comments
and visit notes for every family in a congregation. A minors-present flag (names
deliberately never stored — a good decision, see §4). Pastoral judgements
recorded as "Do not contact". This is sensitive personal data about identifiable
private individuals, including children, held by a non-technical user.

**Adversaries.**

1. Content the user imports — a directory PDF or CSV received from elsewhere.
2. A backup archive received from elsewhere and opened via Restore.
3. Any third party able to influence code the application loads at runtime.
4. Another process or user on the same machine.
5. The user's own mistakes, against which the app is the only safeguard.

**Non-adversary.** The user. Commands that let the user reach arbitrary paths are
assessed by what they enable *for adversaries 1–4*, not by whether the user can
misuse them.

---

---

## Issue index

Every finding below is filed in `sparsileg/lostsheep`. Ordered by issue number.

| Issue                                                   | Severity          | Finding                                                            |
| ------------------------------------------------------- | ----------------- | ------------------------------------------------------------------ |
| [#17](https://github.com/sparsileg/lostsheep/issues/17) | Critical          | Remote script loaded into a privileged webview                     |
| [#18](https://github.com/sparsileg/lostsheep/issues/18) | Critical          | Stored XSS via unescaped `role` from imported files                |
| [#19](https://github.com/sparsileg/lostsheep/issues/19) | High              | Visit history destroyed by cascade                                 |
| [#20](https://github.com/sparsileg/lostsheep/issues/20) | High              | Household comments overwritten on replace/merge                    |
| [#21](https://github.com/sparsileg/lostsheep/issues/21) | High              | Destructive operations are not transactional                       |
| [#22](https://github.com/sparsileg/lostsheep/issues/22) | High              | Bulk tagging silently truncates at 500                             |
| [#23](https://github.com/sparsileg/lostsheep/issues/23) | High              | "Do not contact" depends on a hardcoded tag name                   |
| [#24](https://github.com/sparsileg/lostsheep/issues/24) | High              | NaN coordinate panics visit-list generation and the map            |
| [#25](https://github.com/sparsileg/lostsheep/issues/25) | Medium            | Restore trusts a malformed backup archive                          |
| [#26](https://github.com/sparsileg/lostsheep/issues/26) | Medium            | Restore replaces the database under a live pool                    |
| [#27](https://github.com/sparsileg/lostsheep/issues/27) | Medium            | Only `info` is ever logged; log level setting is inert             |
| [#28](https://github.com/sparsileg/lostsheep/issues/28) | Medium            | Saving Settings silently deletes records; settings mass assignment |
| [#29](https://github.com/sparsileg/lostsheep/issues/29) | Medium            | Search misses fields; no multi-keyword AND                         |
| [#30](https://github.com/sparsileg/lostsheep/issues/30) | Medium            | Import name-match picks an arbitrary row                           |
| [#31](https://github.com/sparsileg/lostsheep/issues/31) | Medium            | Blocking commands and quadratic hot paths                          |
| [#32](https://github.com/sparsileg/lostsheep/issues/32) | Medium            | Commands accept arbitrary filesystem paths                         |
| [#33](https://github.com/sparsileg/lostsheep/issues/33) | Low               | Integer overflow in page offset arithmetic                         |
| [#34](https://github.com/sparsileg/lostsheep/issues/34) | Medium            | CSV import splits on commas with no quote handling                 |
| [#35](https://github.com/sparsileg/lostsheep/issues/35) | Medium            | Visit dates are unvalidated free text                              |
| [#36](https://github.com/sparsileg/lostsheep/issues/36) | High (functional) | Reporting cannot identify households needing a visit               |

Second-pass findings N-1 to N-8 are not filed; see that section.

## Executive Summary

### Overall security rating: **4 / 10**

The cryptographic and data-protection design is genuinely good and clearly
thought through — SQLCipher with an OS-keychain-held key, a portable backup
re-keyed via Argon2id, road-graph data deliberately excluded from backups,
minors' names never stored at all. Parameterised SQL is used consistently and
correctly; there is no SQL injection anywhere in the codebase. The rating is not
a comment on that work.

It is 4 because a well-encrypted database is reachable through an unguarded front
door. Two findings independently let non-user-authored code execute inside a
webview that has unrestricted access to every backend command, and a third means
those commands will read and write any file on the system. The three compose into
full compromise of the protected data.

The correctness picture is more concerning than the security one. Five separate
defects silently destroy user-created data during ordinary operations — deleting a
household, or accepting a routine re-import. The visit history and the household
comments are the only data in this application that cannot be re-derived from the
source PDF, and both are lost on paths the user is expected to take regularly,
with no warning and no recovery.

### Top five risks

1. **Leaflet is loaded from a public CDN into a webview with full IPC access.**
   `withGlobalTauri: true` publishes `window.__TAURI__` to every script in the
   page, including remote ones. A CDN compromise, a hostile network, or a DNS
   substitution yields arbitrary local file read and write. The declared CSP does
   not permit this load at all, which means either the map is silently broken or
   the CSP is not being enforced — and that ambiguity is itself a finding.
   *(issue #17)*

2. **Stored XSS from imported file content.** One field — `incoming.role` — is
   interpolated into `innerHTML` unescaped while every neighbouring field is
   escaped, and `import_csv` carries that column verbatim from the file with no
   validation. A shared directory CSV becomes code execution when the user opens
   Review Updates. *(issue #18)*

3. **Visit history is destroyed by cascade on three ordinary paths** — delete,
   replace, and merge — and is not carried into `deleted_households`, so
   "restore" does not restore it. The code demonstrably knows about this hazard:
   it rescues tag ids across the same delete with an explanatory comment, and
   does not rescue visits. *(issue #19)*

4. **"Do not contact" enforcement rests on a hardcoded string.** Renaming or
   deleting the tag silently re-enables visits to those households, with the tag
   still displayed on screen. This is the one failure in the codebase whose
   consequence is a person knocking on a door they were asked not to.
   *(issue #23)*

5. **Every path-taking command trusts an unvalidated string.** Backup writes and
   creates directories anywhere; restore, both imports and the road ingest read
   anywhere. Harmless alone; it is the multiplier that turns risks 1 and 2 into
   full filesystem access. *(issue #32)*

### Most likely attack vectors

In descending order of realism for this application:

- A directory CSV or PDF shared between congregations, carrying a payload in a
  field that is rendered unescaped (#18).
- The CDN dependency, which requires no attacker action against this user
  specifically — only against a widely-used piece of infrastructure (#17).
- A backup archive received from someone else and opened via Restore, which can
  panic the backend on a malformed salt and is read into memory unbounded (#25).
- Nothing at all: the data-loss defects (#19, #20, #21) fire during normal
  correct use, with no adversary involved. Statistically these are the ones most
  likely to actually harm this user.

### Most severe vulnerabilities

#17 and #18, jointly, with #32 as the amplifier. Either alone gives script
execution in a privileged context; #32 gives that script the filesystem. The
realistic worst case is that the entire congregational directory is copied off
the machine, or the live database replaced, from a file the user had every reason
to trust.

---

## Detailed Findings

Severities: **Critical** — full compromise of protected data.
**High** — silent destruction of unrecoverable data, or failure of a safety
control. **Medium** — meaningful correctness or robustness defect.
**Low** — hardening, hygiene, latent.

Each finding is filed as a GitHub issue in `sparsileg/lostsheep` and is headed
by its issue number here. This section states the finding, the evidence, and the
impact; attack scenarios, options, and recommended fixes are in the issue itself
at `https://github.com/sparsileg/lostsheep/issues/<n>`.

### Security findings

---

#### Issue #17 — Remote script loaded into a privileged webview

**Severity: Critical · Confidence: High**
`src/index.html:17,77` · `src-tauri/tauri.conf.json:13,15`

Leaflet's JS and CSS load from `https://cdnjs.cloudflare.com/ajax/libs/leaflet/1.9.4/`.
No local copy exists — `src/include/` contains only `pdfmake.min.js` and
`vfs_fonts.js`. `withGlobalTauri: true` makes `window.__TAURI__.core.invoke`
available to every script in the page, so remote code can call every command in
`main.rs`'s `generate_handler!` list.

Separately, the declared CSP has no `script-src` directive and therefore falls
back to `default-src 'self'`, which forbids the CDN. `style-src 'self'
'unsafe-inline'` likewise forbids the CDN stylesheet. Either the CSP is enforced
and Leaflet never loads (the map is broken), or it is not reaching the webview
(there is no script-source restriction at all). Both are defects and they need to
be distinguished by observation before anything else is decided.

Also a direct contradiction of the local-first architecture: `roads.rs`'s own
header says "local-first, no network calls", `pdftotext` is bundled as a sidecar
specifically to avoid an external dependency, and `Functional_Requirements.md`
treats offline operation as a design constraint. Leaflet breaks that at page
load.

**Impact:** arbitrary local file read and write, and exfiltration of the entire
household dataset, contingent only on a third party's infrastructure.

**Requires Verification:** no `src-tauri/capabilities/` directory was present in
the reviewed source. Tauri 2 gates commands and plugins through capability
grants; a broad `shell:allow-execute` grant would widen this to arbitrary
process execution.

---

#### Issue #18 — Stored XSS via unescaped `role` from imported files

**Severity: Critical · Confidence: High**
`src/js/views/review-view.js:46` · `src-tauri/src/commands/import.rs:58`

```js
`${escapeHtml(incoming.first_name)} ${escapeHtml(incoming.last_name)} (${incoming.role}) — ${escapeHtml(incoming.address_line1)}`
```

Three of four fields escaped, one not. The value reaches this line from
`import_csv`, which takes column 2 verbatim (`role: if f.len() > 2 { f[2].to_string() }`)
with no validation. The `households.role` CHECK constraint would reject a bad
value — but only at INSERT, which happens after rendering and only if the user
accepts the item. The payload executes during review regardless.

Assigned via `list.innerHTML = items.map(renderReviewItem).join('')` at line 37.
Execution context is the privileged webview described in #17.

The PDF path is narrower only by accident: `pdf_parser.rs:360` hardcodes
`role: "head"`. That is incidental, not a control.

**Impact:** as #17, triggered by a shared directory file rather than by
infrastructure compromise. This is the more realistic of the two for this user.

Same pattern, not currently exploitable, worth closing: `backup-restore.js:104`
(`${r.kind}`) and `log-viewer.js:57` (`${r.level.toUpperCase()}`), both
server-controlled enumerations today.

---

#### Issue #25 — Restore trusts a malformed backup archive

**Severity: Medium · Confidence: High**
`src-tauri/src/crypto.rs:38-43` · `src-tauri/src/commands/backup.rs:101-122`

The hand-rolled `hex::decode` slices `&s[i..i+2]` by byte index across
`(0..s.len()).step_by(2)`. An odd-length salt panics on an out-of-range slice; a
salt containing multi-byte UTF-8 panics on a char boundary. The salt is read from
a user-supplied zip and only `.trim()`ed. `derive_key_hex`'s `anyhow::Result`
gives no protection — the failure is a panic, not an error.

Both zip entries are read with `read_to_end` / `read_to_string` with no size cap.
`zip` is built with `default-features = false`, so compression codecs are not
linked and a classic ratio bomb is unavailable — but a large Stored entry still
reads in full. Same unbounded pattern in `import_csv`'s `read_to_string`
(`import.rs:45`) and in `roads.rs`'s in-memory graph build.

Separately, `showBackupModal` accepts a one-character passphrase — the only check
is `!p1 || p1 !== p2`. Argon2id at 19 MiB / t=2 / p=1 is the OWASP interactive
floor and appropriate for a real passphrase; it cannot compensate for a trivial
one. This protects a portable file containing the whole congregation's PII, which
`Functional_Requirements.md` positions as the primary recovery mechanism and which
will therefore be copied to places the SQLCipher key never goes.

---

#### Issue #26 — Restore replaces the database under a live connection pool

**Severity: Medium · Confidence: High**
`src-tauri/src/commands/backup.rs:227-251`

`restore_commit` renames a new file over `state.db_path` and deletes the `-wal`
and `-shm` sidecars while the r2d2 pool (`max_size(8)`) still holds connections
open on the old file, in WAL mode. On POSIX those connections keep reading the
old inode; on Windows the rename may fail outright. The frontend then asks the
user to restart — advisory only, with every write command live in the meantime.

`restore_commit` also never verifies that `restore_preview` ran against the same
file, so the before/after review that `Functional_Requirements.md` requires is a
UI convention rather than an enforced step.

---

#### Issue #32 — Commands accept arbitrary filesystem paths

**Severity: Medium · Confidence: High**
`backup.rs:32,74,163,227` · `import.rs:36,44` · `roads.rs:47`

Six commands take a path as an unvalidated `String`. `write_backup_zip` calls
`create_dir_all(parent)` then `File::create(dest_path)` — arbitrary directory
creation and file write, silently overwriting an existing file. `restore_preview`
is a read-side probe whose distinct error strings ("could not open", "not a valid
backup file", "missing its lost-sheep.db entry") disclose file existence and
type.

`Functional_Requirements.md` already states the intended bound and nothing
enforces it: "Backup/Restore can browse to arbitrary file location within the
user's home directory."

Not exploitable by itself — paths come from a native dialog in normal use, which
is why this is Medium. It is the component that converts #17 or #18 from "script
runs" into "script owns the filesystem".

`import_pdf` additionally passes the path into a sidecar argument vector
(`pdf_parser.rs:190`). **This is not command injection** — direct exec, no shell —
but a leading `-` would be read as an option.

---

#### Issue #28 (in part) — Mass assignment on settings

**Severity: Low · Confidence: High**
`src-tauri/src/commands/settings.rs:53`

`save_settings` writes any key/value pair the frontend sends, with no whitelist
and no transaction. Only `routeStart*` is validated. A partial failure leaves
some settings written and others not. `theme` flows to `themeLink.href`
(`sidebar.js:38`) — currently constrained by `style-src 'self'`, which is one of
the two things #17 casts doubt on.

---

### Categories assessed as not applicable

**Authentication, authorization, BOLA, function-level authorization, privilege
escalation (both axes), API key handling, session management, CSRF, CORS, rate
limiting, replay, timing attacks.** No server, no accounts, no credentials, no
multi-tenancy. There is exactly one user and one dataset; there is no "other
user's data" to reach.

**SQL injection.** Assessed thoroughly and **none found.** Every user value is
bound. `search_households` builds `where_sql` via `format!` but interpolates only
fixed clause strings and parameter indices; values go through
`params_from_iter`. `prune_old_deleted_and_logs` interpolates two integers into
`datetime()` modifiers, but both pass `.parse::<i64>()` first, so only an integer
can reach the string. Worth converting to bound parameters anyway — the safety
currently depends on a parse two lines away rather than on the query.

**Unsafe Rust.** No `unsafe` blocks anywhere in the codebase.

**Unsafe deserialization, template injection, eval, dynamic script loading.** No
`eval`, no `Function` constructor, no template engine. `serde_json::from_str`
into a well-typed `ParsedRecord` is not an unsafe-deserialization surface.

**Browser storage / token leakage.** No `localStorage`, no `sessionStorage`, no
cookies, no tokens. The backup passphrase is held transiently in a JS closure
during the restore flow and never persisted — acceptable, though it is never
zeroized in Rust either.

---

### Correctness findings

---

#### Issue #19 — Visit history destroyed by cascade

**Severity: High · Confidence: High**
`schema.sql:102` · `households.rs:160,235` · `import.rs:341,378`

`visits.household_id ... ON DELETE CASCADE` with `PRAGMA foreign_keys = ON`.
Three call sites `DELETE FROM households`: soft delete, review-resolve as
replace/merge, and review-resolve as delete. `deleted_households` mirrors 22
columns and no visits, so restore cannot bring them back — and
`restore_deleted_household` assigns a fresh id by design, so no future fix can
re-link by the old one.

The decisive evidence that this is an oversight rather than a decision is in the
code's own comment at `import.rs:328`: *"replace/merge delete the existing row,
and ON DELETE CASCADE would silently wipe its tag along with it — preserve it
here and restore it after the reinsert."* Tags are rescued. Visits, which the
user typed and cannot re-derive, are not.

`household_tags` also cascades, and `soft_delete_household` does not rescue tags
— so a deleted-and-restored household returns uncategorised and falls out of
every tag-filtered view, including visit-list generation.

**Impact:** permanent, silent loss of the only data in the application that
exists nowhere else. The directory PDF can be re-imported; coordinates can be
re-geocoded; tags are one click. Visit dates and notes cannot be recovered.

---

#### Issue #20 — Household comments overwritten on replace/merge

**Severity: High · Confidence: High**
`src-tauri/src/commands/import.rs:344`

Replace and merge re-insert from the parsed record, supplying `rec.comments`.
`pdf_parser.rs`'s own doc comment states that field is "never auto-populated by
the parser" and it is `None` for every record except the rare 3+-heads case;
`import_csv` sets it to `None` unconditionally. So the INSERT writes `NULL` over
the user's notes on every Replace.

Compounding: **`merge` and `replace` execute identical code.** They share a match
arm; the only difference anywhere is the string recorded in
`review_queue.resolution`. A user choosing Merge to preserve their own data is
choosing a label that does nothing.

---

#### Issue #21 — Destructive operations are not transactional

**Severity: High · Confidence: High**
`households.rs:160,235` · `import.rs:309`

`soft_delete_household`, `restore_deleted_household` and `resolve_review_item`
each perform multi-statement destructive sequences with no transaction. The worst
is `resolve_review_item`: five statements, and a failure between the DELETE and
the INSERT destroys the household outright with no `deleted_households` copy —
that path exists only in the `delete` arm.

The correct pattern is already established in the same crate:
`tags::tag_households`, `import::run_diff` and `import::auto_accept_all` all use
`conn.transaction()`. The single-record destructive paths were left out.

Also: `soft_delete_household` returns `Ok(())` for a nonexistent id.
`restore_deleted_household` checks `affected == 0` and errors. The two disagree.

---

#### Issue #23 — "Do not contact" depends on a hardcoded tag name

**Severity: High · Confidence: High**
`visits.rs:141,147` · `tags.rs:52,63` · `schema.sql:90,179`

The exclusion is `WHERE t2.name_norm = 'do not contact'`, matched as a literal
in both query branches. `rename_tag` will rename that row with no check;
`delete_tag` is a bare `DELETE` with no substitute and cascades its assignments
away. In either case the NOT IN subquery matches nothing and every affected
household silently becomes eligible for visits — while the tag still displays on
screen, so the user has every reason to believe the exclusion holds.

`schema.sql:179` reinforces the assumption that tags are disposable by name:
`DELETE FROM tags WHERE name = 'Deleted';` runs on **every** startup, not once,
so a user-created tag with that name is destroyed at the next launch.

Currently latent: `households-view.js:222` notes that no create-tag UI exists,
and `create_tag`/`rename_tag`/`delete_tag` have no caller in `src/js/`. It stops
being latent the moment tag management returns.

**Impact:** unique among these findings in that the consequence is not data loss
but a person arriving at a door where they were explicitly asked not to.

---

#### Issue #24 — NaN coordinate panics visit-list generation and the map

**Severity: High · Confidence: High**
`visits.rs:211` · `geo.rs:8` · `import.rs:71-72`

`entries.sort_by(|a, b| a.distance_meters.partial_cmp(&b.distance_meters).unwrap())`
panics when either operand is NaN. Two reachable sources:

- `import_csv` parses coordinates with `.parse().ok()`. Rust's `f64::from_str`
  accepts `"NaN"`, `"inf"`, `"-inf"` and returns `Ok`. Straight into the column.
- `geo.rs` computes `a.sqrt().asin()`; floating-point rounding can push `a` above
  1.0 for near-antipodal points, and `asin(>1)` is NaN. Out-of-range coordinates
  make this easy: `coord_re` matches `"999.999,-999.999"`, and nothing checks the
  ±90/±180 bounds between the regex and the INSERT.

`map_data::get_map_data` calls `generate_visit_list` directly, and the map is the
startup view (`core.js:111`). One bad row means every launch lands on a panicking
view.

The validation already exists in this codebase — `settings.rs:41`'s
`validate_route_start` checks both ranges correctly. It was applied to the one
coordinate the user types and not to the thousands that arrive by import.

Note the frontend's `haversineMeters()` uses the `atan2` formulation and does not
have this failure mode. Three copies of the same function, two formulations.

---

#### Issue #22 — Bulk tagging silently truncates at 500

**Severity: High · Confidence: High**
`src-tauri/src/commands/tags.rs:109`

`bulk_tag_search_results` requests `page_size: 100000`; `search_households:113`
clamps to 500. The count returned is `ids.len()` — 500 — which reads as success.
The function's own doc comment claims the opposite of what it does, and
`Functional_Requirements.md` states the requirement directly: "The complete
results set from search/filters can be tagged."

The clamp is correct and should stay; the caller assumed an opt-out that does not
exist. Note there is currently no UI caller for this command at all.

---

#### Issue #27 — Only `info` is ever logged; the log level setting is inert

**Severity: Medium · Confidence: High**
`logs.rs` · `settings-modal.js:66` · six call sites

Every `logs::log` call in the backend passes `"info"`, and all six are on the
success path. No failure is ever recorded — errors return a string to the
frontend, which shows it for four seconds and discards it. `record_visit`, the
application's central action, is not logged at all. `logLevel` is seeded, exposed
as a four-option dropdown, persisted, and read by nothing.

`Functional_Requirements.md`: "Important operations should be logged. Multiple
levels of logging available in the interface (error, warning, info, debug)."

Two sub-defects: `get_logs`'s unfiltered branch binds `?1,?2` against SQL
referencing `?2,?3`, so it always fails on parameter count (latent — the frontend
always passes a level); and Log Viewer pagination is per-level and merged, so
"Page 2" is the second page of each level independently, not of anything the user
sees.

---

#### Issue #28 — Saving Settings silently and irreversibly deletes records

**Severity: Medium · Confidence: High**
`settings-modal.js:86` · `settings.rs:67`

Save calls `pruneOldDeletedAndLogs()` unconditionally — a destructive sweep as a
hidden side effect of an unrelated button, with no confirmation and no report of
what was removed. Retention inputs allow `0`, and `datetime('now','-0 days')` is
now, so `0` empties Deleted Records entirely — the app's only safety net for an
accidental deletion. A negative value produces the modifier `'--5 days'`, which
SQLite evaluates to `NULL`, so the comparison is `NULL` and the operation
silently no-ops.

---

#### Issue #30 — Import matches a changed household on name alone, arbitrarily

**Severity: Medium · Confidence: High**
`src-tauri/src/commands/import.rs:137`

`query_row` with no `ORDER BY` and no ambiguity detection, matching on names only
and ignoring the address — in the branch that exists precisely because the
address changed. When two households share a name (common in a congregation;
`Functional_Requirements.md` explicitly anticipates multiple heads per address),
the pairing is arbitrary and the review UI offers Replace, which destroys the
mis-paired household's visits and comments.

---

#### Issue #29 — Search misses tags and five stored fields; no multi-keyword AND

**Severity: Medium · Confidence: High**
`src-tauri/src/commands/households.rs:90-96`

Seven fields concatenated; `address_line2`, `state`, `zip`, both phones and both
emails are absent. Tags are excluded by explicit decision
("deliberately NOT tags"), contradicting the requirement ("Search is a simple
text search across the entire record, including tags and comments"). The whole
input is one substring, so `"Smith Winchester"` matches nothing —
`Functional_Requirements.md` specifies "always an implicit AND between
keywords". `%` and `_` in user input act as wildcards; no `ESCAPE` clause.

---

#### Issue #31 — Heavy commands block the UI thread; three quadratic hot paths

**Severity: Medium · Confidence: Medium (threading) / High (complexity)**
`roads.rs:47` · `map_data.rs:9` · `import.rs:177` · `visits.rs:237`

`ingest_road_database` and `import_csv` are declared `pub fn`, not `async fn`, so
they run on the main thread. `roads.rs`'s doc comment asserts the opposite —
"Runs on Tauri's command thread pool (not the UI thread) same as import_pdf" —
but `import_pdf` *is* `async fn` and this is not. The feature's own progress
events cannot render while the thread they would render on is blocked.
*Requires Verification by observation: does the stage text update mid-ingest?*

`get_map_data` calls `generate_visit_list` with `count: 100000`, which runs the
O(n²) nearest-neighbour walk over the whole database on every Dashboard load
whenever a route start is configured — to produce an ordering `map_data.rs`'s own
comment says the frontend ignores. `run_diff` does `Vec::contains` per existing
household (10⁸ string comparisons at the documented 10,000-record ceiling).

---

#### Issue #34 — CSV import splits on commas with no quote handling

**Severity: Medium · Confidence: High**
`src-tauri/src/commands/import.rs:50`

`line.split(',')` — a quoted address containing a comma shifts every later column
left, so `city` receives half an address and `latitude` receives a ZIP, which
parses successfully as a number. Quote characters are never stripped. `.skip(1)`
discards the first line without checking it is a header. `role` is unvalidated
(the #18 vector). Labelled a stub in its own comment and in the requirements —
but wired to a live UI button that fails silently rather than declining.

---

#### Issue #33 — Integer overflow in page offset arithmetic

**Severity: Low · Confidence: High**
`households.rs:114` · `logs.rs:27`

`(page - 1) * page_size` in `u32`. `page_size` is clamped both ends; `page` is
clamped only at the bottom by `.max(1)`. Panics on overflow in debug, wraps
silently in release. Not reachable from the UI; reachable over IPC.

---

#### Issue #35 — Visit dates are unvalidated free text

**Severity: Medium · Confidence: High**
`visits.rs:8` · `schema.sql:103`

`record_visit` accepts and stores any string. The only check is a frontend regex
that also accepts `2026-02-30` and `2026-13-45`. `get_visits_report` filters with
`BETWEEN` — a lexicographic string comparison — so a non-ISO date lands outside
every range and vanishes from reporting while remaining visible in the household
modal. The two views disagree with no explanation. This directly undermines any
recency-based reporting: one malformed row makes a visited household look
never-visited.

---

#### Issue #36 — Reporting cannot identify households needing a visit

**Severity: High (functional) · Confidence: High**

`get_visits_report` is `FROM visits v JOIN households h` — an inner join from
visits, so a household with zero visits cannot appear. No `LEFT JOIN visits`,
`MAX(v.visit_date)` or `last_visit` concept exists anywhere in the backend.
`generate_visit_list` never reads the `visits` table, so the routing tool has no
knowledge of where anyone has been. `visits` has no outcome field, so a knock
with no answer is indistinguishable from a conversation.

Treated in full in the companion document `Visit_Reporting_Design.md`.

---

## Second pass

Re-read assuming the first pass missed subtleties, looking specifically for
privilege escalation, authorization bypass, race conditions, business-logic
flaws, data leakage, TOCTOU, and frontend/backend assumption mismatches. Only
findings not already listed above appear here.

**None of these have been filed as issues.** They are either too small to track
separately or fold naturally into an existing one; where that applies, the
relevant issue is named. Decide per item whether to file, fold, or drop.

**N-1 — `map_data` bypasses the visit-list contract (Medium).** *Overlaps #31, which proposes separating the two callers.* `get_map_data`
calls `generate_visit_list` with `seed_household_id: 0`, relying on the seed
lookup failing and `.unwrap_or((0.0, 0.0))` falling back to Null Island. It
therefore inherits the do-not-contact exclusion by accident rather than by
design. If `generate_visit_list`'s filtering is ever changed for routing reasons,
the map's filtering changes silently with it. Two callers with different
requirements sharing one function through a documented fudge
("Reuses generate_visit_list's grouping/shape with an unreachable seed").

**N-2 — Nondeterministic visit-list ordering (Low).** *Folds into #24, which already proposes a deterministic tiebreak.* Groups are built in a
`HashMap` and sorted by distance with no tiebreak, so equal-distance addresses
order differently between runs. Two identical requests can produce different
routes and different PDFs.

**N-3 — TOCTOU on the backup destination (Low).** *Folds into #32.* `write_backup_zip` calls
`create_dir_all`, then `File::create`, then `backup_database` separately
`metadata()`s the path to confirm success. Three unsynchronised filesystem
operations; the success confirmation can validate a file other than the one
written. Minor here, but the confirmation exists specifically to catch backups
that appeared to succeed and were not found afterward, so a check that can be
fooled undercuts its own purpose.

**N-4 — `restore_deleted_household` can create duplicate `source_key` rows
(Medium).** *Related to #21 and #30; not covered by either as written.* Restore assigns a fresh id, correctly reasoning that the old one may
be occupied. But it does not check whether a household with the same
`source_key` already exists — which is exactly the case when a household was
deleted, re-imported, and then restored from Deleted Records. Two rows then share
a `source_key`, and `run_diff`'s `query_row` on that key silently picks one,
corrupting every subsequent import diff for that household.

**N-5 — Frontend/backend disagreement on the tag cardinality invariant (Low).**
`tag_households` enforces one tag per household by deleting all existing
assignments first. `household_tags`'s primary key permits many. `renderTagChips`
maps over an array, the household modal offers per-chip removal, and
`households-view.js:70` tests `.includes('Known')` — which would also match a tag
named "Known family". The one-tag rule is a backend behaviour, not a constraint,
and three layers assume different things about it.

**N-6 — `emit_progress` can be lost (Low).** `import.rs:30` emits only when
`processed % 10 == 0 || processed == total`. For an import of fewer than 10
records the first emission is also the last, so the progress indicator jumps
straight to complete — which `import-view.js` already works around with a
double-`requestAnimationFrame` and a 400 ms minimum display. The workaround
suggests this was noticed and treated as a display problem rather than an
emission one.

**N-7 — No concurrency guard between destructive commands (Medium).** *Overlaps #26's Option A, which would supply the lock.* Nothing
prevents `restore_commit` running while an import transaction is open, or
`prune_old_deleted_and_logs` running during `resolve_review_item`. Single-user
UI makes this unlikely, but the IPC surface has no such restriction and
`resolve_all_new_records` loops issuing separate non-transactional calls, each
taking its own pool connection — a window during which other commands can
interleave.

**N-8 — Error strings returned verbatim to the frontend (Low).** Every command
ends `.map_err(|e| e.to_string())`. Raw rusqlite and IO errors reach the message
bar, including absolute filesystem paths and SQLite internals. No security
consequence in a local single-user app — the user already knows their own paths —
but it produces messages a non-technical user cannot act on, which is the
population this app is explicitly built for.

---

## Positive Findings

Genuine strengths, several of which are better than what this class of
application usually does.

**Parameterised SQL, without exception.** Every user-supplied value is bound.
The two places that build SQL with `format!` interpolate only fixed clause
fragments and parameter indices, and the one place that interpolates a value
parses it to `i64` first. There is no injection anywhere, and that is not luck —
it is consistent across nine command modules by different-looking code paths.

**No `unsafe`.** Not a line of it in the crate.

**Minors' names are never stored.** `pdf_parser.rs` deliberately parses and
discards them, keeping only a boolean, with the reasoning recorded in the struct
field's doc comment and a regression test (`flags_minors_as_a_boolean_not_stored_names`)
asserting the names do not leak into comments. This is data minimisation applied
to the most sensitive field in the dataset, and it was done proactively.

**`backupFolder` is stripped from backup payloads.** A local path is not portable
data and never enters the file — a small, thoughtful distinction most
implementations miss.

**The road graph is excluded from backups.** Correctly reasoned as re-ingestible
data that would otherwise dominate the archive.

**Backups are cryptographically independent of the machine.** Re-keyed via
Argon2id from a user passphrase rather than the keychain key, which is what makes
"restore on a new machine" and "recover from a keychain failure" actually work.
The Help modal documents this path.

**Argon2id chosen correctly**, with parameters matching OWASP interactive
guidance and a comment explaining why slowness is acceptable here.

**`TmpFile` with a `Drop` implementation.** Scratch files are cleaned up on every
early-return `?` path by construction rather than by remembering.

**The WAL sidecar cleanup in `restore_commit`.** The comment explains a real,
subtle bug — stale `-wal` frames winning over a freshly swapped file, producing a
restore that "completes" and shows no data. That is a hard-won diagnosis, and the
fix is right even though the surrounding pool handling (#26) is not.

**`escapeHtml` is correct** and applied at the large majority of interpolation
sites. The two-and-a-half exceptions are the finding; the discipline is real.

**Regression tests encode diagnosed bugs.** `does_not_drop_common_csz_line_as_boilerplate`
exists because a frequency-based boilerplate filter once ate legitimate
city/state/zip lines, and the test says so. The `pdf-extract` rejection is
documented in the `parse_pdf` doc comment with the specific symptom and an
instruction not to revert without re-verifying. This is unusually good
institutional memory for a solo project.

**Cached regexes with the reason recorded.** `OnceLock` on every parser regex,
with a comment stating the measured cost of getting it wrong (~7,800 compiles
per import).

**Tag preservation across replace/merge** is handled deliberately, with a comment
explaining the cascade hazard. That the same care was not extended to visits is
#19 — but the pattern to copy is already there.

**The dropdown replacement is justified in code.** `dropdown.js` explains that
native `<select>` popups are rendered by the OS and ignore theme variables on
WebKitGTK. Comparable notes exist for the date-picker freeze and for Leaflet's
z-index behaviour. Platform constraints are recorded where the next reader needs
them, not lost.

**Runtime map sizing.** `resizeMapEl()` computes height from the element's actual
position rather than a guessed `calc(100vh - 40px)`, with the comment explaining
why the static version was always wrong by exactly `#messageArea`'s height.

---

## Recommended order of work

1. **#18** — one-line render fix plus boundary validation. Smallest fix, largest
   risk reduction, no design decisions.
2. **#17** — vendor Leaflet locally; determine whether the CSP is enforced. This
   and (1) together close both paths to `invoke()`.
3. **#19, #20, #21 together** — they edit the same ~40 lines of
   `resolve_review_item`. One change, not three passes. This is the data the user
   cannot get back.
4. **#24, #23** — a crash on the startup view, and a safety control that fails
   silently.
5. **#22, #35, #28** — silent truncation, silent date loss, silent deletion.
6. **#32, #25, #26** — defence in depth, once the paths that would exploit them
   are closed.
7. **#36** — the reporting build, per `Visit_Reporting_Design.md`. Stage 1
   requires no schema change and can start in parallel with anything above.

Everything else is hygiene and can be batched.
