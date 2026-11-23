// The pyramid article's diagram: one frame's Gaussian and detail
// (Laplacian) levels as tilted planes, computed in plain JavaScript with
// the app's 5-tap blur. A picture of the idea; the figures below show the
// app's own results. Started by the labs loader (`data-lab="diagram"`).

import { mediaUrl } from "../media.ts";
import { startFigure } from "./figure.ts";
import { clampi, down, sub, up, type Plane } from "./laplacian.ts";

const FRAME = mediaUrl("/media/labs/diagram/spider-23.jpg");
/** Crop center (fractions of the frame), size in diagram pixels, and frame pixels per diagram pixel: the whole spider. */
const CROP = { fx: 0.5, fy: 0.5, w: 384, h: 256, span: 2 };
const LEVELS = 4;
/** A plane as a PNG data URL; `detail` shows signed values around mid-gray, `gain` times stronger. */
function toUrl(p: Plane, detail = false, gain = 4): string {
  const canvas = document.createElement("canvas");
  canvas.width = p.w;
  canvas.height = p.h;
  const ctx = canvas.getContext("2d")!;
  const img = ctx.createImageData(p.w, p.h);
  for (let k = 0; k < p.w * p.h; k++) {
    for (let c = 0; c < 3; c++) {
      const v = p.data[k * 3 + c];
      img.data[k * 4 + c] = Math.round(255 * clampi(detail ? 0.5 + gain * v : v, 0, 1));
    }
    img.data[k * 4 + 3] = 255;
  }
  ctx.putImageData(img, 0, 0);
  return canvas.toDataURL();
}

async function loadCrop(): Promise<Plane> {
  const img = new Image();
  img.crossOrigin = "anonymous"; // read back below, and the media may be on another origin
  img.src = FRAME;
  await img.decode();
  const canvas = document.createElement("canvas");
  canvas.width = CROP.w;
  canvas.height = CROP.h;
  const ctx = canvas.getContext("2d", { willReadFrequently: true })!;
  const [sw, sh] = [CROP.w * CROP.span, CROP.h * CROP.span];
  const sx = Math.round(CROP.fx * img.naturalWidth - sw / 2);
  const sy = Math.round(CROP.fy * img.naturalHeight - sh / 2);
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(img, sx, sy, sw, sh, 0, 0, CROP.w, CROP.h);
  const rgba = ctx.getImageData(0, 0, CROP.w, CROP.h).data;
  const data = new Float32Array(CROP.w * CROP.h * 3);
  for (let k = 0; k < CROP.w * CROP.h; k++) for (let c = 0; c < 3; c++) data[k * 3 + c] = rgba[k * 4 + c] / 255;
  return { w: CROP.w, h: CROP.h, data };
}

const SVG = "http://www.w3.org/2000/svg";
function el<K extends keyof SVGElementTagNameMap>(name: K, attrs: Record<string, string | number>, text?: string) {
  const e = document.createElementNS(SVG, name);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  if (text !== undefined) e.textContent = text;
  return e;
}

// Isometric: the image's x axis runs down-right, its y axis down-left.
const ISO_X = [0.866, 0.5];
const ISO_Y = [-0.866, 0.5];

/** A tilted plane of `w`×`h` units centered at (cx, cy): its image and outline. */
function plane(url: string, w: number, h: number, cx: number, cy: number): SVGGElement {
  const e = cx - (ISO_X[0] * w + ISO_Y[0] * h) / 2;
  const f = cy - (ISO_X[1] * w + ISO_Y[1] * h) / 2;
  const g = el("g", { transform: `matrix(${ISO_X[0]} ${ISO_X[1]} ${ISO_Y[0]} ${ISO_Y[1]} ${e} ${f})` });
  const image = el("image", { href: url, width: w, height: h, preserveAspectRatio: "none" });
  image.style.imageRendering = "pixelated";
  g.append(image, el("rect", { width: w, height: h, class: "pd-edge", "vector-effect": "non-scaling-stroke" }));
  return g;
}

