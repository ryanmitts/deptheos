// Makes the docs figures' images from the original frames, as
// `scripts/lab-manifest.ts` describes them, into `public/media/labs/<figure>/`
// with a `figure.json` that lists what is there.
//
//   npm run lab-assets                        every figure
//   npm run lab-assets -- radius steps        only these figures
//   npm run lab-assets -- --work ~/lab-work   keep the aligned frames and renders there, and reuse them next time
//
// The originals stay outside the repo, under LAB_SOURCES (default
// ~/Desktop/sample_images). Each stack a figure uses is aligned once at
// camera resolution (about 8 GB of TIFFs for 66 frames) into a work folder,
// a temporary one removed at the end unless --work names one. The CLI and
// the `web_lab` example are built in release mode first.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { rescale, type Warp } from "../src/labs/affine.ts";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { FIGURES, STACKS, type Figure, type Stack, type StackId } from "./lab-manifest.ts";

const web = resolve(import.meta.dirname, "..");
const repo = resolve(web, "..");
const sources_root = (process.env.LAB_SOURCES ?? "~/Desktop/sample_images").replace(/^~/, homedir());

const argv = process.argv.slice(2);
const workAt = argv.indexOf("--work");
if (workAt >= 0 && !argv[workAt + 1]) throw new Error("--work requires a directory path");
const kept = workAt >= 0 ? resolve(argv[workAt + 1].replace(/^~/, homedir())) : undefined;
const wanted = argv.filter((a, i) => !a.startsWith("--") && !(workAt >= 0 && i === workAt + 1));
const figures = wanted.length ? FIGURES.filter((f) => wanted.includes(f.id)) : FIGURES;
for (const id of wanted) if (!FIGURES.some((f) => f.id === id)) throw new Error(`no figure "${id}" in lab-manifest.ts`);

const run = (cmd: string, args: string[], opts: { cwd?: string } = {}) => {
  console.log(`  $ ${[cmd, ...args].join(" ").slice(0, 200)}`);
  execFileSync(cmd, args, { stdio: ["ignore", "inherit", "inherit"], cwd: opts.cwd ?? repo });
};

console.log("Building the CLI and web_lab (release)…");
run("cargo", ["build", "-q", "--release", "-p", "deptheos_cli"]);
run("cargo", ["build", "-q", "--release", "-p", "deptheos_core", "--features", "examples", "--example", "web_lab", "--example", "web_sample_raw"]);
const cli = join(repo, "target/release/deptheos");
const example = (name: string) => join(repo, "target/release/examples", name);

const work = kept ?? mkdtempSync(join(tmpdir(), "deptheos-labs-"));
mkdirSync(work, { recursive: true });

/** The stack's original frames, in name order. */
function sources(id: StackId): string[] {
  const stack: Stack = STACKS[id];
  const folder = join(sources_root, stack.folder);
  let files = readdirSync(folder).filter((f) => /\.(jpe?g|png|tiff?)$/i.test(f)).sort();
  if (stack.range) files = files.slice(stack.range[0] - 1, stack.range[1]);
  return files.map((f) => join(folder, f));
}

/** The stack's frames aligned at camera resolution, as TIFFs in name order; aligned on first use. */
function aligned(id: StackId): string[] {
  const dir = join(work, id, "aligned");
  if (!existsSync(join(dir, "stack.json"))) {
    const files = sources(id);
    console.log(`Aligning ${id}: ${files.length} frames`);
    run(example("web_sample_raw"), [dir, "1000000", "95", id, "--align", "--full-size", "--format", "tif", ...files]);
  }
  const list = JSON.parse(readFileSync(join(dir, "stack.json"), "utf8")) as { frames: { file: string }[] };
  return list.frames.map((f) => join(dir, f.file));
}

