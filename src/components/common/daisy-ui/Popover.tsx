import { makeEventListener } from "@solid-primitives/event-listener";
import { createResizeObserver } from "@solid-primitives/resize-observer";
import {
  ElevationProvider,
  POPOVER_DELTA,
  useElevation,
} from "@/contexts/elevation";
import { registerOverlayEscape } from "@/utils/overlayEscape";
import {
  Show,
  createContext,
  createEffect,
  createSignal,
  onCleanup,
  useContext,
  type Accessor,
  type JSXElement,
} from "solid-js";
import { Portal } from "solid-js/web";
import { TransitionScale } from "../transitions/TransitionScale";

export type PopoverPlacement = "bottom" | "top";

const GAP = 4;
const DEFAULT_PLACEMENT: PopoverPlacement = "bottom";

/** Click point (pointer coordinates at open time). */
interface PointerPoint {
  x: number;
  y: number;
}

/** Activator rectangle — all the panel positioning needs. */
interface AnchorRect {
  top: number;
  bottom: number;
  left: number;
  width: number;
}

/** All ancestors of the element that scroll themselves (overflow auto/scroll). */
const scrollableAncestors = (el: Element): Element[] => {
  const result: Element[] = [];
  for (let node = el.parentElement; node; node = node.parentElement) {
    const { overflowX, overflowY } = window.getComputedStyle(node);
    if (
      overflowX === "auto" ||
      overflowX === "scroll" ||
      overflowY === "auto" ||
      overflowY === "scroll"
    ) {
      result.push(node);
    }
  }
  return result;
};

/**
 * Popover nesting context.
 *
 * Every `Popover` panel goes into `<Portal mount={document.body}>`, so in the
 * DOM a child popover's panel is a *sibling* of the parent's panel, not its
 * descendant. Because of that the parent's `panel.contains(target)` check
 * misses clicks on the nested panel and closes immediately — cascading the
 * whole chain down.
 *
 * Fix: each `Popover` registers its panel in the nearest popover ancestor's
 * context (`registerInside`), and the ancestor counts it as "inside" during
 * the click-outside check. Registration is forwarded up the whole popover
 * chain, so a click into a panel of any depth counts as "inside" for every
 * one of its ancestors.
 *
 * Descendant activators don't need registration: they physically render
 * inside the parent's panel (same place as its `children`), so
 * `panel.contains(activatorDescendant)` is already true.
 */
export interface PopoverInsideContextValue {
  /**
   * Register an element accessor (usually a nested popover's panel) as
   * "inside" — clicks on it must not close the ancestor. Returns an
   * unregister function.
   */
  registerInside: (el: Accessor<Element | undefined>) => () => void;
}

const PopoverInsideContext = createContext<PopoverInsideContextValue>();

/**
 * API handed to the activator. Similar to the `activator` slot of v-menu
 * (Vuetify): the activator is a render function receiving the state controls
 * and a ref for positioning.
 */
export interface PopoverActivatorApi {
  isOpen: Accessor<boolean>;
  /**
   * Open, capturing the anchor right now — in the click handler. For a
   * static/sticky button the rect at click time is the truth; whatever
   * happens to the layout afterwards does not affect the open position. The
   * click point is a fallback for when the rect is unavailable.
   */
  show: (point?: PointerPoint) => void;
  hide: () => void;
  toggle: (point?: PointerPoint) => void;
  /** Bind to the activator's root element — needed for positioning. */
  ref: (el: Element) => void;
  /** Spread onto a plain click-toggle activator (a button etc.). */
  activatorProps: {
    ref: (el: Element) => void;
    onClick: (e: MouseEvent) => void;
  };
}

