import type { Accessor, ParentComponent } from "solid-js";
import { createContext, useContext } from "solid-js";

const ScrollZoneContext = createContext<Accessor<HTMLElement | null>>(() => null);

export const ScrollZoneProvider: ParentComponent<{ scroller: Accessor<HTMLElement | null> }> = (
  props,
) => (
  <ScrollZoneContext.Provider value={props.scroller}>{props.children}</ScrollZoneContext.Provider>
);

/**
 * The scrollable ancestor of the current content zone — the desktop layout's
 * rounded frame with its own scrollbar. `null` outside a zone: content
 * scrolls the document window (the mobile layout). Scroll-position-aware
 * components (the virtualized grid) read this to pick their scroll source.
 */
export const useScrollZone = (): Accessor<HTMLElement | null> => useContext(ScrollZoneContext);
