// log-viewer.js — native rewrite of the reference LogViewer.svelte.
// Backed by the `logs` DB table (commands::logs) rather than log files.

// Issue #27: severity order used to translate the logLevel setting into
// which checkboxes start checked. Read-time filtering — the setting
// only affects the DEFAULT view; every level is always fully written,
// and the checkboxes remain freely togglable regardless of this default.
const LV_SEVERITY = { debug: 0, info: 1, warning: 2, error: 3 };

registerView('logs', {
    async init() {
        const settings = await Api.getSettings().catch(() => ({}));
        const threshold = LV_SEVERITY[settings.logLevel] ?? LV_SEVERITY.info;

        document.getElementById('logsRoot').innerHTML = `
            <h1>Log Viewer</h1>
            <div class="lv-toggles">
                <label class="lv-check lv-check-error"><input type="checkbox" data-lvl="error" ${LV_SEVERITY.error >= threshold ? 'checked' : ''}> ERROR</label>
                <label class="lv-check lv-check-warn"><input type="checkbox" data-lvl="warning" ${LV_SEVERITY.warning >= threshold ? 'checked' : ''}> WARNING</label>
                <label class="lv-check lv-check-info"><input type="checkbox" data-lvl="info" ${LV_SEVERITY.info >= threshold ? 'checked' : ''}> INFO</label>
                <label class="lv-check lv-check-debug"><input type="checkbox" data-lvl="debug" ${LV_SEVERITY.debug >= threshold ? 'checked' : ''}> DEBUG</label>
                <div id="lvPager" class="lv-pager-inline"></div>
                <span id="lvLineCount" class="lv-line-count"></span>
                <button class="btn" id="lvCopyBtn" style="margin-left:24px;">⎘ Copy</button>
            </div>
            <div id="lvOutput"></div>
        `;
        lvState.page = 1;
        document.querySelectorAll('#logsRoot [data-lvl]').forEach(cb => cb.addEventListener('change', () => { lvState.page = 1; loadLogs(); }));
        document.getElementById('lvCopyBtn').addEventListener('click', copyAllLogs);
        startLogTail();
    },
    async onShow() { await loadLogs(); },
});

// Polls for newly-written log rows while this view is on screen — a
// backup/import/etc. writes its log entry well after the operation's
// own success message already appeared, so without this the Log Viewer
// looked stale until manually reopened. Only refetches when the view is
// actually the active one (same `.active` class check core.js/sidebar.js
// use elsewhere) and only when page 1 is showing — polling wouldn't make
// sense mid-review of older pages, since a new row at the top would
// shift everything and silently move the user's place.
let lvTailInterval = null;
function startLogTail() {
    if (lvTailInterval !== null) return; // idempotent — init() can run more than once
    lvTailInterval = setInterval(() => {
        const root = document.getElementById('logsRoot');
        if (!root || !root.classList.contains('active')) return;
        if (lvState.page !== 1) return;
        loadLogs();
    }, 3000);
}

const lvState = { page: 1, pageSize: 10, rows: [], moreAvailable: false };

function activeLevels() {
    return Array.from(document.querySelectorAll('#logsRoot [data-lvl]:checked')).map(cb => cb.dataset.lvl);
}

async function loadLogs() {
    const levels = activeLevels();
    // Issue #27 (#5): one query across every checked level, globally
    // ordered and paginated by the backend — "page 2" now means the
    // actual second page of this exact filtered set, not the second
    // page of each level merged independently.
    let rows;
    try {
        rows = await Api.getLogs(levels, lvState.page, lvState.pageSize);
    } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); return; }
    lvState.rows = rows;
    lvState.moreAvailable = rows.length === lvState.pageSize;
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

// Global pager — one query per page now (issue #27), not per-level.
function renderPager() {
    const pager = document.getElementById('lvPager');
    if (!pager) return;
    pager.innerHTML = `
        <button class="btn" id="lvPrevPage" ${lvState.page <= 1 ? 'disabled' : ''}>‹ Prev</button>
        Page ${lvState.page}
        <button class="btn" id="lvNextPage" ${lvState.moreAvailable ? '' : 'disabled'}>Next ›</button>`;
    document.getElementById('lvPrevPage')?.addEventListener('click', () => {
        lvState.page = Math.max(1, lvState.page - 1);
        loadLogs();
    });
    document.getElementById('lvNextPage')?.addEventListener('click', () => {
        if (lvState.moreAvailable) lvState.page += 1;
        loadLogs();
    });
}

// Copies the FULL filtered log (every page), not just the page currently
// on screen — loops Api.getLogs at a large page size until a short batch
// signals the end, same active-levels filter the view is showing.
async function copyAllLogs() {
    const levels = activeLevels();
    const btn = document.getElementById('lvCopyBtn');
    const BATCH_SIZE = 500;
    let rows = [];
    let page = 1;
    while (true) {
        let batch;
        try {
            batch = await Api.getLogs(levels, page, BATCH_SIZE);
        } catch (e) {
            showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
            return;
        }
        rows = rows.concat(batch);
        if (batch.length < BATCH_SIZE) break;
        page += 1;
    }
    const text = rows.map(r => `${r.created_at}  ${r.level.toUpperCase().padEnd(7)}  ${r.message}`).join('\n');
    try { await navigator.clipboard.writeText(text); btn.textContent = 'Copied!'; }
    catch (e) { btn.textContent = 'Copy failed'; }
    setTimeout(() => { btn.textContent = '⎘ Copy'; }, 1500);
}
