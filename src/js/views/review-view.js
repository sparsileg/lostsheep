registerView('review', {
    init() {
        document.getElementById('reviewRoot').innerHTML = `
            <h1>Review Updates</h1>
            <div class="review-toolbar">
                <button class="btn" id="addAllNewBtn">Add all new records</button>
                <button class="btn btn-danger" id="discardBatchBtn">Discard Batch</button>
            </div>
            <div id="reviewList"></div>
            <button class="btn btn-primary" id="commitBatchBtn" style="margin-top:16px;">Commit Batch</button>
        `;
        document.getElementById('commitBatchBtn').addEventListener('click', commitBatch);
        document.getElementById('addAllNewBtn').addEventListener('click', addAllNew);
        document.getElementById('discardBatchBtn').addEventListener('click', discardBatch);
    },
    async onShow() { await loadReviewQueue(); },
});

// An import batch's pending review items live in the database, not just
// in memory — this survives an app restart instead of losing track of an
// unfinished review the moment the JS variable resets.
async function currentBatchId() {
    if (window.__lastImportBatchId) return window.__lastImportBatchId;
    const pending = await Api.getPendingImportBatch().catch(() => null);
    if (pending) window.__lastImportBatchId = pending;
    return pending;
}

async function loadReviewQueue() {
    const batchId = await currentBatchId();
    const list = document.getElementById('reviewList');
    if (!batchId) { list.innerHTML = '<p>No pending import batch. Run an import first.</p>'; return; }

    let items;
    try { items = await Api.getReviewQueue(batchId); } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); return; }

    if (!items.length) { list.innerHTML = '<p>Nothing left to review — ready to commit.</p>'; return; }

    // Issue #70 (link): 'removed' items in this batch are candidates a
    // 'new' item can be manually linked to, when auto-matching failed at
    // import time (e.g. name and address both changed together). Scoped
    // to this batch only, per the issue's own scope decision.
    const removedItems = items.filter(i => i.match_type === 'removed');

    list.innerHTML = items.map(item => renderReviewItem(item, removedItems)).join('');
    list.querySelectorAll('[data-resolve]').forEach(btn => {
        btn.addEventListener('click', () => resolveItem(btn.dataset.itemId, btn.dataset.resolve));
    });
    list.querySelectorAll('[data-link-dropdown]').forEach(el => {
        const itemId = el.dataset.linkDropdown;
        mountDropdown(el, {
            items: removedItems.map(r => ({ value: String(r.id), label: r.existing_summary || `household #${r.existing_household_id}` })),
            staticLabel: 'Link to removed household…',
            onSelect: (value) => {
                if (!value) return;
                const target = removedItems.find(r => String(r.id) === value);
                if (target) resolveItem(itemId, 'link', target.existing_household_id);
            },
        });
    });
}

