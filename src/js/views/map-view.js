registerView('map', {
    init() {
        document.getElementById('mapViewRoot').innerHTML = `
            <h1>Dashboard</h1>
            <div class="dash-stats" id="dashTagStats"></div>
            <div class="map-toolbar">
                <div id="mapTagDropdown" style="min-width:260px;"></div>
                <input type="number" id="mapVisitCount" min="1" value="10" style="width:70px;" title="Number of households to visit">
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
        wireMapResize();
    },
    async onShow() {
        await populateMapTagSelect();
        await loadTagStats();
        setTimeout(resizeMapEl, 50);
        await loadMapData();
    },
});
const MapView = ViewRegistry.map; // convenient alias for handlers below

// Sizes #mapEl from its own actual on-screen position, not a guessed
// vh-minus-padding constant (#15 follow-up — the old calc(100vh - 40px)
// never accounted for #messageArea's height above this view, so the map
// was always short by roughly that much and always carried a scrollbar,
// regardless of window size). 20 matches #mainContent's own bottom
// padding (base.css). Re-run on every onShow (dash-stats/toolbar content
// can change row count between visits) and on window resize.
function resizeMapEl() {
    const el = document.getElementById('mapEl');
    if (!el) return;
    const top = el.getBoundingClientRect().top;
    const mainContentBottomPadding = 20;
    const height = Math.max(300, window.innerHeight - top - mainContentBottomPadding);
    el.style.height = `${height}px`;
    if (MapView.map) MapView.map.invalidateSize();
}

let _mapResizeWired = false;
function wireMapResize() {
    if (_mapResizeWired) return;
    window.addEventListener('resize', resizeMapEl);
    _mapResizeWired = true;
}

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
    MapView.tagsById = {};
    tags.forEach(t => { MapView.tagsById[String(t.id)] = t.name; });
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

// A numbered badge overlaid on a household's existing marker — shows
// both which households a generated visit list included AND the order
// they fall in the route (#13 follow-up), so the route's shape is
// visible on the map itself, not just in the list below it.
function routeMarkerIcon(n) {
    return L.divIcon({
        html: `<div class="map-route-marker">${n}</div>`,
        className: '', iconSize: [28, 28], iconAnchor: [14, 14],
    });
}

// Plain haversine, meters — mirrors commands/geo.rs's haversine_meters()
// (and roads.rs's own separate copy) rather than importing it; this is
// the frontend's only distance calc, needed here to show the loop's
// closing leg (last stop back to the configured start point), which the
// backend doesn't compute or return.
function haversineMeters(lat1, lon1, lat2, lon2) {
    const R = 6371000;
    const toRad = (d) => (d * Math.PI) / 180;
    const dLat = toRad(lat2 - lat1);
    const dLon = toRad(lon2 - lon1);
    const a = Math.sin(dLat / 2) ** 2 + Math.cos(toRad(lat1)) * Math.cos(toRad(lat2)) * Math.sin(dLon / 2) ** 2;
    return R * 2 * Math.atan2(Math.sqrt(a), Math.sqrt(1 - a));
}

// Visit-list distances display in miles, not meters — the backend
// (distance_meters) and haversineMeters() above still compute/return
// meters throughout; this only converts at render time.
function metersToMiles(m) {
    return m / 1609.344;
}

// Configured route start point — label + coords straight from Settings
// (routeStartLabel/Lat/Lon). No geocoding: the label the user already
// typed in Settings is the address, used verbatim for both the "Starting
// at" line and the return leg below. Returns null when unconfigured.
async function getRouteStartInfo() {
    let settings;
    try { settings = await Api.getSettings(); } catch (e) { return null; }
    const lat = parseFloat(settings.routeStartLat);
    const lon = parseFloat(settings.routeStartLon);
    if (Number.isNaN(lat) || Number.isNaN(lon)) return null;
    return { label: settings.routeStartLabel || 'start point', lat, lon };
}

// Closing leg — last stop back to the configured route start point.
// Only meaningful when the route-start setting is actually in play
// (distance_context === 'route' on the last entry); an unconfigured
// setup has no start point to loop back to. Not a real loop
// optimization (that reorders the whole walk to account for the return
// trip) — just surfaces the cost of the naive close so a lopsided route
// is visible. Real loop optimization is a follow-on, not part of this.
function computeReturnLeg(entries, startInfo) {
    if (!startInfo || !entries.length || entries[entries.length - 1].distance_context !== 'route') return null;
    const last = entries[entries.length - 1];
    return {
        label: startInfo.label,
        meters: haversineMeters(last.latitude, last.longitude, startInfo.lat, startInfo.lon),
    };
}

// The generated visit list now pulls from whatever tag the dashboard's
// own filter dropdown has selected — the same pool that's plotted on
// the map — rather than always hardcoding "Not known" (#15 follow-up).
// '' means "All households with coordinates": no tag restriction.
function currentTagLabel() {
    if (!MapView.selectedTagId) return 'All households with coordinates';
    return (MapView.tagsById && MapView.tagsById[MapView.selectedTagId]) || 'Tag';
}

// Filesystem-safe form of the current tag label, for PDF filenames.
// "All households with coordinates" collapses to the shorter
// "All_Households" per spec; any other tag just gets its spaces
// replaced (tags can contain spaces — Functional_Requirements.md).
function tagLabelForFilename(label) {
    if (label === 'All households with coordinates') return 'All_Households';
    return label.replace(/\s+/g, '_');
}

async function generateVisitList() {
    if (!MapView.seedHouseholdId) return;
    const tagId = MapView.selectedTagId ? Number(MapView.selectedTagId) : null;
    const count = parseInt(document.getElementById('mapVisitCount').value, 10) || 10;
    let entries;
    try {
        entries = await Api.generateVisitList({
            seed_household_id: MapView.seedHouseholdId,
            tag_id: tagId,
            count,
        });
    } catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); return; }

    // Previous run's badges need to go back to the default pin before this
    // run's results get their own — otherwise a badge from a household
    // that isn't part of the new list sticks around looking like it still
    // is.
    Object.values(MapView.markersByAddressKey).forEach(marker => marker.setIcon(new L.Icon.Default()));
    entries.forEach((e, idx) => {
        const marker = MapView.markersByAddressKey[e.address_key];
        if (marker) marker.setIcon(routeMarkerIcon(idx + 1));
    });

    const startInfo = await getRouteStartInfo();
    const startsAtRoute = entries.length > 0 && entries[0].distance_context === 'route' && !!startInfo;
    const returnLeg = computeReturnLeg(entries, startInfo);
    const tagLabel = currentTagLabel();

    MapView.lastVisitListText = buildVisitListText(entries, returnLeg, startsAtRoute ? startInfo : null);
    MapView.lastVisitEntries = entries;
    MapView.lastVisitReturnLeg = returnLeg;
    MapView.lastVisitTagLabel = tagLabel;
    MapView.lastVisitStartInfo = startsAtRoute ? startInfo : null;

    const overlay = modalShell(`
        <h2>Visit List (${entries.length} addresses)</h2>
        <button class="btn" id="mapCopyVisitListBtn">⎘ Copy</button>
        <button class="btn" id="mapPdfVisitListBtn">⎙ PDF</button>
        ${startsAtRoute ? `<div class="visit-list-start">Starting at ${escapeHtml(startInfo.label)}</div>` : ''}
        <ol class="visit-list-items">${entries.map((e, idx) => {
            const cityLine = [e.city, e.state].filter(Boolean).join(' ') + (e.zip ? ' ' + e.zip : '');
            const phones = e.phones.length ? ` — ${e.phones.map(escapeHtml).join(', ')}` : '';
            const distLabel = e.distance_context === 'route'
                ? (idx === 0 ? 'from start point' : 'from previous stop')
                : 'from seed';
            return `<li>${escapeHtml(e.address_line1 || '(no address on file)')}${cityLine.trim() ? ', ' + escapeHtml(cityLine.trim()) : ''}
                — ${e.names.map(escapeHtml).join(', ')}${phones}
                <span style="opacity:.6;"> (${metersToMiles(e.distance_meters).toFixed(2)} mi ${distLabel})</span></li>`;
        }).join('')}${returnLeg ? `<li style="list-style:none; margin-top:8px; opacity:.75;">↩ Back to ${escapeHtml(returnLeg.label)}
                <span style="opacity:.6;"> (${metersToMiles(returnLeg.meters).toFixed(2)} mi)</span></li>` : ''}</ol>
        <div class="modal-buttons">
            <button class="btn" id="mapCloseVisitListBtn">Close</button>
        </div>
    `);
    overlay.querySelector('#mapCopyVisitListBtn').addEventListener('click', () => copyVisitList(overlay));
    overlay.querySelector('#mapPdfVisitListBtn').addEventListener('click', () => downloadVisitListPdf());
    overlay.querySelector('#mapCloseVisitListBtn').addEventListener('click', () => overlay.remove());
}

function buildVisitListText(entries, returnLeg, startInfo) {
    const lines = [];
    if (startInfo) lines.push(`Starting at ${startInfo.label}`);
    entries.forEach(e => {
        const cityLine = [e.city, e.state].filter(Boolean).join(' ') + (e.zip ? ' ' + e.zip : '');
        const phones = e.phones.length ? ` — ${e.phones.join(', ')}` : '';
        lines.push(`${e.address_line1 || '(no address on file)'}${cityLine.trim() ? ', ' + cityLine.trim() : ''} — ${e.names.join(', ')}${phones}`);
    });
    if (returnLeg) lines.push(`↩ Back to ${returnLeg.label} (${metersToMiles(returnLeg.meters).toFixed(2)} mi)`);
    return lines.join('\n');
}

// PDF export of the currently-open Visit List modal (#15 follow-up).
// Single-column list, not the two-column directory-pdf.js layout — kept
// inline here rather than a separate file given its size; flag if a
// dedicated visit-route-pdf.js is preferred for consistency later.
function downloadVisitListPdf() {
    const entries = MapView.lastVisitEntries || [];
    if (entries.length === 0) return;
    const returnLeg = MapView.lastVisitReturnLeg;
    const tagLabel = MapView.lastVisitTagLabel || 'All households with coordinates';
    const startInfo = MapView.lastVisitStartInfo;

    const faint = '#777777';
    const body = entries.map((e, idx) => {
        const cityLine = [e.city, e.state].filter(Boolean).join(' ') + (e.zip ? ' ' + e.zip : '');
        const phones = e.phones.length ? ` — ${e.phones.join(', ')}` : '';
        const distLabel = e.distance_context === 'route'
            ? (idx === 0 ? 'from start point' : 'from previous stop')
            : 'from seed';
        return {
            margin: [0, 0, 0, 6],
            text: [
                { text: `${idx + 1}. `, bold: true },
                `${e.address_line1 || '(no address on file)'}${cityLine.trim() ? ', ' + cityLine.trim() : ''} — ${e.names.join(', ')}${phones} `,
                { text: `(${metersToMiles(e.distance_meters).toFixed(2)} mi ${distLabel})`, color: faint, fontSize: 8 },
            ],
        };
    });
    if (returnLeg) {
        body.push({
            italics: true,
            margin: [0, 8, 0, 0],
            text: [
                `↩ Back to ${returnLeg.label} `,
                { text: `(${metersToMiles(returnLeg.meters).toFixed(2)} mi)`, color: faint, fontSize: 8 },
            ],
        });
    }

    const docDefinition = {
        pageSize: 'LETTER',
        pageMargins: [54, 54, 54, 40],
        defaultStyle: { font: 'Roboto', fontSize: 10 },
        content: [
            { text: `Visit Route — ${tagLabel}`, fontSize: 14, bold: true, color: '#2c3e50', margin: [0, 0, 0, 12] },
            ...(startInfo ? [{ text: `Starting at ${startInfo.label}`, italics: true, margin: [0, 4, 0, 8] }] : []),
            ...body,
        ],
        footer: (currentPage, pageCount) => ({
            text: `Page ${currentPage} of ${pageCount}`, alignment: 'center', fontSize: 8, color: faint, margin: [0, 10, 0, 0],
        }),
    };

    const filename = `LostSheep-Visits-${tagLabelForFilename(tagLabel)}.pdf`;
    pdfMake.createPdf(docDefinition).download(filename);
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

