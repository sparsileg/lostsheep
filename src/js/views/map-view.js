// #72: tiles are cached to disk (via Api.getCachedTile/saveCachedTile,
// backed by commands/tiles.rs) so a tile already viewed once is never
// re-requested from tile.openstreetmap.org. The fetch itself still
// happens here in the webview, which already has CSP permission
// (connect-src) to reach the tile domain — the backend only does local
// disk I/O. getTileUrl() is inherited unchanged from L.TileLayer, so the
// existing {s}/{z}/{x}/{y} subdomain-rotation template still applies to
// the URL used on a cache miss.
const CachedTileLayer = L.TileLayer.extend({
    initialize(url, options) {
        L.TileLayer.prototype.initialize.call(this, url, options);
        // Object URLs the browser will otherwise keep alive forever —
        // revoked as soon as Leaflet unloads the tile that used them.
        this.on('tileunload', (e) => {
            if (e.tile && e.tile.src && e.tile.src.startsWith('blob:')) {
                URL.revokeObjectURL(e.tile.src);
            }
        });
    },
    createTile(coords, done) {
        const tile = document.createElement('img');
        const { z, x, y } = coords;
        (async () => {
            try {
                const cached = await Api.getCachedTile(z, x, y);
                if (cached) {
                    tile.src = tileBytesToBlobUrl(cached);
                    done(null, tile);
                    return;
                }
                const url = this.getTileUrl(coords);
                const resp = await fetch(url);
                if (!resp.ok) throw new Error(`tile fetch failed: ${resp.status}`);
                const bytes = Array.from(new Uint8Array(await resp.arrayBuffer()));
                tile.src = tileBytesToBlobUrl(bytes);
                done(null, tile);
                // Fire-and-forget — a failed write just means this tile
                // isn't cached yet and gets fetched again next time; it
                // doesn't block the tile from displaying now.
                Api.saveCachedTile(z, x, y, bytes).catch(e => console.error('saveCachedTile failed', e));
            } catch (e) {
                done(e, tile);
            }
        })();
        return tile;
    },
});

function tileBytesToBlobUrl(bytes) {
    return URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: 'image/png' }));
}

