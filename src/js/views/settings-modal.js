// settings-modal.js — Settings is a modal now, not a sidebar view. Changes
// are held locally and only written via Api.saveSettings on "Save";
// "Cancel" just closes without touching anything. Cache-region deletes
// are still immediate (they're not really a "setting" to stage/undo).

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
            <div class="settings-row"><label for="sDeletedDays">Deleted-record retention (days)</label><input type="number" id="sDeletedDays" min="0"></div>
            <div class="settings-row"><label for="sLogDays">Log retention (days)</label><input type="number" id="sLogDays" min="0"></div>
            <div class="settings-row"><label>Minimum log level</label><div id="sLogLevelDropdown" class="inline-dropdown"></div></div>
            <div class="settings-row"><label>Households page size</label><div id="sPageSizeDropdown" class="inline-dropdown"></div></div>
            <div class="settings-row"><label for="sVisitSize">Default visit-list size</label><input type="number" id="sVisitSize" min="1"></div>

            <label>Visit route start point</label>
            <div class="settings-row"><label for="sRouteStartLabel">Label</label><input type="text" id="sRouteStartLabel" placeholder="e.g. Church"></div>
            <div class="settings-row"><label for="sRouteStartLat">Latitude</label><input type="number" id="sRouteStartLat" step="any" placeholder="-90 to 90"></div>
            <div class="settings-row"><label for="sRouteStartLon">Longitude</label><input type="number" id="sRouteStartLon" step="any" placeholder="-180 to 180"></div>
            <p style="opacity:.6; margin-top:-8px;">Fill in all three to route generated visit lists from this point, or leave all three blank.</p>

            <label>Backup folder</label>
            <div class="settings-folder-row">
                <input type="text" id="sBackupFolder" readonly placeholder="Not set">
                <button class="btn" id="sChooseFolderBtn">Choose…</button>
            </div>

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

        document.getElementById('sDeletedDays').value = settings.deletedRetentionDays || 365;
        document.getElementById('sLogDays').value = settings.logRetentionDays || 30;
        document.getElementById('sVisitSize').value = settings.defaultVisitGroupSize || 10;
        document.getElementById('sRouteStartLabel').value = settings.routeStartLabel || '';
        document.getElementById('sRouteStartLat').value = settings.routeStartLat || '';
        document.getElementById('sRouteStartLon').value = settings.routeStartLon || '';
        document.getElementById('sBackupFolder').value = settings.backupFolder || '';

        document.getElementById('sChooseFolderBtn').addEventListener('click', async () => {
            const { open } = window.__TAURI__.dialog;
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

        document.getElementById('sSaveBtn').addEventListener('click', async () => {
            pending.deletedRetentionDays = document.getElementById('sDeletedDays').value;
            pending.logRetentionDays = document.getElementById('sLogDays').value;
            pending.defaultVisitGroupSize = document.getElementById('sVisitSize').value;
            pending.routeStartLabel = document.getElementById('sRouteStartLabel').value.trim();
            pending.routeStartLat = document.getElementById('sRouteStartLat').value.trim();
            pending.routeStartLon = document.getElementById('sRouteStartLon').value.trim();
            try {
                await Api.saveSettings(pending);
                await Api.pruneOldDeletedAndLogs();
                showMessage('Settings saved.', CONSTANTS.MESSAGE_TYPES.INFO);
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
