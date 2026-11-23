/**
 * Every gallery stack, driving both /gallery/ and /gallery/<slug>/.
 * Adding a new stack only requires adding an entry here (and running scripts/make-gallery.sh).
 */

export interface GallerySampleLink {
  label: string;
  href: string;
}

export interface GalleryItem {
  slug: string;
  name: string;
  pageTitle: string;
  description: string;
  lede: string;
  alt: string;
  zoomLabel: string;
  /** Pixel dimensions of the full-resolution tiled image ("WxH"). */
  fullSize: string;
  /** Dimensions of the 2400px-wide display image. */
  width2400: number;
  height2400: number;
  /** Dimensions of the 900px-wide index card thumbnail. */
  cardWidth: number;
  cardHeight: number;
  /** Dimensions of the single-frame comparison image (defaults to width2400 x height2400). */
  compareHeight?: number;
  /** Dimensions of the grow sequence video. */
  videoWidth: number;
  videoHeight: number;
  videoLabel: string;
  /** Short EXIF chips shown on the /gallery/ index card. */
  cardFrames: string;
  cardMethod: string;
  /** Frame count used in the comparison section ("all N stacked"). */
  frameCount: number;
  /** Facts displayed in the "How it was shot and stacked" definition list. */
  facts: {
    frames: string;
    camera: string;
    lens: string;
    exposure: string;
    method: string;
    alignedWith: string;
    result: string;
  };
  /** Optional benchmark key under measured/benchmarks/ to display stacking time. */
  benchmarkKey?: string;
  /** Optional downloadable sample links shown as a fine-print line below facts. */
  sampleFine?: GallerySampleLink[];
  /** Optional downloadable sample links shown as a row inside the facts list. */
  sampleFact?: GallerySampleLink[];
}

