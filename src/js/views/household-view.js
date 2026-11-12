// ── Household page ──────────────────────────────────────────────────────
// One household, full window: replaced the household detail modal, which
// had outgrown a popup (details, tags, comments, visits, clerk referrals,
// with more to come in Visit_Reporting_Design.md Stage 2).
//
// Not in the sidebar. openHousehold(id) shows it from wherever the user
// is (Households list, Clerk Referrals, a map popup), and ← Back returns
// to that view. Views keep their DOM while hidden, so the Households
// list's search and page are still there on return.
//
// Mostly read-only, matching the source directory's own layout. Only
// comments, tags, visits and referrals are editable here; name/address/
// phone corrections happen through re-import + Review Updates.
//
// Layout: a sticky header (Back, name, address), then two columns —
// left "who they are" (heads, contact info, tags, comments), right "what
// we've done" (record a visit, visit history, clerk referrals). One
// column on a narrow window.

const HouseholdPage = { id: null, returnTo: 'households', savedComments: '' };

function activeViewName() {
    const el = document.querySelector('.view.active');
    return el ? el.id.replace(/^view-/, '') : 'households';
}

function openHousehold(id) {
    if (id == null) return;
    const from = activeViewName();
    if (from !== 'household') HouseholdPage.returnTo = from;
    HouseholdPage.id = id;
    showView('household');
}

// Unsaved comment edits or a typed-but-unsaved visit note. Checked by
// core.js's showView before leaving this view (Back or the sidebar).
function householdPageIsDirty() {
    const comments = document.getElementById('fComments');
    const visit = document.getElementById('fVisitComments');
    if (!comments || !visit) return false;
    return comments.value !== HouseholdPage.savedComments || visit.value.trim() !== '';
}

registerView('household', {
    async onShow() {
        // Keep the originating view's sidebar item highlighted: this page
        // has no nav item of its own.
        document.querySelector(`.nav-item[data-view="${HouseholdPage.returnTo}"]`)?.classList.add('active');
        await renderHouseholdPage(HouseholdPage.id);
    },
    async beforeLeave() {
        if (!householdPageIsDirty()) return true;
        return confirmDiscard('You have unsaved comments or visit notes. Discard them?');
    },
});

