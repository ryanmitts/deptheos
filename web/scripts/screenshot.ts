// Screenshots of a page under headless Chrome, for checking the pages
// without a person. Needs `npm run dev` running.
//
//   npm run shot -- <url> <out.png> [options]
//     --click <selector>  click it after loading (repeatable, in order)
//     --set <selector>=<value>  set an input's value and fire "input" (repeatable, after the clicks)
//     --wait <ms>         then wait this long (default 300)
//     --scroll <selector> scroll it into view before the shot
//     --hover <selector>@<fx>,<fy>  then move the pointer to that point of the element (fractions)
//     --el <selector>     shoot only this element (default: the viewport)
//     --width <px>        viewport width (default 1280)
//     --full              the full page instead of the viewport
//     --load-all          first scroll down the whole page and back to the top, so lazy images and figures load
//     --dark              prefer the dark color scheme
//     --light             prefer the light color scheme (headless Chrome follows the system otherwise)
//
// Chrome: $CHROME, or the usual install path on Windows, macOS or Linux (chrome.ts).

import puppeteer from "puppeteer-core";
import { findChrome } from "./chrome.ts";

const args = process.argv.slice(2);
const [url, out] = args;
if (!url || !out) {
  console.error("usage: shot <url> <out.png> [--click sel]... [--wait ms] [--scroll sel] [--el sel] [--width px] [--full]");
  process.exit(2);
}
const opt = (name: string) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : undefined;
};
const all = (name: string) => args.flatMap((a, i) => (a === `--${name}` ? [args[i + 1]] : []));

const browser = await puppeteer.launch({
  executablePath: findChrome(),
  headless: true,
  acceptInsecureCerts: true,
  args: ["--hide-scrollbars"],
});
try {
  const page = await browser.newPage();
  await page.setViewport({ width: Number(opt("width") ?? 1280), height: 900, deviceScaleFactor: 1 });
  page.on("console", (m) => {
    if (m.type() === "error" || m.type() === "warn") console.log(`[page ${m.type()}] ${m.text()}`);
  });
  page.on("pageerror", (e) => console.log(`[page error] ${e}`));
  page.on("response", (r) => {
    if (r.status() >= 400) console.log(`[http ${r.status()}] ${r.url()}`);
  });
  if (args.includes("--dark")) await page.emulateMediaFeatures([{ name: "prefers-color-scheme", value: "dark" }]);
  if (args.includes("--light")) await page.emulateMediaFeatures([{ name: "prefers-color-scheme", value: "light" }]);
  await page.goto(url, { waitUntil: "networkidle0" });
  for (const sel of all("click")) await page.click(sel);
  for (const pair of all("set")) {
    const i = pair.lastIndexOf("=");
    await page.$eval(
      pair.slice(0, i),
      (el, value) => {
        (el as HTMLInputElement).value = value;
        el.dispatchEvent(new Event("input", { bubbles: true }));
      },
      pair.slice(i + 1),
    );
  }
  if (args.includes("--load-all")) {
    const height = await page.evaluate(() => document.documentElement.scrollHeight);
    for (let y = 0; y < height; y += 600) {
      await page.evaluate((top) => window.scrollTo(0, top), y);
      await new Promise((r) => setTimeout(r, 120));
    }
    await page.evaluate(() => window.scrollTo(0, 0));
  }
  const scroll = opt("scroll");
  if (scroll) await page.$eval(scroll, (e) => e.scrollIntoView({ block: "start" }));
  await new Promise((r) => setTimeout(r, Number(opt("wait") ?? 300)));
  const hover = opt("hover");
  if (hover) {
    const [sel, at] = hover.split("@");
    const [fx, fy] = at.split(",").map(Number);
    const target = await page.$(sel);
    if (!target) throw new Error(`hover element not found: ${sel}`);
    const box = await target.boundingBox();
    if (!box) throw new Error(`hover element has no bounding box: ${sel}`);
    await page.mouse.move(box.x + box.width * fx, box.y + box.height * fy);
    await new Promise((r) => setTimeout(r, 300));
  }
  const el = opt("el");
  if (el) {
    const target = await page.$(el);
    if (!target) throw new Error(`element not found: ${el}`);
    await target.screenshot({ path: out as `${string}.png` });
  } else await page.screenshot({ path: out as `${string}.png`, fullPage: args.includes("--full") });
  console.log("wrote", out);
} finally {
  await browser.close();
}
