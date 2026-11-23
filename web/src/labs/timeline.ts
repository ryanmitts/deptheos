// A play position over a stack's frames and its play / scrub controls;
// playback holds on the last frame for `holdMs`, then loops.

export interface TimelineOptions {
  fps?: number;
  /** How long to stay on the last frame before starting again. */
  holdMs?: number;
}

/** Why a timeline paused: the reader (the button or the scrubber), or the page (the figure scrolled away). */
export type PauseCause = "reader" | "page";

/**
 * Lifecycle: paused ⇄ playing, changed only by `play` and `pause`. Playing holds the one
 * pending animation frame, so a pause cancels it and a quick replay can't start a second loop.
 */
type State = { kind: "paused" } | { kind: "playing"; raf: number };

export class Timeline {
  frame = 0;
  private state: State = { kind: "paused" };
  private listeners: ((frame: number) => void)[] = [];
  private stateListeners: ((playing: boolean, cause?: PauseCause) => void)[] = [];
  private last = 0;
  private opts: Required<TimelineOptions>;

  constructor(
    public count: number,
    options: TimelineOptions = {},
  ) {
    this.opts = { fps: 12, holdMs: 1500, ...options };
  }

  /** Called whenever the frame changes. */
  onChange(fn: (frame: number) => void): void {
    this.listeners.push(fn);
  }

  /** Called when it starts playing, and when it pauses, with why. */
  onPlayState(fn: (playing: boolean, cause?: PauseCause) => void): void {
    this.stateListeners.push(fn);
  }

  seek(frame: number): void {
    const k = Math.min(Math.max(Math.round(frame), 0), this.count - 1);
    if (k === this.frame) return;
    this.frame = k;
    this.emit();
  }

  get playing(): boolean {
    return this.state.kind === "playing";
  }

  play(): void {
    if (this.state.kind !== "paused") return;
    this.last = performance.now();
    this.state = { kind: "playing", raf: requestAnimationFrame(this.tick) };
    for (const fn of this.stateListeners) fn(true);
  }

  /** Stop playing; `cause` says who stopped it (the reader, unless said otherwise). */
  pause(cause: PauseCause = "reader"): void {
    if (this.state.kind !== "playing") return;
    cancelAnimationFrame(this.state.raf);
    this.state = { kind: "paused" };
    for (const fn of this.stateListeners) fn(false, cause);
  }

  toggle(): void {
    if (this.playing) this.pause();
    else this.play();
  }

  private tick = (now: number): void => {
    if (this.state.kind !== "playing") return;
    this.state = { kind: "playing", raf: requestAnimationFrame(this.tick) };
    const atEnd = this.frame >= this.count - 1;
    const wait = atEnd ? this.opts.holdMs : 1000 / this.opts.fps;
    if (now - this.last < wait) return;
    this.last = now;
    this.frame = atEnd ? 0 : this.frame + 1;
    this.emit();
  };

  private emit(): void {
    for (const fn of this.listeners) fn(this.frame);
  }
}

/** Play button, scrubber and frame label for `timeline`, appended to `parent`. */
export function playControls(parent: HTMLElement, timeline: Timeline, label: (frame: number) => string): void {
  const bar = document.createElement("div");
  bar.className = "play";
  const button = document.createElement("button");
  button.type = "button";
  button.className = "play-button";
  const range = document.createElement("input");
  range.type = "range";
  range.min = "0";
  range.max = String(timeline.count - 1);
  range.step = "1";
  range.setAttribute("aria-label", "Frame");
  const out = document.createElement("output");
  bar.append(button, range, out);
  parent.append(bar);

  // The label alone says what a press does (no aria-pressed, which would read "Pause, pressed").
  const showState = () => {
    button.textContent = timeline.playing ? "Pause" : "Play";
  };
  const showFrame = (k: number) => {
    range.value = String(k);
    out.value = label(k);
  };
  button.addEventListener("click", () => timeline.toggle());
  range.addEventListener("input", () => {
    timeline.pause();
    timeline.seek(Number(range.value));
  });
  timeline.onChange(showFrame);
  timeline.onPlayState(showState);
  showState();
  showFrame(timeline.frame);
}

/**
 * Play `timeline` while `el` is in view, unless the reader prefers less motion. Once the
 * reader pauses it, it no longer starts on its own but still pauses off screen.
 */
export function autoplayWhenVisible(el: HTMLElement, timeline: Timeline): void {
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  let auto = true;
  timeline.onPlayState((playing, cause) => {
    if (!playing && cause === "reader") auto = false;
  });
  const io = new IntersectionObserver(
    (entries) => {
      const visible = entries[entries.length - 1].isIntersecting;
      if (!visible) timeline.pause("page");
      else if (auto) timeline.play();
    },
    { threshold: 0.25 },
  );
  io.observe(el);
}
