#!/usr/bin/env bash
# Build the site and publish it: the media to the bucket behind
# images.deptheos.com, then the pages to deptheos.com (wrangler.jsonc).
#
#   npm run deploy              build, upload the media, deploy the pages: the domain serves them at once
#   npm run deploy -- --preview the same, but the pages are uploaded as a version that is not
#                               served; wrangler prints a preview URL to look at it
#
# The media goes first and under new addresses (a hash per set, src/media.ts),
# so the pages being served never lose a picture. Refuses a working tree with
# changes, so what is live is always a commit; the deploy's message in
# Cloudflare is that commit, and scripts/deploy-manifest.ts records it in the
# bucket with the hashes of the media its pages name. Needs `npx wrangler login` and an rclone remote
# named `r2` (README.md, "Deploying").
set -euo pipefail
cd "$(dirname "$0")/.."

case "${1:-}" in
  "")
    kind=deploy
    ;;
  --preview)
    kind=preview
    ;;
  *)
    echo "deploy: unknown argument '${1}' (usage: deploy.sh [--preview])" >&2
    exit 2
    ;;
esac
if [ "$#" -gt 1 ]; then
  echo "deploy: unexpected extra arguments (usage: deploy.sh [--preview])" >&2
  exit 2
fi

if [ -n "$(git status --porcelain -- .)" ]; then
  echo "deploy: web/ has changes; commit or stash them first" >&2
  exit 1
fi
commit=$(git rev-parse --short HEAD)

npm run typecheck
npm test
MEDIA=cdn npm run build
npx tsx scripts/sync-media.ts
log=$(mktemp)
trap 'rm -f "$log"' EXIT
if [ "$kind" = "preview" ]; then
  npx wrangler versions upload --message "$commit" 2>&1 | tee "$log"
else
  npx wrangler deploy --message "$commit" 2>&1 | tee "$log"
fi

# What went out, for later: the commit, the pages' version and each media set's hash.
version=$(sed $'s/\x1b\\[[0-9;]*m//g' "$log" | sed -n 's/.*Version ID: *//p' | tail -1 | tr -d '[:space:]')
if [ -z "$version" ]; then
  echo "deploy: the pages are out, but wrangler printed no version id, so the deploy is not recorded" >&2
  exit 1
fi
npx tsx scripts/deploy-manifest.ts "$commit" "$kind" "$version"
