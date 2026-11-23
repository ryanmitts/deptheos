# ADR-160: The website is static Astro pages whose numbers come from the engine and whose figures show the app's own renders; one tokens file colors both apps, and the website keeps its own

## Status

Accepted.

## Context

The website (`web/`) is static marketing, gallery and documentation pages. Its prose cites the engine's defaults, slider ranges, internal constants and benchmark times; typed into HTML by hand, they drift whenever a constant changes in Rust. Its docs explain each stacking method and aligner so a photographer can choose, with figures of what a setting does.

A figure can't be computed from a crop in the browser and be true to the app. Stacking a small crop is not stacking the full frame and cropping after: smaller dimensions build fewer Laplacian pyramid levels (on the PCB stack at Smoothness 4, a black capacitor measured 0.8/255 in the full-frame stack and 10.5/255 stacked as a 1024×768 crop), Depth's Radius 50 blur is wider than a small crop's map grid, and its confidence stretch uses whole-frame percentiles. In-browser stackers also need WebGPU, are slow on phones, and compute the same few outputs on every visit for a control with fixed stops.

Deptheos has three visual interfaces: the desktop app (`gpui-kit` JSON themes plus colors it paints itself), the iOS app (asset catalog color sets) and the website (CSS). The two apps share dense, system-font chrome around a dark-neutral photo backdrop; the website is for long-form reading.

## Decision

### The pages

- **Astro, built to static HTML, no client framework.** Each page is an `.astro` file under `web/src/pages/`, built (`web/astro.config.mjs`) to `dist/<route>/index.html` so addresses need no `.html`; `web/src/pages.ts` lists every built page, and the build fails when the two differ. `web/src/layouts/BaseLayout.astro` holds the shared chrome: the `<head>` metadata, canonical and Open Graph tags, top bar, footer and the site script; `web/src/layouts/DocsLayout.astro` adds the docs sidebar, the section outline and the previous and next links, from `web/src/data/docs.ts`. The gallery's pages are one template over `web/src/data/gallery.ts`. Interactive figures are plain TypeScript modules over the DOM and Canvas, bundled by Astro's `<script>` tags; the production build ships no framework runtime and hydrates nothing (the React in `package.json` is a dev-only overlay, `web/src/dev/agentation.tsx`, loaded only under `import.meta.env.DEV`).
- **No engine number is typed into the pages.** Components render them at build time and in `npm run dev`, through `web/src/lib/site-data.ts`:
  - `<Param path="…">` (`web/src/components/Param.astro`) from `generated/manifest.json`, which `crates/deptheos_webexport` writes by calling the engine's own functions and constants (`npm run export-params`);
  - `<Measured path="…">` (`web/src/components/Measured.astro`) from `measured/benchmarks/<machine>-<stack>-<format>.json`, records written by `deptheos bench` (`web/scripts/bench.sh`);
  - optional `digits`, `scale` and `offset` format the number.
  An unknown path fails the build. `generated/manifest.json` is committed, so the site builds and runs with Node alone; `deptheos_webexport --check` fails when it differs from what the engine makes now, and the Tests and Release workflows run it, so it can't lag the Rust code unnoticed.
- **The benchmarks are named machines.** The home page's speed table and hardware notes come from the per-machine records in `web/measured/benchmarks/`.
- **The build's own checks are an Astro integration** (`web/src/integrations/deptheos.ts`): after the build it checks every `/media/…` path exists under `public/`, rewrites them to the media bucket when built for deploy (ADR-168), and writes `sitemap.xml` and `robots.txt` from `web/src/pages.ts`.
- **A figure's async loads go through `latest()`** (`web/src/latest.ts`): a new request aborts the one in flight through its `AbortSignal`, and a result applies only while its request is still the latest (ADR-143).

### The docs figures

- **A figure whose control has fixed stops switches between 100% crops cut from full-resolution stacks** rendered by the real engine: `web/scripts/make-lab-assets.ts`, driven by `web/scripts/lab-manifest.ts` and the `web_lab` example (`crates/deptheos_core/examples/web_lab.rs`), into `public/media/labs/`.
- **The browser computes only what is continuous or a picture of the idea, never a stack.** The Pyramid page's level mixer (`web/src/labs/mixer-math.ts`) runs in TypeScript on the CPU, behind a `<details>` disclosure that loads when opened; the pyramid diagram uses the same arithmetic (`web/src/labs/laplacian.ts`, the app's 5-tap weights). The aligning figure draws the frames moved by transforms computed once when its assets were made (`web/src/labs/moves.ts`, `affine.ts`); the browser only draws.
- **Direct explanations, no quizzes.** A figure shows its controls, what they do and what is on screen at once, without "guess before you look" prompts (`web/README.md`).
- **Each interactive figure's state is a pure reducer with unit tests** (`web/src/labs/*-state.ts`).

