/**
 * Build-time lookup and formatting for engine parameters (`generated/manifest.json`)
 * and measured benchmark records (`measured/*.json`, `measured/benchmarks/*.json`).
 *
 * Unknown paths throw so stale or mistyped references always fail the build.
 */

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

function findWebRoot(): string {
  let dir = dirname(fileURLToPath(import.meta.url));
  for (let i = 0; i < 8; i++) {
    if (existsSync(resolve(dir, "generated/manifest.json"))) return dir;
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return process.cwd();
}

const WEB_ROOT = findWebRoot();

export interface FormatOptions {
  digits?: number;
  scale?: number;
  offset?: number;
}

function loadJson(relPath: string, hint: string): Record<string, unknown> {
  const fullPath = resolve(WEB_ROOT, relPath);
  if (!existsSync(fullPath)) throw new Error(`web/${relPath} is missing: ${hint}`);
  return JSON.parse(readFileSync(fullPath, "utf8")) as Record<string, unknown>;
}

let cachedParams: unknown;
let cachedMeasured: Record<string, unknown> | undefined;

function isProdBuild(): boolean {
  return Boolean((import.meta as ImportMeta & { env?: { PROD?: boolean } }).env?.PROD);
}

function loadParams(): unknown {
  if (isProdBuild() && cachedParams !== undefined) return cachedParams;
  const params = loadJson("generated/manifest.json", "run `npm run export-params` and commit it").params;
  if (isProdBuild()) cachedParams = params;
  return params;
}

function loadMeasured(): Record<string, unknown> {
  if (isProdBuild() && cachedMeasured !== undefined) return cachedMeasured;
  const dir = resolve(WEB_ROOT, "measured");
  const all: Record<string, unknown> = {};
  for (const f of existsSync(dir) ? readdirSync(dir) : []) {
    if (f.endsWith(".json")) {
      all[f.slice(0, -5)] = (JSON.parse(readFileSync(resolve(dir, f), "utf8")) as { values: unknown }).values;
    }
  }
  const bench = resolve(dir, "benchmarks");
  const records: Record<string, unknown> = {};
  for (const f of existsSync(bench) ? readdirSync(bench) : []) {
    if (f.endsWith(".json")) {
      records[f.slice(0, -5)] = JSON.parse(readFileSync(resolve(bench, f), "utf8"));
    }
  }
  all.benchmarks = records;
  if (isProdBuild()) cachedMeasured = all;
  return all;
}

function lookup(data: unknown, path: string, source: string): number | string {
  let v: unknown = data;
  for (const key of path.split(".")) {
    if (v === null || typeof v !== "object" || !(key in v)) {
      throw new Error(`${source} has no "${path}"`);
    }
    v = (v as Record<string, unknown>)[key];
  }
  if (typeof v !== "number" && typeof v !== "string") {
    throw new Error(`${source} "${path}" is not a number or text`);
  }
  return v;
}

export function formatValue(v: number | string, opts: FormatOptions = {}): string {
  if (typeof v === "string") return v;
  const x = v * (opts.scale ?? 1) + (opts.offset ?? 0);
  const text = opts.digits !== undefined ? x.toFixed(opts.digits) : String(Math.round(x * 1e6) / 1e6);
  // Thousands separators on the whole part: 1650 → 1,650.
  return text.replace(/^(-?\d+)/, (whole) => (Math.abs(Number(whole)) >= 1000 ? Number(whole).toLocaleString("en") : whole));
}

export function getParam(path: string): number | string {
  return lookup(loadParams(), path, "manifest.json params (re-run `npm run export-params` if it is stale)");
}

export function formatParam(path: string, opts?: FormatOptions): string {
  return formatValue(getParam(path), opts);
}

export function getMeasured(path: string): number | string {
  return lookup(loadMeasured(), path, "measured/");
}

export function formatMeasured(path: string, opts?: FormatOptions): string {
  return formatValue(getMeasured(path), opts);
}

function decodeEntities(text: string): string {
  return text
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'");
}

/**
 * Extract a docs page's `<h2>` sections for the sidebar outline: each `<h2>` with an `id`
 * of its own or of the `<section>` that opens just before it.
 */
export function extractOutline(html: string): [string, string][] {
  const found: [string, string][] = [];
  let from = 0;
  for (const h2 of html.matchAll(/<h2([^>]*)>([\s\S]*?)<\/h2>/g)) {
    const before = html.slice(from, h2.index);
    const section = [...before.matchAll(/<section\b[^>]*\bid="([^"]+)"/g)].at(-1)?.[1];
    const id = /\bid="([^"]+)"/.exec(h2[1])?.[1] ?? section;
    from = h2.index + h2[0].length;
    if (id) found.push([id, decodeEntities(h2[2].replace(/<[^>]+>/g, "").replace(/\s+/g, " ").trim())]);
  }
  return found;
}
