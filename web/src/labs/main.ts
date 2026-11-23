// The docs pages' figures: each `[data-lab="<name>"]` starts when it comes
// near the screen (its poster shows until then). A link can pick a figure's
// stop or place: `#<id>/<choice>` (`#methods-figure/board+wavelet`).

import "./labs.css";
import { showError, type StartOpts } from "./figure.ts";

const FIGURES: Record<string, () => Promise<{ start: (el: HTMLElement, opts: StartOpts) => Promise<void> }>> = {
  depthmap: () => import("./depthmap.ts"),
  mixer: () => import("./mixer.ts"),
  moves: () => import("./moves.ts"),
  stops: () => import("./stops.ts"),
  diagram: () => import("./diagram.ts"),
};

// The figure a link points at, and its choice. The browser can't scroll to a hash with a
// choice itself. A malformed hash (`#%E0`) points at nothing.
const [target, choice] = (() => {
  try {
    return decodeURIComponent(location.hash.slice(1)).split("/");
  } catch {
    return [];
  }
})();
if (choice !== undefined) {
  // A figure inside a closed <details> has no place on the page until it is opened.
  const figure = document.getElementById(target);
  figure?.closest("details")?.setAttribute("open", "");
  figure?.scrollIntoView();
}

/** Reload the page, unless this tab already did for a missing chunk; true if it reloads. */
function reloadOnce(): boolean {
  const key = `deptheos-reloaded:${location.pathname}`;
  try {
    if (sessionStorage.getItem(key)) return false;
    sessionStorage.setItem(key, "1");
  } catch {
    return false;
  }
  location.reload();
  return true;
}

const near = new IntersectionObserver(
  (entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      const el = entry.target as HTMLElement;
      near.unobserve(el);
      const load = FIGURES[el.dataset.lab!];
      if (!load) {
        console.error(`no figure named "${el.dataset.lab}"`);
        continue;
      }
      const linked = el.id !== "" && el.id === target;
      void load()
        .catch((err) => {
          // A page open across a deploy names chunks that are gone: reload it once for the new ones.
          if (reloadOnce()) return new Promise<never>(() => {});
          throw err;
        })
        .then((m) => m.start(el, { choice: linked ? choice : undefined }))
        .catch((err) => showError(el, err));
    }
  },
  { rootMargin: "800px 0px" },
);

for (const el of document.querySelectorAll<HTMLElement>("[data-lab]")) near.observe(el);
