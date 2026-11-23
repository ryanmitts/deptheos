// A detail panel: its picture at 100% in a possibly smaller box. Touch pans
// with native scrolling (so a swipe at the edge carries on to the page); a
// mouse by dragging, the wheel or the arrow keys. Panels in a group pan
// together. Until the reader moves it, a larger picture shows a drag hint.

export class PanView {
  readonly el: HTMLDivElement;
  private group: PanView[] = [this];

  constructor(
    readonly content: HTMLElement,
    readonly width: number,
    readonly height: number,
    label: string,
  ) {
    this.el = document.createElement("div");
    this.el.className = "pan";
    this.el.tabIndex = 0;
    this.el.setAttribute("role", "group");
    this.el.setAttribute("aria-label", `${label} (drag or use the arrow keys to look around)`);
    content.style.width = `${width}px`;
    content.style.height = `${height}px`;
    const hint = document.createElement("div");
    hint.className = "pan-hint";
    hint.setAttribute("aria-hidden", "true");
    hint.append(Object.assign(document.createElement("span"), { textContent: "Drag to look around" }));
    this.el.append(hint, content);
    // The reader's first move retires the hint, in every panel of the group.
    const hintEvents = ["pointerdown", "wheel", "keydown"] as const;
    const retireHint = () => {
      this.group.forEach((v) => v.el.classList.add("panned"));
      for (const type of hintEvents) this.el.removeEventListener(type, retireHint);
    };
    for (const type of hintEvents) {
      this.el.addEventListener(type, retireHint, { passive: true });
    }
    // The hint shows only where there is more picture than box.
    new ResizeObserver(() => {
      // A sliver more is not worth a hint.
      const over = this.el.scrollWidth > this.el.clientWidth + 32 || this.el.scrollHeight > this.el.clientHeight + 32;
      this.el.classList.toggle("can-pan", over);
    }).observe(this.el);
    this.el.addEventListener("scroll", () => this.sync(), { passive: true });
    dragToPan(this.el);
  }

  /** Pan these panels together. */
  static link(views: PanView[]): void {
    for (const v of views) v.group = views;
  }

  /** Bring the image point (x, y) to the middle of the box. */
  centerOn(x: number, y: number): void {
    this.el.scrollTo({ left: x - this.el.clientWidth / 2, top: y - this.el.clientHeight / 2 });
  }

  private sync(): void {
    const left = this.el.scrollLeft;
    const top = this.el.scrollTop;
    for (const other of this.group) {
      if (other === this) continue;
      if (other.el.scrollLeft !== left) other.el.scrollLeft = left;
      if (other.el.scrollTop !== top) other.el.scrollTop = top;
    }
  }
}

/** Pan `el` by dragging with the mouse's main button, marked `.dragging` meanwhile; buttons inside keep their presses. */
export function dragToPan(el: HTMLElement): void {
  let from: { id: number; x: number; y: number; left: number; top: number } | undefined;
  el.addEventListener("pointerdown", (e) => {
    if (e.pointerType !== "mouse" || e.button !== 0 || (e.target instanceof Element && e.target.closest("button"))) return;
    from = { id: e.pointerId, x: e.clientX, y: e.clientY, left: el.scrollLeft, top: el.scrollTop };
    el.setPointerCapture(e.pointerId);
    el.classList.add("dragging");
    e.preventDefault();
  });
  el.addEventListener("pointermove", (e) => {
    if (!from || e.pointerId !== from.id) return;
    el.scrollLeft = from.left - (e.clientX - from.x);
    el.scrollTop = from.top - (e.clientY - from.y);
  });
  const end = (e: PointerEvent) => {
    if (from && e.pointerId !== from.id) return;
    from = undefined;
    el.classList.remove("dragging");
  };
  el.addEventListener("pointerup", end);
  el.addEventListener("pointercancel", end);
  el.addEventListener("lostpointercapture", end);
}
