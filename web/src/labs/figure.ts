// What every docs figure shares: starting, loading its images
// (`public/media/labs/<id>/`, from `scripts/make-lab-assets.ts`), its
// explanations, and the parts its controls are built from.

import { mediaUrl } from "../media.ts";
import { PanView } from "./pan.ts";

/** A figure's `figure.json`: what the asset script wrote, plus fields of its kind. */
export interface FigureFiles {
  id: string;
  files: { file: string; bytes: number }[];
}

/** How a figure starts: on the stop or place a link to it names (`#<figure>/<choice>`), if one does. */
export type StartOpts = { choice?: string };

/** The address of a file in a figure's folder (public/media/labs/<id>/). */
export const labUrl = (id: string, file: string) => mediaUrl(`/media/labs/${id}/${file}`);

/** Frame `k` (from 1) of a figure's folder. */
export const frameFile = (k: number) => `frame-${String(k).padStart(2, "0")}.jpg`;

/** Start a figure: `aria-busy` until `build` shows it (the build clears it), or the failure in its place. */
export async function startFigure(figure: HTMLElement, build: () => Promise<void>): Promise<void> {
  figure.setAttribute("aria-busy", "true");
  try {
    await build();
  } catch (err) {
    showError(figure, err);
  }
}

export async function fetchFigure<T extends FigureFiles>(id: string, signal?: AbortSignal): Promise<T> {
  const res = await fetch(labUrl(id, "figure.json"), { signal });
  if (!res.ok) throw new Error(`the figure's data is missing (${res.status})`);
  return (await res.json()) as T;
}

/** Fetch and decode one image, with `crossOrigin = "anonymous"` so a canvas it is drawn on can be read. */
export async function loadImage(url: string): Promise<HTMLImageElement> {
  const img = new Image();
  img.crossOrigin = "anonymous";
  img.src = url;
  await img.decode().catch(() => {
    throw new Error(`an image of the figure did not load: ${url}`);
  });
  return img;
}

/** An `x,y` attribute as numbers; a missing or empty part is NaN, not 0. */
export const parsePoint = (s: string | undefined): number[] => (s ?? "").split(",").map((v) => (v.trim() === "" ? NaN : Number(v)));

/** Fetch and decode images with bounded concurrency, telling `onProgress` how many are ready. */
export async function loadImages(urls: string[], onProgress?: (done: number, total: number) => void, concurrency = 8): Promise<HTMLImageElement[]> {
  const results = new Array<HTMLImageElement>(urls.length);
  let next = 0;
  let done = 0;
  onProgress?.(0, urls.length);
  const worker = async () => {
    while (next < urls.length) {
      const i = next++;
      results[i] = await loadImage(urls[i]);
      onProgress?.(++done, urls.length);
    }
  };
  await Promise.all(Array.from({ length: Math.min(concurrency, urls.length) }, worker));
  return results;
}

/** Frames 1…`count` of a figure's folder, counted in `body` as they arrive. */
export async function loadFrames(body: HTMLElement, id: string, count: number): Promise<HTMLImageElement[]> {
  const status = document.createElement("p");
  status.className = "lab-status";
  body.append(status);
  const urls = Array.from({ length: count }, (_, i) => labUrl(id, frameFile(i + 1)));
  try {
    return await loadImages(urls, (done, total) => (status.textContent = `Loading the frames: ${done} of ${total}`));
  } finally {
    status.remove();
  }
}

/** Show the explanations for `key` (and those with no `data-for`), or all with `undefined`. */
export function showWhy(figure: HTMLElement, key: string | undefined): void {
  for (const why of figure.querySelectorAll<HTMLElement>(".lab-why")) {
    const forKey = why.dataset.for;
    why.hidden = key !== undefined && forKey !== undefined && forKey !== key;
  }
}

/** Replace the figure's body with a plain message, kept with the poster. */
export function showError(figure: HTMLElement, err: unknown): void {
  console.error(err);
  figure.removeAttribute("aria-busy");
  const p = document.createElement("p");
  p.className = "figure-error";
  p.textContent = `This figure could not load: ${err instanceof Error ? err.message : String(err)}`;
  const body = figure.querySelector(".lab-body");
  // A figure without a body (the pyramid diagram) shows the error in place of what it holds.
  if (body) body.append(p);
  else figure.replaceChildren(p);
}

/** A row of buttons, one pressed at a time. */
export function stops(labels: string[], label: string, onPick: (i: number) => void): { el: HTMLElement; set: (i: number) => void } {
  const el = document.createElement("div");
  el.className = "lab-stops";
  el.setAttribute("role", "group");
  el.setAttribute("aria-label", label);
  const buttons = labels.map((text, i) => {
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = text;
    b.addEventListener("click", () => onPick(i));
    el.append(b);
    return b;
  });
  const set = (i: number) => buttons.forEach((b, k) => b.setAttribute("aria-pressed", String(k === i)));
  return { el, set };
}

/** A control with its label before it. */
export function row(label: string, control: HTMLElement): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "lab-row";
  const span = document.createElement("span");
  span.className = "lab-row-label";
  span.textContent = label;
  wrap.append(span, control);
  return wrap;
}

let scrubIds = 0;

/** A slider through frames 1…`frames`, "Frame n of N" beside it; `onFrame` gets the frame from 0. */
export function frameScrub(frames: number, onFrame: (frame: number) => void): { el: HTMLElement; set: (frame: number) => void } {
  const el = document.createElement("div");
  el.className = "lab-scrub";
  const inputId = `lab-scrub-${++scrubIds}`;
  const label = document.createElement("label");
  label.htmlFor = inputId;
  const scrub = document.createElement("input");
  scrub.id = inputId;
  scrub.type = "range";
  scrub.min = "1";
  scrub.max = String(frames);
  scrub.step = "1";
  const out = document.createElement("output");
  out.setAttribute("for", inputId);
  label.append("Frame ", out, ` of ${frames}`);
  scrub.addEventListener("input", () => onFrame(Number(scrub.value) - 1));
  el.append(label, scrub);
  const set = (frame: number) => {
    scrub.value = String(frame + 1);
    out.textContent = String(frame + 1);
  };
  return { el, set };
}

/** A `w`×`h` canvas at 100% in a pan view (named `label`), over its caption (the label, unless given). */
export function panPanel(w: number, h: number, label: string, caption = label): { canvas: HTMLCanvasElement; view: PanView; wrap: HTMLElement; caption: HTMLElement } {
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const view = new PanView(canvas, w, h, label);
  const wrap = document.createElement("div");
  wrap.className = "lab-panel";
  const text = document.createElement("p");
  text.className = "lab-panel-label";
  text.textContent = caption;
  wrap.append(view.el, text);
  return { canvas, view, wrap, caption: text };
}
