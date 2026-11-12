// ── Clerk Referrals (docs/Record_Referral_Design.md, Stage 1) ──────────────
// A referral is a record change the ward clerk has to make: a move, a
// death, wrong contact info, a removal request. This file holds the
// Clerk Referrals view (to report / awaiting clerk / closed), and the
// shared "Report to clerk" modal that the household detail modal and the
// map popup open. The PDF and Copy Text for the clerk are in
// referrals-pdf.js.
//
// Stage 1 closes referrals by hand (Confirm / Cancel). Import-driven
// confirmation and the Overdue tab come in Stage 2.

const REFERRAL_KINDS = [
    { value: 'moved_known', label: 'Moved — new address known' },
    { value: 'moved_unknown', label: 'Moved — address unknown' },
    { value: 'deceased', label: 'Deceased' },
    { value: 'contact_info', label: 'Contact info wrong' },
    { value: 'remove_records', label: 'Asked to have records removed' },
    { value: 'other', label: 'Other' },
];

function referralKindLabel(kind) {
    return (REFERRAL_KINDS.find(k => k.value === kind) || { label: kind }).label;
}

const REFERRAL_STATUS_LABELS = {
    to_report: 'To report',
    reported: 'Reported',
    confirmed: 'Confirmed',
    cancelled: 'Cancelled',
};

function referralIsOpen(r) {
    return r.status === 'to_report' || r.status === 'reported';
}

// ── Create / edit modal ─────────────────────────────────────────────────
// opts.households: [{ id, name }] — more than one when opened from a map
//   marker that groups several directory entries at one address; the
//   user picks which one.
// opts.referral: an existing referral to edit instead of creating one.
// Resolves to the saved referral, or null if the user cancelled.
function openReferralModal(opts) {
    return new Promise((resolve) => {
        const editing = opts.referral || null;
        const households = opts.households || [];
        const overlay = document.createElement('div');
        overlay.className = 'modal-overlay';

        let whoHtml;
        if (editing) {
            whoHtml = `<div class="ref-modal-who"><strong>${escapeHtml(editing.reference)}</strong> ${escapeHtml(editing.snapshot_name)}</div>`;
        } else if (households.length === 1) {
            whoHtml = `<div class="ref-modal-who"><strong>${escapeHtml(households[0].name)}</strong></div>`;
        } else {
            whoHtml = `<label>Household</label><div id="refHousehold" class="ref-dropdown"></div>`;
        }

        overlay.innerHTML = `
            <div class="modal ref-modal">
                <h2>${editing ? 'Edit clerk referral' : 'Report to clerk'}</h2>
                ${whoHtml}
                <label>What the clerk needs to do</label><div id="refKind" class="ref-dropdown"></div>
                <label id="refNewAddressLabel">New address <input type="text" id="refNewAddress" value="${escapeHtml(editing?.new_address || '')}" placeholder="Street, city, state, zip"></label>
                <label>Details <textarea id="refDetails" rows="3" placeholder="How you learned it, dates, who told you">${escapeHtml(editing?.details || '')}</textarea></label>
                <label>Learned on (YYYY-MM-DD) <input type="text" id="refLearnedOn" value="${escapeHtml(editing?.learned_on || localIsoDate())}"></label>
                <div class="modal-buttons">
                    <button class="btn btn-primary" id="refSave">${editing ? 'Save' : 'Add referral'}</button>
                    <button class="btn" id="refCancel">Cancel</button>
                </div>
            </div>`;
        document.body.appendChild(overlay);

        // Themed dropdowns (dropdown.js), not native <select>: WebKitGTK
        // draws a native select's list and selected value with its own
        // colours, unreadable in the dark themes.
        let kind = editing ? editing.kind : REFERRAL_KINDS[0].value;
        const syncNewAddress = () => {
            overlay.querySelector('#refNewAddressLabel').style.display = kind === 'moved_known' ? '' : 'none';
        };
        mountDropdown(overlay.querySelector('#refKind'), {
            items: REFERRAL_KINDS,
            value: kind,
            onSelect: (v) => { kind = v; syncNewAddress(); },
        });
        let householdId = households.length ? households[0].id : null;
        const hhEl = overlay.querySelector('#refHousehold');
        if (hhEl) {
            mountDropdown(hhEl, {
                items: households.map(h => ({ value: String(h.id), label: h.name })),
                value: String(householdId),
                onSelect: (v) => { householdId = Number(v); },
            });
        }
        syncNewAddress();

        const finish = (value) => { overlay.remove(); resolve(value); };
        overlay.querySelector('#refCancel').addEventListener('click', () => finish(null));
        overlay.addEventListener('click', (e) => { if (e.target === overlay) finish(null); });

        overlay.querySelector('#refSave').addEventListener('click', async () => {
            const details = overlay.querySelector('#refDetails').value;
            const newAddress = overlay.querySelector('#refNewAddress').value;
            const learnedOn = overlay.querySelector('#refLearnedOn').value.trim();
            if (!isValidIsoDate(learnedOn)) {
                showMessage('Enter a real date as YYYY-MM-DD (e.g. 2026-03-05).', CONSTANTS.MESSAGE_TYPES.ERROR);
                return;
            }
            try {
                if (editing) {
                    finish(await Api.updateReferral(editing.id, kind, details, newAddress, learnedOn));
                    return;
                }
                // Same kind already open for this household: warn, don't block
                // (design §3.1) — it may be a genuinely separate change.
                const existing = await Api.listReferrals(householdId);
                const dup = existing.find(r => r.kind === kind && referralIsOpen(r));
                if (dup) {
                    const go = await confirmYesNo(
                        `${dup.reference} (${referralKindLabel(kind)}) is already open for this household. Add another anyway?`,
                        'Add anyway');
                    if (!go) return;
                }
                const saved = await Api.createReferral(householdId, kind, details, newAddress, learnedOn);
                showMessage(`Clerk referral ${saved.reference} added.`, CONSTANTS.MESSAGE_TYPES.INFO);
                finish(saved);
            } catch (e) {
                showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
            }
        });
    });
}

