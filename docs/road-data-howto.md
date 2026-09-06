# Getting Road Data for Lost Sheep's Road Graph

External steps — not part of the app itself. Produces a `.pbf` file you feed
into the app's native file picker for road-graph ingest (issue #7 / #39).

## 1. Download source data

Geofabrik hosts free, regularly-updated OSM extracts:

- **US, whole country:** https://download.geofabrik.de/north-america/us-latest.osm.pbf
  (large — several GB)
- **Per-state extracts:** https://download.geofabrik.de/north-america/us.html
  lists every state individually (e.g. `west-virginia-latest.osm.pbf`,
  `virginia-latest.osm.pbf`)

**Use per-state files only if your area of interest fits inside one state.**
If your area spans a state line (e.g. part of WV and part of VA), download a
single larger source file that covers both — a regional file
(`north-america-latest.osm.pbf`) or the whole-country `us-latest.osm.pbf` —
and extract once from that. Combining two separately-extracted state files
does **not** reliably preserve road connectivity across the state line (see
below); extracting once from one larger source avoids the problem entirely.

## 2. Install osmium-tool

Not bundled with the app — install separately:

- **Linux:** `sudo apt install osmium-tool` (Ubuntu/Debian) or your distro's
  package manager
- **macOS:** `brew install osmium-tool`
- **Windows:** via WSL2, same as Linux, or see osmium-tool's own docs for a
  native build

## 3. Define your area

**Bounding box** (simple rectangle) — good enough when your area doesn't
need to hug an irregular boundary and doesn't cross into territory you want
excluded:

```bash
osmium extract -b <left>,<bottom>,<right>,<top> \
  us-latest.osm.pbf \
  -o my-area.osm.pbf
```

`left,bottom,right,top` = `min_longitude,min_latitude,max_longitude,max_latitude`
(decimal degrees). Find these by looking up your area's corners on any map
tool that shows lat/long (e.g. openstreetmap.org — right-click a point shows
its coordinates).

Example — a box roughly covering a chunk of the WV/VA border area:

```bash
osmium extract -b -80.5,37.0,-79.5,38.0 \
  us-latest.osm.pbf \
  -o border-area.osm.pbf
```

**Polygon** (irregular shape, or spanning a state line) — recommended when a
rectangle would pull in far more than you need, or when your area straddles
a state boundary. Define a polygon in a `.poly` file (Osmosis polygon
filter format — a simple text list of lat/long vertices; several tools can
export one, or write it by hand for a simple shape) and run:

```bash
osmium extract -p my-area.poly \
  us-latest.osm.pbf \
  -o my-area.osm.pbf
```

A single polygon extraction — even across the WV/VA line — keeps the road
network's connectivity intact within your area, because the cut only happens
at your polygon's outer edge, never down the middle where you need roads to
connect.

## 4. Ingest into Lost Sheep

Open the app's road-graph ingest screen and pick the resulting `.pbf` file
(`my-area.osm.pbf` / `border-area.osm.pbf`) via the native file picker.
Ingest is a one-time operation per file — see #39 for the plan to stop this
data from being wiped on every database restore.

## Notes

- Bigger source file (`us-latest.osm.pbf` vs. a state extract) costs more
  disk space and a longer `osmium extract` run, but only has to be
  downloaded once — you can extract multiple different areas from the same
  source file without re-downloading.
- If two areas you've extracted separately need to connect to each other
  (e.g. you extract WV and VA as two separate polygon runs for some other
  reason), the connectivity problem described above still applies — export
  one polygon spanning both if you need the roads between them to link up.


## Appendix

**Stake Bounding Box**

Upper left:  39.66391, -79.63754
Lower right: 38.22396, -77.71308
