# ADR-168: The pages are a Cloudflare Worker's static assets and the media is in an R2 bucket under content hashes; deployed by hand

## Status

Accepted

## Context

`web/` builds to static HTML, CSS, JS, and WASM assets (ADR-160), accompanied by large media sets: full-resolution gallery stills and deep-zoom tile pyramids (~50 files and ~7 MB per gallery item), AV1/H.264 loop videos, documentation figure crops, and downloadable sample stacks. Cloudflare Workers static assets return `200 OK` with the entire file on HTTP `Range` requests, whereas Safari requires `206 Partial Content` responses to play video. In addition, gallery items and figure crops are updated in sets (`make-gallery.sh`, `make-lab-assets.ts`), and runtime scripts construct tile URLs dynamically (`tiles/<row>-<col>.jpg`).

## Decision

- **HTML/CSS/JS/WASM pages are served as a Cloudflare Worker's static assets** (`web/wrangler.jsonc`, `web/public/_headers`), with 1-year immutable caching on the Astro build's content-hashed `/assets/` files and `X-Robots-Tag: noindex` on the preview `workers.dev` subdomain.
- **Media is served from the Cloudflare R2 bucket `deptheos` at `images.deptheos.com` under per-set content hashes:**
  - Local development references files under `web/public/media/`.
  - Production builds rewrite media URLs to `https://images.deptheos.com/site/<set>/<hash>/<file>`, where `<set>` is a gallery item, a figure set, the screenshots, the turntable video or the home page's media, and `<hash>` is the content hash of that set (`web/src/media.ts`, `web/scripts/media-sets.ts`).
  - Because any change to a set produces a new `<hash>` prefix, media URLs are immutable and cached for a year, while runtime tile URLs (`tiles/<row>-<col>.jpg`) still resolve under a single set base URL.
  - Sample stacks live in the same R2 bucket under `samples/`.
- **Manual deploy via `npm run deploy` (`web/scripts/deploy.sh`):** Refuses a dirty working tree, builds the site, uploads any new media sets to R2 first (never deleting old sets so live pages and rollbacks stay intact), records the deploy manifest under `site/_deploys/` (`web/scripts/deploy-manifest.ts`), and deploys the Worker static assets; `npm run deploy -- --preview` uploads them as a version that is not served, with a preview URL to check it. GitHub Actions (`web.yml`) builds and tests on pushes and PRs without deploying automatically, so the live site updates only when a release or doc update is ready.

## Alternatives considered

### Serving videos and gallery tiles from Worker static assets

Worker static assets do not serve `206 Partial Content` byte ranges out of the box, breaking video playback in Safari, and bundling hundreds of megabytes of gallery media inside every Worker upload couples page deploys to media size.

### Per-file content hashes for media

Because the deep-zoom tile viewer and interactive figures construct paths like `tiles/<row>-<col>.jpg` at runtime, per-file hashes would require shipping a manifest of every tile filename to the browser. Hashing per media set gives one immutable prefix per gallery item or figure.

## Consequences

- Media sits on a separate origin (`images.deptheos.com`) with CORS enabled (`web/r2-cors.json`); any client script that reads image pixels into a canvas must load with `crossOrigin` and resolve URLs through `mediaUrl()`.
- Deploying requires `npx wrangler login` and an `rclone` remote configured for the R2 bucket (`web/README.md`).

## Code

- `web/wrangler.jsonc`, `web/r2-cors.json`, `web/public/_headers`.
- `web/scripts/deploy.sh`, `web/scripts/sync-media.ts`, `web/scripts/media-sets.ts`, `web/scripts/deploy-manifest.ts`.
- `web/src/media.ts`, `web/src/integrations/deptheos.ts`, `web/README.md`.
