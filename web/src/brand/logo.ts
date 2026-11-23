// The Deptheos logo, from the app icon's glyph
// (DeptheosAppIcon.icon/Assets/deptheos_icon.svg, a 1025-unit square),
// its paths copied whole, never redrawn. `BrandTile.astro` inlines it in the topbar,
// footer, and wherever a page renders `<BrandTile />`;
// scripts/make-icons.ts writes the favicons from it. "Deptheos" and the
// logo are trademarks (TRADEMARKS.md).
//
// Runs in Node (the Vite config and scripts), not in the browser.

import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

function findRepoRoot(): string {
  let dir = dirname(fileURLToPath(import.meta.url));
  for (let i = 0; i < 8; i++) {
    if (existsSync(resolve(dir, "DeptheosAppIcon.icon"))) return dir;
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return resolve(process.cwd(), "..");
}

export const LOGO_SOURCE = resolve(findRepoRoot(), "DeptheosAppIcon.icon/Assets/deptheos_icon.svg");

/** The icon's fill (DeptheosAppIcon.icon/icon.json): the app's blue, lighter at the top, solid from 70% down. */
const TILE_TOP = "#3b7bd0";
const TILE_BOTTOM = "#1f57a3";
const SIZE = 1025.06;
/** Corner radius of the tile, in the source's units (Apple's icon grid uses about 22.5% of the side). */
const TILE_RADIUS = 230;

let paths: string[] | undefined;
function glyphPaths(): string[] {
  if (!paths) {
    const svg = readFileSync(LOGO_SOURCE, "utf8");
    paths = [...svg.matchAll(/<path\b[^>]*?\sd="([^"]+)"/g)].map((m) => m[1]);
    if (paths.length === 0) throw new Error(`no paths in ${LOGO_SOURCE}`);
  }
  return paths;
}

const body = (fill: string) => `<g fill="${fill}">${glyphPaths().map((d) => `<path d="${d}"/>`).join("")}</g>`;

let tiles = 0;
/**
 * The app icon: the glyph on the blue rounded square, a gradient id per call. `standalone`
 * makes a file of its own; `radius` 0 the square iOS rounds itself (apple-touch-icon).
 */
export function tileSvg(className = "", standalone = false, radius = TILE_RADIUS): string {
  const id = `deptheos-tile-${++tiles}`;
  const attrs = standalone ? ` xmlns="http://www.w3.org/2000/svg" width="${SIZE}" height="${SIZE}"` : ` class="${className}" aria-hidden="true" focusable="false"`;
  return (
    `<svg${attrs} viewBox="0 0 ${SIZE} ${SIZE}">` +
    `<defs><linearGradient id="${id}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${TILE_TOP}"/><stop offset="0.7" stop-color="${TILE_BOTTOM}"/></linearGradient></defs>` +
    `<rect width="${SIZE}" height="${SIZE}" rx="${radius}" fill="url(#${id})"/>${body("#fff")}</svg>`
  );
}
