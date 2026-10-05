import {
  ElevationProvider,
  MODAL_DELTA,
  useElevation,
} from "@/contexts/elevation";
import { TransitionFade } from "@/components/common/transitions/TransitionFade";
import { TransitionScale } from "@/components/common/transitions/TransitionScale";
import { TransitionSheet } from "@/components/common/transitions/TransitionSheet";
import { Button } from "@/components/common/daisy-ui/Button";
import { registerOverlayEscape } from "@/utils/overlayEscape";
import clsx from "clsx";
import { TbOutlineX } from "solid-icons/tb";
import { makeEventListener } from "@solid-primitives/event-listener";
import {
  createEffect,
  createSignal,
  onCleanup,
  Show,
  type Accessor,
  type Component,
  type JSXElement,
  type ParentComponent,
} from "solid-js";
import { Dynamic, Portal } from "solid-js/web";
import { createModel } from "@/hooks/createModel";

export type ActivatorProps = {
  opened: Accessor<boolean>;
  setOpened: (value: boolean) => void;
};

/** Tailwind `sm` breakpoint — the line between the mobile bottom sheet and
 *  the centered desktop dialog (matches the previous `sm:modal-middle`). */
const DESKTOP_MEDIA = "(min-width: 640px)";

/**
 * A modal dialog above the whole application.
 *
 * Implemented via `<Portal>` into `document.body` + `position: fixed` +
 * `z-index`, without a native `<dialog>`/`showModal()`: the latter puts the
 * dialog into the browser top layer, which ignores any `z-index` and
 * therefore covers any other portal overlay (e.g. a `Popover` inside the
 * modal). Instead the elevation context (`MODAL_DELTA`) is used — nested
 * popovers, adding their delta, reliably end up above the modal.
 *
 * Native-dialog behaviors are restored: backdrop click and Escape close,
 * background scrolling is blocked while open. Responsive: below the Tailwind
 * `sm` breakpoint the panel is a bottom sheet (slides up from the screen
 * bottom, full width, safe-area padding for the iOS home indicator); from
 * `sm` up it is the classic centered, scaled dialog. The panel carries no
 * width classes of its own — pass `max-w-*` via `class` (applied only on
 * desktop; the bottom sheet is always full width). A focus trap is
 * deliberately not implemented.
 */
export const Modal: ParentComponent<{
  opened?: Accessor<boolean>;
  setOpened?: (value: boolean) => void;
  activator?: Component<ActivatorProps>;
  title?: string | JSXElement;
  actions?: JSXElement;
  class?: string;
}> = (props) => {
  const [opened, setOpened] = createModel<boolean>([props.opened, props.setOpened]);

  const base = useElevation();
  const backdropZ = base + MODAL_DELTA;
  const panelZ = base + MODAL_DELTA + 1;

  // Mobile-first: below `sm` the panel docks to the bottom as a sheet.
  const desktopMedia = window.matchMedia(DESKTOP_MEDIA);
  const [isDesktop, setIsDesktop] = createSignal(desktopMedia.matches);
  makeEventListener(desktopMedia, "change", (event) =>
    setIsDesktop(event.matches),
  );

  // Background scrolling is blocked while the modal is open. The previous
  // value is restored instead of resetting to "" so someone else's setting
  // is not clobbered.
  createEffect(() => {
    if (!opened()) return;
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    onCleanup(() => {
      document.body.style.overflow = prev;
    });
  });

  // Escape closes only the topmost overlay: with a popover open inside, the
  // first press closes the popover and only the next one the modal
  // (overlayEscape stack).
  createEffect(() => {
    if (!opened()) return;
    onCleanup(registerOverlayEscape(() => setOpened(false)));
  });

  const Activator = () =>
    props.activator?.({
      opened,
      setOpened: (value: boolean) => setOpened(value),
    });

  const panelClasses = () =>
    clsx(
      // Panel styling deliberately avoids the DaisyUI `.modal-box` class: it
      // hard-codes opacity:0 / scale:95% and only resets them under a
      // `.modal[open]` / `.modal-open` parent. We moved away from
      // `<dialog>/showModal`, so there is no parent — the class would leave
      // the panel invisible forever. The utilities mirror `.modal-box`
      // visuals and match the Popover style.
      "bg-base-100 shadow-xl border border-base-300",
      "flex flex-col overflow-hidden",
      "fixed max-h-[90vh]",
      // Width classes (`max-w-*`) are for the centered desktop dialog only —
      // on the bottom sheet they would shrink it away from full width.
      isDesktop() && props.class,
    );

  return (
    <>
      <Show when={props.activator}>
        <Activator />
      </Show>
      {/* The portal lives forever so TransitionFade/TransitionScale get to
          finish their exit animation after closing. If the Portal were inside
          the Show, it would unmount immediately together with its content. */}
      <Portal mount={document.body}>
        {/* Raise the base for the whole modal subtree: the backdrop, the
            panel and nested overlays (Popover etc.) compute their z-index
            from base + MODAL_DELTA. */}
        <ElevationProvider delta={MODAL_DELTA}>
          {/* Backdrop — fade in/out. The Transition outside the Show catches
              the child's appear/disappear and applies enter/exit classes. */}
          <TransitionFade>
            <Show when={opened()}>
              <div
                class="fixed inset-0 bg-black/40"
                style={{ "z-index": backdropZ }}
                onClick={() => setOpened(false)}
              />
            </Show>
          </TransitionFade>
          {/* Panel — a sliding bottom sheet on mobile, scale in/out on
              desktop. Same Transition-outside-Show scheme as the backdrop. */}
          <Dynamic
            component={isDesktop() ? TransitionScale : TransitionSheet}
          >
            <Show when={opened()}>
              <div
                class={panelClasses()}
                style={{ "z-index": panelZ }}
                role="dialog"
                aria-modal="true"
                classList={{
                  "inset-x-0 bottom-0 rounded-t-2xl border-b-0 pb-[env(safe-area-inset-bottom)]":
                    !isDesktop(),
                  "left-1/2 top-1/2 w-[91.666667%] -translate-x-1/2 -translate-y-1/2 rounded-box":
                    isDesktop(),
                }}
              >
                {/* Header: title on the left, ✕ on the right — always one line. */}
                <div class="flex items-center justify-between gap-4 border-b border-base-300 px-6 py-4 shrink-0">
                  <Show when={props.title}>
                    <h2 class="card-title m-0 min-w-0">{props.title}</h2>
                  </Show>
                  <Button
                    ghost
                    circle
                    size="sm"
                    aria-label="Close"
                    onClick={() => setOpened(false)}
                  >
                    <TbOutlineX size={18} />
                  </Button>
                </div>
                {/* Body: scrolls, never carries the header/footer away. */}
                <div class="flex-1 overflow-y-auto p-6 min-h-0">
                  {props.children}
                </div>
                {/* Footer: only when actions are passed; pinned to the bottom. */}
                <Show when={props.actions}>
                  <div class="flex justify-end gap-2 border-t border-base-300 px-6 py-4 shrink-0">
                    {props.actions}
                  </div>
                </Show>
              </div>
            </Show>
          </Dynamic>
        </ElevationProvider>
      </Portal>
    </>
  );
};
