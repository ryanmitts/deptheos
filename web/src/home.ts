// The home page: the before/after wiper, the hero's build-up video and 100% viewer,
// and platform-aware download labels.

import "./compare.ts";
import { detectSystem, type System } from "./platform.ts";
import { onVideoFailed } from "./video-failed.ts";
import { attachViewer } from "./viewer.ts";

const SYSTEM_NAMES: Record<System, string> = {
  mac: "macOS",
  windows: "Windows",
  linux: "Linux",
};

const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
const system = detectSystem(nav.userAgentData?.platform || navigator.platform || "", navigator.userAgent, navigator.maxTouchPoints);
if (system) {
  for (const btn of document.querySelectorAll<HTMLAnchorElement>('.hero-cta .btn.primary[href="/download/"]')) {
    btn.textContent = `Download for ${SYSTEM_NAMES[system]}`;
  }
}

const hero = document.querySelector<HTMLElement>(".hero");
if (hero) {
  if (hero.hasAttribute("data-viewer")) attachViewer(hero);
  // The stack building up from the back: played once on load, then faded out to show the still
  // underneath, with a Replay button to watch it again. Skipped with reduced motion (the CSS
  // hides it too) or if it can't play.
  const video = hero.querySelector<HTMLVideoElement>("[data-hero-video]");
  const replay = hero.querySelector<HTMLButtonElement>(".hero-replay");
  if (video) {
    const finish = () => {
      video.classList.add("done");
      if (replay) replay.hidden = false;
    };
    const remove = () => {
      video.remove();
      replay?.remove();
    };
    const onPlayError = (err: unknown) => {
      const name = err instanceof Error ? err.name : "";
      if (name === "AbortError") return;
      if (name === "NotAllowedError") {
        finish();
        return;
      }
      remove();
    };
    video.addEventListener("ended", finish);
    onVideoFailed(video, remove);
    replay?.addEventListener("click", (e) => {
      e.stopPropagation();
      replay.hidden = true;
      video.currentTime = 0;
      video.classList.remove("done");
      video.play().catch(onPlayError);
    });
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) remove();
    else video.play().catch(onPlayError);
  }
}
