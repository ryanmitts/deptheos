// What every docs figure's images are made from: the source frames,
// the place cut out of each result (in pixels of the stack's aligned
// frames) and why that place, and how each image is rendered.
// `scripts/make-lab-assets.ts` reads this and writes `public/media/labs/<id>/`.
// A figure's entry here and its images change together.
//
// Each stack is aligned once at camera resolution with the app's ECC
// aligner (`web_sample_raw --full-size`), and every render runs the CLI on
// those aligned frames with `--align none`. So every figure of a stack shares
// one coordinate frame: the depth map lines up with the frames.

/** x, y, width, height in pixels of the stack's aligned frames. */
export type Rect = [number, number, number, number];

export interface Stack {
  /** The folder of frames, relative to the originals' root (`LAB_SOURCES`, default `~/Desktop/sample_images`). */
  folder: string;
  /** Which frames of the folder (1-based, inclusive), in name order; all of them if absent. */
  range?: [number, number];
  /** Where the frames came from and why this set. */
  about: string;
}

export const STACKS = {
  hornet: {
    folder: "jpeg/hornet",
    about: "A bald-faced hornet on a branch against green card: 66 JPEG exports of the camera's raw files, 8192×5464, the whole focus range.",
  },
  spider: {
    folder: "jpeg/canon1",
    about: "A jumping spider: 46 edited JPEG exports, 8192×5464.",
  },
  pcb: {
    folder: "jpeg/pcb1",
    range: [79, 154],
    about: "A circuit board with solder and black capacitors: frames 79 to 154 of 200, the part of the focus range that holds the subject.",
  },
} satisfies Record<string, Stack>;

export type StackId = keyof typeof STACKS;

/** One rectangle cut from every image of a figure. */
export interface Place {
  name: string;
  rect: Rect;
  /** Why here: what the reader sees at this place. */
  why: string;
  /** The stack this place is cut from, when not the figure's (a renders figure only). */
  stack?: StackId;
}

/** One image the app renders: a CLI stacking command on the aligned frames. */
export interface Render {
  name: string;
  method: "pyramid" | "depth" | "wavelet";
  /** CLI settings, e.g. `["--radius", "50"]`. */
  args?: string[];
  /** Stack every Nth frame only (1, 1+N, …). */
  every?: number;
  /** Also cut the depth map (depth method only, made by `web_lab depthmap` with the render's Radius), as `<render>-<place>-map.png`: the frame nearest each pixel, as a gray level (0 is frame 1). */
  depthMap?: boolean;
  /** Cut only the depth map, not the result: for a figure that shows the map beside another figure's frames. */
  mapOnly?: boolean;
  /** Also cut the merged gray picture (the default wavelet only, made by `web_lab merged`), as `<render>-<place>-grey.jpg`: the sharp gray target the colors are then matched to. */
  mergedMap?: boolean;
}

export type Figure =
  /** Crops of app renders: `<render>-<place>.jpg` for every render and place. */
  | { kind: "renders"; id: string; stack: StackId; quality: number; places: Place[]; renders: Render[] }
  /** Every aligned frame, cut at one place: `frame-NN.jpg`. */
  | { kind: "frames"; id: string; stack: StackId; quality: number; place: Place }
  /**
   * The per-pixel pick, which no app method does, made by `web_lab pick`
   * from the aligned frames: `w<side>.jpg` for each square side (the frame
   * maps `web_lab pick` also writes, `w<side>-map.png`, are not kept).
   */
  | { kind: "pick"; id: string; stack: StackId; quality: number; place: Place; sides: number[] }
  /** The frames as shot, whole, `frame-NN.jpg`, and the app's alignment of them: every frame's transform from frame 1 and the area every frame covers (`#moves`). */
  | { kind: "moves"; id: string; stack: StackId; width: number; quality: number }
  /** Two aligned frames cut at one place, `a.jpg` and `b.jpg`, for a figure that computes with them in the browser. */
  | { kind: "pair"; id: string; stack: StackId; quality: number; place: Place; a: number; b: number };

