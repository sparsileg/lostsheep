// ── Deleted Records view ─────────────────────────────────────────────────
// Browses deleted_households (the real soft-delete table — see
// soft_delete_household in commands/households.rs) and lets the user
// restore a record before its retention period expires and Settings'
// cleanup job removes it for good.
registerView('deleted-records', {
    init() {
        document.getElementById('deletedRecordsRoot').innerHTML = `
            <h1>Deleted Records</h1>
            <div id="drResultsMeta"></div>
            <table id="drTable">
                <thead><tr><th>Name</th><th>Address</th><th>Reason</th><th>Deleted</th><th></th></tr></thead>
                <tbody id="drTableBody"></tbody>
            </table>
        `;
    },
    async onShow() {
        await loadDeletedRecords();
    },
});
const DeletedRecordsView = ViewRegistry['deleted-records'];

async function loadDeletedRecords() {
    let rows;
    try {
        rows = await Api.listDeletedHouseholds();
    } catch (e) {
        showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
        return;
    }
    document.getElementById('drResultsMeta').textContent = `${rows.length} deleted record${rows.length === 1 ? '' : 's'}`;
    document.getElementById('drTableBody').innerHTML = rows.length
        ? rows.map(r => {
            const name = `${r.last_name}, ${r.first_name}` + (r.first_name_2 ? ` & ${r.first_name_2}` : '');
            const addr = [r.address_line1, r.city, r.state, r.zip].filter(Boolean).join(', ');
            return `<tr>
                <td>${escapeHtml(name)}</td>
                <td>${escapeHtml(addr)}</td>
                <td>${escapeHtml(r.deletion_reason || '')}</td>
                <td>${escapeHtml(r.deleted_at)}</td>
                <td><button class="btn" data-restore="${r.id}">Restore</button></td>
            </tr>`;
        }).join('')
        : '<tr><td colspan="5">No deleted records.</td></tr>';

    document.querySelectorAll('#drTableBody [data-restore]').forEach(btn =>
        btn.addEventListener('click', () => restoreRecord(Number(btn.dataset.restore))));
}

async function restoreRecord(id) {
    try {
        await Api.restoreDeletedHousehold(id);
        showMessage('Household restored.', CONSTANTS.MESSAGE_TYPES.INFO);
        await loadDeletedRecords();
    } catch (e) {
        showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
    }
}