/** Cut rectangles (x,y,w,h, or that text with `@width` to resize) out of an image, as JPEGs. */
const crop = (image: string, quality: number, cuts: [rect: number[] | string, out: string][]) =>
  run(example("web_lab"), ["crop", image, String(quality), ...cuts.map(([r, out]) => `${typeof r === "string" ? r : r.join(",")}=${out}`)]);

/** A JPEG's size, from its frame header. */
function jpegSize(file: string): { width: number; height: number } {
  const b = readFileSync(file);
  for (let i = 2; i + 9 < b.length; ) {
    if (b[i] !== 0xff) throw new Error(`${file}: not a JPEG marker at ${i}`);
    const marker = b[i + 1];
    const len = b.readUInt16BE(i + 2);
    if (marker >= 0xc0 && marker <= 0xcf && marker !== 0xc4 && marker !== 0xc8 && marker !== 0xcc) {
      return { height: b.readUInt16BE(i + 5), width: b.readUInt16BE(i + 7) };
    }
    i += 2 + len;
  }
  throw new Error(`${file}: no JPEG frame header`);
}

/** Every file in the figure's output folder with its size, for figure.json and the budget report. */
function listing(out: string) {
  return readdirSync(out)
    .filter((f) => f !== "figure.json")
    .sort()
    .map((f) => ({ file: f, bytes: statSync(join(out, f)).size }));
}

/** A figure of one kind. */
type Of<K extends Figure["kind"]> = Extract<Figure, { kind: K }>;
/** What a kind adds to its figure.json. */
type Extra = Record<string, unknown>;

function make(figure: Figure) {
  const out = join(web, "public/media/labs", figure.id);
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  const extra = contents(figure, out);
  const files = listing(out);
  const total = files.reduce((n, f) => n + f.bytes, 0);
  writeFileSync(join(out, "figure.json"), JSON.stringify({ id: figure.id, kind: figure.kind, ...extra, files }, null, 2) + "\n");
  console.log(`${figure.id}: ${files.length} files, ${(total / 1e6).toFixed(2)} MB`);
}

/** Write the figure's files into `out`. Every kind but moves works on the aligned frames. */
function contents(figure: Figure, out: string): Extra {
  switch (figure.kind) {
    case "renders":
      return renders(figure, out);
    case "moves":
      return moves(figure, out);
    case "pair":
      return pair(figure, out);
    case "pick":
      return pick(figure, out);
    case "frames":
      return framesFigure(figure, out);
  }
}

function renders(figure: Of<"renders">, out: string): Extra {
  const frames = aligned(figure.stack);
  // Each stack the places come from is stacked once per render.
  const stacks = [...new Set(figure.places.map((p) => p.stack ?? figure.stack))];
  for (const stack of stacks) {
    const all = stack === figure.stack ? frames : aligned(stack);
    const places = figure.places.filter((p) => (p.stack ?? figure.stack) === stack);
    for (const render of figure.renders) {
      const subset = all.filter((_, i) => i % (render.every ?? 1) === 0);
      const dir = join(work, stack, "renders");
      mkdirSync(dir, { recursive: true });
      const key = createHash("sha256")
        .update(JSON.stringify({ method: render.method, args: render.args ?? [], every: render.every ?? 1 }))
        .digest("hex")
        .slice(0, 8);
      const image = join(dir, `${figure.id}-${render.name}-${key}.png`);
      if (!render.mapOnly) {
        if (!existsSync(image)) run(cli, [render.method, "--align", "none", ...(render.args ?? []), "-o", image, ...subset]);
        crop(image, figure.quality, places.map((p) => [p.rect, join(out, `${render.name}-${p.name}.jpg`)]));
      }
      // The depth map as the frame nearest each pixel, colored in the browser like every frame map.
      if (render.depthMap) {
        if (render.method !== "depth") throw new Error(`${figure.id}: a depth map is the depth method's`);
        const cuts = places.map((p) => `${p.rect.join(",")}=${join(out, `${render.name}-${p.name}-map.png`)}`);
        run(example("web_lab"), ["depthmap", ...(render.args ?? []), ...cuts, "--", ...subset]);
      }
      if (render.mergedMap) {
        if (render.method !== "wavelet" || render.args?.length) throw new Error(`${figure.id}: the merged gray picture is the default wavelet's`);
        const cuts = places.map((p) => `${p.rect.join(",")}=${join(out, `${render.name}-${p.name}-grey.jpg`)}`);
        run(example("web_lab"), ["merged", String(figure.quality), ...cuts, "--", ...subset]);
      }
    }
  }
  return {
    frames: frames.length,
    renders: figure.renders.map(({ name, method, args, every }) => ({ name, method, args: args ?? [], every: every ?? 1 })),
    places: figure.places.map(({ name, rect }) => ({ name, width: rect[2], height: rect[3] })),
  };
}

