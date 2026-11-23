// The depth map figure's state. Pure, so it is tested alone.

/**
 * The figure's state.
 *
 * Lifecycle: one state, `ready` on a frame, starting on the first. Only
 * `nextDepthMap` changes it, on the slider or a tap.
 */
export type DepthMapState = { kind: "ready"; frame: number };

export type DepthMapEvent = { kind: "frame"; frame: number };

export const initialDepthMap: DepthMapState = { kind: "ready", frame: 0 };

/** The state after `event`; the same object when the event changes nothing. */
export function nextDepthMap(state: DepthMapState, event: DepthMapEvent): DepthMapState {
  return event.frame === state.frame ? state : { kind: "ready", frame: event.frame };
}

/** Whether a pixel at `depth` lights up on `frame`: within one frame of it, as the depth blend shares a spot between neighbors. */
export const lit = (depth: number, frame: number) => Math.abs(depth - frame) <= 1;
