// ── Needs Attention (Visit_Reporting_Design.md §3.1, Stage 1) ──────────────
// Who should we go to next? One row per household (not per visit), from
// commands::visits::get_needs_attention. Never-visited households always
// appear; the rest appear once their last visit is older than the chosen
// threshold. Do-not-contact households are excluded by the backend
// unconditionally — this report sends people to doors.
//
// Lives as a tab inside the Reports view (visits-report-view.js), which
// calls NeedsAttention.mount() once from its own init() and
// NeedsAttention.refresh() whenever the tab is shown.
//
// Stage 1 has no visit outcome yet, so every recorded visit counts toward
// recency and there is no Last Outcome / Attempts column.

const naState = {
    tagId: '',          // '' = all households
    threshold: '90',    // days, or 'any'
    sort: 'overdue',    // 'overdue' | 'name'
    result: null,
    tagDropdown: null,
};

const NA_THRESHOLDS = ['30', '60', '90', '180', '365'];

const NeedsAttention = {
    mount(container) {
        container.innerHTML = `
            <div class="na-toolbar">
                <label class="na-label">Tag <div id="naTagDropdown" class="na-dropdown"></div></label>
                <label class="na-label">Not visited in <div id="naThresholdDropdown" class="na-dropdown"></div></label>
                <label class="na-label">Sort <div id="naSortDropdown" class="na-dropdown"></div></label>
            </div>
            <div id="naSummary" class="na-summary"></div>
            <table id="naTable">
                <thead><tr>
                    <th>Household</th><th>Address</th><th>Tags</th>
                    <th>Last visit</th><th>Days since</th><th></th>
                </tr></thead>
                <tbody id="naTableBody"></tbody>
            </table>
        `;

        naState.tagDropdown = mountDropdown(document.getElementById('naTagDropdown'), {
            items: [{ value: '', label: 'All' }],
            value: '',
            onSelect: (val) => { naState.tagId = val; NeedsAttention.refresh(); },
        });
        mountDropdown(document.getElementById('naThresholdDropdown'), {
            items: [
                ...NA_THRESHOLDS.map(n => ({ value: n, label: `${n} days` })),
                { value: 'any', label: 'Any (show all)' },
            ],
            value: naState.threshold,
            onSelect: (val) => { naState.threshold = val; NeedsAttention.refresh(); },
        });
        mountDropdown(document.getElementById('naSortDropdown'), {
            items: [
                { value: 'overdue', label: 'Most overdue first' },
                { value: 'name', label: 'Name (A–Z)' },
            ],
            value: naState.sort,
            onSelect: (val) => { naState.sort = val; renderNaRows(); },
        });
    },

    async refresh() {
        await refreshNaTagOptions();
        const tagId = naState.tagId ? Number(naState.tagId) : null;
        const threshold = naState.threshold === 'any' ? null : Number(naState.threshold);
        try {
            naState.result = await Api.getNeedsAttention(tagId, threshold);
        } catch (e) {
            showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
            return;
        }
        renderNaRows();
    },
};

// System tags (do not contact) are excluded by the backend regardless, so
// offering one here would only ever show an empty table — same reasoning
// as map-view.js's populateMapTagSelect (#23).
async function refreshNaTagOptions() {
    const tags = await Api.listTags().catch(() => []);
    const pickable = tags.filter(t => !t.system_key);
    if (naState.tagId && !pickable.some(t => String(t.id) === naState.tagId)) {
        naState.tagId = ''; // selected tag was deleted since last visit to this tab
    }
    naState.tagDropdown.setItems([
        { value: '', label: 'All' },
        ...pickable.map(t => ({ value: String(t.id), label: t.name })),
    ]);
    naState.tagDropdown.setValue(naState.tagId);
}

// "Last, First & First2" when both heads share a last name (the directory's
// own header format, as households-view.js's formatDirectoryName), else
// "Last, First & Last2, First2" so neither surname is lost.
function naFormatName(r) {
    const first = `${r.last_name}, ${r.first_name}`;
    if (!r.first_name_2) return first;
    if (!r.last_name_2 || r.last_name_2 === r.last_name) return `${first} & ${r.first_name_2}`;
    return `${first} & ${r.last_name_2}, ${r.first_name_2}`;
}

