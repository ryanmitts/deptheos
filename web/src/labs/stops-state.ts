// The stops figure's state. Pure, so it is tested alone.

/**
 * The figure's state.
 *
 * Lifecycle: one state, `ready` on a stop and a place, starting where the
 * figure (or a link) names. Only `nextStops` changes it, on the reader's choice.
 */
export type StopsState = { kind: "ready"; stop: number; place: number };

export type StopsEvent = { kind: "stop"; stop: number } | { kind: "place"; place: number };

/** The state after `event`; the same object when the event changes nothing. */
export function nextStops(state: StopsState, event: StopsEvent): StopsState {
  switch (event.kind) {
    case "stop":
      return event.stop === state.stop ? state : { ...state, stop: event.stop };
    case "place":
      return event.place === state.place ? state : { ...state, place: event.place };
  }
}
