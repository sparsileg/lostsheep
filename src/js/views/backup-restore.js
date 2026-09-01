// backup-restore.js — modal flows invoked from the hamburger menu.
const BackupRestore = {
    async showBackupModal() {
        const settings = await Api.getSettings().catch(() => ({}));
        if (!settings.backupFolder) {
            showMessage('Set a backup folder in Settings before backing up.', CONSTANTS.MESSAGE_TYPES.ERROR);
            return;
        }

        const overlay = modalShell(`
            <h2>Backup Database</h2>
            <p>Choose a passphrase to protect the backup — store it safely, it's required to restore.</p>
            <label>Passphrase
                <div class="passphrase-row">
                    <input type="password" id="bkPass" size="20">
                    <button type="button" class="btn passphrase-toggle" data-toggle="bkPass">👁</button>
                </div>
            </label>
            <label>Confirm passphrase
                <div class="passphrase-row">
                    <input type="password" id="bkPass2" size="20">
                    <button type="button" class="btn passphrase-toggle" data-toggle="bkPass2">👁</button>
                </div>
            </label>
            <div class="modal-buttons">
                <button class="btn btn-primary" id="bkGo">Back Up</button>
                <button class="btn" id="bkCancel">Cancel</button>
            </div>
        `);
        overlay.querySelectorAll('[data-toggle]').forEach(btn => btn.addEventListener('click', () => {
            const input = overlay.querySelector(`#${btn.dataset.toggle}`);
            input.type = input.type === 'password' ? 'text' : 'password';
        }));
        overlay.querySelector('#bkCancel').addEventListener('click', () => overlay.remove());
        overlay.querySelector('#bkGo').addEventListener('click', async () => {
            const p1 = overlay.querySelector('#bkPass').value;
            const p2 = overlay.querySelector('#bkPass2').value;
            if (!p1 || p1 !== p2) { showMessage('Passphrases must match and not be empty.', CONSTANTS.MESSAGE_TYPES.ERROR); return; }

            const { join } = window.__TAURI__.path;
            const dest = await join(settings.backupFolder, `lost-sheep-backup-${todayStamp()}.zip`);

            try {
                // backup_database returns the path it actually wrote to and
                // only after confirming the file is really there — echoed
                // back here so it's obvious where the backup landed instead
                // of a generic "complete" that gives no way to check.
                const writtenPath = await Api.backupDatabase(dest, p1);
                showMessage(`Backup written to ${writtenPath}`, CONSTANTS.MESSAGE_TYPES.INFO, 8000);
                overlay.remove();
            } catch (e) { showMessage(`Backup failed: ${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); }
        });
    },

    async showRestoreModal() {
        const settings = await Api.getSettings().catch(() => ({}));
        const overlay = modalShell(`
            <h2>Restore Database</h2>
            <label>Backup file <button class="btn" id="rsPick">Choose file…</button> <span id="rsPickedPath"></span></label>
            <label>Passphrase <input type="password" id="rsPass"></label>
            <div class="modal-buttons">
                <button class="btn btn-primary" id="rsPreview" disabled>Preview changes</button>
                <button class="btn" id="rsCancel">Cancel</button>
            </div>
            <div id="rsDiffArea"></div>
        `);
        let srcPath = null;
        overlay.querySelector('#rsCancel').addEventListener('click', () => overlay.remove());
        overlay.querySelector('#rsPick').addEventListener('click', async () => {
            const { open } = window.__TAURI__.dialog;
            const { homeDir } = window.__TAURI__.path;
            srcPath = await open({ multiple: false, defaultPath: settings.backupFolder || await homeDir(), filters: [{ name: 'Backup', extensions: ['zip'] }] });
            if (srcPath) {
                overlay.querySelector('#rsPickedPath').textContent = srcPath;
                overlay.querySelector('#rsPreview').disabled = false;
            }
        });
        overlay.querySelector('#rsPreview').addEventListener('click', async () => {
            const pass = overlay.querySelector('#rsPass').value;
            if (!srcPath || !pass) { showMessage('Choose a file and enter the passphrase.', CONSTANTS.MESSAGE_TYPES.ERROR); return; }
            let preview;
            try { preview = await Api.restorePreview(srcPath, pass); }
            catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); return; }
            renderDiff(overlay, preview, srcPath, pass);
        });
    },
};

function renderDiff(overlay, preview, srcPath, pass) {
    const area = overlay.querySelector('#rsDiffArea');
    area.innerHTML = `
        <h3>Before / After</h3>
        <p>Current: ${preview.current_household_count} records. Backup: ${preview.backup_household_count} records.</p>

        <h3>By Tag (current → after restore)</h3>
        <table><thead><tr><th>Tag</th><th>Current</th><th>After Restore</th></tr></thead><tbody>
            ${(preview.tag_counts || []).map(t => `<tr><td>${escapeHtml(t.name)}</td><td>${t.current_count}</td><td>${t.backup_count}</td></tr>`).join('') || '<tr><td colspan="3">No tags.</td></tr>'}
        </tbody></table>

        <h3>Household Changes</h3>
        <table><thead><tr><th>Change</th><th>Record</th></tr></thead><tbody>
            ${preview.rows.map(r => `<tr><td>${r.kind}</td><td>${escapeHtml(r.description)}</td></tr>`).join('') || '<tr><td colspan="2">No differences.</td></tr>'}
        </tbody></table>
        <button class="btn btn-danger" id="rsCommit">Restore Now</button>
    `;
    area.querySelector('#rsCommit').addEventListener('click', async () => {
        if (!confirm('This replaces the current database with the backup. Continue?')) return;
        try {
            await Api.restoreCommit(srcPath, pass);
            showMessage('Restore complete. Please restart the app.', CONSTANTS.MESSAGE_TYPES.INFO, 8000);
            overlay.remove();
        } catch (e) { showMessage(`Restore failed: ${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); }
    });
}

function modalShell(innerHtml) {
    const overlay = document.createElement('div');
    overlay.className = 'modal-overlay';
    overlay.innerHTML = `<div class="modal">${innerHtml}</div>`;
    document.body.appendChild(overlay);
    overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });
    return overlay;
}

function todayStamp() { return new Date().toISOString().slice(0, 10); }

window.BackupRestore = BackupRestore;
