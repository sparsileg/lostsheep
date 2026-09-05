// log-viewer.js — native rewrite of the reference LogViewer.svelte.
// Backed by the `logs` DB table (commands::logs) rather than log files.
registerView('logs', {
    init() {
        document.getElementById('logsRoot').innerHTML = `
            <h1>Log Viewer</h1>
            <div class="lv-toggles">
                <label class="lv-check lv-check-error"><input type="checkbox" data-lvl="error" checked> ERROR</label>
                <label class="lv-check lv-check-warn"><input type="checkbox" data-lvl="warning" checked> WARNING</label>
                <label class="lv-check lv-check-info"><input type="checkbox" data-lvl="info" checked> INFO</label>
                <label class="lv-check lv-check-debug"><input type="checkbox" data-lvl="debug"> DEBUG</label>
                <span id="lvLineCount" class="lv-line-count"></span>
                <button class="btn" id="lvCopyBtn">⎘ Copy</button>
            </div>
            <div class="modal-body lv-output" id="lvOutput"></div>
            <div id="lvPager"></div>
        `;
        lvState.page = 1;
        document.querySelectorAll('#logsRoot [data-lvl]').forEach(cb => cb.addEventListener('change', () => { lvState.page = 1; loadLogs(); }));
        document.getElementById('lvCopyBtn').addEventListener('click', copyVisibleLogs);
    },
    async onShow() { await loadLogs(); },
});

const lvState = { page: 1, pageSize: 200, rows: [], moreAvailable: false };

function activeLevels() {
    return Array.from(document.querySelectorAll('#logsRoot [data-lvl]:checked')).map(cb => cb.dataset.lvl);
}

async function loadLogs() {
    const levels = activeLevels();
    // Backend filters by a single level; fetch each active level and
    // merge client-side (log volume here is small — settings caps retention).
    let rows = [];
    let more = false;
    try {
        for (const lvl of levels) {
            const chunk = await Api.getLogs(lvl, lvState.page, lvState.pageSize);
            if (chunk.length === lvState.pageSize) more = true;
            rows.push(...chunk);
        }
    } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); return; }
    rows.sort((a, b) => (a.created_at < b.created_at ? 1 : -1));
    lvState.rows = rows;
    lvState.moreAvailable = more;
    renderLogRows();
    renderPager();
}

function renderLogRows() {
    const out = document.getElementById('lvOutput');
    document.getElementById('lvLineCount').textContent = `${lvState.rows.length} lines`;
    if (!lvState.rows.length) { out.innerHTML = '<div class="modal-loading">No entries match the active filters.</div>'; return; }
    out.innerHTML = `<table class="kw-table"><thead><tr><th>Timestamp</th><th>Level</th><th>Message</th></tr></thead><tbody>
        ${lvState.rows.map(r => `<tr class="lv-${r.level}"><td class="lv-ts">${escapeHtml(r.created_at)}</td>
            <td class="lv-level">${escapeHtml(r.level.toUpperCase())}</td><td class="lv-msg">${escapeHtml(r.message)}</td></tr>`).join('')}
    </tbody></table>`;
}

// Was built by init() but never actually populated or wired to anything —
// page never advanced past 1 no matter how many log rows existed.
function renderPager() {
    const pager = document.getElementById('lvPager');
    if (!pager) return;
    pager.innerHTML = `
        <button class="btn" id="lvPrevPage" ${lvState.page <= 1 ? 'disabled' : ''}>‹ Prev</button>
        Page ${lvState.page}
        <button class="btn" id="lvNextPage" ${lvState.moreAvailable ? '' : 'disabled'}>Next ›</button>`;
    document.getElementById('lvPrevPage')?.addEventListener('click', () => { lvState.page--; loadLogs(); });
    document.getElementById('lvNextPage')?.addEventListener('click', () => { lvState.page++; loadLogs(); });
}

async function copyVisibleLogs() {
    const text = lvState.rows.map(r => `${r.created_at}  ${r.level.toUpperCase().padEnd(7)}  ${r.message}`).join('\n');
    const btn = document.getElementById('lvCopyBtn');
    try { await navigator.clipboard.writeText(text); btn.textContent = 'Copied!'; }
    catch (e) { btn.textContent = 'Copy failed'; }
    setTimeout(() => { btn.textContent = '⎘ Copy'; }, 1500);
}
