registerView('households', {
    init() {
        document.getElementById('householdsRoot').innerHTML = `
            <h1>Households</h1>
            <div class="households-toolbar">
                <div class="hh-search-wrap">
                    <input type="text" id="hhSearchInput" placeholder="Search name, address, phone, email, comments…" />
                    <button type="button" class="hh-search-clear" id="hhSearchClearBtn" aria-label="Clear search" title="Clear search">&times;</button>
                </div>
                <div id="hhTagFilterDropdown"></div>
                <button class="btn" id="hhGenerateDirectoryBtn">Generate Directory PDF</button>
                <button class="btn" id="hhPasteVisitBtn">Paste Visit</button>
            </div>
            <div id="hhResultsMeta"></div>
            <table id="hhTable">
                <thead><tr><th>Name</th><th>Address</th><th>Tag</th><th></th></tr></thead>
                <tbody id="hhTableBody"></tbody>
            </table>
            <div id="hhPager"></div>
        `;
        state.page = 1;
        const hhSearchInput = document.getElementById('hhSearchInput');
        const hhSearchClearBtn = document.getElementById('hhSearchClearBtn');
        // Shared search string (SharedSearch, defined near the top of
        // map-view.js) — Stan's ask: typing "Walker" here and switching to
        // the Dashboard should show "Walker" there too, and back. Seed
        // this input from whatever's already there (e.g. typed on the
        // Dashboard first) rather than always starting blank.
        hhSearchInput.value = window.SharedSearch.query;
        syncHhSearchClearBtnVisibility();
        hhSearchInput.addEventListener('input', () => {
            window.SharedSearch.query = hhSearchInput.value;
            syncHhSearchClearBtnVisibility();
        });
        hhSearchInput.addEventListener('input', debounce(() => { state.page = 1; loadHouseholds(); }, 300));
        hhSearchClearBtn.addEventListener('click', () => {
            hhSearchInput.value = '';
            window.SharedSearch.query = '';
            syncHhSearchClearBtnVisibility();
            hhSearchInput.focus();
            state.page = 1;
            loadHouseholds();
        });
        // Tags are capped at one per household now, so filtering by more
        // than one at once would always return nothing — single-select,
        // not the old multi-chip filter.
        state.tagFilterDropdown = mountDropdown(document.getElementById('hhTagFilterDropdown'), {
            items: [{ value: '', label: 'All' }],
            value: '',
            onSelect: (val) => {
                state.tagFilter = val || null;
                state.page = 1;
                loadHouseholds();
            },
        });
        document.getElementById('hhGenerateDirectoryBtn').addEventListener('click', generateDirectoryPdf);
        document.getElementById('hhPasteVisitBtn').addEventListener('click', openPasteVisitModal);
    },
    async onShow() {
        const settings = await Api.getSettings().catch(() => ({}));
        state.pageSize = parseInt(settings.pageSize || '25', 10) || 25;
        await refreshTagFilterOptions();
        // Re-sync from SharedSearch every time this view is shown — the
        // Dashboard's own search box may have changed it since init() ran.
        const hhSearchInput = document.getElementById('hhSearchInput');
        if (hhSearchInput) hhSearchInput.value = window.SharedSearch.query;
        syncHhSearchClearBtnVisibility();
        await loadHouseholds();
    },
});

// Cross-view shared search string (see map-view.js's own copy of this
// guard) — whichever of households-view.js/map-view.js loads first wins
// the actual creation; the other's identical guard then finds it already
// there and leaves it alone, so load order between the two files doesn't
// matter.
window.SharedSearch = window.SharedSearch || { query: '' };

const state = { page: 1, pageSize: 25, tagFilter: null, lastResult: null };

// Pulled out of init()'s closure so onShow() can also call it without
// duplicating the DOM lookups.
function syncHhSearchClearBtnVisibility() {
    const input = document.getElementById('hhSearchInput');
    const btn = document.getElementById('hhSearchClearBtn');
    if (input && btn) btn.classList.toggle('hh-search-clear-visible', input.value.length > 0);
}

