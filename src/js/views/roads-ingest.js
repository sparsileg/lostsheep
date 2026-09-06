// roads-ingest.js — "Road Management" modal (issues #7, #40). Ingest
// picks an already-prepared roads-only .pbf (clipped + filtered
// externally, per #7) and hands it to ingest_road_database, showing
// progress events as it parses/builds/stores. The two toggles below
// (#40) just persist a this-machine display preference via
// Api.saveSettings — map-view.js is what actually reads them and draws
// the road/route overlays; this modal doesn't touch the Leaflet map
// directly; it just prompts a redraw if the map view happens to be live.
// Depends on modalShell() and escapeHtml()/showMessage() from
// backup-restore.js/core.js, which load before this file — same reuse
// pattern backup-restore.js itself follows. modalShell is now an
// explicit window export (see backup-restore.js) since module scripts
// don't leak top-level declarations onto `window`.
import { open } from '../../include/tauri-api/dialog.js';
import { homeDir } from '../../include/tauri-api/path.js';
import { listen } from '../../include/tauri-api/event.js';

const RoadsIngest = {
    async showModal() {
        let settings = {};
        try { settings = await Api.getSettings(); } catch (e) { console.error(e); }
        const roadsChecked = settings.showRoadsOverlay === 'true' ? 'checked' : '';
        const routeChecked = settings.showRouteOverlay === 'true' ? 'checked' : '';

        const overlay = modalShell(`
            <h2>Road Management</h2>
            <p>Select a roads-only <code>.pbf</code> file already prepared (clipped and
            filtered to <code>highway=*</code> ways by your own extract script).</p>
            <label>File <button class="btn" id="riPick">Choose file…</button> <span id="riPickedPath"></span></label>
            <div class="modal-buttons">
                <button class="btn btn-primary" id="riGo" disabled>Ingest</button>
                <button class="btn" id="riCancel">Cancel</button>
            </div>
            <p id="riStage" style="opacity:.75;"></p>
            <hr>
            <label><input type="checkbox" id="riShowRoads" ${roadsChecked}> Show roads on map</label><br>
            <label><input type="checkbox" id="riShowRoute" ${routeChecked}> Show route on map (includes snap lines to nearest road)</label>
        `);

        let srcPath = null;
        let unlisten = null;
        const cleanup = () => { if (unlisten) { unlisten(); unlisten = null; } };

        overlay.querySelector('#riCancel').addEventListener('click', () => { cleanup(); overlay.remove(); });
        // modalShell() already removes the overlay on an outside click —
        // this just makes sure the progress listener is torn down too.
        overlay.addEventListener('click', (e) => { if (e.target === overlay) cleanup(); });

        overlay.querySelector('#riPick').addEventListener('click', async () => {
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

            unlisten = await listen('road-ingest-progress', (event) => {
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

        // Issue #40 — just persist the preference. map-view.js reads it
        // (Api.getSettings) whenever the map view is shown or a visit
        // list is generated; MapView.applyRoadSettings() is called here
        // too so a redraw happens immediately if the map view happens to
        // already be live behind this modal.
        overlay.querySelector('#riShowRoads').addEventListener('change', async (e) => {
            try { await Api.saveSettings({ showRoadsOverlay: e.target.checked ? 'true' : 'false' }); }
            catch (err) { showMessage(`${err}`, CONSTANTS.MESSAGE_TYPES.ERROR); return; }
            if (typeof MapView !== 'undefined' && MapView.applyRoadSettings) MapView.applyRoadSettings();
        });
        overlay.querySelector('#riShowRoute').addEventListener('change', async (e) => {
            try { await Api.saveSettings({ showRouteOverlay: e.target.checked ? 'true' : 'false' }); }
            catch (err) { showMessage(`${err}`, CONSTANTS.MESSAGE_TYPES.ERROR); return; }
            if (typeof MapView !== 'undefined' && MapView.applyRoadSettings) MapView.applyRoadSettings();
        });
    },
};

window.RoadsIngest = RoadsIngest;
