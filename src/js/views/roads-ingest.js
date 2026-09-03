// roads-ingest.js — "Ingest Road Database" modal (issue #7). Picks an
// already-prepared roads-only .pbf (clipped + filtered externally, per
// the issue) and hands it to ingest_road_database, showing progress
// events as it parses/builds/stores. Depends on modalShell() and
// escapeHtml()/showMessage() from backup-restore.js/core.js, which load
// before this file — same reuse pattern backup-restore.js itself follows.
const RoadsIngest = {
    async showModal() {
        const overlay = modalShell(`
            <h2>Ingest Road Database</h2>
            <p>Select a roads-only <code>.pbf</code> file already prepared (clipped and
            filtered to <code>highway=*</code> ways by your own extract script).</p>
            <label>File <button class="btn" id="riPick">Choose file…</button> <span id="riPickedPath"></span></label>
            <div class="modal-buttons">
                <button class="btn btn-primary" id="riGo" disabled>Ingest</button>
                <button class="btn" id="riCancel">Cancel</button>
            </div>
            <p id="riStage" style="opacity:.75;"></p>
        `);

        let srcPath = null;
        let unlisten = null;
        const cleanup = () => { if (unlisten) { unlisten(); unlisten = null; } };

        overlay.querySelector('#riCancel').addEventListener('click', () => { cleanup(); overlay.remove(); });
        // modalShell() already removes the overlay on an outside click —
        // this just makes sure the progress listener is torn down too.
        overlay.addEventListener('click', (e) => { if (e.target === overlay) cleanup(); });

        overlay.querySelector('#riPick').addEventListener('click', async () => {
            const { open } = window.__TAURI__.dialog;
            const { homeDir } = window.__TAURI__.path;
            srcPath = await open({ multiple: false, defaultPath: await homeDir(), filters: [{ name: 'Road data', extensions: ['pbf'] }] });
            if (srcPath) {
                overlay.querySelector('#riPickedPath').textContent = srcPath;
                overlay.querySelector('#riGo').disabled = false;
            }
        });

        overlay.querySelector('#riGo').addEventListener('click', async () => {
            if (!srcPath) return;
            const goBtn = overlay.querySelector('#riGo');
            const pickBtn = overlay.querySelector('#riPick');
            const stageEl = overlay.querySelector('#riStage');
            goBtn.disabled = true;
            pickBtn.disabled = true;

            unlisten = await window.__TAURI__.event.listen('road-ingest-progress', (event) => {
                stageEl.textContent = event.payload.stage;
            });

            try {
                const result = await Api.ingestRoadDatabase(srcPath);
                showMessage(`Road graph ingested: ${result}`, CONSTANTS.MESSAGE_TYPES.INFO, 8000);
                cleanup();
                overlay.remove();
            } catch (e) {
                cleanup();
                stageEl.textContent = '';
                showMessage(`Ingest failed: ${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
                goBtn.disabled = false;
                pickBtn.disabled = false;
            }
        });
    },
};

window.RoadsIngest = RoadsIngest;
