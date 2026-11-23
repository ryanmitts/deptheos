// The frames move (`#moves`, the alignment page): the frames as shot beside
// them aligned, with charts of the move and growth, and a toggle for the
// area the stack keeps. The transforms come from `figure.json`.
//
//   <figure class="lab lab-wide" id="moves" data-lab="moves" data-assets="moves">

import { drawMoved, invert, apply, motionAt, type Warp } from "./affine.ts";
import { fetchFigure, loadFrames, showWhy, startFigure, type FigureFiles } from "./figure.ts";
import { frameColor } from "./frames.ts";
import { initialMoves, nextMoves, type MovesEvent, type MovesState } from "./moves-state.ts";
import { autoplayWhenVisible, playControls, Timeline } from "./timeline.ts";

interface MovesFiles extends FigureFiles {
  frames: number;
  width: number;
  height: number;
  /** Frame 1 → each frame, pixel centers. */
  transforms: Warp[];
  /** The area every frame covers, [x, y, width, height] in frame 1's pixels. */
  covered: [number, number, number, number];
}

const SVG = "http://www.w3.org/2000/svg";

export function start(figure: HTMLElement): Promise<void> {
  const body = figure.querySelector<HTMLElement>(".lab-body")!;
  return startFigure(figure, async () => {
    const id = figure.dataset.assets!;
    const data = await fetchFigure<MovesFiles>(id);
    build(figure, body, data, await loadFrames(body, id, data.frames));
  });
}

function build(figure: HTMLElement, body: HTMLElement, data: MovesFiles, images: HTMLImageElement[]) {
  const { width: w, height: h, frames, transforms, covered } = data;
  let state: MovesState = initialMoves;

  const panel = (label: string) => {
    const canvas = document.createElement("canvas");
    canvas.width = w;
    canvas.height = h;
    canvas.className = "lab-fit";
    const wrap = document.createElement("div");
    wrap.className = "lab-panel";
    const caption = document.createElement("p");
    caption.className = "lab-panel-label";
    caption.textContent = label;
    wrap.append(canvas, caption);
    return { ctx: canvas.getContext("2d")!, wrap };
  };
  const shot = panel("As shot");
  const aligned = panel("Aligned to frame 1");
  const panels = document.createElement("div");
  panels.className = "lab-panels";
  panels.append(shot.wrap, aligned.wrap);

  // How the picture moved at the middle of the frame, and how much larger it shows.
  const motion = transforms.map((t) => motionAt(t, (w - 1) / 2, (h - 1) / 2));
  const shiftChart = chart(
    "How far the picture has moved since frame 1 (pixels)",
    [motion.map((m) => m.dx), motion.map((m) => m.dy)],
    ["across", "down"],
  );
  const sizeChart = chart("How much larger the picture looks than in frame 1 (%)", [motion.map((m) => (m.size - 1) * 100)], ["size"]);
  const charts = document.createElement("div");
  charts.className = "lab-panels";
  charts.append(shiftChart.el, sizeChart.el);

  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.className = "lab-toggle";
  toggle.textContent = "Show what the stack keeps";
  toggle.addEventListener("click", () => dispatch({ kind: "keeps", on: !state.keeps }));

  const timeline = new Timeline(frames, { fps: 8, holdMs: 1500 });
  const controls = document.createElement("div");
  controls.className = "lab-scrub";
  playControls(controls, timeline, (k) => `Frame ${k + 1} of ${frames}`);
  controls.append(toggle);
  body.replaceChildren(controls, panels, charts);
  figure.removeAttribute("aria-busy");

  timeline.onChange(() => render());
  autoplayWhenVisible(figure, timeline);

  function dispatch(event: MovesEvent) {
    const before = state;
    state = nextMoves(state, event);
    if (state === before) return;
    render();
  }

  function render() {
    const k = timeline.frame;
    shot.ctx.drawImage(images[k], 0, 0);

    const ctx = aligned.ctx;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.fillStyle = "#111";
    ctx.fillRect(0, 0, w, h);
    ctx.setTransform(...drawMoved(transforms[k]));
    ctx.drawImage(images[k], 0, 0);
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    if (state.keeps) drawKeeps(ctx, k);

    shiftChart.mark(k);
    sizeChart.mark(k);
    toggle.setAttribute("aria-pressed", String(state.keeps));
    figure.dataset.state = state.kind;
    showWhy(figure, undefined);
  }

  /** Dim what not every frame covers, outline each frame so far in its color, and dash the area kept. */
  function drawKeeps(ctx: CanvasRenderingContext2D, k: number) {
    const [x, y, cw, ch] = covered;
    ctx.beginPath();
    ctx.rect(0, 0, w, h);
    ctx.rect(x, y, cw, ch);
    ctx.fillStyle = "rgb(0 0 0 / 0.55)";
    ctx.fill("evenodd");
    ctx.lineWidth = 2;
    for (let j = 0; j <= k; j++) {
      // The frame's corners (pixel-center coordinates) in frame 1's place, back in corner coordinates.
      const back = invert(transforms[j]);
      const corners = [
        [-0.5, -0.5],
        [w - 0.5, -0.5],
        [w - 0.5, h - 0.5],
        [-0.5, h - 0.5],
      ].map(([cx, cy]) => apply(back, cx, cy).map((v) => v + 0.5));
      const [r, g, b] = frameColor(j, frames);
      ctx.strokeStyle = `rgb(${r} ${g} ${b} / ${j === k ? 1 : 0.5})`;
      ctx.beginPath();
      corners.forEach(([px, py], i) => (i ? ctx.lineTo(px, py) : ctx.moveTo(px, py)));
      ctx.closePath();
      ctx.stroke();
    }
    ctx.setLineDash([10, 6]);
    ctx.strokeStyle = "#fff";
    ctx.strokeRect(x, y, cw, ch);
    ctx.setLineDash([]);
  }

  render();
}

