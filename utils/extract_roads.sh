#!/usr/bin/env bash

set -euo pipefail

STATE_FILE="$HOME/Downloads/virginia.osm.pbf"
BBOX="-78.348759,38.942655,-77.928386,39.465228"   # west,south,east,north

osmium extract -b "$BBOX" "$STATE_FILE" -o clipped.osm.pbf --overwrite
osmium tags-filter clipped.osm.pbf w/highway -o roads.osm.pbf --overwrite

echo "Done: roads.osm.pbf"
