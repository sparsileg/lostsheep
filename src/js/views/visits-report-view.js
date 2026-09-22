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
registerView('visits-report', {
    init() {
        document.getElementById('visitsReportRoot').innerHTML = `
            <h1>Visit Report</h1>
            <div class="vr-toolbar">
                <label class="vr-date-label">From <input type="text" id="vrDateFrom" placeholder="YYYY-MM-DD"></label>
                <label class="vr-date-label">To <input type="text" id="vrDateTo" placeholder="YYYY-MM-DD"></label>
                <button class="btn btn-primary" id="vrRunBtn">Run Report</button>
                <button class="btn" id="vrExportCsvBtn">Export CSV</button>
                <div id="vrSortDropdown" style="min-width:220px;"></div>
                <label class="vr-group-label"><input type="checkbox" id="vrGroupToggle"> Group by household</label>
            </div>
            <div id="vrResultsMeta"></div>
            <table id="vrTable">
                <thead><tr><th>Household</th><th>Date</th><th>Comments</th></tr></thead>
                <tbody id="vrTableBody"></tbody>
            </table>
        `;

        vrState.sortDropdown = mountDropdown(document.getElementById('vrSortDropdown'), {
            items: [
                { value: 'desc', label: 'Last Visited (descending)' },
                { value: 'asc', label: 'Last Visited (ascending)' },
            ],
            value: 'desc',
            onSelect: (val) => { vrState.sort = val; renderVrRows(); },
        });
        document.getElementById('vrRunBtn').addEventListener('click', runVisitsReport);
        document.getElementById('vrExportCsvBtn').addEventListener('click', exportVisitsCsv);
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
        await runVisitsReport();
    },
});
const VisitsReportView = ViewRegistry['visits-report'];

const vrState = { sort: 'desc', rows: [], groupByHousehold: false };

function isoDate(d) { return d.toISOString().slice(0, 10); }

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
        vrState.rows = await Api.getVisitsReport(dateFrom, dateTo);
    } catch (e) {
        showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
        return;
    }
    renderVrRows();
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

function renderVrRows() {
    const sorted = getSortedVrRows();
    const metaEl = document.getElementById('vrResultsMeta');
    const bodyEl = document.getElementById('vrTableBody');

    if (!sorted.length) {
        metaEl.textContent = '0 visits';
        bodyEl.innerHTML = '<tr><td colspan="3">No visits in this date range.</td></tr>';
        return;
    }

    if (!vrState.groupByHousehold) {
        metaEl.textContent = `${sorted.length} visit${sorted.length === 1 ? '' : 's'}`;
        bodyEl.innerHTML = sorted.map(r => `
            <tr>
                <td>${escapeHtml(r.household_name)}</td>
                <td>${escapeHtml(r.visit_date)}</td>
                <td>${escapeHtml(r.comments || '')}</td>
            </tr>`).join('');
        return;
    }

    const groups = groupRowsByHousehold(sorted);
    metaEl.textContent = `${sorted.length} visit${sorted.length === 1 ? '' : 's'} across ${groups.length} household${groups.length === 1 ? '' : 's'}`;
    bodyEl.innerHTML = groups.map(g => `
        <tr class="vr-group-header">
            <td colspan="3" style="font-weight:bold;">${escapeHtml(g.name)} (${g.rows.length} visit${g.rows.length === 1 ? '' : 's'})</td>
        </tr>
        ${g.rows.map(r => `
            <tr class="vr-group-row">
                <td></td>
                <td>${escapeHtml(r.visit_date)}</td>
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