/** A small line chart over the frames, with a marker at the current one. */
function chart(title: string, series: number[][], names: string[]): { el: HTMLElement; mark: (k: number) => void } {
  const W = 400;
  const H = 130;
  const pad = { l: 34, r: 8, t: 8, b: 18 };
  const all = series.flat();
  let lo = Math.min(0, ...all);
  let hi = Math.max(0, ...all);
  if (hi - lo < 1e-6) hi = lo + 1;
  const n = series[0].length;
  const sx = (i: number) => pad.l + ((W - pad.l - pad.r) * i) / Math.max(n - 1, 1);
  const sy = (v: number) => pad.t + ((H - pad.t - pad.b) * (hi - v)) / (hi - lo);
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("viewBox", `0 0 ${W} ${H}`);
  svg.setAttribute("class", "move-chart");
  svg.setAttribute("role", "img");
  svg.setAttribute("aria-label", title);
  const nice = (v: number) => (Math.abs(v) >= 10 ? v.toFixed(0) : v.toFixed(1));
  let inner = `<line class="axis" x1="${pad.l}" x2="${W - pad.r}" y1="${sy(0)}" y2="${sy(0)}"/>`;
  inner += `<text class="tick" x="${pad.l - 4}" y="${sy(hi) + 4}" text-anchor="end">${nice(hi)}</text>`;
  inner += `<text class="tick" x="${pad.l - 4}" y="${sy(lo)}" text-anchor="end">${nice(lo)}</text>`;
  inner += `<text class="tick" x="${pad.l}" y="${H - 4}">frame 1</text><text class="tick" x="${W - pad.r}" y="${H - 4}" text-anchor="end">${n}</text>`;
  series.forEach((s, i) => {
    inner += `<polyline class="series s${i}" points="${s.map((v, j) => `${sx(j).toFixed(1)},${sy(v).toFixed(1)}`).join(" ")}"/>`;
  });
  svg.innerHTML = inner;
  const marker = document.createElementNS(SVG, "line");
  marker.setAttribute("class", "marker");
  marker.setAttribute("y1", String(pad.t));
  marker.setAttribute("y2", String(H - pad.b));
  svg.append(marker);
  const el = document.createElement("figure");
  el.className = "lab-chart";
  const caption = document.createElement("figcaption");
  caption.innerHTML = `${title}${names.length > 1 ? names.map((nm, i) => ` <span class="key s${i}">${nm}</span>`).join("") : ""}`;
  el.append(svg, caption);
  return {
    el,
    mark: (k) => {
      marker.setAttribute("x1", String(sx(k)));
      marker.setAttribute("x2", String(sx(k)));
    },
  };
}
