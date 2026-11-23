// Checks that no page scrolls sideways at phone widths. Needs `npm run dev` or `vite preview` running.
//
//   npx tsx scripts/overflow-check.ts http://localhost:5173 [width ...]

import puppeteer from "puppeteer-core";
import { pagePath, PAGES } from "../src/pages.ts";
import { findChrome } from "./chrome.ts";

const [base, ...widthArgs] = process.argv.slice(2);
if (!base) {
  console.error("usage: overflow-check <base url> [width ...]");
  process.exit(2);
}
const widths = widthArgs.length ? widthArgs.map(Number) : [390];
const browser = await puppeteer.launch({ executablePath: findChrome(), headless: true, acceptInsecureCerts: true });
let failed = 0;
try {
  const page = await browser.newPage();
  for (const width of widths) {
    await page.setViewport({ width, height: 800 });
    for (const path of Object.values(PAGES).map(pagePath)) {
      await page.goto(new URL(path, base).href, { waitUntil: "networkidle2" });
      const wide = await page.evaluate(() => document.documentElement.scrollWidth);
      if (wide > width) {
        failed++;
        console.log(`FAIL ${path} @ ${width}px: ${wide} px wide`);
      } else console.log(`ok   ${path} @ ${width}px`);
    }
  }
} finally {
  await browser.close();
}
process.exit(failed ? 1 : 0);
