// The download page: sets `data-os` on <html> so CSS promotes and highlights the visitor's system
// (an inline head script does the same before first paint). On iPhone and iPad the iOS block,
// otherwise last, moves above the desktop downloads.

import { detectSystem, isIOS } from "./platform.ts";

const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
const platform = nav.userAgentData?.platform || navigator.platform || "";
const system = detectSystem(platform, navigator.userAgent, navigator.maxTouchPoints);
const ios = !system && isIOS(platform, navigator.userAgent, navigator.maxTouchPoints);

if (ios) {
  document.documentElement.dataset.os = "ios";
  const block = document.getElementById("ios");
  const systems = document.querySelector(".dl-systems");
  if (block && systems) systems.before(block);
} else if (system) {
  document.documentElement.dataset.os = system;
}
