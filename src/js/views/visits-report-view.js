// ── Visit Report (issue #4, item #1) ────────────────────────────────────────
// Thin UI over the already-existing backend: Api.getVisitsReport /
// commands::visits::get_visits_report. That query and its Rust command were
// built earlier but never had a view calling them.
//
// Also carries item #5 (last-visited sort) per Stan's direction: rather
// than adding a "last visited" column to the Households table, the sort
// control lives here instead — "Last Visited (ascending)"/"Last Visited
// (descending)" sort the report's own rows by visit date. Worth noting:
// this sorts *visits within the chosen date range*, not "households never
// visited" — a household with zero visits in range won't appear at all,
// since the underlying query is a visit log, not a per-household summary.
//
// Reports view: a tab strip over Needs Attention (needs-attention.js, the
// default tab — Visit_Reporting_Design.md §5), this file's own Visit Log,
// Coverage (coverage-report.js) and Contact Plans (contact-plans.js). The
// sort labels above are now "Date (newest/oldest first)" (§3.2). The view key stays 'visits-report' so nothing else that routes to
// it has to change; only the sidebar label became "Reports".
registerView('visits-report', {
    init() {
        document.getElementById('visitsReportRoot').innerHTML = `
            <h1>Reports</h1>
            <div class="rp-tabs" role="tablist">
                <button type="button" class="rp-tab active" role="tab" data-rp-tab="needs-attention">Needs Attention</button>
                <button type="button" class="rp-tab" role="tab" data-rp-tab="visit-log">Visit Log</button>
                <button type="button" class="rp-tab" role="tab" data-rp-tab="coverage">Coverage</button>
                <button type="button" class="rp-tab" role="tab" data-rp-tab="contact-plans">Contact Plans</button>
            </div>
            <div id="rpPaneNeedsAttention" class="rp-pane"></div>
            <div id="rpPaneCoverage" class="rp-pane" hidden></div>
            <div id="rpPaneContactPlans" class="rp-pane" hidden></div>
            <div id="rpPaneVisitLog" class="rp-pane" hidden>
            <div class="vr-toolbar">
                <label class="vr-date-label">From <input type="text" id="vrDateFrom" placeholder="YYYY-MM-DD"></label>
                <label class="vr-date-label">To <input type="text" id="vrDateTo" placeholder="YYYY-MM-DD"></label>
                <label class="na-label">Tag <div id="vrTagDropdown" class="na-dropdown"></div></label>
                <button class="btn btn-primary" id="vrRunBtn">Run Report</button>
                <button class="btn" id="vrExportCsvBtn">Export CSV</button>
                <button class="btn" id="vrExportPdfBtn">Export PDF</button>
                <div id="vrSortDropdown"></div>
                <label class="vr-group-label"><input type="checkbox" id="vrGroupToggle"> Group by household</label>
            </div>
            <div id="vrResultsMeta"></div>
            <table id="vrTable">
                <thead><tr><th>Household</th><th>Date</th><th>How</th><th>Comments</th></tr></thead>
                <tbody id="vrTableBody"></tbody>
            </table>
            </div>
        `;

        NeedsAttention.mount(document.getElementById('rpPaneNeedsAttention'));
        CoverageReport.mount(document.getElementById('rpPaneCoverage'));
        ContactPlans.mount(document.getElementById('rpPaneContactPlans'));
        document.querySelectorAll('[data-rp-tab]').forEach(btn =>
            btn.addEventListener('click', () => selectReportsTab(btn.dataset.rpTab)));

        vrState.sortDropdown = mountDropdown(document.getElementById('vrSortDropdown'), {
            // Sorts visit rows, so it is named for the date (design §3.2).
            items: [
                { value: 'desc', label: 'Date (newest first)' },
                { value: 'asc', label: 'Date (oldest first)' },
            ],
            value: 'desc',
            sizeToItems: true,
            onSelect: (val) => { vrState.sort = val; renderVrRows(); },
        });
        vrState.tagDropdown = mountDropdown(document.getElementById('vrTagDropdown'), {
            items: [{ value: '', label: 'All' }],
            value: '',
            onSelect: (val) => { vrState.tagId = val; runVisitsReport(); },
        });
        document.getElementById('vrRunBtn').addEventListener('click', runVisitsReport);
        document.getElementById('vrExportCsvBtn').addEventListener('click', exportVisitsCsv);
        document.getElementById('vrExportPdfBtn').addEventListener('click', exportVisitsPdf);
        // Screen-only convenience — the sort control still governs the order
        // of visits inside each household's block; this only decides how
        // those blocks are grouped and ordered (always alphabetical). CSV
        // export deliberately ignores this toggle (Stan's call) and always
        // exports the flat, sorted list — a spreadsheet's own sort/group can
        // do this if wanted, and a flat file is easier to re-import or pivot.
        document.getElementById('vrGroupToggle').addEventListener('change', (e) => {
            vrState.groupByHousehold = e.target.checked;
            renderVrRows();
        });

        // Default window: last 90 days through today — a reasonable
        // starting point the user can widen (e.g. back to 1900-01-01) to
        // effectively see all visits ever recorded.
        const today = new Date();
        const from = new Date(today.getTime() - 90 * 24 * 60 * 60 * 1000);
        document.getElementById('vrDateFrom').value = isoDate(from);
        document.getElementById('vrDateTo').value = isoDate(today);
    },
    async onShow() {
        await refreshReportsTab();
    },
});
const VisitsReportView = ViewRegistry['visits-report'];

