registerView('import', {
    init() {
        document.getElementById('importRoot').innerHTML = `
            <h1>Import Directory</h1>
            <p>Choose a congregational directory PDF (or CSV) to compare against the current database.</p>
            <div class="import-actions">
                <button class="btn btn-primary" id="pickPdfBtn">Choose PDF…</button>
                <button class="btn" id="pickCsvBtn">Choose CSV…</button>
            </div>
            <div id="importProgress" style="display:none;"></div>
            <div id="importSummary"></div>
        `;
        document.getElementById('pickPdfBtn').addEventListener('click', () => runImport('pdf'));
        document.getElementById('pickCsvBtn').addEventListener('click', () => runImport('csv'));
    },
    onShow() {},
});

async function runImport(kind) {
    const { open } = window.__TAURI__.dialog;
    const filters = kind === 'pdf' ? [{ name: 'PDF', extensions: ['pdf'] }] : [{ name: 'CSV', extensions: ['csv'] }];
    const filePath = await open({ multiple: false, filters });
    if (!filePath) return;

    const progressEl = document.getElementById('importProgress');
    const startedAt = Date.now();
    progressEl.style.display = 'block';
    progressEl.textContent = 'Starting…';
    document.getElementById('importSummary').innerHTML = '';

    // Force a real paint before doing anything else. A local import (all
    // SQLite, no network) can finish in well under one frame — without
    // this, the browser may never actually render "Starting…" before the
    // whole thing is done and the div gets hidden again, making progress
    // look like it never appeared at all.
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));

    let unlisten = () => {};
    try {
        unlisten = await window.__TAURI__.event.listen('import-progress', (event) => {
            const { processed, total } = event.payload;
            console.debug('import-progress', processed, total);
            progressEl.textContent = `Processing ${processed} / ${total} records…`;
        });
    } catch (e) {
        // Progress just won't update live if this fails — the import
        // itself doesn't depend on it, so don't block on it. Logged so
        // it's visible in devtools if this is ever the actual cause.
        console.warn('could not subscribe to import-progress events', e);
    }

    try {
        const summary = kind === 'pdf' ? await Api.importPdf(filePath) : await Api.importCsv(filePath);
        // Guarantee the progress indicator was visible for a moment even
        // if the whole import completed near-instantly.
        const elapsed = Date.now() - startedAt;
        if (elapsed < 400) await new Promise((r) => setTimeout(r, 400 - elapsed));
        renderImportSummary(summary);
        if (!summary.auto_accepted && (summary.new_count + summary.changed_count + summary.removed_count > 0)) {
            window.__lastImportBatchId = summary.batch_id;
        }
    } catch (e) {
        showMessage(`Import failed: ${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
    } finally {
        unlisten();
        progressEl.style.display = 'none';
    }
}

function renderImportSummary(s) {
    const el = document.getElementById('importSummary');

    if (s.auto_accepted) {
        el.innerHTML = `
            <h2>Import Complete</h2>
            <p>Database was empty — all <strong>${s.total_rows}</strong> households were imported automatically, no review needed.</p>
            ${s.warnings.length ? `<h3>Warnings (${s.warnings.length})</h3><ul>${s.warnings.map(w => `<li>${escapeHtml(w)}</li>`).join('')}</ul>` : ''}
            <button class="btn btn-primary" id="goHouseholdsBtn">View Households →</button>
        `;
        document.getElementById('goHouseholdsBtn')?.addEventListener('click', () => showView('households'));
        return;
    }

    el.innerHTML = `
        <h2>Import Summary</h2>
        <table>
            <tr><td>Total rows parsed</td><td>${s.total_rows}</td></tr>
            <tr><td>Unchanged (skipped)</td><td>${s.unchanged_count}</td></tr>
            <tr><td>New households</td><td>${s.new_count}</td></tr>
            <tr><td>Possibly changed</td><td>${s.changed_count}</td></tr>
            <tr><td>Possibly removed</td><td>${s.removed_count}</td></tr>
        </table>
        ${s.warnings.length ? `<h3>Warnings (${s.warnings.length})</h3><ul>${s.warnings.map(w => `<li>${escapeHtml(w)}</li>`).join('')}</ul>` : ''}
        ${(s.new_count + s.changed_count + s.removed_count > 0)
            ? `<button class="btn btn-primary" id="goReviewBtn">Go to Review Updates →</button>`
            : `<p>Nothing needs review — database already matches this file.</p>`}
    `;
    document.getElementById('goReviewBtn')?.addEventListener('click', () => showView('review'));
}