registerView('map', {
    init() {
        document.getElementById('mapViewRoot').innerHTML = `
            <h1>Dashboard</h1>
            <div class="dash-stats" id="dashTagStats"></div>
            <div class="map-toolbar">
                <div id="mapTagDropdown" style="min-width:260px;"></div>
                <label for="mapVisitCount">Stops: </label>
                <input type="number" id="mapVisitCount" min="1" value="10" style="width:70px;" title="Number of households to visit">
                <button class="btn" id="mapResetSeedBtn">Reset</button>
                <div class="map-search-wrap">
                    <input type="text" id="mapIconSearchInput" placeholder="Highlight households…" />
                    <button type="button" class="map-search-clear" id="mapIconSearchClearBtn" aria-label="Clear search" title="Clear search">&times;</button>
                </div>
            </div>
            <div class="map-container">
                <div id="mapEl"></div>
                <div id="mapPostRouteControls" class="map-postroute-controls" hidden>
                    <button type="button" class="btn btn-small" id="mapCopyVisitListBtn">⎘ Copy Text</button>
                    <button type="button" class="btn btn-small" id="mapPdfVisitListBtn">⎙ PDF</button>
                    <button type="button" class="btn btn-small" id="mapPreviewToggleBtn">Preview route</button>
                </div>
                <div id="mapPreviewPanel" class="map-preview-panel" hidden></div>
            </div>
        `;
        // Issue #43 follow-up — zoomSnap 0.25 lets fitBounds() land on
        // quarter-levels instead of only whole ones, so
        // frameRouteBounds() doesn't have to back off a full level to
        // fit a route. zoomDelta is a separate Leaflet setting governing
        // the +/- control buttons specifically — without it those
        // buttons still step by a whole level regardless of zoomSnap.
        // Scroll-wheel zoom isn't affected by either; it already snaps
        // to the nearest zoomSnap value on its own. Revisit 0.25 for
        // either if it feels off.
        this.map = L.map('mapEl', { zoomSnap: 0.25, zoomDelta: 0.25 }).setView([39.5, -98.35], 4);
        new CachedTileLayer('https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png', {
            attribution: '&copy; OpenStreetMap contributors', maxZoom: 19,
        }).addTo(this.map);
        this.markersLayer = L.layerGroup().addTo(this.map);
        this.markersByAddressKey = {};
        // Issue #41 — each marker's "real" icon (default pin, or a
        // route-order badge from generateVisitList()) tracked separately
        // from whatever the icon-search overlay is showing right now, so
        // clearing a search restores whichever of those actually applied
        // — not unconditionally the default pin. Keyed by address_key,
        // same as markersByAddressKey. See applyMarkerBaseIcon() /
        // applyIconSearch() below.
        this.markerBaseState = {};
        this.seedGroupKey = null;
        this.selectedTagId = '';

        // Issue #40 — Road Management's roads overlay. Starts detached;
        // applyRoadSettings() (called from onShow() below, and from the
        // Road Management modal on toggle) adds/removes it based on the
        // persisted showRoadsOverlay setting. Redraws on pan/zoom since a
        // bounds-bound query only covers the viewport at the time it ran.
        //
        // The route overlay (also #40 originally) is no longer a toggle
        // (#84) — routeLayer is added to the map unconditionally here and
        // stays on for the life of this view; drawRouteOverlay() just
        // draws whatever route is current, or nothing if none has been
        // generated yet.
        this.roadsLayer = L.layerGroup();
        this.routeLayer = L.layerGroup().addTo(this.map);
        this.roadsOverlayEnabled = false;
        this.map.on('moveend zoomend', () => { if (this.roadsOverlayEnabled) loadRoadsOverlay(); });

        // Right-click: copy the clicked point's coordinates. Leaflet
        // doesn't preventDefault the browser's own context menu on its
        // own — done explicitly here so only ours shows. Shift+right-
        // click is the escape hatch back to the native menu (Inspect
        // Element, Reload, etc.) — needed since this fires for every
        // right-click anywhere on the map, which is most of the screen.
        this.map.on('contextmenu', (e) => {
            if (e.originalEvent.shiftKey) return;
            L.DomEvent.preventDefault(e.originalEvent);
            showMapCoordMenu(e);
        });

        this.tagDropdown = mountDropdown(document.getElementById('mapTagDropdown'), {
            items: [{ value: '', label: 'All households with coordinates' }],
            value: '',
            onSelect: (val) => { this.selectedTagId = val; clearActiveRoute(); loadMapData(); },
        });
        document.getElementById('mapResetSeedBtn').addEventListener('click', resetSeed);

        // Issue #43 — post-route controls. Copy/PDF read from
        // MapView.lastVisit* state regardless of what's on screen (set by
        // generateVisitList()); the toggle just shows/hides the preview
        // panel using that same state. Wired once here, at init — the
        // buttons/panel elements are never rebuilt afterward, only shown
        // or hidden via the hidden attribute, so a single binding covers
        // every generated route for the life of this view.
        document.getElementById('mapCopyVisitListBtn').addEventListener('click', copyVisitList);
        document.getElementById('mapPdfVisitListBtn').addEventListener('click', downloadVisitListPdf);
        document.getElementById('mapPreviewToggleBtn').addEventListener('click', togglePreviewPanel);

        // Dismissed by any outside click — same pattern as sidebar.js's
        // hamburger menu / theme dropdown. Clicks on the toggle button
        // itself are excluded so toggling closed doesn't immediately
        // reopen via this same handler.
        document.addEventListener('click', (event) => {
            const panel = document.getElementById('mapPreviewPanel');
            const toggleBtn = document.getElementById('mapPreviewToggleBtn');
            if (!panel || panel.hidden) return;
            if (panel.contains(event.target) || event.target === toggleBtn) return;
            panel.hidden = true;
        });

        // Debounced live search — same fields as the households view
        // search (name/address/phone/email/comments), reusing
        // Api.searchHouseholds rather than a second matching
        // implementation. Shorter delay than households-view.js's 300ms
        // since this is just highlighting existing markers, not
        // re-querying a whole table — adjust if it still feels laggy.
        const mapIconSearchInput = document.getElementById('mapIconSearchInput');
        const mapIconSearchClearBtn = document.getElementById('mapIconSearchClearBtn');
        // Shared search string (SharedSearch — see the guard near the
        // bottom of this file) — Stan's ask: a search typed in Households
        // shows up here too, and vice versa. Seed from whatever's already
        // there rather than always starting blank.
        mapIconSearchInput.value = window.SharedSearch.query;
        syncMapSearchClearBtnVisibility();
        mapIconSearchInput.addEventListener('input', () => {
            window.SharedSearch.query = mapIconSearchInput.value;
            syncMapSearchClearBtnVisibility();
        });
        mapIconSearchInput.addEventListener('input', debounce((e) => {
            applyIconSearch(e.target.value);
        }, 200));
        mapIconSearchClearBtn.addEventListener('click', () => {
            mapIconSearchInput.value = '';
            window.SharedSearch.query = '';
            syncMapSearchClearBtnVisibility();
            mapIconSearchInput.focus();
            applyIconSearch('');
        });
        wireMapResize();
    },
    async onShow() {
        await populateMapTagSelect();
        await loadTagStats();
        setTimeout(resizeMapEl, 50);
        // Re-sync from SharedSearch every time this view is shown — the
        // Households view's own search box may have changed it since
        // init() ran. loadMapData() below calls reapplySearchOverlay(),
        // which reads this same input's current value, so setting it
        // here is enough to have the highlight follow along.
        const mapIconSearchInput = document.getElementById('mapIconSearchInput');
        if (mapIconSearchInput) mapIconSearchInput.value = window.SharedSearch.query;
        syncMapSearchClearBtnVisibility();
        await loadMapData();
        // loadMapData() just rebuilt every marker fresh (badges reset to
        // default) — a leftover route from before this view was left
        // would otherwise still get redrawn by applyRoadSettings() below
        // from stale lastVisitEntries, leaving default-icon markers next
        // to a route that no longer corresponds to anything selected.
        clearActiveRoute();
        await applyRoadSettings();
    },
});
const MapView = ViewRegistry.map; // convenient alias for handlers below
MapView.applyRoadSettings = applyRoadSettings;

// Cross-view shared search string (see households-view.js's own copy of
// this guard for why it's duplicated rather than defined once — whichever
// file loads first creates it, the other's identical guard just finds it
// already there).
window.SharedSearch = window.SharedSearch || { query: '' };