function arrow(x1: number, y1: number, x2: number, y2: number, cls = "pd-arrow"): SVGPathElement {
  return el("path", { d: `M${x1} ${y1} L${x2} ${y2}`, class: cls, "marker-end": "url(#pd-head)" });
}

/** The levels as image URLs, computed once: the Gaussian levels and their detail. */
interface Levels {
  sizes: [number, number][];
  gaussian: string[];
  detail: string[];
}

async function levels(): Promise<Levels> {
  const g: Plane[] = [await loadCrop()];
  for (let k = 1; k < LEVELS; k++) g.push(down(g[k - 1]));
  // Detail level k: level k minus level k+1 scaled back up. The last level is kept as it is.
  const detail = g.map((p, k) => (k === LEVELS - 1 ? p : sub(p, up(g[k + 1], p.w, p.h))));
  return {
    sizes: g.map((p) => [p.w, p.h]),
    gaussian: g.map((p) => toUrl(p)),
    detail: detail.map((p, k) => toUrl(p, k < LEVELS - 1)),
  };
}

/** Narrower than this (CSS pixels), the two columns are drawn one above the other. */
const NARROW = 600;

/** The Gaussian and detail levels side by side, or, `narrow`, one above the other to fit a phone. */
function diagram(lv: Levels, narrow: boolean): SVGSVGElement {
  // SVG units: level 0 is 180 wide, each level up half that, spaced so the outlines don't overlap.
  const W0 = 180;
  const H0 = (W0 * CROP.h) / CROP.w;
  const colG = narrow ? 250 : 230;
  const colL = narrow ? 250 : 640;
  // Narrow: the detail section starts this far below the Gaussian one.
  const dyL = narrow ? 490 : 0;
  const width = narrow ? 440 : 930;
  const height = narrow ? 2 * 490 + 10 : 500;
  const size = (k: number) => [W0 / 2 ** k, H0 / 2 ** k];
  const centers = [420];
  for (let k = 1; k < LEVELS; k++) {
    const [w, h] = size(k - 1);
    centers.push(centers[k - 1] - (0.375 * (w + h) + 36));
  }
  const cy = (k: number) => centers[k];
  const svg = el("svg", {
    viewBox: `0 0 ${width} ${height}`,
    class: "pd-svg",
    role: "img",
    "aria-label":
      `A Laplacian pyramid of one frame of the spider, ${LEVELS} levels. First: the frame and ever smaller, blurrier copies of it. ` +
      "Then: each level's detail, the level minus the next smaller one scaled back up; the smallest is kept as it is.",
  });
  const defs = el("defs", {});
  const marker = el("marker", { id: "pd-head", viewBox: "0 0 10 10", refX: 9, refY: 5, markerWidth: 7, markerHeight: 7, orient: "auto-start-reverse" });
  marker.append(el("path", { d: "M0 0 L10 5 L0 10 z", class: "pd-head" }));
  defs.append(marker);
  svg.append(defs);

  const tG = narrow ? width / 2 : colG;
  const tL = narrow ? width / 2 : colL;
  svg.append(el("text", { x: tG, y: 26, class: "pd-title", "text-anchor": "middle" }, "Gaussian pyramid"));
  svg.append(el("text", { x: tG, y: 44, class: "pd-sub", "text-anchor": "middle" }, "the frame, blurred and halved again and again"));
  svg.append(el("text", { x: tL, y: dyL + 26, class: "pd-title", "text-anchor": "middle" }, "Detail levels"));
  if (narrow) {
    // No arrows across in this layout: the subtitle says how each detail level is made.
    const sub = el("text", { x: tL, y: dyL + 44, class: "pd-sub", "text-anchor": "middle" });
    for (const [i, line] of ["each level minus the next one scaled up", "(mid-gray is zero); the smallest kept as it is"].entries()) {
      sub.append(el("tspan", { x: tL, dy: i === 0 ? 0 : 15 }, line));
    }
    svg.append(sub);
  } else {
    svg.append(el("text", { x: tL, y: 44, class: "pd-sub", "text-anchor": "middle" }, "what each halving removed (mid-gray is zero)"));
  }

  // Draw the largest first, so the smaller levels sit on top of it.
  for (let k = 0; k < LEVELS; k++) {
    const [w, h] = size(k);
    const [pw, ph] = lv.sizes[k];
    svg.append(plane(lv.gaussian[k], w, h, colG, cy(k)));
    const last = k === LEVELS - 1;
    svg.append(plane(lv.detail[k], w, h, colL, cy(k) + dyL));
    const half = (ISO_X[0] * (w + h)) / 2;
    svg.append(
      el("text", { x: colG - half - 10, y: cy(k) + 4, class: "pd-label", "text-anchor": "end" }, `level ${k}`),
      el("text", { x: colG - half - 10, y: cy(k) + 20, class: "pd-sub", "text-anchor": "end" }, `${pw}×${ph}`),
    );
    if (narrow) {
      svg.append(el("text", { x: colL - half - 10, y: cy(k) + dyL + 4, class: "pd-label", "text-anchor": "end" }, `level ${k}`));
    } else {
      // Across: subtract the next level scaled up (or keep the smallest as it is).
      const x1 = colG + half + 12;
      const x2 = colL - half - 12;
      svg.append(arrow(x1, cy(k), x2, cy(k)));
      svg.append(
        el(
          "text",
          { x: (x1 + x2) / 2, y: cy(k) - 8, class: "pd-op", "text-anchor": "middle" },
          last ? "kept as it is" : `minus level ${k + 1} scaled up`,
        ),
      );
    }
    // Up the Gaussian levels: blur and halve.
    if (!last) {
      const [w1, h1] = size(k + 1);
      const y1 = cy(k) - 0.25 * (w + h) - 4;
      const y2 = cy(k + 1) + 0.25 * (w1 + h1) + 4;
      svg.append(arrow(colG, y1, colG, y2, "pd-arrow pd-up"));
      svg.append(el("text", { x: colG + 8, y: (y1 + y2) / 2 + 4, class: "pd-op" }, "blur + halve"));
    }
  }
  const xr = colL + (ISO_X[0] * (W0 + H0)) / 2 + (narrow ? 16 : 24);
  svg.append(arrow(xr, cy(LEVELS - 1) + dyL - 10, xr, cy(0) + dyL + 10, "pd-arrow pd-rebuild"));
  const steps = ["rebuild: scale up,", "add the next", "detail level,", "repeat"];
  if (narrow) {
    // Little room beside the arrow: the steps in one line along it.
    const mid = (cy(LEVELS - 1) + cy(0)) / 2 + dyL;
    svg.append(
      el("text", { x: 0, y: 0, class: "pd-op", "text-anchor": "middle", transform: `translate(${xr + 14} ${mid}) rotate(90)` }, "rebuild: scale up, add the detail, repeat"),
    );
  } else {
    const tx = xr + 8;
    const ty = (cy(LEVELS - 1) + cy(0)) / 2;
    const rebuild = el("text", { x: tx, y: ty - 18, class: "pd-op" });
    for (const [i, line] of steps.entries()) rebuild.append(el("tspan", { x: tx, dy: i === 0 ? 0 : 16 }, line));
    svg.append(rebuild);
  }
  return svg;
}

export function start(figure: HTMLElement): Promise<void> {
  return startFigure(figure, async () => {
    const lv = await levels();
    let narrow: boolean | undefined;
    // The page's room for the figure, not the figure's own width.
    const room = figure.parentElement ?? figure;
    const fit = () => {
      const now = room.clientWidth < NARROW;
      if (now === narrow) return;
      narrow = now;
      figure.classList.toggle("pd-narrow", now);
      figure.replaceChildren(diagram(lv, now));
    };
    fit();
    new ResizeObserver(fit).observe(room);
    figure.removeAttribute("aria-busy");
    // Drawn: nothing in it changes but its layout.
    figure.dataset.state = "ready";
  });
}
