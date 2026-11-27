//! Asks before in-app navigation would throw away unsaved work.

type Guard = () => boolean;
let guard: Guard | null = null;

/** While set, `mayLeave` asks the guard; returns a function that removes it. */
export function setLeaveGuard(g: Guard): () => void {
  guard = g;
  return () => {
    if (guard === g) guard = null;
  };
}

/** True when leaving the current view is fine (nothing unsaved, or the user agreed). */
export function mayLeave(): boolean {
  return !guard || guard();
}
