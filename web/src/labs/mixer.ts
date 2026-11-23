// The level mixer (the pyramid page): a sharp frame A and a blurred frame B
// split into levels, and a result taking each level from one, computed live
// on the CPU. The switches form a pyramid, a row of A's and B's tiles per
// level, coarsest at the top.

import { fetchFigure, labUrl, loadImages, panPanel, parsePoint, showWhy, startFigure, stops, type FigureFiles } from "./figure.ts";
import type { Plane } from "./laplacian.ts";
import { DETAIL_LEVELS, mix, prepare, tile, type Mixer } from "./mixer-math.ts";
import { ALL_A, ALL_B, COARSE_FROM_A, FINE_FROM_A, initialMixer, nextMixer, type MixerEvent, type MixerState } from "./mixer-state.ts";

interface PairFiles extends FigureFiles {
  a: number;
  b: number;
  width: number;
  height: number;
}

/** The switches, finest first, as `mixer-math.ts` orders them. */
const LEVELS = ["Finest", "Level 2", "Level 3", "Level 4", "Coarsest"];
/** The patch every tile shows, in image pixels, and how much narrower each coarser row's tile is: the pyramid's shape. */
const TILE = { w: 120, h: 80, taper: 12 };
/** Detail is small and signed: the tiles show it this many times stronger, about mid-gray. */
const GAIN = 6;
const PRESETS: [string, boolean[]][] = [
  ["All from B", ALL_B],
  ["Fine from A, coarse from B", FINE_FROM_A],
  ["Coarse from A, fine from B", COARSE_FROM_A],
  ["All from A", ALL_A],
];

export function start(figure: HTMLElement): Promise<void> {
  return startFigure(figure, async () => {
    const id = figure.dataset.assets!;
    const data = await fetchFigure<PairFiles>(id);
    const [a, b] = await loadImages([labUrl(id, "a.jpg"), labUrl(id, "b.jpg")]);
    const t0 = performance.now();
    const mixer = prepare(plane(a), plane(b));
    console.debug(`${figure.id}: levels split in ${(performance.now() - t0).toFixed(0)} ms`);
    build(figure, data, mixer);
  });
}

function plane(img: HTMLImageElement): Plane {
  const canvas = document.createElement("canvas");
  canvas.width = img.naturalWidth;
  canvas.height = img.naturalHeight;
  const ctx = canvas.getContext("2d", { willReadFrequently: true })!;
  ctx.drawImage(img, 0, 0);
  const rgba = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
  const data = new Float32Array(canvas.width * canvas.height * 3);
  for (let i = 0, j = 0; i < rgba.length; i += 4, j += 3) {
    data[j] = rgba[i] / 255;
    data[j + 1] = rgba[i + 1] / 255;
    data[j + 2] = rgba[i + 2] / 255;
  }
  return { w: canvas.width, h: canvas.height, data };
}

/** A tile: what one frame holds at one level, over the patch. Detail levels are drawn about mid-gray. */
function tileCanvas(m: Mixer, level: number, fromA: boolean, x0: number, y0: number, w: number): HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = TILE.h;
  const values = tile(m, level, fromA, x0, y0, w, TILE.h);
  const detail = level < DETAIL_LEVELS;
  const ctx = canvas.getContext("2d")!;
  const pixels = ctx.createImageData(w, TILE.h);
  for (let p = 0, q = 0; p < values.length; p += 3, q += 4) {
    for (let c = 0; c < 3; c++) pixels.data[q + c] = (detail ? 0.5 + GAIN * values[p + c] : values[p + c]) * 255;
    pixels.data[q + 3] = 255;
  }
  ctx.putImageData(pixels, 0, 0);
  return canvas;
}