### Colors and type

- **`design/tokens.json` is the one source of the desktop's and iOS's colors**, tokens named by role (`accent`, `surface`, `viewer`, `hud*`, …). `node script/tokens.mjs` writes:
  - the iOS color sets in `ios/DeptheosMobile/Assets.xcassets` (`AccentColor`, `Viewer`, `AccentFill`, `Edited`, `Hud`, `HudBorder`, `HudText`);
  - `crates/deptheos_gpui/themes/deptheos.json`, the light and dark `gpui-kit` themes;
  - `crates/deptheos_gpui/src/brand.rs`, the colors the desktop paints itself.
- **The generator refuses a token file it can't use** (`validate` in `script/tokens.mjs`): colors are lowercase `#rrggbb`, or `#rrggbbaa` where see-through, which only the desktop's own `brand.rs` colors may be; `hud`, `hud-border` and `hud-text` must exist; and a `viewer` or `hud*` token must be one value in light and dark, because chrome drawn over a photo must not change with the window's theme.
- **`node script/tokens.mjs --check` fails when a generated file is out of date**; the release workflow runs it (`.github/workflows/release.yml`).
- **On iOS only the accent (its tint and the one primary button's fill), the viewer backdrop, the edited dot and the chrome over the photo are custom;** lists, backgrounds and controls use the system's colors (`design/README.md`).
- **The website has its own hand-written layer, `web/src/site/site-tokens.css`**, for long reading: the apps' cool grays with the page a shade off its panels and running text a step softer than headings, a 17 px type scale, and IBM Plex Sans and IBM Plex Mono self-hosted from `@fontsource`, so no request leaves the site for fonts.

## Alternatives considered

### Live WebGPU stackers in the browser, on cropped frames

They made crop-boundary artifacts and pyramid-depth differences that don't match what the app renders on full camera frames, and a docs figure would need WebGPU to be seen.

### Plain HTML pages with a Vite transform plugin for the shared chrome and the numbers

It ships the same static HTML, but every page repeats its structure by hand and the plugin is string substitution over HTML. Astro's layouts and components give the chrome, the docs navigation and the engine's numbers one typed source each, render them at build time, and still ship no framework runtime, since none of its components hydrate.

### A client framework (React, Preact) for the pages or figures

The pages are static reading and the figures self-contained DOM and Canvas modules; a runtime and hydration would add weight to every page for nothing they need.

### Hardcoding parameters and timings in the HTML

It drifts from the engine whenever a default, a constant or a benchmark changes.

### The website's CSS generated from `design/tokens.json`

Long-form pages need softer text contrast, other surface steps and reading type, not the dense panel tokens of the apps.

## Consequences

- Every docs figure is the app's own full-frame output and works in every browser and on phones, with no WebGPU. After an algorithm change that alters output, `npm run lab-assets` makes the figures' crops again.
- Building the site needs only Node. A change to a parameter the prose cites, or a new engine constant in it (exported by `crates/deptheos_webexport` and cited with `<Param>`), is followed by `npm run export-params` and a commit of `generated/manifest.json`.
- An app color changes in `design/tokens.json` and through `node script/tokens.mjs`; a release fails while the generated files lag. The brand accent changes in both `design/tokens.json` and `web/src/site/site-tokens.css`.

## Code

- `web/astro.config.mjs`, `web/src/integrations/deptheos.ts`, `web/src/layouts/BaseLayout.astro`, `web/src/layouts/DocsLayout.astro`, `web/src/components/Param.astro`, `web/src/components/Measured.astro`, `web/src/lib/site-data.ts`, `web/src/pages.ts`, `web/src/data/docs.ts`, `web/src/latest.ts`, `web/README.md`.
- `crates/deptheos_webexport/src/main.rs`, `crates/deptheos_webexport/src/params.rs`, `web/generated/manifest.json`, `.github/workflows/tests.yml`, `web/scripts/bench.sh`, `web/measured/benchmarks/`.
- `web/scripts/make-lab-assets.ts`, `web/scripts/lab-manifest.ts`, `crates/deptheos_core/examples/web_lab.rs`, `web/src/labs/mixer-math.ts`, `web/src/labs/laplacian.ts`, `web/src/labs/moves.ts`.
- `design/tokens.json`, `design/README.md`, `script/tokens.mjs`, `crates/deptheos_gpui/themes/deptheos.json`, `crates/deptheos_gpui/src/brand.rs`, `web/src/site/site-tokens.css`.
