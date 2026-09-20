import type { Component } from "solid-js";
import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { makeEventListener } from "@solid-primitives/event-listener";
import { createResizeObserver } from "@solid-primitives/resize-observer";
import { Transition } from "solid-transition-group";
import { TbOutlineArrowUp } from "solid-icons/tb";

/** A hair of scroll away from the very top shouldn't flash the button. */
const SHOW_AFTER_PX = 300;

/**
 * The global back-to-top corner button (mounted in AppLayout). The jump is
 * instant, like the grid's reveal scrolls: one range change — one window
 * load, no intermediate chunk fetches on the way up. On narrow screens it
 * sits above the endpoints page's centered bottom action bar (same
 * clearance the toasts keep), in the corner everywhere else.
 */
export const ScrollTopButton: Component = () => {
  const [visible, setVisible] = createSignal(false);

  // rAF-batched like VirtualWindowGrid's measure — one read per scroll frame
  let frame = 0;
  const measure = () => {
    frame = 0;
    setVisible(window.scrollY > SHOW_AFTER_PX);
  };
  const scheduleMeasure = () => {
    if (!frame) {
      frame = requestAnimationFrame(measure);
    }
  };

  onMount(() => {
    measure();
    makeEventListener(window, "scroll", scheduleMeasure, { passive: true });
  });
  // route swaps change the page height without any scroll event
  createResizeObserver(document.body, scheduleMeasure);
  onCleanup(() => {
    if (frame) {
      cancelAnimationFrame(frame);
    }
  });

  return (
    <div class="fixed bottom-24 right-4 z-40 sm:bottom-6 sm:right-6">
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
              onClick={() => window.scrollTo({ top: 0 })}
            >
              <TbOutlineArrowUp size={18} />
            </button>
          </div>
        </Show>
      </Transition>
    </div>
  );
};