async function renderHouseholdPage(id) {
    const root = document.getElementById('householdRoot');
    const h = await Api.getHousehold(id).catch(e => { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); return null; });
    if (!h) {
        root.innerHTML = `<button class="btn" id="hpBack">← Back</button><p>This household is no longer in the directory.</p>`;
        document.getElementById('hpBack').addEventListener('click', () => showView(HouseholdPage.returnTo));
        return;
    }

    const addressLines = [h.address_line1, h.address_line2].filter(Boolean);
    const cityLine = ([h.city, h.state].filter(Boolean).join(' ') + (h.zip ? ' ' + h.zip : '')).trim();
    const oneLineAddress = [...addressLines, cityLine].filter(Boolean).join(', ');
    const latLon = (h.latitude != null && h.longitude != null) ? `${h.latitude}, ${h.longitude}` : null;
    const backLabel = (document.querySelector(`.nav-item[data-view="${HouseholdPage.returnTo}"]`)?.textContent) || 'Back';

    const head = (first, last, phone, email) => `
        <div class="hh-detail-head">
            <strong>${escapeHtml(first)} ${escapeHtml(last || '')}</strong>
            ${phone ? `<div>${escapeHtml(phone)}</div>` : ''}
            ${email ? `<div>${escapeHtml(email)}</div>` : ''}
        </div>`;

    root.innerHTML = `
        <div class="hp">
            <div class="hp-header">
                <button class="btn" id="hpBack">← ${escapeHtml(backLabel)}</button>
                <div class="hp-title">
                    <h1>${escapeHtml(formatDirectoryName(h))}</h1>
                    <div class="hp-subtitle">${escapeHtml(oneLineAddress || '(no address on file)')}</div>
                </div>
            </div>
            <div class="hp-columns">
                <section class="hp-col">
                    <div class="hp-card">
                        <h3>Household</h3>
                        ${head(h.first_name, h.last_name, h.phone_1, h.email_1)}
                        ${h.first_name_2 ? head(h.first_name_2, h.last_name_2, h.phone_2, h.email_2) : ''}
                        ${h.has_minors ? '<div class="hh-minors-marker">&lt;Minor Children&gt;</div>' : ''}
                        <div class="hh-detail-address">
                            ${addressLines.map(l => `<div>${escapeHtml(l)}</div>`).join('')}
                            ${cityLine ? `<div>${escapeHtml(cityLine)}</div>` : ''}
                            ${latLon ? `<div class="hh-latlon">${escapeHtml(latLon)}</div>` : ''}
                        </div>
                    </div>
                    <div class="hp-card">
                        <h3>Tags</h3>
                        <div id="modalTags">${renderTagChips(h.tags, { onRemove: true })}</div>
                        <div id="modalTagDropdown" class="inline-dropdown"></div>
                    </div>
                    <div class="hp-card">
                        <div class="hh-comments-header">
                            <h3>Comments</h3>
                            <button class="btn" id="fSaveComments">Save Comments</button>
                        </div>
                        <textarea id="fComments" rows="6">${escapeHtml(h.comments || '')}</textarea>
                    </div>
                </section>
                <section class="hp-col">
                    <div class="hp-card">
                        <h3>Record New Visit</h3>
                        <label>Date (YYYY-MM-DD) <input type="text" id="fVisitDate" placeholder="YYYY-MM-DD" value="${localIsoDate()}"></label>
                        <label>Comments <textarea id="fVisitComments" rows="3"></textarea></label>
                        <div class="modal-buttons">
                            <button class="btn" id="fCancelVisit">Clear</button>
                            <button class="btn btn-primary" id="fAddVisit">Save Visit</button>
                        </div>
                    </div>
                    <div class="hp-card">
                        <div class="hh-comments-header">
                            <h3>Visit History</h3>
                            <button type="button" class="hh-visit-icon-btn hp-copy-btn" id="hpCopyVisits" title="Copy visit history" aria-label="Copy visit history">&#9112; Copy</button>
                        </div>
                        <div id="hhVisitHistory" class="hh-visit-history"><em>Loading…</em></div>
                    </div>
                    <div class="hp-card">
                        <h3>Clerk Referrals</h3>
                        <div id="hhReferrals" class="hh-referrals"><em>Loading…</em></div>
                    </div>
                </section>
            </div>
        </div>`;
    HouseholdPage.savedComments = h.comments || '';
    document.getElementById('mainContent').scrollTop = 0;

    document.getElementById('hpBack').addEventListener('click', () => showView(HouseholdPage.returnTo));

    // Tags: dropdown of existing tags only.
    const allTags = await Api.listTags().catch(() => []);
    mountDropdown(document.getElementById('modalTagDropdown'), {
        items: allTags.map(t => ({ value: t.name, label: t.name })),
        staticLabel: '+ set tag',
        onSelect: async (name) => {
            if (!name) return;
            // allowSystemTagChange: true (#59) — this page is the one
            // authorized place a system tag ("Do not contact") can be
            // assigned or replaced: an explicit, per-household decision
            // made from that household's own screen.
            await Api.tagHouseholds([id], name, true);
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
            const value = document.getElementById('fComments').value;
            await Api.updateHouseholdComments(id, value || null);
            HouseholdPage.savedComments = value;
            showMessage('Comments saved.', CONSTANTS.MESSAGE_TYPES.INFO);
        } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); }
    });

    document.getElementById('hpCopyVisits').addEventListener('click', () => copyVisitHistory(id, formatDirectoryName(h)));

    document.getElementById('fCancelVisit').addEventListener('click', () => {
        document.getElementById('fVisitDate').value = localIsoDate();
        document.getElementById('fVisitComments').value = '';
    });
    document.getElementById('fAddVisit').addEventListener('click', async () => {
        const date = document.getElementById('fVisitDate').value.trim();
        if (!isValidIsoDate(date)) { showMessage('Enter a real date as YYYY-MM-DD (e.g. 2026-03-05).', CONSTANTS.MESSAGE_TYPES.ERROR); return; }
        try {
            await Api.recordVisit(id, date, document.getElementById('fVisitComments').value || null);
            document.getElementById('fVisitComments').value = '';
            await refreshVisitHistory(id);
            showMessage('Visit recorded.', CONSTANTS.MESSAGE_TYPES.INFO);
        } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); }
    });

    await Promise.all([
        refreshVisitHistory(id),
        renderHouseholdReferrals(document.getElementById('hhReferrals'), h),
    ]);
}