function debounce(fn, ms) { let t; return (...a) => { clearTimeout(t); t = setTimeout(() => fn(...a), ms); }; }

// The old /^\d{4}-\d{2}-\d{2}$/ check only validated shape, not whether the
// date exists — it happily accepted "2026-13-45" and "2026-02-30" (#35).
// This is a convenience check only; record_visit on the backend is the
// load-bearing validation and rejects the same cases plus non-padded input.
function isValidIsoDate(str) {
    const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(str);
    if (!m) return false;
    const y = Number(m[1]), mo = Number(m[2]), d = Number(m[3]);
    const dt = new Date(Date.UTC(y, mo - 1, d));
    return dt.getUTCFullYear() === y && dt.getUTCMonth() === mo - 1 && dt.getUTCDate() === d;
}

// "Lastname, First1[ & First2]" — mimics the source directory's own
// header-line format for a household entry.
function formatDirectoryName(h) {
    let out = `${h.last_name}, ${h.first_name}`;
    if (h.first_name_2) out += ` & ${h.first_name_2}`;
    return out;
}

async function refreshTagFilterOptions() {
    const tags = await Api.listTags().catch(() => []);
    state.tagFilterDropdown?.setItems([
        { value: '', label: 'All' },
        ...tags.map(t => ({ value: t.name, label: `${t.name} (${t.household_count})` })),
    ]);
}

async function loadHouseholds() {
    const query = document.getElementById('hhSearchInput').value;
    const params = { query, tag_names: state.tagFilter ? [state.tagFilter] : [], page: state.page, page_size: state.pageSize };
    let result;
    try { result = await Api.searchHouseholds(params); } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); return; }
    state.lastResult = result;

    document.getElementById('hhResultsMeta').textContent = `${result.total} household record(s)`;
    document.getElementById('hhTableBody').innerHTML = result.households.map(h => {
        const isKnown = (h.tags || []).includes('Known');
        const label = isKnown ? 'Not Known' : 'Known';
        const targetTag = isKnown ? 'Not known' : 'Known';
        return `
        <tr class="hh-row" data-open="${h.id}">
            <td>${escapeHtml(formatDirectoryName(h))}</td>
            <td>${escapeHtml(h.address_line1)}${h.address_line2 ? ', ' + escapeHtml(h.address_line2) : ''}${h.city ? ', ' + escapeHtml(h.city) : ''}</td>
            <td>${renderTagChips(h.tags)}</td>
            <td><button class="btn" data-known="${h.id}" data-target-tag="${escapeHtml(targetTag)}">${label}</button></td>
        </tr>`;
    }).join('');

    document.querySelectorAll('[data-open]').forEach(tr => tr.addEventListener('click', () => openHousehold(Number(tr.dataset.open))));
    document.querySelectorAll('[data-known]').forEach(btn => btn.addEventListener('click', (e) => {
        e.stopPropagation(); // don't also trigger the row's open-modal click
        markKnown(Number(btn.dataset.known), btn.dataset.targetTag);
    }));

    const totalPages = Math.max(1, Math.ceil(result.total / state.pageSize));
    document.getElementById('hhPager').innerHTML = `
        <button class="btn" id="hhPrevPage" ${state.page <= 1 ? 'disabled' : ''}>‹ Prev</button>
        Page ${state.page} / ${totalPages}
        <button class="btn" id="hhNextPage" ${state.page >= totalPages ? 'disabled' : ''}>Next ›</button>`;
    document.getElementById('hhPrevPage')?.addEventListener('click', () => {
        state.page = Math.max(1, state.page - 1);
        loadHouseholds();
    });
    document.getElementById('hhNextPage')?.addEventListener('click', () => {
        state.page = Math.min(totalPages, state.page + 1);
        loadHouseholds();
    });
}

