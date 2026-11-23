// At phone size, a swipe over a figure's panel pans the picture inside it,
// and a swipe over the text scrolls the page. The panels pan with the
// browser's own scrolling (`src/labs/pan.ts`), so this checks no handler
// swallows the gesture.
//
//   npm run dev
//   npx tsx scripts/touch-check.ts http://localhost:5173 [/docs/pyramid/ [#mixer]]

import puppeteer from "puppeteer-core";
import { findChrome } from "./chrome.ts";

const [base, path = "/docs/pyramid/", figure = "#mixer"] = process.argv.slice(2);
if (!base) throw new Error("usage: touch-check <base url> [path] [figure]");
const browser = await puppeteer.launch({ executablePath: findChrome(), headless: true, acceptInsecureCerts: true });
let ok = true;
try {
  const page = await browser.newPage();
  await page.emulate({ viewport: { width: 390, height: 844, isMobile: true, hasTouch: true, deviceScaleFactor: 2 }, userAgent: "touch-check" });
  await page.goto(new URL(path + figure, base).href, { waitUntil: "networkidle0" });
  await page.waitForFunction((sel) => document.querySelector<HTMLElement>(sel)?.dataset.state !== undefined, { timeout: 60000 }, figure);
  const pan = await page.$(`${figure} .pan`);
  if (!pan) throw new Error(`no ${figure} .pan element found`);
  const cdp = await page.createCDPSession();

  /** Swipe 300 px from (x, y), up or to the left. */
  const swipe = async (x: number, y: number, left = false) => {
    await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x, y }] });
    for (let i = 1; i <= 15; i++) {
      const d = i * 20;
      await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [left ? { x: x - d, y } : { x, y: y - d }] });
      await new Promise((r) => setTimeout(r, 16));
    }
    await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
    await new Promise((r) => setTimeout(r, 500));
  };

  // Over the panel: the picture moves, along whichever way it overflows.
  const left = await pan.evaluate((p) => {
    p.scrollIntoView({ block: "center" });
    p.scrollTo(0, 0);
    return p.scrollHeight <= p.clientHeight;
  });
  await new Promise((r) => setTimeout(r, 300));
  const box = await pan.boundingBox();
  if (!box) throw new Error(`no bounding box for ${figure} .pan`);
  await swipe(left ? box.x + box.width - 20 : box.x + box.width / 2, left ? box.y + box.height / 2 : box.y + box.height - 20, left);
  const panned = await pan.evaluate((p) => p.scrollLeft + p.scrollTop);
  console.log(`over the panel: the picture panned ${panned} px (should be > 50)`);
  if (panned <= 50) ok = false;

  // Over the text: the page moves.
  const prose = await page.$(".prose p");
  if (!prose) throw new Error("no .prose p element found");
  await prose.evaluate((p) => p.scrollIntoView({ block: "center" }));
  await new Promise((r) => setTimeout(r, 300));
  const text = await prose.boundingBox();
  if (!text) throw new Error("no bounding box for .prose p");
  const before = await page.evaluate(() => scrollY);
  await swipe(text.x + text.width / 2, Math.min(800, text.y + text.height / 2 + 150));
  const scrolled = (await page.evaluate(() => scrollY)) - before;
  console.log(`over the text: the page scrolled ${scrolled} px (should be > 100)`);
  if (scrolled <= 100) ok = false;
} finally {
  await browser.close();
}
console.log(ok ? "PASS" : "FAIL");
process.exit(ok ? 0 : 1);
