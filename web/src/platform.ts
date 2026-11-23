// Which desktop system a visitor is on, for the download page to show its block first.
// Pure, so it is tested without a browser (platform.test.ts).

/** A desktop system Deptheos is built for, by its block's id on /download/. */
export type System = "mac" | "windows" | "linux";

/**
 * The desktop system the browser names, or `undefined` for a phone, tablet (an iPad reports a
 * Mac but has touch), ChromeOS or anything unknown. `touchPoints` is `navigator.maxTouchPoints`.
 */
export function detectSystem(platform: string, userAgent: string, touchPoints: number): System | undefined {
  const p = platform.toLowerCase();
  const ua = userAgent.toLowerCase();
  if (/iphone|ipad|ipod|android|cros/.test(ua)) return undefined;
  if (p.startsWith("mac") || (!p && ua.includes("mac os x"))) return touchPoints > 1 ? undefined : "mac";
  if (p.startsWith("win") || (!p && ua.includes("windows"))) return "windows";
  if (p.includes("linux") || (!p && ua.includes("linux"))) return "linux";
  return undefined;
}

/** True on an iPhone, iPad (including desktop-mode Safari on iPadOS) or iPod. */
export function isIOS(platform: string, userAgent: string, touchPoints: number): boolean {
  const p = platform.toLowerCase();
  const ua = userAgent.toLowerCase();
  if (/iphone|ipad|ipod/.test(ua)) return true;
  if ((p.startsWith("mac") || (!p && ua.includes("mac os x"))) && touchPoints > 1) return true;
  return false;
}
