# Lost Sheep — Visit Reporting Design

Status: proposal, for discussion
Companion to issue: "No report answers 'who has not been visited?'"

---

## 1. The problem this document addresses

Lost Sheep can currently answer one reporting question: *what visits happened
between two dates?* That is `visits-report-view.js` and it works.

It cannot answer the questions the application exists for:

- Who have we never been to?
- Who have we not been to in a long time?
- Are we actually covering the congregation, or circling the same streets?
- When we did go, did we make contact, or knock on an empty house?

The last one is not a reporting gap but a schema gap, and it constrains
everything else, so it is addressed first.

### 1.1 Gap analysis

| #   | Gap                                              | Where it lives                                     | Consequence                                                                                             |
| --- | ------------------------------------------------ | -------------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| G1  | No outcome on a visit                            | `visits` table has only date + free-text comments  | Cannot distinguish contact from attempt. Every effectiveness question needs a human to read prose.      |
| G2  | No never-visited view                            | `get_visits_report` inner-joins from `visits`      | Households with zero visits are invisible to reporting. The named use case.                             |
| G3  | No recency measure                               | No `MAX(visit_date)` query anywhere in the backend | "Not visited since" cannot be asked.                                                                    |
| G4  | Report is visit-anchored, not household-anchored | One row per visit                                  | "Last Visited" sort sorts visit rows, not households. The label misleads; the source comment admits it. |
| G5  | Routing ignores history                          | `generate_visit_list` never reads `visits`         | The tool that picks where to go next does not know where anyone has been.                               |
| G6  | Tag and visit state drift                        | "Not known" is a manual proxy for "not visited"    | Two sources of truth, updated independently.                                                            |
| G7  | No coverage rollup                               | Dashboard cards count categorisation, not contact  | Cannot answer "what fraction have we reached this quarter?"                                             |
| G8  | No export                                        | No PDF or CSV from any visit report                | Nothing can leave the app for a planning meeting.                                                       |
| G9  | History is destructible                          | Visits cascade on household delete                 | Historical reports silently shrink over time.                                                           |
| G10 | Dates unvalidated                                | `visits.visit_date` is free-form `TEXT`            | A malformed date makes a visited household look never-visited.                                          |

G9 and G10 are tracked as their own issues. They are listed because no report
built on top of them is trustworthy until they are fixed — a recency report is
exactly as reliable as the dates it compares.

---

## 2. Schema additions

Two changes. Both are additive; neither breaks existing rows.

### 2.1 `visits.outcome` — the important one

```sql
ALTER TABLE visits ADD COLUMN outcome TEXT NOT NULL DEFAULT 'contacted'
    CHECK (outcome IN ('contacted','no_answer','not_home','refused','moved',
                       'deceased','sent','other'));
ALTER TABLE visits ADD COLUMN method TEXT NOT NULL DEFAULT 'visit'
    CHECK (method IN ('visit','phone','card'));
ALTER TABLE visits ADD COLUMN invited INTEGER NOT NULL DEFAULT 0;  -- invitation extended (activity, meeting, dinner…)
```

`deceased` was added for clerk referrals (`Record_Referral_Design.md` §3.1):
choosing `moved` or `deceased` offers to create a referral.

`method` records how the contact was made. `invited` is a flag on top of
it, not a fourth method, because an invitation is a *purpose*: it can be
given at the door, by phone, or on a card. `sent` is the only outcome a
`card` can have (the outcome selector is hidden for cards).

**What counts as contact (settled):** a row counts as contact, and resets the
contact-plan clock (§2.3), only when `outcome = 'contacted'` **and**
`method IN ('visit','phone')`. Cards, not-home and no-answer attempts are
logged and appear in history, but do not reset the clock. An invitation
counts when it was given in person or by phone, since that row is
already a contacted visit or call.