// Issue #15 — pulls every household matching the current search/tag
// filter, not just the page on screen, for the PDF directory export.
// Reuses search_households (same query the table already shows) rather
// than a second filtering implementation. page_size is server-clamped to
// 500 (households.rs), so results beyond that are paged through here;
// household counts in this app are documented to stay well under that
// per page in practice, but the loop is unconditional so it's correct
// regardless of scale.
async function fetchAllFilteredHouseholds() {
    const query = document.getElementById('hhSearchInput').value;
    const tag_names = state.tagFilter ? [state.tagFilter] : [];
    const page_size = 500;
    let page = 1;
    let all = [];
    for (;;) {
        const result = await Api.searchHouseholds({ query, tag_names, page, page_size });
        all = all.concat(result.households);
        if (all.length >= result.total || result.households.length === 0) break;
        page += 1;
    }
    return all;
}

async function generateDirectoryPdf() {
    const btn = document.getElementById('hhGenerateDirectoryBtn');
    btn.disabled = true;
    const originalLabel = btn.textContent;
    btn.textContent = 'Generating…';
    try {
        const households = await fetchAllFilteredHouseholds();
        if (households.length === 0) {
            showMessage('No households match the current filter.', CONSTANTS.MESSAGE_TYPES.INFO);
            return;
        }
        DirectoryPdf.download(households, state.tagFilter || 'All');
    } catch (e) {
        showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
    } finally {
        btn.disabled = false;
        btn.textContent = originalLabel;
    }
}

// Clears whatever tag was there and sets targetTag ("Known" or "Not
// known"), then reloads the current page from the server. Always
// reloads now — even when the active filter still matches the new tag
// or "All" — rather than removing just this row from the DOM:
// local-only removal left state.page pointing at a numeric offset that
// no longer matched the (now smaller) filtered result set, silently
// skipping unreviewed households when the user hit Next (issue #5).
//
// allowSystemTagChange: false (#59) — this quick-toggle button must
// never displace a "Do not contact" tag. A household holding one comes
// back from tag_households untouched, reported via skipped_system; the
// user gets a toast explaining why instead of the tag silently staying
// put with no indication anything was refused.
async function markKnown(id, targetTag) {
    try {
        const result = await Api.tagHouseholds([id], targetTag, false);
        if (result.skipped_system > 0) {
            showMessage(
                "This household is marked Do not contact — that can only be changed from the household's own edit screen.",
                CONSTANTS.MESSAGE_TYPES.WARNING,
                5000
            );
        }
        await loadHouseholds();
        await refreshTagFilterOptions();
    } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); }
}

// Small in-app confirm dialog. window.confirm() is unreliable inside the
// Tauri webview (#74 — it can resolve without ever showing a prompt), so
// anything that must actually block the user gets its own overlay instead
// of a native dialog. Returns a Promise<boolean>: true = user chose to
// proceed (discard), false = cancel (backdrop click counts as cancel).
// Generalized from the original confirmDiscard (Discard-labeled only) so
// the same in-app dialog (see #74 above for why not window.confirm()) can
// also confirm a visit delete with its own button label. confirmDiscard
// keeps its original name/signature as a thin wrapper — every existing
// call site is unaffected.
function confirmYesNo(message, confirmLabel) {
    return new Promise((resolve) => {
        const confirmOverlay = document.createElement('div');
        confirmOverlay.className = 'modal-overlay';
        confirmOverlay.innerHTML = `
            <div class="modal hh-confirm-modal">
                <p>${escapeHtml(message)}</p>
                <div class="modal-buttons">
                    <button class="btn" id="fConfirmCancel">Cancel</button>
                    <button class="btn btn-primary" id="fConfirmYes">${escapeHtml(confirmLabel)}</button>
                </div>
            </div>`;
        document.body.appendChild(confirmOverlay);
        const finish = (result) => { confirmOverlay.remove(); resolve(result); };
        document.getElementById('fConfirmYes').addEventListener('click', () => finish(true));
        document.getElementById('fConfirmCancel').addEventListener('click', () => finish(false));
        confirmOverlay.addEventListener('click', (e) => { if (e.target === confirmOverlay) finish(false); });
    });
}

function confirmDiscard(message) {
    return confirmYesNo(message, 'Discard');
}