function renderReviewItem(item, removedItems) {
    const incoming = item.incoming_data ? JSON.parse(item.incoming_data) : null;
    const incomingHtml = incoming
        ? `${escapeHtml(incoming.first_name)} ${escapeHtml(incoming.last_name)}` +
          (incoming.first_name_2 ? ` & ${escapeHtml(incoming.first_name_2)} ${escapeHtml(incoming.last_name_2)}` : '') +
          ` (${escapeHtml(incoming.role)}) — ${escapeHtml(incoming.address_line1)}`
        : '<em>(record removed from source)</em>';

    const changedHtml = (item.changed_fields && item.changed_fields.length > 0)
        ? `<div class="review-changed-fields"><strong>Changed:</strong> ${item.changed_fields.map(escapeHtml).join(', ')}</div>`
        : '';

    // Issue #75: reassurance that Replace/Merge never touch the user's own
    // notes — shown only where those actions are offered (match_type
    // 'changed'), regardless of whether Comments happens to be in
    // changed_fields this time (the 3+-heads case can still list it).
    const commentsNoteHtml = item.match_type === 'changed'
        ? `<div class="review-comments-note">Your household comments are always kept as-is on Replace or Merge.</div>`
        : '';

    // Issue #62: a 'changed' or 'removed' item can be left pointing at
    // nothing by an earlier resolution in this same batch (a Replace/Merge/
    // Delete elsewhere deleted the household this item's
    // existing_household_id referred to). The backend now refuses
    // Delete/Replace/Merge on these outright, but the user should see why
    // before clicking rather than after — hence this warning, and the
    // affected buttons are left out of `actions` entirely rather than
    // wired up disabled, so there's nothing to click that can only fail.
    const staleHtml = item.stale
        ? `<div class="review-stale-warning">This item's linked household was already changed earlier in this batch — Delete/Replace would fail. Choose Ignore, or Add as New if you still want this record.</div>`
        : '';

    let actions = '';
    if (item.match_type === 'new') actions = actionBtn(item.id, 'add', 'Add');
    if (item.match_type === 'changed') {
        // Issue #82: "Merge" retired — it ran the exact same code as
        // Replace (no field-level reconciliation was ever implemented),
        // so it only invited the user to expect behavior that didn't
        // exist. The backend still accepts a 'merge' resolution value for
        // already-resolved rows from before this change; it's just no
        // longer offered here.
        actions = item.stale
            ? actionBtn(item.id, 'add', 'Add as New')
            : actionBtn(item.id, 'replace', 'Replace') + actionBtn(item.id, 'add', 'Add as New');
    }
    if (item.match_type === 'removed' && !item.stale) actions = actionBtn(item.id, 'delete', 'Confirm Delete');
    actions += actionBtn(item.id, 'ignore', 'Ignore');

    // Issue #70 (link): only a 'new' item, and only when this batch has at
    // least one 'removed' item to offer — no dropdown otherwise, to avoid
    // showing an empty menu.
    const linkDropdownHtml = (item.match_type === 'new' && removedItems && removedItems.length > 0)
        ? `<div class="review-link-dropdown" data-link-dropdown="${item.id}"></div>`
        : '';

    return `
        <div class="review-item review-${item.match_type}${item.stale ? ' review-stale' : ''}">
            <span class="review-badge">${item.match_type}</span>
            <div class="review-body">
                <div><strong>Incoming:</strong> ${incomingHtml}</div>
                ${item.existing_summary ? `<div><strong>Existing:</strong> ${escapeHtml(item.existing_summary)}</div>` : ''}
                ${changedHtml}
                ${commentsNoteHtml}
                ${staleHtml}
                ${linkDropdownHtml}
            </div>
            <div class="review-actions">${actions}</div>
        </div>`;
}

function actionBtn(id, action, label) {
    return `<button class="btn" data-item-id="${id}" data-resolve="${action}">${label}</button>`;
}

async function resolveItem(itemId, action, linkTargetId) {
    let comment = null;
    if (action === 'delete') comment = prompt('Reason for deletion (optional):') || null;
    try {
        await Api.resolveReviewItem(Number(itemId), action, comment, linkTargetId ?? null);
        await loadReviewQueue();
    } catch (e) {
        showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
    }
}

async function addAllNew() {
    const batchId = await currentBatchId();
    if (!batchId) return;
    try {
        // #63: backend now reports partial progress instead of aborting on
        // the first failure — { added, failed: [{item_id, error}, ...] }.
        const result = await Api.resolveAllNewRecords(batchId);
        if (result.failed.length > 0) {
            showMessage(
                `Added ${result.added} of ${result.added + result.failed.length}; ` +
                `${result.failed.length} could not be added — see the Log Viewer. Safe to try again.`,
                CONSTANTS.MESSAGE_TYPES.ERROR
            );
        } else {
            showMessage(`Added ${result.added} new record(s).`, CONSTANTS.MESSAGE_TYPES.INFO);
        }
        await loadReviewQueue();
    } catch (e) {
        showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
    }
}

async function commitBatch() {
    const batchId = await currentBatchId();
    if (!batchId) return;
    try {
        await Api.commitImportBatch(batchId);
        showMessage('Batch committed.', CONSTANTS.MESSAGE_TYPES.INFO);
        window.__lastImportBatchId = null;
        await loadReviewQueue();
    } catch (e) {
        showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
    }
}

// Drops every still-pending item in the batch outright — for a test
// import, an import you don't want, or an older batch orphaned by
// starting a second import before finishing this one (only the most
// recent pending batch is ever reachable through Review Updates).
// Anything already resolved (add/replace/merge/delete/ignore) already
// wrote its real household-side effects and is left untouched — this
// can't undo those, only clear out decisions never made.
async function discardBatch() {
    const batchId = await currentBatchId();
    if (!batchId) return;
    const ok = confirm(
        'Discard this import batch? Any item you have not already resolved will be dropped. ' +
        'This cannot undo anything already added, replaced, merged, or deleted.'
    );
    if (!ok) return;
    try {
        const result = await Api.discardImportBatch(batchId);
        const extra = result.already_resolved > 0
            ? ` ${result.already_resolved} already-resolved item(s) left in place.`
            : '';
        showMessage(`Discarded ${result.discarded} pending item(s).${extra}`, CONSTANTS.MESSAGE_TYPES.INFO);
        window.__lastImportBatchId = null;
        await loadReviewQueue();
    } catch (e) {
        showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
    }
}