export const GALLERY_ITEMS: readonly GalleryItem[] = [
  {
    slug: "hornet",
    name: "Hornet",
    pageTitle: "Hornet: a 66-frame focus stack",
    description:
      "A bald-faced hornet on a branch, in 66 frames shot handheld. See it at 100%, compare one frame with the stack, and watch the stack build.",
    lede: "A bald-faced hornet on a branch, in 66 frames shot handheld. Click the picture to look around it at 100%.",
    alt: "A hornet's face on a branch, sharp from the antennae to the jaw",
    zoomLabel: "See the hornet at 100%",
    fullSize: "8138x5449",
    width2400: 2400,
    height2400: 1607,
    cardWidth: 900,
    cardHeight: 602,
    videoWidth: 1920,
    videoHeight: 1286,
    videoLabel: "The hornet coming into focus as each frame is stacked, from the back",
    cardFrames: "66 frames, handheld",
    cardMethod: "Pyramid",
    frameCount: 66,
    facts: {
      frames: "66 JPEG, handheld",
      camera: "Canon EOS R5 Mark II",
      lens: "RF 100 mm f/2.8 L Macro",
      exposure: "f/8 · 1/125 s · ISO 200",
      method: "Pyramid, the default settings",
      alignedWith: "ECC",
      result: "8,138 × 5,449 px",
    },
    benchmarkKey: "m5-max-hornet-jpeg",
    sampleFine: [
      { label: "JPEG (264 MB)", href: "https://images.deptheos.com/samples/hornet-jpeg.zip" },
      { label: "CR3 (1.1 GB)", href: "https://images.deptheos.com/samples/hornet-cr3.zip" },
    ],
  },
  {
    slug: "pcb",
    name: "Circuit board",
    pageTitle: "Circuit board: a 200-frame focus stack",
    description:
      "Capacitors and solder on a circuit board, in 200 frames from a tripod. See it at 100%, compare one frame with the stack, and watch the stack build.",
    lede: "Capacitors and solder on a circuit board, in 200 frames from a tripod. Click the picture to look around it at 100%.",
    alt: "Black capacitors and rows of small parts on a circuit board, all in focus",
    zoomLabel: "See the board at 100%",
    fullSize: "8192x5464",
    width2400: 2400,
    height2400: 1600,
    cardWidth: 900,
    cardHeight: 600,
    videoWidth: 1920,
    videoHeight: 1280,
    videoLabel: "The board coming into focus as each frame is stacked, from the back",
    cardFrames: "200 frames, tripod",
    cardMethod: "Pyramid",
    frameCount: 200,
    facts: {
      frames: "200 JPEG, on a tripod",
      camera: "Canon EOS R5 Mark II",
      lens: "RF 100 mm f/2.8 L Macro",
      exposure: "f/8 · 1/125 s · ISO 200",
      method: "Pyramid, the default settings",
      alignedWith: "ECC",
      result: "8,192 × 5,464 px",
    },
    benchmarkKey: "m5-max-pcb1-jpeg",
    sampleFine: [
      { label: "JPEG (412 MB)", href: "https://images.deptheos.com/samples/pcb-jpeg.zip" },
      { label: "CR3 (3.1 GB)", href: "https://images.deptheos.com/samples/pcb-cr3.zip" },
    ],
  },
  {
    slug: "spider",
    name: "Spider",
    pageTitle: "Spider: a 46-frame focus stack",
    description:
      "A spider on the edge of a leaf, in 46 frames. See it at 100%, compare one frame with the stack, and watch the stack build.",
    lede: "A spider on the edge of a leaf, in 46 frames. Click the picture to look around it at 100%.",
    alt: "A brown spider facing the camera on the edge of a dark leaf, sharp from its front legs to its body",
    zoomLabel: "See the spider at 100%",
    fullSize: "7853x4619",
    width2400: 2400,
    height2400: 1412,
    cardWidth: 900,
    cardHeight: 530,
    videoWidth: 1920,
    videoHeight: 1130,
    videoLabel: "The spider coming into focus as each frame is stacked, from the back",
    cardFrames: "46 frames",
    cardMethod: "Pyramid",
    frameCount: 46,
    facts: {
      frames: "46 JPEG",
      camera: "Canon EOS R5 Mark II",
      lens: "RF 100 mm f/2.8 L Macro",
      exposure: "f/8 · 1/125 s · ISO 200",
      method: "Pyramid, the default settings",
      alignedWith: "ECC",
      result: "7,853 × 4,619 px",
    },
    sampleFact: [
      { label: "JPEG (250 MB)", href: "https://images.deptheos.com/spider1-jpeg.zip" },
      { label: "CR3 (711 MB)", href: "https://images.deptheos.com/spider1-cr3.zip" },
    ],
  },
  {
    slug: "om-hornet",
    name: "Hornet, head-on",
    pageTitle: "Hornet, head-on: a 63-frame focus stack",
    description:
      "A bald-faced hornet face to face on a branch, in 63 RAW frames from an OM System OM-1. See it at 100%, compare one frame with the stack, and watch the stack build.",
    lede: "A bald-faced hornet face to face on a branch, in 63 RAW frames. Click the picture to look around it at 100%.",
    alt: "A bald-faced hornet face to face on a branch, sharp from its antennae to its claws",
    zoomLabel: "See the hornet at 100%",
    fullSize: "5100x3841",
    width2400: 2400,
    height2400: 1808,
    cardWidth: 900,
    cardHeight: 678,
    videoWidth: 1920,
    videoHeight: 1446,
    videoLabel: "The hornet coming into focus as each frame is stacked, from the back",
    cardFrames: "63 RAW frames",
    cardMethod: "Pyramid",
    frameCount: 63,
    facts: {
      frames: "63 RAW (ORF)",
      camera: "OM System OM-1",
      lens: "M.Zuiko 90 mm f/3.5 Macro",
      exposure: "f/8 · 1/100 s · ISO 200",
      method: "Pyramid, the default settings",
      alignedWith: "ECC",
      result: "5,100 × 3,841 px",
    },
  },
  {
    slug: "screwdriver",
    name: "Screwdriver",
    pageTitle: "Screwdriver: a 250-frame focus stack",
    description:
      "The knurled grip of a screwdriver, in 250 RAW frames from an OM System OM-1. See it at 100%, compare one frame with the stack, and watch the stack build.",
    lede: "The knurled grip of a screwdriver, in 250 RAW frames. Click the picture to look around it at 100%.",
    alt: "The knurled steel grip of a screwdriver: rows of small pyramids against a dark red background, all in focus",
    zoomLabel: "See the screwdriver at 100%",
    fullSize: "5207x3911",
    width2400: 2400,
    height2400: 1802,
    cardWidth: 900,
    cardHeight: 676,
    compareHeight: 1803,
    videoWidth: 1920,
    videoHeight: 1442,
    videoLabel: "The screwdriver coming into focus as each frame is stacked, from the back",
    cardFrames: "250 RAW frames",
    cardMethod: "Pyramid",
    frameCount: 250,
    facts: {
      frames: "250 RAW (ORF)",
      camera: "OM System OM-1",
      lens: "M.Zuiko 90 mm f/3.5 Macro with the MC-20 teleconverter, at 180 mm",
      exposure: "f/11 · 1/100 s · ISO 200",
      method: "Pyramid, the default settings",
      alignedWith: "ECC",
      result: "5,207 × 3,911 px",
    },
  },
];