const vrState = { sort: 'desc', rows: [], groupByHousehold: false, tagId: '', tagDropdown: null, ran: null };

// Which Reports tab is showing. Not persisted — Needs Attention is the
// landing tab each launch.
let rpActiveTab = 'needs-attention';

function selectReportsTab(tab) {
    rpActiveTab = tab;
    document.querySelectorAll('[data-rp-tab]').forEach(btn => {
        const on = btn.dataset.rpTab === tab;
        btn.classList.toggle('active', on);
        btn.setAttribute('aria-selected', on ? 'true' : 'false');
    });
    document.getElementById('rpPaneNeedsAttention').hidden = tab !== 'needs-attention';
    document.getElementById('rpPaneVisitLog').hidden = tab !== 'visit-log';
    document.getElementById('rpPaneCoverage').hidden = tab !== 'coverage';
    document.getElementById('rpPaneContactPlans').hidden = tab !== 'contact-plans';
    refreshReportsTab().catch(e => showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR));
}

async function refreshReportsTab() {
    if (rpActiveTab === 'needs-attention') await NeedsAttention.refresh();
    else if (rpActiveTab === 'coverage') await CoverageReport.refresh();
    else if (rpActiveTab === 'contact-plans') await ContactPlans.refresh();
    else {
        await refreshVrTagOptions();
        await runVisitsReport();
    }
}

// Every tag, system ones included: this is a record of what happened,
// not a list to send anyone to.
async function refreshVrTagOptions() {
    const tags = await Api.listTags().catch(() => []);
    if (vrState.tagId && !tags.some(t => String(t.id) === vrState.tagId)) vrState.tagId = '';
    vrState.tagDropdown.setItems([
        { value: '', label: 'All' },
        ...tags.map(t => ({ value: String(t.id), label: t.name })),
    ]);
    vrState.tagDropdown.setValue(vrState.tagId);
}

// Local calendar date (core.js), not UTC: toISOString() gave tomorrow's
// date in the evening west of Greenwich.
function isoDate(d) { return localIsoDate(d); }

// Duplicated from households-view.js's isValidIsoDate rather than shared —
// small enough that adding a load-order dependency between the two view
// files isn't worth it. Old check only required non-empty fields, so a
// malformed From/To silently returned an empty table, indistinguishable
// from "no visits in range" (#35). Backend validation in get_visits_report
// is still the load-bearing check; this is fail-fast for the common case.
function isValidIsoDate(str) {
    const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(str);
    if (!m) return false;
    const y = Number(m[1]), mo = Number(m[2]), d = Number(m[3]);
    const dt = new Date(Date.UTC(y, mo - 1, d));
    return dt.getUTCFullYear() === y && dt.getUTCMonth() === mo - 1 && dt.getUTCDate() === d;
}

async function runVisitsReport() {
    const dateFrom = document.getElementById('vrDateFrom').value.trim();
    const dateTo = document.getElementById('vrDateTo').value.trim();
    if (!isValidIsoDate(dateFrom) || !isValidIsoDate(dateTo)) {
        showMessage('Enter both From and To as a real date, YYYY-MM-DD (e.g. 2026-03-05).', CONSTANTS.MESSAGE_TYPES.ERROR);
        return;
    }
    try {
        vrState.rows = await Api.getVisitsReport(dateFrom, dateTo, vrState.tagId ? Number(vrState.tagId) : null);
    } catch (e) {
        showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
        return;
    }
    // What the rows on screen were run with, for the PDF header.
    vrState.ran = { from: dateFrom, to: dateTo, tagLabel: vrTagLabel() };
    renderVrRows();
}

