#!/usr/bin/env bash
# Builds the installers for a tag on GitHub Actions (release.yml), waits
# for the run, and downloads the release's installers into dist/<tag>/.
#
# Usage: scripts/ci-build.sh [TAG] [PLATFORMS]
#   TAG: defaults to v<version in src-tauri/Cargo.toml>. It must already
#     be pushed (git tag vX.Y.Z && git push origin vX.Y.Z).
#   PLATFORMS: all (default), linux or windows.
#
# The run builds the tag's code with the workflow from main, and replaces
# the release's assets of the same name. Needs `gh`, logged in.
set -euo pipefail
cd "$(dirname "$0")/.."

repo=sparsileg/lostsheep
version=$(sed -n 's/^version = "\(.*\)"/\1/p' src-tauri/Cargo.toml | head -1)
tag=${1:-v$version}
platforms=${2:-all}

case $platforms in
    all|linux|windows) ;;
    *) echo "PLATFORMS must be all, linux or windows, not '$platforms'" >&2; exit 2 ;;
esac

if ! git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null; then
    echo "Tag $tag isn't on GitHub. Push it first: git tag $tag && git push origin $tag" >&2
    exit 1
fi

# A published release's files are what people download: ask first.
if gh release view "$tag" -R "$repo" --json isDraft --jq .isDraft 2>/dev/null | grep -qx false; then
    read -r -p "Release $tag is published; replace its $platforms installers? [y/N] " answer
    [[ $answer == [yY]* ]] || { echo "Cancelled."; exit 1; }
fi

# Find the run this dispatch starts: the newest manual run created after now.
since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
gh workflow run release.yml -R "$repo" --ref main -f tag="$tag" -f platforms="$platforms"
echo "Started the $platforms build of $tag; waiting for the run to appear..."
run=""
for _ in $(seq 30); do
    sleep 4
    run=$(gh run list -R "$repo" --workflow release.yml --event workflow_dispatch --limit 5 \
        --json databaseId,createdAt \
        --jq "map(select(.createdAt >= \"$since\")) | sort_by(.createdAt) | last | .databaseId // empty")
    [ -n "$run" ] && break
done
[ -n "$run" ] || { echo "Couldn't find the run; see https://github.com/$repo/actions" >&2; exit 1; }
echo "https://github.com/$repo/actions/runs/$run"

if ! gh run watch "$run" -R "$repo" --exit-status --interval 30; then
    echo "The build failed; see https://github.com/$repo/actions/runs/$run" >&2
    exit 1
fi

dir=dist/$tag
mkdir -p "$dir"
case $platforms in
    all)     patterns=(-p '*.deb' -p '*.rpm' -p '*.exe') ;;
    linux)   patterns=(-p '*.deb' -p '*.rpm') ;;
    windows) patterns=(-p '*.exe') ;;
esac
gh release download "$tag" -R "$repo" -D "$dir" --clobber "${patterns[@]}"
echo
echo "Downloaded to $dir:"
ls -lh "$dir"