Six values, deliberately few. The distinction that matters most is
`contacted` versus everything else: it separates "we reached this family" from
"we went and it did not happen". Without it, staleness reporting counts three
fruitless doorstep attempts as three visits and marks the household as
well-covered.

`moved` and `refused` earn their place because they change what should happen
next. `refused` is a candidate for the "Do not contact" category. `moved` (and
`deceased`) are **only recorded** on the visit. What the next import acts on
is the clerk referral the user creates from it (`Record_Referral_Design.md`
§3.3). A mistaken `moved` outcome therefore never deletes anything on its own.

The `DEFAULT 'contacted'` matters for migration. Existing rows predate the
column and there is no way to know what they were, so they must default to
something. `contacted` is the honest choice: it is what the user meant when the
only option was "record a visit", and it is the conservative direction for the
gap analysis (it will not manufacture apparent neglect that did not exist).
Rows created before the migration should be distinguishable — see §6.

**Settled:** `no_answer` / `not_home` do **not** count toward recency by
default. The goal is contact, and a failed attempt hasn't achieved it. The
"Count attempts as visits" checkbox in §3.1 remains for the occasional
question "where have I at least *been*?", default off.

### 2.2 A derived last-visit view

Not a stored column. A view, so it cannot drift:

```sql
CREATE VIEW IF NOT EXISTS household_visit_summary AS
SELECT h.id AS household_id,
       COUNT(v.id)                                        AS visit_count,
       SUM(CASE WHEN v.outcome = 'contacted' THEN 1 ELSE 0 END) AS contact_count,
       MAX(v.visit_date)                                   AS last_visit_date,
       MAX(CASE WHEN v.outcome = 'contacted' AND v.method IN ('visit','phone')
                THEN v.visit_date END)                     AS last_contact_date
FROM households h
LEFT JOIN visits v ON v.household_id = h.id
GROUP BY h.id;
```

The `LEFT JOIN` is the entire fix for G2: a household with no visits appears with
`visit_count = 0` and `last_visit_date = NULL`, which is precisely the set the
user cannot currently see.

A stored `households.last_visited_at` column would be faster. It is not
recommended: it is a second source of truth that must be maintained on insert,
delete, and restore, and G6 is already a live example of what happens when two
representations of the same fact are updated independently. At the documented
ceiling of 10,000 households with an index on `visits(household_id)` — which
`schema.sql` already has — the aggregate is not a performance concern.

### 2.3 `households.contact_plan`: how often we intend to reach out

```sql
ALTER TABLE households ADD COLUMN contact_plan TEXT
    CHECK (contact_plan IS NULL OR contact_plan IN ('monthly','quarterly','as_needed'));
```

Plus the same nullable column on `deleted_households`, so soft-delete and
restore carry it, and carried forward by Replace / Merge / Link like
`comments` is.

**Why this exists.** Some households warrant extra effort: more visits, more
invitations. The obvious way to mark them is an "Active" / "Less active"
label, and that is a judgment about a person that can hurt if they ever see
it. A contact plan records **our commitment** ("we reach out monthly")
rather than **their status**. It still identifies the extra-effort group,
because that group is simply everyone on Monthly. Moving a household from
Monthly to Quarterly when they start attending removes no label from
anyone. The same principle is why "Not known" works: it describes what
*we* know, not what *they* are.

| Plan        | Due when `last_contact_date` is older than |
| ----------- | ------------------------------------------ |
| `monthly`   | 30 days                                    |
| `quarterly` | 91 days                                    |
| `as_needed` | never (for now)                            |
| NULL        | no plan; never due by plan                 |

Decisions:

- **No plan for existing Known households at rollout.** The focus for the
  next year or so is the Not known group. Plans are assigned as households
  move from Not known to Known (§5, Recording a visit). They can also be set
  manually on any household at any time.
- **`as_needed` never comes due** for now. Revisit once there's enough
  history to know whether a yearly check-in is wanted.
- **"Not known" is not a plan.** A household with no contact yet is already
  surfaced by Needs Attention as never-visited, so a "Get acquainted" plan
  would only duplicate the tag.
