registerView('map', {
    init() {
        document.getElementById('mapViewRoot').innerHTML = `
            <h1>Dashboard</h1>
            <div class="dash-stats" id="dashTagStats"></div>
            <div class="map-toolbar">
                <div id="mapTagDropdown" style="min-width:260px;"></div>
                <input type="number" id="mapVisitCount" min="1" value="10" style="width:70px;" title="Number of addresses to include">
                <button class="btn btn-primary" id="mapGenerateBtn" disabled>Generate visit list from selected seed</button>
                <button class="btn" id="mapResetSeedBtn">Reset Seed</button>
            </div>
            <div id="mapEl"></div>
        `;
        this.map = L.map('mapEl').setView([39.5, -98.35], 4);
        L.tileLayer('https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png', {
            attribution: '&copy; OpenStreetMap contributors', maxZoom: 19,
        }).addTo(this.map);
        this.markersLayer = L.layerGroup().addTo(this.map);
        this.markersByAddressKey = {};
        this.seedGroupKey = null;
        this.selectedTagId = '';

        this.tagDropdown = mountDropdown(document.getElementById('mapTagDropdown'), {
            items: [{ value: '', label: 'All households with coordinates' }],
            value: '',
            onSelect: (val) => { this.selectedTagId = val; loadMapData(); },
        });
        document.getElementById('mapGenerateBtn').addEventListener('click', generateVisitList);
        document.getElementById('mapResetSeedBtn').addEventListener('click', resetSeed);
    },
    async onShow() {
        await populateMapTagSelect();
        await loadTagStats();
        setTimeout(() => MapView.map.invalidateSize(), 50);
        await loadMapData();
    },
});
const MapView = ViewRegistry.map; // convenient alias for handlers below

// Dashboard's per-tag breakdown — replaces the old separate Dashboard
// view's "how many tags exist" stat, which wasn't useful; a count per
// tag tells you something.
async function loadTagStats() {
    const tags = await Api.listTags().catch(() => []);
    document.getElementById('dashTagStats').innerHTML = tags
        .map(t => `<div class="dash-card"><div class="dash-num">${t.household_count}</div><div>${escapeHtml(t.name)}</div></div>`)
        .join('');
}

async function populateMapTagSelect() {
    const tags = await Api.listTags().catch(() => []);
    MapView.tagDropdown?.setItems([
        { value: '', label: 'All households with coordinates' },
        ...tags.map(t => ({ value: String(t.id), label: t.name })),
    ]);
}

async function loadMapData() {
    const tagId = MapView.selectedTagId || null;
    let groups;
    try { groups = await Api.getMapData(tagId ? Number(tagId) : null); }
    catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); return; }

    MapView.markersLayer.clearLayers();
    MapView.markersByAddressKey = {};
    MapView.currentGroups = groups;
    const bounds = [];
    groups.forEach(g => {
        const marker = L.marker([g.latitude, g.longitude]).addTo(MapView.markersLayer);
        MapView.markersByAddressKey[g.address_key] = marker;
        marker.bindPopup(`<strong>${escapeHtml(g.address_line1 || '(no address on file)')}</strong><br>${g.names.map(escapeHtml).join('<br>')}
            <br><button class="btn" data-select-seed="${escapeHtml(g.address_key)}">Use as seed</button>`);
        marker.on('popupopen', () => {
            document.querySelector(`[data-select-seed="${CSS.escape(g.address_key)}"]`)?.addEventListener('click', () => {
                MapView.seedGroupKey = g.address_key;
                MapView.seedHouseholdId = g.household_ids[0];
                document.getElementById('mapGenerateBtn').disabled = false;
                showMessage(`Seed set: ${g.address_line1 || '(no address on file)'}`, CONSTANTS.MESSAGE_TYPES.INFO, 2500);
            });
        });
        bounds.push([g.latitude, g.longitude]);
    });
    if (bounds.length) MapView.map.fitBounds(bounds, { padding: [30, 30] });
}

