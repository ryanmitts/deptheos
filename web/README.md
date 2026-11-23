# deptheos.com

The site: the home page, Features, Download, the gallery and the docs.
Static pages built by Astro; the one figure that computes in the browser is
the pyramid page's level mixer, on the CPU, loaded when its disclosure
opens.

```sh
npm install
npm test                 # unit tests (src/**/*.test.ts); `just web-check` adds the typecheck
npm run dev              # the site at localhost:5173 (`just web`), with Agentation overlay enabled
npm run dev:lan          # the same over HTTPS for phones on the network: accept the self-signed certificate
npm run build            # static site in dist/, plus sitemap.xml and robots.txt
npm run preview          # preview the built dist/ locally
```

In `npm run dev`, [Agentation](https://github.com/benjitaylor/agentation) is mounted automatically
(`src/dev/agentation.tsx`) so you can click elements and copy structured feedback for AI agents.
Set `PUBLIC_AGENTATION_ENDPOINT=http://localhost:4747` to live-sync with `npx -y agentation-mcp server`.
Agentation and React are completely tree-shaken from production builds (`npm run build`).

`generated/manifest.json`, the app's parameters the text cites, is
committed, so the site builds without Rust. After a change to a parameter
in the engine, `npm run export-params` (Rust) writes it again; the Tests and
Release workflows fail while it lags (`deptheos_webexport --check`).

## Pages

Pages live under `src/pages/` as `.astro` files, built to `dist/<route>/index.html`
so URLs have no `.html`. Shared chrome (topbar, `<head>` metadata, favicons, canonical/OG tags,
skip link, footer, and `src/site.ts`) is in `src/layouts/BaseLayout.astro`.

- **Docs** (`src/pages/docs/`): use `src/layouts/DocsLayout.astro`. The docs' order, groups,
  and index card summaries live in one place, `src/data/docs.ts`, which drives the sidebar,
  the `/docs/` index, prev/next links, the footer, and `src/pages.ts`.
- **Gallery** (`src/pages/gallery/`): driven by `src/data/gallery.ts` and rendered by
  `src/pages/gallery/[slug].astro` and `src/pages/gallery/index.astro`.

The logo is the app icon's glyph from its one source,
`DeptheosAppIcon.icon/Assets/deptheos_icon.svg`, read by
`src/brand/logo.ts` and rendered by `<BrandTile />`. `npx tsx scripts/make-icons.ts` writes the
favicons from it. The name and logo are trademarks (`TRADEMARKS.md`): use
the artwork as drawn.

The look is `src/site/site-tokens.css`, web only (`design/README.md` says
why the apps' tokens don't reach it). Type is IBM Plex Sans and Mono,
self-hosted from `@fontsource`, so no request leaves the site for fonts;
Mono is for data (`.exif` captions, `.eyebrow` labels, numbers).

What the pages need that isn't there yet is marked in red: a screenshot as
`<figure class="shot"><div class="shot-todo" data-shot="name">what to capture</div>…</figure>`
(put the PNG at `~/DeptheosShots/<name>.png`, run `scripts/make-shots.sh` and replace the
`div` with `<Shot name="<name>" width=… height=… alt=…>` from `src/components/Shot.astro`), anything else as `<p class="todo" data-todo="…">`.
`grep -rn 'shot-todo\|class="todo"' .` lists them.

Media made by scripts:

- A gallery picture: `scripts/make-gallery.sh <name> <folder of frames>`
  writes `public/media/gallery/<name>/` (stills, 1024 px tiles, the
  build-up video); add its entry to `src/data/gallery.ts`. The home
  page's hero is the hornet's stills and video.
- The screenshots: the PNGs are kept outside git, in `~/DeptheosShots`
  (`SHOTS_SRC` names another folder). `scripts/make-shots.sh` writes
  `public/media/shots/<name>.avif`, `<name>-half.avif` and a half-width
  `<name>.jpg` from each.
- The docs figures: `npm run lab-assets` (below).

The turntable video on Features and the gallery,
`public/media/turntable/pcb.*.mp4` and `pcb-poster.jpg`, is a cut made by
hand, not by a script.

Links can add `#<figure or section id>`; `#<figure>/<choice>` also picks a
figure's stop or place (`#methods-figure/board+wavelet`).

## Writing the docs

A doc answers what someone using the app needs: what a setting does, when
to change it and what it costs, with a little of how it works, in terms
true of any focus stack. What a sample stack shows goes in the figure's
caption; click-by-click instructions go in captions too. Each doc reads on
its own, and comparisons between methods live on `/docs/methods/` and
`/docs/alignment/#which`. State findings directly: no question a figure
asks and then answers, no expander that holds back the answer. A
`<details>` is for what the page does not need: a figure to play with, a
heavy download, the fine print, and a section's constants and formulas
(`<details class="numbers">`, "In numbers"). The wavelet page credits the
method's paper and focus-stack.

Numbers are never typed into the text. Both kinds are filled in at build
time (and by `npm run dev`), and an unknown path fails the build:

- The app's settings and constants: `<Param path="defaults.pyramid.smoothness" />`,
  a path into the `params` block of `generated/manifest.json` (`src/lib/site-data.ts`).
- Stacking times: `<Measured path="benchmarks.m5-max-pcb1-jpeg.runs.pyramid.ecc.seconds" digits={1} />`,
  from `measured/benchmarks/<machine>-<stack>-<format>.json`, written by
  the CLI's `bench`. `…stack.frames`, `…label` and `…machine.os` are
  addressable too.

To time a machine's four records again, from the repo root:

```sh
web/scripts/bench.sh m5-max ~/stacks/jpeg/pcb1 ~/stacks/jpeg/hornet ~/stacks/PCB/115EOSR5 ~/stacks/hornet2
RUST_LOG=warn web/scripts/bench.sh rtx-5090 <the four folders> --machine "Ryzen 9 9950X3D + RTX 5090"
```

The four stacks are pcb1 (200 frames) and hornet (66) as JPEG, and pcb1 as CR3 decoded by LibRaw and as
pre-converted uncompressed DNG, all 8192 × 5464. Each timed run uses `--live --repeat 5 --cool 30` (median of
five runs on the live app path, with a 30-second cooldown before each run). The home page's RAW row uses the
LibRaw CR3 record (`*-pcb1-cr3.json`), which decodes directly from the camera's CR3 files without Adobe DNG
Converter. The table's machine name (`label`) is the CPU and, on a Mac, its GPU core count, so a new Mac of the
same family keeps its file names; `--machine` overrides it. To add a machine, run the script there with a new
prefix, cite its files in `src/pages/index.astro` and commit them.

## The docs figures' images

The figures show results the app rendered at camera resolution, cut where
each effect is plain and drawn at 100%. One script makes them from the
original frames, as `scripts/lab-manifest.ts` describes each figure (the
stack, the place in pixels and why, the CLI settings):

```sh
npm run lab-assets                          # every figure, into public/media/labs/<figure>/
npm run lab-assets -- radius --work ~/lab   # one figure; keep the aligned frames and renders for next time
```

Each stack is aligned once at full size with the ECC aligner
(`web_sample_raw --full-size --format tif`, about 8 GB for 66 frames), and
every render runs `deptheos <method> --align none` on those frames, so all
of a stack's figures share its coordinates. The `web_lab` example cuts the
crops and frame maps and makes the per-pixel pick no app method makes. The
originals are read from `LAB_SOURCES` (default `~/Desktop/sample_images`).
Each figure's folder has a `figure.json` with its files and what the figure
reads. `public/media/labs/diagram/spider-23.jpg`, the pyramid page's static
diagram, is not written by the script.

Figures start when they near the screen and each fails on its own
(`src/labs/main.ts`); until then the box shows a still. `figure.ts` loads a
figure's folder and builds the shared parts, `pan.ts` is the 100% panel,
and each figure's state is a pure reducer with tests (`*-state.ts`).

## Checks

```sh
npm run smoke -- http://localhost:5173           # every page loads, every figure answers its controls, links land on the stop they name, no console errors
npm run touch-check -- http://localhost:5173     # on a phone, a swipe over a figure's panel pans it, and over the text scrolls the page
npx tsx scripts/overflow-check.ts http://localhost:5173   # no page scrolls sideways at a phone's width
npm run shot -- http://localhost:5173/docs/pyramid/ out.png --scroll "#mixer" --wait 2000 --el "#mixer"
```

`just web-smoke` runs the first two. `npm run shot` needs Chrome and
`npm run dev` running; `scripts/screenshot.ts` lists its options
(`--light`, `--dark`, `--full`, `--load-all`, …).

## Deploying

Two places serve the site (why, in `docs/decisions/168-website-hosting.md`):

- **The pages** (HTML, scripts, styles): Cloudflare, as a Worker's static
  assets (`wrangler.jsonc`), at deptheos.com.
- **The media** (everything in `public/media/`): the R2 bucket `deptheos`,
  at images.deptheos.com, under `site/`. The bucket also holds the sample
  stacks (`samples/`) and the README's pictures.

Nothing deploys on a push:

```sh
npm run deploy -- --preview   # build, upload the media, upload the pages as a version; wrangler prints a URL to look at it
npm run deploy                # build, upload the media, deploy the pages (`just web-deploy`)
```

`scripts/deploy.sh` refuses a working tree with changes, so what is live is
always a commit. `npx wrangler rollback` goes back to the pages of the
deploy before; their media is still in the bucket.

**Media addresses.** Pages name a media file by its path in `public/`
(`/media/gallery/hornet/1200.jpg`), which `npm run dev` serves. A deploy
builds with `MEDIA=cdn`: the `deptheosIntegration` (`src/integrations/deptheos.ts`) rewrites
every `/media/…` in the HTML, and `mediaUrl` (`src/media.ts`) does the same
for addresses a script builds:

```
/media/gallery/hornet/tiles/0-0.jpg
→ https://images.deptheos.com/site/gallery/hornet/<hash>/tiles/0-0.jpg
```

The hash is of a set (a gallery picture's folder, a figure's folder, the
home page's files: `setOf` in `src/media.ts`, hashed by
`scripts/media-sets.ts`). Changing any file gives the whole set a new
address, so an address never changes its content and is cached for a year.
`scripts/sync-media.ts` uploads the sets with rclone before the pages go
out, skipping what the bucket has, and deletes nothing: old sets stay for
the pages that still name them. In a script, name a media file through
`mediaUrl("/media/…")`, and give an image whose pixels a canvas reads
`crossOrigin = "anonymous"` (`loadImages` in `src/labs/figure.ts`).

**What each deploy holds.** `scripts/deploy-manifest.ts` writes a record
to the bucket after the pages are out: the time, the commit, deploy or
preview, the Worker version and each media set's hash and file count. To
clear old sets, `rclone purge` a hash no deploy you would roll back to
names:

```sh
rclone lsf r2:deptheos/site/_deploys/            # every deploy, oldest first
rclone cat r2:deptheos/site/_deploys/<file>      # one of them
```

**Once on a machine:**

```sh
npx wrangler login
brew install rclone
# An R2 API token (dashboard: R2, Manage API tokens; Object Read & Write on the bucket) gives the two keys:
rclone config create r2 s3 provider=Cloudflare access_key_id=… secret_access_key=… \
  endpoint=https://<account id>.r2.cloudflarestorage.com acl=private
```

**The bucket** was made by hand and is described nowhere but here. To make
it again: `npx wrangler r2 bucket create deptheos` (location WNAM),
`npx wrangler r2 bucket domain add deptheos --domain images.deptheos.com --zone-id <the zone's id>`,
and `npx wrangler r2 bucket cors set deptheos --file r2-cors.json` (any
origin may read: the figures read pictures' pixels, and the preview address
is another origin). Its public r2.dev address is off.

`public/_headers` sets a year's caching for the hashed files in `/assets/`
and `X-Robots-Tag: noindex` on the workers.dev address, which is for
looking at a deploy. An address with no page gets `404.html`.
