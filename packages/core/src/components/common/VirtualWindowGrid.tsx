import { makeEventListener } from "@solid-primitives/event-listener";
import { createResizeObserver, createWindowSize } from "@solid-primitives/resize-observer";
import {
  For,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
  type JSXElement,
} from "solid-js";

/** Viewport-width step: at `minWidth` and above the grid has `columns` cells per row. */
export interface GridBreakpoint {
  minWidth: number;
  columns: number;
}

export interface VirtualWindowGridProps<T> {
  /** Flat cell list; laid out into rows of the current column count. */
  items: T[];
  /** Fixed cell height in px — every rendered cell must actually be this tall. */
  itemHeight: number;
  /** Cell gap (both axes) in px. */
  gap?: number;
  /** Responsive column ladder (ascending); below the first step the grid is 1 column. */
  breakpoints: GridBreakpoint[];
  /** Extra rows kept rendered above/below the viewport, to hide the chunk loading. */
  overscanRows?: number;
  /** Fires with the visible cell range (half-open, cell indices) whenever it
   *  changes — the hook for windowed data loading. */
  onRangeChange?: (start: number, end: number) => void;
  /** Receives the imperative API (scroll-to-cell) once, on mount. */
  onApi?: (api: VirtualWindowGridApi) => void;
  /** Scrollable element driving the window (the desktop layout's content
   *  zone). Omitted — the document window scrolls (the mobile layout). */
  scroller?: () => HTMLElement | null;
  children: (item: T) => JSXElement;
}

/** Imperative surface of a mounted grid. */
export interface VirtualWindowGridApi {
  /** Scrolls the window so the cell at `index` sits fully visible below
   *  `topOffset` px reserved for whatever floats above the grid (a sticky
   *  header). Instant, not smooth: one jump = exactly one window load. */
  scrollToIndex: (index: number, topOffset?: number) => void;
}

/** Window-scrolled virtualized responsive grid: only the rows intersecting the
 *  viewport (plus an overscan margin) are in the DOM, positioned inside a
 *  full-height spacer via translateY. Cell height is fixed and column count
 *  derives from the window width, so both scroll math and chunking survive
 *  window resizes. */
export const VirtualWindowGrid = <T,>(props: VirtualWindowGridProps<T>) => {
  const windowSize = createWindowSize();
  const scroller = () => props.scroller?.() ?? null;

  let container: HTMLDivElement | undefined;
  const [scrollTop, setScrollTop] = createSignal(0);
  const [containerHeight, setContainerHeight] = createSignal(window.innerHeight);
  const [gridTop, setGridTop] = createSignal(0);

  const columns = createMemo(() => {
    let count = 1;
    for (const step of props.breakpoints) {
      if (windowSize.width >= step.minWidth) {
        count = Math.max(count, step.columns);
      }
    }
    return count;
  });

  // Reading the container rect together with the scroll position gives the
  // grid's top in scroll space — document space for the window, content
  // space for a scroll zone — immune to whatever happens above it (sticky
  // header wrapping, alerts appearing). rAF-batched to one read per frame.
  let frame = 0;
  const measure = () => {
    frame = 0;
    const zone = scroller();
    if (zone) {
      setScrollTop(zone.scrollTop);
      setContainerHeight(zone.clientHeight);
      if (container) {
        setGridTop(
          container.getBoundingClientRect().top -
            zone.getBoundingClientRect().top +
            zone.scrollTop,
        );
      }
      return;
    }
    setScrollTop(window.scrollY);
    setContainerHeight(window.innerHeight);
    if (container) {
      setGridTop(container.getBoundingClientRect().top + window.scrollY);
    }
  };
  const scheduleMeasure = () => {
    if (!frame) {
      frame = requestAnimationFrame(measure);
    }
  };

  onMount(() => {
    measure();
    props.onApi?.({ scrollToIndex });
    makeEventListener(window, "scroll", scheduleMeasure, { passive: true });
    makeEventListener(window, "resize", scheduleMeasure);
    const zone = scroller();
    if (zone) {
      makeEventListener(zone, "scroll", scheduleMeasure, { passive: true });
    }
  });
  // a zone element that shows up late (or gets swapped) must be re-measured
  createEffect(() => {
    scroller();
    scheduleMeasure();
  });
  // Layout shifts above the grid (an alert toggling, late fonts) move it
  // without any scroll/resize — watching the page height catches those too.
  createResizeObserver(document.body, scheduleMeasure);
  onCleanup(() => {
    if (frame) {
      cancelAnimationFrame(frame);
    }
  });

  const rowStep = () => props.itemHeight + (props.gap ?? 0);
  const rowCount = createMemo(() => Math.ceil(props.items.length / columns()));
  const totalHeight = createMemo(() => Math.max(0, rowCount() * rowStep() - (props.gap ?? 0)));

  const scrollToIndex = (index: number, topOffset = 0) => {
    const row = Math.floor(Math.max(0, index) / columns());
    const top = Math.max(0, gridTop() + row * rowStep() - topOffset);
    const zone = scroller();
    if (zone) {
      zone.scrollTo({ top });
    } else {
      window.scrollTo({ top });
    }
  };

  /** Visible row range [start, end) in row units. */
  const range = createMemo<[number, number]>(() => {
    const overscan = props.overscanRows ?? 0;
    const step = rowStep();
    const top = gridTop();
    const rows = rowCount();
    const viewport = () => (scroller() ? containerHeight() : windowSize.height);
    const first = Math.floor((scrollTop() - top) / step);
    const last = Math.ceil((scrollTop() + viewport() - top) / step);
    const start = Math.min(Math.max(0, first - overscan), rows);
    const end = Math.min(rows, Math.max(0, last) + overscan);
    return [start, Math.max(start, end)];
  });

  const visibleCells = createMemo(() => {
    const cols = columns();
    const [start, end] = range();
    return props.items.slice(start * cols, end * cols);
  });

  createEffect(() => {
    const onRangeChange = props.onRangeChange;
    if (!onRangeChange) {
      return;
    }
    const cols = columns();
    const [start, end] = range();
    onRangeChange(start * cols, end * cols);
  });

  return (
    <div ref={container} class="relative w-full" style={{ height: `${totalHeight()}px` }}>
      <div
        class="absolute inset-x-0 top-0 grid"
        style={{
          "grid-template-columns": `repeat(${columns()}, minmax(0, 1fr))`,
          "column-gap": `${props.gap ?? 0}px`,
          "row-gap": `${props.gap ?? 0}px`,
          transform: `translateY(${range()[0] * rowStep()}px)`,
        }}
      >
        <For each={visibleCells()}>{(cell) => props.children(cell)}</For>
      </div>
    </div>
  );
};