// A star badge overlaid on a household's existing marker — shows which
// households a generated visit list actually included.
const starIcon = L.divIcon({
    html: '<div class="map-star-marker">★</div>',
    className: '', iconSize: [52, 52], iconAnchor: [26, 26],
});

// The generated visit list is always restricted to "Not known" households,
// regardless of whatever tag the dashboard's own filter dropdown currently
// has selected (that dropdown only controls what's plotted on the map).
async function notKnownTagId() {
    const tags = await Api.listTags().catch(() => []);
    const tag = tags.find(t => t.name === 'Not known');
    return tag ? tag.id : null;
}

async function generateVisitList() {
    if (!MapView.seedHouseholdId) return;
    const tagId = await notKnownTagId();
    const count = parseInt(document.getElementById('mapVisitCount').value, 10) || 10;
    let entries;
    try {
        entries = await Api.generateVisitList({
            seed_household_id: MapView.seedHouseholdId,
            tag_id: tagId,
            count,
        });
    } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); return; }

    // Previous run's stars need to go back to the default pin before this
    // run's results get their own stars — otherwise stars from a household
    // that isn't part of the new list stick around looking like it still is.
    Object.values(MapView.markersByAddressKey).forEach(marker => marker.setIcon(new L.Icon.Default()));
    entries.forEach(e => {
        const marker = MapView.markersByAddressKey[e.address_key];
        if (marker) marker.setIcon(starIcon);
    });

    MapView.lastVisitListText = buildVisitListText(entries);
    const overlay = modalShell(`
        <h2>Visit List (${entries.length} addresses)</h2>
        <button class="btn" id="mapCopyVisitListBtn">⎘ Copy</button>
        <ol class="visit-list-items">${entries.map(e => {
            const cityLine = [e.city, e.state].filter(Boolean).join(' ') + (e.zip ? ' ' + e.zip : '');
            const phones = e.phones.length ? ` — ${e.phones.map(escapeHtml).join(', ')}` : '';
            return `<li>${escapeHtml(e.address_line1 || '(no address on file)')}${cityLine.trim() ? ', ' + escapeHtml(cityLine.trim()) : ''}
                — ${e.names.map(escapeHtml).join(', ')}${phones}
                <span style="opacity:.6;"> (${Math.round(e.distance_meters)} m from seed)</span></li>`;
        }).join('')}</ol>
        <div class="modal-buttons">
            <button class="btn" id="mapCloseVisitListBtn">Close</button>
        </div>
    `);
    overlay.querySelector('#mapCopyVisitListBtn').addEventListener('click', () => copyVisitList(overlay));
    overlay.querySelector('#mapCloseVisitListBtn').addEventListener('click', () => overlay.remove());
}

function buildVisitListText(entries) {
    return entries.map(e => {
        const cityLine = [e.city, e.state].filter(Boolean).join(' ') + (e.zip ? ' ' + e.zip : '');
        const phones = e.phones.length ? ` — ${e.phones.join(', ')}` : '';
        return `${e.address_line1 || '(no address on file)'}${cityLine.trim() ? ', ' + cityLine.trim() : ''} — ${e.names.join(', ')}${phones}`;
    }).join('\n');
}

async function copyVisitList(overlay) {
    const btn = overlay.querySelector('#mapCopyVisitListBtn');
    try { await navigator.clipboard.writeText(MapView.lastVisitListText || ''); btn.textContent = 'Copied!'; }
    catch (e) { btn.textContent = 'Copy failed'; }
    setTimeout(() => { if (btn) btn.textContent = '⎘ Copy'; }, 1500);
}

function resetSeed() {
    MapView.seedGroupKey = null;
    MapView.seedHouseholdId = null;
    document.getElementById('mapGenerateBtn').disabled = true;
    Object.values(MapView.markersByAddressKey).forEach(marker => marker.setIcon(new L.Icon.Default()));
    showMessage('Seed cleared.', CONSTANTS.MESSAGE_TYPES.INFO, 2000);
}

