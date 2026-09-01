registerView('households', {
    init() {
        document.getElementById('householdsRoot').innerHTML = `
            <h1>Households</h1>
            <div class="households-toolbar">
                <input type="text" id="hhSearchInput" placeholder="Search name, address, comments…" />
                <div id="hhTagFilterDropdown"></div>
                <button class="btn" id="hhBulkTagBtn">Tag all results…</button>
            </div>
            <div id="hhResultsMeta"></div>
            <table id="hhTable">
                <thead><tr><th>Name</th><th>Address</th><th>Tag</th><th></th></tr></thead>
                <tbody id="hhTableBody"></tbody>
            </table>
            <div id="hhPager"></div>
        `;
        state.page = 1;
        document.getElementById('hhSearchInput').addEventListener('input', debounce(() => { state.page = 1; loadHouseholds(); }, 300));
        // Tags are capped at one per household now, so filtering by more
        // than one at once would always return nothing — single-select,
        // not the old multi-chip filter.
        state.tagFilterDropdown = mountDropdown(document.getElementById('hhTagFilterDropdown'), {
            items: [{ value: '', label: 'All' }],
            value: '',
            onSelect: (val) => {
                state.tagFilter = val || null;
                state.page = 1;
                loadHouseholds();
            },
        });
        document.getElementById('hhBulkTagBtn').addEventListener('click', bulkTagResults);
    },
    async onShow() {
        const settings = await Api.getSettings().catch(() => ({}));
        state.pageSize = parseInt(settings.pageSize || '25', 10) || 25;
        await refreshTagFilterOptions();
        await loadHouseholds();
    },
});

const state = { page: 1, pageSize: 25, tagFilter: null, lastResult: null };

function debounce(fn, ms) { let t; return (...a) => { clearTimeout(t); t = setTimeout(() => fn(...a), ms); }; }

// "Lastname, First1[ & First2]" — mimics the source directory's own
// header-line format for a household entry.
function formatDirectoryName(h) {
    let out = `${h.last_name}, ${h.first_name}`;
    if (h.first_name_2) out += ` & ${h.first_name_2}`;
    return out;
}

async function refreshTagFilterOptions() {
    const tags = await Api.listTags().catch(() => []);
    state.tagFilterDropdown?.setItems([
        { value: '', label: 'All' },
        ...tags.map(t => ({ value: t.name, label: `${t.name} (${t.household_count})` })),
    ]);
}

async function loadHouseholds() {
    const query = document.getElementById('hhSearchInput').value;
    const params = { query, tag_names: state.tagFilter ? [state.tagFilter] : [], page: state.page, page_size: state.pageSize };
    let result;
    try { result = await Api.searchHouseholds(params); } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); return; }
    state.lastResult = result;

    document.getElementById('hhResultsMeta').textContent = `${result.total} household record(s)`;
    document.getElementById('hhTableBody').innerHTML = result.households.map(h => `
        <tr class="hh-row" data-open="${h.id}">
            <td>${escapeHtml(formatDirectoryName(h))}</td>
            <td>${escapeHtml(h.address_line1)}${h.city ? ', ' + escapeHtml(h.city) : ''}</td>
            <td>${renderTagChips(h.tags)}</td>
            <td><button class="btn" data-known="${h.id}">Known</button></td>
        </tr>`).join('');

    document.querySelectorAll('[data-open]').forEach(tr => tr.addEventListener('click', () => openHouseholdModal(Number(tr.dataset.open))));
    document.querySelectorAll('[data-known]').forEach(btn => btn.addEventListener('click', (e) => {
        e.stopPropagation(); // don't also trigger the row's open-modal click
        markKnown(Number(btn.dataset.known));
    }));

    const totalPages = Math.max(1, Math.ceil(result.total / state.pageSize));
    document.getElementById('hhPager').innerHTML = `
        <button class="btn" id="hhPrevPage" ${state.page <= 1 ? 'disabled' : ''}>‹ Prev</button>
        Page ${state.page} / ${totalPages}
        <button class="btn" id="hhNextPage" ${state.page >= totalPages ? 'disabled' : ''}>Next ›</button>`;
    document.getElementById('hhPrevPage')?.addEventListener('click', () => { state.page--; loadHouseholds(); });
    document.getElementById('hhNextPage')?.addEventListener('click', () => { state.page++; loadHouseholds(); });
}

