/**
 * Deptheos Astro integration:
 * - Injects `__MEDIA__` into Vite so `mediaUrl()` resolves to `/media/` in dev and
 *   content-hashed R2 URLs when `MEDIA=cdn`.
 * - Adds `@vitejs/plugin-basic-ssl` and Tailscale MagicDNS host allowance when `LAN=1`.
 * - After static build:
 *   1. Validates every `/media/...` path in built HTML exists under `public/`.
 *   2. When `MEDIA=cdn`, rewrites `/media/...` URLs to the R2 bucket, deletes `dist/media/`,
 *      fails if any `.todo` / `.shot-todo` placeholders remain, and fails if any `/media/`
 *      reference was left behind in HTML or CSS.
 *   3. Writes `sitemap.xml` and `robots.txt` into `dist/`.
 */

import { existsSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import basicSsl from "@vitejs/plugin-basic-ssl";
import type { AstroIntegration } from "astro";
import { mediaTarget } from "../../scripts/media-sets.ts";
import { resolveMedia, type MediaTarget } from "../media.ts";
import { NOT_FOUND, pagePath, PAGES } from "../pages.ts";
import { SITE_URL } from "../data/site.ts";

const escapedSiteUrl = SITE_URL.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const mediaHrefPattern = new RegExp(`(?:${escapedSiteUrl})?(\\/media\\/[^"'\\s,)<>]+)`, "g");

export function deptheosIntegration(): AstroIntegration {
  const target: MediaTarget | null = process.env.MEDIA === "cdn" ? mediaTarget() : null;
  const isLan = process.env.LAN === "1";
  let projectRoot = "";

  return {
    name: "deptheos-site",
    hooks: {
      "astro:config:setup"({ config, updateConfig }) {
        projectRoot = fileURLToPath(config.root);
        updateConfig({
          vite: {
            define: { __MEDIA__: JSON.stringify(target) },
            ...(isLan
              ? {
                  plugins: [basicSsl()],
                  server: { allowedHosts: [".ts.net"] },
                }
              : {}),
          },
        });
      },
      "astro:build:done"({ dir }) {
        const outDir = fileURLToPath(dir);
        const publicDir = resolve(projectRoot, "public");

        const htmlFiles = readdirSync(outDir, { recursive: true, encoding: "utf8" }).filter((f) => f.endsWith(".html"));
        const expected = new Set([...Object.values(PAGES), NOT_FOUND]);
        const built = new Set(htmlFiles);
        const unexpected = [...built].filter((f) => !expected.has(f));
        const missing = [...expected].filter((f) => !built.has(f));
        if (unexpected.length || missing.length) {
          throw new Error(
            `src/pages.ts is out of sync with built HTML` +
              (unexpected.length ? `; unlisted: ${unexpected.join(", ")}` : "") +
              (missing.length ? `; missing: ${missing.join(", ")}` : ""),
          );
        }

        for (const rel of htmlFiles) {
          const full = resolve(outDir, rel);
          const before = readFileSync(full, "utf8");
          const after = before.replace(mediaHrefPattern, (whole, path: string) => {
            if (!existsSync(resolve(publicDir, path.slice(1)))) {
              throw new Error(`${rel}: no file public${path}`);
            }
            return target ? resolveMedia(path, target) : whole;
          });
          if (after !== before) writeFileSync(full, after, "utf8");
        }

        if (target) {
          rmSync(resolve(outDir, "media"), { recursive: true, force: true });

          const todos = htmlFiles.filter((file) => /class="[^"]*\b(?:shot-todo|todo)\b/.test(readFileSync(resolve(outDir, file), "utf8")));
          if (todos.length) {
            throw new Error(`built for deploy, but placeholders remain in: ${todos.join(", ")}`);
          }

          const left = readdirSync(outDir, { recursive: true, encoding: "utf8" })
            .filter((file) => /\.(html|css)$/.test(file))
            .filter((file) => readFileSync(resolve(outDir, file), "utf8").includes("/media/"));
          if (left.length) {
            throw new Error(`built for the bucket, but these still name /media/: ${left.join(", ")}`);
          }
        }

        const urls = Object.values(PAGES).map((file) => `${SITE_URL}${pagePath(file)}`);
        writeFileSync(
          resolve(outDir, "sitemap.xml"),
          `<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">${urls
            .map((url) => `<url><loc>${url}</loc></url>`)
            .join("")}</urlset>`,
          "utf8",
        );
        writeFileSync(
          resolve(outDir, "robots.txt"),
          `User-agent: *\nAllow: /\nSitemap: ${SITE_URL}/sitemap.xml\n`,
          "utf8",
        );
      },
    },
  };
}
