// Loads every page in headless Chrome and reports figures that failed,
// console errors and failed requests. Exits 1 if anything went wrong.
//
// The docs pages' figures (`[data-lab]`, src/labs/) need no GPU: each must
// load, answer its controls, and show what its data says (the depth-map
// figure: the frame its map holds where it is tapped); a link that names a
// stop or place (`#<figure>/<choice>`) must land on it.
//
//   npm run dev   (or: npm run build && npm run preview)
//   npm run smoke -- http://localhost:5173 [path ...]
//
// Paths default to every page of the site (src/pages.ts). CHROME overrides Chrome's path.

import puppeteer, { type ElementHandle, type Page } from "puppeteer-core";
import { pagePath, PAGES } from "../src/pages.ts";
import { findChrome } from "./chrome.ts";

const [base, ...given] = process.argv.slice(2);
if (!base) {
  console.error("usage: smoke <base url> [path ...]");
  process.exit(2);
}
const paths = given.length ? given : Object.values(PAGES).map(pagePath);

const browser = await puppeteer.launch({
  executablePath: findChrome(),
  headless: true,
  acceptInsecureCerts: true,
  args: ["--hide-scrollbars"],
});
let failed = false;
try {
  for (const path of paths) {
    const page = await browser.newPage();
    await page.setViewport({ width: 1280, height: 900 });
    const problems: string[] = [];
    page.on("console", (m) => {
      if (m.type() === "error") problems.push(`console: ${m.text()}`);
    });
    page.on("pageerror", (e) => problems.push(`page error: ${e}`));
    page.on("response", (r) => {
      if (r.status() >= 400) problems.push(`http ${r.status()}: ${r.url()}`);
    });
    const t0 = Date.now();
    await page.goto(new URL(path, base).href, { waitUntil: "networkidle0" });
    // Catch Astro whitespace stripping that fuses text with an adjacent inline element (e.g. "in2.0", "andthe").
    const fused = await page.evaluate(() => {
      const hits: string[] = [];
      const inlineTags = new Set(["A", "SPAN", "CODE", "EM", "STRONG"]);
      const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
      let node: Text | null;
      while ((node = walker.nextNode() as Text | null)) {
        const val = node.nodeValue ?? "";
        if (!val) continue;
        const next = node.nextSibling;
        if (next && next.nodeType === Node.ELEMENT_NODE) {
          const el = next as HTMLElement;
          if (inlineTags.has(el.tagName) && !el.classList.contains("sub") && !el.classList.contains("tag")) {
            const nextText = el.textContent ?? "";
            if (/[a-zA-Z0-9,]$/.test(val) && /^[a-zA-Z0-9]/.test(nextText)) {
              const style = getComputedStyle(el);
              if (style.display === "inline") {
                hits.push(`${val.slice(-12)}|${nextText.slice(0, 12)}`);
              }
            }
          }
        }
      }
      return hits;
    });
    for (const hit of fused) problems.push(`missing space before inline element: "${hit}"`);
    let summary = "";
    const labs = await page.$$eval("[data-lab]", (all) => all.map((el) => ({ name: (el as HTMLElement).dataset.lab!, id: el.id })));
    if (labs.length) {
      for (const lab of labs) {
        problems.push(...(await checkLab(page, lab.id, lab.name)));
        // A link to a stops figure can name a stop: it lands on that one.
        const last = await page.$eval(`#${lab.id}`, (el) => (el as HTMLElement).dataset.stops?.split("|").pop()?.split("=")[0]);
        if (lab.name === "stops" && last) {
          const linked = await browser.newPage();
          try {
            linked.on("console", (m) => {
              if (m.type() === "error") problems.push(`linked console: ${m.text()}`);
            });
            linked.on("pageerror", (e) => problems.push(`linked page error: ${e}`));
            linked.on("response", (r) => {
              if (r.status() >= 400) problems.push(`linked http ${r.status()}: ${r.url()}`);
            });
            await linked.goto(new URL(`${path}#${lab.id}/${encodeURIComponent(last)}`, base).href, { waitUntil: "networkidle0" });
            const state = await ready(linked, lab.id);
            const stop = await linked.$eval(`#${lab.id}`, (el) => (el as HTMLElement).dataset.stop);
            if (state !== "ready" || stop !== last) problems.push(`figure ${lab.name}: a link to #${lab.id}/${last} lands ${state} on ${stop}`);
          } finally {
            await linked.close();
          }
        }
      }
      summary = `${labs.length} figures: ${labs.map((l) => l.name).join(", ")}`;
    }
    const ok = problems.length === 0;
    failed ||= !ok;
    console.log(`${ok ? "ok  " : "FAIL"} ${path} (${((Date.now() - t0) / 1000).toFixed(1)} s) ${summary}`);
    for (const p of problems) console.log(`     ${p}`);
    await page.close();
  }
} finally {
  await browser.close();
}
process.exit(failed ? 1 : 0);

