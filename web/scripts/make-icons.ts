// Renders the PNG icons from the logo's one source (src/brand/logo.ts):
//
//   npx tsx scripts/make-icons.ts
//
// writes public/favicon.svg (the app icon's tile), public/apple-touch-icon.png
// (180 px, a full square: iOS rounds it) and public/favicon-32.png (the tile, for
// browsers without SVG favicons). Run again if the artwork changes. Chrome: $CHROME or the usual install path (chrome.ts).

import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import puppeteer from "puppeteer-core";
import { tileSvg } from "../src/brand/logo.ts";
import { findChrome } from "./chrome.ts";

const out = resolve(import.meta.dirname, "../public");
const icons: [string, number, number | undefined][] = [
  ["apple-touch-icon.png", 180, 0],
  ["favicon-32.png", 32, undefined],
];
writeFileSync(resolve(out, "favicon.svg"), tileSvg("", true) + "\n");
console.log("wrote public/favicon.svg");
const browser = await puppeteer.launch({ executablePath: findChrome(), headless: true });
try {
  const page = await browser.newPage();
  for (const [file, px, radius] of icons) {
    await page.setViewport({ width: px, height: px, deviceScaleFactor: 1 });
    const svg = tileSvg("", false, radius);
    await page.setContent(`<!doctype html><style>html,body{margin:0;background:transparent}svg{display:block;width:${px}px;height:${px}px}</style>${svg}`);
    const png = await page.screenshot({ omitBackground: true, clip: { x: 0, y: 0, width: px, height: px } });
    writeFileSync(resolve(out, file), png);
    console.log(`wrote public/${file}`);
  }
} finally {
  await browser.close();
}
