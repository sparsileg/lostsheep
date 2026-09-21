// profile-modal.js — issue #85: hamburger-menu "Switch Profile" flow,
// layered on the commands::profiles Tauri commands (list/get active/
// create/switch/restart) from Piece 1. A profile switch is disruptive —
// it changes which congregation's data every other view in the app is
// looking at — so both switching to an existing profile and creating a
// new one go through an in-app confirm dialog before the app actually
// restarts into the new data. Plain window.confirm() is unreliable inside
// this app's Tauri webview (#74), so this reuses that same pattern
// instead of the native dialog.
//
// Classic script (like households-view.js/review-view.js), not a module —
// nothing here needs the module-only Tauri API imports backup-restore.js
// and roads-ingest.js need.

const ProfileManager = {
    async showModal() {
        let profiles = [];
        let active = null;
        try {
            [profiles, active] = await Promise.all([Api.listProfiles(), Api.getActiveProfile()]);
        } catch (e) {
            showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
            return;
        }

        const overlay = modalShell(`
            <h2>Switch Profile</h2>
            ${profiles.length
                ? '<div id="pmProfileList"></div><hr>'
                : '<p>No profiles yet — this install is using its original, un-profiled database.</p>'}
            <label>New profile name <input type="text" id="pmNewName" placeholder="e.g. Winchester"></label>
            <div id="pmError"></div>
            <div class="modal-buttons">
                <button class="btn btn-primary" id="pmCreate">Create &amp; Switch</button>
                <button class="btn" id="pmCancel">Cancel</button>
            </div>
        `);

        const pmError = overlay.querySelector('#pmError');
        const clearError = () => { pmError.textContent = ''; pmError.className = ''; };
        const showError = (msg) => { pmError.textContent = msg; pmError.className = 'restore-warning'; };

        if (profiles.length) {
            const list = overlay.querySelector('#pmProfileList');
            list.innerHTML = profiles.map(p => {
                const isActive = active && p.slug === active.slug;
                return `<div>
                    <span>${escapeHtml(p.name)}${isActive ? ' (current)' : ''}</span>
                    ${isActive ? '' : `<button class="btn" data-switch-to="${escapeHtml(p.slug)}" data-switch-name="${escapeHtml(p.name)}">Switch</button>`}
                </div>`;
            }).join('');
            list.querySelectorAll('[data-switch-to]').forEach(btn => {
                btn.addEventListener('click', () => switchTo(btn.dataset.switchTo, btn.dataset.switchName));
            });
        }

        async function switchTo(slug, name) {
            const proceed = await confirmProfileAction(
                `Switch to "${name}"? The app will restart, opening that profile's own database.`
            );
            if (!proceed) return;
            try {
                await Api.switchProfile(slug);
                await Api.restartApp();
            } catch (e) {
                showError(`${e}`);
            }
        }

        overlay.querySelector('#pmCancel').addEventListener('click', () => overlay.remove());
        overlay.querySelector('#pmNewName').addEventListener('input', clearError);
        overlay.querySelector('#pmCreate').addEventListener('click', async () => {
            const name = overlay.querySelector('#pmNewName').value.trim();
            clearError();
            if (!name) { showError('Enter a name for the new profile.'); return; }
            const proceed = await confirmProfileAction(
                `Create profile "${name}" and switch to it now? The app will restart with a brand-new, empty database.`
            );
            if (!proceed) return;
            try {
                await Api.createProfile(name);
                await Api.restartApp();
            } catch (e) {
                showError(`${e}`);
            }
        });
    },
};

// In-app confirm — same window.confirm()-is-unreliable-in-this-webview
// reasoning as households-view.js's confirmDiscard (#74). Kept as its own
// copy here rather than importing that one, which is module-local, not
// exported.
function confirmProfileAction(message) {
    return new Promise((resolve) => {
        const confirmOverlay = document.createElement('div');
        confirmOverlay.className = 'modal-overlay';
        confirmOverlay.innerHTML = `
            <div class="modal hh-confirm-modal">
                <p>${escapeHtml(message)}</p>
                <div class="modal-buttons">
                    <button class="btn" id="pmConfirmCancel">Cancel</button>
                    <button class="btn btn-primary" id="pmConfirmGo">Continue</button>
                </div>
            </div>`;
        document.body.appendChild(confirmOverlay);
        const finish = (result) => { confirmOverlay.remove(); resolve(result); };
        document.getElementById('pmConfirmGo').addEventListener('click', () => finish(true));
        document.getElementById('pmConfirmCancel').addEventListener('click', () => finish(false));
        confirmOverlay.addEventListener('click', (e) => { if (e.target === confirmOverlay) finish(false); });
    });
}

window.ProfileManager = ProfileManager;
