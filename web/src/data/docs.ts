/**
 * The docs in reading order, after the index at /docs/.
 * Single source of truth for the docs sidebar, prev/next links, the /docs/ index cards,
 * the footer docs column, and src/pages.ts.
 */

export interface DocMeta {
  key: string;
  group: string;
  /** Full title (used for the "Previous" / "Next" links and index cards). */
  title: string;
  /** Short name (used for the sidebar and footer). */
  short: string;
  /** One-sentence summary for the /docs/ index card. */
  summary: string;
}

export const DOCS: readonly DocMeta[] = [
  {
    key: "install",
    group: "Start",
    title: "Installing",
    short: "Installing",
    summary: "The download for your system, and the warnings macOS, Windows and Linux may show the first time.",
  },
  {
    key: "getting-started",
    group: "Start",
    title: "Your first stack",
    short: "Your first stack",
    summary: "From an empty window to a saved picture: add the frames, stack, look at the result and export it.",
  },
  {
    key: "concepts",
    group: "Start",
    title: "Stacks, renders and the Library",
    short: "Stacks and renders",
    summary: "What the app calls things, where renders are kept, and how to go back to one.",
  },
  {
    key: "methods",
    group: "Stacking",
    title: "Choosing a method",
    short: "Choosing a method",
    summary: "A table for the quick answer, and the three methods side by side on the same places.",
  },
  {
    key: "pyramid",
    group: "Stacking",
    title: "Pyramid",
    short: "Pyramid",
    summary: "What it is good at, and what the Smoothness setting changes.",
  },
  {
    key: "depth-map",
    group: "Stacking",
    title: "Depth",
    short: "Depth",
    summary: "What Radius, Smoothing and Blend Range change, and when to move them.",
  },
  {
    key: "wavelet",
    group: "Stacking",
    title: "Wavelet",
    short: "Wavelet",
    summary: "Where it comes from, what the Denoise setting changes, and when to choose it.",
  },
  {
    key: "alignment",
    group: "Stacking",
    title: "Aligning the frames",
    short: "Aligning",
    summary: "ECC or the grid search, and how far a frame may move.",
  },
  {
    key: "raw",
    group: "Files",
    title: "RAW, DNG and color",
    short: "RAW, DNG and color",
    summary: "The formats it reads, the RAW decoders, and what a stacked DNG keeps.",
  },
  {
    key: "export",
    group: "Files",
    title: "Exporting",
    short: "Exporting",
    summary: "DNG, TIFF, PNG or JPEG, bit depth, color profile and metadata.",
  },
  {
    key: "compare",
    group: "Working",
    title: "Viewing and comparing",
    short: "Viewing and comparing",
    summary: "The split view, side by side and flicker, and the depth-map and contribution overlays.",
  },
  {
    key: "retouching",
    group: "Working",
    title: "Retouching",
    short: "Retouching",
    summary: "Paint detail in from a source frame or another render.",
  },
  {
    key: "batch",
    group: "Working",
    title: "Batch",
    short: "Batch",
    summary: "Many stacks in one run, sorted by capture time or one per folder.",
  },
  {
    key: "cli",
    group: "Working",
    title: "The command line",
    short: "Command line",
    summary: "The same stacking from a terminal: one stack, a batch, or a sequence.",
  },
  {
    key: "library",
    group: "Help",
    title: "Your Library and backups",
    short: "Library and backups",
    summary: "What the Library holds and where, backing it up, Recently Deleted, and what Delete Library leaves alone.",
  },
  {
    key: "troubleshooting",
    group: "Help",
    title: "Troubleshooting and timings",
    short: "Troubleshooting",
    summary: "What to try when a stack goes wrong, what the app needs, and how to time it on your own machine.",
  },
  {
    key: "shortcuts",
    group: "Help",
    title: "Keyboard shortcuts",
    short: "Keyboard shortcuts",
    summary: "Every key, by what you are doing: the list, the viewer, comparing and retouching.",
  },
  {
    key: "faq",
    group: "Help",
    title: "Questions and answers",
    short: "Questions",
    summary: "Short answers to what people ask first, with links to the page that says more.",
  },
];

export const docHref = (key: string) => `/docs/${key}/`;

export const DOC_GROUPS: readonly string[] = [...new Set(DOCS.map((d) => d.group))];
