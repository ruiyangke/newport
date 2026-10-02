import { createStore } from "@tanstack/react-store";

/**
 * Distraction-free terminal: the sidebar and window toolbar step aside so the
 * session owns the window. Session-only state; it is never persisted.
 */
export const immersiveStore = createStore({ active: false });

export function setImmersive(active: boolean) {
  if (immersiveStore.state.active !== active)
    immersiveStore.setState(() => ({ active }));
}