export const FIGURES: Figure[] = [
  {
    kind: "frames",
    // The depth-map figure's frames (`data-frames-from="sharpest"`).
    id: "sharpest",
    stack: "hornet",
    quality: 85,
    place: {
      name: "head",
      rect: [3880, 1160, 768, 768],
      why: "The right side of the head: hairs over the plain green, the rim of the eye, and the antenna crossing in front of the hairy head, each at a different depth.",
    },
  },
  {
    kind: "pick",
    id: "pick",
    stack: "hornet",
    quality: 88,
    place: {
      name: "head",
      rect: [3880, 1160, 768, 768],
      why: "The same place as the depth-map figure. At 1 px the plain green turns grainy beside the hairs; at 64 px the squares leave a seam across the green and a blurred halo round the hairs.",
    },
    sides: [1, 8, 64],
  },
  {
    kind: "pair",
    id: "mixer",
    stack: "hornet",
    quality: 95,
    place: {
      name: "leg",
      rect: [4600, 2350, 512, 384],
      why: "The pale front leg against the dark hairy body. In frame 40 the leg's edge is crisp; in frame 5 the leg is a pale blur that spills over the dark band beside it, so taking the coarse levels from frame 5 visibly brings the spill in.",
    },
    a: 40,
    b: 5,
  },
  {
    kind: "renders",
    id: "smoothness",
    stack: "hornet",
    quality: 90,
    places: [
      {
        name: "antenna",
        rect: [5200, 1500, 768, 512],
        why: "The tip of an antenna against the plain green, where Smoothness 1 and 10 differ most on the hornet (7 to 8 gray levels on average over 256 px tiles): at 1 the green is grainy, at 10 it is calm and the tip a touch softer. The circuit board's largest differences were in a featureless blur.",
      },
      {
        name: "leg",
        stack: "spider",
        rect: [4560, 1500, 768, 512],
        why: "The methods figure's spider leg, pale against a dark background: where the pyramid's glow shows.",
      },
    ],
    renders: [1, 4, 10].map((s) => ({ name: `s${s}`, method: "pyramid" as const, args: ["--smoothness", String(s)] })),
  },
  {
    kind: "renders",
    id: "radius",
    stack: "hornet",
    quality: 88,
    places: [
      {
        name: "hairs",
        rect: [3234, 918, 1024, 768],
        why: "Hairs on the top of the head against the plain green: Radius 50 softens them and fades them into the background; Radius 1 leaves them crisp but speckles the depth map.",
      },
    ],
    renders: [1, 8, 50].map((r) => ({ name: `r${r}`, method: "depth" as const, args: ["--radius", String(r)], depthMap: true })),
  },
  {
    // The depth-map figure shows this beside the sharpest folder's frames of the same place, so it ships only the map.
    kind: "renders",
    id: "depthmap",
    stack: "hornet",
    quality: 88,
    places: [
      {
        name: "head",
        rect: [3880, 1160, 768, 768],
        why: "The place of the sharpest frames, which this figure shows: the eye, the hairy head behind it, the antenna crossing in front, and the plain green, each a different depth.",
      },
    ],
    renders: [{ name: "default", method: "depth", depthMap: true, mapOnly: true }],
  },
  { kind: "moves", id: "moves", stack: "spider", width: 1024, quality: 90 },
  {
    kind: "renders",
    id: "methods",
    stack: "hornet",
    quality: 88,
    places: [
      {
        name: "hairs",
        rect: [3434, 1068, 768, 512],
        why: "Hairs on the top of the head against the green: crisp in the pyramid, softer with a haze between them in the depth map, crisp in the wavelet.",
      },
      {
        name: "green",
        rect: [5632, 2304, 768, 512],
        why: "Plain out-of-focus green with a shadow: grainy in the pyramid and the wavelet, smooth in the depth map, the methods' largest difference on the hornet (9.4 gray levels over 512 px tiles).",
      },
      {
        name: "antenna",
        rect: [5120, 1536, 768, 512],
        why: "The tip of an antenna: crisp in the pyramid and the depth map; in the wavelet its lower half is washed pale, filters on or off (the largest wavelet-against-pyramid difference on the hornet).",
      },
      {
        name: "leg",
        stack: "spider",
        rect: [4560, 1500, 768, 512],
        why: "A spider's pale leg against the dark, out-of-focus background: in the pyramid the dark beside the leg is lifted to a haze, the leg's out-of-focus glow; the depth map keeps it dark; the wavelet is blotchy there, with an orange fringe along the leg.",
      },
      {
        name: "board",
        stack: "pcb",
        rect: [768, 3328, 768, 512],
        why: "Solder beside a capacitor, at a jump in depth: crisp in the pyramid, softened at the capacitor's edge in the depth map, gray streaks into the black in the wavelet (the largest wavelet-against-pyramid difference on the board).",
      },
    ],
    renders: [
      { name: "pyramid", method: "pyramid" },
      { name: "depth", method: "depth" },
      { name: "wavelet", method: "wavelet" },
      { name: "wavelet-c2", method: "wavelet", args: ["--consistency", "2"] },
    ],
  },
  {
    kind: "renders",
    id: "wavelet",
    stack: "hornet",
    quality: 90,
    places: [
      {
        name: "hairs",
        rect: [3434, 1068, 768, 512],
        why: "Hairs on the top of the head against the green, the first place of the methods figure: in the merged gray picture every hair is sharp at once; in the color result the hairs stay sharp and the plain green is grainy.",
      },
    ],
    renders: [{ name: "wavelet", method: "wavelet", mergedMap: true }],
  },
];