export interface PopoverProps {
  /** Controlled state. Without onOpenChange — uncontrolled. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** Preferred side. Auto-flips when there is not enough room. */
  placement?: PopoverPlacement;
  contentClass?: string;
  /**
   * Match the panel width to the activator width (dropdown mode, e.g.
   * `ComboBox`). By default the panel is content-sized — the common case of a
   * small activator (an icon) with a large panel.
   */
  matchActivatorWidth?: boolean;
  /**
   * Follow the activator on scroll/resize (default: yes). Disable
   * (`false`) when the activator is known to be stationary — e.g. a button in
   * a sticky table header while only the table body scrolls: the position
   * captured at open time is never recalculated and cannot be "spoiled" by
   * scrolling.
   */
  followScroll?: boolean;
  activator: (api: PopoverActivatorApi) => JSXElement;
  children: JSXElement;
}

/**
 * Popover above everything (portal + position:fixed), no arrow tip. DaisyUI
 * `dropdown`/`dropdown-content` classes are used for theming (shadows,
 * rounding); the z-index does not come from DaisyUI but from the elevation
 * context (`useElevation() + POPOVER_DELTA`) — letting overlays nest
 * correctly into each other and into Modal. The positioning mechanics are
 * ours: plain CSS `.dropdown` gets clipped by `.card` overflow, and DaisyUI's
 * `[popover]` mode relies on CSS Anchor Positioning with unstable WebView
 * (WKWebView/WebView2) support — hence a portal with manual
 * `getBoundingClientRect`.
 *
 * The panel anchor is the activator rect captured **at open time**
 * (synchronously in the click handler): for a static/sticky button it never
 * changes, and the panel must open right at the button regardless of layout
 * effects after the click (WebKit can report stale rects for sticky
 * elements). When the rect is unavailable (activator detached/degenerate) we
 * anchor to the click point, and if that is missing too — close. Afterward
 * the anchor is re-read only on explicit events: scroll (with rAF deferral —
 * see the effect below) and resize; if the activator leaves the viewport
 * entirely, the popover closes. The final position is hard-clamped into the
 * viewport (the panel never sticks out of the window). Appear/disappear is
 * animated (opacity + scale), the scale growing from the edge nearest the
 * activator.
 */