- **Do not contact stays a separate system tag.** It is the member's wish,
  not our plan, and routing already depends on it (`do_not_contact`
  system key).
- **No review/approval tracking.** The user shares plans with ward council
  informally (§3.5). The app holds only the current plan.

**Where it is shown.** On screen freely: the user is the sole user of the
app. In exports, **only** the Contact Plans PDF (§3.5). It never appears on
the route PDF / Copy Text, the clerk referral PDF, or the directory and
data-validation PDFs. Those go to other audiences (the doorstep, the clerk)
who don't need it.

---

## 3. The report suite

Four reports. One is new and is the point of this document; one exists and is
kept; two are small.

### 3.1 Needs Attention — the primary report

**Question answered:** who should we go to next?

This becomes the default landing view of the reporting section, replacing the
visit log in that role. The visit log answers "what did we do"; this answers
"what should we do", and only one of those is a planning tool.

**Row = one household.** Not one visit. This is the correction for G4.

Columns:

| Column       | Content                         | Notes                                                                            |
| ------------ | ------------------------------- | -------------------------------------------------------------------------------- |
| Household    | `Lastname, First & First`       | Same format as the Households view and the directory PDF                         |
| Address      | Street, city                    | Enough to recognise                                                              |
| Tag          | Current category                | Chip, same rendering as elsewhere                                                |
| Plan         | Contact plan, or blank          | §2.3; "due" marker when past the plan's interval                                 |
| Last visit   | Date, or **Never**              | "Never" set in the accent colour — it is the answer the report exists to surface |
| Days since   | Integer, or blank               | Sortable; the working column                                                     |
| Last outcome | Chip: contacted / no answer / … | Reveals the household visited three times with no answer                         |
| Attempts     | Count since last contact        | Distinguishes neglected from unreachable                                         |
| —            | "Use as seed" button            | The link into routing; see §4                                                    |

Default sort: never-visited first (alphabetically within that group), then by
descending days-since. That ordering is itself the answer to the question.

Controls, one row above the table:

```
[ Tag: All ▾ ]  [ Not visited in: 90 days ▾ ]  [ ☑ Count attempts as visits ]  [ Export PDF ]
```

- **Tag** reuses the existing `mountDropdown` and the same tag list the Dashboard
  and Households views use. Do-not-contact households are excluded
  unconditionally and are not selectable — this report exists to send people to
  doors and must never surface one of those.
- **Not visited in** filters to households past the threshold. Never-visited
  households always appear regardless of the threshold; they cannot be "recent"
  by any definition. Options: 30 / 60 / 90 / 180 / 365 days, "Due per contact
  plan" (§2.3), and "Any".
- **Count attempts as visits** is the §2.1 open question, exposed. Off means
  recency is measured from `last_contact_date`; on, from `last_visit_date`. The
  default should be off — the goal is contact — but it is one checkbox and it
  settles an argument rather than pre-empting it.

Above the table, a single summary line, because the aggregate is often the whole
answer:

> **47 households need attention** — 12 never visited, 35 not visited in over 90
> days. 214 of 261 are current.

**Empty state matters here more than usual.** "No households need attention"
with a count of what is current is a genuinely good outcome and should read like
one, not like a failed query.

### 3.2 Visit Log — keep, extend

The existing report, largely as-is. It answers a real and different question:
what happened in a period. Keep the date range, the sort, and the table.

Changes:

- Add an **Outcome** column.
- Add a **tag filter**, matching every other view.
- Add a summary line: `38 visits across 31 households — 22 contacted, 16 no answer`.
- Add **Export PDF** (G8).
- Fix the sort label. "Last Visited (descending)" describes a per-household
  concept but sorts visit rows; rename to "Date (newest first)".
- Validate the date inputs rather than returning an empty table for a malformed
  range (tracked as its own issue).

The default 90-day window is a good default and should stay.