// Pulled out of init()'s closure so onShow() can also call it without
// duplicating the DOM lookups.
function syncMapSearchClearBtnVisibility() {
    const input = document.getElementById('mapIconSearchInput');
    const btn = document.getElementById('mapIconSearchClearBtn');
    if (input && btn) btn.classList.toggle('map-search-clear-visible', input.value.length > 0);
}

// Sizes #mapEl from its own actual on-screen position, not a guessed
// vh-minus-padding constant (#15 follow-up — the old calc(100vh - 40px)
// never accounted for #messageArea's height above this view, so the map
// was always short by roughly that much and always carried a scrollbar,
// regardless of window size). 20 matches #mainContent's own bottom
// padding (base.css). Re-run on every onShow (dash-stats/toolbar content
// can change row count between visits) and on window resize.
// Issue #43 follow-up — clears whatever route is currently "on display"
// (the drawn overlay polyline, the floating post-route controls, and the
// preview panel), and forgets it (lastVisitEntries = null) so nothing
// downstream (applyRoadSettings' redraw, a stale Copy/PDF/preview) can
// act on it after the underlying data it referred to has changed.
// Shared by three places a currently-shown route stops being valid:
// leaving/re-entering this view (onShow), an explicit Reset
// (resetSeed), and switching the tag filter (which rebuilds every
// marker via loadMapData() but wasn't clearing this UI, leaving stale
// controls up referencing a route that no longer matches what's on
// screen).
function clearActiveRoute() {
    MapView.lastVisitEntries = null;
    MapView.routeLayer.clearLayers();
    document.getElementById('mapPostRouteControls').hidden = true;
    document.getElementById('mapPreviewPanel').hidden = true;
}

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

// Reads a CSS custom property's current computed value — used so the
// road/route overlay lines follow whatever theme is active rather than
// a hardcoded color, same spirit as base.css's --chart1/--chart2 (#82).
function themeColor(varName, fallback) {
    const val = getComputedStyle(document.documentElement).getPropertyValue(varName).trim();
    return val || fallback;
}

// Issue #40, updated for #84 — reads the persisted showRoadsOverlay
// setting and adds/removes the roads layer accordingly. Called from
// onShow() so a fresh visit to the map view picks up whatever was
// toggled in the Road Management modal, and also called directly by that
// modal's toggle handler so a redraw happens immediately without waiting
// for the next onShow().
//
// The route layer is no longer gated by a setting (#84) — it's added to
// the map once at init() and stays there; this just redraws whatever
// route is already generated, if any, so returning to the map view shows
// it without regenerating the list.
async function applyRoadSettings() {
    let settings;
    try { settings = await Api.getSettings(); } catch (e) { console.error(e); return; }

    MapView.roadsOverlayEnabled = settings.showRoadsOverlay === 'true';

    if (MapView.roadsOverlayEnabled) {
        MapView.roadsLayer.addTo(MapView.map);
        await loadRoadsOverlay();
    } else {
        MapView.roadsLayer.clearLayers();
        MapView.map.removeLayer(MapView.roadsLayer);
    }

    if (MapView.lastVisitEntries) await drawRouteOverlay(MapView.lastVisitEntries);
}

// Viewport-bounded road overlay (issue #40). Queries only the current
// map bounds — the road graph can be far larger than is reasonable to
// load/render all at once — and redraws on every pan/zoom (wired in
// init() above). MAX_ROAD_EDGES_PER_QUERY on the backend is a cap this
// call can hit; when it does, truncated comes back true and edges is
// empty rather than a silently-partial layer.
async function loadRoadsOverlay() {
    if (!MapView.roadsOverlayEnabled || !MapView.map) return;
    const bounds = MapView.map.getBounds();
    let result;
    try {
        result = await Api.getRoadsInBounds(
            bounds.getSouth(), bounds.getNorth(), bounds.getWest(), bounds.getEast()
        );
    } catch (e) { console.error('getRoadsInBounds failed', e); return; }

    MapView.roadsLayer.clearLayers();
    if (result.truncated) {
        showMessage('Too many roads to show at this zoom level — zoom in to see roads.', CONSTANTS.MESSAGE_TYPES.INFO, 3000);
        return;
    }
    const color = themeColor('--chart1', '#0d6efd');
    result.edges.forEach(seg => {
        drawOutlinedRoadSegment([[seg.lat1, seg.lon1], [seg.lat2, seg.lon2]], MapView.roadsLayer, color);
    });
}

// Ad hoc request — roads were hard to see against busy OSM tile imagery
// at a thin 2px weight, so each segment now gets a 2px black border on
// either side. Same two-polyline layering drawRoadStyledPolyline() below
// uses for the route line: a wider solid black line drawn first, the
// original theme-colored line on top at its original weight — 2px black
// + 2px color + 2px black = 6px total. Opacity/weight of the colored
// line itself are unchanged from before this change.
function drawOutlinedRoadSegment(latlngs, layerGroup, color) {
    L.polyline(latlngs, { color: '#000000', weight: 6, opacity: 0.9 }).addTo(layerGroup);
    L.polyline(latlngs, { color, weight: 2, opacity: 0.6 }).addTo(layerGroup);
}