// Paste Visit — Stan pastes one row copied straight from a spreadsheet of
// prior visits (tab-separated: Name, Address, Phone, Frequency, Email,
// Last Visit, Comments — see formatDirectoryName for the name column's
// expected "Last, First1[ & First2]" shape). Address/phone/frequency/email
// are read but deliberately unused: they're for the user's own eyeballing
// only, and updating household fields from them isn't in scope here —
// name/address corrections go through re-import + Review, same as
// the household page's own doc comment says (household-view.js). Only name (for search),
// last-visit date, and comments are used. Multiple pasted rows aren't
// supported — only the first non-empty line is read — matching how Stan
// described the feature (one row per paste).
function nameSearchWords(raw) {
    // Strips the punctuation formatDirectoryName's own "Last, First1 &
    // First2" format adds back in, leaving bare words — findHousehold
    // above searches each one separately and unions the results (OR), so
    // a name column with a word not present verbatim on the household
    // record still turns up whatever the other words do match.
    return (raw || '').replace(/[,&]/g, ' ').split(/\s+/).map(w => w.trim()).filter(Boolean);
}

// Converts the loose date formats a "Last Visit" spreadsheet column tends
// to hold into the YYYY-MM-DD record_visit requires. Unparseable input
// returns null rather than guessing — the confirm screen below always
// shows an editable date field, defaulting to empty (not today's date)
// when this returns null, so a bad parse can never silently save the
// wrong date.
function parseApproxDate(raw) {
    const s = (raw || '').trim();
    if (!s) return null;
    if (isValidIsoDate(s)) return s;
    // "YYYY-MM" or "YYYY-M" -> day 01
    let m = /^(\d{4})-(\d{1,2})$/.exec(s);
    if (m) return `${m[1]}-${String(m[2]).padStart(2, '0')}-01`;
    // "M/YYYY" or "MM/YYYY" -> day 01
    m = /^(\d{1,2})\/(\d{4})$/.exec(s);
    if (m) return `${m[2]}-${String(m[1]).padStart(2, '0')}-01`;
    // "Apr 2022", "April 2022", "Apr. 2022" -> day 01
    const months = { jan: 1, feb: 2, mar: 3, apr: 4, may: 5, jun: 6, jul: 7, aug: 8, sep: 9, oct: 10, nov: 11, dec: 12 };
    m = /^([A-Za-z]{3,9})\.?\s+(\d{4})$/.exec(s);
    if (m) {
        const mo = months[m[1].slice(0, 3).toLowerCase()];
        if (mo) return `${m[2]}-${String(mo).padStart(2, '0')}-01`;
    }
    return null;
}

