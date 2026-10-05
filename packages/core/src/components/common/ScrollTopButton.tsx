import type { Component } from "solid-js";
import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { makeEventListener } from "@solid-primitives/event-listener";
import { createResizeObserver } from "@solid-primitives/resize-observer";
import { Transition } from "solid-transition-group";
import { TbOutlineArrowUp } from "solid-icons/tb";

import { useScrollZone } from "@/contexts/scrollZone";

/** A hair of scroll away from the very top shouldn't flash the button. */
const SHOW_AFTER_PX = 300;

/** The default corner: above the endpoints page's centered bottom action
 *  bar on narrow screens (the same clearance the toasts keep), flush in
 *  the corner everywhere else. */
const DEFAULT_CORNER = "fixed bottom-24 right-4 z-40 sm:bottom-6 sm:right-6";

/**
 * The global back-to-top corner button. The mobile AppLayout mounts it
 * bare and it follows the document window; every desktop DesktopPage
 * mounts it inside its ScrollZoneProvider (with its own corner class,
 * clear of the status bar) and it follows the content zone instead. The
 * jump is instant, like the grid's reveal scrolls: one range change — one
 * window load, no intermediate chunk fetches on the way up.
 */
export const ScrollTopButton: Component<{ class?: string }> = (props) => {
  const zone = useScrollZone();
  const [visible, setVisible] = createSignal(false);

  // rAF-batched like VirtualWindowGrid's measure — one read per scroll frame
  let frame = 0;
  const measure = () => {
    frame = 0;
    setVisible((zone()?.scrollTop ?? window.scrollY) > SHOW_AFTER_PX);
  };
  const scheduleMeasure = () => {
    if (!frame) {
      frame = requestAnimationFrame(measure);
    }
  };
  const jumpToTop = () => {
    const el = zone();
    if (el) {
      el.scrollTo({ top: 0 });
    } else {
      window.scrollTo({ top: 0 });
    }
  };

  onMount(() => {
    measure();
    const el = zone();
    if (el) {
      makeEventListener(el, "scroll", scheduleMeasure, { passive: true });
    } else {
      makeEventListener(window, "scroll", scheduleMeasure, { passive: true });
    }
  });
  // route swaps change the page height without any scroll event
  createResizeObserver(document.body, scheduleMeasure);
  onCleanup(() => {
    if (frame) {
      cancelAnimationFrame(frame);
    }
  });

  return (
    <div class={props.class ?? DEFAULT_CORNER}>
      <Transition
        enterClass="translate-y-2 opacity-0"
        enterActiveClass="transition duration-200 ease-out"
        enterToClass="translate-y-0 opacity-100"
        exitClass="translate-y-0 opacity-100"
        exitActiveClass="transition duration-150 ease-in"
        exitToClass="translate-y-2 opacity-0"
      >
        <Show when={visible()}>
          {/* the wrapper carries the transition classes so they never fight
              the button's own `transition-colors` hover */}
          <div class="grid place-items-center">
            <button
              type="button"
              class="flex size-10 items-center justify-center rounded-full bg-base-content/10 text-base-content/70 shadow-lg backdrop-blur-md transition-colors hover:bg-base-content/20 hover:text-base-content"
              title="Back to top"
              aria-label="Back to top"
              onClick={jumpToTop}
            >
              <TbOutlineArrowUp size={18} />
            </button>
          </div>
        </Show>
      </Transition>
    </div>
  );
};