function vrTagLabel() {
    if (!vrState.tagId) return 'All';
    const item = document.querySelector(`#vrTagDropdown [data-value="${CSS.escape(vrState.tagId)}"]`);
    return item ? item.textContent : 'Tag';
}

async function exportVisitsPdf() {
    const rows = getSortedVrRows();
    if (!rows.length || !vrState.ran) {
        showMessage('Nothing to export — run a report with at least one visit first.', CONSTANTS.MESSAGE_TYPES.ERROR);
        return;
    }
    let unitName = '';
    try { unitName = (await Api.getActiveProfile())?.name || ''; } catch (_) { /* optional header line */ }
    ReportsPdf.visitLog(rows, { ...vrState.ran, unitName, summary: vrSummaryText(rows) });
}

// Issue #77: pulled out of renderVrRows so the CSV export below sorts the
// exact same way the on-screen table does — one comparator, not two copies
// that could drift (#22's own reasoning, applied here).
function getSortedVrRows() {
    return vrState.rows.slice().sort((a, b) => {
        if (a.visit_date === b.visit_date) return 0;
        const cmp = a.visit_date < b.visit_date ? -1 : 1;
        return vrState.sort === 'asc' ? cmp : -cmp;
    });
}

// Parses a stored household name ("First [Middle] Last" for one person,
// people joined by " & " for multiple) into "Last, First" display form
// per Stan's spec:
//   "Constance Lynn Miller"            -> "Miller, Constance Lynn"
//   "Jerry Cyril & Arooj Cyril"        -> "Cyril, Jerry & Arooj"
//   "Kelvin Whitmore & Diana Frederick" -> "Whitmore, Kelvin & Frederick, Diana"
// Last name = final whitespace-separated token of each person's name;
// everything before it is the first/middle name(s). When every person
// shares the same last name, it's printed once with first names joined
// by " & "; otherwise each person gets their own "Last, First".
function formatHouseholdName(raw) {
    if (!raw) return raw || '';
    const people = raw.split('&').map(p => p.trim()).filter(Boolean);
    const parsed = people.map(p => {
        const parts = p.split(/\s+/);
        const last = parts.pop();
        return { first: parts.join(' '), last };
    });
    if (parsed.length === 1) {
        return `${parsed[0].last}, ${parsed[0].first}`;
    }
    const sameLast = parsed.every(p => p.last === parsed[0].last);
    if (sameLast) {
        return `${parsed[0].last}, ${parsed.map(p => p.first).join(' & ')}`;
    }
    return parsed.map(p => `${p.last}, ${p.first}`).join(' & ');
}

// Ad hoc request — partitions the already-sorted rows into per-household
// blocks, ordered by last name (Stan's call: grouping is for finding a
// household and scanning its whole history, not another axis of the date
// sort). Grouping key is still the raw household_name (avoids collisions
// if two different raw names formatted the same); display name and sort
// order both use the "Last, First" form from formatHouseholdName, since
// that form already puts the sort-relevant part first.
// `sorted` is expected to already be in the visit-date order the sort
// control specifies, so each group's own rows come out in that same order
// (Map preserves insertion order; sort() below only reorders the groups
// themselves, never the rows inside one).
function groupRowsByHousehold(sorted) {
    const groups = new Map();
    sorted.forEach(r => {
        const key = r.household_name || '';
        if (!groups.has(key)) groups.set(key, []);
        groups.get(key).push(r);
    });
    return [...groups.entries()]
        .map(([name, rows]) => ({ name: formatHouseholdName(name), rows }))
        .sort((a, b) => a.name.localeCompare(b.name));
}