// A small modal asking for an optional note before closing a referral.
// Resolves to the note text ('' if left blank), or null on cancel.
function promptReferralNote(title, message, confirmLabel) {
    return new Promise((resolve) => {
        const overlay = document.createElement('div');
        overlay.className = 'modal-overlay';
        overlay.innerHTML = `
            <div class="modal ref-modal">
                <h2>${escapeHtml(title)}</h2>
                <p>${escapeHtml(message)}</p>
                <label>Note (optional) <textarea id="refNote" rows="2"></textarea></label>
                <div class="modal-buttons">
                    <button class="btn btn-primary" id="refNoteOk">${escapeHtml(confirmLabel)}</button>
                    <button class="btn" id="refNoteCancel">Back</button>
                </div>
            </div>`;
        document.body.appendChild(overlay);
        const finish = (v) => { overlay.remove(); resolve(v); };
        overlay.querySelector('#refNoteOk').addEventListener('click', () => finish(overlay.querySelector('#refNote').value));
        overlay.querySelector('#refNoteCancel').addEventListener('click', () => finish(null));
        overlay.addEventListener('click', (e) => { if (e.target === overlay) finish(null); });
    });
}

// ── Per-household list, for the household detail modal ─────────────────
// Renders this household's referrals into `el`, with a "Report to
// clerk…" button. Closed ones are shown too: they are its history.
async function renderHouseholdReferrals(el, household) {
    let list = [];
    try { list = await Api.listReferrals(household.id); }
    catch (e) { el.innerHTML = `<em>${escapeHtml(String(e))}</em>`; return; }
    const rows = list.map(r => `
        <div class="ref-hh-row ${referralIsOpen(r) ? '' : 'ref-closed'}">
            <strong>${escapeHtml(r.reference)}</strong>
            ${escapeHtml(referralKindLabel(r.kind))} ·
            <span class="ref-status ref-status-${r.status}">${escapeHtml(REFERRAL_STATUS_LABELS[r.status] || r.status)}</span>
            <span class="ref-dim">learned ${escapeHtml(r.learned_on)}${r.reported_on ? `, reported ${escapeHtml(r.reported_on)}` : ''}</span>
        </div>`).join('');
    el.innerHTML = `${rows || '<div class="ref-dim">None.</div>'}
        <button class="btn" type="button" data-ref-new>Report to clerk…</button>`;
    el.querySelector('[data-ref-new]').addEventListener('click', async () => {
        const name = `${household.last_name}, ${household.first_name}` +
            (household.first_name_2 ? ` & ${household.first_name_2}` : '');
        const saved = await openReferralModal({ households: [{ id: household.id, name }] });
        if (saved) await renderHouseholdReferrals(el, household);
    });
}