/** Scroll the figure near the screen and wait until it has loaded; its state, or why it has none. */
async function ready(page: Page, id: string): Promise<string> {
  // A figure inside a closed <details> builds when it is opened.
  await page.$eval(`#${id}`, (el) => {
    el.closest("details")?.setAttribute("open", "");
    el.scrollIntoView({ block: "center" });
  });
  try {
    await page.waitForFunction(
      (id) => {
        const el = document.getElementById(id)!;
        return el.querySelector(".figure-error") || el.dataset.state;
      },
      { timeout: 60000 },
      id,
    );
  } catch {
    return "never loaded";
  }
  return page.$eval(`#${id}`, (el) => el.querySelector(".figure-error")?.textContent ?? (el as HTMLElement).dataset.state!);
}

/** Click a control in the middle of the screen, clear of the sticky top bar. */
async function press(button: ElementHandle): Promise<void> {
  await button.evaluate((b) => b.scrollIntoView({ block: "center" }));
  await button.click();
}

/** Work one docs figure's controls and check what it shows against its data. */
async function checkLab(page: Page, id: string, name: string): Promise<string[]> {
  const problems: string[] = [];
  const state = await ready(page, id);
  if (state !== "ready") return [`figure ${name}: ${state}`];
  switch (name) {
    case "moves":
    case "diagram":
      // Shown from the start; nothing to press.
      break;
    case "mixer": {
      // A tile takes its level from its frame; a shortcut sets every level.
      const tile = await page.$(`#${id} .mixer-row:last-child .mixer-tile`);
      if (!tile) {
        problems.push(`figure ${name}: missing mixer tile`);
        break;
      }
      await press(tile);
      const one = await page.$eval(`#${id}`, (el) => (el as HTMLElement).dataset.mix);
      if (one !== "ABBBB") problems.push(`figure ${name}: the finest level's A tile left the mix ${one}`);
      const preset = await page.$(`#${id} .mixer-presets button:last-child`);
      if (!preset) {
        problems.push(`figure ${name}: missing mixer preset button`);
        break;
      }
      await press(preset);
      const all = await page.$eval(`#${id}`, (el) => (el as HTMLElement).dataset.mix);
      if (all !== "AAAAA") problems.push(`figure ${name}: "All from A" left the mix ${all}`);
      break;
    }
    case "depthmap": {
      // The slider moves the frame; tapping the map jumps to the frame the map holds there.
      await page.$eval(`#${id} input[type=range]`, (input) => {
        (input as HTMLInputElement).value = "25";
        input.dispatchEvent(new Event("input", { bubbles: true }));
      });
      const now = await page.$eval(`#${id}`, (el) => (el as HTMLElement).dataset.frame);
      if (now !== "25") problems.push(`figure ${name}: the slider at 25 shows frame ${now}`);
      const at = [700, 600];
      const expected = await page.evaluate(async (at) => {
        const fig = document.querySelector<HTMLElement>("[data-lab=depthmap]");
        if (!fig?.dataset.map) return null;
        // The map's address as the page loaded it: under /media/, or in the bucket on the deployed site.
        const entry = performance.getEntriesByType("resource").find((e) => e.name.endsWith(`/${fig.dataset.map}`));
        if (!entry) return null;
        const bitmap = await createImageBitmap(await (await fetch(entry.name)).blob(), { colorSpaceConversion: "none" });
        const ctx = new OffscreenCanvas(bitmap.width, bitmap.height).getContext("2d")!;
        ctx.drawImage(bitmap, 0, 0);
        bitmap.close();
        return ctx.getImageData(at[0], at[1], 1, 1).data[0] + 1;
      }, at);
      if (expected === null) {
        problems.push(`figure ${name}: depth map resource not found`);
        break;
      }
      const point = await page.$eval(
        `#${id} canvas.tappable`,
        (canvas, at) => {
          const box = canvas.parentElement!;
          box.scrollIntoView({ block: "center" });
          box.scrollTo(at[0] - box.clientWidth / 2, at[1] - box.clientHeight / 2);
          const r = canvas.getBoundingClientRect();
          return { x: r.left + at[0] + 0.5, y: r.top + at[1] + 0.5 };
        },
        at,
      );
      await new Promise((r) => setTimeout(r, 200));
      await page.mouse.click(point.x, point.y);
      const shown = await page.$eval(`#${id}`, (el) => (el as HTMLElement).dataset.frame);
      if (shown !== String(expected)) problems.push(`figure ${name}: tapping the map at ${at} shows frame ${shown}, the map says ${expected}`);
      break;
    }
    default: {
      // A stops figure: every stop can be chosen.
      for (const button of await page.$$(`#${id} .lab-row:last-of-type .lab-stops button, #${id} .lab-body > .lab-stops button`)) {
        await press(button);
        const pressed = await button.evaluate((b) => b.getAttribute("aria-pressed"));
        if (pressed !== "true") problems.push(`figure ${name} (#${id}): a stop did not take`);
      }
    }
  }
  return problems;
}