### 3.3 Coverage Summary — small, high value

**Question answered:** are we actually reaching people?

One compact table, no row-level detail. Per tag, plus a total row:

| Tag       | Households | Never visited | Visited in period | Coverage |
| --------- | ---------- | ------------- | ----------------- | -------- |
| Not known | 180        | 96            | 44                | 24%      |
| Known     | 78         | 2             | 61                | 78%      |
| **Total** | **258**    | **98**        | **105**           | **41%**  |

Period selector: this quarter / this year / last 12 months / custom.

This is the report that goes to a planning meeting, and it is roughly twenty
lines of SQL over the view in §2.2. It also gives the Dashboard's per-tag cards
something meaningful to link to — those cards currently show categorisation
counts, which look like coverage and are not (G7).

### 3.4 Household Visit History — exists, keep

Already implemented in the household detail modal via `get_household_visits`.
Add the outcome chip (and method / invited markers) to each entry. Nothing
else needs to change.

### 3.5 Contact Plans: list and ward council PDF

**Question answered:** who are we committed to reaching, and how often?

A table of every household with a non-NULL `contact_plan`, grouped Monthly /
Quarterly / As needed, sorted by last name within each group. Columns:
household, address, plan, last contact date, due marker. Inline plan change
from the row (same `mountDropdown`).

**Export PDF** produces the list the user takes to ward council for
informal discussion:

```
Contact Plans                                     Confidential — ward council
<Unit name>                                                 Prepared 2026-11-11

MONTHLY (6)
  Household               Last contact    Adjustment
  Smith, John & Mary      2026-10-28      ____________
  ...

QUARTERLY (9) ...
AS NEEDED (14) ...

DO NOT CONTACT — at the member's request (3)
  Household
  Jones, Alice
```

- **Contents are deliberately minimal**: name, plan, last contact date. **No
  household comments, no visit comments, no addresses.** Notes are where
  sensitive details live (health, family circumstances, what was said at the
  door). The council needs the plan, not the notes. Anything else worth
  raising is said in the meeting.
- **The blank Adjustment column** is for marking changes by hand during the
  meeting. The user updates the app afterward, so the app remains the one
  place the plan lives.
- **Do not contact** is listed so no one on the council assigns a visit there
  by mistake, and it is framed as the member's request.
- "Confidential — ward council" header and a prepared-on date, because a
  printed list can travel further than the meeting.
- Filename: `LostSheep-Contact-Plans-<YYYYMMDD>.pdf`.

---

## 4. Closing the loop with routing

This is the part that turns reporting from a record into a tool, and it is the
reason Needs Attention should be the default view.

Each row carries a **Use as seed** button. Pressing it switches to the Dashboard,
sets `MapView.seedGroupKey` / `seedHouseholdId` to that household, applies the
matching tag filter, and generates the visit list — the exact state the user
currently has to reach by finding the household on the map and clicking through
its popup.

That is a modest UI convenience. The larger question is G5: should
`generate_visit_list` itself become visit-aware?

Today it selects the N nearest addresses to a seed and orders them by a
nearest-neighbour walk. Visit history is not consulted, so a household visited
yesterday outranks one never visited a street further on. Three options:

- **A — leave selection alone, annotate the output.** Show days-since-last-visit
  against each stop in the visit list modal and the route PDF. No behaviour
  change, no argument to have, immediately useful. The user sees that stop 4 was
  visited last week and can skip it.
- **B — filter the candidate pool.** Add a "not visited in N days" filter to the
  visit-list generator, so recently-visited households are excluded from
  selection entirely. Simple, predictable, and the user stays in control of N.
- **C — weight selection.** Rank candidates by a combination of distance and
  staleness rather than distance alone. Most powerful; also the one where the
  user can no longer predict why a household was chosen, in a tool whose current
  virtue is that its output is obvious.