// Route overlay (issue #40, updated for #38's road-path geometry). Each
// entry now carries its own leg's route_path — [household] → [snap
// node] → [A* nodes...] → [snap node] → [household] when the backend
// found real road distance, or just [household, household] on a
// straight-line fallback (no graph / snap miss / no path found). Drawing
// each leg's own path (rather than one polyline through every household
// point) is what makes the line actually follow roads instead of
// cutting straight between stops — and it already includes the
// household-to-road offset, so the separate per-household snap-line
// lookup (Api.getNearestRoadNode) this used to make is redundant now and
// has been dropped.
//
// Route line mimics a real road's look — a solid gray "asphalt" base
// with a dashed gold centerline on top, rather than an app-themed color.
// Deliberately NOT theme-dependent: the goal is a route that reads as
// "a road" and stays visible against real OSM tile imagery regardless of
// the app's theme. (A tick-mark/crosshatch texture like a paper map's
// casing style would need a Leaflet plugin this project doesn't vendor —
// skipped per Stan; this dashed-centerline version uses only stock
// Leaflet polylines.)
function drawRoadStyledPolyline(latlngs, layerGroup) {
    L.polyline(latlngs, { color: '#808080', weight: 7, opacity: 1 }).addTo(layerGroup);
    L.polyline(latlngs, { color: '#FFD700', weight: 3, opacity: 1, dashArray: '10, 10' }).addTo(layerGroup);
}

async function drawRouteOverlay(entries) {
    MapView.routeLayer.clearLayers();
    if (!entries || entries.length === 0) return;

    const anyPaths = entries.some(e => e.route_path && e.route_path.length > 1);

    if (anyPaths) {
        entries.forEach(e => {
            if (!e.route_path || e.route_path.length < 2) return;
            const latlngs = e.route_path.map(p => [p.lat, p.lon]);
            drawRoadStyledPolyline(latlngs, MapView.routeLayer);
        });
    } else {
        // No per-leg path data at all (e.g. a seed-only list with no
        // route_start configured) — fall back to the old straight-line-
        // between-households polyline so the toggle still shows something.
        const latlngs = entries.map(e => [e.latitude, e.longitude]);
        drawRoadStyledPolyline(latlngs, MapView.routeLayer);
    }
}

// Dashboard's per-tag breakdown — replaces the old separate Dashboard
// view's "how many tags exist" stat, which wasn't useful; a count per
// tag tells you something.
async function loadTagStats() {
    const tags = await Api.listTags().catch(() => []);
    const tagCards = tags
        .map(t => `<div class="dash-card"><div class="dash-num">${t.household_count}</div><div>${escapeHtml(t.name)}</div></div>`)
        .join('');

    // #64 — households with no coordinates are silently dropped from the
    // map and every visit list; this card surfaces the count so the gap
    // is visible without running Data Validation. Not a tag — it cuts
    // across the other cards (a Known household can still lack
    // coordinates) — so it's kept visually distinct (pushed to the right,
    // red border via .dash-card-alert) rather than presented as a fourth
    // bucket in that same tag partition. Failure is silent (card just
    // doesn't render) rather than surfaced as an error — this is a
    // secondary stat, not worth a message bar entry if it can't load.
    const missingCoords = await Api.getMissingCoordsCount().catch(() => null);
    const noCoordsCard = missingCoords === null ? '' :
        `<div class="dash-card dash-card-alert"><div class="dash-num">${missingCoords}</div><div>No Coords</div></div>`;

    document.getElementById('dashTagStats').innerHTML = tagCards + noCoordsCard;
}

async function populateMapTagSelect() {
    const tags = await Api.listTags().catch(() => []);
    MapView.tagsById = {};
    tags.forEach(t => { MapView.tagsById[String(t.id)] = t.name; });
    // System tags (system_key set) are excluded from generate_visit_list/
    // get_map_data on the backend regardless of what's selected here — so
    // offering one in this dropdown only ever produced a silently empty
    // map (#23). Filtering here keeps the picker honest about what it can
    // actually show, and works for any future system tag automatically.
    const pickable = tags.filter(t => !t.system_key);
    MapView.tagDropdown?.setItems([
        { value: '', label: 'All households with coordinates' },
        ...pickable.map(t => ({ value: String(t.id), label: t.name })),
    ]);
}

async function loadMapData() {
    const tagId = MapView.selectedTagId || null;
    let groups;
    try { groups = await Api.getMapData(tagId ? Number(tagId) : null); }
    catch (e) { showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR); return; }

    MapView.markersLayer.clearLayers();
    MapView.markersByAddressKey = {};
    MapView.markerBaseState = {};
    MapView.currentGroups = groups;
    const bounds = [];
    groups.forEach(g => {
        const marker = L.marker([g.latitude, g.longitude]).addTo(MapView.markersLayer);
        MapView.markersByAddressKey[g.address_key] = marker;
        MapView.markerBaseState[g.address_key] = { type: 'default' };
        marker.bindPopup(`<strong>${escapeHtml(formatEntryAddress(g))}</strong><br>${g.names.map(escapeHtml).join('<br>')}
            <br><button class="btn" data-select-seed="${escapeHtml(g.address_key)}">Visit around here</button>`);
        marker.on('popupopen', () => {
            document.querySelector(`[data-select-seed="${CSS.escape(g.address_key)}"]`)?.addEventListener('click', () => {
                MapView.seedGroupKey = g.address_key;
                MapView.seedHouseholdId = g.household_ids[0];
                marker.closePopup();
                generateVisitList();
            });
        });
        bounds.push([g.latitude, g.longitude]);
    });
    if (bounds.length) MapView.map.fitBounds(bounds, { padding: [30, 30] });
    reapplySearchOverlay();
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

