// Every page of the site. The sitemap (src/integrations/deptheos.ts) and the pages
// scripts/smoke.ts and scripts/overflow-check.ts load all come from this list.
//
// Runs in Node (Astro config and scripts), not in the browser.

import { DOCS } from "./data/docs.ts";
import { GALLERY_ITEMS } from "./data/gallery.ts";

/** Every built page: its key and its output HTML file, one folder per page so URLs need no `.html`. */
export const PAGES: Record<string, string> = {
  home: "index.html",
  features: "features/index.html",
  download: "download/index.html",
  gallery: "gallery/index.html",
  ...Object.fromEntries(GALLERY_ITEMS.map((item) => [item.slug, `gallery/${item.slug}/index.html`])),
  docs: "docs/index.html",
  ...Object.fromEntries(DOCS.map((doc) => [doc.key, `docs/${doc.key}/index.html`])),
  support: "support/index.html",
  privacy: "privacy/index.html",
  about: "about/index.html",
  iphone: "iphone/index.html",
};

/**
 * The page served for an address with no file (`not_found_handling` in wrangler.jsonc). Built
 * like the others, but not one of `PAGES`: it is not in the sitemap and no check loads it.
 */
export const NOT_FOUND = "404.html";

/** A page's path on the site, from its HTML file: `docs/pyramid/index.html` → `/docs/pyramid/`. */
export const pagePath = (file: string) => `/${file.replace(/index\.html$/, "")}`;
