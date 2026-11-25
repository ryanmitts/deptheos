/** Site-wide constants, navigation, and metadata defaults. */

export const SITE_URL = "https://deptheos.com";

/** The public repository: the source, the license and the downloads (its releases). */
export const REPO = "https://github.com/ryanmitts/deptheos";

/** The iPhone and iPad app's public beta: TestFlight's join link. */
export const TESTFLIGHT = "https://testflight.apple.com/join/XheabQmv";

export const DEFAULT_DESCRIPTION = "Focus stacking for macro photography.";
export const DEFAULT_OG_IMAGE = `${SITE_URL}/media/gallery/hornet/1200.jpg`;
export const DEFAULT_OG_ALT = "A hornet photographed with a focus stack";

export interface NavItem {
  key: string;
  label: string;
  href: string;
}

/** Top navigation bar links. A link is marked active on every page under its URL. */
export const NAV: readonly NavItem[] = [
  { key: "features", label: "Features", href: "/features/" },
  { key: "iphone", label: "iPhone & iPad", href: "/iphone/" },
  { key: "docs", label: "Docs", href: "/docs/" },
  { key: "gallery", label: "Gallery", href: "/gallery/" },
];

export const FOOTER_SITE_LINKS: readonly { label: string; href: string }[] = [
  ...NAV.map(({ label, href }) => ({ label, href })),
  { label: "Timings", href: "/#speed" },
  { label: "Download", href: "/download/" },
  { label: "About", href: "/about/" },
  { label: "Support", href: "/support/" },
  { label: "Privacy", href: "/privacy/" },
  // The site says it is open source, so it links to the source and the license it is under.
  { label: "Source code", href: REPO },
  { label: "License", href: `${REPO}/blob/main/LICENSE.md` },
];