function build(figure: HTMLElement, data: PairFiles, mixer: Mixer) {
  const body = figure.querySelector<HTMLElement>(".lab-body")!;
  const { width: w, height: h } = data;
  let state: MixerState = initialMixer;
  // Where the tiles look and the result opens: `data-patch` (x,y), or the middle.
  const [px, py] = parsePoint(figure.dataset.patch);
  const cx = Number.isFinite(px) ? px : w / 2;
  const cy = Number.isFinite(py) ? py : h / 2;

  const { canvas, view, wrap: result, caption } = panPanel(w, h, "The result", "");
  const ctx = canvas.getContext("2d")!;
  const pixels = ctx.createImageData(w, h);

  // The pyramid of switches: a row per level, the coarsest at the top.
  const pyramid = document.createElement("div");
  pyramid.className = "mixer-pyramid";
  const head = document.createElement("div");
  head.className = "mixer-row mixer-head";
  head.append(document.createElement("span"), text("span", `A · frame ${data.a}\nin focus`), text("span", `B · frame ${data.b}\nout of focus`));
  pyramid.append(head);
  const rows = LEVELS.map((label, level) => {
    const tw = TILE.w - TILE.taper * level;
    const x0 = Math.round(Math.min(Math.max(cx - tw / 2, 0), w - tw));
    const y0 = Math.round(Math.min(Math.max(cy - TILE.h / 2, 0), h - TILE.h));
    const row = document.createElement("div");
    row.className = "mixer-row";
    row.setAttribute("role", "group");
    row.setAttribute("aria-label", `${label} level: take it from`);
    const buttons = [true, false].map((fromA) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "mixer-tile";
      button.setAttribute("aria-label", `${label} level from ${fromA ? "A" : "B"}`);
      button.append(tileCanvas(mixer, level, fromA, x0, y0, tw));
      button.addEventListener("click", () => dispatch({ kind: "take", level, fromA }));
      return button;
    });
    row.append(text("span", label), ...buttons);
    return { row, a: buttons[0], b: buttons[1] };
  });
  // The coarsest row first, so the rows widen downward like the pyramid diagram.
  pyramid.append(...rows.map((r) => r.row).reverse());
  // Release full-size `a` and `levelsB` planes now that the preview tiles are rendered.
  const mixSource = { b: mixer.b, differences: mixer.differences };
  mixer = undefined as unknown as Mixer;

  const presets = stops(
    PRESETS.map(([label]) => label),
    "Shortcuts",
    (i) => dispatch({ kind: "set", fromA: PRESETS[i][1] }),
  );
  presets.el.classList.add("mixer-presets");

  const top = document.createElement("div");
  top.className = "mixer-top";
  top.append(result, pyramid);
  body.replaceChildren(top, presets.el);
  figure.removeAttribute("aria-busy");
  view.centerOn(cx, cy);

  function dispatch(event: MixerEvent) {
    const before = state;
    state = nextMixer(state, event);
    if (state === before) return;
    render();
  }

  function render() {
    const { fromA } = state;
    const t0 = performance.now();
    mix(mixSource, fromA, pixels.data);
    ctx.putImageData(pixels, 0, 0);
    figure.dataset.mixMs = (performance.now() - t0).toFixed(1);
    rows.forEach((r, i) => {
      r.a.setAttribute("aria-pressed", String(fromA[i]));
      r.b.setAttribute("aria-pressed", String(!fromA[i]));
    });
    presets.set(PRESETS.findIndex(([, p]) => p.every((x, i) => x === fromA[i])));
    const names = LEVELS.filter((_, i) => fromA[i]).map((l) => l.toLowerCase());
    caption.textContent =
      names.length === 0 ? "The result: every level from B" : names.length === LEVELS.length ? "The result: every level from A" : `The result: ${names.join(", ")} from A, the others from B`;
    figure.dataset.state = state.kind;
    figure.dataset.mix = fromA.map((x) => (x ? "A" : "B")).join("");
    showWhy(figure, undefined);
  }

  render();
}

function text(tag: string, content: string): HTMLElement {
  const el = document.createElement(tag);
  el.textContent = content;
  return el;
}