function moves(figure: Of<"moves">, out: string): Extra {
  // The frames as shot, whole, and where the app's aligner moves each one: both
  // from web_sample_raw at half size, the transforms rescaled to the frames written.
  const shot = sources(figure.stack);
  const dir = join(work, figure.stack, "moves");
  rmSync(dir, { recursive: true, force: true });
  run(example("web_sample_raw"), [join(dir, "shot"), String(figure.width), String(figure.quality), "frame", ...shot]);
  run(example("web_sample_raw"), [join(dir, "aligned"), String(figure.width), String(figure.quality), "frame", "--align", ...shot]);
  const list = readdirSync(join(dir, "shot")).filter((f) => f.endsWith(".jpg")).sort();
  for (const f of list) copyFileSync(join(dir, "shot", f), join(out, f));
  const stack = JSON.parse(readFileSync(join(dir, "aligned", "stack.json"), "utf8")) as { crop: number[]; frames: { transform: number[] }[] };
  // The aligner worked at half the camera's size (JPEG exports are halved); its transforms
  // map pixel centers, so rescale them about the center convention to the frames written.
  const camera = jpegSize(shot[0]);
  const written = jpegSize(join(out, list[0]));
  const k = written.width / (camera.width / 2);
  return {
    frames: list.length,
    width: written.width,
    height: written.height,
    transforms: stack.frames.map((fr) => rescale(fr.transform as Warp, k).map((v) => Number(v.toPrecision(7)))),
    covered: stack.crop.map((v) => Math.round(v * k * 10) / 10),
  };
}

function pair(figure: Of<"pair">, out: string): Extra {
  const frames = aligned(figure.stack);
  const { rect } = figure.place;
  crop(frames[figure.a - 1], figure.quality, [[rect, join(out, "a.jpg")]]);
  crop(frames[figure.b - 1], figure.quality, [[rect, join(out, "b.jpg")]]);
  return { a: figure.a, b: figure.b, width: rect[2], height: rect[3] };
}

function pick(figure: Of<"pick">, out: string): Extra {
  const frames = aligned(figure.stack);
  run(example("web_lab"), ["pick", out, String(figure.quality), figure.place.rect.join(","), figure.sides.join(","), ...frames]);
  // The page shows the pictures only, not the frame maps web_lab pick writes beside them.
  for (const side of figure.sides) rmSync(join(out, `w${side}-map.png`));
  return { sides: figure.sides, frames: frames.length, width: figure.place.rect[2], height: figure.place.rect[3] };
}

/** The `frames` kind: a crop of every frame. */
function framesFigure(figure: Of<"frames">, out: string): Extra {
  const frames = aligned(figure.stack);
  const { rect } = figure.place;
  frames.forEach((frame, i) => crop(frame, figure.quality, [[rect, join(out, `frame-${String(i + 1).padStart(2, "0")}.jpg`)]]));
  return { frames: frames.length, width: rect[2], height: rect[3] };
}

try {
  for (const figure of figures) make(figure);
} finally {
  if (!kept) rmSync(work, { recursive: true, force: true });
}
