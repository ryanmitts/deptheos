// Upload public/media/ to the bucket, each set under its hash (scripts/media-sets.ts), with
// rclone. A set already there is skipped file by file, so a deploy uploads only what changed.
// Nothing is deleted: the sets of earlier deploys stay, so the live pages and a rollback
// keep their media.
//
//   npx tsx scripts/sync-media.ts            upload
//   npx tsx scripts/sync-media.ts --dry-run  say what would be uploaded
//
// Needs rclone with a remote named `r2` for the account's R2 (README.md, "Deploying").

import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { MEDIA_DIR, MEDIA_REMOTE, mediaSets } from "./media-sets.ts";

const dryRun = process.argv.includes("--dry-run");

for (const [set, { files, hash }] of Object.entries(mediaSets())) {
  const to = `${MEDIA_REMOTE}/${set}/${hash}`;
  console.log(`${set} → ${to} (${files.length} files)`);
  const run = spawnSync(
    "rclone",
    [
      "copy",
      "--s3-no-check-bucket",
      join(MEDIA_DIR, set),
      to,
      "--files-from-raw",
      "-",
      // The address holds this content for good (the hash is in it).
      "--header-upload",
      "Cache-Control: public, max-age=31536000, immutable",
      "--transfers",
      "16",
      "--stats-one-line",
      "-v",
      ...(dryRun ? ["--dry-run"] : []),
    ],
    { input: files.join("\n"), stdio: ["pipe", "inherit", "inherit"] },
  );
  if (run.error) throw new Error(`rclone did not start (is it installed?): ${run.error.message}`);
  if (run.status !== 0) process.exit(run.status ?? 1);
  if (dryRun) continue;
  // The pages are about to name these files: every one must be in the bucket.
  const listed = spawnSync("rclone", ["lsf", "-R", "--files-only", to], { encoding: "utf8" });
  if (listed.error) throw new Error(`rclone lsf did not start: ${listed.error.message}`);
  if (listed.status !== 0) throw new Error(`could not list ${to}: ${listed.stderr}`);
  const there = new Set(listed.stdout.split("\n"));
  const missing = files.filter((file) => !there.has(file));
  if (missing.length) throw new Error(`${to} lacks ${missing.length} files after the upload, the first ${missing[0]}`);
}
