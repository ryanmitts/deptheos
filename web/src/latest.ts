// Async work where only the latest start counts: each `run` aborts the one
// before through its AbortSignal, and a superseded result is never applied.
// `stop` is for teardown.

export interface Latest {
  /**
   * Aborts the work in flight and starts `work`. Calls `apply` with its result
   * unless a later `run`, `cancel` or `stop` came first; then its result, or
   * its rejection, is dropped. Other rejections pass through.
   */
  run<T>(work: (signal: AbortSignal) => Promise<T>, apply: (value: T) => void): Promise<void>;
  /** Aborts the work in flight, if any; later runs start as usual. */
  cancel(): void;
  /** Aborts the work in flight, and later runs do nothing. */
  stop(): void;
  readonly stopped: boolean;
}

export function latest(): Latest {
  let current: AbortController | undefined;
  let stopped = false;
  const cancel = () => {
    current?.abort();
    current = undefined;
  };
  return {
    async run(work, apply) {
      cancel();
      if (stopped) return;
      const controller = new AbortController();
      current = controller;
      const { signal } = controller;
      let value;
      try {
        value = await work(signal);
      } catch (e) {
        if (signal.aborted) return;
        throw e;
      } finally {
        if (current === controller) current = undefined;
      }
      if (!signal.aborted) apply(value);
    },
    cancel,
    stop() {
      stopped = true;
      cancel();
    },
    get stopped() {
      return stopped;
    },
  };
}
