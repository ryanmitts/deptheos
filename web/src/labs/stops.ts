// A figure that switches between pre-rendered images: a row of stops, an
// optional row of places, and one or two panels at 100% that pan together.
// Configured in the HTML:
//
//   <figure class="lab" id="pick" data-lab="stops" data-assets="pick"
//     data-stops="w1=1 px|w8=8 px|w64=64 px" data-stops-label="Compare frames over"
//     data-panels="{stop}.jpg=The picture|frames:{stop}-map.png=Frame map"
//     [data-places="hairs=Hairs|wing=Wing"] [data-places-label="Place"]
//     [data-center="520,250"] [data-layout="column"] [data-why-by="place"]>
//
// `{stop}` and `{place}` become the chosen keys. A `frames:` panel is a
// frame map in gray levels (0 = the first frame), drawn in the frame colors.
// `data-center` is where the panels open (image pixels; `|`-separated per
// place). `.lab-why[data-for]` blocks show for the chosen stop (or place,
// with `data-why-by="place"`); one without `data-for` always shows.

import { fetchFigure, labUrl, loadImage, panPanel, parsePoint, row, showWhy, startFigure, stops, type FigureFiles, type StartOpts } from "./figure.ts";
import { frameKey, frameMap } from "./frames.ts";
import { latest } from "../latest.ts";
import { PanView } from "./pan.ts";
import { nextStops, type StopsEvent, type StopsState } from "./stops-state.ts";

interface StopsFiles extends FigureFiles {
  /** Frames in the stack, for the frame colors. */
  frames?: number;
}

interface Choice {
  key: string;
  label: string;
}

const choices = (attr: string | undefined): Choice[] =>
  (attr ?? "")
    .split("|")
    .filter(Boolean)
    .map((part) => {
      const [key, label] = part.split("=");
      return { key, label: label ?? key };
    });

export function start(figure: HTMLElement, opts: StartOpts): Promise<void> {
  return startFigure(figure, async () => {
    const id = figure.dataset.assets!;
    const data = await fetchFigure<StopsFiles>(id);
    await build(figure, id, data, opts);
  });
}

async function build(figure: HTMLElement, id: string, data: StopsFiles, opts: StartOpts) {
  const body = figure.querySelector<HTMLElement>(".lab-body")!;
  const stopList = choices(figure.dataset.stops);
  const placeList = choices(figure.dataset.places);
  const places = placeList.length ? placeList : [{ key: "", label: "" }];
  const panels = choices(figure.dataset.panels).map(({ key, label }) => ({
    frames: key.startsWith("frames:"),
    template: key.replace(/^frames:/, ""),
    label,
  }));
  const centers = (figure.dataset.center ?? "").split("|").map((c) => parsePoint(c) as [number, number]);
  // A link's choice (`stop`, `place` or `place+stop`) wins over the figure's own start.
  const picked = (opts.choice ?? "").split("+");
  const pick = (list: { key: string }[], fallback?: string) => {
    const linked = list.findIndex((c) => picked.includes(c.key));
    return Math.max(0, linked >= 0 ? linked : list.findIndex((c) => c.key === fallback));
  };
  const start = pick(stopList, figure.dataset.start);
  const startPlace = pick(placeList);
  const file = (template: string, stop: number, place: number) =>
    labUrl(id, template.replace("{stop}", stopList[stop].key).replace("{place}", places[place].key));

  // Every panel image of a place, decoded once; frame maps colored once.
  const cache = new Map<string, Promise<CanvasImageSource>>();
  const image = (url: string, frames: boolean): Promise<CanvasImageSource> => {
    let p = cache.get(url);
    if (!p) {
      p = (frames ? frameMap(url, data.frames ?? 2) : loadImage(url)).catch((err) => {
        cache.delete(url);
        throw err;
      });
      cache.set(url, p);
    }
    return p;
  };
  const stopImages = (stop: number, place: number) =>
    Promise.all(panels.map((p) => image(file(p.template, stop, place), p.frames)));
  const prefetchPlace = (place: number) => {
    for (let s = 0; s < stopList.length; s++) void stopImages(s, place).catch(() => {});
  };

  const first = await stopImages(start, startPlace);
  prefetchPlace(startPlace);
  const size = (img: CanvasImageSource) => ({ w: (img as HTMLImageElement).width, h: (img as HTMLImageElement).height });
  const { w, h } = size(first[0]);

  let state: StopsState = { kind: "ready", stop: start, place: startPlace };
  const views = panels.map((p) => {
    const { canvas, view, wrap } = panPanel(w, h, p.label);
    if (p.frames) wrap.append(frameKey(data.frames ?? 2));
    return { view, wrap, ctx: canvas.getContext("2d")!, panel: p };
  });
  PanView.link(views.map((v) => v.view));

  const stopRow = stops(
    stopList.map((s) => s.label),
    figure.dataset.stopsLabel ?? "Setting",
    (stop) => dispatch({ kind: "stop", stop }),
  );
  const labeled = (text: string | undefined, control: HTMLElement) => (text ? row(text, control) : control);
  const rows: HTMLElement[] = [labeled(figure.dataset.stopsLabel, stopRow.el)];
  const placeRow = placeList.length
    ? stops(
        placeList.map((p) => p.label),
        figure.dataset.placesLabel ?? "Place",
        (place) => dispatch({ kind: "place", place }),
      )
    : undefined;
  if (placeRow) rows.unshift(labeled(figure.dataset.placesLabel, placeRow.el));
  const panelBox = document.createElement("div");
  panelBox.className = `lab-panels${figure.dataset.layout === "column" ? " column" : ""}`;
  panelBox.append(...views.map((v) => v.wrap));
  body.replaceChildren(...rows, panelBox);
  figure.removeAttribute("aria-busy");

  let shown: { stop: number; place: number } | undefined;
  const drawing = latest();

  function dispatch(event: StopsEvent) {
    const before = state;
    state = nextStops(state, event);
    if (state === before) return;
    void render();
  }

  async function render() {
    const { stop, place } = state;
    stopRow.set(stop);
    placeRow?.set(place);
    figure.dataset.state = state.kind;
    figure.dataset.stop = stopList[stop].key;
    const whyKey = figure.dataset.whyBy === "place" ? placeList[place]?.key : stopList[stop].key;
    showWhy(figure, whyKey);
    // Only the latest choice draws: a slow place's images must not land over a newer one.
    try {
      await drawing.run(
        () => stopImages(stop, place),
        (images) => {
          body.querySelector(".figure-error")?.remove();
          const newPlace = shown?.place !== place;
          views.forEach((v, i) => v.ctx.drawImage(images[i], 0, 0));
          if (newPlace) {
            const [cx, cy] = centers[placeList.length ? place : 0] ?? [];
            views[0].view.centerOn(Number.isFinite(cx) ? cx : w / 2, Number.isFinite(cy) ? cy : h / 2);
            prefetchPlace(place);
          }
          shown = { stop, place };
        },
      );
    } catch (err) {
      console.error(err);
      let note = body.querySelector<HTMLParagraphElement>(".figure-error");
      if (!note) {
        note = document.createElement("p");
        note.className = "figure-error";
        body.append(note);
      }
      note.textContent = `Could not load this view: ${err instanceof Error ? err.message : String(err)}`;
    }
  }

  void render();
}
