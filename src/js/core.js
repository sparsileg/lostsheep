// core.js — app boot, view router, tiny shared utilities.
const CONSTANTS = {
    APP_VERSION: '0.1.0',
    VIEWS: ['import', 'review', 'households', 'map', 'logs'],
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

function showMessage(text, type = CONSTANTS.MESSAGE_TYPES.INFO, timeoutMs = 4000) {
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
    },
};

window.CONSTANTS = CONSTANTS;
window.escapeHtml = escapeHtml;
window.showMessage = showMessage;
window.registerView = registerView;
window.showView = showView;
window.Core = Core;