export function Popover(props: PopoverProps): JSXElement {
  const isControlled = () => props.onOpenChange !== undefined;
  const [internal, setInternal] = createSignal(false);
  const isOpen = () => (isControlled() ? !!props.open : internal());
  const setOpen = (v: boolean) => {
    if (isControlled()) props.onOpenChange?.(v);
    else setInternal(v);
  };

  let activatorEl: Element | undefined;
  let panelEl: HTMLDivElement | undefined;
  let lastPanelHeight = 0;
  let lastPanelWidth = 0;
  /**
   * Anchor captured at open time (in the click handler). For a
   * static/sticky button the window-relative rect does not change — the panel
   * must open right at it, whatever happens to the layout after the click.
   * Re-read only on explicit events (scroll/resize) in `followActivator`;
   * reset on close.
   */
  let anchorRect: AnchorRect | undefined;
  /** Click point — fallback for when the activator rect is unavailable/degenerate. */
  let pointerPoint: PointerPoint | undefined;

  /**
   * Raw activator rect; undefined only when the element is detached.
   * Zero sizes are NOT filtered: in layout-less environments (jsdom) the rect
   * is always zero, and for programmatic opens (no click point) it is still
   * the best anchor available.
   */
  const liveAnchorRect = (): AnchorRect | undefined => {
    if (!activatorEl?.isConnected) return undefined;
    const r = activatorEl.getBoundingClientRect();
    return { top: r.top, bottom: r.bottom, left: r.left, width: r.width };
  };

  /**
   * Empty rect — zero sizes. A visible button is never zero, so in real UI
   * this means a junk rect (WebKit reports those for sticky elements after
   * scrolling), not a real position.
   */
  const isEmptyRect = (a: AnchorRect): boolean =>
    a.width === 0 || a.bottom <= a.top;

  /** Click visually inside the activator rect (with tolerance) — else the rect is false. */
  const pointOnRect = (a: AnchorRect, p: PointerPoint): boolean =>
    p.x >= a.left - 2 &&
    p.x <= a.left + Math.max(a.width, 12) + 2 &&
    p.y >= a.top - 2 &&
    p.y <= a.bottom + 2;

  /** Best available anchor: snapshot from the click → live rect → click point. */
  const resolveAnchor = (): AnchorRect | undefined => {
    if (anchorRect) return anchorRect;
    const live = liveAnchorRect();
    // An empty rect is not used when a click point exists (see captureAnchor…).
    if (live && !(pointerPoint && isEmptyRect(live))) return live;
    if (pointerPoint)
      return { top: pointerPoint.y, bottom: pointerPoint.y, left: pointerPoint.x, width: 0 };
    return undefined;
  };

  // The panel z-index derives from the current elevation: at the app root it
  // is POPOVER_DELTA, inside a Modal — base + POPOVER_DELTA (above the modal).
  const panelZ = useElevation() + POPOVER_DELTA;

  const [pos, setPos] = createSignal({
    placement: DEFAULT_PLACEMENT,
    top: 0,
    left: 0,
    width: 0,
  });

  // Position not computed yet — the panel stays hidden until the first
  // reposition, avoiding a flash at (0,0) at open time.
  const [positioned, setPositioned] = createSignal(false);

  // --- Popover nesting ------------------------------------------------------
  // This popover's panel goes into a Portal and becomes a DOM sibling of the
  // parent's panel. So that clicks on our panel (and on panels of any depth
  // below us) don't close ancestors, we register our elements in their
  // contexts. See PopoverInsideContextValue.
  const parentInside = useContext(PopoverInsideContext);
  const [descendantInside, setDescendantInside] = createSignal<
    Accessor<Element | undefined>[]
  >([]);

  const registerInside: PopoverInsideContextValue["registerInside"] = (el) => {
    setDescendantInside((prev) => [...prev, el]);
    // Cascade upward: forward the registration to all ancestors — a click in
    // a child popover's panel must close none of the ancestors in the chain.
    const unregisterParent = parentInside?.registerInside(el);
    return () => {
      setDescendantInside((prev) => prev.filter((item) => item !== el));
      unregisterParent?.();
    };
  };

  const insideContextValue: PopoverInsideContextValue = { registerInside };

  /**
   * Side selection with free space and panel height in mind. When neither
   * side fits, the roomier side wins (the panel is then clamped into the
   * viewport, see `reposition`).
   */
  const resolvePlacement = (
    requested: PopoverPlacement,
    panelHeight: number,
    activatorTop: number,
    activatorBottom: number,
  ): PopoverPlacement => {
    const spaceBelow = window.innerHeight - activatorBottom - GAP;
    const spaceAbove = activatorTop - GAP;
    const fitsBelow = spaceBelow >= panelHeight;
    const fitsAbove = spaceAbove >= panelHeight;
    if (fitsBelow && fitsAbove) return requested;
    if (fitsBelow) return "bottom";
    if (fitsAbove) return "top";
    return spaceBelow >= spaceAbove ? "bottom" : "top";
  };

  const reposition = () => {
    if (!panelEl) return;
    const a = resolveAnchor();
    if (!a) {
      // Nothing to anchor to: activator detached, rect degenerate and no
      // click point. A panel "nowhere" is not kept — close.
      if (isOpen()) setOpen(false);
      return;
    }
    const panelWidth = panelEl.offsetWidth;
    const panelHeight = panelEl.offsetHeight;
    lastPanelHeight = panelHeight;
    lastPanelWidth = panelWidth;
    const placement = resolvePlacement(
      props.placement ?? DEFAULT_PLACEMENT,
      panelHeight,
      a.top,
      a.bottom,
    );
    // Position "flush against the activator", then a hard clamp on both
    // axes: the panel never hides behind window edges — even when the
    // activator sits at the very edge or the panel is taller than the space
    // on the chosen side.
    const maxTop = Math.max(GAP, window.innerHeight - panelHeight - GAP);
    const top = Math.max(
      GAP,
      Math.min(
        placement === "bottom" ? a.bottom + GAP : a.top - GAP - panelHeight,
        maxTop,
      ),
    );
    // Horizontal clamp: the panel must not stick out of the viewport. When
    // wider than the screen it is pressed to the left edge (GAP).
    const maxLeft = window.innerWidth - panelWidth - GAP;
    const left = Math.max(GAP, Math.min(a.left, maxLeft));
    setPos({ placement, top, left, width: a.width });
    setPositioned(true);
  };

  createEffect(() => {
    if (!isOpen()) {
      setPositioned(false);
      anchorRect = undefined;
      pointerPoint = undefined;
      return;
    }
    reposition();

    /**
     * Re-anchoring to the actual activator position — only on explicit
     * events (scroll/resize): refresh the anchor from the live rect and
     * recompute the panel. When the activator leaves the viewport entirely or
     * is detached there is nothing to anchor to — close.
     */
    const followActivator = () => {
      const live = liveAnchorRect();
      if (!live) {
        setOpen(false);
        return;
      }
      if (live.bottom <= 0 || live.top >= window.innerHeight) {
        setOpen(false);
        return;
      }
      anchorRect = live;
      reposition();
    };

    // Escape closes only the topmost overlay: a popover inside a modal must
    // not close the modal on the same key press (overlayEscape stack).
    onCleanup(registerOverlayEscape(() => setOpen(false)));

    // Following the activator (scroll/resize). Disabled by followScroll=false:
    // for a known-stationary activator (sticky header) the position captured
    // at open is the truth that neighboring scroll must not spoil (and WebKit
    // may hand out a stale rect and drag/close the panel).
    if (props.followScroll !== false) {
      // Scroll: do NOT read the rect synchronously in the handler. During a
      // scroll (especially in WebKit) sticky elements update their position
      // asynchronously, and a synchronous read yields an intermediate
      // "un-stuck" position — the panel would drift by the scroll delta away
      // from the activator. Recompute on the next frame (layout has settled)
      // and, as a control pass, one more frame later. If the activator did
      // not move (sticky header) → the panel stays put.
      let raf = 0;
      const scheduleFollow = () => {
        if (raf) return;
        raf = requestAnimationFrame(() => {
          raf = requestAnimationFrame(() => {
            raf = 0;
            followActivator();
          });
          followActivator();
        });
      };
      onCleanup(() => {
        if (raf) cancelAnimationFrame(raf);
      });

      // capture: true — catch scrolls in any nested container.
      makeEventListener(window, "scroll", scheduleFollow, true);
      // Duplicate listeners on the explicit scrollable ancestors of the
      // activator — don't rely on the window capture phase alone
      // (TableView/WKWebView).
      if (activatorEl) {
        for (const scroller of scrollableAncestors(activatorEl)) {
          makeEventListener(scroller, "scroll", scheduleFollow);
        }
      }
      makeEventListener(window, "resize", followActivator);
    }

    // Recompute when the content changes the panel size (e.g. items loading
    // in). The baseline is updated here and compared against itself
    // (border-box via offsetWidth/Height) — otherwise contentRect (without
    // border) and offsetHeight (with border) always disagree → an infinite
    // ResizeObserver loop.
    if (panelEl)
      createResizeObserver(panelEl, () => {
        if (!panelEl) return;
        const w = panelEl.offsetWidth;
        const h = panelEl.offsetHeight;
        if (
          Math.abs(w - lastPanelWidth) < 1 &&
          Math.abs(h - lastPanelHeight) < 1
        )
          return;
        lastPanelWidth = w;
        lastPanelHeight = h;
        reposition();
      });
  });

  // Clicks outside the activator and the panel close the popover. The ready
  // `useClickOutside` from `@elysiumorg/solid-use` freezes its element list
  // at setup and can't handle a dynamic set — which we need: descendant
  // panels register into `descendantInside` after those popovers open. Hence
  // an explicit check, with the source element read from the signal straight
  // in the handler (event handlers create no reactivity — the value is always
  // fresh, without re-creating listeners).
  createEffect(() => {
    const onPointer = (e: MouseEvent | TouchEvent) => {
      const target = e.target;
      if (!target || typeof (target as Node).contains !== "function") return;
      const inside: (Element | undefined)[] = [activatorEl, panelEl];
      for (const accessor of descendantInside()) {
        const el = accessor();
        if (el) inside.push(el);
      }
      const isInside = inside.some((el) => !!el && el.contains(target as Node));
      if (!isInside) setOpen(false);
    };
    document.addEventListener("mousedown", onPointer);
    document.addEventListener("touchstart", onPointer);
    onCleanup(() => {
      document.removeEventListener("mousedown", onPointer);
      document.removeEventListener("touchstart", onPointer);
    });
  });

  const setActivatorEl = (el: Element) => {
    activatorEl = el;
  };

  /** Captures the anchor (rect at call time + click point) and opens the panel. */
  const captureAnchorAndOpen = (point: PointerPoint | undefined, open: boolean) => {
    if (!open) {
      setOpen(false);
      return;
    }
    pointerPoint = point;
    const live = liveAnchorRect();
    if (!live || (point && (isEmptyRect(live) || !pointOnRect(live, point)))) {
      // Rect unavailable or knowingly false. The click on the button
      // happened — so the click point is guaranteed near it; but the rect can
      // be junk: WebKit gives sticky elements their pre-scroll position after
      // scrolling (negative, with sane sizes — degeneracy checks miss it).
      // Anchor to the click point — the panel opens right at the button.
      anchorRect = point
        ? { top: point.y, bottom: point.y, left: point.x, width: 0 }
        : undefined;
    } else {
      anchorRect = live;
    }
    setOpen(true);
  };

  const api: PopoverActivatorApi = {
    isOpen,
    show: (point) => captureAnchorAndOpen(point, true),
    hide: () => setOpen(false),
    toggle: (point) => captureAnchorAndOpen(point, !isOpen()),
    ref: setActivatorEl,
    activatorProps: {
      ref: setActivatorEl,
      // detail > 0 — mouse clicks only; a keyboard click has (0,0) coords.
      onClick: (e) => {
        const point = e.detail > 0 ? { x: e.clientX, y: e.clientY } : undefined;
        captureAnchorAndOpen(point, !isOpen());
      },
    },
  };

  return (
    <>
      {props.activator(api)}
      {/* The portal lives forever, so the exit animation gets to finish. */}
      <Portal mount={document.body}>
        <TransitionScale mode="outin">
          <Show when={isOpen()}>
            <div
              ref={(el) => {
                panelEl = el;
                // Position synchronously the moment the panel appears in the
                // DOM — before the first paint, so there is no (0,0) flash.
                reposition();
                // Tell popover ancestors this panel is "theirs": clicks on it
                // must not close them. The accessor reads the fresh panelEl
                // on every call, so it stays correct after a remount.
                const unregister = parentInside?.registerInside(() => panelEl);
                onCleanup(() => unregister?.());
              }}
              classList={{
                "dropdown-content bg-base-100 rounded-box shadow-xl border border-base-300": true,
                [props.contentClass ?? ""]: !!props.contentClass,
                "origin-top": pos().placement === "bottom",
                "origin-bottom": pos().placement === "top",
              }}
              style={{
                position: "fixed",
                "z-index": panelZ,
                left: `${pos().left}px`,
                top: `${pos().top}px`,
                visibility: positioned() ? "visible" : "hidden",
                // In dropdown mode the panel mirrors the activator width;
                // otherwise the content dictates the width (min/max-width via
                // contentClass).
                ...(props.matchActivatorWidth
                  ? { width: `${pos().width}px` }
                  : {}),
              }}
            >
              {/* Raise the base for the panel content — for nested overlays
                  (a tooltip inside a menu item etc.). */}
              <ElevationProvider delta={POPOVER_DELTA}>
                {/* Provide the nested-panels registration context: popovers
                    opened from `children` will see it and register their
                    panels — otherwise a click on them would close this one. */}
                <PopoverInsideContext.Provider value={insideContextValue}>
                  {props.children}
                </PopoverInsideContext.Provider>
              </ElevationProvider>
            </div>
          </Show>
        </TransitionScale>
      </Portal>
    </>
  );
}
