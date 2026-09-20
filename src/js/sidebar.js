// sidebar.js — theme/font-size/hamburger chrome. Settings persist via
// Api.getSettings/saveSettings (backed by the `settings` table).
const THEME_LABELS = {
    'css/themes/nordic.css': 'Nordic',
    'css/themes/dark.css': 'Dark',
    'css/themes/light.css': 'Light',
    'css/themes/matrix.css': 'Matrix',
    'css/themes/flat.css': 'Flat',
};

async function initSidebarChrome() {
    wireThemeDropdownItems();
    wireChromeEvents();
    renderVersion();

    let settings = {};
    try { settings = await Api.getSettings(); } catch (e) { console.error(e); }
    if (settings.theme) selectTheme(settings.theme, THEME_LABELS[settings.theme] || 'Theme', false);
    applyFontSize(parseInt(settings.fontSize || '16', 10));
}

function renderVersion() {
    const el = document.getElementById('sidebarVersionFooter');
    if (el) el.textContent = `v${CONSTANTS.APP_VERSION}`;
}

function wireThemeDropdownItems() {
    document.querySelectorAll('.theme-dropdown-item').forEach(item => {
        item.addEventListener('click', () => selectTheme(item.getAttribute('data-theme'), item.textContent, true));
    });
    document.addEventListener('click', (e) => {
        const dd = document.getElementById('theme-dropdown-menu');
        if (dd && !dd.contains(e.target) && !e.target.closest('#theme-dropdown-trigger')) dd.classList.remove('open');
    });
}

async function selectTheme(path, label, persist) {
    document.getElementById('themeLink').setAttribute('href', path);
    document.getElementById('theme-dropdown-label').textContent = label;
    document.getElementById('theme-dropdown-menu').classList.remove('open');
    if (persist) {
        try { await Api.saveSettings({ theme: path }); } catch (e) { console.error(e); }
    }
}

function wireChromeEvents() {
    document.getElementById('theme-dropdown-trigger')?.addEventListener('click', () =>
        document.getElementById('theme-dropdown-menu').classList.toggle('open'));
    document.getElementById('font-size-down')?.addEventListener('click', () => adjustFontSize(-1));
    document.getElementById('font-size-up')?.addEventListener('click', () => adjustFontSize(1));
    document.getElementById('hamburger-btn')?.addEventListener('click', toggleHamburgerMenu);
    document.getElementById('mobile-menu-btn')?.addEventListener('click', toggleMobileSidebar);
    document.getElementById('hamburgerMenu')?.addEventListener('click', handleHamburgerMenuClick);
    document.addEventListener('click', (e) => {
        if (!e.target.closest('#hamburgerMenu') && !e.target.closest('#hamburger-btn')) closeHamburgerMenu();
    });
    window.addEventListener('resize', () => { if (window.innerWidth > 768) closeMobileSidebar(); });
}

function applyFontSize(px) {
    document.documentElement.style.fontSize = `${px}px`;
    const label = document.getElementById('font-size-label');
    if (label) label.textContent = `Size: ${px}`;
}

async function adjustFontSize(direction) {
    let settings = {};
    try { settings = await Api.getSettings(); } catch (e) { console.error(e); }
    const current = parseInt(settings.fontSize || '16', 10);
    const next = Math.min(30, Math.max(8, current + direction));
    applyFontSize(next);
    try { await Api.saveSettings({ fontSize: String(next) }); } catch (e) { console.error(e); }
}

function toggleHamburgerMenu() {
    const menu = document.getElementById('hamburgerMenu');
    const btn = document.getElementById('hamburger-btn');
    if (!menu || !btn) return;
    const rect = btn.getBoundingClientRect();
    menu.style.top = `${rect.bottom + 8}px`;
    menu.style.left = `${Math.max(8, Math.min(rect.left, window.innerWidth - 228))}px`;
    menu.classList.toggle('open');
}
function closeHamburgerMenu() { document.getElementById('hamburgerMenu')?.classList.remove('open'); }

function toggleMobileSidebar() { document.getElementById('appShell')?.classList.toggle('mobile-sidebar-open'); }
function closeMobileSidebar() { document.getElementById('appShell')?.classList.remove('mobile-sidebar-open'); }

function handleHamburgerMenuClick(e) {
    const item = e.target.closest('[data-action]');
    if (!item) return;
    closeHamburgerMenu();
    switch (item.dataset.action) {
        case 'settings': openSettingsModal(); break;
        case 'backup': BackupRestore.showBackupModal(); break;
        case 'restore': BackupRestore.showRestoreModal(); break;
        case 'roads': RoadsIngest.showModal(); break;
        case 'diagnostics': showPotentialProblemsModal(); break;
        case 'logs': showView('logs'); break;
        case 'about': showAboutModal(); break;
        case 'help': showHelpModal(); break;
    }
}

