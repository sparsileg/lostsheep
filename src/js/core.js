// core.js — app boot, view router, tiny shared utilities.
const CONSTANTS = {
    APP_VERSION: '1.3.0',
    VIEWS: ['import', 'review', 'households', 'visits-report', 'deleted-records', 'map', 'logs'],
    MESSAGE_TYPES: { INFO: 'info', ERROR: 'error', WARNING: 'warning' },
};

function escapeHtml(str) {
    if (str == null) return '';
    return String(str)
        .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

// A single persistent slot, not a new element per message — appending
// and removing divs was causing the whole page to shift as the message
// area's height changed. The bar itself (background/border, see
// #messageArea in base.css) is always visible from app start, whether or
// not there's currently a message; only the text and its accent color
// change over time. That's what makes it read as a real status bar
// instead of space that happens to be reserved but invisible.
let messageHideTimer = null;
function initMessageBar() {
    const area = document.getElementById('messageArea');
    if (!area || area.querySelector('.message-slot')) return;
    const slot = document.createElement('div');
    slot.className = 'message-slot';
    slot.textContent = 'Ready';
    area.appendChild(slot);
}

function showMessage(text, type = CONSTANTS.MESSAGE_TYPES.INFO, timeoutMs = 15000) {
    const area = document.getElementById('messageArea');
    if (!area) return;
    initMessageBar();
    const slot = area.querySelector('.message-slot');
    slot.className = `message-slot ${type}`;
    slot.textContent = text;
    clearTimeout(messageHideTimer);
    if (timeoutMs) {
        messageHideTimer = setTimeout(() => {
            slot.className = 'message-slot';
            slot.textContent = 'Ready';
        }, timeoutMs);
    }
}

// Views register an init(context) function here; called once each time
// the view is shown so it can refresh its data.
const ViewRegistry = {};
function registerView(name, handlers) { ViewRegistry[name] = handlers; }

function showView(viewName, navEl) {
    document.querySelectorAll('.view').forEach(v => v.classList.remove('active'));
    const target = document.getElementById(`view-${viewName}`);
    if (target) target.classList.add('active');

    document.querySelectorAll('.nav-item').forEach(li => li.classList.remove('active'));
    if (navEl) navEl.classList.add('active');
    else {
        const li = document.querySelector(`.nav-item[data-view="${viewName}"]`);
        if (li) li.classList.add('active');
    }

    if (typeof updateHamburgerContextualSection === 'function') {
        updateHamburgerContextualSection(viewName);
    }

    const handlers = ViewRegistry[viewName];
    if (handlers && typeof handlers.onShow === 'function') {
        Promise.resolve(handlers.onShow()).catch(e => {
            console.error(`view '${viewName}' failed to load`, e);
            showMessage(`Could not load ${viewName}: ${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
        });
    }
}

const Core = {
    async init() {
        initMessageBar();

        // Nav list is built by sidebar.js's initNavigation(); wire it here
        // so every view is reachable even if a later init step throws.
        // "Dashboard" IS the map/visit-list view now — no separate
        // dashboard page, and Tags has no standalone view anymore either
        // (tag management happens through the household detail modal).
        const NAV = [
            { view: 'map', label: 'Dashboard' },
            { view: 'import', label: 'Import' },
            { view: 'review', label: 'Review Updates' },
            { view: 'households', label: 'Households' },
            { view: 'visits-report', label: 'Visit Report' },
            { view: 'deleted-records', label: 'Deleted Records' },
        ];
        const list = document.getElementById('sidebarNavList');
        NAV.forEach(item => {
            const li = document.createElement('li');
            li.className = 'nav-item';
            li.textContent = item.label;
            li.dataset.view = item.view;
            li.addEventListener('click', () => { showView(item.view, li); closeMobileSidebar(); });
            list.appendChild(li);
        });

        if (typeof initSidebarChrome === 'function') await initSidebarChrome();

        Object.keys(ViewRegistry).forEach(name => {
            if (typeof ViewRegistry[name].init === 'function') ViewRegistry[name].init();
        });

        showView('map', list.querySelector('.nav-item'));

        // #58: was an unattended, unprompted prune at Rust startup —
        // silently destroying deleted households' entire visit history
        // after as little as a month. Not awaited: this is a courtesy
        // check, it must never delay the app becoming usable.
        checkStartupPruneCandidates();
    },
};

// ── Startup retention-cleanup confirmation (#58) ────────────────────────
// Nothing is ever pruned without the user seeing exactly what would go
// first. Runs on every launch; if there's nothing past the retention
// window, it's a no-op and nothing is shown.
async function checkStartupPruneCandidates() {
    let candidates;
    try {
        candidates = await Api.listPruneCandidates();
    } catch (e) {
        console.error('checkStartupPruneCandidates: could not load candidates', e);
        return;
    }
    const households = candidates.deleted_households || [];
    const logs = candidates.logs || 0;
    if (households.length === 0 && logs === 0) return;
    showPruneConfirmModal(households, logs);
}

function daysAgoLabel(isoString) {
    const then = new Date(isoString).getTime();
    if (isNaN(then)) return '';
    const days = Math.floor((Date.now() - then) / (1000 * 60 * 60 * 24));
    if (days <= 0) return 'today';
    if (days === 1) return '1 day ago';
    return `${days} days ago`;
}

function showPruneConfirmModal(households, logs) {
    // Same defensive clear as settings-modal.js — never stack an
    // unremoved overlay under a new one.
    document.querySelectorAll('.modal-overlay').forEach(el => el.remove());

    const overlay = document.createElement('div');
    overlay.className = 'modal-overlay';

    const rows = households.map(h => `
        <tr>
            <td>${escapeHtml(h.first_name)} ${escapeHtml(h.last_name)}</td>
            <td>${escapeHtml(daysAgoLabel(h.deleted_at))}</td>
        </tr>
    `).join('');

    overlay.innerHTML = `
        <div class="modal">
            <h2>Retention cleanup ready</h2>
            <p>These records have passed the retention period set in Settings and are due to
               be permanently removed, including all visit history and comments. This cannot
               be undone.</p>
            ${households.length > 0 ? `
                <table class="kw-table">
                    <thead><tr><th>Household</th><th>Deleted</th></tr></thead>
                    <tbody>${rows}</tbody>
                </table>
            ` : ''}
            <p>${households.length} deleted household${households.length === 1 ? '' : 's'}, ${logs} log entrie${logs === 1 ? '' : 's'}.</p>
            <div class="modal-buttons">
                <button class="btn btn-primary" id="pruneConfirmBtn">Delete permanently</button>
                <button class="btn" id="pruneCancelBtn">Not now</button>
            </div>
        </div>`;
    document.body.appendChild(overlay);

    // "Not now" just closes — nothing is dismissed permanently, the same
    // candidates (plus whatever else has aged past the window since) are
    // offered again next launch.
    document.getElementById('pruneCancelBtn').addEventListener('click', () => overlay.remove());
    overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });

    document.getElementById('pruneConfirmBtn').addEventListener('click', async () => {
        try {
            const result = await Api.pruneOldDeletedAndLogs();
            showMessage(
                `Removed ${result.deleted_households} deleted household(s), ${result.logs} log entrie(s).`,
                CONSTANTS.MESSAGE_TYPES.SUCCESS
            );
        } catch (e) {
            console.error('showPruneConfirmModal: prune failed', e);
            showMessage(`Retention cleanup failed: ${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
        }
        overlay.remove();
    });
}

window.CONSTANTS = CONSTANTS;
window.escapeHtml = escapeHtml;
window.showMessage = showMessage;
window.registerView = registerView;
window.showView = showView;
window.Core = Core;
