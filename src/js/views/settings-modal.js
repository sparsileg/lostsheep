// settings-modal.js — Settings is a modal now, not a sidebar view. Changes
// are held locally and only written via Api.saveSettings on "Save";
// "Cancel" just closes without touching anything. Cache-region deletes
// are still immediate (they're not really a "setting" to stage/undo).
import { open } from '../../include/tauri-api/dialog.js';

async function openSettingsModal() {
    // Defensive: if a previous modal (this one or any other) didn't get
    // cleanly removed for some reason, a stray overlay left in the DOM
    // would dim the screen while this new one silently fails to render
    // on top of it. Clear the decks first.
    document.querySelectorAll('.modal-overlay').forEach(el => el.remove());

    const overlay = document.createElement('div');
    overlay.className = 'modal-overlay';
    overlay.innerHTML = `
        <div class="modal settings-modal">
            <h2>Settings</h2>
            <div class="settings-row"><label>Deleted-record retention</label><div id="sDeletedDaysDropdown" class="inline-dropdown"></div></div>
            <div class="settings-row"><label>Log retention</label><div id="sLogDaysDropdown" class="inline-dropdown"></div></div>
            <div class="settings-row"><label>Minimum log level</label><div id="sLogLevelDropdown" class="inline-dropdown"></div></div>
            <div class="settings-row"><label>Households page size</label><div id="sPageSizeDropdown" class="inline-dropdown"></div></div>
            <div class="settings-row"><label for="sVisitSize">Default visit-list size</label><input type="number" id="sVisitSize" min="1"></div>

            <label>Backup folder</label>
            <div class="settings-folder-row">
                <input type="text" id="sBackupFolder" readonly placeholder="Not set">
                <button class="btn" id="sChooseFolderBtn">Choose…</button>
            </div>

            <label>Visit route start point</label>
            <div class="settings-row"><label for="sRouteStartLabel">Label</label><input type="text" id="sRouteStartLabel" placeholder="e.g. Church"></div>
            <div class="settings-row"><label for="sRouteStartLat">Latitude</label><input type="number" id="sRouteStartLat" step="any" placeholder="-90 to 90"></div>
            <div class="settings-row"><label for="sRouteStartLon">Longitude</label><input type="number" id="sRouteStartLon" step="any" placeholder="-180 to 180"></div>
            <p style="opacity:.6; margin-top:-8px;">Fill in all three to route generated visit lists from this point, or leave all three blank.</p>

            <div class="modal-buttons">
                <button class="btn btn-primary" id="sSaveBtn">Save</button>
                <button class="btn" id="sCancelBtn">Cancel</button>
            </div>
        </div>`;
    document.body.appendChild(overlay);
    overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });
    document.getElementById('sCancelBtn').addEventListener('click', () => overlay.remove());

    try {
        const settings = await Api.getSettings().catch(() => ({}));
        const pending = { ...settings };

        document.getElementById('sVisitSize').value = settings.defaultVisitGroupSize || 10;
        document.getElementById('sRouteStartLabel').value = settings.routeStartLabel || '';
        document.getElementById('sRouteStartLat').value = settings.routeStartLat || '';
        document.getElementById('sRouteStartLon').value = settings.routeStartLon || '';
        document.getElementById('sBackupFolder').value = settings.backupFolder || '';

        document.getElementById('sChooseFolderBtn').addEventListener('click', async () => {
            const folder = await open({ directory: true, defaultPath: settings.backupFolder || undefined });
            if (folder) {
                pending.backupFolder = folder;
                document.getElementById('sBackupFolder').value = folder;
            }
        });

        mountDropdown(document.getElementById('sLogLevelDropdown'), {
            items: ['error', 'warning', 'info', 'debug'].map(l => ({ value: l, label: l })),
            value: settings.logLevel || 'info',
            onSelect: (val) => { pending.logLevel = val; },
        });
        mountDropdown(document.getElementById('sPageSizeDropdown'), {
            items: ['10', '25', '100', '500'].map(n => ({ value: n, label: n })),
            value: settings.pageSize || '25',
            onSelect: (val) => { pending.pageSize = val; },
        });
        const retentionItems = ['30', '90', '180', '365'].map(n => ({ value: n, label: `${n} days` }));
        mountDropdown(document.getElementById('sDeletedDaysDropdown'), {
            items: retentionItems,
            value: settings.deletedRetentionDays || '365',
            onSelect: (val) => { pending.deletedRetentionDays = val; },
        });
        mountDropdown(document.getElementById('sLogDaysDropdown'), {
            items: retentionItems,
            value: settings.logRetentionDays || '30',
            onSelect: (val) => { pending.logRetentionDays = val; },
        });

        document.getElementById('sSaveBtn').addEventListener('click', async () => {
            pending.defaultVisitGroupSize = document.getElementById('sVisitSize').value;
            pending.routeStartLabel = document.getElementById('sRouteStartLabel').value.trim();
            pending.routeStartLat = document.getElementById('sRouteStartLat').value.trim();
            pending.routeStartLon = document.getElementById('sRouteStartLon').value.trim();
            // deletedRetentionDays/logRetentionDays already live on `pending`
            // via the dropdowns' onSelect above — nothing to read from an
            // input here now that they're fixed values, not free text.
            try {
                const deletedDays = parseInt(pending.deletedRetentionDays, 10);
                const logDays = parseInt(pending.logRetentionDays, 10);
                const impact = await Api.previewPruneImpact(deletedDays, logDays);
                if (impact.deleted_households > 0 || impact.logs > 0) {
                    const proceed = window.confirm(
                        `At these retention settings, ${impact.deleted_households} deleted record(s) and ` +
                        `${impact.logs} log entrie(s) will be permanently removed the next time the app starts. ` +
                        `This cannot be undone. Continue?`
                    );
                    if (!proceed) return;
                }
                // Saving only stores the setting — it does not delete
                // anything itself (#28). The actual prune runs unattended
                // at the next app startup.
                await Api.saveSettings(pending);
                showMessage('Settings saved. Retention pruning runs on next app start.', CONSTANTS.MESSAGE_TYPES.INFO);
                overlay.remove();
            } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); }
        });
    } catch (e) {
        // Whatever went wrong, don't leave a dimmed screen with an
        // invisible/broken modal — that's strictly worse than an error
        // message and a closed dialog.
        console.error('Settings modal failed to initialize', e);
        showMessage(`Could not open Settings: ${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
        overlay.remove();
    }
}

window.openSettingsModal = openSettingsModal;
