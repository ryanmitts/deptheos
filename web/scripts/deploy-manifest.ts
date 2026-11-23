// Record a deploy in the bucket: which commit, which version of the pages, and which hash of
// each media set its pages name. One JSON file per deploy in site/_deploys/, named by time
// and commit so a listing is in order:
//
//   rclone lsf r2:deptheos/site/_deploys/                      every deploy
//   rclone cat r2:deptheos/site/_deploys/<file>                one of them
//
// A set's files are at site/<set>/<hash>/, so a hash no kept deploy names can be removed.
//
//   npx tsx scripts/deploy-manifest.ts <commit> <deploy|preview> <pages version id>
//
// Run by scripts/deploy.sh after the pages are out. Run it from the commit that was deployed:
// the hashes are read from public/media/ as it is now.

import { spawnSync } from "node:child_process";
import { MEDIA_BASE, MEDIA_REMOTE, mediaSets } from "./media-sets.ts";

const [commit, kind, version] = process.argv.slice(2);
if (!commit || (kind !== "deploy" && kind !== "preview") || !version) {
  console.error("usage: deploy-manifest.ts <commit> <deploy|preview> <pages version id>");
  process.exit(2);
}

const at = new Date().toISOString().replace(/\.\d+Z$/, "Z");
const manifest = {
  at,
  commit,
  // "deploy": deptheos.com served these pages from this moment. "preview": uploaded to look at, not served.
  kind,
  // The Worker version holding the pages (`npx wrangler versions list`, and what a rollback names).
  pages: version,
  mediaBase: MEDIA_BASE,
  media: Object.fromEntries(Object.entries(mediaSets()).map(([set, { hash, files }]) => [set, { hash, files: files.length }])),
};

const to = `${MEDIA_REMOTE}/_deploys/${at.replace(/:/g, "-")}-${commit}.json`;
// --s3-no-check-bucket: rcat would otherwise try to create the bucket, which a token for one bucket may not.
const run = spawnSync("rclone", ["rcat", to, "--s3-no-check-bucket", "--header-upload", "Content-Type: application/json"], {
  input: `${JSON.stringify(manifest, null, 2)}\n`,
  stdio: ["pipe", "inherit", "inherit"],
});
if (run.error) throw new Error(`rclone did not start: ${run.error.message}`);
if (run.status !== 0) process.exit(run.status ?? 1);
console.log(`recorded ${to}`);