// Issue #41 — icon-search match marker. Literal gold, not a theme
// variable — same reasoning drawRoadStyledPolyline() uses for the route
// line: this needs to read the same "found it" signal regardless of
// which theme is active, rather than blending into whatever the theme's
// primary color happens to be.
function searchMatchMarkerIcon() {
    return L.divIcon({
        html: `<div class="map-search-star-marker">★</div>`,
        className: '', iconSize: [56, 56], iconAnchor: [28, 28],
    });
}

// Applies whichever icon actually applies to this marker right now —
// the default pin, or a route-order badge if generateVisitList() set
// one — independent of any icon-search overlay currently on top of it.
// Called both to lay down the real state (loadMapData/generateVisitList/
// resetSeed) and to restore it once a search is cleared.
function applyMarkerBaseIcon(addressKey) {
    const marker = MapView.markersByAddressKey[addressKey];
    if (!marker) return;
    const st = MapView.markerBaseState[addressKey];
    marker.setIcon(st && st.type === 'route' ? routeMarkerIcon(st.order) : new L.Icon.Default());
    marker.setZIndexOffset(0);
}

// Fetches every household id matching the current icon-search text,
// scoped to whatever tag the dashboard's own filter has selected (the
// same pool plotted on the map) — same paging pattern
// fetchAllFilteredHouseholds() in households-view.js uses to walk past
// the server's 500-per-page cap. Reuses Api.searchHouseholds itself
// (same fields: name/address/phone/email/comments) rather than a
// second, divergent matching implementation.
async function fetchMatchingHouseholdIds(query) {
    const tag_names = MapView.selectedTagId ? [MapView.tagsById[MapView.selectedTagId]] : [];
    const page_size = 500;
    let page = 1;
    const ids = new Set();
    for (;;) {
        let result;
        try { result = await Api.searchHouseholds({ query, tag_names, page, page_size }); }
        catch (e) { console.error('map icon search failed', e); break; }
        result.households.forEach(h => ids.add(h.id));
        if (ids.size >= result.total || result.households.length === 0) break;
        page += 1;
    }
    return ids;
}

// Issue #41 — live icon-search on the dashboard. An empty query restores
// every marker to its real base icon/state (see applyMarkerBaseIcon).
// A non-empty query turns matches into gold stars at full opacity and
// fades everything else — the faded markers keep their real underlying
// icon (default pin or route badge), just dimmed, so clearing the search
// doesn't need to guess what was there before.
async function applyIconSearch(query) {
    const trimmed = (query || '').trim();
    if (!trimmed) {
        Object.keys(MapView.markersByAddressKey).forEach(key => {
            MapView.markersByAddressKey[key].setOpacity(1);
            applyMarkerBaseIcon(key);
        });
        return;
    }
    const matchedIds = await fetchMatchingHouseholdIds(trimmed);
    (MapView.currentGroups || []).forEach(g => {
        const marker = MapView.markersByAddressKey[g.address_key];
        if (!marker) return;
        if (g.household_ids.some(id => matchedIds.has(id))) {
            marker.setIcon(searchMatchMarkerIcon());
            marker.setOpacity(1);
            // Leaflet otherwise stacks markers by latitude (further
            // south wins ties), which can bury a star match under a
            // nearby non-matching pin — force matches to the front
            // regardless of position.
            marker.setZIndexOffset(1000);
        } else {
            applyMarkerBaseIcon(g.address_key);
            marker.setOpacity(0.35);
        }
    });
}

// Re-runs whatever icon-search is currently typed, if any — called after
// anything that rebuilds base icon state (a fresh generateVisitList()
// run, resetSeed(), a reloaded loadMapData()) so a search the user is
// mid-typing doesn't silently go stale against the new marker state.
function reapplySearchOverlay() {
    const el = document.getElementById('mapIconSearchInput');
    if (el && el.value.trim()) applyIconSearch(el.value);
}

// Clears the map icon-search field itself (not just the highlighting) —
// used when something invalidates what the search was highlighting
// against, e.g. a freshly generated visit route.
function clearIconSearch() {
    const input = document.getElementById('mapIconSearchInput');
    const clearBtn = document.getElementById('mapIconSearchClearBtn');
    if (input) input.value = '';
    if (clearBtn) clearBtn.classList.remove('map-search-clear-visible');
    applyIconSearch('');
}

// Plain haversine, meters. #65: the two Rust copies (commands/geo.rs and
// commands/roads.rs) converged onto one function (geo::haversine_meters);
// this JS copy is the intentional remaining exception, not sharing that
// convergence, since it computes the loop's closing leg (last stop back
// to the configured start point), which the backend doesn't compute or
// return — pulling this into an IPC round-trip per calculation isn't
// worth it for an interactive map. It already uses atan2(sqrt(a),
// sqrt(1-a)) rather than asin(sqrt(a)), which doesn't have the
// near-antipodal NaN failure mode #24/#65 fixed on the Rust side — no
// clamp needed here.
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

