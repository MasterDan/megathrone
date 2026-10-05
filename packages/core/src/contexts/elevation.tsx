import { createContext, useContext, type ParentComponent } from "solid-js";

/**
 * Elevation deltas — "height above sea level" for overlays.
 *
 * Components floating above the rest of the UI (Modal, Popover, future
 * Tooltip/Toast/ContextMenu) add their delta to the current context value
 * and use the result as their z-index. The delta step leaves "air" between
 * layers so nesting reads correctly:
 *
 *   app base                      z = 0
 *   └─ Modal (delta 1000)         z = 1000
 *      └─ Popover (delta 100)     z = 1100
 *         └─ nested Popover       z = 1200
 *
 * The modal step is intentionally larger than the popover step: between a
 * modal and its popovers there is a range for utility layers (overlay,
 * backdrop etc.) without risking a collision with a nested modal (2000).
 */
export const MODAL_DELTA = 1000;
export const POPOVER_DELTA = 100;

const ElevationContext = createContext<number>(0);

/** Current "height" — the z-index base for an overlay at this point in the tree. */
export const useElevation = (): number => useContext(ElevationContext);

/**
 * Raises the base for a child subtree by `delta`. Every Provider sums the
 * delta with the parent base — arbitrary layer nesting therefore works
 * automatically, without passing absolute values around.
 *
 * SolidJS `<Portal>` preserves ownership (see `solid-js/web` sources:
 * `runWithOwner(owner, …)`), so elevation is read correctly inside portals
 * that render overlays into `document.body`.
 */
export const ElevationProvider: ParentComponent<{ delta: number }> = (
  props,
) => {
  const value = useElevation() + props.delta;
  return (
    <ElevationContext.Provider value={value}>
      {props.children}
    </ElevationContext.Provider>
  );
};
