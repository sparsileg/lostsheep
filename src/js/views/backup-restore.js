// backup-restore.js — modal flows invoked from the hamburger menu.
import { join, homeDir } from '../../include/tauri-api/path.js';
import { open } from '../../include/tauri-api/dialog.js';

// Issue #25: this passphrase is the only thing protecting the entire
// congregation directory once it leaves the OS keychain's protection
// (email, shared drive, USB stick). Argon2id can't compensate for a
// one-character secret, so a floor is enforced here.
const MIN_PASSPHRASE_LEN = 10;

const BackupRestore = {
    async showBackupModal() {
        const settings = await Api.getSettings().catch(() => ({}));
        if (!settings.backupFolder) {
            showMessage('Set a backup folder in Settings before backing up.', CONSTANTS.MESSAGE_TYPES.ERROR);
            return;
        }

        const overlay = modalShell(`
            <h2>Backup Database</h2>
            <p>Choose a passphrase to protect the backup — store it safely, it's required to restore.
               This passphrase is the only protection on the file once it leaves this computer.
               Minimum ${MIN_PASSPHRASE_LEN} characters.</p>
            <label>Passphrase
                <div class="passphrase-row">
                    <input type="password" id="bkPass" size="20">
                </div>
            </label>
            <label>Confirm passphrase
                <div class="passphrase-row">
                    <input type="password" id="bkPass2" size="20">
                    <button type="button" class="btn passphrase-toggle" id="bkPassToggle">👁</button>
                </div>
            </label>
            <div class="modal-buttons">
                <button class="btn btn-primary" id="bkGo">Back Up</button>
                <button class="btn" id="bkCancel">Cancel</button>
            </div>
        `);
        overlay.querySelector('#bkPassToggle').addEventListener('click', () => {
            const p1 = overlay.querySelector('#bkPass');
            const p2 = overlay.querySelector('#bkPass2');
            const nextType = p1.type === 'password' ? 'text' : 'password';
            p1.type = nextType;
            p2.type = nextType;
        });
        overlay.querySelector('#bkCancel').addEventListener('click', () => overlay.remove());
        overlay.querySelector('#bkGo').addEventListener('click', async () => {
            const p1 = overlay.querySelector('#bkPass').value;
            const p2 = overlay.querySelector('#bkPass2').value;
            if (!p1 || p1 !== p2) { showMessage('Passphrases must match and not be empty.', CONSTANTS.MESSAGE_TYPES.ERROR); return; }
            if (p1.length < MIN_PASSPHRASE_LEN) { showMessage(`Passphrase must be at least ${MIN_PASSPHRASE_LEN} characters.`, CONSTANTS.MESSAGE_TYPES.ERROR); return; }

            const dest = await join(settings.backupFolder, `lost-sheep-backup-${todayStamp()}.zip`);

            try {
                // backup_database returns the path it actually wrote to and
                // only after confirming the file is really there — echoed
                // back here so it's obvious where the backup landed instead
                // of a generic "complete" that gives no way to check.
                const writtenPath = await Api.backupDatabase(dest, p1);
                showMessage(`Backup written to ${writtenPath} (road graph not included — re-ingest after restore if needed)`, CONSTANTS.MESSAGE_TYPES.INFO, 8000);
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
            ${preview.rows.map(r => `<tr><td>${escapeHtml(r.kind)}</td><td>${escapeHtml(r.description)}</td></tr>`).join('') || '<tr><td colspan="2">No differences.</td></tr>'}
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

// Modules don't leak top-level declarations onto `window` the way classic
// scripts did — roads-ingest.js uses this as a bare global (see its own
// header comment), so it needs an explicit export now that both files are
// modules (withGlobalTauri: false follow-on, issue #17/#18).
window.modalShell = modalShell;
window.BackupRestore = BackupRestore;
