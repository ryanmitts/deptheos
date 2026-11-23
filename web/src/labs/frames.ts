// Frame maps: which frame each pixel came from, stored as gray levels
// (0 = the first frame) and drawn in the site's frame colors, turbo from
// dark blue at the first frame to dark red at the last.

/** Where the frame colors start along turbo (as `RAMP_START` in the viewer): its first few percent are nearly black. */
const RAMP_START = 0.08;

/** Turbo, the polynomial fit of view.wgsl, as 0..255 RGB. */
function turbo(t: number): [number, number, number] {
  const x = Math.min(Math.max(t, 0), 1);
  const [x2, x3] = [x * x, x * x * x];
  const [x4, x5] = [x3 * x, x3 * x2];
  const f = (c: number[]) => Math.round(255 * Math.min(Math.max(c[0] + c[1] * x + c[2] * x2 + c[3] * x3 + c[4] * x4 + c[5] * x5, 0), 1));
  return [
    f([0.13572138, 4.6153926, -42.66032258, 132.13108234, -152.94239396, 59.28637943]),
    f([0.09140261, 2.19418839, 4.84296658, -14.18503333, 4.27729857, 2.82956604]),
    f([0.1066733, 12.64194608, -60.58204836, 110.36276771, -89.90310912, 27.34824973]),
  ];
}

export const frameColor = (frame: number, frames: number) => turbo(RAMP_START + ((1 - RAMP_START) * frame) / Math.max(frames - 1, 1));

/** A frame map's numbers: the frame (0 = the first) at each pixel, row by row. */
export interface FrameIndex {
  width: number;
  height: number;
  index: Uint8Array;
}

/** Load a frame map stored as frame numbers in gray levels (a PNG from `scripts/make-lab-assets.ts`). */
export async function frameIndex(url: string, signal?: AbortSignal): Promise<FrameIndex> {
  const res = await fetch(url, { signal });
  if (!res.ok) throw new Error(`an image of the figure did not load: ${url}`);
  // No color conversion: the gray levels are numbers, not colors.
  const bitmap = await createImageBitmap(await res.blob(), { colorSpaceConversion: "none", premultiplyAlpha: "none" });
  try {
    const canvas = document.createElement("canvas");
    canvas.width = bitmap.width;
    canvas.height = bitmap.height;
    const ctx = canvas.getContext("2d", { willReadFrequently: true })!;
    ctx.drawImage(bitmap, 0, 0);
    const rgba = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
    const index = new Uint8Array(canvas.width * canvas.height);
    for (let i = 0; i < index.length; i++) index[i] = rgba[i * 4];
    return { width: canvas.width, height: canvas.height, index };
  } finally {
    bitmap.close();
  }
}

/** Each frame's color as a lookup table, for `frames` frames. */
export const frameLut = (frames: number) => Array.from({ length: 256 }, (_, i) => frameColor(Math.min(i, frames - 1), frames));

/** A frame map drawn in the frame colors. */
export async function frameMap(url: string, frames: number, signal?: AbortSignal): Promise<HTMLCanvasElement> {
  const { width, height, index } = await frameIndex(url, signal);
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d")!;
  const pixels = ctx.createImageData(width, height);
  const lut = frameLut(frames);
  const d = pixels.data;
  for (let i = 0; i < index.length; i++) {
    const [r, g, b] = lut[index[i]];
    d[i * 4] = r;
    d[i * 4 + 1] = g;
    d[i * 4 + 2] = b;
    d[i * 4 + 3] = 255;
  }
  ctx.putImageData(pixels, 0, 0);
  return canvas;
}

/** The key under a frame map: the colors from the first frame to the last. */
export function frameKey(frames: number): HTMLElement {
  const key = document.createElement("div");
  key.className = "frame-key";
  const stopsCss = Array.from({ length: 9 }, (_, i) => {
    const [r, g, b] = frameColor((i / 8) * (frames - 1), frames);
    return `rgb(${r} ${g} ${b}) ${(i / 8) * 100}%`;
  });
  key.innerHTML = `<span>Frame 1</span><i style="background: linear-gradient(to right, ${stopsCss.join(", ")})"></i><span>Frame ${frames}</span>`;
  return key;
}