// "38 visits across 31 households — 22 contacted, 9 not reached, 7 cards"
// (Visit_Reporting_Design.md §3.2). Calls count as visits here.
function vrSummaryText(rows) {
    const plural = (n, w) => `${n} ${w}${n === 1 ? '' : 's'}`;
    const households = new Set(rows.map(r => r.household_id)).size;
    const contacted = rows.filter(visitIsContact).length;
    const cards = rows.filter(r => r.method === 'card').length;
    const notReached = rows.length - contacted - cards;
    const parts = [`${contacted} contacted`];
    if (notReached) parts.push(`${notReached} not reached`);
    if (cards) parts.push(plural(cards, 'card'));
    return `${plural(rows.length, 'visit')} across ${plural(households, 'household')} — ${parts.join(', ')}`;
}

function renderVrRows() {
    const sorted = getSortedVrRows();
    const metaEl = document.getElementById('vrResultsMeta');
    const bodyEl = document.getElementById('vrTableBody');

    if (!sorted.length) {
        metaEl.textContent = '0 visits';
        bodyEl.innerHTML = '<tr><td colspan="4">No visits in this date range.</td></tr>';
        return;
    }

    metaEl.textContent = vrSummaryText(sorted);
    const how = r => `<span class="vk-chip ${visitIsContact(r) ? 'vk-chip-contact' : ''}">${escapeHtml(visitKindText(r))}</span>`;

    if (!vrState.groupByHousehold) {
        bodyEl.innerHTML = sorted.map(r => `
            <tr>
                <td>${escapeHtml(r.household_name)}</td>
                <td>${escapeHtml(r.visit_date)}</td>
                <td>${how(r)}</td>
                <td>${escapeHtml(r.comments || '')}</td>
            </tr>`).join('');
        return;
    }

    const groups = groupRowsByHousehold(sorted);
    bodyEl.innerHTML = groups.map(g => `
        <tr class="vr-group-header">
            <td colspan="4" style="font-weight:bold;">${escapeHtml(g.name)} (${g.rows.length} visit${g.rows.length === 1 ? '' : 's'})</td>
        </tr>
        ${g.rows.map(r => `
            <tr class="vr-group-row">
                <td></td>
                <td>${escapeHtml(r.visit_date)}</td>
                <td>${how(r)}</td>
                <td>${escapeHtml(r.comments || '')}</td>
            </tr>`).join('')}
    `).join('');
}

// Issue #77: proper RFC 4180 quoting — a value is wrapped in double quotes
// if it contains a comma, a double quote, or a line break (CR or LF), with
// any embedded double quote doubled. import_csv's own reader
// (commands/import.rs) does a naive line.split(',') with no quote handling
// at all — deliberately not modeling this writer on that reader (see #77's
// own constraints); a comment with a comma or an embedded newline must
// still round-trip correctly into a spreadsheet.
function csvEscape(value) {
    const s = value == null ? '' : String(value);
    if (/[",\r\n]/.test(s)) {
        return '"' + s.replace(/"/g, '""') + '"';
    }
    return s;
}

// Column set comes from whatever fields the row objects actually carry
// (Api.getVisitsReport's own shape), not a hardcoded three columns — so a
// later backend addition (household id, address, etc.) shows up here too
// without this file needing a matching edit. Falls back to the three
// columns the table renders when there are no rows to export at all.
function vrCsvColumns(rows) {
    return rows.length ? Object.keys(rows[0]) : ['household_name', 'visit_date', 'comments'];
}

function downloadCsv(filename, text) {
    // Leading BOM so Excel opens the file as UTF-8 rather than guessing a
    // legacy codepage and mangling any accented name.
    const blob = new Blob(['﻿' + text], { type: 'text/csv;charset=utf-8;' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
}

// Issue #77: exports every row currently matched by the date range, in the
// exact order shown on screen — same getSortedVrRows() call renderVrRows
// uses, same Api.getVisitsReport data already on screen, so there's no
// second query path that could disagree with the table (#22 precedent).
function exportVisitsCsv() {
    const rows = getSortedVrRows();
    if (rows.length === 0) {
        showMessage('Nothing to export — run a report with at least one visit first.', CONSTANTS.MESSAGE_TYPES.ERROR);
        return;
    }
    const columns = vrCsvColumns(rows);
    const lines = [columns.map(csvEscape).join(',')];
    rows.forEach(r => {
        lines.push(columns.map(c => csvEscape(r[c])).join(','));
    });

    const dateFrom = document.getElementById('vrDateFrom').value.trim();
    const dateTo = document.getElementById('vrDateTo').value.trim();
    downloadCsv(`LostSheep-VisitReport-${dateFrom}_to_${dateTo}.csv`, lines.join('\r\n'));
}