function naSortedRows() {
    const rows = (naState.result?.rows || []).map(r => ({ ...r, display: naFormatName(r) }));
    const byName = (a, b) => a.display.localeCompare(b.display);
    if (naState.sort === 'name') return rows.sort(byName);
    // Never-visited first (alphabetical), then most days since last visit.
    // A household whose last visit date couldn't be parsed (days_since
    // null but visit_count > 0) sorts after everything with a real number.
    return rows.sort((a, b) => {
        const aNever = a.visit_count === 0, bNever = b.visit_count === 0;
        if (aNever !== bNever) return aNever ? -1 : 1;
        if (aNever) return byName(a, b);
        const ad = a.days_since ?? -Infinity, bd = b.days_since ?? -Infinity;
        if (ad !== bd) return bd - ad;
        return byName(a, b);
    });
}

function naSummaryText(result) {
    const n = result.rows.length;
    const plural = (k, word) => `${k} ${word}${k === 1 ? '' : 's'}`;
    if (naState.threshold === 'any') {
        return `${plural(result.total_eligible, 'household')} — ${result.never_count} never visited.`;
    }
    if (n === 0) {
        return `No households need attention — all ${plural(result.total_eligible, 'household')} visited in the last ${naState.threshold} days.`;
    }
    const stale = n - result.never_count;
    const current = result.total_eligible - n;
    return `${plural(n, 'household')} need${n === 1 ? 's' : ''} attention — ` +
        `${result.never_count} never visited, ${stale} not visited in over ${naState.threshold} days. ` +
        `${current} of ${result.total_eligible} are current.`;
}

function renderNaRows() {
    const result = naState.result;
    if (!result) return;
    document.getElementById('naSummary').textContent = naSummaryText(result);

    const bodyEl = document.getElementById('naTableBody');
    const rows = naSortedRows();
    if (!rows.length) {
        bodyEl.innerHTML = `<tr><td colspan="6" class="na-empty">Everyone in this group is current.</td></tr>`;
        return;
    }
    bodyEl.innerHTML = rows.map(r => {
        const address = [r.address_line1, r.city].filter(Boolean).join(', ') || '(no address on file)';
        const tags = r.tags.map(t => `<span class="tag-chip">${escapeHtml(t)}</span>`).join('');
        const lastVisit = r.visit_count === 0
            ? '<span class="na-never">Never</span>'
            : escapeHtml(r.last_visit_date || '');
        const seedBtn = r.has_coords
            ? `<button class="btn btn-small" data-na-seed="${r.household_id}">Visit around here</button>`
            : `<button class="btn btn-small" disabled title="No map coordinates for this address">Visit around here</button>`;
        return `
            <tr>
                <td>${escapeHtml(r.display)}</td>
                <td>${escapeHtml(address)}</td>
                <td>${tags}</td>
                <td>${lastVisit}</td>
                <td class="na-num">${r.days_since == null ? '' : r.days_since}</td>
                <td>${seedBtn}</td>
            </tr>`;
    }).join('');

    bodyEl.querySelectorAll('[data-na-seed]').forEach(btn => btn.addEventListener('click', () => {
        const row = rows.find(r => r.household_id === Number(btn.dataset.naSeed));
        if (row) naUseAsSeed(row);
    }));
}

// Switches to the Dashboard and generates a visit list seeded at this
// household — the same state a map popup's "Visit around here" produces.
// The map's tag filter is set to this report's tag filter, so the seed is
// always inside the candidate pool. map-view.js's onShow() picks up
// pendingSeed after it has reloaded its markers (see there).
function naUseAsSeed(row) {
    MapView.pendingSeed = {
        householdId: row.household_id,
        addressKey: row.address_key,
        tagId: naState.tagId,
    };
    showView('map');
}

window.NeedsAttention = NeedsAttention;
