// ── Coverage (Visit_Reporting_Design.md §3.3) ─────────────────────────────
// Are we actually reaching people? One row per tag plus a total, from
// commands::visits::get_coverage. Do-not-contact households are left out.
// "Reached" means contacted (a contacted visit or call), unless "Count
// attempts as visits" is ticked, matching Needs Attention.
//
// A tab of the Reports view (visits-report-view.js), which calls
// CoverageReport.mount() once and refresh() whenever the tab is shown.

const cvState = { period: 'quarter', countAttempts: false, result: null, from: '', to: '', periodDropdown: null };

const CV_PERIODS = [
    { value: 'quarter', label: 'This quarter' },
    { value: 'year', label: 'This year' },
    { value: '12m', label: 'Last 12 months' },
    { value: 'custom', label: 'Custom' },
];

// [from, to] for a preset, as local YYYY-MM-DD; null for custom.
function cvPeriodRange(period, today = new Date()) {
    const y = today.getFullYear(), m = today.getMonth();
    if (period === 'quarter') return [localIsoDate(new Date(y, m - (m % 3), 1)), localIsoDate(today)];
    if (period === 'year') return [localIsoDate(new Date(y, 0, 1)), localIsoDate(today)];
    if (period === '12m') {
        const from = new Date(today);
        from.setFullYear(y - 1);
        from.setDate(from.getDate() + 1);
        return [localIsoDate(from), localIsoDate(today)];
    }
    return null;
}

function cvReachedWord() { return cvState.countAttempts ? 'visited' : 'contacted'; }

function cvPct(r) { return r.households ? `${Math.round(100 * r.reached_in_period / r.households)}%` : '—'; }

const CoverageReport = {
    mount(container) {
        container.innerHTML = `
            <div class="na-toolbar">
                <label class="na-label">Period <div id="cvPeriodDropdown" class="na-dropdown"></div></label>
                <label class="vr-date-label">From <input type="text" id="cvFrom" placeholder="YYYY-MM-DD"></label>
                <label class="vr-date-label">To <input type="text" id="cvTo" placeholder="YYYY-MM-DD"></label>
                <button class="btn" id="cvRunBtn">Run</button>
                <label class="na-check"><input type="checkbox" id="cvCountAttempts"> Count attempts as visits</label>
                <button class="btn" id="cvPdfBtn">Export PDF</button>
            </div>
            <div class="na-summary" id="cvSummary"></div>
            <table id="cvTable" class="cv-table">
                <thead><tr>
                    <th>Tag</th><th class="na-num">Households</th><th class="na-num" id="cvNeverHead">Never contacted</th>
                    <th class="na-num" id="cvReachedHead">Contacted in period</th><th class="na-num">Coverage</th>
                </tr></thead>
                <tbody id="cvTableBody"></tbody>
            </table>
        `;
        cvState.periodDropdown = mountDropdown(document.getElementById('cvPeriodDropdown'), {
            items: CV_PERIODS, value: cvState.period, sizeToItems: true,
            onSelect: (v) => { cvState.period = v; CoverageReport.applyPeriod(); CoverageReport.refresh(); },
        });
        const runOnEnter = (e) => { if (e.key === 'Enter') CoverageReport.runCustom(); };
        ['cvFrom', 'cvTo'].forEach(id => document.getElementById(id).addEventListener('keydown', runOnEnter));
        document.getElementById('cvRunBtn').addEventListener('click', () => CoverageReport.runCustom());
        document.getElementById('cvCountAttempts').addEventListener('change', (e) => {
            cvState.countAttempts = e.target.checked;
            CoverageReport.refresh();
        });
        document.getElementById('cvPdfBtn').addEventListener('click', () => CoverageReport.exportPdf());
        CoverageReport.applyPeriod();
    },

    // Fills From/To from the preset; editing either switches to Custom.
    applyPeriod() {
        const range = cvPeriodRange(cvState.period);
        if (range) {
            document.getElementById('cvFrom').value = range[0];
            document.getElementById('cvTo').value = range[1];
        }
    },

    runCustom() {
        const range = cvPeriodRange(cvState.period);
        const from = document.getElementById('cvFrom').value.trim();
        const to = document.getElementById('cvTo').value.trim();
        if (range && (range[0] !== from || range[1] !== to)) {
            cvState.period = 'custom';
            cvState.periodDropdown.setValue('custom');
        }
        CoverageReport.refresh();
    },

    async refresh() {
        const from = document.getElementById('cvFrom').value.trim();
        const to = document.getElementById('cvTo').value.trim();
        if (!isValidIsoDate(from) || !isValidIsoDate(to)) {
            showMessage('Enter both From and To as a real date, YYYY-MM-DD (e.g. 2026-03-05).', CONSTANTS.MESSAGE_TYPES.ERROR);
            return;
        }
        try {
            cvState.result = await Api.getCoverage(from, to, cvState.countAttempts);
        } catch (e) {
            showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
            return;
        }
        cvState.from = from;
        cvState.to = to;
        renderCoverage();
    },

    async exportPdf() {
        if (!cvState.result) { showMessage('Run the report first.', CONSTANTS.MESSAGE_TYPES.ERROR); return; }
        let unitName = '';
        try { unitName = (await Api.getActiveProfile())?.name || ''; } catch (_) { /* optional header line */ }
        ReportsPdf.coverage(cvState.result, {
            unitName, from: cvState.from, to: cvState.to,
            periodLabel: CV_PERIODS.find(p => p.value === cvState.period)?.label || 'Period',
            reachedWord: cvReachedWord(),
        });
    },
};

function renderCoverage() {
    const r = cvState.result;
    const word = cvReachedWord();
    document.getElementById('cvNeverHead').textContent = `Never ${word}`;
    document.getElementById('cvReachedHead').textContent = `${word[0].toUpperCase()}${word.slice(1)} in period`;
    const t = r.total;
    document.getElementById('cvSummary').textContent = t.households
        ? `${t.reached_in_period} of ${t.households} households ${word} between ${cvState.from} and ${cvState.to} (${cvPct(t)}). ${t.never} never ${word}.`
        : 'No households to count.';
    const line = (row, cls = '') => `
        <tr class="${cls}">
            <td>${row.tag ? escapeHtml(row.tag) : '<span class="cv-untagged">(No tag)</span>'}</td>
            <td class="na-num">${row.households}</td>
            <td class="na-num">${row.never}</td>
            <td class="na-num">${row.reached_in_period}</td>
            <td class="na-num">${cvPct(row)}</td>
        </tr>`;
    document.getElementById('cvTableBody').innerHTML =
        r.rows.map(row => line(row)).join('') + line({ ...t, tag: 'Total' }, 'cv-total');
}

window.CoverageReport = CoverageReport;
