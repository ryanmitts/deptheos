// The media sets in public/media/ and the hash of each one's contents: what a deploy's
// addresses are made from (src/media.ts). Used by the build (src/integrations/deptheos.ts) and by the
// upload (scripts/sync-media.ts), so the pages and the bucket agree.
//
// Runs in Node.

import { createHash } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { setOf, type MediaTarget } from "../src/media.ts";

/** public/media/, the media as the pages name it. */
export const MEDIA_DIR = resolve(import.meta.dirname, "../public/media");
/** Where a deploy's media is served from: the `site/` folder of the bucket behind images.deptheos.com. */
export const MEDIA_BASE = "https://images.deptheos.com/site";
/** The same folder as rclone names it: the remote `r2` (README.md, "Deploying") and the bucket. */
export const MEDIA_REMOTE = "r2:deptheos/site";

/** A set's files (paths within the set) and the hash of their names and contents. */
export interface MediaSet {
  files: string[];
  hash: string;
}

/** Every file under `dir`, as sorted paths relative to it; dotfiles (.DS_Store) are left out. */
function walk(dir: string, prefix = ""): string[] {
  return readdirSync(dir, { withFileTypes: true })
    .filter((entry) => !entry.name.startsWith("."))
    .sort((a, b) => (a.name < b.name ? -1 : 1))
    .flatMap((entry) => (entry.isDirectory() ? walk(join(dir, entry.name), `${prefix}${entry.name}/`) : [`${prefix}${entry.name}`]));
}

/** Every media set by name, with its files and hash. */
export function mediaSets(): Record<string, MediaSet> {
  const bySet: Record<string, string[]> = {};
  for (const file of walk(MEDIA_DIR)) (bySet[setOf(file)] ??= []).push(file);
  return Object.fromEntries(
    Object.entries(bySet).map(([set, paths]) => {
      const hash = createHash("sha256");
      for (const path of paths) {
        hash.update(`${path.slice(set.length + 1)}\0`);
        hash.update(readFileSync(join(MEDIA_DIR, path)));
      }
      return [set, { files: paths.map((path) => path.slice(set.length + 1)), hash: hash.digest("hex").slice(0, 12) }];
    }),
  );
}

/** The target a deploy's pages are built for. */
export function mediaTarget(): MediaTarget {
  return { base: MEDIA_BASE, sets: Object.fromEntries(Object.entries(mediaSets()).map(([set, { hash }]) => [set, hash])) };
}