function showAboutModal() {
    const overlay = document.createElement('div');
    overlay.className = 'modal-overlay';
    overlay.innerHTML = `<div class="modal">
        <h2>About Lost Sheep</h2>
        <p>Version ${CONSTANTS.APP_VERSION}</p>
        <h3>Third-Party Software</h3>
        <p><strong>pdftotext</strong> (poppler-utils) is bundled for PDF import.
        Licensed under the GPL. Source:
        <a href="https://github.com/unpins/poppler-utils/releases" target="_blank" rel="noopener">unpins/poppler-utils</a>.</p>
        <p>Road and map data &copy; <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noopener">OpenStreetMap contributors</a>,
        licensed under the Open Database License (ODbL). Map tile images are fetched from OpenStreetMap's
        servers over the internet as needed; this discloses the general area being viewed to that server.
        Already-fetched tiles are cached locally and are not re-fetched until you clear the cache
        (Settings &rarr; Refresh Map).</p>
        <button class="btn" id="closeAbout">Close</button>
    </div>`;
    document.body.appendChild(overlay);
    overlay.querySelector('#closeAbout').addEventListener('click', () => overlay.remove());
    overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });
}

function showHelpModal() {
    const overlay = document.createElement('div');
    overlay.className = 'modal-overlay';
    overlay.innerHTML = `<div class="modal">
        <h2>Help</h2>
        <h3>First run</h3>
        <p>Use Import to load your congregational directory PDF. New and changed households land in
        Review Updates for you to confirm before they're saved.</p>
        <h3>OS keychain not working?</h3>
        <p>If the app can't unlock the database on startup, restore your most recent backup file
        (hamburger menu &rarr; Restore) — the backup is independent of this machine's keychain.</p>
        <button class="btn" id="closeHelp">Close</button>
    </div>`;
    document.body.appendChild(overlay);
    overlay.querySelector('#closeHelp').addEventListener('click', () => overlay.remove());
    overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });
}

// Issue #48 — read-only diagnostic scan. Runs on demand, not cached; a
// fresh scan is cheap enough at this app's target scale (<10,000
// households) to just re-run every time the menu item is clicked.
async function showPotentialProblemsModal() {
    let unlisten = null;
    const cleanup = () => {
        if (unlisten) { unlisten(); unlisten = null; }
        ProgressRing.hide();
    };

    let report;
    try {
        // sidebar.js is a classic script (no static import support), same
        // constraint roads-ingest.js's module doesn't have — dynamic
        // import() reaches the same vendored event API from here.
        const { listen } = await import('../include/tauri-api/event.js');
        ProgressRing.show('Scanning households…');
        unlisten = await listen('diagnostics-progress', (event) => {
            const { processed, total } = event.payload;
            ProgressRing.update(total > 0 ? (processed / total) * 100 : 100);
        });
        report = await Api.findPotentialProblems();
    } catch (e) {
        console.error('findPotentialProblems failed', e);
        cleanup();
        showMessage('Could not run diagnostics — see console for details.', CONSTANTS.MESSAGE_TYPES.ERROR);
        return;
    }
    cleanup();

    const { problems, roads_checked, missing_coords } = report;

    const overlay = document.createElement('div');
    overlay.className = 'modal-overlay';
    overlay.innerHTML = `<div class="modal" style="max-width:700px;max-height:80vh;overflow-y:auto;">
        <h2>Potential Problems</h2>
        <div id="potentialProblemsBody"></div>
        <div style="margin-top:12px;">
            <button class="btn" id="closePotentialProblems">Close</button>
        </div>
    </div>`;
    document.body.appendChild(overlay);
    overlay.querySelector('#closePotentialProblems').addEventListener('click', () => overlay.remove());
    overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });

    const body = overlay.querySelector('#potentialProblemsBody');

    // Issue #80: roads_checked is checked BEFORE the "nothing flagged"
    // shortcut below — an empty/un-ingested roads.db and a fully-working
    // one with zero findings used to render identically ("No potential
    // problems found."), which is exactly the silent-degradation this
    // issue is about. false here means the street-name/snap checks never
    // ran for any household this scan, not that they ran clean.
    const roadsWarning = !roads_checked
        ? '<p style="color:#b02a2a;font-weight:bold;">⚠ Road database has no data — street-name checks were skipped for every household this scan. Re-ingest under Road Management to restore them.</p>'
        : '';

    // #64 follow-up — missing_coords is a separate list from problems (see
    // diagnostics.rs's MissingCoordsEntry doc comment), so "nothing to
    // report" now means both are empty, not just problems.
    const missingCoordsCount = (missing_coords || []).length;
    if (!problems.length && !missingCoordsCount) {
        body.innerHTML = roadsWarning || '<p>No potential problems found.</p>';
        // Still worth a PDF when roads were skipped, even with zero
        // findings — the report itself is the durable record of why
        // fewer/no issues showed up this time.
        if (!roads_checked) PotentialProblemsPdf.download(problems, roads_checked, missing_coords);
        return;
    }

    // Report is generated and saved automatically — no on-screen results
    // list, no separate Download button. pdfmake's own .download() call
    // (PotentialProblemsPdf.download) is what actually saves it; this app
    // has no way to know the real destination path (webview download
    // behavior, OS/user-config dependent), so the modal states the
    // presumed default rather than a path it can't actually confirm.
    PotentialProblemsPdf.download(problems, roads_checked, missing_coords);
    const summaryParts = [];
    if (problems.length) summaryParts.push(`${problems.length} household(s) flagged`);
    if (missingCoordsCount) summaryParts.push(`${missingCoordsCount} household(s) missing geo-coordinates`);
    body.innerHTML = roadsWarning + `<p>${summaryParts.join('; ')}. Report saved to your Downloads folder.</p>`;
}

function updateHamburgerContextualSection() { /* no per-view hamburger sections in v1 */ }
