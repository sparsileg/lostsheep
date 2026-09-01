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
            <label>Deleted-record retention (days) <input type="number" id="sDeletedDays" min="0"></label>
            <label>Log retention (days) <input type="number" id="sLogDays" min="0"></label>
            <label>Minimum log level</label>
            <div id="sLogLevelDropdown" class="inline-dropdown"></div>
            <label>Households page size</label>
            <div id="sPageSizeDropdown" class="inline-dropdown"></div>
            <label>Default visit-list size <input type="number" id="sVisitSize" min="1"></label>
            <label class="checkbox-label"><input type="checkbox" id="sOfflineCache"> Use cached map tiles when offline</label>

            <label>Backup folder</label>
            <div class="settings-folder-row">
                <input type="text" id="sBackupFolder" readonly placeholder="Not set">
                <button class="btn" id="sChooseFolderBtn">Choose…</button>
            </div>

            <h3>Cached Map Regions</h3>
            <p class="settings-note">Tile downloading for offline use isn't implemented yet — a saved region records
            its boundary only; tile count and size will show 0 until that's built.</p>
            <table id="sCacheTable"><thead><tr><th>Name</th><th>Tiles</th><th>Size</th><th></th></tr></thead><tbody id="sCacheTableBody"></tbody></table>

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
        document.getElementById('sOfflineCache').checked = settings.mapOfflineCacheEnabled === 'true';
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

        await loadCacheRegionsInto(document.getElementById('sCacheTableBody'));

        document.getElementById('sSaveBtn').addEventListener('click', async () => {
            pending.deletedRetentionDays = document.getElementById('sDeletedDays').value;
            pending.logRetentionDays = document.getElementById('sLogDays').value;
            pending.defaultVisitGroupSize = document.getElementById('sVisitSize').value;
            pending.mapOfflineCacheEnabled = String(document.getElementById('sOfflineCache').checked);
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

async function loadCacheRegionsInto(tbody) {
    const regions = await Api.listCacheRegions().catch(() => []);
    tbody.innerHTML = regions.map(r => `
        <tr><td>${escapeHtml(r.name)}</td><td>${r.tile_count}</td><td>${(r.bytes_on_disk / 1024).toFixed(0)} KB</td>
        <td><button class="btn btn-danger" data-del-region="${r.id}">Delete</button></td></tr>`).join('');
    tbody.querySelectorAll('[data-del-region]').forEach(b => b.addEventListener('click', async () => {
        await Api.deleteCacheRegion(Number(b.dataset.delRegion));
        await loadCacheRegionsInto(tbody);
    }));
}

window.openSettingsModal = openSettingsModal;
