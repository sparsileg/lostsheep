# Lost Sheep — Clerk Referral Design

Status: proposal, for discussion
Companion to: `Visit_Reporting_Design.md` (issue #36)

---

## 1. The problem this document addresses

The original concept of operations was simple: visit the *Not known*
households, and once visited they become *Known*. In practice a visit often
turns up something the ward record needs to reflect:

- the family has moved (new address known, or not),
- a member has died,
- the phone, email or address on file is wrong,
- the member has asked to have their records removed.

None of these can be fixed in Lost Sheep. The ward clerk has to update
the membership record: move it to another unit, send it to the
"address unknown" file at headquarters, record a death, and so on. The
user's job is to **tell the clerk, then make sure it actually got done.**

Lost Sheep currently has nowhere to keep that work. The options today are:

- a free-text note in the household comments, which nothing reads back, or
- a tag such as "Moved", which has no dates and no states. Telling "found
  out" apart from "told the clerk" apart from "clerk did it" would need
  three tags, swapped by hand, and the dates would be lost on every swap.

### 1.1 Why not tags

Tags describe **who a household is to us**: Known, Not known, Do not
contact. They are long-lived categories, used for grouping and routing.

A referral is a **piece of work**. It has a beginning (we learned
something), a handoff (we reported it), and an end (the record changed, or we
gave up). It has dates at each step and can go stale. That is a different
shape of data, and forcing it into tags would erode what tags are good at.

### 1.2 The existing tool that closes the loop

The user re-imports the ward directory roughly **weekly** (fortnightly
when travelling). Every import already diffs against the database and
produces `review_queue` items of type `removed` and `changed`
(`commands/import.rs::run_diff`).

That diff *is* the evidence that the clerk acted:

| Referral                 | What the clerk does                         | What the next import shows                    |
| ------------------------ | ------------------------------------------- | --------------------------------------------- |
| Moved out of the unit    | Transfers record to new unit                | `removed`                                     |
| Moved, address unknown   | Sends record to headquarters' unknown file  | `removed`                                     |
| Deceased                 | Records death                               | `removed`                                     |
| Moved within the unit    | Updates address                             | `changed` (address fields)                    |
| Contact info wrong       | Updates phone / email / address             | `changed` (those fields)                      |
| Asked to be removed      | Processes removal request                   | `removed`                                     |

So nothing new has to be detected. The design connects what already
exists: a referral the user has reported to the clerk is checked
against each import, and closed or flagged automatically.

---

## 2. Schema additions

### 2.1 `referrals` table

New table, added to `schema.sql` with `CREATE TABLE IF NOT EXISTS`. The
schema batch already runs on every open, so existing databases pick it up
with no migration function.

```sql
CREATE TABLE IF NOT EXISTS referrals (
    id              INTEGER PRIMARY KEY,
    -- Durable identity: keyed to household_uid, NOT households.id, and
    -- with NO foreign key. The whole point of a referral is that its
    -- household is expected to disappear (removed on import → soft-
    -- deleted → eventually pruned by retention). The referral must
    -- outlive all of that, and must survive Replace/Merge/Link, which
    -- regenerate households.id but carry household_uid forward (#69).
    household_uid   TEXT NOT NULL,
    -- Snapshot at creation time, so the referral list, the clerk PDF and
    -- the history all still read correctly after the household row is
    -- gone. Display only; never used for matching.
    snapshot_name   TEXT NOT NULL,     -- "Smith, John & Mary"
    snapshot_address TEXT,             -- one-line address as it was in the directory
    kind            TEXT NOT NULL CHECK (kind IN (
                        'moved_known',      -- moved, new address known
                        'moved_unknown',    -- moved, whereabouts unknown
                        'deceased',
                        'contact_info',     -- phone / email / address wrong
                        'remove_records',   -- member asked to have records removed
                        'other')),
    details         TEXT,              -- free text: new address, date of death, who told us
    new_address     TEXT,              -- optional, moved_known only; used for §3.3 matching
    learned_on      TEXT NOT NULL,     -- ISO date we found out
    visit_uid       TEXT,              -- the visit that surfaced it, if any (no FK, same reasoning)
    status          TEXT NOT NULL DEFAULT 'to_report' CHECK (status IN (
                        'to_report',        -- recorded, not yet sent to the clerk
                        'reported',         -- sent; waiting for the directory to reflect it
                        'confirmed',        -- an import showed the change
                        'cancelled')),      -- withdrawn by the user (mistaken, moved back, etc.)
    reported_on     TEXT,              -- ISO date of the clerk PDF that included it
    resolved_on     TEXT,              -- ISO date confirmed or cancelled
    resolution_note TEXT,              -- "Removed in import of 2026-11-18", or user's cancel reason
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_referrals_household_uid ON referrals(household_uid);
CREATE INDEX IF NOT EXISTS idx_referrals_status        ON referrals(status);
```

The display reference is `R-` plus the id zero-padded to four digits
(`R-0007`). It appears on the clerk PDF so the user and the clerk can
refer to an item by number in email.

### 2.2 "Overdue" is computed, not stored

A `reported` referral is **overdue** when its household has been
present and unchanged in at least **N committed imports** dated after
`reported_on` (setting `referralOverdueImports`, default **2**).

This counts imports rather than days on purpose. At weekly imports, N = 2
means roughly two weeks. A fortnight of travel with no import does not
make anything overdue by itself, because overdue means the directory
was checked and the change was still not there. Nothing goes overdue
just because time passed without an import.

`import_batches` already records `imported_at` and `status`, so this
needs no new column.

---

## 3. Flows

### 3.1 Creating a referral

Three entry points, all opening the same small modal (kind, details, new
address if `moved_known`, learned-on date defaulting to today):

1. **Household row / detail modal**: a "Report to clerk…" action next to
   the existing visit and tag actions in `households-view.js`.
2. **Map popup**: same action, for use in the field.
3. **After recording a visit** (Stage 2, once visits have an outcome, see
   §5): choosing outcome *Moved* or *Deceased* offers "Create a clerk
   referral?" with the kind and the visit linked.

A household can have more than one open referral (e.g. `contact_info` now,
`moved_known` months later). Creating a second referral of the **same
kind** while one is still open shows a warning, not a block.

### 3.2 Reporting to the clerk: the PDF

The **Clerk Referrals** view (§4) has an **Export for Clerk** button. It
produces a PDF built client-side with pdfmake, following the same pattern as
`directory-pdf.js` and the route export in `map-view.js`. The user attaches
it to an email to the clerk.

```
Record Update Requests                               Prepared 2026-11-11
<Unit name from profile>

NEW REQUESTS (3)

R-0012  Smith, John & Mary          123 Oak St, Winchester VA
        MOVED — new address known
        New address: 45 Elm Rd, Front Royal VA 22630
        Learned 2026-11-08 (visit). Neighbor confirmed; moved in October.

R-0013  Jones, Alice                9 Pine Ct, Winchester VA
        DECEASED
        Learned 2026-11-09. Obituary, Winchester Star, 2026-10-30.

R-0014  Brown, Robert               (no address on file)
        CONTACT INFO — phone on file disconnected
        ...

FOLLOW-UP — PREVIOUSLY SENT, NOT YET REFLECTED IN DIRECTORY (1)

R-0007  Garcia, Luis                77 Maple Ave, Winchester VA
        MOVED — address unknown
        First sent 2026-10-14. Still listed in directory as of 2026-11-10.
```

- **New requests** = all `to_report`.
- **Follow-up** = all overdue `reported` (§2.2). It can be left off
  with a checkbox before export.
- The PDF contains member names and addresses. It goes to the clerk,
  who already holds this data, and nothing is sent by the app itself.
  The user's email client does the sending, the same way route PDFs
  leave the app today.

**After the download**, the app asks: *"Mark 3 new requests as
reported today?"* Yes sets `status = 'reported'` and `reported_on =
today` on exactly the items that were in the PDF. Follow-up items keep
their original `reported_on` so the "first sent" date stays truthful. The
question is asked rather than done silently, because a download is
not proof the email was sent.

A **Copy Text** variant of the same content (as the route export has)
covers the case where the user would rather paste into the email body.

### 3.3 Confirmation on import

This happens in two places in `commands/import.rs`.

**At diff time (`run_diff`)**, for each `removed` or `changed` review item, look up
open (`reported` or `to_report`) referrals on that household's
`household_uid`. Attach them to the `ReviewItem` returned by
`get_review_queue` so Review Updates can show them:

```
REMOVED   Smith, John & Mary — 123 Oak St
          ✔ Matches clerk referral R-0012 (Moved, reported 2026-11-11)
          [ Delete ]  [ Ignore ]
```

**At resolution (`resolve_review_item`)**, inside the same transaction:

| Review item | User resolves as        | Referral kind                                   | Effect on referral                                               |
| ----------- | ----------------------- | ----------------------------------------------- | ---------------------------------------------------------------- |
| `removed`   | Delete                  | any                                             | → `confirmed`, note "Removed from directory, import of <date>"   |
| `changed`   | Replace / Merge         | `moved_known`, address now matches `new_address` | → `confirmed`, note "Address updated, import of <date>"         |
| `changed`   | Replace / Merge         | `contact_info`, and phone/email/address changed | → `confirmed`                                                    |
| `changed`   | Replace / Merge         | `moved_known`, address changed but ≠ new_address | **not** auto-confirmed. Review shows "address changed, but not to the reported one" and offers Confirm / Leave open |
| any         | Ignore                  | any                                             | no change                                                        |

`deletion_reason` on the soft-deleted household is pre-filled from the
referral ("Moved — clerk referral R-0012"), so the deleted-records view
also explains why the record went away.

Address matching for `new_address` uses the same normalisation as
`address_key`. A fuzzy miss falls through to the "not auto-confirmed"
row, which is safe because the user decides.

A `to_report` referral that gets confirmed (the clerk heard from
someone else first) is allowed. It goes straight to `confirmed` with
`reported_on` left NULL.

### 3.4 Cancelling

Any open referral can be cancelled with a reason (mistaken identity, they
moved back, clerk says no action needed). Cancelled and confirmed referrals
stay in the database permanently as history. The table is small (dozens
per year), so no retention pruning is needed.

---

## 4. UI placement

### 4.1 Clerk Referrals view (new sidebar item)

```
Clerk Referrals
[ To report (3) ] [ Awaiting clerk (5) ] [ Overdue (1) ] [ Closed ]      [ Export for Clerk ]

 Ref     Household               Kind              Learned     Reported    Imports since
 R-0007  Garcia, Luis            Moved – unknown   2026-10-12  2026-10-14  4   ⚠ overdue
 R-0010  Lee, Ann                Contact info      2026-10-30  2026-11-01  1
 ...
```

- **Awaiting clerk** = `reported` and not yet overdue. **Overdue** is
  its own tab because it's the actionable one.
- "Imports since" shows the count behind §2.2, so the warning is
  explainable at a glance.
- Row click shows details, with Edit / Cancel / Mark reported (manual
  override, for referrals sent by phone or in person).

### 4.2 Where open referrals show up elsewhere

- **Households view**: a small badge on households with an open
  referral (e.g. "Clerk: reported"), and a filter "Has open referral".
- **Map**: households with an open `moved_*`, `deceased` or
  `remove_records` referral are **excluded from visit-list generation**
  by default, the same way `do_not_contact` is excluded today in
  `visits.rs` / `map_data.rs`. There is no reason to route to a house we
  have already established the member doesn't live in.
- **Dashboard**: a single line "Clerk referrals: 3 to report · 1 overdue"
  linking to the view, shown only when either count is non-zero.
- **After an import is committed**: the summary includes "2 clerk
  referrals confirmed, 1 now overdue."

---

## 5. Relationship to `Visit_Reporting_Design.md`

That document proposes `visits.outcome` (its §2.1, Stage 2) and a
Needs Attention report (its §3.1). This design depends on neither for
its first stage and builds on both later:

- **Outcome values.** Add `'deceased'` to the proposed CHECK list. `moved`
  is already there. Choosing either in the visit modal triggers the
  referral prompt in §3.1(3).
- **Its open question 5** ("Is `moved` acted on automatically at the next
  import, or only recorded?") is answered here: a *visit outcome* is
  only recorded, and the *referral* is what the next import acts on. Keeping
  them separate means a "moved" visit outcome that turns out to be wrong
  never deletes anything, because the user still resolves the import.
- **Revisit timing** is handled there by per-household contact plans
  (its §2.3) and the Needs Attention report. It is a computed query, not a
  tag, and is **out of scope here** to avoid two designs for one feature.
- **Not known → Known.** Once outcomes exist, a contacting visit on a *Not
  known* household prompts to switch the tag and pick a contact plan (that
  doc's §5, Recording a visit).
- **Contact plan is never printed on the clerk PDF** (§3.2). See that doc's
  §2.3 for which exports may include it.

---

## 6. Delivery stages

**Stage 1: referrals and the clerk PDF.** The `referrals` table, the
create modal (household row + map popup), the Clerk Referrals view, Export
for Clerk with "mark reported" and Copy Text, and Cancel. Manual
confirmation only. Useful immediately, because it replaces whatever the user
is tracking in their head or in comments today.

*Implemented (v1.4.0, unreleased):* `commands/referrals.rs`,
`referrals-view.js`, `referrals-pdf.js`. The Clerk Referrals view has To
report / Awaiting clerk / Closed tabs, with Edit, Mark reported, Confirm,
Cancel and Reopen per row. Reopen was added for closing one by mistake.
"Report to clerk…" is in the household detail modal, which also lists
the household's referrals, and in the map popup, with a household picker
when a marker groups several entries. The PDF header uses the profile
name as the unit name. The PDF's follow-up section waits for Stage 2's
overdue computation.

**Stage 2: import confirmation.** §3.3: annotate review items, confirm
on resolution, pre-fill `deletion_reason`, and the overdue computation and
tab. Add the post-import summary line. This stage closes the loop.

**Stage 3: routing and visibility.** Exclude open move/death referrals
from visit-list generation, the households badge and filter, the dashboard
line.

**Stage 4: visit outcomes.** Done together with Stage 2 of
`Visit_Reporting_Design.md`, not separately: add `deceased` to the outcome
list, and add the referral prompt and the Known prompt to the visit modal.

Each stage ships on its own. Stages 1–3 need no change to existing tables.

### 6.1 Tests

- `referrals` survive Replace / Merge / Link (keyed by `household_uid`).
- `referrals` survive soft-delete and retention pruning of the household.
- Overdue counts only committed batches, not pending or discarded ones.
- Each row of the §3.3 table, including the address-changed-but-not-to-
  the-reported-one case.
- Backup/restore round-trip includes the new table (it's in the same
  SQLCipher file, so this should be free; worth one assertion).

---

## 7. Questions to settle before Stage 1

1. **Kinds list.** Is the six-value `kind` list right for how the clerk
   thinks about these? In particular, is "Moved within the unit" worth
   separating from `moved_known`, or does `changed` vs `removed` on import
   already tell them apart well enough? (Proposed: keep one `moved_known`.)
2. **Overdue threshold.** Is 2 imports the right default, given clerks
   sometimes batch their record work monthly? (Proposed: 2, as a setting.)
3. **Unit name on the PDF.** Take it from the profile name, or a
   separate setting? (Proposed: profile name, editable in settings.)
4. **Should exporting automatically mark referrals reported**, skipping
   the confirmation prompt? (Proposed: no. A download isn't a sent email.)