// Joins address_line1 + address_line2 (unit/lot number, etc.) the same
// way households-view.js's household detail modal already does — that
// second line existed on the backend all along but every address string
// built here (map popups, the visit list, its text/PDF export) only ever
// read address_line1, silently dropping it.
function formatEntryAddress(e) {
    return [e.address_line1, e.address_line2].filter(Boolean).join(', ') || '(no address on file)';
}

// Issue #89 — fetches visit history for every household on a generated
// route and attaches it to each entry, so the Copy Text and PDF exports
// can show visit dates/comments per stop. A stop can carry more than one
// household_id (a shared address) — visits from all of them are merged
// into one list per entry, newest first, each tagged with the household's
// name only when the entry has more than one household (visitsMultiHousehold),
// so a single-household stop's lines don't carry a redundant name prefix.
// Uses Api.getHouseholdVisits, the same command the household detail modal
// already calls (commands/visits.rs::get_household_visits) — no backend
// change needed. Household ids are deduped across the whole route before
// fetching, so a shared address's households are each only fetched once
// even though generate_visit_list groups by address per entry.
async function attachVisitHistory(entries) {
    const uniqueIds = [...new Set(entries.flatMap(e => e.household_ids))];
    const visitsById = {};
    await Promise.all(uniqueIds.map(async (id) => {
        try { visitsById[id] = await Api.getHouseholdVisits(id); }
        catch (e) { console.error('attachVisitHistory: getHouseholdVisits failed', e); visitsById[id] = []; }
    }));
    entries.forEach(e => {
        const combined = [];
        e.household_ids.forEach((id, i) => {
            (visitsById[id] || []).forEach(v => combined.push({ date: v.visit_date, comments: v.comments, name: e.names[i] || '' }));
        });
        combined.sort((a, b) => b.date.localeCompare(a.date));
        e.visits = combined;
        e.visitsMultiHousehold = e.household_ids.length > 1;
    });
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

// Issue #43 — auto-frame the map to just the n households in a generated
// route. Deliberately excludes the leg leading into the #1 household —
// the configured start-point leg when one's set, or the seed→household-1
// leg otherwise — and the return-to-start leg: "the route" for framing
// purposes is the stops themselves and the road paths between them, not
// whatever brought the walk to its first stop.
//
// Bounds include each remaining leg's actual route_path geometry, not
// just the household endpoints — a road can bow well outside the
// straight line between two points, and framing on endpoints alone let
// that bulge hang off the edge of the viewport. Entry 0's own household
// point is still included via the fallback below; only the incoming leg
// geometry is dropped, unconditionally, regardless of whether a
// configured start or a seed produced it.
//
// map.getSize() gives real on-screen pixels so the ~10% padding scales
// with whatever window size Stan is actually running at, rather than a
// fixed pixel guess. maxZoom stops a tiny 2-3 household cluster from
// zooming in absurdly far — value picked by feel, revisit if it looks
// wrong in practice.
const ROUTE_FRAME_MAX_ZOOM = 16;

function frameRouteBounds(entries) {
    if (!entries || entries.length === 0 || !MapView.map) return;
    const points = [];
    entries.forEach((e, idx) => {
        if (idx > 0 && e.route_path && e.route_path.length > 1) {
            e.route_path.forEach(p => points.push([p.lat, p.lon]));
        } else {
            points.push([e.latitude, e.longitude]);
        }
    });
    const size = MapView.map.getSize();
    const padX = Math.round(size.x * 0.1);
    const padY = Math.round(size.y * 0.1);
    MapView.map.fitBounds(points, { padding: [padX, padY], maxZoom: ROUTE_FRAME_MAX_ZOOM });
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

// Issue #80: an empty/un-ingested roads.db and a fully-ingested one that
// simply has no nearby roads for this particular route produce the same
// downstream shape today — a normal-looking route where every leg
// happens to be straight-line. route_distance_source already carries
// enough information to tell "the graph was never usable for this route"
// apart from "this route has a mix of road and straight-line legs, as
// expected on the edge of coverage": only the former — every route-
// context leg falling back — gets a banner. Non-route lists
// (distance_context !== 'route', no route start configured) never carry
// route_distance_source at all and are excluded rather than counted as
// vacuously "all straight-line".
function routeUsedStraightLineFallback(entries) {
    const routeEntries = entries.filter(e => e.distance_context === 'route');
    if (routeEntries.length === 0) return false;
    return routeEntries.every(e =>
        e.route_distance_source === 'straight_line_no_snap' ||
        e.route_distance_source === 'straight_line_no_graph'
    );
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

    // Issue #89 — attach each stop's visit history before building the
    // Copy Text string / PDF, both of which read entry.visits.
    await attachVisitHistory(entries);

    // Previous run's badges need to go back to the default pin before this
    // run's results get their own — otherwise a badge from a household
    // that isn't part of the new list sticks around looking like it still
    // is.
    Object.keys(MapView.markerBaseState).forEach(key => { MapView.markerBaseState[key] = { type: 'default' }; });
    Object.values(MapView.markersByAddressKey).forEach(marker => marker.setIcon(new L.Icon.Default()));
    entries.forEach((e, idx) => {
        MapView.markerBaseState[e.address_key] = { type: 'route', order: idx + 1 };
        const marker = MapView.markersByAddressKey[e.address_key];
        if (marker) marker.setIcon(routeMarkerIcon(idx + 1));
    });
    // A newly generated route replaces whatever badges/state a search
    // might have been highlighting against — clear the search rather
    // than reapply it over icons that no longer mean what they did.
    clearIconSearch();

    const startInfo = await getRouteStartInfo();
    const startsAtRoute = entries.length > 0 && entries[0].distance_context === 'route' && !!startInfo;
    const returnLeg = computeReturnLeg(entries, startInfo);
    const tagLabel = currentTagLabel();
    const roadsDegraded = routeUsedStraightLineFallback(entries);

    MapView.lastVisitListText = buildVisitListText(entries, returnLeg, startsAtRoute ? startInfo : null, roadsDegraded);
    MapView.lastVisitEntries = entries;
    MapView.lastVisitReturnLeg = returnLeg;
    MapView.lastVisitTagLabel = tagLabel;
    MapView.lastVisitStartInfo = startsAtRoute ? startInfo : null;
    MapView.lastVisitRoadsDegraded = roadsDegraded;

    // Issue #40, #84 — draw the route/snap overlay for this new list.
    // Always shown now; no setting gates it.
    await drawRouteOverlay(entries);

    // Issue #43 — frame the map to just this route's households, start/
    // return legs excluded.
    frameRouteBounds(entries);

    // Issue #43 — full-screen modal is gone for this flow. Non-blocking
    // controls float over the map instead; the preview panel's content
    // is built now but stays hidden until the person opts in via the
    // toggle button (on-demand, not shown automatically — most routes
    // are checked visually on the map, not by reading the list).
    document.getElementById('mapPreviewPanel').innerHTML = buildVisitListHtml(entries, returnLeg, startsAtRoute ? startInfo : null, roadsDegraded);
    document.getElementById('mapPreviewPanel').hidden = true;
    document.getElementById('mapPostRouteControls').hidden = false;
}

function buildVisitListHtml(entries, returnLeg, startInfo, roadsDegraded) {
    const items = entries.map((e, idx) => {
        const cityLine = [e.city, e.state].filter(Boolean).join(' ') + (e.zip ? ' ' + e.zip : '');
        const phones = e.phones.length ? ` — ${e.phones.map(escapeHtml).join(', ')}` : '';
        const distLabel = e.distance_context === 'route'
            ? (idx === 0 ? 'from start point' : 'from previous stop')
            : 'from seed';
        return `<li>${escapeHtml(formatEntryAddress(e))}${cityLine.trim() ? ', ' + escapeHtml(cityLine.trim()) : ''}
            — ${e.names.map(escapeHtml).join(', ')}${phones}
            <span class="visit-list-dist"> (${metersToMiles(e.distance_meters).toFixed(2)} mi ${distLabel})</span></li>`;
    }).join('');
    const returnItem = returnLeg
        ? `<li class="visit-list-return">↩ Back to ${escapeHtml(returnLeg.label)}
            <span class="visit-list-dist"> (${metersToMiles(returnLeg.meters).toFixed(2)} mi)</span></li>`
        : '';
    // Issue #80 — same warning as the Copy Text / PDF outputs, styled
    // like the on-screen message bar's error state rather than a plain
    // paragraph, so it doesn't read as just another list note.
    const warning = roadsDegraded
        ? '<div class="visit-list-warning" style="color:#b02a2a;font-weight:bold;margin-bottom:8px;">⚠ Road database has no usable data for this route — distances below are straight-line, not road distance. Re-ingest under Road Management.</div>'
        : '';
    return `
        <h3>Visit List (${entries.length} addresses)</h3>
        ${warning}
        ${startInfo ? `<div class="visit-list-start">Starting at ${escapeHtml(startInfo.label)}</div>` : ''}
        <ol class="visit-list-items">${items}${returnItem}</ol>
    `;
}

function togglePreviewPanel() {
    const panel = document.getElementById('mapPreviewPanel');
    if (panel) panel.hidden = !panel.hidden;
}

function buildVisitListText(entries, returnLeg, startInfo, roadsDegraded) {
    const lines = [];
    if (roadsDegraded) lines.push('⚠ Road database has no usable data for this route — distances below are straight-line, not road distance. Re-ingest under Road Management.');
    if (startInfo) lines.push(`Starting at ${startInfo.label}`);
    entries.forEach((e, idx) => {
        const cityLine = [e.city, e.state].filter(Boolean).join(' ') + (e.zip ? ' ' + e.zip : '');
        const phones = e.phones.length ? ` — ${e.phones.join(', ')}` : '';
        // Issue #89 — numbered to match the PDF/preview-panel stop
        // numbering, and followed by this stop's visit history (if any).
        lines.push(`${idx + 1}. ${formatEntryAddress(e)}${cityLine.trim() ? ', ' + cityLine.trim() : ''} — ${e.names.join(', ')}${phones}`);
        (e.visits || []).forEach(v => {
            const who = e.visitsMultiHousehold && v.name ? `${v.name} — ` : '';
            lines.push(`   - ${v.date}: ${who}${v.comments || '(no comments)'}`);
        });
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
    const roadsDegraded = MapView.lastVisitRoadsDegraded;

    const faint = '#777777';
    const body = entries.map((e, idx) => {
        const cityLine = [e.city, e.state].filter(Boolean).join(' ') + (e.zip ? ' ' + e.zip : '');
        const phones = e.phones.length ? ` — ${e.phones.join(', ')}` : '';
        const distLabel = e.distance_context === 'route'
            ? (idx === 0 ? 'from start point' : 'from previous stop')
            : 'from seed';
        const mainLine = {
            text: [
                { text: `${idx + 1}. `, bold: true },
                `${formatEntryAddress(e)}${cityLine.trim() ? ', ' + cityLine.trim() : ''} — ${e.names.join(', ')}${phones} `,
                { text: `(${metersToMiles(e.distance_meters).toFixed(2)} mi ${distLabel})`, color: faint, fontSize: 8 },
            ],
        };
        // Issue #89 — visit history under the stop, compact: same 8pt
        // fontSize the distance annotation above already uses, tight
        // 1pt top margin. Base defaultStyle fontSize (10) is untouched —
        // this is added content, not a shrink of the existing line.
        if (e.visits && e.visits.length) {
            return {
                margin: [0, 0, 0, 6],
                stack: [
                    mainLine,
                    ...e.visits.map(v => ({
                        text: `${v.date}: ${e.visitsMultiHousehold && v.name ? v.name + ' — ' : ''}${v.comments || '(no comments)'}`,
                        fontSize: 8,
                        color: faint,
                        margin: [12, 1, 0, 0],
                    })),
                ],
            };
        }
        mainLine.margin = [0, 0, 0, 6];
        return mainLine;
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
            // Issue #80 — same signal as the on-screen preview and Copy
            // Text output, printed prominently since this may be the
            // only copy someone has once it's on paper.
            ...(roadsDegraded ? [{
                text: '⚠ Road database has no usable data for this route — distances below are straight-line, not road distance. Re-ingest under Road Management.',
                bold: true,
                color: '#b02a2a',
                margin: [0, 0, 0, 12],
            }] : []),
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

async function copyVisitList() {
    const btn = document.getElementById('mapCopyVisitListBtn');
    if (!btn) return;
    const defaultLabel = '⎘ Copy Text';
    try { await navigator.clipboard.writeText(MapView.lastVisitListText || ''); btn.textContent = 'Copied!'; }
    catch (e) { btn.textContent = 'Copy failed'; }
    setTimeout(() => { if (btn) btn.textContent = defaultLabel; }, 1500);
}

function resetSeed() {
    MapView.seedGroupKey = null;
    MapView.seedHouseholdId = null;
    Object.keys(MapView.markerBaseState).forEach(key => { MapView.markerBaseState[key] = { type: 'default' }; });
    Object.values(MapView.markersByAddressKey).forEach(marker => marker.setIcon(new L.Icon.Default()));
    // Icons going back to normal here means whatever route was on
    // display no longer corresponds to anything selected — erase it too,
    // along with the floating controls/preview panel that referred to it.
    clearActiveRoute();
    // Clears the search field itself, not just the highlighting it was
    // producing — same as a freshly generated route (generateVisitList).
    clearIconSearch();
    showMessage('Seed cleared.', CONSTANTS.MESSAGE_TYPES.INFO, 2000);
}

// Right-click "copy lat, lon" menu. Reuses .hamburger-menu/.hamburger-
// menu-item (sidebar.js/hamburger-menu.css) for theming; the
// .map-coord-menu class (map-view.css) widens it beyond that class's
// fixed 220px so the coordinate string doesn't wrap. Position (top/left)
// is still set here at runtime since it follows the click point, same
// reason sidebar.js's positionHamburgerMenu() sets its menu's position
// in JS rather than CSS — no inline styling beyond that positioning.
function showMapCoordMenu(e) {
    document.querySelectorAll('.map-coord-menu').forEach(el => el.remove());
    const { lat, lng } = e.latlng;
    const label = `${lat.toFixed(6)}, ${lng.toFixed(6)}`;

    const menu = document.createElement('div');
    menu.className = 'hamburger-menu map-coord-menu open';
    menu.innerHTML = `<div class="hamburger-menu-item" data-action="copy-latlon">Copy ${escapeHtml(label)}</div>`;
    document.body.appendChild(menu);

    const { clientX, clientY } = e.originalEvent;
    const maxLeft = window.innerWidth - menu.offsetWidth - 8;
    const maxTop = window.innerHeight - menu.offsetHeight - 8;
    menu.style.left = `${Math.max(8, Math.min(clientX, maxLeft))}px`;
    menu.style.top = `${Math.max(8, Math.min(clientY, maxTop))}px`;

    menu.querySelector('[data-action="copy-latlon"]').addEventListener('click', async () => {
        try {
            await navigator.clipboard.writeText(label);
            showMessage('Coordinates copied.', CONSTANTS.MESSAGE_TYPES.SUCCESS, 2000);
        } catch (err) {
            console.error('showMapCoordMenu: clipboard write failed', err);
            showMessage('Could not copy coordinates — see console for details', CONSTANTS.MESSAGE_TYPES.ERROR);
        }
        menu.remove();
    });

    // Dismiss on any outside click, one shot — same pattern
    // mapPreviewPanel uses above and sidebar.js's hamburger menu uses.
    setTimeout(() => {
        function onDocClick(ev) {
            if (!menu.contains(ev.target)) {
                menu.remove();
                document.removeEventListener('click', onDocClick);
            }
        }
        document.addEventListener('click', onDocClick);
    }, 0);
}