// Clears whatever tag was there and sets "Known". If the active filter
// isn't "Known" (or "All"), this household no longer matches the current
// view, so it should visibly drop out rather than sit there mistagged.
async function markKnown(id) {
    try {
        await Api.tagHouseholds([id], 'Known');
        if (state.tagFilter && state.tagFilter !== 'Known') {
            document.querySelector(`tr[data-open="${id}"]`)?.remove();
        } else {
            await loadHouseholds();
        }
        await refreshTagFilterOptions();
    } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); }
}

async function bulkTagResults() {
    const name = prompt('Tag name to apply to ALL matching results (replaces any existing tag on each):');
    if (!name) return;
    const params = {
        query: document.getElementById('hhSearchInput').value,
        tag_names: state.tagFilter ? [state.tagFilter] : [],
        page: 1, page_size: 1,
    };
    try {
        const count = await Api.bulkTagSearchResults(params, name);
        showMessage(`Tagged ${count} household(s) with "${name}".`, CONSTANTS.MESSAGE_TYPES.INFO);
        await refreshTagFilterOptions();
        await loadHouseholds();
    } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); }
}

// Household detail modal — mostly read-only, matching the source
// directory's own layout. Only comments, tags, and visits are editable;
// name/address/phone corrections happen through re-import + Review, not
// here (deletes too — see Review Updates).
async function openHouseholdModal(id) {
    const h = await Api.getHousehold(id).catch(e => { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); return null; });
    if (!h) return;

    const addressLines = [h.address_line1, h.address_line2].filter(Boolean);
    const cityLine = [h.city, h.state].filter(Boolean).join(' ') + (h.zip ? ' ' + h.zip : '');
    const latLon = (h.latitude != null && h.longitude != null) ? `${h.latitude}, ${h.longitude}` : null;

    const overlay = document.createElement('div');
    overlay.className = 'modal-overlay';
    overlay.innerHTML = `
        <div class="modal hh-detail-modal">
            <div class="hh-detail-scroll">
                <h2>${escapeHtml(formatDirectoryName(h))}</h2>

                <div class="hh-detail-head">
                    <strong>${escapeHtml(h.first_name)} ${escapeHtml(h.last_name)}</strong>
                    ${h.phone_1 ? `<div>${escapeHtml(h.phone_1)}</div>` : ''}
                    ${h.email_1 ? `<div>${escapeHtml(h.email_1)}</div>` : ''}
                </div>
                ${h.first_name_2 ? `
                <div class="hh-detail-head">
                    <strong>${escapeHtml(h.first_name_2)} ${escapeHtml(h.last_name_2 || '')}</strong>
                    ${h.phone_2 ? `<div>${escapeHtml(h.phone_2)}</div>` : ''}
                    ${h.email_2 ? `<div>${escapeHtml(h.email_2)}</div>` : ''}
                </div>` : ''}
                ${h.has_minors ? '<div class="hh-minors-marker">&lt;Minor Children&gt;</div>' : ''}

                <div class="hh-detail-address">
                    ${addressLines.map(l => `<div>${escapeHtml(l)}</div>`).join('')}
                    ${cityLine.trim() ? `<div>${escapeHtml(cityLine.trim())}</div>` : ''}
                    ${latLon ? `<div class="hh-latlon">${escapeHtml(latLon)}</div>` : ''}
                </div>

                <h3>Tags</h3>
                <div id="modalTags">${renderTagChips(h.tags, { onRemove: true })}</div>
                <div id="modalTagDropdown" class="inline-dropdown"></div>

                <h3>Comments</h3>
                <textarea id="fComments" rows="3">${escapeHtml(h.comments || '')}</textarea>
                <button class="btn" id="fSaveComments">Save Comments</button>

                <h3>Visit History</h3>
                <div id="hhVisitHistory" class="hh-visit-history"><em>Loading…</em></div>
            </div>

            <div class="hh-detail-fixed">
                <h3>Record New Visit</h3>
                <label>Date (YYYY-MM-DD) <input type="text" id="fVisitDate" placeholder="YYYY-MM-DD" value="${new Date().toISOString().slice(0,10)}"></label>
                <label>Comments <textarea id="fVisitComments" rows="2"></textarea></label>
                <div class="modal-buttons">
                    <button class="btn btn-primary" id="fAddVisit">Save Visit</button>
                    <button class="btn" id="fCancelVisit">Cancel</button>
                </div>

                <hr>
                <div class="modal-buttons">
                    <button class="btn" id="fClose">Close</button>
                </div>
            </div>
        </div>`;
    document.body.appendChild(overlay);
    overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });
    document.getElementById('fClose').addEventListener('click', () => overlay.remove());

    // Tags: dropdown of existing tags only — no create-new-tag UI exists
    // anywhere now that the Tags management page is gone (see PR notes;
    // flagging this as a real gap, not silently patching around it).
    const allTags = await Api.listTags().catch(() => []);
    mountDropdown(document.getElementById('modalTagDropdown'), {
        items: allTags.map(t => ({ value: t.name, label: t.name })),
        staticLabel: '+ set tag',
        onSelect: async (name) => {
            if (!name) return;
            await Api.tagHouseholds([id], name);
            const fresh = await Api.getHousehold(id);
            document.getElementById('modalTags').innerHTML = renderTagChips(fresh.tags, { onRemove: true });
            wireTagRemoval();
        },
    });
    wireTagRemoval();
    function wireTagRemoval() {
        document.querySelectorAll('#modalTags [data-remove-tag]').forEach(el => {
            el.addEventListener('click', async () => {
                const tagName = el.dataset.removeTag;
                const t = allTags.find(t => t.name === tagName);
                if (t) { await Api.untagHousehold(id, t.id); el.closest('.tag-chip').remove(); }
            });
        });
    }

    document.getElementById('fSaveComments').addEventListener('click', async () => {
        try {
            await Api.updateHouseholdComments(id, document.getElementById('fComments').value || null);
            showMessage('Comments saved.', CONSTANTS.MESSAGE_TYPES.INFO);
        } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); }
    });

    await refreshVisitHistory(id);
    document.getElementById('fCancelVisit').addEventListener('click', () => {
        document.getElementById('fVisitDate').value = new Date().toISOString().slice(0, 10);
        document.getElementById('fVisitComments').value = '';
    });
    document.getElementById('fAddVisit').addEventListener('click', async () => {
        const date = document.getElementById('fVisitDate').value.trim();
        if (!/^\d{4}-\d{2}-\d{2}$/.test(date)) { showMessage('Enter the date as YYYY-MM-DD.', CONSTANTS.MESSAGE_TYPES.ERROR); return; }
        try {
            await Api.recordVisit(id, date, document.getElementById('fVisitComments').value || null);
            document.getElementById('fVisitComments').value = '';
            await refreshVisitHistory(id);
            showMessage('Visit recorded.', CONSTANTS.MESSAGE_TYPES.INFO);
        } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); }
    });
}

async function refreshVisitHistory(householdId) {
    const el = document.getElementById('hhVisitHistory');
    if (!el) return;
    let visits;
    try { visits = await Api.getHouseholdVisits(householdId); }
    catch (e) { el.innerHTML = `<em>Could not load visits: ${escapeHtml(String(e))}</em>`; return; }

    el.innerHTML = visits.length
        ? visits.map(v => `
            <div class="hh-visit-entry">
                <div class="hh-visit-date">${escapeHtml(v.visit_date)}</div>
                <div class="hh-visit-comments">${escapeHtml(v.comments || '')}</div>
            </div>`).join('')
        : '<em>No visits recorded yet.</em>';
}
