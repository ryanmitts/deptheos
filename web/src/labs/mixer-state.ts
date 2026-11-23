// The level mixer's state: five switches, each taking one level from frame A
// (sharp) or B (blurred). Pure, so it is tested alone.

import { DETAIL_LEVELS } from "./mixer-math.ts";

/** Every level from A: frame A itself. */
export const ALL_A: boolean[] = Array(DETAIL_LEVELS + 1).fill(true);
/** Every level from B: frame B itself. */
export const ALL_B: boolean[] = ALL_A.map(() => false);
/** The three finest levels from A, the coarser two from B. */
export const FINE_FROM_A: boolean[] = [true, true, true, false, false];
/** The reverse. */
export const COARSE_FROM_A: boolean[] = FINE_FROM_A.map((x) => !x);

/**
 * The figure's state.
 *
 * Lifecycle: one state, `ready` with each level's source, starting all B.
 * Only `nextMixer` changes it: `take` sets one level, `set` them all.
 */
export type MixerState = { kind: "ready"; fromA: boolean[] };

export type MixerEvent = { kind: "take"; level: number; fromA: boolean } | { kind: "set"; fromA: boolean[] };

export const initialMixer: MixerState = { kind: "ready", fromA: ALL_B };

const same = (a: boolean[], b: boolean[]) => a.every((x, i) => x === b[i]);

/** The state after `event`; the same object when the event changes nothing. */
export function nextMixer(state: MixerState, event: MixerEvent): MixerState {
  switch (event.kind) {
    case "take":
      return state.fromA[event.level] === event.fromA ? state : { kind: "ready", fromA: state.fromA.map((x, i) => (i === event.level ? event.fromA : x)) };
    case "set":
      return same(state.fromA, event.fromA) ? state : { kind: "ready", fromA: [...event.fromA] };
  }
}