// ── The view ───────────────────────────────────────────────────────────
const refState = { tab: 'to_report', all: [] };

const REFERRAL_TABS = [
    { key: 'to_report', label: 'To report', match: r => r.status === 'to_report' },
    { key: 'reported', label: 'Awaiting clerk', match: r => r.status === 'reported' },
    { key: 'closed', label: 'Closed', match: r => !referralIsOpen(r) },
];

registerView('referrals', {
    init() {
        document.getElementById('referralsRoot').innerHTML = `
            <h1>Clerk Referrals</h1>
            <div class="ref-toolbar">
                <div class="rp-tabs ref-tabs" role="tablist">
                    ${REFERRAL_TABS.map(t => `<button type="button" class="rp-tab" role="tab" data-ref-tab="${t.key}">${t.label} <span data-ref-count="${t.key}"></span></button>`).join('')}
                </div>
                <div class="ref-export">
                    <button class="btn btn-primary" id="refExportPdfBtn">Export for Clerk</button>
                    <button class="btn" id="refCopyTextBtn">⎘ Copy Text</button>
                </div>
            </div>
            <p class="ref-help" id="refHelp"></p>
            <table id="refTable" class="ref-table">
                <thead id="refTableHead"></thead>
                <tbody id="refTableBody"></tbody>
            </table>
        `;
        document.querySelectorAll('[data-ref-tab]').forEach(btn =>
            btn.addEventListener('click', () => { refState.tab = btn.dataset.refTab; renderReferrals(); }));
        document.getElementById('refExportPdfBtn').addEventListener('click', () => exportForClerk('pdf'));
        document.getElementById('refCopyTextBtn').addEventListener('click', () => exportForClerk('text'));
    },
    async onShow() {
        await loadReferrals();
    },
});

async function loadReferrals() {
    try { refState.all = await Api.listReferrals(null); }
    catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); return; }
    renderReferrals();
}

function renderReferrals() {
    REFERRAL_TABS.forEach(t => {
        const n = refState.all.filter(t.match).length;
        document.querySelector(`[data-ref-count="${t.key}"]`).textContent = `(${n})`;
        document.querySelector(`[data-ref-tab="${t.key}"]`).classList.toggle('active', t.key === refState.tab);
    });
    const toReport = refState.all.filter(r => r.status === 'to_report').length;
    document.getElementById('refExportPdfBtn').disabled = toReport === 0;
    document.getElementById('refCopyTextBtn').disabled = toReport === 0;

    const help = {
        to_report: 'Recorded, not yet sent. Export for Clerk makes a PDF (or text for an email) of these.',
        reported: 'Sent to the clerk. Confirm one when the directory shows the change.',
        closed: 'Confirmed or cancelled. Kept as history.',
    };
    document.getElementById('refHelp').textContent = help[refState.tab];

    const tab = REFERRAL_TABS.find(t => t.key === refState.tab);
    const rows = refState.all.filter(tab.match);
    const closed = refState.tab === 'closed';
    document.getElementById('refTableHead').innerHTML = `<tr>
        <th>Ref</th><th>Household</th><th>Change</th><th>Learned</th><th>Reported</th>
        ${closed ? '<th>Closed</th>' : ''}<th></th></tr>`;
    const cols = closed ? 7 : 6;
    document.getElementById('refTableBody').innerHTML = rows.length
        ? rows.map(r => renderReferralRow(r, closed)).join('')
        : `<tr><td colspan="${cols}" class="ref-dim">Nothing here.</td></tr>`;

    document.querySelectorAll('#refTableBody [data-ref-action]').forEach(btn =>
        btn.addEventListener('click', () => referralAction(btn.dataset.refAction, Number(btn.dataset.refId))));
}

