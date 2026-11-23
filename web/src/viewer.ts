// The full-resolution viewer of the home page's hero and the gallery pages.

import { dragToPan } from "./labs/pan.ts";

// The stack at 100%, as lazily loaded 1024 px tiles over the hero image scaled up. The zoom
// button (`.hero-zoom`) opens it on the middle; a click on the picture, on the point clicked.
//
// The host element: `data-viewer data-size="<width>x<height>" data-tiles="<folder>"`, holding the `<img>`
// (make the tiles with scripts/make-gallery.sh).
export function attachViewer(hero: HTMLElement): void {
  const [width, height] = hero.dataset.size!.split("x").map(Number);
  const tiles = hero.dataset.tiles!;
  const TILE = 1024;
  const imgEl = hero.querySelector("img")!;
  const thumbUrl = () => imgEl.currentSrc || imgEl.src;

  const viewer = document.createElement("dialog");
  viewer.className = "viewer";
  viewer.tabIndex = -1;
  viewer.setAttribute("aria-label", "The picture at full resolution");
  const plane = document.createElement("div");
  plane.className = "viewer-plane";
  plane.style.width = `${width}px`;
  plane.style.height = `${height}px`;
  plane.style.backgroundImage = `url("${thumbUrl()}")`;
  viewer.append(plane);

  const bar = document.createElement("div");
  bar.className = "viewer-bar";
  bar.innerHTML = `<span>100% · drag to look around · Esc to close</span><button type="button">Close</button>`;
  viewer.append(bar);

  const mapW = 132;
  const mapH = Math.round((mapW * height) / width);
  const minimap = document.createElement("div");
  minimap.className = "viewer-minimap";
  minimap.style.width = `${mapW}px`;
  minimap.style.height = `${mapH}px`;
  minimap.style.backgroundImage = `url("${thumbUrl()}")`;
  minimap.setAttribute("aria-hidden", "true");
  const mapBox = document.createElement("div");
  mapBox.className = "viewer-minimap-box";
  minimap.append(mapBox);
  viewer.append(minimap);

  document.body.append(viewer);

  let built = false;
  const build = () => {
    for (let r = 0; r * TILE < height; r++) {
      for (let c = 0; c * TILE < width; c++) {
        const img = document.createElement("img");
        img.loading = "lazy";
        img.decoding = "async";
        img.alt = "";
        img.src = `${tiles}/${r}-${c}.jpg`;
        img.style.left = `${c * TILE}px`;
        img.style.top = `${r * TILE}px`;
        img.width = Math.min(TILE, width - c * TILE);
        img.height = Math.min(TILE, height - r * TILE);
        plane.append(img);
      }
    }
    built = true;
  };

  const updateMinimap = () => {
    if (!viewer.open) return;
    const left = Math.max(0, Math.min(1, viewer.scrollLeft / width));
    const top = Math.max(0, Math.min(1, viewer.scrollTop / height));
    const w = Math.min(1 - left, viewer.clientWidth / width);
    const h = Math.min(1 - top, viewer.clientHeight / height);
    mapBox.style.left = `${left * 100}%`;
    mapBox.style.top = `${top * 100}%`;
    mapBox.style.width = `${w * 100}%`;
    mapBox.style.height = `${h * 100}%`;
  };
  viewer.addEventListener("scroll", updateMinimap, { passive: true });
  window.addEventListener("resize", updateMinimap, { passive: true });

  // Clicking or dragging the minimap jumps straight to that part of the picture.
  let mapDragging = false;
  const jumpFromMinimap = (e: PointerEvent) => {
    const r = minimap.getBoundingClientRect();
    const fx = Math.max(0, Math.min(1, (e.clientX - r.left) / r.width));
    const fy = Math.max(0, Math.min(1, (e.clientY - r.top) / r.height));
    viewer.scrollLeft = fx * width - viewer.clientWidth / 2;
    viewer.scrollTop = fy * height - viewer.clientHeight / 2;
  };
  minimap.addEventListener("pointerdown", (e) => {
    if (e.pointerType === "mouse" && e.button !== 0) return;
    e.stopPropagation();
    mapDragging = true;
    minimap.setPointerCapture(e.pointerId);
    jumpFromMinimap(e);
    e.preventDefault();
  });
  minimap.addEventListener("pointermove", (e) => {
    if (mapDragging) jumpFromMinimap(e);
  });
  const stopMapDrag = () => (mapDragging = false);
  minimap.addEventListener("pointerup", stopMapDrag);
  minimap.addEventListener("pointercancel", stopMapDrag);
  minimap.addEventListener("lostpointercapture", stopMapDrag);

  const open = (fx: number, fy: number) => {
    const src = `url("${thumbUrl()}")`;
    plane.style.backgroundImage = src;
    minimap.style.backgroundImage = src;
    if (!built) build();
    viewer.showModal();
    bar.classList.add("open");
    minimap.classList.add("open");
    document.documentElement.style.overflow = "hidden";
    viewer.scrollLeft = fx * width - viewer.clientWidth / 2;
    viewer.scrollTop = fy * height - viewer.clientHeight / 2;
    updateMinimap();
    viewer.focus();
  };
  const close = () => viewer.close();
  const zoom = hero.querySelector<HTMLButtonElement>(".hero-zoom");
  viewer.addEventListener("close", () => {
    bar.classList.remove("open");
    minimap.classList.remove("open");
    document.documentElement.style.overflow = "";
    (zoom ?? hero).focus();
  });

  zoom?.addEventListener("click", (e) => {
    e.stopPropagation();
    open(0.5, 0.5);
  });
  hero.addEventListener("click", (e) => {
    if (e.target instanceof Element && e.target.closest("button, a")) return;
    const r = hero.getBoundingClientRect();
    // The hero crops the picture (object-fit: cover), so map the click back to the whole.
    const scale = Math.max(r.width / width, r.height / height);
    const shownW = width * scale, shownH = height * scale;
    const [px, py] = getComputedStyle(imgEl).objectPosition.split(" ").map((v) => parseFloat(v) / 100);
    const offX = (r.width - shownW) * px, offY = (r.height - shownH) * py;
    open((e.clientX - r.left - offX) / shownW, (e.clientY - r.top - offY) / shownH);
  });
  bar.querySelector("button")!.addEventListener("click", close);

  dragToPan(viewer);
}
