// A <video> fires `error` at each <source> it can't play before trying the next, so an error is
// final only when the video itself failed, it gave up (NETWORK_NO_SOURCE), or the last source failed.

/** `HTMLMediaElement.NETWORK_NO_SOURCE`, a constant here so the check runs outside a browser. */
const NETWORK_NO_SOURCE = 3;

type Video = Pick<HTMLMediaElement, "error" | "networkState"> & { querySelector(selector: string): unknown };

/** Whether an `error` event from `target` (the video or one of its sources) means the video can't play. */
export function failedForGood(video: Video, target: EventTarget | null): boolean {
  return video.error !== null || video.networkState === NETWORK_NO_SOURCE || target === video.querySelector("source:last-of-type");
}

/** Calls `failed` once, when the video has no source left to try. */
export function onVideoFailed(video: HTMLVideoElement, failed: () => void): void {
  if (video.error !== null || video.networkState === NETWORK_NO_SOURCE) {
    failed();
    return;
  }
  const listener = (e: Event) => {
    if (!failedForGood(video, e.target)) return;
    video.removeEventListener("error", listener, { capture: true });
    failed();
  };
  // Capture, since a source's `error` doesn't bubble up to the video.
  video.addEventListener("error", listener, { capture: true });
}
