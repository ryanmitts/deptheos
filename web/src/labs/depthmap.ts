// The depth map (`#depth-map`): a frame beside the stack's depth map, which
// lights the parts at the frame shown; tapping the map jumps to its frame.
//
//   <figure class="lab" id="depth-map" data-lab="depthmap" data-assets="depthmap"
//     data-frames-from="sharpest" data-map="default-head-map.png" [data-center="x,y"]>
//
// The frames are another figure's (`data-frames-from`), so only the map loads anew.

import { fetchFigure, frameScrub, labUrl, loadFrames, panPanel, parsePoint, showWhy, startFigure, type FigureFiles } from "./figure.ts";
import { frameIndex, frameKey, frameLut, type FrameIndex } from "./frames.ts";
import { initialDepthMap, lit, nextDepthMap, type DepthMapEvent, type DepthMapState } from "./depthmap-state.ts";
import { PanView } from "./pan.ts";

interface FramesFiles extends FigureFiles {
  frames: number;
  width: number;
  height: number;
}

/** How far a pixel may move between press and release and still count as a tap. */
const TAP_SLOP = 5;

export function start(figure: HTMLElement): Promise<void> {
  const body = figure.querySelector<HTMLElement>(".lab-body")!;
  return startFigure(figure, async () => {
    const from = figure.dataset.framesFrom!;
    const [data, map] = await Promise.all([fetchFigure<FramesFiles>(from), frameIndex(labUrl(figure.dataset.assets!, figure.dataset.map!))]);
    if (map.width !== data.width || map.height !== data.height) throw new Error("the map and the frames differ in size");
    build(figure, body, data, await loadFrames(body, from, data.frames), map);
  });
}

function build(figure: HTMLElement, body: HTMLElement, data: FramesFiles, images: HTMLImageElement[], map: FrameIndex) {
  const { width: w, height: h, frames } = data;
  let state: DepthMapState = initialDepthMap;

  const panel = (label: string) => {
    const p = panPanel(w, h, label);
    return { ...p, ctx: p.canvas.getContext("2d")! };
  };
  const frame = panel("The frame");
  const depth = panel("The depth map");
  depth.canvas.classList.add("tappable");
  depth.wrap.append(frameKey(frames));
  PanView.link([frame.view, depth.view]);
  const pixels = depth.ctx.createImageData(w, h);
  const lut = frameLut(frames);
  const litR = new Uint8Array(256);
  const litG = new Uint8Array(256);
  const litB = new Uint8Array(256);
  const dimR = new Uint8Array(256);
  const dimG = new Uint8Array(256);
  const dimB = new Uint8Array(256);
  for (let i = 0; i < 256; i++) {
    const [r, g, b] = lut[i];
    litR[i] = r;
    litG[i] = g;
    litB[i] = b;
    dimR[i] = Math.round(28 + r * 0.22);
    dimG[i] = Math.round(28 + g * 0.22);
    dimB[i] = Math.round(28 + b * 0.22);
  }

  // A tap on the map jumps to its frame; the pan box captures a mouse drag, so listen there.
  let press: { id: number; x: number; y: number } | undefined;
  const box = depth.view.el;
  box.addEventListener("pointerdown", (e) => {
    if (!e.isPrimary || (e.pointerType === "mouse" && e.button !== 0)) return;
    press = { id: e.pointerId, x: e.clientX, y: e.clientY };
  });
  box.addEventListener("pointercancel", () => (press = undefined));
  box.addEventListener("pointerup", (e) => {
    if (!press || e.pointerId !== press.id) return;
    const tap = Math.hypot(e.clientX - press.x, e.clientY - press.y) <= TAP_SLOP;
    press = undefined;
    if (!tap) return;
    const boxRect = box.getBoundingClientRect();
    if (e.clientX >= boxRect.left + box.clientWidth || e.clientY >= boxRect.top + box.clientHeight) return;
    const at = depth.canvas.getBoundingClientRect();
    const x = Math.floor(e.clientX - at.left);
    const y = Math.floor(e.clientY - at.top);
    if (x < 0 || y < 0 || x >= w || y >= h) return;
    dispatch({ kind: "frame", frame: Math.min(map.index[y * w + x], frames - 1) });
  });

  const scrub = frameScrub(frames, (frame) => dispatch({ kind: "frame", frame }));

  const panels = document.createElement("div");
  panels.className = "lab-panels";
  panels.append(frame.wrap, depth.wrap);
  body.replaceChildren(scrub.el, panels);
  figure.removeAttribute("aria-busy");
  const [cx, cy] = parsePoint(figure.dataset.center);
  frame.view.centerOn(Number.isFinite(cx) ? cx : w / 2, Number.isFinite(cy) ? cy : h / 2);

  function dispatch(event: DepthMapEvent) {
    const before = state;
    state = nextDepthMap(state, event);
    if (state === before) return;
    render();
  }

  function render() {
    const f = state.frame;
    frame.ctx.drawImage(images[f], 0, 0);
    frame.caption.textContent = `Frame ${f + 1}`;
    // The map: the pixels whose depth is this frame (or a neighbor) in their color, the rest faint.
    const d = pixels.data;
    const idx = map.index;
    for (let i = 0, q = 0; i < idx.length; i++, q += 4) {
      const k = idx[i];
      if (lit(k, f)) {
        d[q] = litR[k];
        d[q + 1] = litG[k];
        d[q + 2] = litB[k];
      } else {
        d[q] = dimR[k];
        d[q + 1] = dimG[k];
        d[q + 2] = dimB[k];
      }
      d[q + 3] = 255;
    }
    depth.ctx.putImageData(pixels, 0, 0);
    depth.caption.textContent = `The depth map, lit where it chose frame ${f + 1} or a neighbor`;
    scrub.set(f);
    figure.dataset.state = state.kind;
    figure.dataset.frame = String(f + 1);
    showWhy(figure, undefined);
  }

  render();
}