async function openPasteVisitModal() {
    const overlay = document.createElement('div');
    overlay.className = 'modal-overlay';
    overlay.innerHTML = `
        <div class="modal hh-paste-visit-modal">
            <h2>Paste Visit</h2>
            <p>Paste one spreadsheet row — Name, Address, Phone, Frequency, Email, Last Visit, Comments, tab-separated (copied straight from Excel/Sheets). Only the name, last-visit date, and comments columns are used.</p>
            <textarea id="pvRaw" rows="4" placeholder="Paste a row here…"></textarea>
            <div id="pvError"></div>
            <div class="modal-buttons">
                <button class="btn btn-primary" id="pvFind">Find Household</button>
                <button class="btn" id="pvCancel">Cancel</button>
            </div>
            <div id="pvMatches"></div>
        </div>`;
    document.body.appendChild(overlay);

    const pvError = overlay.querySelector('#pvError');
    const showError = (msg) => { pvError.textContent = msg; pvError.className = 'restore-warning'; };
    const clearError = () => { pvError.textContent = ''; pvError.className = ''; };

    overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });
    overlay.querySelector('#pvCancel').addEventListener('click', () => overlay.remove());

    async function findHousehold() {
        clearError();
        const matchesEl = overlay.querySelector('#pvMatches');
        matchesEl.innerHTML = '';
        const raw = overlay.querySelector('#pvRaw').value;
        const line = raw.split('\n').map(l => l.trim()).find(l => l.length > 0);
        if (!line) { showError('Paste a row first.'); return; }
        const cols = line.split('\t');
        if (cols.length < 7) {
            showError(`Expected 7 tab-separated columns (name, address, phone, frequency, email, last visit, comments) — got ${cols.length}. Make sure you copied whole spreadsheet cells, not plain text.`);
            return;
        }
        const [nameCol, , , , , dateCol, commentsCol] = cols;
        const words = nameSearchWords(nameCol);
        if (words.length === 0) { showError('Could not read a name from the first column.'); return; }
        const parsedDate = parseApproxDate(dateCol);

        // Each word searched separately and the results unioned (OR), not
        // search_households' own AND-every-token behavior — a name column
        // that includes a word not present verbatim on the household record
        // (a nickname, a middle name, a misspelling) would otherwise return
        // zero matches even when one of the other words is a clean hit.
        // Wider net, user picks the right one from the list either way.
        const byId = new Map();
        try {
            for (const word of words) {
                const result = await Api.searchHouseholds({ query: word, tag_names: [], page: 1, page_size: 20 });
                for (const h of result.households) byId.set(h.id, h);
            }
        } catch (e) { showError(`${e}`); return; }
        const households = [...byId.values()].sort((a, b) => formatDirectoryName(a).localeCompare(formatDirectoryName(b)));

        if (households.length === 0) {
            showError(`No household found matching "${nameCol.trim()}". Open the household directly and record the visit from there instead.`);
            return;
        }

        matchesEl.innerHTML = `<h3>Select the matching household</h3><div id="pvMatchList"></div>`;
        const list = matchesEl.querySelector('#pvMatchList');
        list.innerHTML = households.map(h => `
            <div class="hh-paste-match" data-match="${h.id}">
                <div>${escapeHtml(formatDirectoryName(h))}</div>
                <div>${escapeHtml(h.address_line1 || '')}${h.address_line2 ? ', ' + escapeHtml(h.address_line2) : ''}${h.city ? ', ' + escapeHtml(h.city) : ''}</div>
            </div>`).join('');
        list.querySelectorAll('[data-match]').forEach(row => {
            row.addEventListener('click', () => showVisitConfirm(Number(row.dataset.match), parsedDate, dateCol.trim(), commentsCol.trim()));
        });
    }

    function showVisitConfirm(householdId, parsedDate, rawDateText, comments) {
        const matchesEl = overlay.querySelector('#pvMatches');
        matchesEl.innerHTML = `
            <h3>Record Visit</h3>
            ${parsedDate ? '' : `<div class="restore-warning">Could not read a date from "${escapeHtml(rawDateText)}" — enter it manually.</div>`}
            <label>Date (YYYY-MM-DD) <input type="text" id="pvVisitDate" value="${escapeHtml(parsedDate || '')}" placeholder="YYYY-MM-DD"></label>
            <label>Comments <textarea id="pvVisitComments" rows="3">${escapeHtml(comments)}</textarea></label>
            <div class="modal-buttons">
                <button class="btn btn-primary" id="pvSaveVisit">Save Visit</button>
                <button class="btn" id="pvBack">Back</button>
            </div>`;
        matchesEl.querySelector('#pvBack').addEventListener('click', findHousehold);
        matchesEl.querySelector('#pvSaveVisit').addEventListener('click', async () => {
            const date = matchesEl.querySelector('#pvVisitDate').value.trim();
            if (!isValidIsoDate(date)) { showMessage('Enter a real date as YYYY-MM-DD (e.g. 2022-04-01).', CONSTANTS.MESSAGE_TYPES.ERROR); return; }
            try {
                await Api.recordVisit(householdId, date, matchesEl.querySelector('#pvVisitComments').value || null);
                showMessage('Visit recorded.', CONSTANTS.MESSAGE_TYPES.INFO);
                overlay.remove();
                await loadHouseholds();
            } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); }
        });
    }

    overlay.querySelector('#pvFind').addEventListener('click', findHousehold);
}
