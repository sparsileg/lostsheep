# Lost Sheep

## Maps and Routing

Download the desired regional maps from
`https://download.geofabrik.de/`. For example, the state of
Virginia. We don't need the entire state, however, so install a
package that allows you to extract the roads that fall within a
bounding box.

```
sudo apt install osmium-tool
```

Then use the vollowing command to extract the roads within the
specified bounding box.

```
#!/usr/bin/env bash
set -euo pipefail

STATE_FILE="virginia.osm.pbf"
BBOX="-78.348759,38.942655,-77.928386,39.465228"   # west,south,east,north

osmium extract -b "$BBOX" "$STATE_FILE" -o clipped.osm.pbf --overwrite
osmium tags-filter clipped.osm.pbf w/highway -o roads.osm.pbf --overwrite

echo "Done: roads.osm.pbf"
```

**Winchester Ward Boundary**

39.465228, -78.348759
38.942655, -77.928386
