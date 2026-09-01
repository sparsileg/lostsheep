registerView('review', {
    init() {
        document.getElementById('reviewRoot').innerHTML = `
            <h1>Review Updates</h1>
            <div class="review-toolbar">
                <button class="btn" id="addAllNewBtn">Add all new records</button>
            </div>
            <div id="reviewList"></div>
            <button class="btn btn-primary" id="commitBatchBtn" style="margin-top:16px;">Commit Batch</button>
        `;
        document.getElementById('commitBatchBtn').addEventListener('click', commitBatch);
        document.getElementById('addAllNewBtn').addEventListener('click', addAllNew);
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

    list.innerHTML = items.map(item => renderReviewItem(item)).join('');
    list.querySelectorAll('[data-resolve]').forEach(btn => {
        btn.addEventListener('click', () => resolveItem(btn.dataset.itemId, btn.dataset.resolve));
    });
}

function renderReviewItem(item) {
    const incoming = item.incoming_data ? JSON.parse(item.incoming_data) : null;
    const incomingHtml = incoming
        ? `${escapeHtml(incoming.first_name)} ${escapeHtml(incoming.last_name)} (${incoming.role}) — ${escapeHtml(incoming.address_line1)}`
        : '<em>(record removed from source)</em>';

    let actions = '';
    if (item.match_type === 'new') actions = actionBtn(item.id, 'add', 'Add');
    if (item.match_type === 'changed') actions = actionBtn(item.id, 'replace', 'Replace') + actionBtn(item.id, 'merge', 'Merge') + actionBtn(item.id, 'add', 'Add as New');
    if (item.match_type === 'removed') actions = actionBtn(item.id, 'delete', 'Confirm Delete');
    actions += actionBtn(item.id, 'ignore', 'Ignore');

    return `
        <div class="review-item review-${item.match_type}">
            <span class="review-badge">${item.match_type}</span>
            <div class="review-body">
                <div><strong>Incoming:</strong> ${incomingHtml}</div>
                ${item.existing_summary ? `<div><strong>Existing:</strong> ${escapeHtml(item.existing_summary)}</div>` : ''}
            </div>
            <div class="review-actions">${actions}</div>
        </div>`;
}

function actionBtn(id, action, label) {
    return `<button class="btn" data-item-id="${id}" data-resolve="${action}">${label}</button>`;
}

async function resolveItem(itemId, action) {
    let comment = null;
    if (action === 'delete') comment = prompt('Reason for deletion (optional):') || null;
    try {
        await Api.resolveReviewItem(Number(itemId), action, comment);
        await loadReviewQueue();
    } catch (e) {
        showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
    }
}

async function addAllNew() {
    const batchId = await currentBatchId();
    if (!batchId) return;
    try {
        const count = await Api.resolveAllNewRecords(batchId);
        showMessage(`Added ${count} new record(s).`, CONSTANTS.MESSAGE_TYPES.INFO);
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