function renderReferralRow(r, closed) {
    const extra = [
        r.new_address ? `New address: ${r.new_address}` : '',
        r.details || '',
    ].filter(Boolean).map(t => `<div class="ref-detail">${escapeHtml(t)}</div>`).join('');
    const gone = r.household_id == null ? '<div class="ref-dim">No longer in the directory</div>' : '';
    const actions = [];
    if (r.household_id != null) actions.push(['open', 'Household']);
    if (referralIsOpen(r)) {
        actions.push(['edit', 'Edit']);
        if (r.status === 'to_report') actions.push(['reported', 'Mark reported']);
        actions.push(['confirm', 'Confirm'], ['cancel', 'Cancel']);
    } else {
        actions.push(['reopen', 'Reopen']);
    }
    const closedCell = closed ? `<td>
        <span class="ref-status ref-status-${r.status}">${escapeHtml(REFERRAL_STATUS_LABELS[r.status])}</span>
        ${escapeHtml(r.resolved_on || '')}
        ${r.resolution_note ? `<div class="ref-detail">${escapeHtml(r.resolution_note)}</div>` : ''}</td>` : '';
    return `<tr>
        <td class="ref-ref">${escapeHtml(r.reference)}</td>
        <td><div>${escapeHtml(r.snapshot_name)}</div><div class="ref-dim">${escapeHtml(r.snapshot_address || '(no address on file)')}</div>${gone}</td>
        <td><div>${escapeHtml(referralKindLabel(r.kind))}</div>${extra}</td>
        <td class="ref-date">${escapeHtml(r.learned_on)}</td>
        <td class="ref-date">${escapeHtml(r.reported_on || '—')}</td>
        ${closedCell}
        <td class="ref-actions">${actions.map(([a, label]) =>
            `<button class="btn btn-small" type="button" data-ref-action="${a}" data-ref-id="${r.id}">${label}</button>`).join('')}</td>
    </tr>`;
}

async function referralAction(action, id) {
    const r = refState.all.find(x => x.id === id);
    if (!r) return;
    try {
        if (action === 'open') {
            openHousehold(r.household_id);
            return;
        }
        if (action === 'edit') {
            if (!await openReferralModal({ referral: r })) return;
        } else if (action === 'reported') {
            // Manual override, for a referral passed on by phone or in person.
            const ok = await confirmYesNo(`Mark ${r.reference} as reported to the clerk today?`, 'Mark reported');
            if (!ok) return;
            await Api.markReferralsReported([id], null);
        } else if (action === 'confirm') {
            const note = await promptReferralNote(`Confirm ${r.reference}`,
                'The clerk has made the change. This closes the referral.', 'Confirm');
            if (note === null) return;
            await Api.closeReferral(id, 'confirmed', note);
        } else if (action === 'cancel') {
            const note = await promptReferralNote(`Cancel ${r.reference}`,
                'Withdraw this referral (mistaken, they moved back, no action needed). It stays in Closed as history.',
                'Cancel referral');
            if (note === null) return;
            await Api.closeReferral(id, 'cancelled', note);
        } else if (action === 'reopen') {
            await Api.reopenReferral(id);
        }
    } catch (e) {
        showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
    }
    await loadReferrals();
}

// Export (PDF or text), then ask before marking anything reported: a
// download or a copy is not proof the email was sent (design §3.2).
async function exportForClerk(format) {
    const items = refState.all.filter(r => r.status === 'to_report')
        .sort((a, b) => a.id - b.id);
    if (items.length === 0) return;
    let unitName = '';
    try { unitName = (await Api.getActiveProfile())?.name || ''; } catch (_) { /* header line is optional */ }

    if (format === 'pdf') {
        ReferralsPdf.download(items, unitName);
    } else {
        try {
            await navigator.clipboard.writeText(ReferralsPdf.text(items, unitName));
            showMessage('Copied. Paste it into your email to the clerk.', CONSTANTS.MESSAGE_TYPES.INFO);
        } catch (e) {
            showMessage(`Could not copy: ${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
            return;
        }
    }

    const n = items.length;
    const ok = await confirmYesNo(
        `Mark ${n} new request${n === 1 ? '' : 's'} as reported today? Choose this once you have sent ${n === 1 ? 'it' : 'them'} to the clerk.`,
        'Mark reported');
    if (!ok) return;
    try {
        const changed = await Api.markReferralsReported(items.map(r => r.id), null);
        showMessage(`${changed} referral${changed === 1 ? '' : 's'} marked reported.`, CONSTANTS.MESSAGE_TYPES.INFO);
    } catch (e) {
        showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
    }
    await loadReferrals();
}

window.openReferralModal = openReferralModal;
window.renderHouseholdReferrals = renderHouseholdReferrals;
window.referralKindLabel = referralKindLabel;
