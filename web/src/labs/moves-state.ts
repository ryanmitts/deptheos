// The frames move figure's state. Pure, so it is tested alone; the frame
// shown is the `Timeline`'s.

/**
 * The figure's state.
 *
 * Lifecycle: one state, `ready` with the outlines (`keeps`) shown or not,
 * starting without. Only `nextMoves` changes it, on the toggle.
 */
export type MovesState = { kind: "ready"; keeps: boolean };

export type MovesEvent = { kind: "keeps"; on: boolean };

export const initialMoves: MovesState = { kind: "ready", keeps: false };

/** The state after `event`; the same object when the event changes nothing. */
export function nextMoves(state: MovesState, event: MovesEvent): MovesState {
  return event.on !== state.keeps ? { kind: "ready", keeps: event.on } : state;
}