Recommended: **A first, then B.** A is purely additive and can ship with the
first stage of the reports. B is a small, explicit, user-controlled change. C
should wait until there is enough real visit history to tell whether it actually
produces better routes than B — and it interacts with the deferred loop
optimisation work, which is a separate open thread.

---

## 5. UI placement

The sidebar currently has six entries: Dashboard, Import, Review Updates,
Households, Visit Report, Deleted Records.

Replace **Visit Report** with **Reports**, a single view containing a tab strip:

```
[ Needs Attention ]  [ Visit Log ]  [ Coverage ]  [ Contact Plans ]
```

Rationale for a tab strip over three sidebar entries: they share a tag filter and
a period concept, they are read together, and three more sidebar items in an app
with six would make the primary navigation top-heavy. The tab strip also makes
the relationship legible — these are three views of the same data, not three
unrelated screens.

Needs Attention is the default tab.

Structural notes, following existing conventions in this codebase:

- New CSS in `src/css/reports-view.css`; no inline styles
  (`Functional_Requirements.md`: "No inline CSS ... Each high-level view/operation
  has its own CSS file").
- Every colour from theme variables. The "Never" accent must be legible in all
  five themes and must not rely on colour alone — the requirement is explicit
  that themes be colour-blind friendly, so "Never" is a word, not a red dot.
- Tag filters use `mountDropdown`, not native `<select>` — native popups ignore
  theme variables on WebKitGTK.
- Date inputs are plain text with a `YYYY-MM-DD` placeholder. The native date
  picker freezes the WebKitGTK webview and is not available.
- PDF export via the vendored `pdfmake`, following `directory-pdf.js`.
  Suggested filenames: `LostSheep-Needs-Attention-<TAG>-<YYYYMMDD>.pdf`,
  `LostSheep-Visit-Log-<FROM>-<TO>.pdf`, `LostSheep-Coverage-<YYYYMMDD>.pdf`.
- A shared `reports-pdf.js` is proposed rather than inline generation, matching
  the `directory-pdf.js` precedent — and noting that the visit-route PDF is
  currently inline in `map-view.js`, which is a known open question. Whatever is
  decided there should apply here too, consistently.

### Recording a visit

The existing modal gains method, outcome and an invitation checkbox:

```
Date (YYYY-MM-DD)  [ 2026-09-04        ]
Method             [ Visit ▾           ]     Visit / Phone / Card
Outcome            [ Contacted ▾       ]     hidden when Method = Card
                   [ ] Invitation extended
Comments           [                   ]
```

Defaults `Visit` / `Contacted`, the common case and the current implicit
meaning. `mountDropdown`s, not native selects.

**Follow-up prompts after saving:**

- **Not known → Known.** When the saved row counts as contact (§2.1) and the
  household is tagged *Not known*, one small dialog follows:

  ```
  You've made contact with the Smith household.
  [x] Mark as Known
  Contact plan  [ None ▾ ]        None / Monthly / Quarterly / As needed
                       [ Skip ]  [ Save ]
  ```

  "Mark as Known" is pre-checked. It is a prompt rather than an automatic
  switch, because the user occasionally reaches someone without really
  getting to know them. Marking Known by hand (the existing toggle in
  `households-view.js`) stays, for people known from church but never visited.
  This closes G6 for the common case without making the tag a derived value:
  the tag still drives imports, map groups and PDF ordering unchanged.
- **Moved / Deceased** offers "Create a clerk referral?" with the kind
  pre-filled and the visit linked (`Record_Referral_Design.md` §3.1).

---

## 6. Delivery stages

Each stage is independently shippable and useful on its own. Do not attempt the
suite in one change.

**Stage 1 — Needs Attention, no schema change.** Build the report against the
existing schema using `MAX(visit_date)` and `COUNT(*)`. No outcome column yet, so
no Last Outcome or Attempts columns and no attempts toggle. This closes G2 and
G3 — the two gaps that matter most — needs no migration, and is useful the day it
ships. Includes the "Use as seed" link (§4) and option A annotation.

*Implemented (branch `feat/needs-attention-report`):* `get_needs_attention`
in `commands/visits.rs` with unit tests; `src/js/views/needs-attention.js`;
the Reports tab strip in `visits-report-view.js` (sidebar entry renamed
"Reports", view key unchanged). The seed button is labelled "Visit around
here" to match the map popup, and it carries the report's tag filter to
the map. Option A was already delivered by #89, which prints each stop's
visit history on the route PDF and Copy Text. Not in Stage 1: Export PDF
(Stage 3) and the outcome-dependent columns (Stage 2).

**Stage 2 — outcome, method, contact plan.** The `visits` columns (§2.1) and
`households.contact_plan` (§2.3), with their migrations (the existing
`migrate_*` pattern in `db/mod.rs` has since become the established
convention); the method / outcome / invitation controls in the visit modal;
the Not known → Known prompt with plan selection; the moved/deceased
referral prompt (if `Record_Referral_Design.md` Stage 1 has shipped); the
Plan column and the columns that depend on outcome. Consider recording the
migration date so pre-migration rows — which were defaulted, not observed —
can be identified later.

*Implemented (v1.5.0):* `db/mod.rs::migrate_stage2` adds `visits.outcome`
/ `method` / `invited`, `households.contact_plan`, and the same columns on
the `deleted_*` mirrors, so soft-delete and restore keep them; the
migration time is in `schema_meta` (`visit_outcome_migrated_at`).
`visits.rs::VisitKind` validates the pair (a card is always `sent`) and
`is_contact()` is the §2.1 rule. Needs Attention measures recency from
the last contact, with a "Count attempts as visits" checkbox, and adds
Plan (with a "due" marker), Last outcome and Attempts. The Visit Log
gains a How column and the contacted / not reached / cards summary. On
the household page: a contact plan picker in the Tags card, method /
outcome / invitation controls on Record New Visit and on visit edit, the
method and outcome in the history and its Copy text, and both post-save
prompts (Moved / Deceased → clerk referral linked to the visit; first
contact with a Not known household → Mark as Known + contact plan).
Replace / Merge / Link carry the plan forward. Not done here: the tag
filter on the Visit Log (left for Stage 3 with the PDF exports).

**Stage 3 — Coverage Summary, Contact Plans tab, and PDF exports.** Includes
the ward council PDF (§3.5) and the "Due per contact plan" filter. All small
once stages 1 and 2 exist. Add a test that the route, clerk, directory and
data-validation PDFs never include `contact_plan`.

**Stage 4 — visit-aware routing (option B).** Behaviour change to an existing
feature; ship separately so it can be evaluated on its own.

**Prerequisites.** Stage 1 can proceed immediately. Before Stage 2, the visit
date validation issue should be fixed (G10) — adding a `CHECK` constraint to a
table that already contains malformed dates will fail the rebuild, and a
migration is the natural moment to do that cleanup. The visit-history cascade
issue (G9) should also be settled before Stage 2, since both change what a
`visits` row is and whether it survives its household.

---

## 7. Decisions (settled 2026-11-11)

1. **`no_answer` / `not_home` do not count toward recency.** Default off,
   with the §3.1 checkbox to include them. Cards don't count either (§2.1).
2. **Needs Attention is the default tab** of Reports.
3. **Visit Log survives** as a tab; it answers a different question.
4. **"Not known" stays** as a manually-held tag. It is no longer kept in sync
   purely by hand: the post-visit prompt (§5) offers the switch to Known at
   the moment of first contact, and assigns a contact plan at the same time.
5. **`moved` is only recorded** on the visit. The next import acts on the
   clerk referral the user creates from it, not on the visit outcome
   (`Record_Referral_Design.md`).
6. **Contact plans** (§2.3): Monthly / Quarterly / As needed; As needed never
   comes due for now; no plans assigned to existing Known households at
   rollout; no council review/approval tracking.
