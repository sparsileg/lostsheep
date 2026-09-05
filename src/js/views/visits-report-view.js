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
                <div id="vrSortDropdown" style="min-width:220px;"></div>
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

const vrState = { sort: 'desc', rows: [] };

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

function renderVrRows() {
    const sorted = vrState.rows.slice().sort((a, b) => {
        if (a.visit_date === b.visit_date) return 0;
        const cmp = a.visit_date < b.visit_date ? -1 : 1;
        return vrState.sort === 'asc' ? cmp : -cmp;
    });
    document.getElementById('vrResultsMeta').textContent = `${sorted.length} visit${sorted.length === 1 ? '' : 's'}`;
    document.getElementById('vrTableBody').innerHTML = sorted.length
        ? sorted.map(r => `
            <tr>
                <td>${escapeHtml(r.household_name)}</td>
                <td>${escapeHtml(r.visit_date)}</td>
                <td>${escapeHtml(r.comments || '')}</td>
            </tr>`).join('')
        : '<tr><td colspan="3">No visits in this date range.</td></tr>';
}