// Copies the whole visit history as plain text for an email: a heading
// with the household name, then one entry per visit, newest first (the
// order the page shows). A multi-line comment stays together, indented
// under its date.
async function copyVisitHistory(householdId, name) {
    let visits;
    try { visits = await Api.getHouseholdVisits(householdId); }
    catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); return; }
    if (!visits.length) { showMessage('No visits to copy.', CONSTANTS.MESSAGE_TYPES.INFO); return; }
    const entries = visits.map(v => {
        const lines = (v.comments || '').trim().split(/\r?\n/);
        return [`${v.visit_date}  ${lines[0]}`.trimEnd(), ...lines.slice(1).map(l => `            ${l}`)].join('\n');
    });
    const text = `${name} — visit history\n\n${entries.join('\n')}\n`;
    try {
        await navigator.clipboard.writeText(text);
        showMessage(`Copied ${visits.length} visit${visits.length === 1 ? '' : 's'}.`, CONSTANTS.MESSAGE_TYPES.INFO);
    } catch (e) {
        showMessage(`Could not copy: ${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
    }
}

// Read-mode row: date/comments plus Edit/Delete icon buttons. Kept as its
// own function (rather than inlined in the .map() below) since
// renderVisitEditRow below needs the same v.id to swap this same slot
// back in on Cancel/Save.
function renderVisitViewRow(v) {
    // Icons sit on the date line, not at the far end of the comments line
    // (Stan's call) — a long unbroken comment can force horizontal scroll
    // on this row, and icons anchored past the comment text would scroll
    // out of view with it. The date line never grows past the row width,
    // so pinning the icons there keeps them visible regardless of what's
    // in the comments below.
    return `
        <div class="hh-visit-entry" data-visit-id="${v.id}">
            <div class="hh-visit-view-row">
                <div class="hh-visit-date">${escapeHtml(v.visit_date)}</div>
                <div class="hh-visit-actions">
                    <button type="button" class="hh-visit-icon-btn" data-edit-visit="${v.id}" title="Edit visit" aria-label="Edit visit">&#9998;</button>
                    <button type="button" class="hh-visit-icon-btn" data-delete-visit="${v.id}" title="Delete visit" aria-label="Delete visit">&#128465;</button>
                </div>
            </div>
            <div class="hh-visit-comments">${escapeHtml(v.comments || '')}</div>
        </div>`;
}

// Edit-mode row: same date/comments fields as "Record New Visit" below,
// morphed in place over the read-mode row rather than a separate modal —
// per Stan's direction, inline is simpler here than another popup layer.
function renderVisitEditRow(v) {
    return `
        <div class="hh-visit-entry hh-visit-entry-editing" data-visit-id="${v.id}">
            <label>Date (YYYY-MM-DD) <input type="text" class="hh-visit-edit-date" value="${escapeHtml(v.visit_date)}" placeholder="YYYY-MM-DD"></label>
            <label>Comments <textarea class="hh-visit-edit-comments" rows="2">${escapeHtml(v.comments || '')}</textarea></label>
            <div class="modal-buttons">
                <button type="button" class="btn btn-primary" data-save-visit="${v.id}">Save</button>
                <button type="button" class="btn" data-cancel-visit="${v.id}">Cancel</button>
            </div>
        </div>`;
}

async function refreshVisitHistory(householdId) {
    const el = document.getElementById('hhVisitHistory');
    if (!el) return;
    let visits;
    try { visits = await Api.getHouseholdVisits(householdId); }
    catch (e) { el.innerHTML = `<em>Could not load visits: ${escapeHtml(String(e))}</em>`; return; }

    el.innerHTML = visits.length
        ? visits.map(renderVisitViewRow).join('')
        : '<em>No visits recorded yet.</em>';
    if (!visits.length) return;

    const byId = new Map(visits.map(v => [v.id, v]));

    el.querySelectorAll('[data-edit-visit]').forEach(btn => {
        btn.addEventListener('click', () => {
            const id = Number(btn.dataset.editVisit);
            const row = el.querySelector(`[data-visit-id="${id}"]`);
            if (row) row.outerHTML = renderVisitEditRow(byId.get(id));
            wireEditRow(id);
        });
    });
    el.querySelectorAll('[data-delete-visit]').forEach(btn => {
        btn.addEventListener('click', async () => {
            const id = Number(btn.dataset.deleteVisit);
            const v = byId.get(id);
            const confirmed = await confirmYesNo(
                `Delete the visit on ${v.visit_date}? This cannot be undone.`,
                'Delete',
            );
            if (!confirmed) return;
            try {
                await Api.deleteVisit(id);
                await refreshVisitHistory(householdId);
                showMessage('Visit deleted.', CONSTANTS.MESSAGE_TYPES.INFO);
            } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); }
        });
    });

    // Wired separately (not via a static querySelectorAll pass above)
    // because Save/Cancel buttons don't exist until an edit row replaces
    // its view row — see the edit-button handler above.
    function wireEditRow(id) {
        const row = el.querySelector(`[data-visit-id="${id}"]`);
        if (!row) return;
        row.querySelector('[data-cancel-visit]').addEventListener('click', () => {
            row.outerHTML = renderVisitViewRow(byId.get(id));
            rewireRow(id);
        });
        row.querySelector('[data-save-visit]').addEventListener('click', async () => {
            const date = row.querySelector('.hh-visit-edit-date').value.trim();
            if (!isValidIsoDate(date)) { showMessage('Enter a real date as YYYY-MM-DD (e.g. 2026-03-05).', CONSTANTS.MESSAGE_TYPES.ERROR); return; }
            const comments = row.querySelector('.hh-visit-edit-comments').value || null;
            try {
                await Api.updateVisit(id, date, comments);
                await refreshVisitHistory(householdId);
                showMessage('Visit updated.', CONSTANTS.MESSAGE_TYPES.INFO);
            } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); }
        });
    }

    // Re-wires a single view row's Edit/Delete buttons after Cancel swaps
    // the edit row back out — the bulk querySelectorAll passes above only
    // ran once, against the rows rendered at initial load.
    function rewireRow(id) {
        const row = el.querySelector(`[data-visit-id="${id}"]`);
        if (!row) return;
        row.querySelector('[data-edit-visit]').addEventListener('click', () => {
            row.outerHTML = renderVisitEditRow(byId.get(id));
            wireEditRow(id);
        });
        row.querySelector('[data-delete-visit]').addEventListener('click', async () => {
            const v = byId.get(id);
            const confirmed = await confirmYesNo(
                `Delete the visit on ${v.visit_date}? This cannot be undone.`,
                'Delete',
            );
            if (!confirmed) return;
            try {
                await Api.deleteVisit(id);
                await refreshVisitHistory(householdId);
                showMessage('Visit deleted.', CONSTANTS.MESSAGE_TYPES.INFO);
            } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); }
        });
    }
}

window.openHousehold = openHousehold;
